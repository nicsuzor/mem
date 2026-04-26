<script lang="ts">
    import { graphData, updateGraphTaskNode } from "../../stores/graph";
    import HierarchyTree from "./HierarchyTree.svelte";
    import { describeTaskMutation, taskOperations } from "../../stores/taskOperations";
    import { PRIORITIES, STATUS_FILLS, STATUS_TEXT, COMPLETED_STATUSES } from "../../data/constants";

    const HIDDEN_METADATA_KEYS = new Set([
        'body', 'id', 'title', 'label', 'node_type', 'status', 'priority', 'project', 'assignee',
        'layouts', 'x', 'y', 'depth', 'maxDepth', 'lines', 'dw', 'downstream_weight', 'modified',
        'created', 'isLeaf', 'parent', 'fullTitle', '_safe_parent', 'filter_dimmed', 'path', 'refile',
        'scope', 'uncertainty', 'criticality',
    ]);

    // Canonical status lifecycle (see aops-core/TAXONOMY.md):
    //   inbox → ready → queued → in_progress → merge_ready → done
    // `ready` is auto-computed from decomposition + dep state.
    // `queued` is the human gate that makes a task dispatchable to agents.
    const STATE_DETAILS: Record<string, { label: string; icon: string; summary: string; tone: 'neutral' | 'ready' | 'active' | 'warning' | 'danger' | 'success' }> = {
        inbox: { label: 'Inbox', icon: 'inbox', summary: 'Captured but not yet triaged.', tone: 'neutral' },
        ready: { label: 'Ready', icon: 'task_alt', summary: 'Decomposed and unblocked (auto).', tone: 'ready' },
        queued: { label: 'Queued', icon: 'playlist_add_check', summary: 'Available for agent dispatch.', tone: 'ready' },
        in_progress: { label: 'In Progress', icon: 'play_circle', summary: 'Claimed and actively being worked.', tone: 'active' },
        merge_ready: { label: 'Merge Ready', icon: 'commit', summary: 'Work complete, awaiting merge.', tone: 'active' },
        review: { label: 'Review', icon: 'rate_review', summary: 'Awaiting human review.', tone: 'warning' },
        blocked: { label: 'Blocked', icon: 'block', summary: 'Waiting on external dependency.', tone: 'danger' },
        paused: { label: 'Paused', icon: 'pause_circle', summary: 'Deferred mid-flight; intent to resume.', tone: 'neutral' },
        someday: { label: 'Someday', icon: 'bookmark', summary: 'Parked idea — may never be worked.', tone: 'neutral' },
        done: { label: 'Done', icon: 'check_circle', summary: 'Completed.', tone: 'success' },
        cancelled: { label: 'Cancelled', icon: 'cancel', summary: 'Will not be done.', tone: 'danger' },
    };

    // Human-initiated transitions. `ready` is auto-computed, so it is not a user action.
    const WORKFLOW_ACTIONS = [
        { status: 'inbox', label: 'Inbox', icon: 'inbox' },
        { status: 'queued', label: 'Enqueue', icon: 'playlist_add_check' },
        { status: 'paused', label: 'Pause', icon: 'pause_circle' },
    ] as const;

    const TERMINAL_ACTIONS = [
        { status: 'done', label: 'Archive', icon: 'inventory_2' },
        { status: 'cancelled', label: 'Cancel', icon: 'cancel' },
    ] as const;

    let { taskId = null, onclose = () => {} }: { taskId?: string | null, onclose?: () => void } = $props();

    let task = $derived(taskId ? ($graphData?.nodes.find(n => n.id === taskId) || null) : null);

    // Check if this is a synthetic project container node (from TreemapView)
    let isProjectContainer = $derived(taskId?.startsWith('__project_') && !taskId.endsWith('_uncategorized__'));
    let projectName = $derived(isProjectContainer ? taskId?.replace(/^__project_/, '').replace(/__$/, '') : null);

    let title = $derived(projectName || (task as any)?.fullTitle || task?.label || "Unknown Task");
    let metadata = $derived((task as any)?._raw || {});

    // Non-null accessor for template use — only referenced inside {:else if task} blocks
    let t = $derived(task!);


    let description = $state("");
    let assigneeDraft = $state("");
    let loadingBody = $state(false);

    $effect(() => {
        assigneeDraft = task?.assignee || "";
    });

    // Fetch body on-demand
    $effect(() => {
        if (taskId && !taskId.startsWith('__')) {
            fetchBody(taskId);
        } else {
            description = "";
            loadingBody = false;
        }
    });

    async function fetchBody(id: string) {
        loadingBody = true;
        try {
            const res = await fetch(`/api/task?id=${encodeURIComponent(id)}`);
            if (res.ok) {
                const data = await res.json();
                description = data.body || "";
            } else {
                description = "Failed to load task description.";
            }
        } catch (e) {
            description = "Error loading task description.";
        } finally {
            loadingBody = false;
        }
    }

    let filteredMetadata = $derived(
        Object.entries(metadata)
            .filter(([key, value]) => !HIDDEN_METADATA_KEYS.has(key))
            .filter(([, value]) => value !== null && value !== undefined && value !== "")
            .filter(([, value]) => typeof value !== 'object')
            .slice(0, 10)
    );

    let currentPriority = $derived(PRIORITIES.find((priority) => priority.value === (t?.priority ?? 2)) ?? PRIORITIES[2]);

    // Downstream weight, normalised against the max in the graph using the
    // same log1p scaling that drives node size + saturation in the visuals.
    let maxWeight = $derived(
        $graphData ? Math.max(1, ...$graphData.nodes.map(n => (n as any).dw || 0)) : 1
    );
    let weightRaw = $derived((task as any)?.dw ?? 0);
    let weightNorm = $derived(
        weightRaw > 0 ? Math.min(Math.log1p(weightRaw) / Math.log1p(maxWeight), 1.0) : 0
    );
    let currentStateDetails = $derived(STATE_DETAILS[t?.status || ''] ?? {
        label: t?.status || 'Unknown',
        icon: 'help',
        summary: 'State is not mapped in the quick controls.',
        tone: 'neutral'
    });

    async function updateTask(updates: Record<string, any>, targetId: string | null = taskId) {
        if (!targetId) return;

        const { rollback } = updateGraphTaskNode(targetId, updates);

        // Persist via API — send any fields that the endpoint accepts
        const apiPayload: Record<string, unknown> = { id: targetId };
        if (updates.status) apiPayload.status = updates.status;
        if (updates.priority !== undefined) apiPayload.priority = updates.priority;
        if (updates.assignee !== undefined) apiPayload.assignee = updates.assignee;

        if (Object.keys(apiPayload).length > 1) {
            const operationId = taskOperations.start(targetId, describeTaskMutation(updates));
            try {
                const res = await fetch('/api/task/status', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify(apiPayload)
                });
                if (!res.ok) {
                    const data = await res.json().catch(() => ({}));
                    const errMsg = data.error ?? `HTTP ${res.status}`;
                    rollback();
                    taskOperations.fail(operationId, errMsg, () => updateTask(updates, targetId));
                } else {
                    taskOperations.succeed(operationId);
                }
            } catch (e: any) {
                const errMsg = e.message ?? 'Network error';
                rollback();
                taskOperations.fail(operationId, errMsg, () => updateTask(updates, targetId));
            }
        }
    }

    // Find active children of this task in the graph
    let activeChildren = $derived(
        taskId && $graphData
            ? $graphData.nodes.filter(n =>
                n.parent === taskId &&
                !COMPLETED_STATUSES.has(n.status)
            )
            : []
    );

    // Whether this task type can be completed via the COMPLETE button
    let canComplete = $derived(task ? task.type !== 'project' && task.type !== 'goal' : false);

    let showConfirmComplete = $state(false);

    function setStatus(status: string) {
        showConfirmComplete = false;
        updateTask({ status });
    }

    function handleComplete() {
        if (!task || !canComplete) return;
        // Warn if epic/task has active children
        if (activeChildren.length > 0) {
            showConfirmComplete = true;
            return;
        }
        setStatus('done');
    }

    function setPriority(p: number) {
        updateTask({ priority: p });
    }

    function priorityUp() {
        if (!task) return;
        const current = task.priority ?? 2;
        if (current > 0) {
            setPriority(current - 1);
            if (task.type === 'epic') {
                activeChildren.forEach(child => {
                    const cPri = child.priority ?? 2;
                    if (cPri > 0) updateTask({ priority: cPri - 1 }, child.id);
                });
            }
        }
    }

    function priorityDown() {
        if (!task) return;
        const current = task.priority ?? 2;
        if (current < 4) {
            setPriority(current + 1);
            if (task.type === 'epic') {
                activeChildren.forEach(child => {
                    const cPri = child.priority ?? 2;
                    if (cPri < 4) updateTask({ priority: cPri + 1 }, child.id);
                });
            }
        }
    }

    let refileMarked = $derived(Boolean((task as any)?._raw?.refile));

    async function handleMarkForRefile() {
        if (!taskId || !task) return;
        const { rollback } = updateGraphTaskNode(taskId, { refile: true });
        const operationId = taskOperations.start(taskId, describeTaskMutation({ refile: true }));
        try {
            const res = await fetch('/api/task/status', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ id: taskId, refile: true }),
            });
            if (res.ok) {
                taskOperations.succeed(operationId);
            } else {
                const data = await res.json().catch(() => ({}));
                const errMsg = data.error ?? `HTTP ${res.status}`;
                rollback();
                taskOperations.fail(operationId, errMsg, () => handleMarkForRefile());
            }
        } catch (e: any) {
            const errMsg = e.message ?? 'Network error';
            rollback();
            taskOperations.fail(operationId, errMsg, () => handleMarkForRefile());
        }
    }

    async function submitAssignee() {
        if (!taskId || !task) return;

        const normalized = assigneeDraft.trim();
        const nextAssignee = normalized || null;
        if ((task.assignee || null) === nextAssignee) return;

        await updateTask({ assignee: nextAssignee });
    }

    function handleAssigneeKeydown(event: KeyboardEvent) {
        if (event.key === 'Enter') {
            event.preventDefault();
            submitAssignee();
            (event.currentTarget as HTMLInputElement)?.blur();
        }

        if (event.key === 'Escape') {
            assigneeDraft = task?.assignee || '';
            (event.currentTarget as HTMLInputElement)?.blur();
        }
    }

    function stateCardClass(status: string, tone: string) {
        const isActive = t?.status === status;
        const toneMap: Record<string, string> = {
            neutral: isActive ? 'border-primary/55 bg-primary/10 text-primary' : 'border-primary/15 bg-black/20 text-primary/70 hover:border-primary/30 hover:text-primary',
            ready: isActive ? 'border-primary/60 bg-primary/12 text-primary' : 'border-primary/15 bg-black/20 text-primary/70 hover:border-primary/30 hover:text-primary',
            active: isActive ? 'border-primary/55 bg-primary/10 text-primary' : 'border-primary/15 bg-black/20 text-primary/70 hover:border-primary/30 hover:text-primary',
            warning: isActive ? 'border-primary/55 bg-primary/10 text-primary' : 'border-primary/15 bg-black/20 text-primary/70 hover:border-primary/30 hover:text-primary',
            danger: isActive ? 'border-destructive/40 bg-destructive/8 text-destructive/90' : 'border-primary/15 bg-black/20 text-primary/70 hover:border-destructive/35 hover:text-destructive/90',
            success: isActive ? 'border-primary/60 bg-primary/12 text-primary' : 'border-primary/15 bg-black/20 text-primary/70 hover:border-primary/30 hover:text-primary',
        };

        return `flex h-10 items-center gap-1.5 rounded-sm border px-2 py-1.5 text-left transition-colors disabled:opacity-50 ${toneMap[tone] || toneMap.neutral}`;
    }

    function stateBadgeClass(tone: string) {
        const toneMap: Record<string, string> = {
            neutral: 'border-primary/20 bg-black/25 text-primary/80',
            ready: 'border-primary/30 bg-primary/10 text-primary',
            active: 'border-primary/30 bg-primary/10 text-primary',
            warning: 'border-primary/25 bg-black/25 text-primary/80',
            danger: 'border-destructive/35 bg-destructive/8 text-destructive/90',
            success: 'border-primary/30 bg-primary/10 text-primary',
        };

        return `inline-flex items-center gap-1 rounded-sm border px-2 py-1 text-[9px] font-bold uppercase tracking-[0.16em] ${toneMap[tone] || toneMap.neutral}`;
    }

    function hexToRgba(hex: string, alpha: number) {
        const normalized = hex.replace('#', '');
        if (normalized.length !== 6) return `rgba(255, 255, 255, ${alpha})`;

        const red = Number.parseInt(normalized.slice(0, 2), 16);
        const green = Number.parseInt(normalized.slice(2, 4), 16);
        const blue = Number.parseInt(normalized.slice(4, 6), 16);
        return `rgba(${red}, ${green}, ${blue}, ${alpha})`;
    }

    function statusBadgeStyle(status: string) {
        const fill = STATUS_FILLS[status] ?? '#1f2937';
        const text = STATUS_TEXT[status] ?? '#e5e7eb';
        return `background:${fill};border-color:${hexToRgba(fill, 0.72)};color:${text};`;
    }

    function prioritySurfaceStyle(color: string, fillAlpha = 0.12, borderAlpha = 0.5) {
        return `background:${hexToRgba(color, fillAlpha)};border-color:${hexToRgba(color, borderAlpha)};color:${color};`;
    }

    function formatMetadataValue(value: unknown) {
        return typeof value === 'string' ? value : String(value);
    }

    function close() {
        onclose();
    }

    function copyToClipboard(text: string) {
        if (navigator.clipboard) {
            navigator.clipboard.writeText(text).catch(() => fallbackCopy(text));
        } else {
            fallbackCopy(text);
        }
    }

    function fallbackCopy(text: string) {
        const ta = document.createElement('textarea');
        ta.value = text;
        ta.style.position = 'fixed';
        ta.style.opacity = '0';
        document.body.appendChild(ta);
        ta.select();
        document.execCommand('copy');
        document.body.removeChild(ta);
    }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && close()} />

