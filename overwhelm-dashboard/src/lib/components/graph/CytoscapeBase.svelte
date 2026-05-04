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
    let mounted = false;

    // React to element changes — diff in place so existing nodes keep their
    // positions. Only kick a layout run when nodes are actually added or
    // removed. Pure data updates (visibility, opacity, colour, size) leave
    // the layout alone, so legend toggles don't reset the graph.
    $: if (cy && elements && mounted) {
        const structural = syncElements(elements);
        if (structural && runLayoutOnMount) {
            runLayout();
        }
    }

    function syncElements(els: any[]): boolean {
        if (!cy) return false;
        const newIds = new Set<string>(els.map((e) => e.data.id));
        let structural = false;

        // Collect stale IDs first to avoid mutating cy mid-iteration
        // (removing a node also takes its connected edges, which would
        // otherwise be visited as ghosts on the next tick).
        const staleIds: string[] = [];
        cy.elements().forEach((el: any) => {
            if (!newIds.has(el.id())) staleIds.push(el.id());
        });
        if (staleIds.length) {
            structural = true;
            staleIds.forEach((id) => cy!.getElementById(id).remove());
        }

        els.forEach((el) => {
            const existing = cy!.getElementById(el.data.id);
            if (existing.length === 0) {
                cy!.add(el);
                structural = true;
            } else {
                existing.data(el.data);
                // Note: we deliberately do NOT push el.position onto existing
                // nodes. Cytoscape's own semantics treat `position` on an
                // element spec as an initial seed used only at add-time.
                // ForceView (re)builds elements with random positions on every
                // filter change; mirroring that here would teleport nodes on
                // every legend click. Views that need to move existing nodes
                // (Metro animation, etc.) must call `.position(...)` directly.
            }
        });

        return structural;
    }

    onMount(() => {
        cy = cytoscape({
            container: containerEl,
            elements: elements,
            style: stylesheet,
            layout: runLayoutOnMount ? layoutOptions : undefined,
            wheelSensitivity: 0.1,
        });
        mounted = true;

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
