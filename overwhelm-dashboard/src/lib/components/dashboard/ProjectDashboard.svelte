<script lang="ts">
    import { graphData } from "../../stores/graph";
    import { toggleSelection } from "../../stores/selection";
    import { projectColor, projectBgTint, projectBorderColor, buildProjectRollupMap, summarizeProjectName, resolveMajorProject } from "../../data/projectUtils";
    import { copyToClipboard } from "../../data/utils";
    import TaskActionButtons from "../shared/TaskActionButtons.svelte";
    import AssigneeBadge from "../shared/AssigneeBadge.svelte";
    import { INCOMPLETE_STATUSES, STATUS_FILLS, STATUS_TEXT, STATUS_LABELS } from "../../data/constants";

    function statusChipStyle(status: string | undefined) {
        const s = status || 'inbox';
        const fill = STATUS_FILLS[s] ?? '#1f2937';
        const text = STATUS_TEXT[s] ?? '#e5e7eb';
        return `background:${fill};color:${text};border-color:${fill};`;
    }
    function statusChipLabel(status: string | undefined) {
        const s = status || 'inbox';
        return STATUS_LABELS[s] ?? s.toUpperCase().replace('_', ' ');
    }
    export let projectProjects: string[] = [];
    export let projectData: any = {};

    // Build rollup map from graph data
    $: rollupMap = $graphData ? buildProjectRollupMap($graphData.nodes) : new Map<string, string>();

    // Merge sub-projects into major projects
    $: mergedProjects = (() => {
        const majorSet = new Set<string>();
        for (const p of projectProjects) {
            majorSet.add(summarizeProjectName(resolveMajorProject(p, rollupMap), rollupMap));
        }
        return Array.from(majorSet);
    })();

    // Map from major project → all raw project names that roll up into it
    $: projectMembers = (() => {
        const map = new Map<string, string[]>();
        for (const p of projectProjects) {
            const major = summarizeProjectName(resolveMajorProject(p, rollupMap), rollupMap);
            if (!map.has(major)) map.set(major, []);
            map.get(major)!.push(p);
        }
        return map;
    })();

    // Show projects that have any activity (tasks, accomplishments, or sessions)
    $: activeProjects = mergedProjects.filter(project => {
        const members = projectMembers.get(project) || [project];
        const storeTasks = $graphData ? $graphData.nodes.filter(n => n.type === 'task' && members.includes(n.project || '') && INCOMPLETE_STATUSES.has(n.status)) : [];
        const tasks = storeTasks.length > 0 ? storeTasks : members.flatMap(p => projectData.tasks?.[p] || []);
        const accomplishments = members.flatMap(p => projectData.accomplishments?.[p] || []);
        const sessions = members.flatMap(p => projectData.sessions?.[p] || []);
        
        return tasks.length > 0 || accomplishments.length > 0 || sessions.length > 0;
    });

    $: hasData = activeProjects.length > 0;

    // Sort by most recent session timestamp as per spec
    $: sortedProjects = [...activeProjects].sort((a, b) => {
        const aMembers = projectMembers.get(a) || [a];
        const bMembers = projectMembers.get(b) || [b];
        const aLatest = Math.max(...aMembers.map(p => (projectData.meta?.[p] || {}).latest_session || 0));
        const bLatest = Math.max(...bMembers.map(p => (projectData.meta?.[p] || {}).latest_session || 0));
        return bLatest - aLatest;
    });

    function dedup(items: any[]): any[] {
        return items.filter((acc, i, arr) => arr.findIndex(a => a.description === acc.description) === i);
    }
</script>

