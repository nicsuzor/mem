<script lang="ts">
    import { onDestroy } from "svelte";
    import cytoscape from "cytoscape";
    import {
        forceSimulation,
        forceLink,
        forceManyBody,
        forceCollide,
        forceX,
        forceY,
    } from "d3-force";
    import { graphData, preparedGraphData, graphStructureKey, preparedStructureKey } from "../../stores/graph";
    import { filters, type VisibilityState } from "../../stores/filters";
    import { selection, toggleSelection } from "../../stores/selection";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";
    import { INCOMPLETE_STATUSES, PRIORITY_BORDERS } from "../../data/constants";
    import { projectColor } from "../../data/projectUtils";

    let containerEl: HTMLDivElement;
    let cy: cytoscape.Core | null = null;
    // Persistent force simulation — kept running after buildGraph so that
    // dragging a node (pinning it via fx/fy) lets the surrounding network
    // react organically. Stopped on destroy / structural rebuild.
    let sim: any = null;
    let simNodes: Array<{ id: string; x: number; y: number; fx: number | null; fy: number | null; anchorX: number; anchorY: number; radius: number }> = [];

    export let running = false;
    // Context stations (nodes on no route) are noise for the "routes to
    // destinations" question — hidden by default. Toggle via the `Show context`
    // control; when enabled, only the top-N by downstream_weight render.
    export let showContext = false;

    // Project-type nodes are structural containers. They are not hidden
    // outright — when they appear on a target's ancestor chain we want the
    // connector to be visible — but we render them as muted backbone stops.
    const HIDDEN_TYPES = new Set<string>();
    const BACKBONE_TYPES = new Set(['project', 'epic', 'goal']);
    const CONTAINER_TYPES = new Set(['goal', 'epic', 'project']);
    const EPIC_TYPES = new Set(['epic', 'goal', 'project']);
    const DEFAULT_PROJECT_COLOR = 'hsl(220, 12%, 46%)';

    // Layout constants
    const ROW_HEIGHT = 80;
    const CONTEXT_STRIP_Y = 140;
    const CONTEXT_CAP = 200;           // hard cap on rendered context stations
    const TERMINAL_ROW_GAP = 56;       // vertical spacing between terminal rows
    const TERMINAL_PER_ROW = 3;        // target number of terminals per row
    const ANCESTOR_HOP_CAP = 1;        // walk at most N parent hops above a target
    const SUBTREE_DEPTH_CAP = 5;       // cap descendant-from-ancestor BFS depth
    const DESCENDANT_DEPTH_CAP = 6;    // cap descendant-from-target BFS depth
    const BLOCKER_DEPTH_CAP = 6;       // cap transitive blocker walk
    const GRID_X = 36;
    const GRID_Y = 32;

    // Hover tooltip state
    let tooltip: {
        x: number;
        y: number;
        title: string;
        status: string;
        priority: number;
        project: string | null;
        destinations: string[];
    } | null = null;

    // ─── Helpers ────────────────────────────────────────────────────────────

    function priorityVisibility(priority: number | undefined): VisibilityState {
        if (priority === 0) return $filters.priority0;
        if (priority === 1) return $filters.priority1;
        if (priority === 2) return $filters.priority2;
        if (priority === 3) return $filters.priority3;
        return $filters.priority4;
    }

    function isIncomplete(node: GraphNode): boolean {
        return INCOMPLETE_STATUSES.has(node.status);
    }

    function getNodeRole(node: GraphNode): 'epic' | 'task' {
        return EPIC_TYPES.has((node.type || '').toLowerCase()) ? 'epic' : 'task';
    }

    function getEdgeRole(edgeType: string): 'parent' | 'dependency' | 'reference' {
        if (edgeType === 'parent') return 'parent';
        if (edgeType === 'depends_on' || edgeType === 'soft_depends_on') return 'dependency';
        return 'reference';
    }

    function getProjectLineColor(project: string | null | undefined): string {
        return project ? projectColor(project) : DEFAULT_PROJECT_COLOR;
    }

    function idHash(id: string): number {
        let h = 2166136261;
        for (let i = 0; i < id.length; i++) {
            h ^= id.charCodeAt(i);
            h = Math.imul(h, 16777619);
        }
        return Math.abs(h);
    }

    // ─── Destination / Route / Depth computation ────────────────────────────

    interface RouteData {
        destinations: GraphNode[];
        routes: Map<string, Set<string>>;   // nodeId -> set of destination ids
        depth: Map<string, number>;         // nodeId -> min distance to serving destination
        destIndex: Map<string, number>;     // destId -> ordinal position
    }

    // Directed adjacency for route discovery.
    //   parentDown[p]  → children p→c  (after prepareGraphData flip)
    //   parentUp[c]    → p             (child's parent)
    //   blockersOut[b] → set of blockers (follows depends_on + soft_depends_on)
    interface DirectedAdjacency {
        parentDown: Map<string, Set<string>>;
        parentUp: Map<string, string>;
        blockersOut: Map<string, Set<string>>;
    }

    function buildDirectedAdjacency(nodes: GraphNode[], edges: GraphEdge[]): DirectedAdjacency {
        const parentDown = new Map<string, Set<string>>();
        const parentUp = new Map<string, string>();
        const blockersOut = new Map<string, Set<string>>();
        for (const n of nodes) {
            parentDown.set(n.id, new Set());
            blockersOut.set(n.id, new Set());
        }
        for (const e of edges) {
            const src = typeof e.source === 'object' ? e.source.id : e.source;
            const tgt = typeof e.target === 'object' ? e.target.id : e.target;
            if (!parentDown.has(src) || !parentDown.has(tgt)) continue;
            if (src === tgt) continue;
            if (e.type === 'parent') {
                parentDown.get(src)!.add(tgt);
                parentUp.set(tgt, src);
            } else if (e.type === 'depends_on' || e.type === 'soft_depends_on') {
                blockersOut.get(src)!.add(tgt);
            }
        }
        return { parentDown, parentUp, blockersOut };
    }

    // Terminals are explicitly target-type nodes. Priority alone doesn't
    // qualify — the user sets targets deliberately.
    function computeDestinations(nodes: GraphNode[]): GraphNode[] {
        return nodes
            .filter(n => {
                if (!isIncomplete(n)) return false;
                if ((n.type || '').toLowerCase() !== 'target') return false;
                return true;
            })
            .sort((a, b) => {
                if (a.priority !== b.priority) return a.priority - b.priority;
                const pa = (a.project || '').toLowerCase();
                const pb = (b.project || '').toLowerCase();
                if (pa !== pb) return pa.localeCompare(pb);
                return a.label.localeCompare(b.label);
            });
    }

    // For each terminal T, the "line" is the set of stations meaningfully on
    // the way to T. We include:
    //   (a) descendants of T via parent (sub-tasks, grand-sub-tasks),
    //   (b) the immediate parent ancestor and its subtree (siblings and cousins
    //       that compose the epic the target sits inside),
    //   (c) transitive blockers (depends_on / soft_depends_on) of every
    //       already-collected node.
    // Walks stop at other terminals so distinct lines don't bleed together.
    // Completed nodes stay on the route — they render desaturated, but the
    // user can still see the full scope of what the target covers.
    function computeRouteData(nodes: GraphNode[], edges: GraphEdge[]): RouteData {
        const destinations = computeDestinations(nodes);
        const destIndex = new Map<string, number>();
        destinations.forEach((d, i) => destIndex.set(d.id, i));

        const routes = new Map<string, Set<string>>();
        const depth = new Map<string, number>();
        for (const n of nodes) routes.set(n.id, new Set());

        if (destinations.length === 0) {
            return { destinations, routes, depth, destIndex };
        }

        const { parentDown, parentUp, blockersOut } = buildDirectedAdjacency(nodes, edges);
        const destSet = new Set(destinations.map(d => d.id));

        for (const dest of destinations) {
            const stopAt = new Set<string>();
            for (const other of destSet) if (other !== dest.id) stopAt.add(other);
            const perDest = new Map<string, number>([[dest.id, 0]]);

            // (a) descendants of the target itself
            const descQueue: Array<{ id: string; d: number }> = [{ id: dest.id, d: 0 }];
            while (descQueue.length) {
                const { id, d } = descQueue.shift()!;
                if (d >= DESCENDANT_DEPTH_CAP) continue;
                const kids = parentDown.get(id);
                if (!kids) continue;
                for (const kid of kids) {
                    if (perDest.has(kid)) continue;
                    if (stopAt.has(kid)) continue;
                    perDest.set(kid, d + 1);
                    descQueue.push({ id: kid, d: d + 1 });
                }
            }

            // (b) walk up at most ANCESTOR_HOP_CAP levels; at each level, include
            // the ancestor and its subtree (siblings + cousins) capped by
            // SUBTREE_DEPTH_CAP.
            let cur = dest.id;
            for (let hop = 0; hop < ANCESTOR_HOP_CAP; hop++) {
                const parent = parentUp.get(cur);
                if (!parent) break;
                if (destSet.has(parent)) break;
                const parentD = (perDest.get(cur) ?? 0) + 1;
                if (!perDest.has(parent)) perDest.set(parent, parentD);
                // Fan the ancestor's subtree down, minus other-terminal branches
                const q: Array<{ id: string; d: number }> = [{ id: parent, d: parentD }];
                while (q.length) {
                    const { id, d } = q.shift()!;
                    if (d - parentD >= SUBTREE_DEPTH_CAP) continue;
                    const kids = parentDown.get(id);
                    if (!kids) continue;
                    for (const kid of kids) {
                        if (perDest.has(kid)) continue;
                        if (stopAt.has(kid)) continue;
                        perDest.set(kid, d + 1);
                        q.push({ id: kid, d: d + 1 });
                    }
                }
                cur = parent;
            }

            // (c) transitive blockers of every on-route node.
            const frontier: Array<{ id: string; d: number }> = [];
            for (const [id, d] of perDest) frontier.push({ id, d });
            while (frontier.length) {
                const { id, d } = frontier.shift()!;
                if (d >= BLOCKER_DEPTH_CAP) continue;
                const blockers = blockersOut.get(id);
                if (!blockers) continue;
                for (const b of blockers) {
                    const existing = perDest.get(b);
                    if (existing !== undefined && existing <= d + 1) continue;
                    if (stopAt.has(b)) continue;
                    perDest.set(b, d + 1);
                    frontier.push({ id: b, d: d + 1 });
                }
            }

            // Commit per-destination visits to the global structures.
            for (const [id, d] of perDest) {
                routes.get(id)!.add(dest.id);
                const prev = depth.get(id);
                if (prev === undefined || d < prev) depth.set(id, d);
            }
        }

        return { destinations, routes, depth, destIndex };
    }


    // ─── Target-anchored layout ─────────────────────────────────────────────

    // Build a persistent d3-force simulation that pulls connected nodes
    // together. Terminals and backbone nodes are pinned at their preset (x,y);
    // route stations get a depth-band y anchor but flex in x. The simulation
    // stays alive so cytoscape drag events can reheat it and the rest of the
    // network reacts organically while the user drags a node.
    type FNode = {
        id: string;
        x: number;
        y: number;
        fx: number | null;
        fy: number | null;
        anchorX: number;
        anchorY: number;
        radius: number;
    };

    function startSimulation(
        metroNodes: GraphNode[],
        edges: GraphEdge[],
        positions: Map<string, { x: number; y: number }>,
        routeData: RouteData
    ): void {
        if (sim) { sim.stop(); sim = null; }
        const isBackbone = (n: GraphNode) => BACKBONE_TYPES.has((n.type || '').toLowerCase());
        simNodes = metroNodes.map(n => {
            const p = positions.get(n.id);
            if (!p) return null as any;
            const fixed = routeData.destIndex.has(n.id);
            return {
                id: n.id,
                x: p.x,
                y: p.y,
                // Only terminals pin — backbones now flow with the network so
                // the radial layout converges instead of fighting itself.
                fx: fixed ? p.x : null,
                fy: fixed ? p.y : null,
                anchorX: p.x,
                anchorY: p.y,
                radius: fixed ? 28 : isBackbone(n) ? 20 : 14,
            } as FNode;
        }).filter(Boolean) as FNode[];

        const idSet = new Set(simNodes.map(f => f.id));
        const flinks = edges
            .map(e => {
                const src = typeof e.source === 'object' ? e.source.id : e.source;
                const tgt = typeof e.target === 'object' ? e.target.id : e.target;
                return { source: src, target: tgt, type: e.type };
            })
            .filter(l => idSet.has(l.source) && idSet.has(l.target));

        sim = forceSimulation<FNode>(simNodes)
            .force('link', forceLink<FNode, any>(flinks)
                .id(d => d.id)
                .distance(l => {
                    if (l.type === 'parent' || l.type === 'depends_on') return 144;
                    if (l.type === 'soft_depends_on') return 200;
                    return 320;
                })
                .strength(l => (l.type === 'parent' || l.type === 'depends_on') ? 0.6 : 0.2))
            .force('charge', forceManyBody<FNode>().strength(-160).distanceMax(360))
            .force('collide', forceCollide<FNode>().radius(d => d.radius).strength(0.9))
            // Gentle pull toward the seeded centroid — enough to bias each
            // station toward its terminal cluster, loose enough that links
            // and drag dominate motion.
            .force('x', forceX<FNode>(d => d.anchorX).strength(0.04))
            .force('y', forceY<FNode>(d => d.anchorY).strength(0.04))
            .alphaDecay(0.02)
            .on('tick', () => {
                if (!cy) return;
                cy.batch(() => {
                    for (const f of simNodes) {
                        const n = cy!.getElementById(f.id);
                        if (!n.length) continue;
                        // While cy user-grabs a node, skip — cytoscape owns the
                        // position until release; we mirror its position to the
                        // sim (handled in drag handler).
                        if (n.grabbed()) continue;
                        n.position({ x: f.x, y: f.y });
                    }
                });
            });
    }

    // Seed starting positions so the sim doesn't have to untangle from
    // a pile. Short warm-up before cy ever renders.
    function warmSimulation(iterations = 120): void {
        if (!sim) return;
        sim.alpha(0.9);
        for (let i = 0; i < iterations; i++) sim.tick();
        // After warm-up, let sim continue breathing but at low energy.
        sim.alpha(0.1);
    }

    function computePositions(
        metroNodes: GraphNode[],
        edges: GraphEdge[],
        routeData: RouteData,
        width: number,
        height: number
    ): Map<string, { x: number; y: number }> {
        const { destinations, routes } = routeData;
        const positions = new Map<string, { x: number; y: number }>();

        const N = Math.max(1, destinations.length);
        const centerX = width / 2;
        const centerY = height / 2;
        // Terminals live on the outer edge of the *laid-out* graph, not the
        // viewport — force sim naturally spreads ~271 nodes over a much
        // larger area than the 780×400 canvas. Scale the perimeter rectangle
        // by expected node density so terminals actually sit at the edge of
        // the final drawing (after cy.fit() zooms the view to match).
        const nodeCount = Math.max(1, metroNodes.length);
        const virtualSide = Math.max(Math.min(width, height), Math.sqrt(nodeCount * 9000));
        const virtualW = Math.max(width, virtualSide * 1.35);
        const virtualH = Math.max(height, virtualSide * 0.95);
        const boxL = centerX - virtualW / 2 + 120;
        const boxR = centerX + virtualW / 2 - 120;
        const boxT = centerY - virtualH / 2 + 100;
        const boxB = centerY + virtualH / 2 - 100;

        const destinationX = new Map<string, number>();
        const destinationY = new Map<string, number>();
        destinations.forEach((d, i) => {
            if (N === 1) {
                destinationX.set(d.id, centerX);
                destinationY.set(d.id, boxT);
                return;
            }
            const angle = (i / N) * Math.PI * 2 - Math.PI / 2;
            const dx = Math.cos(angle);
            const dy = Math.sin(angle);
            // Scale the ray until it hits the nearest rectangle edge.
            const tx = Math.abs(dx) < 1e-6 ? Infinity : (dx > 0 ? (boxR - centerX) / dx : (boxL - centerX) / dx);
            const ty = Math.abs(dy) < 1e-6 ? Infinity : (dy > 0 ? (boxB - centerY) / dy : (boxT - centerY) / dy);
            const t = Math.min(tx, ty);
            destinationX.set(d.id, centerX + dx * t);
            destinationY.set(d.id, centerY + dy * t);
        });

        for (const d of destinations) {
            positions.set(d.id, { x: destinationX.get(d.id)!, y: destinationY.get(d.id)! });
        }

        // Route nodes: seed at the centroid of their serving terminals so the
        // force sim converges from a sensible starting state. Stations serving
        // a single terminal bias toward it; interchanges end up in the middle.
        const contextNodes: GraphNode[] = [];
        for (const n of metroNodes) {
            if (positions.has(n.id)) continue;
            const rs = routes.get(n.id);
            if (!rs || rs.size === 0) {
                contextNodes.push(n);
                continue;
            }
            let xSum = 0, ySum = 0;
            for (const did of rs) {
                xSum += destinationX.get(did) ?? centerX;
                ySum += destinationY.get(did) ?? centerY;
            }
            // Blend 70% centroid of terminals + 30% centre — keeps stations
            // pulled inward from the perimeter so the whole network breathes.
            const cxMean = xSum / rs.size;
            const cyMean = ySum / rs.size;
            const h = idHash(n.id);
            const jitter = ((h % 1000) / 1000 - 0.5) * 40;
            const x = cxMean * 0.7 + centerX * 0.3 + jitter;
            const y = cyMean * 0.7 + centerY * 0.3 + jitter;
            positions.set(n.id, { x, y });
        }

        // Context stations: a compact strip along the top, outside the
        // terminal ring but still readable. Callers control whether these
        // nodes reach buildGraph at all via showContext.
        if (contextNodes.length > 0) {
            const ranked = contextNodes
                .slice()
                .sort((a, b) => (b.dw || 0) - (a.dw || 0))
                .slice(0, CONTEXT_CAP);
            const stripY = Math.max(20, boxT - 40);
            const stripXMin = width * 0.05;
            const stripXSpan = width * 0.9;
            const cols = Math.max(1, Math.ceil(Math.sqrt(ranked.length)));
            ranked.forEach((n, i) => {
                const h = idHash(n.id);
                const col = i % cols;
                const row = Math.floor(i / cols);
                const jitter = (h % 500) / 500 - 0.5;
                const x = stripXMin + ((col + 0.5) / cols) * stripXSpan + jitter * 10;
                const y = Math.max(20, stripY - row * 24);
                positions.set(n.id, { x, y });
            });
        }

        // Collision spread: bucket by grid cell and offset siblings along x.
        // Tie-break by stable id hash — sorting by id string meant re-renders
        // with added/removed ids shifted bucket siblings, breaking stability.
        const buckets = new Map<string, string[]>();
        for (const [id, p] of positions) {
            const key = `${Math.round(p.x / GRID_X)}|${Math.round(p.y / GRID_Y)}`;
            const arr = buckets.get(key) ?? [];
            arr.push(id);
            buckets.set(key, arr);
        }
        for (const ids of buckets.values()) {
            if (ids.length <= 1) continue;
            ids.sort((a, b) => idHash(a) - idHash(b));
            ids.forEach((id, i) => {
                const p = positions.get(id)!;
                const offset = (i - (ids.length - 1) / 2) * (GRID_X * 0.9);
                positions.set(id, { x: p.x + offset, y: p.y });
            });
        }

        return positions;
    }

    // ─── Cytoscape node / edge data ─────────────────────────────────────────

    interface NodeData {
        id: string;
        label: string;
        displayLabel: string;
        nodeType: string;
        priority: number;
        visibilityState: VisibilityState;
        isDestination: 0 | 1;
        isOnRoute: 0 | 1;
        isBackbone: 0 | 1;
        routeIds: string;
        nodeSize: number;
        fillColor: string;
        borderColor: string;
        isCompleted: boolean;
        nodeOpacity: number;
    }

    // Visual encoding — kept deliberately sparse while we figure out the
    // metaphor. Three categories for now:
    //   - terminal (isDestination): large, priority-coloured, always labelled
    //   - starting station (no blockers): green round dot — nothing blocks it,
    //     so it is actionable right now. User intent: surface the concrete
    //     entry points of each line.
    //   - station (everything else): grey round dot, small
    // Completed nodes fade. No shape-by-type, no project fills, no interchange.
    const TERMINAL_FILL_P0 = PRIORITY_BORDERS[0] || '#dc3545';
    const TERMINAL_FILL_P1 = PRIORITY_BORDERS[1] || '#f59e0b';
    const STATION_FILL = '#94a3b8';
    const START_FILL = '#22c55e';
    const BAD_CHOICE_FILL = '#6b7280';       // dull grey body
    const BAD_CHOICE_BORDER = '#dc2626';     // red outline — "you picked this as priority but it isn't on any line"

    // A starting station is an on-route node with no incomplete blocker via
    // depends_on / soft_depends_on. Parent edges don't count — a parent epic
    // isn't a blocker of its child. Backbone (project/epic/goal) nodes are
    // excluded — they're structural, not actionable entry points.
    function computeStartingStations(
        nodes: GraphNode[],
        edges: GraphEdge[],
        routeData: RouteData
    ): Set<string> {
        const onRoute = new Set<string>();
        for (const n of nodes) {
            if ((routeData.routes.get(n.id)?.size ?? 0) >= 1) onRoute.add(n.id);
        }
        const nodeById = new Map(nodes.map(n => [n.id, n]));
        const hasBlocker = new Set<string>();
        for (const e of edges) {
            if (e.type !== 'depends_on' && e.type !== 'soft_depends_on') continue;
            const src = typeof e.source === 'object' ? e.source.id : e.source;
            const tgt = typeof e.target === 'object' ? e.target.id : e.target;
            if (!onRoute.has(src)) continue;
            const tgtNode = nodeById.get(tgt);
            if (!tgtNode) continue;
            if (!isIncomplete(tgtNode)) continue;
            hasBlocker.add(src);
        }
        const starts = new Set<string>();
        for (const id of onRoute) {
            const n = nodeById.get(id);
            if (!n) continue;
            if (!isIncomplete(n)) continue;
            if (routeData.destIndex.has(id)) continue;
            if (BACKBONE_TYPES.has((n.type || '').toLowerCase())) continue;
            if (!hasBlocker.has(id)) starts.add(id);
        }
        return starts;
    }

    function truncate(s: string, n: number): string {
        if (!s) return '';
        return s.length <= n ? s : s.slice(0, n - 1) + '…';
    }

    function getNodeData(node: GraphNode, routeData: RouteData, startingStations: Set<string>): NodeData {
        const rs = routeData.routes.get(node.id) ?? new Set();
        const isDestination = routeData.destIndex.has(node.id);
        const isOnRoute = rs.size >= 1;
        const isStart = startingStations.has(node.id);
        const visibilityState = priorityVisibility(node.priority);
        const completed = !isIncomplete(node);
        const typeLower = (node.type || '').toLowerCase();
        const isBackbone = BACKBONE_TYPES.has(typeLower);

        let nodeSize: number;
        let fillColor: string;
        let borderColor: string;
        let displayLabel: string;

        const isPriorityStation = !isDestination && node.priority <= 1 && isIncomplete(node) && typeLower !== 'target';
        const isBadChoice = isPriorityStation && !isOnRoute;

        if (isDestination) {
            nodeSize = 34;
            fillColor = node.priority === 0 ? TERMINAL_FILL_P0 : TERMINAL_FILL_P1;
            borderColor = '#ffffff';
            displayLabel = node.label;
        } else if (isBadChoice) {
            nodeSize = 14;
            fillColor = BAD_CHOICE_FILL;
            borderColor = BAD_CHOICE_BORDER;
            displayLabel = truncate(node.label, 40);
        } else if (isOnRoute && isBackbone) {
            // Epic / project / goal backbones — larger, squared, dim. These
            // anchor the line structurally but aren't the work itself.
            nodeSize = 18;
            fillColor = '#475569';
            borderColor = '#cbd5e1';
            displayLabel = truncate(node.label, 36);
        } else if (isStart) {
            nodeSize = isPriorityStation ? 16 : 12;
            fillColor = START_FILL;
            borderColor = '#ffffff';
            displayLabel = truncate(node.label, 40);
        } else if (isPriorityStation) {
            nodeSize = 16;
            fillColor = STATION_FILL;
            borderColor = 'rgba(255,255,255,0.45)';
            displayLabel = truncate(node.label, 40);
        } else if (isOnRoute) {
            // A station on a terminal's line — sub-task or blocker. Give it a
            // visible body + a label so the line is readable, not just dots.
            nodeSize = 12;
            fillColor = STATION_FILL;
            borderColor = 'rgba(255,255,255,0.35)';
            displayLabel = truncate(node.label, 40);
        } else {
            nodeSize = 3;
            fillColor = STATION_FILL;
            borderColor = 'rgba(255,255,255,0.08)';
            displayLabel = '';
        }

        const baseOpacity = visibilityState === 'half' ? 0.45 : 0.95;
        const nodeOpacity = completed ? baseOpacity * 0.35 : baseOpacity;

        return {
            id: node.id,
            label: node.label,
            displayLabel,
            nodeType: node.type,
            priority: node.priority,
            visibilityState,
            isDestination: isDestination ? 1 : 0,
            isOnRoute: isOnRoute ? 1 : 0,
            isBackbone: (isOnRoute && isBackbone) ? 1 : 0,
            routeIds: Array.from(rs).join(','),
            nodeSize,
            fillColor,
            borderColor,
            isCompleted: completed,
            nodeOpacity,
        };
    }

    function getEdgeVisibilityState(sourceVisibility: VisibilityState, targetVisibility: VisibilityState): VisibilityState {
        if (sourceVisibility === 'hidden' || targetVisibility === 'hidden') return 'hidden';
        if (sourceVisibility === 'half' || targetVisibility === 'half') return 'half';
        return 'bright';
    }

    function getEdgeOpacity(visibilityState: VisibilityState, isOnRoute: boolean): number {
        const base = isOnRoute ? 0.5 : 0.18;
        return visibilityState === 'half' ? base * 0.45 : base;
    }

    function getEdgeWidth(_edgeRole: string, isOnRoute: boolean): number {
        return isOnRoute ? 5 : 1;
    }

    // Shared destinations that both endpoints are on the route to.
    function sharedRouteIds(sourceRoutes: Set<string>, targetRoutes: Set<string>): string[] {
        const shared: string[] = [];
        for (const r of sourceRoutes) if (targetRoutes.has(r)) shared.push(r);
        shared.sort();
        return shared;
    }

    // For a collapsed route edge (single stroke), pick the dominant colour —
    // used by filter-update code. buildGraph emits per-route strokes for
    // multi-route edges so browser alpha compositing handles the blend.
    function getEdgeLineColor(
        shared: string[],
        destById: Map<string, GraphNode>,
        fallback: string
    ): string {
        if (shared.length === 0) return fallback;
        const dest = destById.get(shared[0]);
        return getProjectLineColor(dest?.project);
    }

    // HSL desaturation — projectColor returns hsl(...).
    function desaturateHsl(hsl: string, amount: number): string {
        const m = hsl.match(/hsl\(\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)%\s*,\s*(-?\d+(?:\.\d+)?)%\s*\)/);
        if (!m) return hsl;
        const h = m[1];
        const s = Math.max(0, parseFloat(m[2]) * (1 - amount));
        const l = parseFloat(m[3]);
        return `hsl(${h}, ${s.toFixed(1)}%, ${l}%)`;
    }

    // ─── Cytoscape lifecycle ────────────────────────────────────────────────

    function buildGraph() {
        // Use the pre-filter prepared graph so route discovery can see
        // completed children, low-priority blockers, and structural ancestors
        // that the UI filter chain would otherwise hide. Per-node visibility
        // (priority/status filters) is still applied via `visibilityState`.
        const sourceGraph = $preparedGraphData ?? $graphData;
        if (!containerEl || !sourceGraph) return;
        if (sim) { sim.stop(); sim = null; simNodes = []; }
        if (cy) { cy.destroy(); cy = null; }

        const width = containerEl.clientWidth || 1200;
        const height = containerEl.clientHeight || 800;

        const allMetroNodes = sourceGraph.nodes.filter(n => !HIDDEN_TYPES.has((n.type || '').toLowerCase()));
        const nodeByIdAll = new Map(allMetroNodes.map(n => [n.id, n]));
        const allMetroEdges = sourceGraph.links.filter(e => {
            const src = typeof e.source === 'object' ? e.source.id : e.source;
            const tgt = typeof e.target === 'object' ? e.target.id : e.target;
            return nodeByIdAll.has(src) && nodeByIdAll.has(tgt);
        });

        const routeData = computeRouteData(allMetroNodes, allMetroEdges);
        const destById = new Map(routeData.destinations.map(d => [d.id, d]));

        // Default view hides context stations. When showContext is true, the
        // top-CONTEXT_CAP by downstream_weight are kept in a dedicated strip.
        let contextKeep: Set<string> | null = null;
        if (showContext) {
            const ranked = allMetroNodes
                .filter(n => (routeData.routes.get(n.id)?.size ?? 0) === 0)
                .slice()
                .sort((a, b) => (b.dw || 0) - (a.dw || 0))
                .slice(0, CONTEXT_CAP);
            contextKeep = new Set(ranked.map(n => n.id));
        }
        // P0/P1 incomplete nodes that aren't on any route are "bad choices":
        // flagged as priority but not serving any declared target. Keep them
        // visible so the user can see them, just not anchored.
        const metroNodes = allMetroNodes.filter(n => {
            const onRoute = (routeData.routes.get(n.id)?.size ?? 0) > 0;
            if (onRoute) return true;
            if (isIncomplete(n) && n.priority <= 1 && (n.type || '').toLowerCase() !== 'target') return true;
            return contextKeep ? contextKeep.has(n.id) : false;
        });
        const nodeById = new Map(metroNodes.map(n => [n.id, n]));
        const metroEdges = allMetroEdges.filter(e => {
            const src = typeof e.source === 'object' ? e.source.id : e.source;
            const tgt = typeof e.target === 'object' ? e.target.id : e.target;
            return nodeById.has(src) && nodeById.has(tgt);
        });

        const positions = computePositions(metroNodes, metroEdges, routeData, width, height);
        const startingStations = computeStartingStations(metroNodes, metroEdges, routeData);

        const cyNodes = metroNodes.map(n => ({
            data: getNodeData(n, routeData, startingStations),
            position: positions.get(n.id) ?? { x: width / 2, y: height / 2 },
        }));

        // Per-route strokes for interchange edges (Tokyo "duplicate-per-line" trick).
        // An edge with shared.length >= 2 emits one stroke per shared destination;
        // each stroke is coloured by that destination's project at low opacity so
        // browser alpha compositing produces the blend naturally.
        const cyEdges: any[] = [];
        metroEdges.forEach((edge, index) => {
            const src = typeof edge.source === 'object' ? edge.source.id : edge.source;
            const tgt = typeof edge.target === 'object' ? edge.target.id : edge.target;
            const sourceNode = nodeById.get(src)!;
            const targetNode = nodeById.get(tgt)!;
            const sourceRoutes = routeData.routes.get(src) ?? new Set();
            const targetRoutes = routeData.routes.get(tgt) ?? new Set();
            const shared = sharedRouteIds(sourceRoutes, targetRoutes);
            const isOnRoute = shared.length >= 1;
            const edgeRole = getEdgeRole(edge.type);
            const sourceVisibility = priorityVisibility(sourceNode.priority);
            const targetVisibility = priorityVisibility(targetNode.priority);
            const visibilityState = getEdgeVisibilityState(sourceVisibility, targetVisibility);
            const fallback = getProjectLineColor(sourceNode.project || targetNode.project);
            const baseOpacity = getEdgeOpacity(visibilityState, isOnRoute);
            const edgeWidth = getEdgeWidth(edgeRole, isOnRoute);

            if (isOnRoute && shared.length >= 2) {
                // Emit one stroke per shared destination (blended by compositing).
                const perStrokeOpacity = baseOpacity * 0.85;
                shared.forEach((destId, k) => {
                    const dest = destById.get(destId);
                    cyEdges.push({
                        data: {
                            id: `e${index}_r${k}`,
                            source: src,
                            target: tgt,
                            edgeRole,
                            visibilityState,
                            isOnRoute: 1,
                            lineColor: getProjectLineColor(dest?.project),
                            edgeOpacity: perStrokeOpacity,
                            edgeWidth,
                        },
                    });
                });
            } else {
                const lineColor = isOnRoute
                    ? getEdgeLineColor(shared, destById, fallback)
                    : '#6b7280';
                cyEdges.push({
                    data: {
                        id: `e${index}`,
                        source: src,
                        target: tgt,
                        edgeRole,
                        visibilityState,
                        isOnRoute: isOnRoute ? 1 : 0,
                        lineColor,
                        edgeOpacity: baseOpacity,
                        edgeWidth,
                    },
                });
            }
        });

        cy = cytoscape({
            container: containerEl,
            elements: [...cyNodes, ...cyEdges],
            style: [
                // Stations — muted uniform dots
                {
                    selector: 'node[visibilityState != "hidden"]',
                    style: {
                        'shape': 'ellipse',
                        'width': 'data(nodeSize)',
                        'height': 'data(nodeSize)',
                        'background-color': 'data(fillColor)',
                        'background-opacity': 0.85,
                        'border-width': 1,
                        'border-color': 'data(borderColor)',
                        'opacity': 'data(nodeOpacity)',
                        'label': '',
                        'text-opacity': 0,
                    } as any,
                },
                // Stations with a displayLabel — route stations (sub-tasks /
                // blockers), priority stations, bad choices. Labels hidden at
                // far zoom so the overview stays clean.
                {
                    selector: 'node[isOnRoute = 1][isDestination = 0][isBackbone = 0]',
                    style: {
                        'label': 'data(displayLabel)',
                        'text-opacity': 1,
                        'color': '#cbd5e1',
                        'font-size': 9,
                        'text-outline-color': '#0b0f17',
                        'text-outline-width': 2,
                        'text-valign': 'center',
                        'text-halign': 'right',
                        'text-margin-x': 6,
                        'text-max-width': '180px',
                        'text-wrap': 'wrap',
                        'min-zoomed-font-size': 8,
                    } as any,
                },
                // Backbones — epic/project/goal on route. Squared, muted, small label.
                {
                    selector: 'node[isBackbone = 1]',
                    style: {
                        'shape': 'round-rectangle',
                        'label': 'data(displayLabel)',
                        'text-opacity': 1,
                        'color': '#e2e8f0',
                        'font-size': 10,
                        'font-weight': '600',
                        'text-outline-color': '#0b0f17',
                        'text-outline-width': 2,
                        'text-valign': 'center',
                        'text-halign': 'right',
                        'text-margin-x': 8,
                        'text-max-width': '200px',
                        'text-wrap': 'wrap',
                        'min-zoomed-font-size': 7,
                    } as any,
                },
                // Terminals — big, priority-coloured, always labelled
                {
                    selector: 'node[isDestination = 1]',
                    style: {
                        'shape': 'round-rectangle',
                        'background-opacity': 1,
                        'border-width': 3,
                        'border-color': '#ffffff',
                        'z-index': 9999,
                        'label': 'data(displayLabel)',
                        'text-opacity': 1,
                        'font-size': 13,
                        'font-weight': '700',
                        'color': '#ffffff',
                        'text-outline-color': '#000',
                        'text-outline-width': 3,
                        'text-valign': 'bottom',
                        'text-halign': 'center',
                        'text-margin-y': 12,
                        'text-max-width': '160px',
                        'text-wrap': 'wrap',
                        'min-zoomed-font-size': 0,
                    } as any,
                },
                {
                    selector: 'node[visibilityState = "hidden"]',
                    style: { 'display': 'none' } as any,
                },
                // Route edges — uniform
                {
                    selector: 'edge[isOnRoute = 1][visibilityState != "hidden"]',
                    style: {
                        'width': 'data(edgeWidth)',
                        'line-color': '#94a3b8',
                        'opacity': 0.5,
                        'curve-style': 'haystack',
                        'haystack-radius': 0,
                    } as any,
                },
                // Non-route edges — thin grey dashed backdrop
                {
                    selector: 'edge[isOnRoute = 0][visibilityState != "hidden"]',
                    style: {
                        'width': 1,
                        'line-color': '#475569',
                        'opacity': 0.2,
                        'curve-style': 'straight',
                        'line-style': 'dashed',
                    } as any,
                },
                {
                    selector: 'edge[visibilityState = "hidden"]',
                    style: { 'display': 'none' } as any,
                },
                {
                    selector: ':selected',
                    style: {
                        'border-width': 5,
                        'border-color': '#fff',
                        'border-opacity': 0.9,
                        'overlay-padding': 8,
                        'overlay-opacity': 0.18,
                    } as any,
                },
                // Route highlight — cytoscape renders to canvas, so these
                // classes must be declared in the stylesheet (not just as DOM
                // CSS) to actually dim/brighten pixels.
                {
                    selector: '.not-path',
                    style: { 'opacity': 0.1 } as any,
                },
                {
                    selector: '.route-active',
                    style: { 'opacity': 1 } as any,
                },
                {
                    selector: '.dimmed',
                    style: { 'opacity': 0.15 } as any,
                },
                {
                    selector: '.highlighted',
                    style: { 'opacity': 1 } as any,
                },
            ],
            layout: { name: 'preset' } as any,
            wheelSensitivity: 0.3,
            minZoom: 0.05,
            maxZoom: 5,
        });

        cy.one('layoutstop', () => { cy?.fit(undefined, 60); running = false; });
        // preset layouts don't always emit layoutstop; fit on next tick as backup
        setTimeout(() => { if (cy) cy.fit(undefined, 60); }, 0);
        (window as any).__cy = cy;

        // Kick off the persistent force simulation so edges pull nodes together
        // live, and dragging a terminal tows its whole network.
        startSimulation(metroNodes, metroEdges, positions, routeData);
        warmSimulation(160);
        // Push warmed positions into cytoscape before the first paint settles
        if (cy) {
            cy.batch(() => {
                for (const f of simNodes) {
                    const n = cy!.getElementById(f.id);
                    if (n.length) n.position({ x: f.x, y: f.y });
                }
            });
            setTimeout(() => cy?.fit(undefined, 60), 0);
        }

        // ── Interactions ──

        // Drag: pin the dragged node in the simulation so the rest of the
        // network is pulled along. Reheat the sim to keep motion alive.
        const simById = new Map<string, FNode>(simNodes.map(f => [f.id, f]));

        cy.on('grab', 'node', (evt) => {
            const id = evt.target.id();
            const f = simById.get(id);
            if (!f) return;
            const pos = evt.target.position();
            f.fx = pos.x;
            f.fy = pos.y;
            if (sim) sim.alphaTarget(0.3).restart();
        });

        cy.on('drag', 'node', (evt) => {
            const id = evt.target.id();
            const f = simById.get(id);
            if (!f) return;
            const pos = evt.target.position();
            f.fx = pos.x;
            f.fy = pos.y;
        });

        cy.on('free', 'node', (evt) => {
            const id = evt.target.id();
            const f = simById.get(id);
            if (!f) return;
            // Terminals and backbones keep their pin. Everything else releases.
            const isDest = evt.target.data('isDestination') === 1;
            const isBackbone = evt.target.data('isBackbone') === 1;
            if (!isDest && !isBackbone) {
                f.fx = null;
                f.fy = null;
            } else {
                // Update anchor so simulation's x/y forces respect the new home
                f.anchorX = f.fx ?? f.x;
                f.anchorY = f.fy ?? f.y;
            }
            if (sim) sim.alphaTarget(0);
        });

        // Keep a reference to the currently-highlighted destination (toggle)
        let activeHighlightDestId: string | null = null;

        function clearHighlight() {
            if (!cy) return;
            cy.elements().removeClass('not-path').removeClass('route-active');
            activeHighlightDestId = null;
        }

        function highlightForNode(nodeId: string) {
            if (!cy) return;
            const rs = routeData.routes.get(nodeId);
            if (!rs || rs.size === 0) { clearHighlight(); return; }
            cy.batch(() => {
                cy!.elements().addClass('not-path').removeClass('route-active');
                // Any node whose routes share at least one destination with the tapped node
                cy!.nodes().forEach(n => {
                    const nodeRoutes = routeData.routes.get(n.id()) ?? new Set();
                    for (const r of rs) {
                        if (nodeRoutes.has(r)) {
                            n.removeClass('not-path').addClass('route-active');
                            break;
                        }
                    }
                });
                // Edges that connect two highlighted nodes
                cy!.edges().forEach(e => {
                    if (e.source().hasClass('route-active') && e.target().hasClass('route-active')) {
                        e.removeClass('not-path').addClass('route-active');
                    }
                });
            });
        }

        cy.on('tap', 'node', (evt) => {
            const id = evt.target.id();
            const isDest = evt.target.data('isDestination') === 1;
            if (isDest) {
                if (activeHighlightDestId === id) {
                    clearHighlight();
                } else {
                    activeHighlightDestId = id;
                    highlightForNode(id);
                }
            } else {
                activeHighlightDestId = null;
                highlightForNode(id);
                toggleSelection(id);
            }
        });

        cy.on('tap', (evt) => {
            if (evt.target === cy) clearHighlight();
        });

        cy.on('mouseover', 'node', (evt) => {
            const node = evt.target;
            const id = node.id();
            selection.update(s => ({ ...s, hoveredNodeId: id }));
            cy!.elements().addClass('dimmed');
            node.removeClass('dimmed').addClass('highlighted');
            node.neighborhood().removeClass('dimmed').addClass('highlighted');

            // Tooltip: title, status, destinations the station serves.
            const raw = nodeById.get(id);
            const rs = routeData.routes.get(id);
            const destinations: string[] = [];
            if (rs) {
                for (const destId of rs) {
                    const d = destById.get(destId);
                    if (d) destinations.push(d.label);
                }
            }
            const pos = node.renderedPosition();
            tooltip = {
                x: pos.x,
                y: pos.y - (node.renderedHeight ? node.renderedHeight() : 20) / 2,
                title: raw?.label || id,
                status: raw?.status || '',
                priority: raw?.priority ?? -1,
                project: raw?.project ?? null,
                destinations,
            };
        });

        cy.on('mousemove', 'node', (evt) => {
            if (!tooltip) return;
            const node = evt.target;
            const pos = node.renderedPosition();
            tooltip = {
                ...tooltip,
                x: pos.x,
                y: pos.y - (node.renderedHeight ? node.renderedHeight() : 20) / 2,
            };
        });

        cy.on('mouseout', 'node', () => {
            selection.update(s => ({ ...s, hoveredNodeId: null }));
            cy!.elements().removeClass('dimmed').removeClass('highlighted');
            tooltip = null;
        });
    }

    // Parent component binds a play/stop control. Metro has no live simulation,
    // so this re-runs the preset layout and animates nodes to their new
    // positions — useful after filter changes. The control is surfaced to the
    // user as "Recompute" in Metro mode (see parent view chrome).
    export function toggleRunning() {
        const sourceGraph = $preparedGraphData ?? $graphData;
        if (!cy || !containerEl || !sourceGraph) return;
        const width = containerEl.clientWidth || 1200;
        const height = containerEl.clientHeight || 800;
        const metroNodes = sourceGraph.nodes.filter(n => !HIDDEN_TYPES.has((n.type || '').toLowerCase()));
        const nodeById = new Map(metroNodes.map(n => [n.id, n]));
        const metroEdges = sourceGraph.links.filter(e => {
            const src = typeof e.source === 'object' ? e.source.id : e.source;
            const tgt = typeof e.target === 'object' ? e.target.id : e.target;
            return nodeById.has(src) && nodeById.has(tgt);
        });
        const routeData = computeRouteData(metroNodes, metroEdges);
        const positions = computePositions(metroNodes, metroEdges, routeData, width, height);

        running = true;
        let pending = 0;
        for (const n of metroNodes) {
            const cyNode = cy.getElementById(n.id);
            if (!cyNode.length) continue;
            const pos = positions.get(n.id);
            if (!pos) continue;
            pending++;
            cyNode.animate({ position: pos }, {
                duration: 500,
                easing: 'ease-in-out-cubic',
                complete: () => {
                    pending--;
                    if (pending === 0) {
                        running = false;
                        cy?.fit(undefined, 60);
                    }
                },
            });
        }
        if (pending === 0) running = false;
    }

    // Rebuild on structural changes
    let lastStructureKey = '';
    let lastShowContext = showContext;
    $: if (containerEl && ($preparedGraphData || $graphData) && ($preparedStructureKey !== lastStructureKey || showContext !== lastShowContext)) {
        lastStructureKey = $preparedStructureKey;
        lastShowContext = showContext;
        buildGraph();
    }

    // Refresh node/edge visibility data when priority filters change but
    // structure doesn't. Edges may have been split into per-route strokes —
    // we iterate cy's actual edges and refresh each by source/target.
    $: if (cy && ($preparedGraphData || $graphData) && $preparedStructureKey === lastStructureKey) {
        const cyInstance = cy;
        const sourceGraph = $preparedGraphData ?? $graphData!;
        const nodeById = new Map(sourceGraph.nodes.map(n => [n.id, n]));
        const allMetroNodes = sourceGraph.nodes.filter(n => !HIDDEN_TYPES.has((n.type || '').toLowerCase()));
        const allMetroEdges = sourceGraph.links.filter(e => {
            const src = typeof e.source === 'object' ? e.source.id : e.source;
            const tgt = typeof e.target === 'object' ? e.target.id : e.target;
            return nodeById.has(src) && nodeById.has(tgt);
        });
        const routeData = computeRouteData(allMetroNodes, allMetroEdges);
        const startingStations = computeStartingStations(allMetroNodes, allMetroEdges, routeData);

        for (const n of allMetroNodes) {
            const cyNode = cyInstance.getElementById(n.id);
            if (!cyNode.length) continue;
            Object.entries(getNodeData(n, routeData, startingStations)).forEach(([k, v]) => cyNode.data(k, v));
        }

        cyInstance.edges().forEach(cyEdge => {
            const src = cyEdge.source().id();
            const tgt = cyEdge.target().id();
            const sourceNode = nodeById.get(src) as any;
            const targetNode = nodeById.get(tgt) as any;
            if (!sourceNode || !targetNode) return;
            const sourceRoutes = routeData.routes.get(src) ?? new Set();
            const targetRoutes = routeData.routes.get(tgt) ?? new Set();
            const shared = sharedRouteIds(sourceRoutes, targetRoutes);
            const isOnRoute = shared.length >= 1;
            const visibilityState = getEdgeVisibilityState(
                priorityVisibility(sourceNode.priority),
                priorityVisibility(targetNode.priority),
            );
            cyEdge.data('visibilityState', visibilityState);
            cyEdge.data('isOnRoute', isOnRoute ? 1 : 0);
            cyEdge.data('edgeOpacity', getEdgeOpacity(visibilityState, isOnRoute));
        });
    }

    $: if (cy && $selection.activeNodeId) {
        cy.nodes().unselect();
        const node = cy.getElementById($selection.activeNodeId);
        if (node.length) node.select();
    }

    onDestroy(() => {
        if (sim) { sim.stop(); sim = null; simNodes = []; }
        if (cy) cy.destroy();
    });
