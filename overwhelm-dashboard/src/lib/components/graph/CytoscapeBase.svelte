<script lang="ts">
    import { onMount, onDestroy, createEventDispatcher } from "svelte";
    import cytoscape from "cytoscape";
    // @ts-ignore
    import cola from "cytoscape-cola";
    // @ts-ignore
    import elk from "cytoscape-elk";
    import dagre from "dagre";
    import { getCytoscapeStyles } from "./CytoscapeStyles";

    cytoscape.use(cola);
    cytoscape.use(elk);

    const dispatch = createEventDispatcher();

    export let elements: any[] = [];
    export let layoutOptions: any = { name: "cola" };
    export let stylesheet: cytoscape.StylesheetStyle[] = getCytoscapeStyles();
    export let cy: cytoscape.Core | null = null;
    export let containerClass: string = "w-full h-full";
    export let runLayoutOnMount: boolean = true;

    let containerEl: HTMLDivElement;

    let currentLayout: any = null;

    // React to element changes
    $: if (cy && elements) {
        cy.elements().remove();
        cy.add(elements);
        if (runLayoutOnMount) {
            runLayout();
        }
    }

    // React to layout changes
    $: if (cy && layoutOptions && runLayoutOnMount) {
        runLayout();
    }

    onMount(() => {
        cy = cytoscape({
            container: containerEl,
            elements: elements,
            style: stylesheet,
            layout: runLayoutOnMount ? layoutOptions : undefined,
            wheelSensitivity: 9,
        });

        cy.on("tap", "node", (evt) => {
            dispatch("nodeClick", evt.target.data());
        });

        cy.on("tap", "edge", (evt) => {
            dispatch("edgeClick", evt.target.data());
        });

        cy.on("cxttap", "node", (evt) => {
            dispatch("nodeRightClick", evt.target.data());
        });

        cy.on("grab", "node", (evt) => {
            dispatch("nodeGrab", evt.target);
        });

        cy.on("free", "node", (evt) => {
            dispatch("nodeFree", evt.target);
        });

        cy.on("position", "node", (evt) => {
            dispatch("nodePosition", evt.target);
        });

        cy.on("layoutstart", () => dispatch("layoutstart"));
        cy.on("layoutstop", () => dispatch("layoutstop"));

        dispatch("init", cy);
    });

    onDestroy(() => {
        if (cy) {
            cy.destroy();
            cy = null;
        }
    });

    export function runLayout() {
        if (cy) {
            if (currentLayout) currentLayout.stop();
            currentLayout = cy.layout(layoutOptions);
            currentLayout.run();
        }
    }

    export function stopLayout() {
        if (currentLayout) {
            currentLayout.stop();
            currentLayout = null;
        }
    }

    export function fit() {
        if (cy) cy.fit();
    }
</script>

<div bind:this={containerEl} class={containerClass}></div>

<style>
    div {
        outline: none;
    }
</style>
