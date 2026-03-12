<script lang="ts">
    import { graphData } from "../../stores/graph";
    import HierarchyTree from "./HierarchyTree.svelte";
    import {
        TYPE_CHARGE,
        STATUS_FILLS
    } from "../../data/constants";

    let { taskId = null, onclose = () => {} }: { taskId?: string | null, onclose?: () => void } = $props();

    let task = $derived(taskId ? ($graphData?.nodes.find(n => n.id === taskId) || null) : null);
    let title = $derived((task as any)?.fullTitle || task?.label || "Unknown Task");
    let metadata = $derived((task as any)?._raw || {});

    let description = $state("");
    let loadingBody = $state(false);

    // Fetch body on-demand
    $effect(() => {
        if (task?.path) {
            fetchBody(task.path);
        } else {
            description = "";
        }
    });

    async function fetchBody(path: string) {
        loadingBody = true;
        try {
            const res = await fetch(`/api/task?path=${encodeURIComponent(path)}`);
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

    // Filter out internal fields from metadata display
    let filteredMetadata = $derived(Object.entries(metadata).filter(([key]) =>
        !['body', 'id', 'title', 'label', 'node_type', 'status', 'priority', 'project', 'assignee', 'layouts', 'x', 'y', 'depth', 'maxDepth', 'lines', 'dw', 'downstream_weight', 'modified', 'created', 'isLeaf', 'parent', 'fullTitle'].includes(key)
    ));

    const statusOptions = Object.keys(STATUS_FILLS).sort();
    const typeOptions = Object.keys(TYPE_CHARGE).sort();

    async function updateTask(updates: Record<string, any>) {
        if (!taskId || !task) return;

        // Optimistic local update
        graphData.update(gd => {
            if (!gd) return gd;
            const nodes = gd.nodes.map(n => {
                if (n.id === taskId) {
                    const updated = { ...n, ...updates };
                    // If status changed, update fill and text colors (simplified)
                    if (updates.status) {
                        updated.status = updates.status;
                    }
                    if (updates.type) {
                        updated.type = updates.type;
                    }
                    return updated;
                }
                return n;
            });
            return { ...gd, nodes };
        });

        console.log(`[AGENT ACTION REQUIRED] Update task ${taskId} with:`, updates);
        // In a real app, this would be an API call.
        // As an agent, I will perform the mcp__pkb__update_task call after this file edit.
    }

    function setStatus(status: string) {
        updateTask({ status });
    }

    function setType(type: string) {
        updateTask({ type });
    }

    function close() {
        onclose();
    }

    function copyToClipboard(text: string) {
        if (!navigator.clipboard) return;
        navigator.clipboard.writeText(text);
        // Maybe a temporary toast or visual feedback
    }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && close()} />

{#if !taskId}
    <div class="flex flex-col items-center justify-center h-full text-primary/30 p-8 text-center bg-background border-l border-primary-border">
        <span class="material-symbols-outlined text-3xl mb-2 opacity-50">check_circle</span>
        <span class="text-[10px] tracking-[0.2em] uppercase font-bold">SYSTEM READY</span>
        <span class="text-[9px] opacity-40 mt-1 uppercase">Select node for telemetry</span>
    </div>
{:else if task}
    <div class="flex flex-col h-full bg-background overflow-hidden font-mono border-l border-primary/20">
        <!-- Breadcrumbs & Header -->
        <div class="flex flex-col gap-1 p-3 border-b border-primary/20 bg-background shrink-0">
            <div class="flex items-center justify-between">
                <div class="flex items-center gap-1.5 text-[9px] font-mono opacity-60">
                    <span class="uppercase">{task.project || 'VOID'}</span>
                    <span class="text-primary/30">/</span>
                    <button class="text-primary hover:underline flex items-center gap-1" onclick={() => copyToClipboard(task.id)}>
                        {task.id}
                        <span class="material-symbols-outlined text-[10px]">content_copy</span>
                    </button>
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
                
                <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-primary/60 text-[9px] font-mono uppercase tracking-wider">
                    <div class="flex items-center gap-1.5 bg-primary/5 px-1.5 py-0.5 rounded border border-primary/10">
                        <span>TYPE:</span>
                        <select
                            class="bg-transparent text-primary outline-none cursor-pointer"
                            value={task.type}
                            onchange={(e) => setType(e.currentTarget.value)}
                        >
                            {#each typeOptions as type}
                                <option value={type}>{type}</option>
                            {/each}
                        </select>
                    </div>
                    <div class="flex items-center gap-1.5 bg-primary/5 px-1.5 py-0.5 rounded border border-primary/10">
                        <span>STATE:</span>
                        <select
                            class="bg-transparent text-primary outline-none cursor-pointer"
                            value={task.status}
                            onchange={(e) => setStatus(e.currentTarget.value)}
                        >
                            {#each statusOptions as status}
                                <option value={status}>{status}</option>
                            {/each}
                        </select>
                    </div>
                </div>

                <div class="flex gap-2 mt-1">
                    <button
                        class="flex-1 py-1.5 border border-primary {task.status === 'done' ? 'bg-primary text-background' : 'bg-primary/5 text-primary'} hover:bg-primary hover:text-background font-bold text-[10px] transition-all rounded-sm uppercase tracking-widest"
                        onclick={() => setStatus('done')}
                    >
                        {task.status === 'done' ? 'FINISHED' : 'COMPLETE'}
                    </button>
                    <button
                        class="px-2 py-1.5 border border-primary/40 {task.status === 'ready' ? 'bg-primary/20 border-primary text-primary' : 'text-primary/60'} hover:border-primary hover:text-primary font-bold text-[10px] transition-all rounded-sm"
                        onclick={() => setStatus('ready')}
                    >
                        READY
                    </button>
                    <button
                        class="px-2 py-1.5 border border-destructive/30 text-destructive/60 hover:bg-destructive/10 hover:text-destructive font-bold text-[10px] transition-all rounded-sm" title="Delete Task"
                    >
                        <span class="material-symbols-outlined text-xs">delete</span>
                    </button>
                </div>
            </div>
        </div>

        <!-- Scrollable content -->
        <div class="flex-1 overflow-y-auto custom-scrollbar">
            <div class="flex flex-col p-3 space-y-4">
                <!-- Main Editor Area -->
                <div class="space-y-1.5">
                    <div class="flex items-center justify-between">
                        <span class="text-[9px] font-bold uppercase tracking-[0.15em] text-primary/50">Core_Intelligence</span>
                        <button class="text-primary/30 hover:text-primary transition-colors flex items-center gap-1 text-[9px]" onclick={() => copyToClipboard(description)}>
                            COPY
                            <span class="material-symbols-outlined text-[10px]">content_copy</span>
                        </button>
                    </div>
                    <div class="border border-primary/20 bg-black/20 p-3 min-h-[160px] relative">
                        <textarea class="w-full h-full min-h-[140px] bg-transparent border-none focus:ring-0 text-[11px] font-mono leading-relaxed text-primary/80 resize-none outline-none custom-scrollbar" placeholder={loadingBody ? "Syncing..." : "No data found."} value={description}></textarea>
                    </div>
                </div>

                <div class="grid grid-cols-2 gap-3">
                    <div class="space-y-1">
                        <span class="text-[9px] font-bold uppercase tracking-widest text-primary/50">Priority</span>
                        <select class="w-full bg-primary/5 border border-primary/20 rounded p-1.5 text-[10px] text-primary focus:border-primary/50 outline-none">
                            <option value="0" selected={task.priority === 0}>P0 CRITICAL</option>
                            <option value="1" selected={task.priority === 1}>P1 HIGH</option>
                            <option value="2" selected={task.priority === 2}>P2 MEDIUM</option>
                            <option value="3" selected={task.priority === 3}>P3 LOW</option>
                            <option value="4" selected={task.priority === 4}>P4 BACKLOG</option>
                        </select>
                    </div>
                    <div class="space-y-1">
                        <span class="text-[9px] font-bold uppercase tracking-widest text-primary/50">Assignee</span>
                        <input class="w-full bg-primary/5 border border-primary/20 rounded p-1.5 text-[10px] text-primary focus:border-primary/50 outline-none placeholder:text-primary/20" placeholder="NONE" type="text" value={task.assignee || ""}/>
                    </div>
                </div>

                <div class="space-y-2">
                    <span class="text-[9px] font-bold uppercase tracking-widest text-primary/50 block border-b border-primary/10 pb-1">Lineage_Map</span>
                    <div class="bg-primary/2 rounded p-1">
                        <HierarchyTree {taskId} />
                    </div>
                </div>

                <div class="space-y-2">
                    <span class="text-[9px] font-bold uppercase tracking-widest text-primary/50 block border-b border-primary/10 pb-1">Dependencies</span>
                    <div class="space-y-1 max-h-32 overflow-y-auto pr-1 custom-scrollbar">
                        {#each ($graphData?.links || []).filter(l => (typeof l.source === 'object' ? l.source.id : l.source) === task.id && l.type === 'depends_on') as dep}
                            <div class="flex items-center justify-between px-2 py-1.5 border border-primary/10 bg-primary/5 rounded-sm">
                                <span class="text-[9px] font-mono text-primary/70 truncate">{typeof dep.target === 'object' ? dep.target.id : String(dep.target)}</span>
                                <span class="material-symbols-outlined text-[10px] text-primary/30 hover:text-primary cursor-pointer">close</span>
                            </div>
                        {:else}
                            <div class="text-[9px] text-primary/30 italic px-1">No active blockers.</div>
                        {/each}
                    </div>
                </div>

                <!-- Metadata List -->
                <div class="space-y-2">
                    <span class="text-[9px] font-bold uppercase tracking-widest text-primary/50 block border-b border-primary/10 pb-1">Extended_Telemetry</span>
                    <div class="grid grid-cols-1 gap-y-2">
                        {#each filteredMetadata as [key, value]}
                            <div class="flex justify-between items-start gap-2 border-b border-primary/5 pb-1">
                                <span class="text-[8px] uppercase text-primary/40 font-bold shrink-0">{key}</span>
                                <span class="text-[10px] text-primary/70 text-right break-all max-w-[140px]" title={String(value)}>{value}</span>
                            </div>
                        {:else}
                            <span class="text-[9px] text-primary/20 italic">No telemetry data.</span>
                        {/each}
                    </div>
                </div>
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
