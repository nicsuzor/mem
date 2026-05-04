<script lang="ts">
    import { onDestroy, onMount } from "svelte";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { filters } from "../../stores/filters";
    import { viewSettings } from "../../stores/viewSettings";
    import { get } from "svelte/store";
    import CytoscapeBase from "../graph/CytoscapeBase.svelte";
    import {
        computeBaseNodeData,
        getEdgeVisibilityState,
        getEdgeLineStyle,
        getEdgeOpacity,
        getEdgeWidth,
        applyEpicGrouping,
    } from "../graph/CytoscapeHelpers";
    import { EDGE_TYPES, getEdgeTypeDef } from "../../data/taxonomy";

    // Canonical edge types the renderer (and the legend) know about. Anything
    // outside this set used to silently fall through to the ref taxonomy entry
    // (dashed pink) — producing edges that the legend never counted, which
    // looked like phantom dashed edges. Skip them at build time instead.
    const KNOWN_EDGE_TYPES = new Set([
        "parent",
        ...Object.keys(EDGE_TYPES),
    ]);
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";
    import { toggleSelection } from "../../stores/selection";

    export let running = false;
    export let restartNonce = 0;
    export let randomizeNonce = 0;

    let cyBase: any;
    let elements: any[] = [];
    let layoutOptions: any = { name: "cola" };

    // Grouping
    function buildElements(
        nodes: GraphNode[],
        edges: GraphEdge[],
        currentFilters: any,
    ) {
        let newElements: any[] = [];

        // Use a simple inclusion for Force View: everything that's not hidden.
        nodes.forEach((n) => {
            const nodeData = computeBaseNodeData(
                n,
                false,
                true,
                false,
                "bright",
            );

            // respect filters if needed
            if (currentFilters.hiddenProjects?.includes(n.project)) return;

            newElements.push({
                data: nodeData,
                position: { x: Math.random() * 800, y: Math.random() * 600 },
            });
        });

        const nodeById = new Set(newElements.map((e) => e.data.id));

        edges.forEach((e, idx) => {
            const src = typeof e.source === "object" ? e.source.id : e.source;
            const tgt = typeof e.target === "object" ? e.target.id : e.target;

            if (!nodeById.has(src) || !nodeById.has(tgt)) return;
            if (!KNOWN_EDGE_TYPES.has(e.type)) return;

            const { linkColor, linkDash } = getEdgeLineStyle(e.type, false);

            const def = getEdgeTypeDef(e.type, false);

            let vis: any = "bright";
            if (def.filterKey) {
                vis = currentFilters[def.filterKey] || "bright";
            }

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
                    curveStyle: "straight", // "bezier",
                },
            });
        });

        return applyEpicGrouping(newElements, nodes, $viewSettings.enableEpicGrouping);
    }

    $: if ($graphData && $filters && $viewSettings.enableEpicGrouping !== undefined) {
        // Rebuild elements when graph structure, filters, or epic grouping changes
        elements = buildElements($graphData.nodes, $graphData.links, $filters);
        setTimeout(() => cyBase?.fit(), 100);
    }

    $: layoutOptions = {
        name: "cola",
        animate: true,
        refresh: 1,
        infinite: true,
        fit: false,
        randomize: false, // Do not scramble on config updates
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
        convergenceThreshold: $viewSettings.colaConvergence,
        maxSimulationTime: 60000,
        // Increase iteration phases so it explores the space (higher entropy)
        // before getting locked down by overlap constraints
        unconstrIter: 40,
        userConstIter: 40,
        allConstIter: 40,
    };

    $: if (running === false && cyBase) {
        cyBase.stopLayout();
    }

    $: if (restartNonce > 0 && cyBase) {
        cyBase.runLayout();
    }

    $: if (randomizeNonce > 0) {
        randomize();
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
        setTimeout(() => {
            layoutOptions = { ...layoutOptions, randomize: false };
        }, 100);
    }

    export function fit() {
        if (cyBase) cyBase.fit();
    }

    function onNodeClick(evt: CustomEvent) {
        toggleSelection(evt.detail.id);
    }
</script>

<div class="w-full h-full relative">
    <CytoscapeBase
        bind:this={cyBase}
        {elements}
        {layoutOptions}
        on:nodeClick={onNodeClick}
        on:layoutstart={() => (running = true)}
        on:layoutstop={() => (running = false)}
    />
</div>