</script>

<div class="metro-root">
    <div
        bind:this={containerEl}
        class="w-full h-full bg-background/50 metro-canvas"
    ></div>
    {#if tooltip}
        <div
            class="metro-tooltip"
            style="transform: translate({tooltip.x}px, {tooltip.y}px);"
        >
            <div class="metro-tooltip-title">{tooltip.title}</div>
            <div class="metro-tooltip-meta">
                {#if tooltip.priority >= 0}<span>P{tooltip.priority}</span>{/if}
                {#if tooltip.status}<span>{tooltip.status}</span>{/if}
                {#if tooltip.project}<span>{tooltip.project}</span>{/if}
            </div>
            {#if tooltip.destinations.length}
                <div class="metro-tooltip-dest-label">On routes to:</div>
                <ul class="metro-tooltip-dest-list">
                    {#each tooltip.destinations.slice(0, 6) as d}
                        <li>{d}</li>
                    {/each}
                    {#if tooltip.destinations.length > 6}
                        <li class="metro-tooltip-more">+{tooltip.destinations.length - 6} more</li>
                    {/if}
                </ul>
            {/if}
        </div>
    {/if}
</div>

<style>
    :global(.dimmed) {
        opacity: 0.15 !important;
        transition: opacity 0.2s ease;
    }
    :global(.highlighted) {
        opacity: 1 !important;
        transition: opacity 0.2s ease;
    }
    :global(.not-path) {
        opacity: 0.1 !important;
        transition: opacity 0.2s ease;
    }
    :global(.route-active) {
        opacity: 1 !important;
        transition: opacity 0.2s ease;
    }

    .metro-root {
        position: relative;
        width: 100%;
        height: 100%;
    }
    .metro-canvas {
        width: 100%;
        height: 100%;
    }
    .metro-tooltip {
        position: absolute;
        top: 0;
        left: 0;
        pointer-events: none;
        max-width: 280px;
        padding: 8px 10px;
        background: rgba(10, 14, 20, 0.94);
        color: #f5f7fb;
        border: 1px solid rgba(255, 255, 255, 0.15);
        border-radius: 6px;
        font-size: 11px;
        line-height: 1.35;
        box-shadow: 0 4px 16px rgba(0, 0, 0, 0.5);
        translate: -50% calc(-100% - 10px);
        z-index: 10000;
    }
    .metro-tooltip-title {
        font-weight: 600;
        margin-bottom: 4px;
        overflow-wrap: anywhere;
    }
    .metro-tooltip-meta {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
        font-size: 9px;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        color: color-mix(in srgb, #f5f7fb 65%, transparent);
        margin-bottom: 6px;
    }
    .metro-tooltip-dest-label {
        font-size: 9px;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        color: color-mix(in srgb, #f5f7fb 55%, transparent);
        margin-bottom: 2px;
    }
    .metro-tooltip-dest-list {
        margin: 0;
        padding-left: 14px;
        font-size: 11px;
    }
    .metro-tooltip-more {
        list-style: none;
        margin-left: -14px;
        opacity: 0.6;
    }
</style>
