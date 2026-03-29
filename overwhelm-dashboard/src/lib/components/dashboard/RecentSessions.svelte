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
                    <div class="absolute left-0 top-0 bottom-0 w-1 {session.outcome === 'success' ? 'bg-green-500/60' : 'bg-primary/40'}"></div>
                    <div class="flex items-center gap-2 mb-1">
                        <span class="text-xs font-bold bg-primary/10 text-primary px-2 py-0.5 border border-primary/20">{session.project || 'unknown'}</span>
                        {#if session.duration_minutes}
                            <span class="text-[10px] text-primary/50">{Math.round(session.duration_minutes)}m</span>
                        {/if}
                        {#if session.outcome}
                            <span class="text-[10px] {session.outcome === 'success' ? 'text-green-400/70' : 'text-yellow-400/70'}">{session.outcome}</span>
                        {/if}
                        {#if session.date}
                            <span class="text-[10px] text-primary/40 ml-auto">{session.date}</span>
                        {/if}
                    </div>
                    {#if session.initial_prompt || session.user_prompts?.[0]}
                        <div class="text-[11px] text-primary/50 mt-1 italic truncate" title={session.initial_prompt || session.user_prompts?.[0]}>
                            "{session.initial_prompt || session.user_prompts?.[0]}"
                        </div>
                    {/if}
                    {#if session.summary}
                        <div class="text-sm text-primary/90 mt-1">{session.summary}</div>
                    {/if}
                    {#if session.accomplishments?.length > 0}
                        <ul class="mt-2 text-xs text-primary/60 list-none">
                            {#each session.accomplishments.slice(0, 3) as item}
                                <li class="before:content-['›_'] before:text-primary/30">{item}</li>
                            {/each}
                        </ul>
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
