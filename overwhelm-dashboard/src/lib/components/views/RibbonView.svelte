<script lang="ts">
    import { graphData } from '../../stores/graph';
    import { selection, toggleSelection } from '../../stores/selection';
    import { projectColor } from '../../data/projectUtils';
    import {
        extractSubgraph,
        pickDefaultTarget,
        findClusters,
        clusterLabel,
        isCompleted,
    } from '../../data/subgraphExtraction';
    import type { GraphNode } from '../../data/prepareGraphData';

    $: targetId = $selection.focusNodeId || pickDefaultTarget($graphData);
    $: subgraph = $graphData && targetId ? extractSubgraph($graphData, targetId) : null;
    $: clusters = subgraph ? findClusters(subgraph) : [];
    $: target = subgraph ? subgraph.nodes.find(n => n.id === subgraph.targetId) : null;

    interface Ribbon {
        cluster: GraphNode[];
        label: string;
        scope: number;          // total scope sum (used for width)
        completedScope: number;
        criticality: number;    // max criticality
        uncertainty: number;    // max uncertainty
        color: string;
        progress: number;       // 0..1
    }

    function buildRibbons(clusters: GraphNode[][]): Ribbon[] {
        return clusters.map((cluster) => {
            const scope = cluster.reduce((s, n) => s + Math.max(1, n.scope || 1), 0);
            const completedScope = cluster
                .filter(isCompleted)
                .reduce((s, n) => s + Math.max(1, n.scope || 1), 0);
            const criticality = Math.max(0, ...cluster.map(n => n.criticality || 0));
            const uncertainty = Math.max(0, ...cluster.map(n => n.uncertainty || 0));
            const projects = [...new Set(cluster.map(n => n.project).filter(Boolean))];
            const color = projects.length === 1 && projects[0]
                ? projectColor(projects[0]!)
                : `hsl(${(cluster.length * 47 + scope * 13) % 360}, 60%, 55%)`;
            return {
                cluster,
                label: clusterLabel(cluster),
                scope,
                completedScope,
                criticality,
                uncertainty,
                color,
                progress: scope > 0 ? completedScope / scope : 0,
            };
        });
    }

    $: ribbons = buildRibbons(clusters);

    // Layout: stack ribbons vertically, width proportional to scope, all
    // converging at a target marker on the right.
    const RIBBON_H = 56;
    const RIBBON_GAP = 14;
    const LEFT_PAD = 30;
    const RIGHT_PAD = 180;
    const MAX_W = 900;
    const MIN_W = 220;

    $: maxScope = Math.max(1, ...ribbons.map(r => r.scope));
    $: layout = ribbons.map((r, i) => {
        const w = MIN_W + (MAX_W - MIN_W) * Math.sqrt(r.scope / maxScope);
        return {
            ribbon: r,
            x: LEFT_PAD,
            y: 30 + i * (RIBBON_H + RIBBON_GAP),
            w,
        };
    });
    $: totalH = ribbons.length === 0 ? 200 : 30 + ribbons.length * (RIBBON_H + RIBBON_GAP) + 30;
    $: targetX = LEFT_PAD + MAX_W + 40;
    $: totalW = targetX + RIGHT_PAD;

    function criticalityColor(c: number): string {
        if (c > 0.7) return '#dc2626';
        if (c > 0.4) return '#f59e0b';
        if (c > 0.2) return '#facc15';
        return '#94a3b8';
    }
</script>

