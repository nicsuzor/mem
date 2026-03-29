<script lang="ts">
    export let synthesis: any;
    export let dailyStory: any;
    /** When true, show only the daily story (for above-the-fold placement).
     *  When false, show alignment/blockers/context cards (for sidebar). */
    export let inline: boolean = false;

    $: hasSynthesis = synthesis && Object.keys(synthesis).length > 0;
    $: rawStoryParagraphs = dailyStory?.story || synthesis?.daily_narrative || synthesis?.narrative || [];
    $: storyParagraphs = rawStoryParagraphs.reduce((acc: any[], curr: any) => {
        const existing = acc.find((item: any) => item.text === curr);
        if (existing) {
            existing.count++;
        } else {
            acc.push({ text: curr, count: 1 });
        }
        return acc;
    }, []).map((item: any) => item.count > 1 ? `[${item.count}x] ${item.text}` : item.text);
    $: hasStory = storyParagraphs && storyParagraphs.length > 0;
    $: ageMinutes = synthesis?._age_minutes;
    $: isStale = ageMinutes !== undefined && ageMinutes > 60;
</script>

{#if inline}
    <!-- Above-the-fold: daily story only -->
    <div class="flex flex-col gap-2 font-mono">
        <div class="flex justify-between items-baseline">
            <h3 class="text-xs font-bold tracking-[0.2em] text-primary/80">TODAY'S STORY</h3>
            <div class="flex items-center gap-2">
                {#if isStale}
                    <span class="text-[10px] font-bold tracking-widest bg-red-900/50 text-red-400 border border-red-500/50 px-2 py-0.5 animate-pulse">STALE</span>
                {/if}
                {#if ageMinutes !== undefined}
                    <span class="text-[10px] text-primary/60"
                        >{ageMinutes < 60 ? Math.round(ageMinutes) + 'm ago' : Math.round(ageMinutes / 60) + 'h ago'}</span
                    >
                {/if}
            </div>
        </div>

        {#if hasStory}
            <div class="text-sm text-primary/80 space-y-2 leading-relaxed">
                {#each storyParagraphs as paragraph}
                    <p>{paragraph}</p>
                {/each}
            </div>
        {:else}
            <div class="text-sm text-primary/40 italic">
                No narrative available. Run <code class="text-primary/60">/daily</code> to generate today's story.
            </div>
        {/if}
        {#if isStale && hasStory}
            <div class="text-[10px] text-primary/40 mt-1">
                Narrative is stale. Run <code class="text-primary/50">/daily</code> to refresh.
            </div>
        {/if}
    </div>

{:else}
    <!-- Sidebar: alignment, blockers, context cards -->
    {#if hasSynthesis}
        <div class="flex flex-col gap-4 font-mono">
            <div class="flex justify-between items-baseline border-b border-primary/30 pb-2 mb-2">
                <h3 class="text-xs font-bold tracking-[0.2em] text-primary/80">INSIGHTS</h3>
                {#if ageMinutes !== undefined}
                    <span class="text-[10px] text-primary/60"
                        >{ageMinutes < 60 ? Math.round(ageMinutes) + 'm ago' : Math.round(ageMinutes / 60) + 'h ago'}</span
                    >
                {/if}
            </div>

            <div class="grid grid-cols-1 gap-4">
                {#if synthesis.alignment}
                    {@const align = synthesis.alignment}
                    {@const alignStatus = typeof align === 'string' ? align : (align.status || align.assessment || 'unknown')}
                    {@const alignNote = typeof align === 'object' ? (align.note || align.assessment || '') : ''}
                    <div class="bg-black/50 border border-primary/30 p-4 hover:border-primary transition-colors">
                        <div class="text-[10px] font-bold tracking-widest text-primary/60 mb-2">ALIGNMENT</div>
                        <div class="flex items-center gap-2">
                            <span class="text-[10px] font-bold px-2 py-0.5 border {alignStatus === 'on_track' ? 'bg-green-900/30 text-green-400 border-green-500/40' : alignStatus === 'drifting' ? 'bg-yellow-900/30 text-yellow-400 border-yellow-500/40' : 'bg-primary/10 text-primary/70 border-primary/30'}">
                                {alignStatus.toUpperCase().replace('_', ' ')}
                            </span>
                            {#if alignNote}
                                <span class="text-xs text-primary/70">{alignNote}</span>
                            {/if}
                        </div>
                    </div>
                {/if}

                {#if synthesis.recent_context}
                    <div class="bg-black/50 border border-primary/30 p-4 hover:border-primary transition-colors">
                        <div class="text-[10px] font-bold tracking-widest text-primary/60 mb-2">CURRENT CONTEXT</div>
                        <div class="text-sm text-primary/90">
                            {synthesis.recent_context}
                        </div>
                    </div>
                {/if}

                {#if synthesis.blockers}
                    <div class="bg-red-900/10 border border-red-500/30 p-4 hover:border-red-500 transition-colors">
                        <div class="text-[10px] font-bold tracking-widest text-red-500/80 mb-2">BLOCKERS</div>
                        <ul class="list-disc list-inside text-sm text-red-400 space-y-1 ml-4">
                            {#each Array.isArray(synthesis.blockers) ? synthesis.blockers : [synthesis.blockers] as blocker}
                                <li>{blocker}</li>
                            {/each}
                        </ul>
                    </div>
                {/if}
            </div>
        </div>
    {/if}
{/if}
