<script lang="ts">
    import { onMount } from 'svelte';
    import { copyToClipboard, formatText } from "../../data/utils";
    import { toggleSessionSelection } from "../../stores/selection";
    import { projectColor, projectBgTint, projectBorderColor } from "../../data/projectUtils";

    type InsightRow = {
        session_id: string;
        date: string;
        started_at: string;
        description: string;
        project: string;
        client?: string;
        surface?: string;
        task_id: string;
        pr: string;
        provider: string;
        subagent: string;
        duration_minutes: number;
        input_tokens: number;
        output_tokens: number;
        cache_create_tokens: number;
        cache_read_tokens: number;
    };

    let rawData: InsightRow[] = [];
    let loading = true;
    let error = "";

    let groupBy: keyof InsightRow = 'session_id';
    let sortField = 'started_at';
    let sortDesc = true;

    const GROUP_OPTIONS = [
        { value: 'session_id', label: 'Session' },
        { value: 'date', label: 'Date' },
        { value: 'project', label: 'Project' },
        { value: 'task_id', label: 'Task ID' },
        { value: 'pr', label: 'PR' },
        { value: 'provider', label: 'Provider' },
        { value: 'subagent', label: 'Subagent' }
    ] as const;

    onMount(async () => {
        try {
            const res = await fetch('/api/insights');
            if (!res.ok) throw new Error('Failed to load insights');
            const json = await res.json();
            if (json.error) throw new Error(json.error);
            rawData = json.insights || [];
        } catch (e: any) {
            error = e.message;
        } finally {
            loading = false;
        }
    });

    $: aggregatedData = (() => {
        if (!rawData.length) return [];
        
        const map = new Map<string, any>();
        
        for (const row of rawData) {
            const key = String(row[groupBy] || 'unknown');
            if (!map.has(key)) {
                map.set(key, {
                    groupKey: key,
                    date: row.date || '',
                    started_at: row.started_at || '',
                    description: row.description || '',
                    project: row.project,
                    client: row.client,
                    surface: row.surface,
                    sessions: new Set(),
                    total_duration: 0,
                    total_input: 0,
                    total_output: 0,
                    total_cache_create: 0,
                    total_cache_read: 0
                });
            }
            const agg = map.get(key);
            if (row.started_at && row.started_at > agg.started_at) {
                agg.started_at = row.started_at;
            }
            if (row.date && row.date > agg.date) {
                agg.date = row.date;
            }
            agg.sessions.add(row.session_id);
            agg.total_duration += row.duration_minutes || 0;
            agg.total_input += row.input_tokens || 0;
            agg.total_output += row.output_tokens || 0;
            agg.total_cache_create += row.cache_create_tokens || 0;
            agg.total_cache_read += row.cache_read_tokens || 0;
        }

        let result = Array.from(map.values()).map(a => ({
            ...a,
            session_count: a.sessions.size
        }));

        result.sort((a, b) => {
            const valA = a[sortField];
            const valB = b[sortField];
            if (valA < valB) return sortDesc ? 1 : -1;
            if (valA > valB) return sortDesc ? -1 : 1;
            return 0;
        });

        return result;
    })();

    function toggleSort(field: string) {
        if (sortField === field) {
            sortDesc = !sortDesc;
        } else {
            sortField = field;
            sortDesc = true;
        }
    }

    function formatNumber(num: number) {
        return new Intl.NumberFormat().format(Math.round(num));
    }
</script>

