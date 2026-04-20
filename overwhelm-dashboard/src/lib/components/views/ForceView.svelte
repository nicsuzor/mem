<script lang="ts">
    import { onDestroy } from "svelte";
    import { graphData } from "../../stores/graph";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";

    export let containerGroup: SVGGElement;
    export let running = false;

    onDestroy(() => {
        // cleanup
    });
</script>
