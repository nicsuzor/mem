<script lang="ts">
    import { graphData, updateGraphTaskNode } from "../../stores/graph";
    import { selection } from "../../stores/selection";
    import { filters } from "../../stores/filters";
    import { describeTaskMutation, taskOperations } from "../../stores/taskOperations";
    import { PRIORITIES } from "../../data/constants";
    import { projectHue } from "../../data/projectUtils";
    import TaskEditorView from "./TaskEditorView.svelte";
    import StatusFilterBar from "../shared/StatusFilterBar.svelte";

    let currentTab = "ACTIVE_TASKS";
    let searchQuery = "";
    let sortField = "priority";
    let sortAsc = true;

    // Build the directory tree (Goals -> Projects -> Epics/Tasks)
    $: projects = $graphData ? Array.from(new Set($graphData.nodes.map(n => n.project).filter((p): p is string => !!p))).sort() : [];

    let expandedProjects: Record<string, boolean> = {};

    function toggleProject(p: string) {
        expandedProjects[p] = !expandedProjects[p];
    }

    function selectProject(p: string | 'ALL') {
        filters.update(f => {
            if (p === 'ALL') {
                return { ...f, hiddenProjects: [] };
            } else {
                return { ...f, hiddenProjects: projects.filter(proj => proj !== p) };
            }
        });
    }

    $: isAllProjects = ($filters.hiddenProjects?.length ?? 0) === 0;
    // Determine which project is active in the sidebar.
    // If exactly one project is visible (all others are hidden), consider it active.
    $: activeProject = isAllProjects ? 'ALL' :
        (projects.length > 0 && ($filters.hiddenProjects?.length ?? 0) === projects.length - 1)
        ? projects.find(p => !($filters.hiddenProjects?.includes(p)))
        : 'MIXED';

    function toggleSort(field: string) {
        if (sortField === field) {
            sortAsc = !sortAsc;
        } else {
            sortField = field;
            sortAsc = true;
        }
    }

    async function toggleTaskStatus(task: any, isChecked: boolean) {
        const newStatus = isChecked ? 'done' : 'ready';

        const { rollback } = updateGraphTaskNode(task.id, { status: newStatus });
        const operationId = taskOperations.start(task.id, describeTaskMutation({ status: newStatus }));

        // Persist
        try {
            const res = await fetch('/api/task/status', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ id: task.id, status: newStatus })
            });
            if (res.ok) {
                taskOperations.succeed(operationId);
            } else {
                rollback();
                taskOperations.fail(operationId, 'Failed to update status');
            }
        } catch (e: any) {
            console.error("Failed to update task status", e);
            rollback();
            taskOperations.fail(operationId, e.message ?? 'Network error');
        }
    }

    $: focusIds = ($graphData as any)?.focusIds as Set<string> || new Set<string>();

    $: tasks = $graphData ? $graphData.nodes.filter(n => {
        const matchesType = n.type === 'task';
        const matchesProject = !($filters.hiddenProjects?.includes(n.project || ''));

        let matchesTab = false;
        if (currentTab === 'ACTIVE_TASKS') {
            matchesTab = !['done', 'completed', 'cancelled', 'deferred', 'paused', 'backlog'].includes(n.status);
        } else if (currentTab === 'COMPLETED') {
            matchesTab = ['done', 'completed'].includes(n.status);
        } else if (currentTab === 'BACKLOG') {
            matchesTab = ['backlog', 'deferred', 'paused', 'cancelled'].includes(n.status);
        }

        let matchesSearch = true;
        if (searchQuery.trim() !== '') {
            const q = searchQuery.toLowerCase();
            matchesSearch = (n.label || '').toLowerCase().includes(q) || (n.id || '').toLowerCase().includes(q);
        }

        const matchesStatusFilter = $filters.selectedStatuses.length === 0 || $filters.selectedStatuses.includes(n.status);

        return matchesType && matchesProject && matchesTab && matchesSearch && matchesStatusFilter;
    }).sort((a, b) => {
        // Pin focus tasks to top
        const aFocus = focusIds.has(a.id) ? 0 : 1;
        const bFocus = focusIds.has(b.id) ? 0 : 1;
        if (aFocus !== bFocus) return aFocus - bFocus;

        let valA, valB;
        if (sortField === 'priority') {
            valA = a.priority ?? 5;
            valB = b.priority ?? 5;
        } else if (sortField === 'status') {
            valA = a.status || '';
            valB = b.status || '';
        } else if (sortField === 'assignee') {
            valA = a.assignee || 'zzzz';
            valB = b.assignee || 'zzzz';
        } else {
            valA = a.label || '';
            valB = b.label || '';
        }

        if (valA < valB) return sortAsc ? -1 : 1;
        if (valA > valB) return sortAsc ? 1 : -1;
        return 0;
    }) : [];

    $: activeCount = $graphData ? $graphData.nodes.filter(n => n.type === 'task' && !['done', 'completed', 'cancelled', 'deferred', 'paused', 'backlog'].includes(n.status)).length : 0;