{#if hasData}
    <div class="flex flex-col gap-6 font-mono text-primary">
        {#each sortedProjects as project}
            {@const members = projectMembers.get(project) || [project]}
            {@const meta = members.reduce((acc, p) => ({ ...acc, ...(projectData.meta?.[p] || {}) }), {} as any)}
            {@const allEpics = members.flatMap(p => (projectData.meta?.[p] || {}).epics || []).filter(e => e.hasPriorityTask)}
            {@const storeTasks = $graphData ? $graphData.nodes.filter(n => n.type === 'task' && members.includes(n.project || '') && INCOMPLETE_STATUSES.has(n.status)) : []}
            {@const tasks = storeTasks.length > 0 ? storeTasks : members.flatMap(p => projectData.tasks?.[p] || [])}
            {@const accomplishments = members.flatMap(p => projectData.accomplishments?.[p] || [])}
            {@const sessions = members.flatMap(p => projectData.sessions?.[p] || [])}

            {#if tasks.length > 0 || accomplishments.length > 0 || sessions.length > 0}
                <div class="flex flex-col gap-4">
                    <div class="flex justify-between items-center pb-2" style="border-bottom: 2px solid {projectBorderColor(project)};">
                        <h3 class="text-sm font-bold tracking-[0.2em] flex items-center gap-2 cursor-pointer hover:text-primary transition-colors"
                            style="color: {projectColor(project)};"
                            role="button" tabindex="0"
                            onclick={() => { const pNode = $graphData?.nodes.find(n => members.includes(n.project || '') && n.type === 'project'); if (pNode) toggleSelection(pNode.id); }}
                            onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { if (e.key === ' ') e.preventDefault(); const pNode = $graphData?.nodes.find(n => members.includes(n.project || '') && n.type === 'project'); if (pNode) toggleSelection(pNode.id); } }}>
                            <span class="material-symbols-outlined text-[16px]">folder_open</span>
                            {project.toUpperCase()}
                            {#if meta.is_spotlight}
                                <span class="bg-primary text-black text-[10px] px-2 py-0.5 ml-2 font-bold tracking-widest animate-pulse">
                                    SPOTLIGHT
                                </span>
                            {/if}
                        </h3>
                    </div>

                    <div class="flex flex-col gap-4">
                        {#if allEpics.length > 0}
                            <div class="grid gap-3" style="grid-template-columns: repeat(auto-fill, minmax(350px, 1fr));">
                                {#each allEpics.slice(0, 3) as epic}
                                    <div class="bg-black/40 border border-primary/20 p-3 hover:border-primary transition-colors cursor-pointer"
                                         role="button" tabindex="0"
                                         onclick={() => { const eNode = $graphData?.nodes.find(n => n.label === epic.title && n.type === 'epic'); if (eNode) toggleSelection(eNode.id); }}
                                         onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { if (e.key === ' ') e.preventDefault(); const eNode = $graphData?.nodes.find(n => n.label === epic.title && n.type === 'epic'); if (eNode) toggleSelection(eNode.id); } }}>
                                        <div class="flex justify-between items-center mb-2">
                                            <span class="text-xs font-bold truncate pr-2">{epic.title}</span>
                                            {#if epic.progress}
                                                <span class="text-[10px] text-primary/60 shrink-0"
                                                    >{epic.progress.completed}/{epic.progress.total}</span
                                                >
                                            {/if}
                                        </div>
                                        {#if epic.progress && epic.progress.total > 0}
                                            <div class="h-1 w-full bg-black border border-primary/30">
                                                <div
                                                    class="h-full"
                                                    style="width: {(epic.progress.completed / epic.progress.total) * 100}%; background: {projectColor(project)};"
                                                ></div>
                                            </div>
                                        {/if}
                                    </div>
                                {/each}
                            </div>
                        {/if}

                        <div class="grid gap-6" style="grid-template-columns: repeat(auto-fill, minmax(350px, 1fr));">
                            <!-- Active Tasks Column -->
                            <div class="flex flex-col gap-2">
                                <h4 class="text-[10px] font-bold tracking-widest text-primary/60 mb-1">TOP PRIORITIES & NEXT TASKS</h4>
                                {#each [...tasks].sort((a, b) => (a.priority ?? 5) - (b.priority ?? 5)).slice(0, 3) as task}
                                    {@const taskId = task.id || task.task_id || ''}
                                    {@const shortId = taskId.slice(-8)}
                                    <div class="group flex items-start gap-2 p-2 bg-primary/5 border-l-2 {task.priority === 0 ? 'border-red-500' : task.priority === 1 ? 'border-orange-500' : 'border-primary/50'} hover:bg-primary/10 transition-colors cursor-pointer"
                                         role="button" tabindex="0"
                                         onclick={() => toggleSelection(taskId)}
                                         onkeydown={(e) => { if (e.key === 'Enter') toggleSelection(taskId); }}>
                                        <span class="text-[10px] font-bold {task.priority === 0 ? 'text-red-500' : task.priority === 1 ? 'text-orange-500' : 'text-primary/70'}">P{task.priority !== undefined ? task.priority : '?'}</span>
                                        
                                        {#if taskId}
                                            <button class="text-[9px] font-bold bg-primary/20 text-primary/40 px-1 py-0.5 hover:bg-primary/40 transition-colors shrink-0" 
                                                    onclick={(e) => { e.stopPropagation(); copyToClipboard(taskId); }}
                                                    title="Click to copy task ID: {taskId}">
                                                {shortId}
                                            </button>
                                        {/if}

                                        <AssigneeBadge assignee={task.assignee} compact={true} />
                                        <span class="text-xs text-primary/90 flex-1">{task.title || task.label}</span>
                                        <span class="text-[10px] font-bold px-1 py-0.5 shrink-0 border rounded-sm {task.status === 'in_progress' ? 'animate-pulse' : ''}"
                                              style={statusChipStyle(task.status)}>{statusChipLabel(task.status)}</span>
                                        {#if taskId}
                                            <TaskActionButtons taskId={taskId} />
                                        {/if}
                                    </div>
                                {:else}
                                    <div class="text-xs text-primary/40 italic">No active tasks.</div>
                                {/each}
                                {#if tasks.length > 3}
                                    <button class="text-[10px] text-primary/30 hover:text-primary/60 text-left pl-2 transition-colors cursor-pointer"
                                            onclick={() => { const pNode = $graphData?.nodes.find(n => members.includes(n.project || '') && n.type === 'project'); if (pNode) toggleSelection(pNode.id); }}>
                                        · · · view all {tasks.length} active
                                    </button>
                                {/if}
                            </div>

                            <!-- Completed Column -->
                            <div class="flex flex-col gap-2">
                                <h4 class="text-[10px] font-bold tracking-widest text-primary/60 mb-1">RECENTLY COMPLETED</h4>
                                {#each dedup(accomplishments).slice(0, 3) as acc}
                                    <div class="flex items-start gap-2 p-2 border border-primary/10 bg-black/30 hover:border-primary/30 transition-colors">
                                        <span class="material-symbols-outlined text-[14px] text-green-500">check</span>
                                        <span class="text-xs text-primary/70 line-clamp-2 flex-1">{acc.description}</span>
                                        {#if acc.time_ago}
                                            <span class="text-[10px] text-primary/40 shrink-0">{acc.time_ago}</span>
                                        {/if}
                                    </div>
                                {:else}
                                    <div class="text-xs text-primary/40 italic">
                                        Nothing recently completed.
                                    </div>
                                {/each}
                                {#if dedup(accomplishments).length > 3}
                                    <button class="text-[10px] text-primary/30 hover:text-primary/60 text-left pl-2 transition-colors cursor-pointer"
                                            onclick={() => { const pNode = $graphData?.nodes.find(n => members.includes(n.project || '') && n.type === 'project'); if (pNode) toggleSelection(pNode.id); }}>
                                        · · · view all {dedup(accomplishments).length} completed
                                    </button>
                                {/if}
                            </div>
                        </div>
                    </div>
                </div>
            {/if}
        {/each}
    </div>
{/if}
