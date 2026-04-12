<script lang="ts">
    import { onDestroy } from "svelte";
    import cytoscape from "cytoscape";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { selection, toggleSelection } from "../../stores/selection";
    import { viewSettings } from "../../stores/viewSettings";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";
    import { INCOMPLETE_STATUSES } from "../../data/constants";

    let containerEl: HTMLDivElement;
    let cy: cytoscape.Core | null = null;

    export let running = false;

    function getLayoutConfig() {
        return {
            name: 'cose',
            animate: true,
            boundingBox: {
                x1: 0, y1: 0,
                w: containerEl?.clientWidth || 1200,
                h: containerEl?.clientHeight || 800,
            },
            nodeRepulsion: () => 20000,
            idealEdgeLength: () => $viewSettings.colaLinkLength,
            edgeElasticity: () => 80,
            gravity: 0.5,
            numIter: 1000,
            padding: 60,
            nodeDimensionsIncludeLabels: true,
            nodeOverlap: 12,
            randomize: false,
        };
    }

    export function toggleRunning() {
        if (!cy) return;
        if (running) {
            cy.stop();
            running = false;
        } else {
            const layout = cy.layout(getLayoutConfig() as any);
            layout.on('layoutstop', () => { running = false; });
            layout.run();
            running = true;
        }
    }

    const CONTAINER_TYPES = new Set(['epic', 'project', 'goal']);

    const LINE_PALETTE = [
        '#e6194b', '#3cb44b', '#4363d8', '#f58231', '#911eb4',
        '#42d4f4', '#f032e6', '#bfef45', '#fabed4', '#469990',
        '#dcbeff', '#9A6324', '#fffac8', '#800000', '#aaffc3',
        '#808000', '#ffd8b1', '#000075', '#a9a9a9', '#e6beff',
    ];

    const STATUS_COLORS: Record<string, string> = {
        active: '#4A90D9',
        in_progress: '#4A90D9',
        review: '#5A9AE9',
        waiting: '#3A7AC9',
        decomposing: '#3A7AC9',
        blocked: '#D94A4A',
        ready: '#4AD97A',
        todo: '#4AD97A',
        inbox: '#3AC96A',
        dormant: '#666',
        done: '#888',
        completed: '#888',
        cancelled: '#555',
    };

    const PRIORITY_COLORS: Record<number, string> = {
        0: '#dc3545',
        1: '#f59e0b',
        2: '#6b7280',
        3: '#4b5563',
        4: '#374151',
    };

    /**
     * Extract priority chains: for each P0/P1 incomplete leaf, walk upstream
     * through parent + depends_on edges. Returns the set of node IDs on any
     * priority chain, and a count of lines passing through each node.
     */
    function extractPriorityChains(data: { nodes: GraphNode[], links: GraphEdge[] }) {
        const nodeById = new Map(data.nodes.map(n => [n.id, n]));
        const childrenOf = new Map<string, Set<string>>();
        const parentOf = new Map<string, string>();
        const depsOf = new Map<string, string[]>();

        data.links.forEach(l => {
            const sid = typeof l.source === 'object' ? (l.source as any).id : l.source;
            const tid = typeof l.target === 'object' ? (l.target as any).id : l.target;

            if (l.type === 'parent') {
                // In prepared data parent links are flipped: source=child, target=parent
                parentOf.set(sid, tid);
                if (!childrenOf.has(tid)) childrenOf.set(tid, new Set());
                childrenOf.get(tid)!.add(sid);
            } else if (l.type === 'depends_on') {
                if (!depsOf.has(sid)) depsOf.set(sid, []);
                depsOf.get(sid)!.push(tid);
            }
        });

        // Find P0/P1 incomplete leaves (no incomplete children)
        const priorityLeaves: GraphNode[] = [];
        for (const n of data.nodes) {
            if (n.priority > 1) continue;
            if (!INCOMPLETE_STATUSES.has(n.status)) continue;
            const kids = childrenOf.get(n.id);
            const hasIncompleteChild = kids && [...kids].some(kid => {
                const kn = nodeById.get(kid);
                return kn && INCOMPLETE_STATUSES.has(kn.status);
            });
            if (!hasIncompleteChild) priorityLeaves.push(n);
        }

        // Walk upstream from each leaf through parent + depends_on
        const onPriorityChain = new Set<string>();
        const lineCount = new Map<string, number>();

        for (const leaf of priorityLeaves) {
            const visited = new Set<string>();
            const queue = [leaf.id];
            while (queue.length > 0) {
                const nid = queue.shift()!;
                if (visited.has(nid)) continue;
                visited.add(nid);
                onPriorityChain.add(nid);
                lineCount.set(nid, (lineCount.get(nid) || 0) + 1);

                const pid = parentOf.get(nid);
                if (pid && nodeById.has(pid)) queue.push(pid);
                const deps = depsOf.get(nid) || [];
                for (const dep of deps) {
                    if (nodeById.has(dep)) queue.push(dep);
                }
            }
        }

        return { onPriorityChain, lineCount, priorityLeaves };
    }

    function buildCyElements(data: { nodes: GraphNode[], links: GraphEdge[] }) {
        const { onPriorityChain, lineCount } = extractPriorityChains(data);
        const elements: cytoscape.ElementDefinition[] = [];

        const projectColors = new Map<string, string>();
        let colorIdx = 0;
        const projects = [...new Set(data.nodes.map(n => n.project).filter(Boolean))];
        projects.forEach(p => {
            projectColors.set(p!, LINE_PALETTE[colorIdx % LINE_PALETTE.length]);
            colorIdx++;
        });

        const nodeIdSet = new Set(data.nodes.map(n => n.id));

        for (const n of data.nodes) {
            const isOnChain = onPriorityChain.has(n.id);
            const isInterchange = (lineCount.get(n.id) || 0) >= 2;
            const isCompleted = ['done', 'completed', 'cancelled'].includes(n.status);
            const isContainer = CONTAINER_TYPES.has(n.type);
            const isHighPriority = n.priority <= 1 && INCOMPLETE_STATUSES.has(n.status);

            elements.push({
                group: 'nodes',
                data: {
                    id: n.id,
                    label: n.label || n.id,
                    type: n.type,
                    status: n.status,
                    priority: n.priority,
                    project: n.project || '',
                    lineColor: projectColors.get(n.project || '') || '#666',
                    statusColor: STATUS_COLORS[n.status] || '#666',
                    priorityColor: PRIORITY_COLORS[n.priority] ?? '#4b5563',
                    isContainer,
                    isCompleted,
                    isHighPriority,
                    isOnChain,
                    isInterchange,
                    fullTitle: n.fullTitle,
                    dw: n.dw,
                },
            });
        }

        const edgeIds = new Set<string>();
        data.links.forEach(l => {
            const sid = typeof l.source === 'object' ? (l.source as any).id : l.source;
            const tid = typeof l.target === 'object' ? (l.target as any).id : l.target;
            if (!sid || !tid || !nodeIdSet.has(sid) || !nodeIdSet.has(tid)) return;
            const edgeId = `${sid}-${tid}-${l.type}`;
            if (edgeIds.has(edgeId)) return;
            edgeIds.add(edgeId);

            const bothOnChain = onPriorityChain.has(sid) && onPriorityChain.has(tid);
            const sourceNode = data.nodes.find(n => n.id === sid);

            elements.push({
                group: 'edges',
                data: {
                    id: edgeId,
                    source: sid,
                    target: tid,
                    type: l.type,
                    lineColor: projectColors.get(sourceNode?.project || '') || '#666',
                    isOnChain: bothOnChain,
                },
            });
        });

        return elements;
    }

    function initCytoscape() {
        if (!containerEl || !$graphData) return;
        if (cy) { cy.destroy(); cy = null; }

        const elements = buildCyElements($graphData);

        cy = cytoscape({
            container: containerEl,
            elements,
            style: [
                // Interchange stations — largest, always labeled
                {
                    selector: 'node[?isInterchange]',
                    style: {
                        'shape': 'round-rectangle',
                        'width': 20,
                        'height': 20,
                        'background-color': 'data(statusColor)',
                        'background-opacity': 0.95,
                        'border-width': 2,
                        'border-color': '#fff',
                        'label': 'data(label)',
                        'text-valign': 'top',
                        'text-halign': 'center',
                        'text-margin-y': -6,
                        'font-size': '10px',
                        'font-weight': '500',
                        'color': '#fff',
                        'text-outline-color': '#0a0a14',
                        'text-outline-width': 2,
                        'text-max-width': '180px',
                        'text-wrap': 'wrap',
                        'min-zoomed-font-size': 4,
                        'overlay-color': '#fff',
                        'overlay-padding': 4,
                        'overlay-opacity': 0.12,
                    } as any,
                },
                // High-priority stations on chain — prominent
                {
                    selector: 'node[?isHighPriority][?isOnChain][!isInterchange]',
                    style: {
                        'shape': 'ellipse',
                        'width': 14,
                        'height': 14,
                        'background-color': 'data(statusColor)',
                        'border-width': 1.5,
                        'border-color': 'data(priorityColor)',
                        'label': 'data(label)',
                        'text-valign': 'bottom',
                        'text-halign': 'center',
                        'text-margin-y': 4,
                        'font-size': '8px',
                        'font-weight': '500',
                        'color': 'data(priorityColor)',
                        'text-outline-color': '#0a0a14',
                        'text-outline-width': 1.5,
                        'text-max-width': '140px',
                        'text-wrap': 'wrap',
                        'min-zoomed-font-size': 5,
                        'overlay-color': 'data(priorityColor)',
                        'overlay-padding': 3,
                        'overlay-opacity': 0.12,
                    } as any,
                },
                // On-chain container nodes (epics) — medium with label
                {
                    selector: 'node[?isContainer][?isOnChain][!isInterchange][!isHighPriority]',
                    style: {
                        'shape': 'round-rectangle',
                        'width': 16,
                        'height': 16,
                        'background-color': 'data(lineColor)',
                        'background-opacity': 0.85,
                        'border-width': 1.5,
                        'border-color': '#ccc',
                        'label': 'data(label)',
                        'text-valign': 'top',
                        'text-halign': 'center',
                        'text-margin-y': -6,
                        'font-size': '9px',
                        'font-weight': '500',
                        'color': '#e5e5e5',
                        'text-outline-color': '#0a0a14',
                        'text-outline-width': 1.5,
                        'text-max-width': '160px',
                        'text-wrap': 'wrap',
                        'min-zoomed-font-size': 5,
                    } as any,
                },
                // On-chain non-priority tasks — medium dots, labels on zoom
                {
                    selector: 'node[?isOnChain][!isContainer][!isHighPriority][!isInterchange]',
                    style: {
                        'shape': 'ellipse',
                        'width': 8,
                        'height': 8,
                        'background-color': 'data(statusColor)',
                        'border-width': 1,
                        'border-color': 'data(lineColor)',
                        'label': 'data(label)',
                        'text-valign': 'bottom',
                        'text-halign': 'center',
                        'text-margin-y': 4,
                        'font-size': '8px',
                        'color': '#aaa',
                        'text-outline-color': '#0a0a14',
                        'text-outline-width': 1.5,
                        'text-max-width': '120px',
                        'text-wrap': 'wrap',
                        'min-zoomed-font-size': 7,
                    } as any,
                },
                // Context stations — at or below track width
                {
                    selector: 'node[!isOnChain]',
                    style: {
                        'shape': 'ellipse',
                        'width': 4,
                        'height': 4,
                        'background-color': 'data(statusColor)',
                        'background-opacity': 0.4,
                        'border-width': 1,
                        'border-color': 'data(lineColor)',
                        'border-opacity': 0.3,
                        'label': '',
                        'min-zoomed-font-size': 10,
                        'opacity': 0.4,
                    } as any,
                },
                // Completed — extra dimmed
                {
                    selector: 'node[?isCompleted]',
                    style: {
                        'opacity': 0.25,
                        'width': 3,
                        'height': 3,
                        'border-width': 0.5,
                    } as any,
                },
                // Priority chain parent edges — thick metro lines
                {
                    selector: 'edge[type="parent"][?isOnChain]',
                    style: {
                        'width': 4,
                        'line-color': 'data(lineColor)',
                        'line-opacity': 0.9,
                        'curve-style': 'taxi',
                        'taxi-direction': 'downward',
                        'taxi-turn': '20px',
                    } as any,
                },
                // Priority chain dependency edges — dashed amber
                {
                    selector: 'edge[type="depends_on"][?isOnChain]',
                    style: {
                        'width': 2,
                        'line-color': '#f59e0b',
                        'line-opacity': 0.7,
                        'curve-style': 'taxi',
                        'taxi-direction': 'downward',
                        'taxi-turn': '15px',
                        'line-style': 'dashed',
                        'target-arrow-shape': 'triangle',
                        'target-arrow-color': '#f59e0b',
                        'arrow-scale': 0.6,
                    } as any,
                },
                // Non-priority edges — thin context
                {
                    selector: 'edge[!isOnChain]',
                    style: {
                        'width': 1,
                        'line-color': '#444',
                        'line-opacity': 0.15,
                        'curve-style': 'taxi',
                        'taxi-direction': 'downward',
                        'taxi-turn': '15px',
                    } as any,
                },
                // Non-priority parent edges — slightly more visible
                {
                    selector: 'edge[type="parent"][!isOnChain]',
                    style: {
                        'width': 2,
                        'line-opacity': 0.2,
                    } as any,
                },
                // Reference edges
                {
                    selector: 'edge[type="ref"], edge[type="soft_depends_on"]',
                    style: {
                        'width': 1,
                        'line-color': '#555',
                        'line-opacity': 0.1,
                        'line-style': 'dotted',
                    } as any,
                },
                // Selected node
                {
                    selector: 'node:selected',
                    style: {
                        'border-color': '#fff',
                        'border-width': 4,
                        'overlay-color': '#fff',
                        'overlay-padding': 6,
                        'overlay-opacity': 0.15,
                    } as any,
                },
            ],
            layout: getLayoutConfig() as any,
            wheelSensitivity: 0.3,
            minZoom: 0.03,
            maxZoom: 5,
        });

        cy.one('layoutstop', () => { cy!.fit(undefined, 50); });

        cy.on('tap', 'node', (evt) => { toggleSelection(evt.target.id()); });
        cy.on('tap', (evt) => { if (evt.target === cy) toggleSelection(''); });

        cy.on('mouseover', 'node', (evt) => {
            selection.update(s => ({ ...s, hoveredNodeId: evt.target.id() }));
            const hood = evt.target.neighborhood().add(evt.target);
            cy!.elements().not(hood).addClass('dimmed');
            hood.addClass('highlighted');
        });

        cy.on('mouseout', 'node', () => {
            selection.update(s => ({ ...s, hoveredNodeId: null }));
            cy!.elements().removeClass('dimmed').removeClass('highlighted');
        });
    }

    $: if (cy && $selection) {
        const activeId = $selection.activeNodeId;
        cy.nodes().unselect();
        if (activeId) {
            const node = cy.getElementById(activeId);
            if (node.length) node.select();
        }
    }

    let lastMetroStructureKey = '';
    let lastColaLinkLength = 0;
    $: if (containerEl && $graphData && ($graphStructureKey !== lastMetroStructureKey || $viewSettings.colaLinkLength !== lastColaLinkLength)) {
        lastMetroStructureKey = $graphStructureKey;
        lastColaLinkLength = $viewSettings.colaLinkLength;
        initCytoscape();
    }

    $: if (cy && $graphData && $graphStructureKey === lastMetroStructureKey) {
        for (const n of $graphData.nodes) {
            const cyNode = cy.getElementById(n.id);
            if (!cyNode.length) continue;
            cyNode.data('statusColor', STATUS_COLORS[n.status] || '#666');
            cyNode.data('isCompleted', ['done', 'completed', 'cancelled'].includes(n.status));
            cyNode.data('isHighPriority', n.priority <= 1 && INCOMPLETE_STATUSES.has(n.status));
            cyNode.data('status', n.status);
            cyNode.data('priority', n.priority);
        }
    }

    onDestroy(() => { if (cy) { cy.destroy(); cy = null; } });
</script>

<div
    bind:this={containerEl}
    class="w-full h-full relative"
    style="background: #0a0a14;"
    data-component="metro-map"
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
</style>
