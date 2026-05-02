<script lang="ts">
    import { graphData } from "../../stores/graph";
    import { filters } from "../../stores/filters";
    import { viewSettings } from "../../stores/viewSettings";
    import CytoscapeBase from "../graph/CytoscapeBase.svelte";
    import { computeBaseNodeData, getEdgeLineStyle, getEdgeOpacity, getEdgeWidth } from "../graph/CytoscapeHelpers";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";
    import { toggleSelection } from "../../stores/selection";
    import { projectColor } from "../../data/projectUtils";

    export let running = false;
    export let restartNonce = 0;
    export let randomizeNonce = 0;

    let cyBase: any;
    let elements: any[] = [];
    let layoutOptions: any = { name: "cola" };

    function buildElements(nodes: GraphNode[], edges: GraphEdge[], currentFilters: any, currentSettings: any) {
        let newElements: any[] = [];
        const nodeById = new Map(nodes.map(n => [n.id, n]));
        const keepIds = new Set<string>();

        // 1. Determine active nodes
        let activeNodes = nodes.filter(n => n.type !== 'project');

        if (!currentSettings.colaHandleDisconnected) {
            const childrenOf = new Map<string, Set<string>>();
            for (const n of activeNodes) {
                const pid = (n as any)._safe_parent;
                if (!pid || !nodeById.has(pid)) continue;
                if (!childrenOf.has(pid)) childrenOf.set(pid, new Set());
                childrenOf.get(pid)!.add(n.id);
            }
            for (const [pid, children] of childrenOf.entries()) {
                if (children.size >= 1) {
                    keepIds.add(pid);
                    for (const cid of children) keepIds.add(cid);
                }
            }
            activeNodes = activeNodes.filter(n => keepIds.has(n.id));
        } else {
            activeNodes.forEach(n => keepIds.add(n.id));
        }

        // Apply filters
        activeNodes = activeNodes.filter(n => !currentFilters.hiddenProjects?.includes(n.project));

        const activeIds = new Set(activeNodes.map(n => n.id));

        // 2. Build Nodes (including compound structures)
        activeNodes.forEach(n => {
            const pid = (n as any)._safe_parent;
            const isGroup = activeNodes.some(child => (child as any)._safe_parent === n.id);
            const parent = pid && activeIds.has(pid) ? pid : undefined;
            
            const nodeData = computeBaseNodeData(n, false, true, false, 'bright');
            
            newElements.push({
                data: {
                    ...nodeData,
                    parent,
                    isGroup: isGroup ? 1 : 0,
                    projectColor: n.project ? projectColor(n.project) : '#475569',
                }
            });
        });

        // 3. Build Edges
        const parentIds = new Set(activeNodes.map(n => (n as any)._safe_parent).filter(Boolean));

        edges.forEach((e, idx) => {
            const src = typeof e.source === 'object' ? e.source.id : e.source;
            const tgt = typeof e.target === 'object' ? e.target.id : e.target;
            
            if (!activeIds.has(src) || !activeIds.has(tgt)) return;

            const srcNode = nodeById.get(src);
            const tgtNode = nodeById.get(tgt);
            const srcParent = (srcNode as any)?._safe_parent;
            const tgtParent = (tgtNode as any)?._safe_parent;

            const isIntraGroup = (srcParent && srcParent === tgtParent) ||
                                 (tgtParent === src && !parentIds.has(tgt)) ||
                                 (srcParent === tgt && !parentIds.has(src));

            // Apply edge filters based on type
            if (e.type !== 'parent' && isIntraGroup) return; // Drop non-parent intra-group edges
            
            let vis: any = 'bright';
            if (e.type === 'parent') {
                if (isIntraGroup) {
                    vis = currentFilters.edgeIntraGroup || 'bright';
                    if (vis === 'hidden') return;
                } else {
                    vis = currentFilters.edgeParent;
                    if (vis === 'hidden') return;
                }
            } else if (e.type === 'depends_on') { vis = currentFilters.edgeDependencies; if (vis === 'hidden') return; }
            else if (e.type === 'soft_depends_on') { vis = currentFilters.edgeSoftDependencies; if (vis === 'hidden') return; }
            else if (e.type === 'contributes_to') { vis = currentFilters.edgeContributes; if (vis === 'hidden') return; }
            else if (e.type === 'similar_to') { vis = currentFilters.edgeSimilar; if (vis === 'hidden') return; }
            else if (e.type === 'ref') { vis = currentFilters.edgeReferences; if (vis === 'hidden') return; }

            const { linkColor, linkDash } = getEdgeLineStyle(e.type, isIntraGroup);
            
            newElements.push({
                data: {
                    id: `e_${idx}`,
                    source: src,
                    target: tgt,
                    edgeType: e.type,
                    visibilityState: vis,
                    linkColor,
                    linkDash,
                    edgeOpacity: getEdgeOpacity(vis, true),
                    edgeWidth: getEdgeWidth(true),
                    isIntraGroup: isIntraGroup ? 1 : 0
                }
            });
        });

        return newElements;
    }

    $: if ($graphData) {
        elements = buildElements($graphData.nodes, $graphData.links, $filters, $viewSettings);
    }

    $: layoutOptions = {
        name: "cola",
        animate: true,
        infinite: true,
        randomize: false,
        nodeSpacing: (node: any) => $viewSettings.colaGroupPadding,
        edgeLength: (edge: any) => {
            if (edge.data('edgeType') === 'parent') {
                return edge.data('isIntraGroup') ? $viewSettings.colaLinkDistIntraParent : $viewSettings.colaLinkDistInterParent;
            }
            if (edge.data('edgeType') === 'depends_on') return $viewSettings.colaLinkDistDependsOn;
            return $viewSettings.colaLinkDistRef;
        },
        edgeSymDiffLength: (edge: any) => {
            if (edge.data('edgeType') === 'parent') {
                return edge.data('isIntraGroup') ? $viewSettings.colaLinkWeightIntraParent : $viewSettings.colaLinkWeightInterParent;
            }
            if (edge.data('edgeType') === 'depends_on') return $viewSettings.colaLinkWeightDependsOn;
            return $viewSettings.colaLinkWeightRef;
        },
    };

    $: if (running === false && cyBase) {
        cyBase.stopLayout();
    }

    $: if (restartNonce > 0 && cyBase) {
        cyBase.runLayout();
    }

    export function toggleRunning() {
        if (running) {
            cyBase?.stopLayout();
        } else {
            cyBase?.runLayout();
        }
    }

    export function randomize() {
        layoutOptions = { ...layoutOptions, randomize: true };
        if (cyBase) cyBase.runLayout();
        setTimeout(() => { layoutOptions = { ...layoutOptions, randomize: false }; }, 100);
    }

    function onNodeClick(evt: CustomEvent) {
        toggleSelection(evt.detail.id);
    }

    // Add cytoscape styles specifically for GroupsView (e.g. compound nodes)
    import { getCytoscapeStyles } from "../graph/CytoscapeStyles";
    let stylesheet = [
        ...getCytoscapeStyles(),
        {
            selector: ':parent',
            style: {
                'background-opacity': 0.15,
                'background-color': 'data(projectColor)',
                'border-width': 1.5,
                'border-color': 'data(projectColor)',
                'border-opacity': 0.3,
                'shape': 'roundrectangle',
                'padding': '10px'
            } as any
        }
    ];

</script>

<div class="w-full h-full relative">
    <CytoscapeBase
        bind:this={cyBase}
        {elements}
        {layoutOptions}
        {stylesheet}
        on:nodeClick={onNodeClick}
        on:layoutstart={() => running = true}
        on:layoutstop={() => running = false}
    />
</div>
