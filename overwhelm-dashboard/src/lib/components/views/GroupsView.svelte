<script lang="ts">
    import { graphData } from "../../stores/graph";
    import { filters } from "../../stores/filters";
    import { viewSettings } from "../../stores/viewSettings";
    import CytoscapeBase from "../graph/CytoscapeBase.svelte";
    import { 
        computeBaseNodeData, 
        getEdgeLineStyle,
        getEdgeOpacity,
        getEdgeWidth,
    } from "../graph/CytoscapeHelpers";
    import { getEdgeTypeDef } from "../../data/taxonomy";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";
    import { toggleSelection } from "../../stores/selection";
    import { projectColor } from "../../data/projectUtils";

    export let running = false;
    export let restartNonce = 0;
    export let randomizeNonce = 0;

    let cyBase: any;
    let elements: any[] = [];
    let layoutOptions: any = { name: "cola" };

    function buildElements(nodes: GraphNode[], edges: GraphEdge[], currentFilters: any) {
        let newElements: any[] = [];
        const nodeById = new Map(nodes.map(n => [n.id, n]));
        const keepIds = new Set<string>();

        // 1. Determine active nodes
        let activeNodes = nodes.filter(n => n.type !== 'project');

        if (!$viewSettings.colaHandleDisconnected) {
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
            
            const def = getEdgeTypeDef(e.type, isIntraGroup);

            let vis: any = "bright";
            if (def.filterKey) {
                vis = currentFilters[def.filterKey] || "bright";
            }
            if (vis === 'hidden') return;
            
            newElements.push({
                data: {
                    id: `e_${idx}`,
                    source: src,
                    target: tgt,
                    edgeType: e.type,
                    visibilityState: vis,
                    linkColor: def.color,
                    linkDash: def.dashStyle,
                    edgeOpacity: getEdgeOpacity(vis, true),
                    edgeWidth: getEdgeWidth(true),
                    isIntraGroup: isIntraGroup ? 1 : 0
                }
            });
        });

        return newElements;
    }

    $: if ($graphData && $filters) {
        // Rebuild elements when graph structure or filters change, not physics
        elements = buildElements($graphData.nodes, $graphData.links, $filters);
        setTimeout(() => cyBase?.fit(), 100);
    }

    $: layoutOptions = {
        name: "cola",
        animate: true,
        refresh: 3,
        infinite: true,
        fit: false,
        randomize: false,
        nodeSpacing: (node: any) => $viewSettings.colaGroupPadding,
        edgeLength: (edge: any) => {
            const edgeType = edge.data("edgeType");
            const def = getEdgeTypeDef(edgeType, false);
            return $viewSettings[def.distKey];
        },
        edgeSymDiffLength: (edge: any) => {
            const edgeType = edge.data("edgeType");
            const def = getEdgeTypeDef(edgeType, false);
            return $viewSettings[def.weightKey];
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

    export function fit() {
        if (cyBase) cyBase.fit();
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
