<script lang="ts">
    import { graphData } from "../../stores/graph";
    import { projectColor, projectBgTint, projectBorderColor, buildProjectRollupMap, resolveMajorProject, summarizeProjectName } from "../../data/projectUtils";
    export let synthesis: any;

    $: recent = synthesis?.sessions?.recent || [];
    $: narrative = synthesis?.narrative || [];

    // Build rollup map from graph data
    $: rollupMap = $graphData ? buildProjectRollupMap($graphData.nodes) : new Map<string, string>();

    // Group sessions by major project
    $: groupedByProject = (() => {
        const groups = new Map<string, any[]>();
        for (const session of recent) {
            const rawProj = session.project || 'unknown';
            const major = summarizeProjectName(resolveMajorProject(rawProj, rollupMap), rollupMap);
            if (!groups.has(major)) groups.set(major, []);
            groups.get(major)!.push(session);
        }
        return Array.from(groups.entries());
    })();
</script>

<div class="flex flex-col gap-4 font-mono">
    <h3 class="text-xs font-bold tracking-[0.2em] text-primary/80 border-b border-primary/30 pb-2 mb-2">RECENT SESSIONS</h3>

    {#if recent.length === 0 && narrative.length === 0}
        <div class="text-primary/50 text-xs italic">No session data available.</div>
    {/if}

    {#if groupedByProject.length > 0}
        <div class="flex flex-col gap-4">
            {#each groupedByProject as [project, sessions]}
                <div class="flex flex-col gap-2">
                    <div class="flex items-center gap-2 mb-1">
                        <span class="text-[10px] font-bold px-2 py-0.5 uppercase tracking-wider"
                              style="background: {projectBgTint(project)}; color: {projectColor(project)}; border: 1px solid {projectBorderColor(project)};">
                            {project}
                        </span>
                        <span class="text-[10px] text-primary/40">{sessions.length} session{sessions.length !== 1 ? 's' : ''}</span>
                    </div>
                    {#each sessions as session}
                        <div class="bg-black border border-primary/30 p-4 relative hover:border-primary transition-colors ml-2"
                             style="border-left: 3px solid {projectColor(project)};">
                            <div class="flex items-center gap-2 mb-1">
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
