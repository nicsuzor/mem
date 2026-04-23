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

    const HIDDEN_TYPES = new Set(['project']);
    const EPIC_TYPES = new Set(['epic', 'goal']);
    const DEFAULT_PROJECT_COLOR = 'hsl(220, 12%, 46%)';

    // Layout constants
    const LANE_WIDTH = 240;
    const ROW_HEIGHT = 120;
    const TERMINAL_Y = 0;
    const CONTEXT_STRIP_Y = 140;
    const GRID_X = 36;
    const GRID_Y = 40;

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

    function computeDestinations(nodes: GraphNode[]): GraphNode[] {
        const parentIds = new Set<string>();
        const incompleteChildIds = new Set<string>();
        for (const n of nodes) {
            if (n.parent) {
                parentIds.add(n.parent);
                if (isIncomplete(n)) incompleteChildIds.add(n.parent);
            }
        }
        return nodes
            .filter(n => {
                if (!isIncomplete(n)) return false;
                if (n.priority > 1) return false;
                const type = (n.type || '').toLowerCase();
                if (type === 'goal') return true;
                // Leaf destinations: P0/P1 with no incomplete descendants
                return !incompleteChildIds.has(n.id);
            })
            .sort((a, b) => {
                if (a.priority !== b.priority) return a.priority - b.priority;
                const pa = (a.project || '').toLowerCase();
                const pb = (b.project || '').toLowerCase();
                if (pa !== pb) return pa.localeCompare(pb);
                return a.label.localeCompare(b.label);
            });
    }

    // Build adjacency: for each node n, which other nodes are "upstream" from n
    // (i.e. complete the destination requires completing them). We traverse:
    //   - depends_on / soft_depends_on edges where source==n -> target is blocker
    //   - parent edges where source==n (parent side) -> target is child (belongs to n)
    function buildUpstreamAdjacency(nodes: GraphNode[], edges: GraphEdge[]): Map<string, Set<string>> {
        const adj = new Map<string, Set<string>>();
        for (const n of nodes) adj.set(n.id, new Set());
        for (const e of edges) {
            const src = typeof e.source === 'object' ? e.source.id : e.source;
            const tgt = typeof e.target === 'object' ? e.target.id : e.target;
            if (!adj.has(src) || !adj.has(tgt)) continue;
            if (e.type === 'depends_on' || e.type === 'soft_depends_on') {
                adj.get(src)!.add(tgt);
            } else if (e.type === 'parent') {
                // prepareGraphData flips parent edges so source=parent, target=child
                adj.get(src)!.add(tgt);
            }
        }
        return adj;
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

        const adj = buildUpstreamAdjacency(nodes, edges);
        const nodeById = new Map(nodes.map(n => [n.id, n]));

        // BFS from each destination; record membership + min distance per node
        for (const dest of destinations) {
            const seen = new Set<string>([dest.id]);
            const queue: Array<{ id: string; d: number }> = [{ id: dest.id, d: 0 }];
            while (queue.length > 0) {
                const { id, d } = queue.shift()!;
                const node = nodeById.get(id);
                if (!node) continue;
                // Only traverse through incomplete nodes (completed = already-traversed track)
                if (!isIncomplete(node) && id !== dest.id) continue;

                routes.get(id)!.add(dest.id);
                const prev = depth.get(id);
                if (prev === undefined || d < prev) depth.set(id, d);

                const upstream = adj.get(id);
                if (!upstream) continue;
                for (const next of upstream) {
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
        const { destinations, routes, depth, destIndex } = routeData;
        const positions = new Map<string, { x: number; y: number }>();

        const N = Math.max(1, destinations.length);
        const xMin = width * 0.1;
        const xMax = width * 0.9;
        const xSpan = xMax - xMin;
        const destinationX = new Map<string, number>();
        destinations.forEach((d, i) => {
            const x = N === 1 ? width / 2 : xMin + (i * xSpan) / (N - 1);
            destinationX.set(d.id, x);
        });

        // Max depth observed — used to size the route area
        let maxDepth = 0;
        for (const d of depth.values()) if (d > maxDepth) maxDepth = d;

        const routeAreaTop = height - (maxDepth + 1) * ROW_HEIGHT - 80;
        const yMax = height - 140; // terminal band y

        // Destinations at the bottom
        for (const d of destinations) {
            positions.set(d.id, { x: destinationX.get(d.id)!, y: yMax });
        }

        // Route nodes: x = mean of serving destinations' anchors, y = yMax - depth*rowHeight
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
            const y = yMax - d * ROW_HEIGHT;
            positions.set(n.id, { x, y });
        }

        // Context nodes sit in a strip above the route area, spread by id hash
        const contextY = Math.min(CONTEXT_STRIP_Y, routeAreaTop - 60);
        const contextXSpan = width * 0.9;
        const contextXMin = width * 0.05;
        contextNodes.forEach((n, i) => {
            const h = idHash(n.id);
            const x = contextXMin + (h % 10000) / 10000 * contextXSpan;
            const y = contextY - ((h >> 16) % 4) * 28;
            positions.set(n.id, { x, y });
        });

        // Collision spread: bucket by grid cell and offset siblings along x
        const buckets = new Map<string, string[]>();
        for (const [id, p] of positions) {
            const key = `${Math.round(p.x / GRID_X)}|${Math.round(p.y / GRID_Y)}`;
            const arr = buckets.get(key) ?? [];
            arr.push(id);
            buckets.set(key, arr);
        }
        for (const ids of buckets.values()) {
            if (ids.length <= 1) continue;
            ids.sort();
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
        nodeRole: 'epic' | 'task';
        visibilityState: VisibilityState;
        isDestination: 0 | 1;
        isInterchange: 0 | 1;
        isOnRoute: 0 | 1;
        routeIds: string;
        nodeSize: number;
        labelSize: number;
        fillColor: string;
        labelColor: string;
        borderColor: string;
        borderWidth: number;
        isCompleted: boolean;
        nodeOpacity: number;
    }

    function getNodeSize(node: GraphNode, isDestination: boolean, isInterchange: boolean, isOnRoute: boolean): number {
        if (!isOnRoute) return 6; // context station (track width)
        const weight = Math.max(0, node.dw || 0);
        const isEpic = getNodeRole(node) === 'epic';
        let base = isEpic ? 18 : 8;
        const maxExtra = isEpic ? 18 : 14;
        const scale = isEpic ? 5.2 : 3.8;
        let size = base + Math.min(maxExtra, Math.log1p(weight) * scale);
        if (isDestination) size *= 2.4;
        else if (isInterchange) size *= 1.3;
        const completedScale = isIncomplete(node) ? 1 : 0.7;
        return Math.round(size * completedScale * 10) / 10;
    }

    function getLabelSize(node: GraphNode, isDestination: boolean, isInterchange: boolean): number {
        if (isDestination) return 13;
        if (isInterchange) return 11;
        const isEpic = getNodeRole(node) === 'epic';
        const base = isEpic ? 9 : 8;
        const maxExtra = isEpic ? 4 : 2;
        return Math.round((base + Math.min(maxExtra, Math.log1p(Math.max(0, node.dw || 0)) * 0.9)) * 10) / 10;
    }

    function getNodeData(
        node: GraphNode,
        routeData: RouteData
    ): NodeData {
        const rs = routeData.routes.get(node.id) ?? new Set();
        const isDestination = routeData.destIndex.has(node.id);
        const isInterchange = !isDestination && rs.size >= 2;
        const isOnRoute = rs.size >= 1;
        const visibilityState = priorityVisibility(node.priority);

        const projectLineColor = getProjectLineColor(node.project);

        const nodeSize = getNodeSize(node, isDestination, isInterchange, isOnRoute);
        const labelSize = getLabelSize(node, isDestination, isInterchange);

        let borderColor = 'rgba(255,255,255,0.18)';
        let borderWidth = 0.9;
        if (isDestination) {
            borderColor = PRIORITY_BORDERS[node.priority] || '#ffffff';
            borderWidth = 4;
        } else if (isInterchange) {
            borderColor = '#ffffff';
            borderWidth = 2.4;
        } else if (node.priority <= 1 && isIncomplete(node)) {
            borderColor = PRIORITY_BORDERS[node.priority] || '#e5e7eb';
            borderWidth = node.priority === 0 ? 2.8 : 2.2;
        }

        const baseOpacity = visibilityState === 'half' ? 0.48 : 0.95;
        const nodeOpacity = isIncomplete(node) ? baseOpacity : baseOpacity * 0.38;

        // Label visibility policy:
        //  - destinations + interchanges: always labelled
        //  - route stations: labelled when priority bright
        //  - context stations: no label at default zoom
        let displayLabel: string;
        if (isDestination || isInterchange) {
            displayLabel = node.label;
        } else if (isOnRoute && visibilityState === 'bright') {
            displayLabel = node.label;
        } else {
            displayLabel = '';
        }

        return {
            id: node.id,
            label: node.label,
            displayLabel,
            nodeType: node.type,
            priority: node.priority,
            nodeRole: getNodeRole(node),
            visibilityState,
            isDestination: isDestination ? 1 : 0,
            isInterchange: isInterchange ? 1 : 0,
            isOnRoute: isOnRoute ? 1 : 0,
            routeIds: Array.from(rs).join(','),
            nodeSize,
            labelSize,
            fillColor: projectLineColor,
            labelColor: projectLineColor,
            borderColor,
            borderWidth,
            isCompleted: !isIncomplete(node),
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

    function getEdgeWidth(edgeRole: string, isOnRoute: boolean): number {
        if (!isOnRoute) return 1;
        if (edgeRole === 'parent') return 7;
        if (edgeRole === 'dependency') return 5;
        return 1;
    }

    // For a route edge, colour by the destination whose route it serves.
    // Single-route: that destination's project colour. Multi-route: the
    // alphabetically-first destination's colour (see spec — full per-route
    // stacking is a follow-up).
    function getEdgeLineColor(
        sourceRoutes: Set<string>,
        targetRoutes: Set<string>,
        destById: Map<string, GraphNode>,
        fallback: string
    ): string {
        const shared: string[] = [];
        for (const r of sourceRoutes) if (targetRoutes.has(r)) shared.push(r);
        if (shared.length === 0) return fallback;
        shared.sort();
        const dest = destById.get(shared[0]);
        return getProjectLineColor(dest?.project);
    }

    // ─── Cytoscape lifecycle ────────────────────────────────────────────────

    function buildGraph() {
        if (!containerEl || !$graphData) return;
        if (cy) { cy.destroy(); cy = null; }

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
        const destById = new Map(routeData.destinations.map(d => [d.id, d]));
        const positions = computePositions(metroNodes, routeData, width, height);

        const cyNodes = metroNodes.map(n => ({
            data: getNodeData(n, routeData),
            position: positions.get(n.id) ?? { x: width / 2, y: height / 2 },
        }));

        const cyEdges = metroEdges.map((edge, index) => {
            const src = typeof edge.source === 'object' ? edge.source.id : edge.source;
            const tgt = typeof edge.target === 'object' ? edge.target.id : edge.target;
            const sourceNode = nodeById.get(src)!;
            const targetNode = nodeById.get(tgt)!;
            const sourceRoutes = routeData.routes.get(src) ?? new Set();
            const targetRoutes = routeData.routes.get(tgt) ?? new Set();
            let shared = 0;
            for (const r of sourceRoutes) if (targetRoutes.has(r)) shared++;
            const isOnRoute = shared >= 1;
            const edgeRole = getEdgeRole(edge.type);
            const sourceVisibility = priorityVisibility(sourceNode.priority);
            const targetVisibility = priorityVisibility(targetNode.priority);
            const visibilityState = getEdgeVisibilityState(sourceVisibility, targetVisibility);
            const fallback = getProjectLineColor(sourceNode.project || targetNode.project);
            const lineColor = isOnRoute
                ? getEdgeLineColor(sourceRoutes, targetRoutes, destById, fallback)
                : '#6b7280';

            return {
                data: {
                    id: `e${index}`,
                    source: src,
                    target: tgt,
                    edgeRole,
                    visibilityState,
                    isOnRoute: isOnRoute ? 1 : 0,
                    lineColor,
                    edgeOpacity: getEdgeOpacity(visibilityState, isOnRoute),
                    edgeWidth: getEdgeWidth(edgeRole, isOnRoute),
                },
            };
        });

        cy = cytoscape({
            container: containerEl,
            elements: [...cyNodes, ...cyEdges],
            style: [
                // Base node styling by role
                {
                    selector: 'node[nodeRole = "epic"][visibilityState != "hidden"]',
                    style: {
                        'shape': 'round-rectangle',
                        'width': 'data(nodeSize)',
                        'height': 'data(nodeSize)',
                        'background-color': 'data(fillColor)',
                        'background-opacity': 0.9,
                        'border-width': 'data(borderWidth)',
                        'border-color': 'data(borderColor)',
                        'opacity': 'data(nodeOpacity)',
                        'label': 'data(displayLabel)',
                        'text-valign': 'top',
                        'text-halign': 'center',
                        'text-margin-y': -6,
                        'font-size': 'data(labelSize)',
                        'font-weight': '600',
                        'color': '#f5f7fb',
                        'text-outline-color': '#0a0a14',
                        'text-outline-width': 2,
                        'text-max-width': '200px',
                        'text-wrap': 'wrap',
                        'min-zoomed-font-size': 6,
                    } as any,
                },
                {
                    selector: 'node[nodeRole = "task"][visibilityState != "hidden"]',
                    style: {
                        'shape': 'ellipse',
                        'width': 'data(nodeSize)',
                        'height': 'data(nodeSize)',
                        'background-color': 'data(fillColor)',
                        'background-opacity': 0.88,
                        'border-width': 'data(borderWidth)',
                        'border-color': 'data(borderColor)',
                        'opacity': 'data(nodeOpacity)',
                        'label': 'data(displayLabel)',
                        'text-valign': 'bottom',
                        'text-halign': 'center',
                        'text-margin-y': 6,
                        'font-size': 'data(labelSize)',
                        'font-weight': '500',
                        'color': 'data(labelColor)',
                        'text-outline-color': '#0a0a14',
                        'text-outline-width': 1.5,
                        'text-max-width': '180px',
                        'text-wrap': 'wrap',
                        'min-zoomed-font-size': 8,
                    } as any,
                },
                // Destinations: label always visible, larger outline glow
                {
                    selector: 'node[isDestination = 1]',
                    style: {
                        'z-index': 9999,
                        'font-size': 'data(labelSize)',
                        'font-weight': '700',
                        'min-zoomed-font-size': 0,
                        'text-outline-width': 3,
                        'text-outline-color': '#000',
                    } as any,
                },
                // Interchanges: label always visible
                {
                    selector: 'node[isInterchange = 1]',
                    style: {
                        'z-index': 500,
                        'min-zoomed-font-size': 0,
                        'font-weight': '600',
                    } as any,
                },
                // Context stations: force small, no label
                {
                    selector: 'node[isOnRoute = 0]',
                    style: {
                        'width': 6,
                        'height': 6,
                        'label': '',
                        'text-opacity': 0,
                        'background-opacity': 0.6,
                        'border-width': 0.5,
                        'opacity': 0.4,
                    } as any,
                },
                {
                    selector: 'node[visibilityState = "half"]',
                    style: {
                        'label': '',
                        'text-opacity': 0,
                    } as any,
                },
                {
                    selector: 'node[visibilityState = "hidden"]',
                    style: { 'display': 'none' } as any,
                },
                // Edges — route edges: thick, semi-transparent, haystack
                {
                    selector: 'edge[isOnRoute = 1][visibilityState != "hidden"]',
                    style: {
                        'width': 'data(edgeWidth)',
                        'line-color': 'data(lineColor)',
                        'opacity': 'data(edgeOpacity)',
                        'curve-style': 'haystack',
                        'haystack-radius': 0,
                    } as any,
                },
                // Dependency route edges keep an arrow
                {
                    selector: 'edge[edgeRole = "dependency"][isOnRoute = 1][visibilityState != "hidden"]',
                    style: {
                        'curve-style': 'bezier', // haystack doesn't render arrows
                        'target-arrow-shape': 'triangle',
                        'target-arrow-color': 'data(lineColor)',
                        'arrow-scale': 0.7,
                        'control-point-step-size': 20,
                    } as any,
                },
                // Non-route edges: thin, grey backdrop
                {
                    selector: 'edge[isOnRoute = 0][visibilityState != "hidden"]',
                    style: {
                        'width': 1,
                        'line-color': '#6b7280',
                        'opacity': 'data(edgeOpacity)',
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
            ],
            layout: { name: 'preset' } as any,
            wheelSensitivity: 0.3,
            minZoom: 0.05,
            maxZoom: 5,
        });

        cy.one('layoutstop', () => { cy?.fit(undefined, 60); running = false; });
        // preset layouts don't always emit layoutstop; fit on next tick as backup
        setTimeout(() => { if (cy) cy.fit(undefined, 60); }, 0);

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
        });

        cy.on('mouseout', 'node', () => {
            selection.update(s => ({ ...s, hoveredNodeId: null }));
            cy!.elements().removeClass('dimmed').removeClass('highlighted');
        });
    }

    // Parent component binds a play/stop control; since this view has no live
    // simulation, toggleRunning recomputes layout and animates nodes to new
    // positions. `running` flips true for the animation window only.
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
    $: if (containerEl && $graphData && $graphStructureKey !== lastStructureKey) {
        lastStructureKey = $graphStructureKey;
        buildGraph();
    }

    // Refresh node data (visibility, labels) when filters change but structure doesn't
    $: if (cy && $graphData && $graphStructureKey === lastStructureKey) {
        const cyInstance = cy;
        const metroNodes = $graphData.nodes.filter(n => !HIDDEN_TYPES.has((n.type || '').toLowerCase()));
        const nodeById = new Map(metroNodes.map(n => [n.id, n]));
        const metroEdges = $graphData.links.filter(e => {
            const src = typeof e.source === 'object' ? e.source.id : e.source;
            const tgt = typeof e.target === 'object' ? e.target.id : e.target;
            return nodeById.has(src) && nodeById.has(tgt);
        });
        const routeData = computeRouteData(metroNodes, metroEdges);
        const destById = new Map(routeData.destinations.map(d => [d.id, d]));

        for (const n of metroNodes) {
            const cyNode = cyInstance.getElementById(n.id);
            if (!cyNode.length) continue;
            Object.entries(getNodeData(n, routeData)).forEach(([k, v]) => cyNode.data(k, v));
        }

        metroEdges.forEach((edge, index) => {
            const src = typeof edge.source === 'object' ? edge.source.id : edge.source;
            const tgt = typeof edge.target === 'object' ? edge.target.id : edge.target;
            const sourceNode = nodeById.get(src);
            const targetNode = nodeById.get(tgt);
            if (!sourceNode || !targetNode) return;
            const cyEdge = cyInstance.getElementById(`e${index}`);
            if (!cyEdge.length) return;
            const sourceRoutes = routeData.routes.get(src) ?? new Set();
            const targetRoutes = routeData.routes.get(tgt) ?? new Set();
            let shared = 0;
            for (const r of sourceRoutes) if (targetRoutes.has(r)) shared++;
            const isOnRoute = shared >= 1;
            const visibilityState = getEdgeVisibilityState(priorityVisibility(sourceNode.priority), priorityVisibility(targetNode.priority));
            const edgeRole = getEdgeRole(edge.type);
            const fallback = getProjectLineColor(sourceNode.project || targetNode.project);
            cyEdge.data('edgeRole', edgeRole);
            cyEdge.data('visibilityState', visibilityState);
            cyEdge.data('isOnRoute', isOnRoute ? 1 : 0);
            cyEdge.data('lineColor', isOnRoute ? getEdgeLineColor(sourceRoutes, targetRoutes, destById, fallback) : '#6b7280');
            cyEdge.data('edgeOpacity', getEdgeOpacity(visibilityState, isOnRoute));
            cyEdge.data('edgeWidth', getEdgeWidth(edgeRole, isOnRoute));
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

<div
    bind:this={containerEl}
    class="w-full h-full bg-background/50"
></div>

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
</style>
