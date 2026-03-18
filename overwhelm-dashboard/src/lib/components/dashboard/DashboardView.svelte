<script lang="ts">
    export let data: any;

    import { graphData } from "../../stores/graph";
    import ActiveSessions from "./ActiveSessions.svelte";
    import WhereYouLeftOff from "./WhereYouLeftOff.svelte";
    import SynthesisPanel from "./SynthesisPanel.svelte";
    import PathTimeline from "./PathTimeline.svelte";
    import ProjectDashboard from "./ProjectDashboard.svelte";
    import QuickCapture from "./QuickCapture.svelte";

    // Extract dynamic project list from graph data
    $: projects = $graphData ? Array.from(new Set($graphData.nodes.map(n => n.project).filter(p => !!p))).sort() : [];
    
    // Fallback logic if server data is empty
    $: activeSessionsData = data?.dashboardData?.active_agents?.length ? data.dashboardData.active_agents : 
        ($graphData ? $graphData.nodes.filter(n => n.status === 'in_progress').map(n => ({
            project: n.project || 'Uncategorized',
            description: n.label,
            started_at: (n as any)._raw?.modified || new Date().toISOString()
        })) : []);
        
    $: leftOffActive = data?.dashboardData?.left_off?.active?.length ? data.dashboardData.left_off.active : 
        ($graphData ? $graphData.nodes.filter(n => n.status === 'in_progress').slice(0, 3).map(n => ({
            project: n.project || 'Uncategorized',
            description: n.label,
            now_task: n.label,
            time_display: 'recently'
        })) : []);

    $: leftOffPaused = data?.dashboardData?.left_off?.paused?.length ? data.dashboardData.left_off.paused :
        ($graphData ? $graphData.nodes.filter(n => n.status === 'paused' || n.status === 'waiting').slice(0, 3).map(n => ({
            project: n.project || 'Uncategorized',
            description: n.label,
            time_display: 'earlier'
        })) : []);
        
    $: hasLeftOff = leftOffActive.length > 0 || leftOffPaused.length > 0;
</script>

<div class="h-full p-8 font-mono text-primary flex flex-col gap-6">
    <div class="border border-primary/30 bg-surface p-4">
        <ActiveSessions
            sessions={activeSessionsData}
            needsYou={data?.dashboardData?.needs_you || []}
        />
    </div>

    <div class="grid grid-cols-12 gap-6 flex-1 min-h-0">
        <div class="col-span-8 flex flex-col gap-6 overflow-y-auto custom-scrollbar pr-2">
            <div class="border border-primary/30 bg-surface p-4">
                <WhereYouLeftOff leftOff={{ active: leftOffActive, paused: leftOffPaused }} />
            </div>

            <div class="border border-primary/30 bg-surface p-4">
                <ProjectDashboard
                    projectProjects={data?.dashboardData?.project_projects?.length ? data.dashboardData.project_projects : projects}
                    projectData={data?.dashboardData?.project_data || {}}
                />
            </div>

            {#if data?.dashboardData?.path}
                <div class="border border-primary/30 bg-surface p-4">
                    <PathTimeline path={data?.dashboardData?.path} />
                </div>
            {/if}
        </div>

        <div class="col-span-4 flex flex-col gap-6 overflow-y-auto custom-scrollbar pr-2 bg-black/40 p-4 border border-primary/20 rounded-xl">
            <div class="border border-primary/30 bg-surface p-4 shadow-lg">
                <QuickCapture />
            </div>

            <div class="border border-primary/30 bg-surface p-4 shadow-lg">
                <SynthesisPanel
                    synthesis={data?.dashboardData?.synthesis}
                    dailyStory={data?.dashboardData?.daily_story}
                />
            </div>
        </div>
    </div>
</div>
