<script lang="ts">
    export let synthesis: any;

    $: recent = synthesis?.sessions?.recent || [];
    $: narrative = synthesis?.narrative || [];
</script>

<div class="flex flex-col gap-4 font-mono">
    <h3 class="text-xs font-bold tracking-[0.2em] text-primary/80 border-b border-primary/30 pb-2 mb-2">RECENT SESSIONS</h3>

    {#if recent.length === 0 && narrative.length === 0}
        <div class="text-primary/50 text-xs italic">No session data available.</div>
    {/if}

    {#if recent.length > 0}
        <div class="flex flex-col gap-3">
            {#each recent as session}
                <div class="bg-black border border-primary/30 p-4 relative hover:border-primary transition-colors">
                    <div class="absolute left-0 top-0 bottom-0 w-1 bg-primary/40"></div>
                    <div class="mb-1">
                        <span class="text-xs font-bold bg-primary/10 text-primary px-2 py-0.5 border border-primary/20">{session.project || 'unknown'}</span>
                    </div>
                    {#if session.summary}
                        <div class="text-sm text-primary/90 mt-2">{session.summary}</div>
                    {/if}
                </div>
            {/each}
        </div>
    {:else if narrative.length > 0}
        <div class="flex flex-col gap-2">
            {#each narrative as line}
                <div class="text-sm text-primary/80 border-l border-primary/30 pl-3 py-1">{line}</div>
            {/each}
        </div>
    {/if}
</div>
