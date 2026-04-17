<script lang="ts">
    import { onDestroy } from "svelte";
    import cytoscape from "cytoscape";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { filters, type VisibilityState } from "../../stores/filters";
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
            nodeRepulsion: () => 60000,
            idealEdgeLength: () => Math.max(220, $viewSettings.colaLinkLength * 0.5),
            edgeElasticity: () => 120,
            gravity: 0.28,
            numIter: 1000,
            padding: 100,
            nodeDimensionsIncludeLabels: true,
            nodeOverlap: 24,
            randomize: true,
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

    function getLineColor(nodeId: string): string {
        let hash = 0;
        for (let i = 0; i < nodeId.length; i++) {
            hash = nodeId.charCodeAt(i) + ((hash << 5) - hash);
        }
        return LINE_PALETTE[Math.abs(hash) % LINE_PALETTE.length];
    }

    function priorityVisibility(priority: number | undefined): VisibilityState {
        if (priority === 0) return $filters.priority0;
        if (priority === 1) return $filters.priority1;
        if (priority === 2) return $filters.priority2;
        if (priority === 3) return $filters.priority3;
        return $filters.priority4;
    }

    function isMetroChainNode(node: GraphNode): boolean {
        const vis = priorityVisibility(node.priority);
        if (vis === 'hidden') return false;

        const isContainer = CONTAINER_TYPES.has(node.type.toLowerCase());
        if (isContainer) {
            return true;
        }

        if (node.priority <= 1 && INCOMPLETE_STATUSES.has(node.status)) {
            return true;
        }

        return vis === 'bright';
    }

    function initCytoscape() {
        if (!containerEl || !$graphData) return;

        if (cy) cy.destroy();

        const cyNodes = $graphData.nodes.map((n: any) => {
            const isInterchange = CONTAINER_TYPES.has(n.type.toLowerCase()) && (n.totalLeafCount > 5 || n.dw > 10);
            const priorityVis = priorityVisibility(n.priority);
            const groupVisibility = CONTAINER_TYPES.has(n.type.toLowerCase()) ? priorityVis : 'bright';
            const isOnChain = isMetroChainNode(n);

            return {
                data: {
                    id: n.id,
                    label: n.label,
                    nodeType: n.type,
                    status: n.status,
                    priority: n.priority,
                    isInterchange,
                    isOnChain,
                    isContainer: CONTAINER_TYPES.has(n.type.toLowerCase()),
                    priorityVisibility: priorityVis,
                    groupVisibility,
                    isHighPriority: n.priority <= 1 && INCOMPLETE_STATUSES.has(n.status),
                    isCompleted: !INCOMPLETE_STATUSES.has(n.status),
                    statusColor: n.fill,
                    priorityColor: n.borderColor,
                    lineColor: getLineColor(n.project || 'default')
                },
            };
        });

        const cyEdges = $graphData.links.map((l: any, i: number) => {
            const source = typeof l.source === 'object' ? l.source.id : l.source;
            const target = typeof l.target === 'object' ? l.target.id : l.target;
            const sNode = $graphData.nodes.find(n => n.id === source);
            const tNode = $graphData.nodes.find(n => n.id === target);
            const isChainEdge = (sNode ? isMetroChainNode(sNode) : false) || (tNode ? isMetroChainNode(tNode) : false);

            return {
                data: {
                    id: `e${i}`,
                    source,
                    target,
                    type: l.type,
                    isOnChain: isChainEdge,
                    lineColor: getLineColor(sNode?.project || 'default')
                }
            };
        });

        cy = cytoscape({
            container: containerEl,
            elements: [...cyNodes, ...cyEdges],
            style: [
                {
                    selector: 'node[?isInterchange][groupVisibility = "bright"]',
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
                {
                    selector: 'node[?isContainer][groupVisibility = "bright"][!isInterchange][!isHighPriority]',
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
                {
                    selector: 'node[?isContainer][groupVisibility = "half"]',
                    style: {
                        'shape': 'ellipse',
                        'width': 8,
                        'height': 8,
                        'background-color': 'data(lineColor)',
                        'background-opacity': 0.7,
                        'border-width': 1,
                        'border-color': '#ccc',
                        'label': '',
                        'text-opacity': 0,
                        'min-zoomed-font-size': 99,
                        'opacity': 0.9,
                    } as any,
                },
                {
                    selector: 'node[?isContainer][groupVisibility = "hidden"]',
                    style: {
                        'display': 'none',
                        'label': '',
                    } as any,
                },
                {
                    selector: 'node[!isContainer][priorityVisibility = "bright"][?isOnChain][!isHighPriority][!isInterchange]',
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
                {
                    selector: 'node[!isContainer][priorityVisibility = "half"]',
                    style: {
                        'shape': 'ellipse',
                        'width': 5,
                        'height': 5,
                        'background-color': 'data(statusColor)',
                        'background-opacity': 0.55,
                        'border-width': 1,
                        'border-color': 'data(lineColor)',
                        'border-opacity': 0.45,
                        'label': '',
                        'opacity': 0.55,
                    } as any,
                },
                {
                    selector: 'node[!isOnChain][!isContainer][priorityVisibility != "half"]',
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
                {
                    selector: 'node[?isCompleted]',
                    style: {
                        'opacity': 0.25,
                        'width': 3,
                        'height': 3,
                        'border-width': 0.5,
                    } as any,
                },
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
                {
                    selector: 'edge[!isOnChain]',
                    style: {
                        'width': 1,
                        'line-color': '#444',
                        'line-opacity': 0.3,
                        'curve-style': 'haystack'
                    } as any,
                },
                {
                    selector: ':selected',
                    style: {
                        'border-width': 4,
                        'border-color': '#fff',
                        'border-opacity': 0.8,
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

        cy.one('layoutstop', () => { cy!.fit(undefined, 50); running = false; });

        cy.on('tap', 'node', (evt) => { toggleSelection(evt.target.id()); });

        cy.on('mouseover', 'node', (evt) => {
            const node = evt.target;
            const id = node.id();
            selection.update(s => ({ ...s, hoveredNodeId: id }));

            // Dim everything else
            cy!.elements().addClass('dimmed');
            node.removeClass('dimmed').addClass('highlighted');
            node.neighborhood().removeClass('dimmed').addClass('highlighted');
        });

        cy.on('mouseout', 'node', () => {
            selection.update(s => ({ ...s, hoveredNodeId: null }));
            cy!.elements().removeClass('dimmed').removeClass('highlighted');
        });
    }

    // Sync selection from store to Cytoscape
    $: if (cy && $selection.activeNodeId) {
        cy.nodes().unselect();
        const node = cy.getElementById($selection.activeNodeId);
        if (node.length) node.select();
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
            const priorityVis = priorityVisibility(n.priority);
            const groupVisibility = CONTAINER_TYPES.has(n.type.toLowerCase()) ? priorityVisibility(n.priority) : 'bright';
            cyNode.data('statusColor', n.fill);
            cyNode.data('priorityVisibility', priorityVis);
            cyNode.data('groupVisibility', groupVisibility);
            cyNode.data('isOnChain', isMetroChainNode(n));
            cyNode.data('isCompleted', !INCOMPLETE_STATUSES.has(n.status));
            cyNode.data('isHighPriority', n.priority <= 1 && INCOMPLETE_STATUSES.has(n.status));
            cyNode.data('status', n.status);
            cyNode.data('priority', n.priority);
        }

        for (const edge of $graphData.links) {
            const source = typeof edge.source === 'object' ? edge.source.id : edge.source;
            const target = typeof edge.target === 'object' ? edge.target.id : edge.target;
            const cyEdge = cy.getElementById(`e${$graphData.links.indexOf(edge)}`);
            if (!cyEdge.length) continue;

            const sNode = $graphData.nodes.find(n => n.id === source);
            const tNode = $graphData.nodes.find(n => n.id === target);
            cyEdge.data('isOnChain', (sNode ? isMetroChainNode(sNode) : false) || (tNode ? isMetroChainNode(tNode) : false));
        }
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
</style>