</script>

<div class="flex flex-1 overflow-hidden h-full relative" data-component="task-list">
    <!-- Directory Tree -->
    <aside class="w-64 border-r border-primary/20 bg-background flex flex-col shrink-0 relative" data-component="project-nav">
        <div class="p-4 border-b border-primary/10">
            <h3 class="text-xs font-bold text-primary/60 uppercase tracking-widest mb-1">Directory_Tree</h3>
            <p class="text-[10px] font-mono text-primary/40">WORKSPACE/PROJECTS</p>
        </div>
        <div class="flex-1 overflow-y-auto p-2 font-mono text-sm custom-scrollbar">
            <div class="mb-2">
                <button
                    class="flex items-center gap-2 p-2 w-full text-left rounded transition-colors group
                    {activeProject === 'ALL' ? 'text-primary bg-primary/20 border-l-2 border-primary' : 'text-primary/60 hover:bg-primary/10'}"
                    onclick={() => selectProject('ALL')}
                >
                    <span class="material-symbols-outlined text-lg">target</span>
                    <span class="flex-1 font-bold">ALL_PROJECTS</span>
                </button>
                <div class="ml-4 border-l border-primary/20 pl-2 mt-1 space-y-1">
                    {#each projects as project}
                        <div>
                            <div class="flex items-center gap-1 group">
                                <button
                                    class="flex-1 flex items-center gap-2 p-1.5 text-left rounded cursor-pointer transition-colors
                                    {activeProject === project ? 'text-primary bg-primary/20' : 'text-primary/80 hover:bg-primary/10'}"
                                    style="border-left: 3px solid hsl({projectHue(project)}, 45%, 45%);"
                                    onclick={() => selectProject(project)}
                                >
                                    <span class="material-symbols-outlined text-base">{activeProject === project || expandedProjects[project] ? 'folder_open' : 'folder'}</span>
                                    <span class="truncate">{project}</span>
                                </button>
                                <button
                                    class="p-1 text-primary/40 hover:text-primary transition-colors"
                                    onclick={(e) => { e.stopPropagation(); filters.update(f => {
                                        const hidden = f.hiddenProjects || [];
                                        return hidden.includes(project)
                                            ? { ...f, hiddenProjects: hidden.filter(p => p !== project) }
                                            : { ...f, hiddenProjects: [...hidden, project] };
                                    }); }}
                                >
                                    <span class="material-symbols-outlined text-sm">{expandedProjects[project] ? 'expand_more' : 'chevron_right'}</span>
                                </button>
                            </div>
                            {#if expandedProjects[project]}
                                <div class="ml-4 border-l border-primary/20 pl-2 mt-1 space-y-1">
                                    {#each ($graphData?.nodes || []).filter(n => n.project === project && n.type === 'task').slice(0, 8) as task}
                                        <button
                                            class="flex items-center gap-2 p-1.5 w-full text-left rounded cursor-pointer text-xs truncate transition-colors
                                            {$selection.activeNodeId === task.id ? 'text-primary bg-primary/10' : 'text-primary/60 hover:text-primary hover:bg-primary/5'}"
                                            onclick={() => selection.update(s => ({...s, activeNodeId: task.id}))}
                                        >
                                            <span class="material-symbols-outlined text-sm">{task.status === 'done' ? 'check_box' : 'check_box_outline_blank'}</span>
                                            <span class="truncate">{task.label}</span>
                                        </button>
                                    {/each}
                                </div>
                            {/if}
                        </div>
                    {/each}
                </div>
            </div>
        </div>
    </aside>

    <!-- Right Content: Breadcrumbs & Task List -->
    <section class="flex-1 flex flex-col bg-background relative overflow-hidden">
        <!-- Breadcrumbs -->
        <div class="px-6 py-4 flex items-center gap-3 border-b border-primary/10 bg-primary/5">
            <div class="flex items-center gap-2 text-primary/60 text-sm font-mono">
                <button class="hover:text-primary transition-colors cursor-pointer" onclick={() => selectProject('ALL')}>WORKSPACE</button>
                <span class="material-symbols-outlined text-xs">chevron_right</span>
                <span class="text-primary font-bold">{activeProject === 'ALL' ? 'ALL_TASKS' : (activeProject || '').toUpperCase()}</span>
            </div>

            <div class="ml-4 flex-1">
                <input
                    type="text"
                    bind:value={searchQuery}
                    placeholder="Search tasks..."
                    class="w-full bg-black/40 border border-primary/30 text-primary text-xs px-3 py-1.5 focus:ring-1 focus:ring-primary outline-none font-mono"
                />
            </div>

            <div class="ml-auto flex gap-2">
                <button class="bg-primary text-background-dark px-3 py-1 text-xs font-bold flex items-center gap-1 hover:brightness-110 font-mono transition-all cursor-pointer">
                    <span class="material-symbols-outlined text-sm">add</span> NEW_TASK
                </button>
            </div>
        </div>

        <!-- Status filter chips -->
        <div class="border-b border-primary/10 bg-surface/40">
            <StatusFilterBar />
        </div>

        <!-- Tabs -->
        <div class="px-6 border-b border-primary/10 flex gap-8 font-mono">
            {#each ['ACTIVE_TASKS', 'COMPLETED', 'BACKLOG'] as tab}
                <button
                    class="py-3 text-sm font-bold transition-colors {currentTab === tab ? 'text-primary border-b-2 border-primary' : 'text-primary/40 hover:text-primary'}"
                    onclick={() => currentTab = tab}
                >
                    {tab} {tab === 'ACTIVE_TASKS' ? `[${activeCount}]` : ''}
                </button>
            {/each}
        </div>

        <!-- Task Table -->
        <div class="flex-1 overflow-auto p-6 custom-scrollbar">
            <div class="border border-primary/20 bg-background shadow-xl">
                <table class="w-full text-left border-collapse font-mono">
                    <thead>
                        <tr class="bg-primary/10 border-b border-primary/20">
                            <th class="px-4 py-3 text-[10px] font-bold text-primary/70 uppercase tracking-widest w-32 cursor-pointer hover:bg-primary/20 transition-colors" onclick={() => toggleSort('id')}>ID {sortField === 'id' ? (sortAsc ? '▲' : '▼') : ''}</th>
                            <th class="px-4 py-3 text-[10px] font-bold text-primary/70 uppercase tracking-widest w-32 cursor-pointer hover:bg-primary/20 transition-colors" onclick={() => toggleSort('status')}>Status {sortField === 'status' ? (sortAsc ? '▲' : '▼') : ''}</th>
                            <th class="px-4 py-3 text-[10px] font-bold text-primary/70 uppercase tracking-widest cursor-pointer hover:bg-primary/20 transition-colors" onclick={() => toggleSort('label')}>Task_Name {sortField === 'label' ? (sortAsc ? '▲' : '▼') : ''}</th>
                            <th class="px-4 py-3 text-[10px] font-bold text-primary/70 uppercase tracking-widest w-32 cursor-pointer hover:bg-primary/20 transition-colors" onclick={() => toggleSort('assignee')}>Assignee {sortField === 'assignee' ? (sortAsc ? '▲' : '▼') : ''}</th>
                            <th class="px-4 py-3 text-[10px] font-bold text-primary/70 uppercase tracking-widest w-28 cursor-pointer hover:bg-primary/20 transition-colors" onclick={() => toggleSort('priority')}>Priority {sortField === 'priority' ? (sortAsc ? '▲' : '▼') : ''}</th>
                            <th class="px-4 py-3 text-[10px] font-bold text-primary/70 uppercase tracking-widest w-12"></th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-primary/10 text-sm">
                        {#each tasks as task}
                            <tr
                                class="hover:bg-primary/5 group transition-colors cursor-pointer {$selection.activeNodeId === task.id ? 'bg-primary/10' : ''} {focusIds.has(task.id) ? 'border-l-[3px] border-l-amber-500/80' : ''}"
                                style="border-left: 3px solid hsl({projectHue(task.project || '')}, 45%, 45%);"
                                onclick={() => selection.update(s => ({...s, activeNodeId: task.id}))}
                            >
                                <td class="px-4 py-4 text-primary/60 font-mono text-xs">
                                    {#if focusIds.has(task.id)}<span class="text-[9px] font-bold text-amber-500 mr-1">FOCUS</span>{/if}
                                    {task.id.length > 12 ? task.id.substring(0, 12) + '...' : task.id}
                                </td>
                                <td class="px-4 py-4">
                                    <span class="inline-flex items-center px-2 py-0.5 rounded text-[10px] font-bold border {task.status === 'in_progress' ? 'bg-primary/20 text-primary border-primary/30' : 'bg-primary/5 text-primary/60 border-primary/20'} uppercase">
                                        {task.status}
                                    </span>
                                </td>
                                <td class="px-4 py-4">
                                    <div class="flex flex-col">
                                        <span class="text-primary font-medium">{task.label}</span>
                                        <span class="text-[10px] text-primary/40 mt-1 uppercase">Project: {task.project || 'None'}</span>
                                    </div>
                                </td>
                                <td class="px-4 py-4">
                                    <div class="flex items-center gap-2">
                                        {#if task.assignee}
                                            <div class="size-6 bg-primary/10 border border-primary/30 flex items-center justify-center text-[10px] text-primary font-bold">
                                                {task.assignee.substring(0, 2).toUpperCase()}
                                            </div>
                                            <span class="text-primary/80 text-xs">{task.assignee}</span>
                                        {:else}
                                            <span class="text-primary/40 text-xs italic">Unassigned</span>
                                        {/if}
                                    </div>
                                </td>
                                <td class="px-4 py-4">
                                    <span class="inline-flex items-center gap-1.5 text-[10px] font-bold {task.priority <= 1 ? 'text-primary' : 'text-primary/60'}">
                                        <span class="size-1.5 rounded-full" style="background-color: {PRIORITIES[task.priority ?? 2]?.color ?? '#6c757d'};"></span>
                                        {PRIORITIES[task.priority ?? 2]?.short ?? 'ACTV'}
                                    </span>
                                </td>
                                <td class="px-4 py-4 text-right">
                                    <button class="opacity-0 group-hover:opacity-100 p-1 text-primary hover:bg-primary/20 transition-all cursor-pointer" onclick={(e) => { e.stopPropagation(); selection.update(s => ({...s, activeNodeId: task.id})); }}>
                                        <span class="material-symbols-outlined text-lg">edit</span>
                                    </button>
                                </td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            </div>

            {#if tasks.length === 0}
                <div class="text-primary/40 italic p-6 text-center text-sm font-mono mt-4">
                    No tasks found matching criteria.
                </div>
            {/if}
        </div>
    </section>

    <!-- Task Editor: Integrated Side-by-Side View -->
    {#if $selection.activeNodeId}
        <aside class="w-[45%] border-l border-primary/30 bg-background shadow-2xl z-10 transition-all animate-in slide-in-from-right duration-300">
            <TaskEditorView taskId={$selection.activeNodeId} onclose={() => selection.update(s => ({...s, activeNodeId: null}))} />
        </aside>
    {/if}
</div>
