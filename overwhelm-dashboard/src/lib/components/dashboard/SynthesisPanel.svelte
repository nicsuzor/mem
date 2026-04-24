<script lang="ts">
    export let synthesis: any;
    export let dailyStory: any;

    $: rawStoryParagraphs = dailyStory?.story || [];

    // Group story entries by project prefix and deduplicate
    $: storyByProject = (() => {
        const groups: Map<string, string[]> = new Map();
        const seen = new Set<string>();
        for (const raw of rawStoryParagraphs) {
            const text = typeof raw === 'string' ? raw : String(raw);
            const match = text.match(/^\[([^\]]+)\]\s*(.+)$/);
            const project = match ? match[1] : '_general';
            let content = match ? match[2] : text;
            
            // Strip robotic preambles
            content = content.replace(/^(Successfully completed|Completed|Successfully finished|Done):\s*/i, '');
            // Capitalize first letter
            content = content.charAt(0).toUpperCase() + content.slice(1);
            
            if (seen.has(content)) continue;
            seen.add(content);
            if (!groups.has(project)) groups.set(project, []);
            groups.get(project)!.push(content);
        }
        return groups;
    })();

    $: hasStory = storyByProject.size > 0;
    $: ageMinutes = synthesis?._age_minutes;
    $: isStale = ageMinutes !== undefined && ageMinutes > 60;
    
    $: lastRun = synthesis?.last_run;
    $: exitCode = synthesis?.exit_code;
    $: inputCompleteness = synthesis?.input_completeness;
</script>

<div class="flex flex-col gap-3 font-mono">
    <div class="flex justify-between items-baseline">
        <h3 class="text-xs font-bold tracking-[0.2em] text-primary/80">TODAY'S STORY</h3>
        <div class="flex flex-col items-end gap-1">
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
            {#if lastRun || exitCode !== undefined}
                <div class="flex gap-2 text-[9px] text-primary/40">
                    {#if lastRun}<span>Run: {new Date(lastRun).toLocaleTimeString()}</span>{/if}
                    {#if exitCode !== undefined}<span class={exitCode === 0 ? 'text-green-500/50' : 'text-red-500/50'}>Exit: {exitCode}</span>{/if}
                    {#if inputCompleteness !== undefined}<span>In: {Math.round(inputCompleteness * 100)}%</span>{/if}
                </div>
            {/if}
        </div>
    </div>

    {#if hasStory}
        <div class="flex flex-col gap-3">
            {#each [...storyByProject.entries()] as [project, items]}
                {#if project === '_general'}
                    <div class="text-sm text-primary/80 space-y-1 leading-relaxed">
                        {#each items as item}
                            <p>{item}</p>
                        {/each}
                    </div>
                {:else}
                    <div class="flex flex-col gap-1">
                        <span class="text-[10px] font-bold bg-primary/15 text-primary/70 px-1.5 py-0.5 w-fit">{project}</span>
                        <ul class="text-sm text-primary/80 space-y-0.5 ml-3">
                            {#each items as item}
                                <li class="before:content-['›_'] before:text-primary/30">{item}</li>
                            {/each}
                        </ul>
                    </div>
                {/if}
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
