<script lang="ts">
    export let data: any;

    import { graphData } from "../../stores/graph";
    import ActiveSessions from "./ActiveSessions.svelte";
    import ProjectDashboard from "./ProjectDashboard.svelte";
    import { INCOMPLETE_STATUSES, COMPLETED_STATUSES } from "../../data/constants";

    // Pseudo-projects come from $AOPS_SESSIONS/projects.json (loaded server-side).
    $: pseudoProjects = new Set<string>(data?.dashboardData?.projects_config?.pseudo_projects || []);

    // Extract dynamic project list from graph data
    $: projects = $graphData ? Array.from(new Set($graphData.nodes.map(n => n.project).filter(p => !!p && !pseudoProjects.has(p as string)))).sort() : [];

    // Session data comes exclusively from server-side sources (synthesis.json / session-state files).
    // No client-side fallback — if the pipeline isn't producing data, the UI shows errors.
    $: activeSessionsData = data?.dashboardData?.active_agents || [];
    $: pipelineErrors = data?.dashboardData?.pipeline_errors || [];
    $: pathData = data?.dashboardData?.path || { activity: [], abandoned_work: [] };

    // Interactive vs Background
    $: interactiveSessions = activeSessionsData.filter((s: any) => s.session_type === 'interactive');
    $: backgroundSessions = activeSessionsData.filter((s: any) => s.session_type !== 'interactive');

    // Build enriched project data from graph store (primary) + server data (enrichment)
    $: graphProjectData = (() => {
        if (!$graphData) return data?.dashboardData?.project_data || {};
        const serverData = data?.dashboardData?.project_data || { meta: {}, tasks: {}, accomplishments: {}, sessions: {} };
        const result: any = { meta: { ...serverData.meta }, tasks: {}, accomplishments: { ...serverData.accomplishments }, sessions: { ...serverData.sessions } };

        const gd = $graphData;
        for (const proj of projects) {
            if (!proj) continue;
            const p = proj as string;
            const projTasks = gd.nodes.filter((n: any) =>
                n.type === 'task' && n.project === p &&
                INCOMPLETE_STATUSES.has(n.status)
            );
            result.tasks[p] = projTasks.length > 0 ? projTasks : (serverData.tasks?.[p] || []);

            if (!result.meta[p]) result.meta[p] = {};

            const projEpics = gd.nodes.filter((n: any) =>
                n.type === 'epic' && n.project === p &&
                !COMPLETED_STATUSES.has(n.status)
            );
            if (projEpics.length > 0) {
                result.meta[p].epics = projEpics.map((e: any) => {
                    const children = gd.nodes.filter((n: any) => n.parent === e.id);
                    const done = children.filter((n: any) => n.status === 'done').length;
                    const outstandingChildren = children.filter((n: any) => !COMPLETED_STATUSES.has(n.status));
                    const hasPriorityTask = outstandingChildren.some((n: any) => n.priority === 0 || n.priority === 1);
                    return { id: e.id, title: e.label, progress: { completed: done, total: children.length }, hasPriorityTask };
                });
            }

            if (!result.accomplishments[p] || result.accomplishments[p].length === 0) {
                const doneTasks = gd.nodes
                    .filter((n: any) => n.type === 'task' && n.project === p && n.status === 'done')
                    .sort((a: any, b: any) => (b.modified || 0) - (a.modified || 0))
                    .slice(0, 5);
                if (doneTasks.length > 0) {
                    result.accomplishments[p] = doneTasks.map((t: any) => {
                        const mod = t._raw?.modified ? new Date(t._raw.modified).getTime() : 0;
                        const days = mod ? Math.round((Date.now() - mod) / 86400000) : 0;
                        const time_ago = days > 0 ? `${days}d ago` : mod ? 'today' : '';
                        return { description: t.label, time_ago };
                    });
                }
            }

            if (!result.sessions[p]) result.sessions[p] = [];
        }

        return result;
    })();

    $: enrichedProjects = projects.length > 0 ? projects :
        (data?.dashboardData?.project_projects || []);
</script>

<div class="h-full p-8 font-mono text-primary flex flex-col gap-6 relative" data-component="dashboard">
    <!-- US-D7: Above-the-fold triage bar — answers "running?", "dropped?", "needs me?" in 5 seconds -->
    <div class="flex items-center gap-4 text-xs">
        <div class="flex items-center gap-2 bg-primary/10 border border-primary/30 px-3 py-2">
            <span class="material-symbols-outlined text-[14px]">bolt</span>
            <span class="font-bold">{activeSessionsData.length}</span>
            <span class="text-primary/60">running</span>
        </div>
        {#if (data?.dashboardData?.needs_you || []).length > 0}
            <div class="flex items-center gap-2 bg-red-900/20 border border-red-500/50 px-3 py-2 text-red-400 animate-pulse">
                <span class="material-symbols-outlined text-[14px]">warning</span>
                <span class="font-bold">{(data?.dashboardData?.needs_you || []).length}</span>
                <span>needs you</span>
            </div>
        {:else}
            <div class="flex items-center gap-2 bg-primary/5 border border-primary/20 px-3 py-2 text-primary/40">
                <span class="material-symbols-outlined text-[14px]">check_circle</span>
                <span>nothing needs you</span>
            </div>
        {/if}
        {#if pathData.abandoned_work?.length > 0}
            <div class="flex items-center gap-2 bg-yellow-900/20 border border-yellow-500/40 px-3 py-2 text-yellow-500">
                <span class="material-symbols-outlined text-[14px]">sync_problem</span>
                <span class="font-bold">{pathData.abandoned_work.length}</span>
                <span class="text-yellow-500/70">dropped threads</span>
            </div>
        {/if}
        {#each pipelineErrors as err}
            <div class="flex items-center gap-2 bg-red-900/20 border border-red-500/40 px-3 py-2 text-red-400">
                <span class="material-symbols-outlined text-[14px]">error</span>
                <span>{err}</span>
            </div>
        {/each}
    </div>

    <!-- PRIORITY 1: What's running + what needs you (above the fold) -->
    <div class="flex flex-col gap-6">
        <div class="border border-primary/30 bg-surface p-4">
            <ActiveSessions
                sessions={interactiveSessions}
                needsYou={data?.dashboardData?.needs_you || []}
                title="RECENT SESSIONS"
            />
        </div>
        {#if backgroundSessions.length > 0}
            <div class="border border-primary/20 bg-surface/50 p-4">
                <ActiveSessions
                    sessions={backgroundSessions}
                    title="BACKGROUND ACTIVITY"
                    compact={true}
                />
            </div>
        {/if}
    </div>

    <!-- PRIORITY 2: Project details (sessions + tasks) -->
    <div class="border border-primary/30 bg-surface p-4">
        <ProjectDashboard
            projectProjects={enrichedProjects}
            projectData={graphProjectData}
        />
    </div>
</div>
