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
    import MetroView from "$lib/components/views/MetroView.svelte";

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

    let forceViewRef: ForceView;
    let forceRunning = false;
    let forceRestartNonce = 0;
    let forceRandomizeNonce = 0;
    let metroViewRef: MetroView;
    let metroRunning = false;
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

        // Tri-state Visibility Filters
        fNodes = fNodes.filter(n => {
            // Projects are still boolean
            if (($filters.hiddenProjects?.length ?? 0) > 0 && $filters.hiddenProjects.includes(n.project || "")) {
                return false;
            }

            let visState = 'bright';

            // Determine status visibility
            let statusVis = 'bright';
            const isActive = ["active", "inbox", "todo", "in_progress", "review", "waiting", "decomposing", "dormant"].includes(n.status);
            const isBlocked = n.status === "blocked";
            const isCompleted = ["done", "completed", "cancelled", "historical", "deferred", "paused", "seed", "early-scaffold"].includes(n.status);

            if (isActive) statusVis = $filters.statusActive;
            else if (isBlocked) statusVis = $filters.statusBlocked;
            else if (isCompleted) statusVis = $filters.statusCompleted;

            let priVis = 'bright';
            if (!STRUCTURAL_TYPES.has(n.type)) {
                if (n.priority === 0) priVis = $filters.priority0;
                else if (n.priority === 1) priVis = $filters.priority1;
                else if (n.priority === 2) priVis = $filters.priority2;
                else if (n.priority === 3) priVis = $filters.priority3;
                else if (n.priority === 4) priVis = $filters.priority4;
            }

            if (statusVis === 'hidden' || priVis === 'hidden') return false;
            if (statusVis === 'half' || priVis === 'half') visState = 'half';

            (n as any).filter_dimmed = (visState === 'half');
            return true;
        });

        const edgeVisibilityFor = (edge: GraphEdge) => {
            if (edge.type === 'parent') return $filters.edgeParent;
            if (edge.type === 'depends_on' || edge.type === 'soft_depends_on') return $filters.edgeDependencies;
            if (edge.type === 'ref') return $filters.edgeReferences;
            return 'bright';
        };

        const edgeOpacityFor = (visibility: string) => {
            if (visibility === 'half') return 0.2;
            return 0.6;
        };

        fLinks = fLinks
            .filter((edge) => edgeVisibilityFor(edge) !== 'hidden')
            .map((edge) => ({
                ...edge,
                opacity: edgeOpacityFor(edgeVisibilityFor(edge)),
            }));

        if ($viewSettings.viewMode === "Force" && $filters.statusOrphans === 'hidden') {
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

        // Restore parent containers needed by surviving children
        {
            const allNodeMap = new Map(prepared.nodes.map(n => [n.id, n]));
            const survivingIds = new Set(fNodes.map(n => n.id));
            const toRestore: typeof fNodes = [];
            for (const n of fNodes) {
                let pid = n.parent;
                while (pid && !survivingIds.has(pid)) {
                    const parent = allNodeMap.get(pid);
                    if (!parent) break;
                    survivingIds.add(pid);
                    toRestore.push(parent);
                    pid = parent.parent;
                }
            }
            if (toRestore.length > 0) fNodes = [...fNodes, ...toRestore];
        }

        // Prune empty structural containers
        {
            let changed = true;
            while (changed) {
                changed = false;
                const childCount = new Map<string, number>();
                for (const n of fNodes) {
                    if (n.parent) childCount.set(n.parent, (childCount.get(n.parent) || 0) + 1);
                }
                const before = fNodes.length;
                fNodes = fNodes.filter(n => {
                    if (!STRUCTURAL_TYPES.has(n.type)) return true;
                    return (childCount.get(n.id) || 0) > 0;
                });
                if (fNodes.length < before) changed = true;
            }
        }

        // Collapse single-child structural containers
        {
            const collapseMap = new Map<string, string>();
            const childrenMap = new Map<string, string[]>();
            for (const n of fNodes) {
                if (n.parent) {
                    const kids = childrenMap.get(n.parent) || [];
                    kids.push(n.id);
                    childrenMap.set(n.parent, kids);
                }
            }
            for (const n of fNodes) {
                if (STRUCTURAL_TYPES.has(n.type)) {
                    const kids = childrenMap.get(n.id) || [];
                    if (kids.length === 1) {
                        collapseMap.set(n.id, kids[0]);
                    }
                }
            }

            let changed = true;
            while (changed) {
                changed = false;
                for (const [k, v] of collapseMap.entries()) {
                    if (collapseMap.has(v)) {
                        collapseMap.set(k, collapseMap.get(v)!);
                        changed = true;
                    }
                }
            }

            if (collapseMap.size > 0) {
                const initialNodeById = new Map(fNodes.map(n => [n.id, n]));
                fNodes = fNodes.filter(n => !collapseMap.has(n.id));
                fNodes.forEach(n => {
                    let curParent = n.parent;
                    const seen = new Set<string>();
                    while (curParent && collapseMap.has(curParent)) {
                        if (seen.has(curParent)) break;
                        seen.add(curParent);
                        const pNode = initialNodeById.get(curParent);
                        curParent = pNode ? pNode.parent : null;
                    }
                    n.parent = curParent;
                });

                fLinks = fLinks.map(l => {
                    const sid = typeof l.source === "object" ? (l.source as any).id : l.source;
                    const tid = typeof l.target === "object" ? (l.target as any).id : l.target;
                    const newSid = collapseMap.get(sid) || sid;
                    const newTid = collapseMap.get(tid) || tid;
                    return {
                        ...l,
                        source: newSid,
                        target: newTid
                    };
                }).filter(l => l.source !== l.target);

                const uniqueEdges = new Map<string, any>();
                fLinks.forEach(e => {
                    const key = `${e.source}-${e.target}-${e.type || ''}`;
                    uniqueEdges.set(key, e);
                });
                fLinks = Array.from(uniqueEdges.values());
            }
        }

        const survivingNodeIds = new Set(fNodes.map((n) => n.id));

        // Sanitize parent references
        fNodes.forEach(n => {
            if (n.parent && !survivingNodeIds.has(n.parent)) n.parent = null;
        });
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

        // Save safe parent reference only after invalid and cyclic parents are removed.
        fNodes.forEach(n => { n._safe_parent = n.parent; });

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

        const parentMap = new Map<string, string>();
        nodes.forEach(n => { if (n.parent) parentMap.set(n.id, n.parent); });

        const activeNeighbors = new Set<string>();
        if (active) {
            activeNeighbors.add(active);
            links.forEach(l => {
                const sid = typeof l.source === "object" ? l.source.id : l.source;
                const tid = typeof l.target === "object" ? l.target.id : l.target;
                if (sid === active) activeNeighbors.add(tid);
                if (tid === active) activeNeighbors.add(sid);
            });
            let curr = parentMap.get(active);
            while (curr) { activeNeighbors.add(curr); curr = parentMap.get(curr); }
            nodes.forEach(n => {
                let c = parentMap.get(n.id);
                while (c) { if (c === active) { activeNeighbors.add(n.id); break; } c = parentMap.get(c); }
            });
            const activeParent = parentMap.get(active);
            if (activeParent && ["force", "arc"].includes(layout)) {
                nodes.forEach(n => { if (n.parent === activeParent) activeNeighbors.add(n.id); });
            }
        }

        nodes.forEach((n) => {
            if (["done", "completed", "cancelled"].includes(n.status)) n.opacity = 0.4;
            else if (n.status === "active") n.opacity = 0.8;
            else n.opacity = 0.6;

            if (isFocus && focusSet) {
                if (!focusSet.has(n.id)) n.opacity = 0.05;
                return;
            }
            if (active && !activeNeighbors.has(n.id)) n.opacity = 0.05;
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
    <div class="col-span-12 flex items-center justify-center h-full text-primary font-mono text-xl animate-pulse">Initializing System...</div>
{:else if errorMsg}
    <div class="col-span-12 flex items-center justify-center h-full text-destructive font-mono text-lg">{errorMsg}</div>
{:else}
    {#if $viewSettings.mainTab === "Threaded Tasks"}
        <section class="col-span-12 flex flex-col h-full bg-background overflow-hidden transition-all"><ThreadedTasksView /></section>
    {:else if $viewSettings.mainTab === "Dashboard"}
        <section class="{$selection.activeNodeId ? 'col-span-9' : 'col-span-12'} bg-background overflow-y-auto custom-scrollbar transition-all"><DashboardView {data} /></section>
        {#if $selection.activeNodeId}
        <aside class="col-span-3 bg-background flex flex-col h-full overflow-y-auto custom-scrollbar">
            <TaskEditorView taskId={$selection.activeNodeId} onclose={() => selection.update(s => ({...s, activeNodeId: null}))} />
        </aside>
        {/if}
    {:else}
    <section class="{$selection.activeNodeId ? 'col-span-9' : 'col-span-12'} relative bg-surface flex flex-col h-full border-r border-primary-border overflow-hidden transition-all" data-component="graph-canvas">
        <div class="absolute inset-0 grid-bg opacity-30 pointer-events-none"></div>
            {#if $selection.focusNodeId}
                <div class="absolute top-4 left-4 z-20 flex items-center gap-3">
                    <button class="px-3 py-1.5 bg-black/80 border border-primary/40 text-primary font-mono text-xs hover:bg-primary/20 transition-colors backdrop-blur-md cursor-pointer" onclick={() => selection.update((s) => ({ ...s, focusNodeId: null, focusNeighborSet: null, }))}>← FULL VIEW</button>
                    <span class="px-3 py-1.5 bg-black/60 border border-primary/20 text-primary/70 font-mono text-xs backdrop-blur-md">FOCUS: {focusNode?.fullTitle || $selection.focusNodeId}</span>
                </div>
            {/if}
            <div class="flex-1 relative z-0 h-full">
                {#if activeLayout === "metro"}
                    <MetroView bind:this={metroViewRef} bind:running={metroRunning} />
                {:else}
                    <ZoomContainer let:containerGroup let:innerWidth let:innerHeight>
                        {#if containerGroup}
                            {#if activeLayout === "treemap" || activeLayout === "tree"}
                                <TreemapView {containerGroup} width={innerWidth} height={innerHeight} />
                            {:else if activeLayout === "circle_pack" || activeLayout === "circle"}
                                <CirclePackView {containerGroup} />
                            {:else if activeLayout === "force" || activeLayout === "sfdp"}
                                <ForceView {containerGroup} bind:this={forceViewRef} bind:running={forceRunning} restartNonce={forceRestartNonce} randomizeNonce={forceRandomizeNonce} />
                            {:else if activeLayout === "arc"}
                                <ArcView {containerGroup} />
                            {/if}
                        {/if}
                    </ZoomContainer>
                {/if}
            </div>
            <Legend />
            {#if activeLayout === "force" || activeLayout === "sfdp" || activeLayout === "metro"}
                <div class="absolute bottom-4 left-4 z-30 flex items-center gap-2">
                    <button class="px-3 py-1.5 rounded border text-xs font-bold uppercase tracking-wider bg-background/80 border-primary/40 text-primary hover:bg-primary/20 transition-colors" onclick={() => activeLayout === "metro" ? metroViewRef?.toggleRunning() : (forceRunning ? forceRunning = false : forceRestartNonce += 1)}>
                        {(activeLayout === "metro" ? metroRunning : forceRunning) ? '⏸ Stop' : '▶ Start'} Layout
                    </button>
                    {#if activeLayout === "force"}
                        <button class="px-3 py-1.5 rounded border text-xs font-bold uppercase tracking-wider bg-background/80 border-primary/40 text-primary hover:bg-primary/20 transition-colors" onclick={() => forceRandomizeNonce += 1}>
                            🎲 Randomise
                        </button>
                    {/if}
                </div>
            {/if}
            <ViewConfigOverlay />
            <div class="absolute inset-0 z-50 bg-background/90 backdrop-blur-lg overflow-y-auto custom-scrollbar" class:hidden={$viewSettings.mainTab !== "Dashboard"}><DashboardView {data} /></div>
    </section>
    {#if $selection.activeNodeId}
    <aside class="col-span-3 bg-background flex flex-col h-full overflow-y-auto custom-scrollbar" data-component="detail-sidebar">
        <TaskEditorView taskId={$selection.activeNodeId} onclose={() => selection.update(s => ({...s, activeNodeId: null}))} />
    </aside>
    {/if}
{/if}
{/if}

<style>
    :global(body) { margin: 0; padding: 0; overflow: hidden; }
</style>
