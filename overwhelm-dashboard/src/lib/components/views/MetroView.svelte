<script lang="ts">
    import { onDestroy } from "svelte";
    import cytoscape from "cytoscape";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { filters, type VisibilityState } from "../../stores/filters";
    import { selection, toggleSelection } from "../../stores/selection";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";
    import { INCOMPLETE_STATUSES, PRIORITY_BORDERS } from "../../data/constants";
    import { projectColor } from "../../data/projectUtils";

    let containerEl: HTMLDivElement;
    let cy: cytoscape.Core | null = null;

    export let running = false;
    // Context stations (nodes on no route) are noise for the "routes to
    // destinations" question — hidden by default. Toggle via the `Show context`
    // control; when enabled, only the top-N by downstream_weight render.
    export let showContext = false;

    const HIDDEN_TYPES = new Set(['project']);
    const CONTAINER_TYPES = new Set(['goal', 'epic']);
    const EPIC_TYPES = new Set(['epic', 'goal']);
    const DEFAULT_PROJECT_COLOR = 'hsl(220, 12%, 46%)';

    // Layout constants
    const ROW_HEIGHT = 120;
    const CONTEXT_STRIP_Y = 140;
    const CONTEXT_CAP = 200;           // hard cap on rendered context stations
    const TERMINAL_ROW_GAP = 32;       // vertical spacing between terminal rows
    const TERMINAL_PER_ROW = 12;       // target number of terminals per row
    const GOAL_PARENT_HOP_CAP = 3;     // limit goal-destination descendant walk depth
    const GRID_X = 36;
    const GRID_Y = 40;

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

    // Build an undirected neighbor map covering every edge type — parent,
    // depends_on, soft_depends_on, ref, anything else. Route discovery treats
    // the graph as undirected so every incident edge counts as a potential
    // route step. Per-edge-type weighting / directionality can be re-layered
    // later if the picture demands it.
    function buildNeighbors(nodes: GraphNode[], edges: GraphEdge[]): Map<string, Set<string>> {
        const nbr = new Map<string, Set<string>>();
        for (const n of nodes) nbr.set(n.id, new Set());
        for (const e of edges) {
            const src = typeof e.source === 'object' ? e.source.id : e.source;
            const tgt = typeof e.target === 'object' ? e.target.id : e.target;
            if (!nbr.has(src) || !nbr.has(tgt)) continue;
            if (src === tgt) continue;
            nbr.get(src)!.add(tgt);
            nbr.get(tgt)!.add(src);
        }
        return nbr;
    }

    // Every P0/P1 incomplete node is a destination — that's what the user set
    // as a target. Whether it has children decides how the route is walked,
    // not whether it qualifies.
    function computeIncompleteChildIds(nodes: GraphNode[]): Set<string> {
        const ids = new Set<string>();
        for (const n of nodes) {
            if (n.parent && isIncomplete(n)) ids.add(n.parent);
        }
        return ids;
    }

    // A destination behaves as a container (pulls its incomplete subtree
    // inward as the route) when its type is goal/epic, or when it has any
    // incomplete children. Otherwise it's a leaf and walks parent ancestors.
    function isContainerDestination(dest: GraphNode, incompleteChildIds: Set<string>): boolean {
        if (CONTAINER_TYPES.has((dest.type || '').toLowerCase())) return true;
        return incompleteChildIds.has(dest.id);
    }

    // Terminals are now explicitly target-type nodes. P0/P1 "priority" tasks
    // that aren't targets are rendered as larger stations but are not the
    // named ends-of-lines. The user sets targets deliberately; priority alone
    // is a weaker signal.
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

        const neighbors = buildNeighbors(nodes, edges);
        const nodeById = new Map(nodes.map(n => [n.id, n]));

        // Undirected BFS from each destination. Every edge type is a step, no
        // direction distinction — we want to see all edges on the map before
        // deciding which distinctions matter.
        for (const dest of destinations) {
            const seen = new Set<string>([dest.id]);
            const queue: Array<{ id: string; d: number }> = [{ id: dest.id, d: 0 }];
            while (queue.length > 0) {
                const { id, d } = queue.shift()!;
                const node = nodeById.get(id);
                if (!node) continue;
                if (!isIncomplete(node) && id !== dest.id) continue;

                routes.get(id)!.add(dest.id);
                const prev = depth.get(id);
                if (prev === undefined || d < prev) depth.set(id, d);

                const nbr = neighbors.get(id);
                if (!nbr) continue;
                for (const next of nbr) {
                    if (seen.has(next)) continue;
                    seen.add(next);
                    queue.push({ id: next, d: d + 1 });
                }
            }
        }

        return { destinations, routes, depth, destIndex };
    }


    // ─── Target-anchored layout ─────────────────────────────────────────────

    function computePositions(
        metroNodes: GraphNode[],
        routeData: RouteData,
        width: number,
        height: number
    ): Map<string, { x: number; y: number }> {
        const { destinations, routes, depth } = routeData;
        const positions = new Map<string, { x: number; y: number }>();

        const N = Math.max(1, destinations.length);
        const xMin = width * 0.1;
        const xMax = width * 0.9;
        const xSpan = xMax - xMin;

        // Multi-row terminal staggering: when there are many destinations,
        // cycle them through `rowCount` rows so adjacent labels don't collide.
        const rowCount = Math.max(1, Math.min(4, Math.ceil(N / TERMINAL_PER_ROW)));
        const terminalYBase = height - 140;

        const destinationX = new Map<string, number>();
        const destinationY = new Map<string, number>();
        destinations.forEach((d, i) => {
            const x = N === 1 ? width / 2 : xMin + (i * xSpan) / (N - 1);
            const row = i % rowCount;
            // lower rows (closer to the bottom) get larger y; we stagger upward
            // so there's always label-clear space below the bottom-most row
            const y = terminalYBase - row * TERMINAL_ROW_GAP;
            destinationX.set(d.id, x);
            destinationY.set(d.id, y);
        });

        // Max depth observed — used to size the route area
        let maxDepth = 0;
        for (const d of depth.values()) if (d > maxDepth) maxDepth = d;

        const topOfTerminals = terminalYBase - (rowCount - 1) * TERMINAL_ROW_GAP;
        const routeAreaTop = topOfTerminals - (maxDepth + 1) * ROW_HEIGHT - 40;

        // Destinations at the bottom, staggered y per row
        for (const d of destinations) {
            positions.set(d.id, { x: destinationX.get(d.id)!, y: destinationY.get(d.id)! });
        }

        // Route nodes: x = mean of serving destinations' anchors, y = topOfTerminals - depth*rowHeight
        const contextNodes: GraphNode[] = [];
        for (const n of metroNodes) {
            if (positions.has(n.id)) continue; // skip destinations
            const rs = routes.get(n.id);
            if (!rs || rs.size === 0) {
                contextNodes.push(n);
                continue;
            }
            const d = depth.get(n.id) ?? 1;
            let xSum = 0;
            for (const did of rs) xSum += destinationX.get(did) ?? width / 2;
            const x = xSum / rs.size;
            const y = topOfTerminals - d * ROW_HEIGHT;
            positions.set(n.id, { x, y });
        }

        // Context stations: only lay out a top-N slice, bucketed above the
        // route area. Everything else stays unpositioned and is excluded from
        // the cytoscape element set in buildGraph. Callers control this via
        // the `showContext` prop; when false, `contextNodes` will not appear
        // in the node list (filtered upstream), and this branch is a no-op.
        if (contextNodes.length > 0) {
            // Preserve top-CONTEXT_CAP by downstream_weight
            const ranked = contextNodes
                .slice()
                .sort((a, b) => (b.dw || 0) - (a.dw || 0))
                .slice(0, CONTEXT_CAP);
            const contextY = Math.min(CONTEXT_STRIP_Y, routeAreaTop - 60);
            const contextXSpan = width * 0.9;
            const contextXMin = width * 0.05;
            const cols = Math.max(1, Math.ceil(Math.sqrt(ranked.length)));
            ranked.forEach((n, i) => {
                const h = idHash(n.id);
                const col = i % cols;
                const row = Math.floor(i / cols);
                const jitter = (h % 500) / 500 - 0.5; // ±0.5
                const x = contextXMin + ((col + 0.5) / cols) * contextXSpan + jitter * 10;
                const y = Math.max(20, contextY - row * 24);
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

    // Build a set of on-route nodes that have no outgoing blocker edge to
    // another incomplete on-route node. Under the schema we're adopting here,
    // every blocker edge (parent flipped, depends_on, soft_depends_on) points
    // from the blocked node to its blocker — so "no outgoing blocker edge" ⇒
    // nothing is stopping this node from starting.
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
            if (routeData.destIndex.has(id)) continue; // terminals keep their own fill
            if (!hasBlocker.has(id)) starts.add(id);
        }
        return starts;
    }

    function getNodeData(node: GraphNode, routeData: RouteData, startingStations: Set<string>): NodeData {
        const rs = routeData.routes.get(node.id) ?? new Set();
        const isDestination = routeData.destIndex.has(node.id);
        const isOnRoute = rs.size >= 1;
        const isStart = startingStations.has(node.id);
        const visibilityState = priorityVisibility(node.priority);
        const completed = !isIncomplete(node);

        let nodeSize: number;
        let fillColor: string;
        let borderColor: string;
        let displayLabel: string;

        const isPriorityStation = !isDestination && node.priority <= 1 && isIncomplete(node) && (node.type || '').toLowerCase() !== 'target';
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
            displayLabel = '';
        } else if (isStart) {
            nodeSize = isPriorityStation ? 14 : 10;
            fillColor = START_FILL;
            borderColor = '#ffffff';
            displayLabel = '';
        } else if (isPriorityStation) {
            nodeSize = 14;
            fillColor = STATION_FILL;
            borderColor = 'rgba(255,255,255,0.35)';
            displayLabel = '';
        } else {
            nodeSize = isOnRoute ? 7 : 3;
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
        if (!containerEl || !$graphData) return;
        if (cy) { cy.destroy(); cy = null; }

        const width = containerEl.clientWidth || 1200;
        const height = containerEl.clientHeight || 800;

        const allMetroNodes = $graphData.nodes.filter(n => !HIDDEN_TYPES.has((n.type || '').toLowerCase()));
        const nodeByIdAll = new Map(allMetroNodes.map(n => [n.id, n]));
        const allMetroEdges = $graphData.links.filter(e => {
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

        const positions = computePositions(metroNodes, routeData, width, height);
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

        // ── Interactions ──

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
        if (!cy || !containerEl || !$graphData) return;
        const width = containerEl.clientWidth || 1200;
        const height = containerEl.clientHeight || 800;
        const metroNodes = $graphData.nodes.filter(n => !HIDDEN_TYPES.has((n.type || '').toLowerCase()));
        const nodeById = new Map(metroNodes.map(n => [n.id, n]));
        const metroEdges = $graphData.links.filter(e => {
            const src = typeof e.source === 'object' ? e.source.id : e.source;
            const tgt = typeof e.target === 'object' ? e.target.id : e.target;
            return nodeById.has(src) && nodeById.has(tgt);
        });
        const routeData = computeRouteData(metroNodes, metroEdges);
        const positions = computePositions(metroNodes, routeData, width, height);

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
    $: if (containerEl && $graphData && ($graphStructureKey !== lastStructureKey || showContext !== lastShowContext)) {
        lastStructureKey = $graphStructureKey;
        lastShowContext = showContext;
        buildGraph();
    }

    // Refresh node/edge visibility data when priority filters change but
    // structure doesn't. Edges may have been split into per-route strokes —
    // we iterate cy's actual edges and refresh each by source/target.
    $: if (cy && $graphData && $graphStructureKey === lastStructureKey) {
        const cyInstance = cy;
        const nodeById = new Map($graphData.nodes.map(n => [n.id, n]));
        const allMetroNodes = $graphData.nodes.filter(n => !HIDDEN_TYPES.has((n.type || '').toLowerCase()));
        const allMetroEdges = $graphData.links.filter(e => {
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
