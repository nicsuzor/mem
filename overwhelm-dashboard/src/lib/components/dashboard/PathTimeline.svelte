<script lang="ts">
    export let path: any;
    /** When true, only render the abandoned work section */
    export let abandonedOnly: boolean = false;

    $: activity = path?.activity || [];
    $: abandoned = path?.abandoned_work || [];

    const INITIAL_PROJECTS = 8;
    let showAll = false;
    $: visible = showAll ? activity : activity.slice(0, INITIAL_PROJECTS);
    $: hiddenCount = Math.max(0, activity.length - INITIAL_PROJECTS);

    function outcomeIcon(outcome: string): string {
        if (outcome === 'success') return '✓';
        if (outcome === 'in_progress' || outcome === 'partial') return '↻';
        if (outcome === 'failure' || outcome === 'error') return '✗';
        return '·';
    }

    function outcomeClass(outcome: string): string {
        if (outcome === 'success') return 'text-green-500';
        if (outcome === 'in_progress' || outcome === 'partial') return 'text-primary/60';
        if (outcome === 'failure' || outcome === 'error') return 'text-red-500';
        return 'text-primary/40';
    }
</script>

{#if abandonedOnly}
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

{:else if activity.length > 0}
    <div class="flex flex-col gap-4 font-mono text-primary">
        <h3 class="text-xs font-bold tracking-[0.2em] text-primary/80 border-b border-primary/30 pb-2">
            RECENT ACTIVITY
            <span class="text-primary/40 font-normal ml-2">({activity.length} projects)</span>
        </h3>

        <div class="flex flex-col gap-4">
            {#each visible as group}
                <div class="flex flex-col gap-1.5">
                    <div class="flex items-center gap-3 text-xs">
                        <span class="font-bold bg-primary/20 text-primary px-2 py-0.5 border border-primary/30">{group.project}</span>
                        <span class="text-primary/30 ml-auto">{group.period}</span>
                    </div>

                    <div class="flex flex-col gap-0.5 ml-1">
                        {#each group.items.slice(0, 5) as item}
                            <div class="flex items-start gap-2 text-xs py-0.5">
                                <span class="{outcomeClass(item.outcome)} shrink-0 w-3 text-center">{outcomeIcon(item.outcome)}</span>
                                <span class="text-primary/80 line-clamp-1">{item.text}</span>
                            </div>
                        {/each}
                        {#if group.items.length > 5}
                            <div class="text-[10px] text-primary/30 ml-5">+ {group.items.length - 5} more</div>
                        {/if}
                    </div>
                </div>
            {/each}
        </div>

        {#if !showAll && hiddenCount > 0}
            <button
                class="text-xs text-primary/50 hover:text-primary transition-colors cursor-pointer border border-primary/20 hover:border-primary/40 px-3 py-2 text-center"
                on:click={() => showAll = true}
            >
                Show {hiddenCount} more project{hiddenCount !== 1 ? 's' : ''}...
            </button>
        {/if}
    </div>
{/if}
