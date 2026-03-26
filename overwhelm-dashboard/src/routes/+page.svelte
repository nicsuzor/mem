<script lang="ts">
    import { onMount } from "svelte";
    import TaskEditorView from "$lib/components/views/TaskEditorView.svelte";
    import ZoomContainer from "$lib/components/shared/ZoomContainer.svelte";
    import Legend from "$lib/components/shared/Legend.svelte";
    import ViewConfigOverlay from "$lib/components/shared/ViewConfigOverlay.svelte";

    import TreemapView from "$lib/components/views/TreemapView.svelte";
    import CirclePackView from "$lib/components/views/CirclePackView.svelte";
    import ForceView from "$lib/components/views/ForceView.svelte";
    import ArcView from "$lib/components/views/ArcView.svelte";

    import DashboardView from "$lib/components/dashboard/DashboardView.svelte";
    import ThreadedTasksView from "$lib/components/views/ThreadedTasksView.svelte";

    import {
        prepareGraphData,
        type PreparedGraph,
        type GraphNode,
        type GraphEdge,
    } from "$lib/data/prepareGraphData";
    import { graphData } from "$lib/stores/graph";
    import {
        viewSettings,
        getLayoutFromViewSettings,
    } from "$lib/stores/viewSettings";
    import { filters } from "$lib/stores/filters";
    import { selection } from "$lib/stores/selection";
    import { browser } from "$app/environment";

    export let data: any;

    let rawGraph: any = null;
    let loading = true;
    let errorMsg = "";

    async function fetchGraph() {
        if (!browser) return;
        if (rawGraph) return;
        loading = true;
        errorMsg = "";
        try {
            const res = await fetch(`/api/graph`);
            if (!res.ok) throw new Error(`HTTP ${res.status}`);
            rawGraph = await res.json();
            recomputeGraph();
        } catch (e: any) {
            errorMsg = `Failed to load graph: ` + e.message;
            console.error(e);
        } finally {
            loading = false;
        }
    }

    onMount(() => {
        fetchGraph();
    });

    // Debounce graph recomputes — filters/settings can fire multiple reactive updates
    let recomputeTimer: ReturnType<typeof setTimeout> | null = null;
    $: if (rawGraph) {
        const _deps = [$filters, $viewSettings];
        if (recomputeTimer) clearTimeout(recomputeTimer);
        recomputeTimer = setTimeout(() => recomputeGraph(), 16);
    }

    $: if ($selection && $graphData) {
        applyHighlightOpacity($graphData.nodes, $graphData.links);
    }
    $: focusNode = $selection.focusNodeId ? $graphData?.nodes.find(n => n.id === $selection.focusNodeId) : null;

    function recomputeGraph() {
        if (!rawGraph) return;

        const prepared = prepareGraphData(rawGraph);
        let fNodes = [...prepared.nodes];
        let fLinks = [...prepared.links];
        const isForce =
            $viewSettings.viewMode === "Force";

        // Only include real task types with explicit ID and status
        // Structural types (epic, project, goal) are always included — they often lack task_id or explicit status
        const TASK_TYPES = new Set(["task", "goal", "project", "epic", "bug", "feature", "learn", "action", "subproject"]);
        const STRUCTURAL_TYPES = new Set(["epic", "project", "goal"]);
        fNodes = fNodes.filter(n => {
            if (!TASK_TYPES.has(n.type)) return false;
            if (STRUCTURAL_TYPES.has(n.type)) return true;
            return n._raw?.task_id != null && n._raw?.status && n._raw.status.trim() !== "" && n.status !== "inbox";
        });

        if (!$filters.showActive) {
            fNodes = fNodes.filter(
                (n) => !["active", "inbox", "todo", "in_progress", "review", "waiting", "decomposing", "dormant"].includes(n.status),
            );
        }
        if (!$filters.showBlocked) {
            fNodes = fNodes.filter((n) => n.status !== "blocked");
        }
        if (!$filters.showCompleted) {
            fNodes = fNodes.filter(
                (n) => !["done", "completed", "cancelled", "historical", "deferred", "paused", "seed", "early-scaffold"].includes(n.status),
            );
        }
        if (isForce && $filters.project !== "ALL") {
            fNodes = fNodes.filter(
                (n) => n.project === $filters.project || n.type === "project" || n.type === "goal",
            );
        }
        if ($viewSettings.viewMode === "Force" && !$filters.showOrphans) {
            const nodesWithEdges = new Set<string>();
            fLinks.forEach((l) => {
                const sid = typeof l.source === "object" ? l.source.id : l.source;
                const tid = typeof l.target === "object" ? l.target.id : l.target;
                nodesWithEdges.add(sid);
                nodesWithEdges.add(tid);
            });
            fNodes = fNodes.filter((n) => nodesWithEdges.has(n.id) || !n.isLeaf);
        }
        // Edge visibility is handled via CSS opacity, not by filtering
        if (isForce && $viewSettings.topNLeaves < fNodes.length) {
            const parents = fNodes.filter((n) => !n.isLeaf);
            let leaves = fNodes.filter((n) => n.isLeaf).sort((a, b) => b.dw - a.dw);
            leaves = leaves.slice(0, $viewSettings.topNLeaves);
            fNodes = [...parents, ...leaves];
        }

        const survivingNodeIds = new Set(fNodes.map((n) => n.id));

        // Sanitize parent references after filtering — prevents stratify failures in tree/circle views
        // 1. Remove parents not in surviving set
        fNodes.forEach(n => {
            if (n.parent && !survivingNodeIds.has(n.parent)) n.parent = null;
        });
        // 2. Break parent cycles (A→B→A) — stratify requires a strict tree
        const parentMap = new Map(fNodes.map(n => [n.id, n.parent]));
        for (const n of fNodes) {
            if (!n.parent) continue;
            const visited = new Set<string>();
            let cur: string | null = n.id;
            while (cur) {
                if (visited.has(cur)) { n.parent = null; break; }
                visited.add(cur);
                cur = parentMap.get(cur) || null;
            }
        }

        fLinks = fLinks.filter((l) => {
            const sid = typeof l.source === "object" ? l.source.id : l.source;
            const tid = typeof l.target === "object" ? l.target.id : l.target;
            return survivingNodeIds.has(sid) && survivingNodeIds.has(tid);
        });

        $graphData = {
            ...prepared,
            nodes: fNodes,
            links: fLinks,
        } as any;
    }

    function applyHighlightOpacity(nodes: GraphNode[], links: GraphEdge[]) {
        const active = $selection.activeNodeId;
        const isFocus = $selection.focusNodeId !== null;
        const focusSet = $selection.focusNeighborSet;
        const layout = getLayoutFromViewSettings($viewSettings);

        // Pre-build maps once instead of iterating per-node
        const parentMap = new Map<string, string>();
        nodes.forEach(n => { if (n.parent) parentMap.set(n.id, n.parent); });

        // Pre-build adjacency set for the active node (O(E) once, not O(N*E))
        const activeNeighbors = new Set<string>();
        if (active) {
            activeNeighbors.add(active);
            links.forEach(l => {
                const sid = typeof l.source === "object" ? l.source.id : l.source;
                const tid = typeof l.target === "object" ? l.target.id : l.target;
                if (sid === active) activeNeighbors.add(tid);
                if (tid === active) activeNeighbors.add(sid);
            });
            // Add ancestors and descendants of active node
            let curr = parentMap.get(active);
            while (curr) { activeNeighbors.add(curr); curr = parentMap.get(curr); }
            // Add descendants: nodes whose ancestor chain includes active
            nodes.forEach(n => {
                let c = parentMap.get(n.id);
                while (c) { if (c === active) { activeNeighbors.add(n.id); break; } c = parentMap.get(c); }
            });
            // Sibling logic for force/arc layouts
            const activeParent = parentMap.get(active);
            if (activeParent && ["force", "arc"].includes(layout)) {
                nodes.forEach(n => { if (n.parent === activeParent) activeNeighbors.add(n.id); });
            }
        }

        nodes.forEach((n) => {
            if (["done", "completed", "cancelled"].includes(n.status)) {
                n.opacity = 0.4;
            } else if (n.status === "active") {
                n.opacity = 0.8;
            } else {
                n.opacity = 0.6;
            }

            if (isFocus && focusSet) {
                if (!focusSet.has(n.id)) n.opacity = 0.05;
                return;
            }

            if (active && !activeNeighbors.has(n.id)) {
                n.opacity = 0.05;
            }
        });

        if (isFocus && focusSet) {
            links.forEach((l) => {
                const sid = typeof l.source === "object" ? l.source.id : l.source;
                const tid = typeof l.target === "object" ? l.target.id : l.target;
                l.color = focusSet.has(sid) && focusSet.has(tid) ? l.color : "transparent";
            });
        }
    }

    $: activeLayout = getLayoutFromViewSettings($viewSettings);
