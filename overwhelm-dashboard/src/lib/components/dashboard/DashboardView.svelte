<script lang="ts">
    export let data: any;

    import { graphData } from "../../stores/graph";
    import ActiveSessions from "./ActiveSessions.svelte";
    import RecentSessions from "./RecentSessions.svelte";
    import SynthesisPanel from "./SynthesisPanel.svelte";
    import PathTimeline from "./PathTimeline.svelte";
    import ProjectDashboard from "./ProjectDashboard.svelte";

    // Extract dynamic project list from graph data
    $: projects = $graphData ? Array.from(new Set($graphData.nodes.map(n => n.project).filter(p => !!p))).sort() : [];

    // Fallback logic if server data is empty
    $: activeSessionsData = data?.dashboardData?.active_agents?.length ? data.dashboardData.active_agents :
        ($graphData ? $graphData.nodes.filter(n => n.status === 'in_progress').map(n => ({
            project: n.project || 'Uncategorized',
            description: n.label,
            started_at: (n as any)._raw?.modified || new Date().toISOString(),
            status_badge: 'running',
            needs_you: false,
            bucket: 'active',
        })) : []);

    // Build dropped threads from graph data when path reconstruction has none
    $: droppedFromGraph = $graphData ? $graphData.nodes
        .filter(n => n.type === 'task' && ['active', 'in_progress'].includes(n.status) && n._raw?.created)
        .filter(n => {
            const created = new Date(n._raw.created).getTime();
            const modified = n._raw?.modified ? new Date(n._raw.modified).getTime() : created;
            const daysSinceModified = (Date.now() - modified) / 86400000;
            return daysSinceModified > 3;
        })
        .sort((a, b) => (a.priority ?? 5) - (b.priority ?? 5))
        .slice(0, 10)
        .map(n => ({
            project: n.project || 'unknown',
            description: n.label,
            time_ago: (() => {
                const mod = n._raw?.modified ? new Date(n._raw.modified).getTime() : Date.now();
                const days = Math.round((Date.now() - mod) / 86400000);
                return days > 0 ? `${days}d ago` : 'recently';
            })(),
        })) : [];

    $: pathData = data?.dashboardData?.path || { threads: [], abandoned_work: [] };
    $: {
        if (pathData.abandoned_work.length === 0 && droppedFromGraph.length > 0) {
            pathData = { ...pathData, abandoned_work: droppedFromGraph };
        }
    }

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
                ['active', 'in_progress', 'blocked', 'waiting', 'review'].includes(n.status)
            );
            result.tasks[p] = projTasks.length > 0 ? projTasks : (serverData.tasks?.[p] || []);

            if (!result.meta[p]) result.meta[p] = {};

            const projEpics = gd.nodes.filter((n: any) =>
                n.type === 'epic' && n.project === p &&
                !['done', 'completed', 'cancelled'].includes(n.status)
            );
            if (projEpics.length > 0) {
                result.meta[p].epics = projEpics.map((e: any) => {
                    const children = gd.nodes.filter((n: any) => n.parent === e.id);
                    const done = children.filter((n: any) => ['done', 'completed'].includes(n.status)).length;
                    return { title: e.label, progress: { completed: done, total: children.length } };
                });
            }

            if (!result.accomplishments[p] || result.accomplishments[p].length === 0) {
                const doneTasks = gd.nodes
                    .filter((n: any) => n.type === 'task' && n.project === p && ['done', 'completed'].includes(n.status))
                    .sort((a: any, b: any) => (b.modified || 0) - (a.modified || 0))
                    .slice(0, 5);
                if (doneTasks.length > 0) {
                    result.accomplishments[p] = doneTasks.map((t: any) => ({ description: t.label }));
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
    </div>

    <!-- PRIORITY 1: What's running + what needs you (above the fold) -->
    <div class="border border-primary/30 bg-surface p-4">
        <ActiveSessions
            sessions={activeSessionsData}
            pausedSessions={data?.dashboardData?.paused_sessions || []}
            staleSessions={data?.dashboardData?.stale_sessions || []}
            needsYou={data?.dashboardData?.needs_you || []}
        />
    </div>

    <!-- PRIORITY 2: Today's Story (narrative context recovery) -->
    <div class="border border-primary/30 bg-surface p-4">
        <SynthesisPanel
            synthesis={data?.dashboardData?.synthesis}
            dailyStory={data?.dashboardData?.daily_story}
            inline={true}
        />
    </div>

    <!-- PRIORITY 3: Dropped threads — promoted to standalone section -->
    {#if pathData.abandoned_work?.length > 0}
        <div class="border border-yellow-500/30 bg-surface p-4">
            <PathTimeline path={pathData} abandonedOnly={true} />
        </div>
    {/if}

    <!-- PRIORITY 4: Recent activity feed (what happened, by project) -->
    {#if pathData.activity?.length > 0}
        <div class="border border-primary/30 bg-surface p-4">
            <PathTimeline path={pathData} />
        </div>
    {/if}

    <!-- PRIORITY 5: Project details + sessions — use auto height, not flex-1 -->
    <div class="grid grid-cols-1 lg:grid-cols-12 gap-6">
        <div class="lg:col-span-8 flex flex-col gap-6">
            <div class="border border-primary/30 bg-surface p-4">
                <RecentSessions synthesis={data?.dashboardData?.synthesis} />
            </div>

            <div class="border border-primary/30 bg-surface p-4">
                <ProjectDashboard
                    projectProjects={enrichedProjects}
                    projectData={graphProjectData}
                />
            </div>
        </div>

        <div class="lg:col-span-4 flex flex-col gap-6 bg-black/40 p-4 border border-primary/20 rounded-xl">
            {#if data?.dashboardData?.synthesis?.blockers || data?.dashboardData?.synthesis?.recent_context}
                <div class="border border-primary/30 bg-surface p-4 shadow-lg">
                    <SynthesisPanel
                        synthesis={data?.dashboardData?.synthesis}
                        dailyStory={null}
                        inline={false}
                    />
                </div>
            {/if}
        </div>
    </div>
</div>