{#if !taskId}
    <div class="flex flex-col items-center justify-center h-full text-primary/30 p-8 text-center bg-background border-l border-primary-border">
        <span class="material-symbols-outlined text-3xl mb-2 opacity-50">check_circle</span>
        <span class="text-[10px] tracking-[0.2em] uppercase font-bold">SYSTEM READY</span>
        <span class="text-[9px] opacity-40 mt-1 uppercase">Select node for telemetry</span>
    </div>
{:else if task || isProjectContainer}
    <div class="flex flex-col h-full bg-background overflow-hidden font-mono border-l border-primary/20" data-component="task-pane">
        <!-- Breadcrumbs & Header -->
        <div class="flex flex-col gap-1 p-3 border-b border-primary/20 bg-background shrink-0">
            <div class="flex items-center justify-between">
                <div class="flex items-center gap-1.5 text-[9px] font-mono opacity-60">
                    <span class="text-[7px] italic opacity-30 mr-1">task-pane</span>
                    <span class="uppercase">{projectName || task?.project || 'VOID'}</span>
                    {#if !isProjectContainer}
                        <span class="text-primary/30">/</span>
                        <button class="text-primary hover:underline flex items-center gap-1" onclick={() => copyToClipboard(t.id)}>
                            {t.id}
                            <span class="material-symbols-outlined text-[10px]">content_copy</span>
                        </button>
                    {/if}
                </div>
                <button class="text-primary/40 hover:text-primary transition-colors" onclick={close}>
                    <span class="material-symbols-outlined text-base">close</span>
                </button>
            </div>

            <div class="flex flex-col gap-2 mt-1">
                <div class="group relative">
                    <h1 class="text-base font-black tracking-tight uppercase text-primary leading-tight pr-6">
                        {title}
                    </h1>
                    <button class="absolute top-0 right-0 text-primary/30 hover:text-primary opacity-0 group-hover:opacity-100 transition-all" onclick={() => copyToClipboard(title)} title="Copy Title">
                        <span class="material-symbols-outlined text-sm">content_copy</span>
                    </button>
                </div>

                {#if isProjectContainer}
                    <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-primary/60 text-[9px] font-mono uppercase tracking-wider mt-2">
                        <div class="flex items-center gap-1.5 bg-primary/10 px-2 py-1 rounded border border-primary/20 text-primary font-bold">
                            <span>VIRTUAL PROJECT CONTAINER</span>
                        </div>
                    </div>
                {:else}
                    <div class="flex flex-wrap items-center gap-2 text-[9px] font-mono uppercase tracking-[0.14em] text-primary/70">
                        <span class="inline-flex items-center gap-1 rounded-full border border-primary/15 bg-primary/5 px-2 py-1 text-primary/85">
                            <span class="opacity-55">Type</span>
                            <span class="font-bold">{t.type}</span>
                        </span>
                    </div>
                {/if}
                {#if showConfirmComplete}
                    <div class="mt-1 p-2 border border-destructive/40 bg-destructive/5 rounded-sm">
                        <p class="text-[9px] text-destructive font-mono mb-1.5">
                            ⚠ {activeChildren.length} active sub-task{activeChildren.length === 1 ? '' : 's'} will remain open:
                        </p>
                        <ul class="text-[8px] text-destructive/80 font-mono mb-2 space-y-0.5 max-h-16 overflow-y-auto">
                            {#each activeChildren.slice(0, 5) as child}
                                <li class="truncate">• {child.label || child.id}</li>
                            {/each}
                            {#if activeChildren.length > 5}
                                <li>… and {activeChildren.length - 5} more</li>
                            {/if}
                        </ul>
                        <div class="flex gap-2">
                            <button
                                class="flex-1 py-1 border border-destructive/40 bg-destructive/10 text-destructive font-bold text-[9px] rounded-sm uppercase hover:bg-destructive/20"
                                onclick={() => setStatus('done')}
                            >
                                COMPLETE ANYWAY
                            </button>
                            <button
                                class="flex-1 py-1 border border-primary/30 text-primary/60 font-bold text-[9px] rounded-sm uppercase hover:bg-primary/10"
                                onclick={() => showConfirmComplete = false}
                            >
                                CANCEL
                            </button>
                        </div>
                    </div>
                {/if}
            </div>
        </div>

        <!-- Scrollable content -->
        <div class="flex-1 overflow-y-auto custom-scrollbar">
            <div class="flex flex-col p-3 space-y-4">
                {#if isProjectContainer}
                    <div class="space-y-4">
                        <div class="p-4 border border-primary/20 bg-primary/5 rounded-sm">
                            <span class="text-[9px] font-bold uppercase tracking-widest text-primary/50 block mb-2">Project_Overview</span>
                            <div class="text-[11px] leading-relaxed text-primary/80">
                                This is a synthetic container for all tasks within the <strong>{projectName}</strong> project.
                                Click a specific task inside the treemap to edit its details.
                            </div>
                        </div>

                        <div class="space-y-2">
                            <span class="text-[9px] font-bold uppercase tracking-widest text-primary/50 block border-b border-primary/10 pb-1">Local_Context</span>
                            <div class="bg-primary/2 rounded p-1">
                                <HierarchyTree {taskId} />
                            </div>
                        </div>
                    </div>
                {:else if task}
                    <div class="space-y-4">
                        <section class="rounded-sm border border-primary/15 bg-black/15 p-3">
                            <div class="flex items-center justify-between gap-2">
                                <div class="text-[9px] font-bold uppercase tracking-[0.18em] text-primary/45">State</div>
                                <span class={stateBadgeClass(currentStateDetails.tone)} style={statusBadgeStyle(t.status)}>
                                    <span class="material-symbols-outlined text-[12px]">{currentStateDetails.icon}</span>
                                    {currentStateDetails.label}
                                </span>
                            </div>
                            <div class="mt-2 grid grid-cols-2 gap-1.5">
                                {#if canComplete}
                                    <button
                                        class={stateCardClass('done', 'success')}
                                        onclick={handleComplete}
                                        title="Mark complete"
                                    >
                                        <span class="material-symbols-outlined text-[14px]">check_circle</span>
                                        <span class="min-w-0">
                                            <span class="block text-[8px] font-black uppercase tracking-[0.06em]">Done</span>
                                        </span>
                                    </button>
                                {/if}
                                {#each WORKFLOW_ACTIONS as action}
                                    <button
                                        class={stateCardClass(action.status, STATE_DETAILS[action.status]?.tone ?? 'neutral')}
                                        onclick={() => setStatus(action.status)}
                                    >
                                        <span class="material-symbols-outlined text-[14px]">{action.icon}</span>
                                        <span class="min-w-0">
                                            <span class="block text-[8px] font-black uppercase tracking-[0.06em]">{action.label}</span>
                                        </span>
                                    </button>
                                {/each}
                                {#each TERMINAL_ACTIONS as action}
                                    <button
                                        class={stateCardClass(action.status, STATE_DETAILS[action.status]?.tone ?? 'neutral')}
                                        onclick={() => setStatus(action.status)}
                                    >
                                        <span class="material-symbols-outlined text-[14px]">{action.icon}</span>
                                        <span class="min-w-0">
                                            <span class="block text-[8px] font-black uppercase tracking-[0.06em]">{action.label}</span>
                                        </span>
                                    </button>
                                {/each}
                            </div>
                        </section>

                        <section class="grid grid-cols-1 gap-3">
                            <div class="rounded-sm border border-primary/15 bg-black/15 p-3">
                                <div class="flex items-center justify-between">
                                    <span class="text-[9px] font-bold uppercase tracking-[0.18em] text-primary/45">Priority</span>
                                    {#if task.type === 'epic'}
                                        <span class="text-[8px] font-mono uppercase tracking-[0.14em] text-primary/35">Cascades to active children</span>
                                    {/if}
                                </div>
                                <div class="mt-2 grid grid-cols-[2rem_minmax(0,1fr)_2rem] items-center gap-2">
                                    <button
                                        class="inline-flex h-8 w-8 items-center justify-center rounded-sm border transition-colors hover:opacity-100 disabled:opacity-40"
                                        style={prioritySurfaceStyle(currentPriority.color, 0.08, 0.35)}
                                        onclick={priorityUp}
                                        disabled={(t.priority ?? 2) <= 0}
                                        title="Increase priority"
                                    >
                                        <span class="material-symbols-outlined text-[14px]">arrow_upward</span>
                                    </button>
                                    <div class="min-w-0 rounded-sm border px-2 py-1.5 text-center" style={prioritySurfaceStyle(currentPriority.color, 0.12, 0.45)}>
                                        <div class="text-[8px] uppercase tracking-[0.14em] text-primary/35">Current</div>
                                        <div class="mt-0.5 text-[12px] font-black leading-tight" style={`color:${currentPriority.color}`}>
                                            P{currentPriority.value} {currentPriority.short}
                                        </div>
                                    </div>
                                    <button
                                        class="inline-flex h-8 w-8 items-center justify-center rounded-sm border transition-colors hover:opacity-100 disabled:opacity-40"
                                        style={prioritySurfaceStyle(currentPriority.color, 0.08, 0.35)}
                                        onclick={priorityDown}
                                        disabled={(t.priority ?? 2) >= 4}
                                        title="Decrease priority"
                                    >
                                        <span class="material-symbols-outlined text-[14px]">arrow_downward</span>
                                    </button>
                                </div>
                            </div>

                            <div class="rounded-sm border border-primary/15 bg-black/15 p-3">
                                <div class="flex items-center justify-between gap-3">
                                    <span class="text-[9px] font-bold uppercase tracking-[0.18em] text-primary/45">Ownership</span>
                                    <button
                                        class="inline-flex shrink-0 items-center gap-1 rounded-sm border border-primary/20 bg-black/20 px-2 py-1 text-[8px] font-bold uppercase tracking-[0.14em] text-primary/75 transition-colors hover:border-primary/35 hover:text-primary disabled:opacity-50"
                                        onclick={handleMarkForRefile}
                                        disabled={refileMarked}
                                        title="Mark this task for refiling or reorganization"
                                    >
                                        <span class="material-symbols-outlined text-[12px]">drive_file_move</span>
                                        {refileMarked ? 'Marked' : 'Refile'}
                                    </button>
                                </div>
                                <label class="mt-3 block">
                                    <span class="mb-1 block text-[9px] uppercase tracking-[0.16em] text-primary/40">Assignee</span>
                                    <input
                                        class="w-full rounded-sm border border-primary/20 bg-black/20 px-3 py-2 text-[11px] text-primary outline-none transition-colors placeholder:text-primary/20 focus:border-primary/45"
                                        placeholder="Unassigned"
                                        type="text"
                                        bind:value={assigneeDraft}
                                        onblur={submitAssignee}
                                        onkeydown={handleAssigneeKeydown}
                                    />
                                </label>
                            </div>
                        </section>

                        {#if t.criticality > 0 || t.uncertainty > 0 || t.scope > 0 || weightRaw > 0 || t.focusScore > 0}
                        <section class="rounded-sm border border-primary/15 bg-black/15 p-3 space-y-2">
                            <div class="text-[9px] font-bold uppercase tracking-[0.18em] text-primary/45 border-b border-primary/10 pb-1">Computed Properties</div>

                            <!-- Weight (downstream_weight, log-normalised against graph max) -->
                            {#if weightRaw > 0}
                            <div class="space-y-0.5" title="Downstream weight: log1p(dw)/log1p(max). Drives node size + fill saturation in graph views.">
                                <div class="flex items-center justify-between text-[8px] font-mono uppercase tracking-[0.12em]">
                                    <span class="text-primary/45">Weight</span>
                                    <span class="font-bold" style="color: {weightNorm > 0.6 ? '#42d4f4' : weightNorm > 0.3 ? '#3aa9c4' : '#a3a3a3'}">{weightRaw.toFixed(1)} <span class="text-primary/40">({Math.round(weightNorm * 100)}%)</span></span>
                                </div>
                                <div class="h-1.5 w-full rounded-full bg-primary/10 overflow-hidden">
                                    <div class="h-full rounded-full transition-all" style="width:{Math.round(weightNorm * 100)}%; background: color-mix(in srgb, #42d4f4 {40 + Math.round(weightNorm * 60)}%, #374151)"></div>
                                </div>
                            </div>
                            {/if}

                            <!-- Criticality -->
                            <div class="space-y-0.5">
                                <div class="flex items-center justify-between text-[8px] font-mono uppercase tracking-[0.12em]">
                                    <span class="text-primary/45">Criticality</span>
                                    <span class="font-bold" style="color: {t.criticality > 0.6 ? '#f59e0b' : t.criticality > 0.3 ? '#d97706' : '#a3a3a3'}">{Math.round(t.criticality * 100)}%</span>
                                </div>
                                <div class="h-1.5 w-full rounded-full bg-primary/10 overflow-hidden">
                                    <div class="h-full rounded-full transition-all" style="width:{Math.round(t.criticality * 100)}%; background: color-mix(in srgb, #f59e0b {40 + Math.round(t.criticality * 60)}%, #374151)"></div>
                                </div>
                            </div>

                            <!-- Uncertainty -->
                            <div class="space-y-0.5">
                                <div class="flex items-center justify-between text-[8px] font-mono uppercase tracking-[0.12em]">
                                    <span class="text-primary/45">Uncertainty</span>
                                    <span class="font-bold" style="color: {t.uncertainty > 0.6 ? '#94a3b8' : t.uncertainty > 0.3 ? '#64748b' : '#a3a3a3'}">{Math.round(t.uncertainty * 100)}%</span>
                                </div>
                                <div class="h-1.5 w-full rounded-full bg-primary/10 overflow-hidden">
                                    <div class="h-full rounded-full transition-all" style="width:{Math.round(t.uncertainty * 100)}%; background: color-mix(in srgb, #94a3b8 {40 + Math.round(t.uncertainty * 60)}%, #374151)"></div>
                                </div>
                            </div>

                            <!-- Scope -->
                            {#if t.scope > 0}
                            <div class="flex items-center justify-between text-[8px] font-mono uppercase tracking-[0.12em]">
                                <span class="text-primary/45">Scope</span>
                                <span class="font-bold text-primary/70">{t.scope} descendant{t.scope === 1 ? '' : 's'}</span>
                            </div>
                            {/if}

                            <!-- Focus Score -->
                            {#if t.focusScore > 0}
                            <div class="flex items-center justify-between text-[8px] font-mono uppercase tracking-[0.12em]">
                                <span class="text-primary/45">Focus Score</span>
                                <span class="font-bold text-[#a78bfa]">{t.focusScore}</span>
                            </div>
                            {/if}
                        </section>
                        {/if}

                        <section class="rounded-sm border border-primary/15 bg-black/15">
                            <div class="flex items-center justify-between border-b border-primary/10 px-3 py-2">
                                <div class="text-[9px] font-bold uppercase tracking-[0.18em] text-primary/45">Description</div>
                                {#if description.trim()}
                                    <button class="text-primary/35 hover:text-primary transition-colors flex items-center gap-1 text-[9px] uppercase tracking-[0.14em]" onclick={() => copyToClipboard(description)}>
                                        Copy
                                        <span class="material-symbols-outlined text-[10px]">content_copy</span>
                                    </button>
                                {/if}
                            </div>
                            <div class="max-h-[24rem] overflow-y-auto px-4 py-3 custom-scrollbar">
                                {#if loadingBody}
                                    <div class="text-[11px] text-primary/40">Syncing description…</div>
                                {:else if description.trim()}
                                    <div class="select-text whitespace-pre-wrap text-[12px] leading-6 text-primary/88">{description}</div>
                                {:else}
                                    <div class="text-[11px] italic text-primary/30">No task description is available.</div>
                                {/if}
                            </div>
                        </section>

                        <section class="space-y-2">
                            <span class="text-[9px] font-bold uppercase tracking-widest text-primary/50 block border-b border-primary/10 pb-1">Local Context</span>
                            <div class="rounded-sm border border-primary/12 bg-black/15 p-2">
                                <HierarchyTree {taskId} />
                            </div>
                        </section>

                        <section class="space-y-2">
                            <span class="text-[9px] font-bold uppercase tracking-widest text-primary/50 block border-b border-primary/10 pb-1">Additional Telemetry</span>
                            <div class="grid grid-cols-1 gap-1.5">
                                {#each filteredMetadata as [key, value]}
                                    <div class="flex items-baseline justify-between gap-3 rounded-sm border border-primary/10 bg-black/15 px-2 py-1.5 font-mono">
                                        <div class="min-w-0 truncate text-[8px] font-bold uppercase tracking-[0.12em] text-primary/35">{key}</div>
                                        <div class="max-w-[58%] truncate text-[8px] uppercase tracking-[0.04em] text-primary/55" title={formatMetadataValue(value)}>{formatMetadataValue(value)}</div>
                                    </div>
                                {:else}
                                    <span class="text-[9px] text-primary/20 italic">No additional telemetry worth showing.</span>
                                {/each}
                            </div>
                        </section>
                    </div>
                {/if}
            </div>
        </div>
    </div>
{:else}
    <div class="flex flex-col items-center justify-center h-full text-primary/30 p-8 text-center bg-background border-l border-primary-border">
        <span class="material-symbols-outlined text-3xl mb-2 text-destructive opacity-50">warning</span>
        <span class="text-[10px] tracking-widest uppercase font-bold text-destructive/80">CORE_SYNC_FAILED</span>
        <button class="mt-4 px-3 py-1.5 border border-primary/20 text-[9px] hover:text-primary hover:border-primary transition-colors uppercase tracking-widest" onclick={close}>REBOOT_VIEW</button>
    </div>
{/if}