</script>

{#if loading}
    <div class="col-span-12 flex items-center justify-center h-full text-primary font-mono text-xl animate-pulse">
        Initializing System...
    </div>
{:else if errorMsg}
    <div class="col-span-12 flex items-center justify-center h-full text-destructive font-mono text-lg">
        {errorMsg}
    </div>
{:else}
    <!-- OPERATOR LAYOUT (12-Column Bento Grid) -->
    {#if $viewSettings.mainTab === "Threaded Tasks"}
        <!-- THREADED TASKS & EDITOR OVERRIDE -->
        <section class="col-span-12 flex flex-col h-full bg-background overflow-hidden transition-all" class:hidden={$viewSettings.mainTab !== "Threaded Tasks"}>
            <ThreadedTasksView />
        </section>
    {:else if $viewSettings.mainTab === "Dashboard"}
        <!-- DASHBOARD: Full width, no graph underneath -->
        <section class="col-span-12 bg-background overflow-y-auto custom-scrollbar">
            <DashboardView {data} />
        </section>

    {:else}
    <!-- MAIN CONTENT: Graph or Dashboard -->
    <section class="{$selection.activeNodeId ? 'col-span-9' : 'col-span-12'} relative bg-surface flex flex-col h-full border-r border-primary-border overflow-hidden transition-all" class:hidden={$viewSettings.mainTab === "Threaded Tasks"}>
        <div class="absolute inset-0 grid-bg opacity-30 pointer-events-none"></div>

            <!-- Focus banner (Absolute Over Graph) -->
            {#if $selection.focusNodeId}
                <div class="absolute top-4 left-4 z-20 flex items-center gap-3">
                    <button
                        class="px-3 py-1.5 bg-black/80 border border-primary/40 text-primary font-mono text-xs hover:bg-primary/20 transition-colors backdrop-blur-md cursor-pointer"
                        onclick={() =>
                            selection.update((s) => ({
                                ...s,
                                focusNodeId: null,
                                focusNeighborSet: null,
                            }))}>← FULL VIEW</button>
                    <span class="px-3 py-1.5 bg-black/60 border border-primary/20 text-primary/70 font-mono text-xs backdrop-blur-md">
                        FOCUS: {focusNode?.fullTitle || $selection.focusNodeId}
                    </span>
                </div>
            {/if}

            <!-- The Graph Area -->
            <div class="flex-1 relative z-0 h-full">
                <ZoomContainer let:containerGroup let:innerWidth let:innerHeight>
                    {#if containerGroup}
                        {#if activeLayout === "treemap" || activeLayout === "tree"}
                            <TreemapView
                                {containerGroup}
                                width={innerWidth}
                                height={innerHeight}
                            />
                        {:else if activeLayout === "circle_pack" || activeLayout === "circle"}
                            <CirclePackView {containerGroup} />
                        {:else if activeLayout === "force" || activeLayout === "sfdp"}
                            <ForceView {containerGroup} />
                        {:else if activeLayout === "arc"}
                            <ArcView {containerGroup} />
                        {/if}
                    {/if}
                </ZoomContainer>
            </div>

            <!-- Legend -->
            <Legend />

            <!-- Graph Configuration Overlay -->
            <ViewConfigOverlay />

            <!-- Overlay Dashboard -->
            <div class="absolute inset-0 z-50 bg-background/90 backdrop-blur-lg overflow-y-auto custom-scrollbar" class:hidden={$viewSettings.mainTab !== "Dashboard"}>
                <DashboardView {data} />
            </div>
    </section>

    <!-- RIGHT SIDEBAR: Details / Editor (only when a task is selected) -->
    {#if $selection.activeNodeId}
    <aside class="col-span-3 bg-background flex flex-col h-full overflow-y-auto custom-scrollbar" class:hidden={$viewSettings.mainTab === "Threaded Tasks"}>
        <TaskEditorView taskId={$selection.activeNodeId} onclose={() => selection.update(s => ({...s, activeNodeId: null}))} />
    </aside>
    {/if}
{/if}
{/if}

<style>
    :global(body) {
        margin: 0;
        padding: 0;
        overflow: hidden;
    }
</style>
