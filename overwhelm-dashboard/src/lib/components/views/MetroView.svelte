<script lang="ts">
    import { onDestroy } from "svelte";
    import cytoscape from "cytoscape";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { selection, toggleSelection } from "../../stores/selection";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";

    let containerEl: HTMLDivElement;
    let cy: cytoscape.Core | null = null;

    // Status → node background color
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
        deferred: '#777',
        paused: '#999',
    };

    // Priority → border color
    const PRIORITY_COLORS: Record<number, string> = {
        [-1]: '#e11d48',
        0: '#f59e0b',
        1: '#d97706',
        2: '#6b7280',
        3: '#4b5563',
        4: '#374151',
    };

    // Metro line colors — each project gets a distinct vivid line
    const LINE_PALETTE = [
        '#e6194b', '#3cb44b', '#4363d8', '#f58231', '#911eb4',
        '#42d4f4', '#f032e6', '#bfef45', '#fabed4', '#469990',
        '#dcbeff', '#9A6324', '#fffac8', '#800000', '#aaffc3',
        '#808000', '#ffd8b1', '#000075', '#a9a9a9', '#e6beff',
    ];

    const CONTAINER_TYPES = new Set(['epic', 'project', 'goal']);

    function buildCyElements(data: { nodes: GraphNode[], links: GraphEdge[] }) {
        const elements: cytoscape.ElementDefinition[] = [];
        const projectColors = new Map<string, string>();
        let colorIdx = 0;

        // Assign line colors to projects
        const projects = [...new Set(data.nodes.map(n => n.project).filter(Boolean))];
        projects.forEach(p => {
            projectColors.set(p!, LINE_PALETTE[colorIdx % LINE_PALETTE.length]);
            colorIdx++;
        });

        const nodeIdSet = new Set(data.nodes.map(n => n.id));

        // ALL nodes are flat — no compound parents
        data.nodes.forEach(n => {
            const lineColor = projectColors.get(n.project || '') || '#666';
            const statusColor = STATUS_COLORS[n.status] || '#666';
            const priorityColor = PRIORITY_COLORS[n.priority] ?? '#4b5563';
            const isCompleted = ['done', 'completed', 'cancelled'].includes(n.status);
            const isContainer = CONTAINER_TYPES.has(n.type);

            elements.push({
                group: 'nodes',
                data: {
                    id: n.id,
                    label: n.label || n.id,
                    type: n.type,
                    status: n.status,
                    priority: n.priority,
                    project: n.project || '',
                    lineColor,
                    statusColor,
                    priorityColor,
                    isContainer,
                    isCompleted,
                    fullTitle: n.fullTitle,
                },
            });
        });

        // Add edges — parent edges become visible "metro line" connections
        const edgeIds = new Set<string>();
        data.links.forEach(l => {
            const sid = typeof l.source === 'object' ? (l.source as any).id : l.source;
            const tid = typeof l.target === 'object' ? (l.target as any).id : l.target;
            if (!sid || !tid) return;
            if (!nodeIdSet.has(sid) || !nodeIdSet.has(tid)) return;
            const edgeId = `${sid}-${tid}-${l.type}`;
            if (edgeIds.has(edgeId)) return;
            edgeIds.add(edgeId);

            const lineColor = projectColors.get(
                data.nodes.find(n => n.id === sid)?.project || ''
            ) || '#666';

            elements.push({
                group: 'edges',
                data: {
                    id: edgeId,
                    source: sid,
                    target: tid,
                    type: l.type,
                    lineColor,
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
            ready: function() { (window as any).__cy = this; },
            style: [
                // Container-type nodes (epics, projects, goals) — larger interchange stations
                {
                    selector: 'node[?isContainer]',
                    style: {
                        'shape': 'round-rectangle',
                        'width': 30,
                        'height': 30,
                        'background-color': 'data(lineColor)',
                        'background-opacity': 0.9,
                        'border-width': 3,
                        'border-color': '#fff',
                        'label': 'data(label)',
                        'text-valign': 'top',
                        'text-halign': 'center',
                        'text-margin-y': -8,
                        'font-size': '11px',
                        'font-weight': 'bold',
                        'color': '#e5e5e5',
                        'text-outline-color': '#0a0a14',
                        'text-outline-width': 2,
                        'text-max-width': '180px',
                        'text-wrap': 'wrap',
                        'min-zoomed-font-size': 6,
                    } as any,
                },
                // Leaf station nodes — small circles
                {
                    selector: 'node[!isContainer]',
                    style: {
                        'shape': 'ellipse',
                        'width': 14,
                        'height': 14,
                        'background-color': 'data(statusColor)',
                        'border-width': 2,
                        'border-color': 'data(lineColor)',
                        'label': 'data(label)',
                        'text-valign': 'center',
                        'text-halign': 'right',
                        'text-margin-x': 6,
                        'font-size': '9px',
                        'color': '#d4d4d8',
                        'text-outline-color': '#0a0a14',
                        'text-outline-width': 1.5,
                        'text-max-width': '140px',
                        'text-wrap': 'ellipsis',
                        'min-zoomed-font-size': 7,
                    } as any,
                },
                // Completed stations — dimmed and smaller
                {
                    selector: 'node[?isCompleted]',
                    style: {
                        'opacity': 0.35,
                        'width': 8,
                        'height': 8,
                        'border-width': 1,
                        'font-size': '7px',
                    } as any,
                },
                // High-priority stations — larger with bold border
                {
                    selector: 'node[priority <= 0][!isContainer]',
                    style: {
                        'width': 20,
                        'height': 20,
                        'border-width': 3,
                        'font-size': '10px',
                        'font-weight': 'bold',
                    } as any,
                },
                // Parent edges — thick metro lines in project color (width ≈ node size)
                {
                    selector: 'edge[type="parent"]',
                    style: {
                        'width': 12,
                        'line-color': 'data(lineColor)',
                        'line-opacity': 0.85,
                        'curve-style': 'taxi',
                        'taxi-direction': 'downward',
                        'taxi-turn': '20px',
                    } as any,
                },
                // Dependency edges — thinner dashed transfer lines
                {
                    selector: 'edge[type="depends_on"], edge[type="soft_depends_on"]',
                    style: {
                        'width': 4,
                        'line-color': '#f59e0b',
                        'line-opacity': 0.5,
                        'curve-style': 'taxi',
                        'taxi-direction': 'downward',
                        'taxi-turn': '25px',
                        'line-style': 'dashed',
                        'target-arrow-shape': 'triangle',
                        'target-arrow-color': '#f59e0b',
                        'arrow-scale': 0.7,
                    } as any,
                },
                // Reference edges — dotted subtle
                {
                    selector: 'edge[type="ref"]',
                    style: {
                        'width': 1,
                        'line-color': '#6b7280',
                        'line-opacity': 0.25,
                        'curve-style': 'taxi',
                        'taxi-direction': 'downward',
                        'taxi-turn': '15px',
                        'line-style': 'dotted',
                        'target-arrow-shape': 'triangle',
                        'target-arrow-color': '#6b7280',
                        'arrow-scale': 0.5,
                    } as any,
                },
                // Selected node highlight
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
            layout: {
                name: 'cose',
                animate: false,
                boundingBox: {
                    x1: 0, y1: 0,
                    w: containerEl.clientWidth || 1200,
                    h: containerEl.clientHeight || 800,
                },
                nodeRepulsion: () => 12000,
                idealEdgeLength: () => 60,
                edgeElasticity: () => 80,
                gravity: 1.0,
                numIter: 600,
                padding: 30,
                nodeDimensionsIncludeLabels: true,
                nodeOverlap: 10,
                randomize: false,
            } as any,
            wheelSensitivity: 0.3,
            minZoom: 0.05,
            maxZoom: 5,
        });

        // Fit after layout settles
        cy.one('layoutstop', () => {
            cy!.fit(undefined, 40);
        });

        // Wire up selection
        cy.on('tap', 'node', (evt) => {
            toggleSelection(evt.target.id());
        });

        cy.on('tap', (evt) => {
            if (evt.target === cy) toggleSelection('');
        });

        // Hover flashlight
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

    // React to selection changes from outside
    $: if (cy && $selection) {
        const activeId = $selection.activeNodeId;
        cy.nodes().unselect();
        if (activeId) {
            const node = cy.getElementById(activeId);
            if (node.length) node.select();
        }
    }

    // Only rebuild the full Cytoscape graph when the structure changes, not on property-only updates
    let lastMetroStructureKey = '';
    $: if (containerEl && $graphData && $graphStructureKey !== lastMetroStructureKey) {
        lastMetroStructureKey = $graphStructureKey;
        initCytoscape();
    }

    // Property-only updates — patch node styles without rebuilding layout
    $: if (cy && $graphData && $graphStructureKey === lastMetroStructureKey) {
        for (const n of $graphData.nodes) {
            const cyNode = cy.getElementById(n.id);
            if (!cyNode.length) continue;
            const statusColor = STATUS_COLORS[n.status] || '#666';
            const isCompleted = ['done', 'completed', 'cancelled'].includes(n.status);
            cyNode.data('statusColor', statusColor);
            cyNode.data('isCompleted', isCompleted);
            cyNode.data('status', n.status);
        }
    }

    onDestroy(() => {
        if (cy) { cy.destroy(); cy = null; }
    });
</script>

<div
    bind:this={containerEl}
    class="w-full h-full"
    style="background: #0a0a14;"
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
