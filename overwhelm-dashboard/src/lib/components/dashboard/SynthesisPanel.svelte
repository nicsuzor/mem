<script lang="ts">
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
</script>

<div class="flex flex-col gap-3 font-mono">
    <div class="flex justify-between items-baseline">
        <h3 class="text-xs font-bold tracking-[0.2em] text-primary/80">RECENT ACTIVITY</h3>
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
            No recent activity found in session summaries.
        </div>
    {/if}

</div>
