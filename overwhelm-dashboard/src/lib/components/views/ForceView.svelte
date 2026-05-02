<script lang="ts">
    import { onDestroy, onMount } from "svelte";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { filters } from "../../stores/filters";
    import { viewSettings } from "../../stores/viewSettings";
    import CytoscapeBase from "../graph/CytoscapeBase.svelte";
    import {
        computeBaseNodeData,
        getEdgeVisibilityState,
        getEdgeLineStyle,
        getEdgeOpacity,
        getEdgeWidth,
    } from "../graph/CytoscapeHelpers";
    import CytoscapeForceConfig from "../graph/CytoscapeForceConfig.svelte";
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
        currentSettings: any,
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

            const { linkColor, linkDash } = getEdgeLineStyle(e.type, false);

            let vis: any = "bright";
            if (e.type === "parent") vis = currentFilters.edgeParent;
            else if (e.type === "depends_on")
                vis = currentFilters.edgeDependencies;
            else if (e.type === "soft_depends_on")
                vis = currentFilters.edgeSoftDependencies;
            else if (e.type === "contributes_to")
                vis = currentFilters.edgeContributes;
            else if (e.type === "similar_to") vis = currentFilters.edgeSimilar;
            else if (e.type === "ref") vis = currentFilters.edgeReferences;

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
                },
            });
        });

        return newElements;
    }

    $: if ($graphData) {
        // Rebuild elements when graph structure or filters change
        elements = buildElements(
            $graphData.nodes,
            $graphData.links,
            $filters,
            $viewSettings,
        );
        setTimeout(() => cyBase?.fit(), 100);
    }

    $: layoutOptions = {
        name: "cola",
        animate: true,
        refresh: 1,
        infinite: true,
        fit: false,
        randomize: false,
        nodeSpacing: (node: any) => $viewSettings.colaGroupPadding,
        edgeLength: (edge: any) => {
            if (edge.data("edgeType") === "parent")
                return $viewSettings.colaLinkDistIntraParent;
            if (edge.data("edgeType") === "depends_on")
                return $viewSettings.colaLinkDistDependsOn;
            return $viewSettings.colaLinkDistRef;
        },
        edgeSymDiffLength: (edge: any) => {
            if (edge.data("edgeType") === "parent")
                return $viewSettings.colaLinkWeightIntraParent;
            if (edge.data("edgeType") === "depends_on")
                return $viewSettings.colaLinkWeightDependsOn;
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