<div class="ribbon-root" data-component="ribbon-view">
    {#if !$graphData || !subgraph}
        <div class="empty">Loading…</div>
    {:else if !target}
        <div class="empty">No target node selected.</div>
    {:else}
        <div class="caption">
            <strong>Project Ribbons</strong>
            <span class="meta">· each ribbon = one independent prerequisite path · width = scope · saturation = progress</span>
        </div>
        <div class="canvas-wrap">
            <svg width={totalW} height={totalH}>
                <defs>
                    <linearGradient id="completedFade" x1="0" y1="0" x2="1" y2="0">
                        <stop offset="0" stop-color="#1e293b" stop-opacity="0.7" />
                        <stop offset="1" stop-color="#1e293b" stop-opacity="0.1" />
                    </linearGradient>
                </defs>

                <!-- Convergence rays from each ribbon to target -->
                {#each layout as l}
                    <path d={`M ${l.x + l.w} ${l.y + RIBBON_H / 2} Q ${(l.x + l.w + targetX) / 2} ${l.y + RIBBON_H / 2}, ${targetX - 30} ${totalH / 2}`}
                          fill="none"
                          stroke={l.ribbon.color}
                          stroke-width="2"
                          stroke-dasharray="4,3"
                          opacity="0.4" />
                {/each}

                <!-- Ribbons -->
                {#each layout as l}
                    {@const r = l.ribbon}
                    <g class="ribbon" onclick={(e) => {
                        e.stopPropagation();
                        const top = [...r.cluster].sort((a, b) => (b.focusScore||0)-(a.focusScore||0))[0];
                        if (top) toggleSelection(top.id);
                    }}>
                        <!-- main ribbon -->
                        <rect x={l.x} y={l.y} width={l.w} height={RIBBON_H} rx="6"
                              fill={r.color} opacity="0.18" stroke={r.color} stroke-width="1.5" />
                        <!-- progress fill -->
                        <rect x={l.x} y={l.y} width={l.w * r.progress} height={RIBBON_H} rx="6"
                              fill={r.color} opacity="0.55" />
                        <!-- buffer marker at the merge point -->
                        <rect x={l.x + l.w - 6} y={l.y + 4} width="6" height={RIBBON_H - 8}
                              fill={criticalityColor(r.criticality)} opacity="0.85" />
                        <!-- uncertainty stipple via opacity -->
                        {#if r.uncertainty > 0.3}
                            <rect x={l.x} y={l.y} width={l.w} height={RIBBON_H} rx="6"
                                  fill="url(#completedFade)" opacity={r.uncertainty * 0.5} />
                        {/if}

                        <text class="ribbon-label" x={l.x + 12} y={l.y + 22}>
                            {r.label.length > 36 ? r.label.slice(0, 35) + '…' : r.label}
                        </text>
                        <text class="ribbon-meta" x={l.x + 12} y={l.y + 40}>
                            {r.cluster.length} task{r.cluster.length === 1 ? '' : 's'}
                            · {Math.round(r.progress * 100)}%
                            {#if r.criticality > 0.4}· crit {r.criticality.toFixed(2)}{/if}
                            {#if r.uncertainty > 0.3}· uncertain {r.uncertainty.toFixed(2)}{/if}
                        </text>

                        <!-- task ticks along the ribbon -->
                        {#each r.cluster.slice(0, Math.floor(l.w / 8)) as task, ti}
                            {@const tx = l.x + (ti + 0.5) * (l.w / r.cluster.length)}
                            <circle cx={tx} cy={l.y + RIBBON_H - 6} r="2"
                                    fill={isCompleted(task) ? '#10b981' : (task.status === 'in_progress' ? '#3b82f6' : '#cbd5e1')}
                                    opacity="0.85" />
                        {/each}
                    </g>
                {/each}

                <!-- Target node -->
                <g class="target" onclick={(e) => { e.stopPropagation(); target && toggleSelection(target.id); }}>
                    <circle cx={targetX} cy={totalH / 2} r="34"
                            fill="#fef3c7" stroke="#f59e0b" stroke-width="3" />
                    <text class="target-icon" x={targetX} y={totalH / 2 - 4}>◎</text>
                    <text class="target-label" x={targetX} y={totalH / 2 + 14}>
                        {target.label.length > 14 ? target.label.slice(0, 13) + '…' : target.label}
                    </text>
                </g>

                <!-- Critical-path indicator (longest by scope) -->
                {#if ribbons.length > 1}
                    {@const critical = ribbons.reduce((a, b) => a.scope > b.scope ? a : b)}
                    <text class="annotation" x={LEFT_PAD} y={totalH - 8}>
                        critical chain: <tspan style="fill: #f59e0b">{critical.label}</tspan>
                        ({critical.cluster.length} tasks, scope {critical.scope})
                    </text>
                {/if}
            </svg>
        </div>
    {/if}
</div>

<style>
    .ribbon-root {
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
    .canvas-wrap {
        flex: 1;
        overflow: auto;
        padding: 16px;
    }
    .ribbon { cursor: pointer; }
    .ribbon:hover rect { filter: brightness(1.3); }
    .ribbon-label {
        font-size: 12px;
        font-weight: 700;
        fill: var(--color-primary);
    }
    .ribbon-meta {
        font-size: 10px;
        fill: color-mix(in srgb, var(--color-primary) 65%, transparent);
    }
    .target { cursor: pointer; }
    .target:hover circle { fill: #fed7aa; }
    .target-icon {
        font-size: 24px;
        text-anchor: middle;
        fill: #92400e;
    }
    .target-label {
        font-size: 10px;
        text-anchor: middle;
        fill: #78350f;
        font-weight: 700;
    }
    .annotation {
        font-size: 11px;
        fill: color-mix(in srgb, var(--color-primary) 55%, transparent);
    }
    .empty { margin: auto; opacity: 0.6; font-size: 12px; }
</style>