<div class="flex flex-col h-full w-full bg-surface text-primary p-6 overflow-hidden">
    <div class="flex justify-between items-center mb-6">
        <h2 class="text-xl font-bold uppercase tracking-widest text-glow">Token Quota Insights</h2>
        <div class="flex items-center gap-3">
            <span class="text-[10px] uppercase tracking-widest opacity-60">Group By:</span>
            <select 
                bind:value={groupBy}
                class="bg-background border border-primary/30 rounded px-3 py-1.5 text-xs focus:border-primary focus:outline-none transition-colors uppercase tracking-wider font-bold"
            >
                {#each GROUP_OPTIONS as opt}
                    <option value={opt.value}>{opt.label}</option>
                {/each}
            </select>
        </div>
    </div>

    {#if loading}
        <div class="flex-1 flex items-center justify-center">
            <div class="animate-pulse opacity-60 uppercase tracking-widest text-sm">Loading Insights Data...</div>
        </div>
    {:else if error}
        <div class="flex-1 flex items-center justify-center text-destructive">
            {error}
        </div>
    {:else if aggregatedData.length === 0}
        <div class="flex-1 flex items-center justify-center opacity-60 uppercase tracking-widest text-sm">
            No session data found.
        </div>
    {:else}
        <div class="flex-1 overflow-auto border border-primary/20 rounded-lg custom-scrollbar bg-background/50">
            <table class="w-full text-left border-collapse">
                <thead class="sticky top-0 bg-background/95 backdrop-blur z-10 text-[10px] uppercase tracking-wider text-primary/70">
                    <tr>
                        <th class="p-3 border-b border-primary/20 cursor-pointer hover:text-primary transition-colors" onclick={() => toggleSort('groupKey')}>
                            <div class="flex items-center gap-1">
                                {GROUP_OPTIONS.find(o => o.value === groupBy)?.label}
                                {#if sortField === 'groupKey'}<span class="text-[8px]">{sortDesc ? '▼' : '▲'}</span>{/if}
                            </div>
                        </th>
                        {#if groupBy !== 'date'}
                        <th class="p-3 border-b border-primary/20 cursor-pointer hover:text-primary transition-colors text-right" onclick={() => toggleSort('started_at')}>
                            <div class="flex items-center justify-end gap-1">
                                Date
                                {#if sortField === 'started_at'}<span class="text-[8px]">{sortDesc ? '▼' : '▲'}</span>{/if}
                            </div>
                        </th>
                        {/if}
                        <th class="p-3 border-b border-primary/20 cursor-pointer hover:text-primary transition-colors text-right" onclick={() => toggleSort('session_count')}>
                            <div class="flex items-center justify-end gap-1">
                                Sessions
                                {#if sortField === 'session_count'}<span class="text-[8px]">{sortDesc ? '▼' : '▲'}</span>{/if}
                            </div>
                        </th>
                        <th class="p-3 border-b border-primary/20 cursor-pointer hover:text-primary transition-colors text-right" onclick={() => toggleSort('total_duration')}>
                            <div class="flex items-center justify-end gap-1">
                                Time (min)
                                {#if sortField === 'total_duration'}<span class="text-[8px]">{sortDesc ? '▼' : '▲'}</span>{/if}
                            </div>
                        </th>
                        <th class="p-3 border-b border-primary/20 cursor-pointer hover:text-primary transition-colors text-right" onclick={() => toggleSort('total_input')}>
                            <div class="flex items-center justify-end gap-1">
                                Input Tokens
                                {#if sortField === 'total_input'}<span class="text-[8px]">{sortDesc ? '▼' : '▲'}</span>{/if}
                            </div>
                        </th>
                        <th class="p-3 border-b border-primary/20 cursor-pointer hover:text-primary transition-colors text-right" onclick={() => toggleSort('total_output')}>
                            <div class="flex items-center justify-end gap-1">
                                Output Tokens
                                {#if sortField === 'total_output'}<span class="text-[8px]">{sortDesc ? '▼' : '▲'}</span>{/if}
                            </div>
                        </th>
                        <th class="p-3 border-b border-primary/20 cursor-pointer hover:text-primary transition-colors text-right" onclick={() => toggleSort('total_cache_create')}>
                            <div class="flex items-center justify-end gap-1">
                                Cache Create
                                {#if sortField === 'total_cache_create'}<span class="text-[8px]">{sortDesc ? '▼' : '▲'}</span>{/if}
                            </div>
                        </th>
                        <th class="p-3 border-b border-primary/20 cursor-pointer hover:text-primary transition-colors text-right" onclick={() => toggleSort('total_cache_read')}>
                            <div class="flex items-center justify-end gap-1">
                                Cache Read
                                {#if sortField === 'total_cache_read'}<span class="text-[8px]">{sortDesc ? '▼' : '▲'}</span>{/if}
                            </div>
                        </th>
                    </tr>
                </thead>
                <tbody class="text-xs">
                    {#each aggregatedData as row}
                        <tr class="hover:bg-primary/5 transition-colors border-b border-primary/5 last:border-0 group">
                            <td class="p-3">
                                {#if groupBy === 'session_id'}
                                    <div class="flex items-center gap-2 shrink-0">
                                        <div class="flex items-center gap-1">
                                            <button class="text-[9px] font-bold bg-primary/20 text-primary/60 px-1 py-0.5 hover:bg-primary/40 transition-colors rounded-sm" 
                                                    onclick={(e) => { e.stopPropagation(); copyToClipboard(row.groupKey); }}
                                                    title="Click to copy session ID: {row.groupKey}">
                                                {(row.groupKey || "").slice(-8)}
                                            </button>
                                            <button class="text-[9px] font-bold bg-primary/20 text-primary/60 px-1 py-0.5 hover:bg-primary/40 transition-colors rounded-sm" 
                                                    onclick={(e) => { e.stopPropagation(); toggleSessionSelection(row.groupKey); }}
                                                    title="View detailed session metadata">
                                                <span class="material-symbols-outlined text-[12px] leading-none">info</span>
                                            </button>
                                        </div>
                                        <div class="flex items-center gap-1">
                                            {#if row.client && row.client !== 'unknown'}
                                                <span class="text-[9px] font-bold bg-blue-900/30 text-blue-400 border border-blue-500/30 px-1.5 py-0.5 rounded-sm uppercase tracking-wider">{row.client}</span>
                                            {/if}
                                            {#if row.surface && row.surface !== 'unknown'}
                                                <span class="text-[9px] font-bold bg-purple-900/30 text-purple-400 border border-purple-500/30 px-1.5 py-0.5 rounded-sm uppercase tracking-wider">{row.surface}</span>
                                            {/if}
                                            {#if row.project && row.project !== 'unknown'}
                                                <span class="text-[9px] font-bold px-1.5 py-0.5 rounded-sm uppercase tracking-wider"
                                                      style="background: {projectBgTint(row.project)}; color: {projectColor(row.project)}; border: 1px solid {projectBorderColor(row.project)};">{row.project}</span>
                                            {/if}
                                        </div>
                                    </div>
                                    {#if row.description}
                                        <div class="mt-1.5 text-xs text-primary/70 line-clamp-2 pr-4 leading-relaxed font-sans whitespace-normal break-words">
                                            {@html formatText(row.description)}
                                        </div>
                                    {/if}
                                {:else}
                                    <span class="font-mono text-primary/90">{row.groupKey}</span>
                                {/if}
                            </td>
                            {#if groupBy !== 'date'}
                            <td class="p-3 text-right opacity-70 group-hover:opacity-100 transition-opacity whitespace-nowrap">
                                {row.started_at ? row.started_at.replace('T', ' ').slice(0, 16) : row.date}
                            </td>
                            {/if}
                            <td class="p-3 text-right opacity-70 group-hover:opacity-100 transition-opacity">{row.session_count}</td>
                            <td class="p-3 text-right opacity-70 group-hover:opacity-100 transition-opacity">{formatNumber(row.total_duration)}</td>
                            <td class="p-3 text-right text-blue-400/70 group-hover:text-blue-400/100 transition-colors">{formatNumber(row.total_input)}</td>
                            <td class="p-3 text-right text-orange-400/70 group-hover:text-orange-400/100 transition-colors font-bold">{formatNumber(row.total_output)}</td>
                            <td class="p-3 text-right text-green-400/70 group-hover:text-green-400/100 transition-colors">{formatNumber(row.total_cache_create)}</td>
                            <td class="p-3 text-right text-purple-400/70 group-hover:text-purple-400/100 transition-colors">{formatNumber(row.total_cache_read)}</td>
                        </tr>
                    {/each}
                </tbody>
            </table>
        </div>
    {/if}
</div>
