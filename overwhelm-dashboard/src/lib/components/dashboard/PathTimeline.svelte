<script lang="ts">
    export let path: any;
    /** When true, only render the abandoned work section (for standalone placement) */
    export let abandonedOnly: boolean = false;

    $: threads = path?.threads || [];
    $: abandoned = path?.abandoned_work || [];

    // Group threads by project so same-project goals don't break the flow
    $: groupedByProject = (() => {
        const groups: { project: string; threads: any[] }[] = [];
        for (const thread of threads) {
            const proj = thread.project || 'unknown';
            const last = groups[groups.length - 1];
            if (last && last.project === proj) {
                last.threads.push(thread);
            } else {
                groups.push({ project: proj, threads: [thread] });
            }
        }
        return groups;
    })();

    const INITIAL_GROUPS = 5;
    let showAllGroups = false;
    $: visibleGroups = showAllGroups ? groupedByProject : groupedByProject.slice(0, INITIAL_GROUPS);
    $: hiddenGroupCount = Math.max(0, groupedByProject.length - INITIAL_GROUPS);

    // Track which project groups have their events expanded
    let expandedGroups: Set<number> = new Set();
    function toggleGroup(idx: number) {
        if (expandedGroups.has(idx)) {
            expandedGroups.delete(idx);
        } else {
            expandedGroups.add(idx);
        }
        expandedGroups = expandedGroups;
    }

    function formatTime(isoString: string): string {
        if (!isoString) return "";
        try {
            const d = new Date(isoString);
            return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
        } catch {
            return isoString;
        }
    }

    function totalEvents(group: { threads: any[] }): number {
        return group.threads.reduce((sum, t) => sum + (t.events?.length || 0), 0);
    }
</script>

{#if abandonedOnly}
    <!-- Standalone abandoned work section -->
    {#if abandoned.length > 0}
        <div class="flex flex-col gap-3 font-mono">
            <h3 class="text-xs font-bold tracking-[0.2em] text-yellow-500/80 border-b border-yellow-500/30 pb-2 flex items-center gap-2">
                <span class="material-symbols-outlined text-[14px]">warning</span>
                DROPPED THREADS ({abandoned.length})
            </h3>
            <div class="flex flex-col gap-2">
                {#each abandoned as item}
                    <div class="flex flex-col gap-1 border-l-2 border-yellow-500/50 pl-3">
                        <div class="flex items-center gap-2">
                            <span class="text-[10px] font-bold bg-yellow-500/20 text-yellow-500 px-1.5 py-0.5">{item.project || "UNKNOWN"}</span>
                            <span class="text-[10px] text-yellow-500/60">{item.time_ago || ""}</span>
                        </div>
                        <div class="text-xs text-yellow-500/90">{item.description}</div>
                    </div>
                {/each}
            </div>
        </div>
    {/if}

{:else if threads.length > 0}
    <!-- Full path reconstruction with gating -->
    <div class="flex flex-col gap-4 font-mono text-primary">
        <h3 class="text-xs font-bold tracking-[0.2em] text-primary/80 border-b border-primary/30 pb-2">
            PATH RECONSTRUCTION
            <span class="text-primary/40 font-normal ml-2">({threads.length} sessions across {groupedByProject.length} projects)</span>
        </h3>

        <div class="flex flex-col gap-4">
            {#each visibleGroups as group, groupIdx}
                {@const eventCount = totalEvents(group)}
                {@const goalCount = group.threads.length}
                {@const isExpanded = expandedGroups.has(groupIdx)}
                <div class="flex flex-col gap-2">
                    <!-- Project header — click to expand/collapse events -->
                    <button
                        class="flex items-center gap-3 text-xs cursor-pointer hover:bg-primary/5 p-1 -m-1 transition-colors rounded text-left w-full"
                        on:click={() => toggleGroup(groupIdx)}
                    >
                        <span class="font-bold bg-primary/20 text-primary px-2 py-0.5 border border-primary/30">{group.project}</span>
                        <span class="text-primary/40">{goalCount} goal{goalCount !== 1 ? 's' : ''}, {eventCount} event{eventCount !== 1 ? 's' : ''}</span>
                        <span class="material-symbols-outlined text-[14px] text-primary/40 ml-auto">{isExpanded ? 'expand_less' : 'expand_more'}</span>
                    </button>

                    <!-- Collapsed: just goal summaries -->
                    {#if !isExpanded}
                        <div class="flex flex-col gap-1 ml-4">
                            {#each group.threads as thread}
                                {#if thread.initial_goal || thread.hydrated_intent}
                                    <div class="text-xs text-primary/60 truncate">
                                        <span class="text-primary/40">›</span>
                                        {thread.hydrated_intent || thread.initial_goal}
                                    </div>
                                {/if}
                            {/each}
                        </div>
                    {:else}
                        <!-- Expanded: full timeline -->
                        <div class="flex flex-col gap-0 ml-2 border-l border-primary/20 pl-4 relative">
                            {#each group.threads as thread}
                                {#if thread.initial_goal || thread.hydrated_intent}
                                    <div class="relative py-3 -ml-4 pl-4 pr-2">
                                        <div class="absolute left-[-4.5px] top-[18px] w-2 h-2 rounded-full bg-primary"></div>
                                        <div class="bg-black/40 border border-primary/20 p-3 text-xs leading-relaxed">
                                            <strong class="text-primary/60">GOAL:</strong>
                                            <span class="text-primary/90">{thread.hydrated_intent || thread.initial_goal}</span>
                                        </div>
                                    </div>
                                {/if}

                                {#each thread.events as event}
                                    <div class="relative py-3 group hover:bg-primary/5 -ml-4 pl-4 pr-2 transition-colors">
                                        <div class="absolute left-[-4.5px] top-[18px] w-2 h-2 rounded-full bg-black border border-primary group-hover:bg-primary transition-colors"></div>
                                        <div class="flex items-start gap-4">
                                            <div class="text-[10px] text-primary/50 pt-0.5 w-12 shrink-0">
                                                {formatTime(event.timestamp)}
                                            </div>
                                            <div class="flex flex-col gap-1 flex-1">
                                                <div class="text-xs text-primary/80 leading-relaxed">
                                                    {event.narrative}
                                                </div>
                                                {#if event.task_id}
                                                    <div class="text-[10px] text-primary/40 mt-1">
                                                        ID: {event.task_id}
                                                    </div>
                                                {/if}
                                            </div>
                                        </div>
                                    </div>
                                {/each}
                            {/each}
                        </div>
                    {/if}
                </div>
            {/each}
        </div>

        {#if !showAllGroups && hiddenGroupCount > 0}
            <button
                class="text-xs text-primary/50 hover:text-primary transition-colors cursor-pointer border border-primary/20 hover:border-primary/40 px-3 py-2 text-center"
                on:click={() => showAllGroups = true}
            >
                Show {hiddenGroupCount} more project{hiddenGroupCount !== 1 ? 's' : ''}...
            </button>
        {/if}
    </div>
{/if}
