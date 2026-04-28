<script lang="ts">
    import { graphData } from '../../stores/graph';
    import { selection, toggleSelection } from '../../stores/selection';
    import { projectColor } from '../../data/projectUtils';
    import {
        extractMultiTargetSubgraph,
        multiAsExtracted,
        findMultiClusters,
        pickAllTargets,
        computeDependencyDepth,
        clusterLabel,
        isCompleted,
    } from '../../data/subgraphExtraction';
    import type { GraphNode } from '../../data/prepareGraphData';

    $: targets = pickAllTargets($graphData);
    $: focusOverride = $selection.focusNodeId;
    $: targetIds = focusOverride ? [focusOverride] : targets.map(t => t.id);
    $: multi = $graphData && targetIds.length > 0
        ? extractMultiTargetSubgraph($graphData, targetIds)
        : null;
    $: extracted = multi ? multiAsExtracted(multi) : null;
    $: depth = extracted ? computeDependencyDepth(extracted) : new Map<string, number>();
    $: clusters = multi ? findMultiClusters(multi) : [];
    $: clusterIdx = (() => {
        const m = new Map<string, number>();
        clusters.forEach((c, i) => c.forEach(n => m.set(n.id, i)));
        return m;
    })();
    $: targetSet = new Set(multi?.targets.map(t => t.id) ?? []);

    interface Wave {
        depth: number;
        groups: { clusterIndex: number; label: string; tasks: GraphNode[] }[];
    }

    function buildWaves(
        nodes: GraphNode[],
        depth: Map<string, number>,
        targetSet: Set<string>,
    ): Wave[] {
        const byDepth = new Map<number, GraphNode[]>();
        let maxNonTargetDepth = 0;
        for (const n of nodes) {
            if (targetSet.has(n.id)) continue;
            const d = depth.get(n.id) ?? 0;
            if (d > maxNonTargetDepth) maxNonTargetDepth = d;
            const arr = byDepth.get(d) || [];
            arr.push(n);
            byDepth.set(d, arr);
        }
        const finishCol = maxNonTargetDepth + 1;
        const targetNodes = nodes.filter(n => targetSet.has(n.id));
        if (targetNodes.length > 0) byDepth.set(finishCol, targetNodes);

        const sortedDepths = [...byDepth.keys()].sort((a, b) => a - b);
        return sortedDepths.map(d => {
            const tasks = byDepth.get(d)!;
            const grouped = new Map<number, GraphNode[]>();
            for (const n of tasks) {
                const ci = targetSet.has(n.id) ? -1 : (clusterIdx.get(n.id) ?? -2);
                const arr = grouped.get(ci) || [];
                arr.push(n);
                grouped.set(ci, arr);
            }
            const groups = [...grouped.entries()].map(([ci, tasks]) => {
                tasks.sort((a, b) => (b.criticality - a.criticality) || a.label.localeCompare(b.label));
                const label = ci === -1 ? '◎ TARGETS'
                    : ci === -2 ? 'unsorted'
                    : clusterLabel(clusters[ci]);
                return { clusterIndex: ci, label, tasks };
            });
            groups.sort((a, b) => a.clusterIndex - b.clusterIndex);
            return { depth: d, groups };
        });
    }

    $: waves = multi ? buildWaves(multi.nodes, depth, targetSet) : [];

    function statusEmoji(s: string): string {
        switch (s) {
            case 'done': return '✓';
            case 'cancelled': return '×';
            case 'blocked': return '⛔';
            case 'in_progress': return '●';
            case 'review': return '◷';
            case 'merge_ready': return '⊞';
            default: return '○';
        }
    }

    function waveLabel(d: number, isFirst: boolean, isLast: boolean): string {
        if (isLast) return `Wave ${d} · finish line`;
        if (isFirst) return `Wave ${d} · ready now`;
        return `Wave ${d}`;
    }

    function routeChips(id: string): string[] {
        const r = multi?.routes.get(id);
        if (!r) return [];
        return [...r];
    }
