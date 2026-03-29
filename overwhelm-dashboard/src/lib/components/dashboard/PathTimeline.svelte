<script lang="ts">
    import { toggleSelection } from "../../stores/selection";
    import { graphData } from "../../stores/graph";
    import { projectColor, projectBgTint, projectBorderColor, buildProjectRollupMap, resolveMajorProject, summarizeProjectName } from "../../data/projectUtils";
    export let path: any;
    /** When true, only render the abandoned work section */
    export let abandonedOnly: boolean = false;

    function findNodeForTask(description: string, project: string): string | null {
        if (!$graphData) return null;
        const match = $graphData.nodes.find(n =>
            (n.label === description || n.fullTitle === description) &&
            (!project || n.project === project)
        );
        return match?.id || null;
    }

    // Build rollup map from graph data
    $: rollupMap = $graphData ? buildProjectRollupMap($graphData.nodes) : new Map<string, string>();

    // Group activity by major project (merge sub-projects into parents)
    $: mergedActivity = (() => {
        const raw = path?.activity || [];
        const byMajor = new Map<string, { items: any[]; latestDate: string; period: string }>();
        for (const group of raw) {
            const major = summarizeProjectName(resolveMajorProject(group.project, rollupMap), rollupMap);
            if (!byMajor.has(major)) {
                byMajor.set(major, { items: [], latestDate: '', period: '' });
            }
            const mg = byMajor.get(major)!;
            mg.items.push(...group.items);
            if (group.latestDate > mg.latestDate) {
                mg.latestDate = group.latestDate;
                mg.period = group.period;
            }
        }
        return Array.from(byMajor.entries()).map(([project, g]) => ({
            project, ...g
        })).sort((a, b) => b.latestDate.localeCompare(a.latestDate));
    })();

    $: abandoned = path?.abandoned_work || [];

    const INITIAL_PROJECTS = 8;
    let showAll = false;
    $: visible = showAll ? mergedActivity : mergedActivity.slice(0, INITIAL_PROJECTS);
    $: hiddenCount = Math.max(0, mergedActivity.length - INITIAL_PROJECTS);

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

    // Group abandoned work by major project
    $: abandonedByProject = (() => {
        const map = new Map<string, any[]>();
        for (const item of abandoned) {
            const rawProj = item.project || 'unknown';
            const major = summarizeProjectName(resolveMajorProject(rawProj, rollupMap), rollupMap);
            if (!map.has(major)) map.set(major, []);
            map.get(major)!.push(item);
        }
        return Array.from(map.entries());
    })();
</script>

{#if abandonedOnly}
    {#if abandoned.length > 0}
        <div class="flex flex-col gap-3 font-mono">
            <h3 class="text-xs font-bold tracking-[0.2em] text-yellow-500/80 border-b border-yellow-500/30 pb-2 flex items-center gap-2">
                <span class="material-symbols-outlined text-[14px]">warning</span>
                DROPPED THREADS ({abandoned.length})
            </h3>
            <div class="flex flex-col gap-4">
                {#each abandonedByProject as [project, items]}
                    <div class="flex flex-col gap-2 pl-3" style="border-left: 3px solid {projectColor(project)};">
                        <span class="text-[10px] font-bold px-1.5 py-0.5 w-fit uppercase tracking-wider"
                              style="background: {projectBgTint(project)}; color: {projectColor(project)};">{project}</span>
                        {#each items as item}
                            <div class="flex items-start gap-2 text-xs">
                                <span class="text-[10px] text-yellow-500/60 shrink-0 pt-0.5">{item.time_ago || ""}</span>
                                <span class="text-yellow-500/90">{item.description}</span>
                            </div>
                        {/each}
                    </div>
                {/each}
            </div>
        </div>
    {/if}

{:else if mergedActivity.length > 0}
    <div class="flex flex-col gap-4 font-mono text-primary">
        <h3 class="text-xs font-bold tracking-[0.2em] text-primary/80 border-b border-primary/30 pb-2">
            RECENT ACTIVITY
            <span class="text-primary/40 font-normal ml-2">({mergedActivity.length} projects)</span>
        </h3>

        <div class="flex flex-col gap-4">
            {#each visible as group}
                <div class="flex flex-col gap-1.5">
                    <div class="flex items-center gap-3 text-xs">
                        <span class="font-bold px-2 py-0.5 uppercase tracking-wider"
                              style="background: {projectBgTint(group.project)}; color: {projectColor(group.project)}; border: 1px solid {projectBorderColor(group.project)};">
                            {group.project}
                        </span>
                        <span class="text-primary/30 ml-auto">{group.period}</span>
                    </div>

                    <div class="flex flex-col gap-0.5 ml-1" style="border-left: 2px solid {projectColor(group.project)}30; padding-left: 8px;">
                        {#each group.items.slice(0, 5) as item}
                            <div class="flex items-start gap-2 text-xs py-0.5 cursor-pointer hover:text-primary transition-colors"
                                 role="button" tabindex="0"
                                 on:click={() => { const id = findNodeForTask(item.text, group.project); if (id) toggleSelection(id); }}
                                 on:keydown={(e) => { if (e.key === 'Enter') { const id = findNodeForTask(item.text, group.project); if (id) toggleSelection(id); } }}>
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
