<script lang="ts">
    import { onDestroy } from "svelte";
    import cytoscape from "cytoscape";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { filters, type VisibilityState } from "../../stores/filters";
    import { selection, toggleSelection } from "../../stores/selection";
    import { viewSettings } from "../../stores/viewSettings";
    import type { GraphNode } from "../../data/prepareGraphData";
    import { INCOMPLETE_STATUSES, PRIORITY_BORDERS } from "../../data/constants";
    import { projectColor } from "../../data/projectUtils";

    let containerEl: HTMLDivElement;
    let cy: cytoscape.Core | null = null;

    export let running = false;

    const HIDDEN_TYPES = new Set(['project']);
    const EPIC_TYPES = new Set(['epic', 'goal']);
    const DEFAULT_PROJECT_COLOR = 'hsl(220, 12%, 46%)';

    function getLayoutConfig(options: { animate?: boolean; randomize?: boolean } = {}) {
        const { animate = false, randomize = true } = options;

        return {
            name: 'cose',
            animate,
            animationDuration: animate ? 900 : 0,
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
            fit: false,
            randomize,
        };
    }

    export function toggleRunning() {
        if (!cy) return;
        if (running) {
            cy.stop();
            running = false;
        } else {
            const layout = cy.layout(getLayoutConfig({ animate: true, randomize: false }) as any);
            layout.on('layoutstop', () => { running = false; });
            layout.run();
            running = true;
        }
    }

    function priorityVisibility(priority: number | undefined): VisibilityState {
        if (priority === 0) return $filters.priority0;
        if (priority === 1) return $filters.priority1;
        if (priority === 2) return $filters.priority2;
        if (priority === 3) return $filters.priority3;
        return $filters.priority4;
    }

    function getProjectLineColor(project: string | null | undefined) {
        return project ? projectColor(project) : DEFAULT_PROJECT_COLOR;
    }

    function isIncomplete(node: GraphNode) {
        return INCOMPLETE_STATUSES.has(node.status);
    }

    function isPriorityBordered(node: GraphNode) {
        return node.priority <= 1 && isIncomplete(node);
    }

    function getNodeRole(node: GraphNode) {
        return EPIC_TYPES.has(node.type.toLowerCase()) ? 'epic' : 'task';
    }

    function getEdgeRole(edgeType: string) {
        if (edgeType === 'parent') return 'parent';
        if (edgeType === 'depends_on') return 'dependency';
        return 'reference';
    }

    function getNodeSize(node: GraphNode) {
        const weight = Math.max(0, node.dw || 0);
        const isEpic = getNodeRole(node) === 'epic';
        const base = isEpic ? 18 : 6;
        const maxExtra = isEpic ? 18 : 12;
        const scale = isEpic ? 5.2 : 3.8;
        const size = base + Math.min(maxExtra, Math.log1p(weight) * scale);
        const completedScale = isIncomplete(node) ? 1 : 0.72;
        return Math.round(size * completedScale * 10) / 10;
    }

    function getDimmedNodeSize(node: GraphNode) {
        return Math.max(4, Math.round(getNodeSize(node) * 0.62 * 10) / 10);
    }

    function getLabelSize(node: GraphNode) {
        const isEpic = getNodeRole(node) === 'epic';
        const base = isEpic ? 9 : 8;
        const maxExtra = isEpic ? 4 : 2;
        return Math.round((base + Math.min(maxExtra, Math.log1p(Math.max(0, node.dw || 0)) * 0.9)) * 10) / 10;
    }

    function getNodeOpacity(node: GraphNode, visibilityState: VisibilityState) {
        const visibilityOpacity = visibilityState === 'half' ? 0.48 : 0.95;
        return isIncomplete(node) ? visibilityOpacity : visibilityOpacity * 0.38;
    }

    function getNodeData(node: GraphNode) {
        const visibilityState = priorityVisibility(node.priority);
        const projectLineColor = getProjectLineColor(node.project);

        return {
            id: node.id,
            label: node.label,
            nodeType: node.type,
            priority: node.priority,
            nodeRole: getNodeRole(node),
            visibilityState,
            nodeSize: getNodeSize(node),
            dimmedNodeSize: getDimmedNodeSize(node),
            labelSize: getLabelSize(node),
            fillColor: projectLineColor,
            labelColor: projectLineColor,
            borderColor: isPriorityBordered(node) ? (PRIORITY_BORDERS[node.priority] || '#e5e7eb') : 'rgba(255,255,255,0.18)',
            borderWidth: isPriorityBordered(node) ? (node.priority === 0 ? 2.8 : 2.2) : 0.9,
            isCompleted: !isIncomplete(node),
            displayLabel: visibilityState === 'bright' ? node.label : '',
            nodeOpacity: getNodeOpacity(node, visibilityState),
        };
    }

    function getEdgeVisibilityState(sourceVisibility: VisibilityState, targetVisibility: VisibilityState): VisibilityState {
        if (sourceVisibility === 'hidden' || targetVisibility === 'hidden') return 'hidden';
        if (sourceVisibility === 'half' || targetVisibility === 'half') return 'half';
        return 'bright';
    }

    function getEdgeOpacity(visibilityState: VisibilityState, role: string) {
        const base = role === 'parent' ? 0.78 : role === 'dependency' ? 0.62 : 0.32;
        if (visibilityState === 'half') return base * 0.42;
        return base;
    }

    function initCytoscape() {
        if (!containerEl || !$graphData) return;

        if (cy) cy.destroy();

        const metroNodes = $graphData.nodes.filter((node) => !HIDDEN_TYPES.has(node.type.toLowerCase()));
        const nodeById = new Map(metroNodes.map((node) => [node.id, node]));

        const cyNodes = metroNodes.map((node) => ({ data: getNodeData(node) }));

        const cyEdges = $graphData.links
            .map((edge, index) => ({ edge, index }))
            .filter(({ edge }) => {
                const source = typeof edge.source === 'object' ? edge.source.id : edge.source;
                const target = typeof edge.target === 'object' ? edge.target.id : edge.target;
                return nodeById.has(source) && nodeById.has(target);
            })
            .map(({ edge, index }) => {
                const source = typeof edge.source === 'object' ? edge.source.id : edge.source;
                const target = typeof edge.target === 'object' ? edge.target.id : edge.target;
                const sourceNode = nodeById.get(source)!;
                const targetNode = nodeById.get(target)!;
                const sourceVisibility = priorityVisibility(sourceNode.priority);
                const targetVisibility = priorityVisibility(targetNode.priority);
                const edgeRole = getEdgeRole(edge.type);
                const visibilityState = getEdgeVisibilityState(sourceVisibility, targetVisibility);

                return {
                    data: {
                        id: `e${index}`,
                        source,
                        target,
                        edgeRole,
                        visibilityState,
                        lineColor: getProjectLineColor(sourceNode.project || targetNode.project),
                        edgeOpacity: getEdgeOpacity(visibilityState, edgeRole),
                    },
                };
            });

        cy = cytoscape({
            container: containerEl,
            elements: [...cyNodes, ...cyEdges],
            style: [
                {
                    selector: 'node[nodeRole = "epic"][visibilityState != "hidden"]',
                    style: {
                        'shape': 'rectangle',
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
                        'text-max-width': '180px',
                        'text-wrap': 'wrap',
                        'min-zoomed-font-size': 5,
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
                        'text-margin-y': 4,
                        'font-size': 'data(labelSize)',
                        'font-weight': '500',
                        'color': 'data(labelColor)',
                        'text-outline-color': '#0a0a14',
                        'text-outline-width': 1.5,
                        'text-max-width': '160px',
                        'text-wrap': 'wrap',
                        'min-zoomed-font-size': 6,
                    } as any,
                },
                {
                    selector: 'node[visibilityState = "half"]',
                    style: {
                        'width': 'data(dimmedNodeSize)',
                        'height': 'data(dimmedNodeSize)',
                        'label': '',
                        'text-opacity': 0,
                    } as any,
                },
                {
                    selector: 'node[visibilityState = "hidden"]',
                    style: {
                        'display': 'none',
                    } as any,
                },
                {
                    selector: 'edge[edgeRole = "parent"][visibilityState != "hidden"]',
                    style: {
                        'width': 2.8,
                        'line-color': 'data(lineColor)',
                        'opacity': 'data(edgeOpacity)',
                        'curve-style': 'taxi',
                        'taxi-direction': 'downward',
                        'taxi-turn': '18px',
                    } as any,
                },
                {
                    selector: 'edge[edgeRole = "dependency"][visibilityState != "hidden"]',
                    style: {
                        'width': 1.9,
                        'line-color': '#f59e0b',
                        'opacity': 'data(edgeOpacity)',
                        'curve-style': 'taxi',
                        'taxi-direction': 'downward',
                        'taxi-turn': '14px',
                        'line-style': 'dashed',
                        'target-arrow-shape': 'triangle',
                        'target-arrow-color': '#f59e0b',
                        'arrow-scale': 0.55,
                    } as any,
                },
                {
                    selector: 'edge[edgeRole = "reference"][visibilityState != "hidden"]',
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
                    style: {
                        'display': 'none',
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
            layout: getLayoutConfig({ animate: false, randomize: true }) as any,
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

            cy!.elements().addClass('dimmed');
            node.removeClass('dimmed').addClass('highlighted');
            node.neighborhood().removeClass('dimmed').addClass('highlighted');
        });

        cy.on('mouseout', 'node', () => {
            selection.update(s => ({ ...s, hoveredNodeId: null }));
            cy!.elements().removeClass('dimmed').removeClass('highlighted');
        });
    }

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
        const cyInstance = cy;
        const metroNodes = $graphData.nodes.filter((node) => !HIDDEN_TYPES.has(node.type.toLowerCase()));
        const nodeById = new Map(metroNodes.map((node) => [node.id, node]));

        for (const node of metroNodes) {
            const cyNode = cyInstance.getElementById(node.id);
            if (!cyNode.length) continue;

            Object.entries(getNodeData(node)).forEach(([key, value]) => {
                cyNode.data(key, value);
            });
        }

        $graphData.links.forEach((edge, index) => {
            const source = typeof edge.source === 'object' ? edge.source.id : edge.source;
            const target = typeof edge.target === 'object' ? edge.target.id : edge.target;
            const cyEdge = cyInstance.getElementById(`e${index}`);
            if (!cyEdge.length) return;

            const sourceNode = nodeById.get(source);
            const targetNode = nodeById.get(target);
            if (!sourceNode || !targetNode) return;

            const visibilityState = getEdgeVisibilityState(priorityVisibility(sourceNode.priority), priorityVisibility(targetNode.priority));
            const edgeRole = getEdgeRole(edge.type);
            cyEdge.data('edgeRole', edgeRole);
            cyEdge.data('visibilityState', visibilityState);
            cyEdge.data('lineColor', getProjectLineColor(sourceNode.project || targetNode.project));
            cyEdge.data('edgeOpacity', getEdgeOpacity(visibilityState, edgeRole));
        });
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