</script>

<div class="wave-root" data-component="wave-kanban-view">
    {#if !$graphData || !multi}
        <div class="empty">Loading…</div>
    {:else if multi.targets.length === 0}
        <div class="empty">No active targets found.</div>
    {:else}
        <div class="caption">
            <strong>Depth-Wave Kanban</strong>
            <span class="meta">· {multi.targets.length} targets · {multi.nodes.length - multi.targets.length} contributing tasks · each column = parallel work at the same dependency depth · italic <em class="sib">contributes</em> = sibling under the target's project</span>
        </div>
        <div class="board">
            {#each waves as wave, wi}
                {@const isFirst = wi === 0}
                {@const isLast = wi === waves.length - 1}
                {@const totalTasks = wave.groups.reduce((s, g) => s + g.tasks.length, 0)}
                {@const doneCount = wave.groups.reduce((s, g) => s + g.tasks.filter(isCompleted).length, 0)}
                <div class="column" class:first={isFirst} class:last={isLast}>
                    <header>
                        <div class="col-title">{waveLabel(wave.depth, isFirst, isLast)}</div>
                        <div class="col-meta">{doneCount}/{totalTasks} done</div>
                        <div class="progress-track">
                            <div class="progress-fill" style="width: {totalTasks > 0 ? (doneCount / totalTasks) * 100 : 0}%"></div>
                        </div>
                    </header>
                    <div class="col-body">
                        {#each wave.groups as group}
                            <div class="group">
                                <div class="group-label" style={group.clusterIndex >= 0 ? `color: hsl(${(group.clusterIndex * 67) % 360}, 60%, 65%)` : ''}>
                                    {group.label}
                                    <span class="group-count">{group.tasks.length}</span>
                                </div>
                                {#each group.tasks as task}
                                    {@const isTarget = targetSet.has(task.id)}
                                    {@const stroke = isTarget ? '#f59e0b' : (task.project ? projectColor(task.project) : '#475569')}
                                    {@const chips = isTarget ? [] : routeChips(task.id)}
                                    <button class="card"
                                            class:done={isCompleted(task)}
                                            class:in-progress={task.status === 'in_progress'}
                                            class:blocked={task.status === 'blocked'}
                                            class:target={isTarget}
                                            style="border-left-color: {stroke}"
                                            title={`${task.label}\n${task.type} · P${task.priority ?? '?'} · ${task.status}${task.criticality > 0 ? ` · crit ${task.criticality.toFixed(2)}` : ''}${chips.length > 1 ? `\nServes ${chips.length} targets` : ''}`}
                                            onclick={(e) => { e.stopPropagation(); toggleSelection(task.id); }}>
                                        <div class="card-row">
                                            <span class="status">{statusEmoji(task.status)}</span>
                                            <span class="title">{task.label}</span>
                                            {#if task.criticality > 0.5}<span class="crit">!</span>{/if}
                                        </div>
                                        <div class="card-meta">
                                            <span>{task.type}</span>
                                            <span>P{task.priority ?? '?'}</span>
                                            {#if task.uncertainty > 0.3}<span class="uncertain">~{task.uncertainty.toFixed(1)}</span>{/if}
                                            {#if chips.length > 1}<span class="route">→ {chips.length} targets</span>{/if}
                                            {#if multi.provenance.get(task.id) === 'sibling'}<span class="sib">contributes</span>{/if}
                                            {#if multi.provenance.get(task.id) === 'ancestor'}<span class="anc">scope</span>{/if}
                                        </div>
                                    </button>
                                {/each}
                            </div>
                        {/each}
                    </div>
                </div>
                {#if !isLast}
                    <div class="wave-arrow">→</div>
                {/if}
            {/each}
        </div>
    {/if}
</div>

<style>
    .wave-root {
        width: 100%;
        height: 100%;
        display: flex;
        flex-direction: column;
        background: var(--color-surface, #0f172a);
        color: var(--color-primary, #cbd5e1);
        font-family: ui-monospace, monospace;
        overflow: hidden;
    }
    .caption {
        padding: 8px 14px;
        font-size: 11px;
        border-bottom: 1px solid color-mix(in srgb, var(--color-primary) 12%, transparent);
    }
    .meta { opacity: 0.6; margin-left: 6px; }
    .board {
        flex: 1;
        display: flex;
        flex-direction: row;
        gap: 0;
        overflow-x: auto;
        overflow-y: hidden;
        padding: 14px;
        align-items: stretch;
    }
    .column {
        min-width: 240px;
        max-width: 280px;
        display: flex;
        flex-direction: column;
        background: rgba(148,163,184,0.04);
        border: 1px solid rgba(148,163,184,0.12);
        border-radius: 6px;
        overflow: hidden;
    }
    .column.first { border-left: 3px solid #10b981; }
    .column.last { border-right: 3px solid #f59e0b; min-width: 290px; }
    .column header {
        padding: 8px 10px;
        background: rgba(148,163,184,0.06);
        border-bottom: 1px solid rgba(148,163,184,0.12);
    }
    .col-title { font-size: 11px; font-weight: 700; letter-spacing: 0.06em; }
    .col-meta { font-size: 9px; opacity: 0.6; margin-top: 2px; }
    .progress-track {
        margin-top: 5px; height: 3px;
        background: rgba(148,163,184,0.15);
        border-radius: 2px; overflow: hidden;
    }
    .progress-fill { height: 100%; background: linear-gradient(90deg, #10b981, #f59e0b); }
    .col-body {
        flex: 1; overflow-y: auto; padding: 8px;
        display: flex; flex-direction: column; gap: 10px;
    }
    .group { display: flex; flex-direction: column; gap: 4px; }
    .group-label {
        font-size: 9px; font-weight: 700; letter-spacing: 0.1em;
        text-transform: uppercase; opacity: 0.85;
        display: flex; align-items: center; gap: 6px;
    }
    .group-count {
        font-size: 8px;
        background: rgba(148,163,184,0.1);
        padding: 1px 5px; border-radius: 6px; opacity: 0.7;
    }
    .card {
        text-align: left;
        background: rgba(15,23,42,0.7);
        border: 1px solid rgba(148,163,184,0.18);
        border-left: 3px solid #475569;
        padding: 6px 8px; border-radius: 4px;
        cursor: pointer; font-family: inherit; color: inherit;
        transition: transform 0.1s, background 0.1s;
    }
    .card:hover { background: rgba(30,41,59,0.9); transform: translateX(2px); }
    .card.done { opacity: 0.45; }
    .card.in-progress { background: rgba(30,64,175,0.2); }
    .card.blocked { background: rgba(127,29,29,0.2); }
    .card.target {
        background: rgba(245,158,11,0.18);
        border-color: #f59e0b;
        box-shadow: 0 0 0 1px rgba(245,158,11,0.3);
    }
    .card-row { display: flex; gap: 6px; align-items: baseline; font-size: 11px; }
    .status { width: 12px; flex-shrink: 0; opacity: 0.8; }
    .title {
        flex: 1; overflow: hidden;
        text-overflow: ellipsis; white-space: nowrap;
        font-weight: 500;
    }
    .crit { color: #f59e0b; font-weight: 700; }
    .card-meta {
        margin-top: 3px; font-size: 8.5px;
        display: flex; gap: 6px; opacity: 0.55;
        text-transform: uppercase; letter-spacing: 0.04em;
    }
    .uncertain { color: #fcd34d; }
    .route { color: #93c5fd; font-weight: 700; }
    .sib { color: #a7f3d0; font-style: italic; }
    .anc { color: #c4b5fd; font-style: italic; }
    .wave-arrow {
        align-self: center; font-size: 18px;
        opacity: 0.35; padding: 0 4px;
    }
    .empty { margin: auto; opacity: 0.6; font-size: 12px; }
</style>
