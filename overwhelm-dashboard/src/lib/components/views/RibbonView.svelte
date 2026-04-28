<script lang="ts">
    import { graphData } from '../../stores/graph';
    import { selection, toggleSelection } from '../../stores/selection';
    import { projectColor } from '../../data/projectUtils';
    import {
        extractMultiTargetSubgraph,
        pickAllTargets,
        isCompleted,
    } from '../../data/subgraphExtraction';
    import type { GraphNode } from '../../data/prepareGraphData';

    $: targets = pickAllTargets($graphData);
    $: focusOverride = $selection.focusNodeId;
    $: targetIds = focusOverride ? [focusOverride] : targets.map(t => t.id);
    $: multi = $graphData && targetIds.length > 0
        ? extractMultiTargetSubgraph($graphData, targetIds)
        : null;

    interface Ribbon {
        target: GraphNode;
        prereqs: GraphNode[];      // tasks that route to this target (excluding target itself)
        scope: number;
        completedScope: number;
        criticality: number;
        uncertainty: number;
        color: string;
        progress: number;
        sharedCount: number;       // number of prereqs also serving other targets
    }

    function buildRibbons(multi: NonNullable<ReturnType<typeof extractMultiTargetSubgraph>>): Ribbon[] {
        return multi.targets.map((t) => {
            const prereqs: GraphNode[] = [];
            let shared = 0;
            for (const n of multi.nodes) {
                if (n.id === t.id) continue;
                const r = multi.routes.get(n.id);
                if (r && r.has(t.id)) {
                    prereqs.push(n);
                    if (r.size > 1) shared++;
                }
            }
            const scope = prereqs.reduce((s, n) => s + Math.max(1, n.scope || 1), 0);
            const completedScope = prereqs
                .filter(isCompleted)
                .reduce((s, n) => s + Math.max(1, n.scope || 1), 0);
            const criticality = Math.max(0, ...prereqs.map(n => n.criticality || 0));
            const uncertainty = Math.max(0, ...prereqs.map(n => n.uncertainty || 0));
            const color = t.project
                ? projectColor(t.project)
                : `hsl(${(t.label.length * 13) % 360}, 65%, 60%)`;
            return {
                target: t,
                prereqs,
                scope,
                completedScope,
                criticality,
                uncertainty,
                color,
                progress: scope > 0 ? completedScope / scope : 0,
                sharedCount: shared,
            };
        });
    }
    $: ribbons = multi ? buildRibbons(multi) : [];

    const RIBBON_H = 64;
    const RIBBON_GAP = 14;
    const LEFT_PAD = 30;
    const MAX_W = 900;
    const MIN_W = 220;
    const TARGET_R = 26;
    const TARGET_GAP = 60;

    $: maxScope = Math.max(1, ...ribbons.map(r => r.scope));
    $: layout = ribbons.map((r, i) => {
        const w = MIN_W + (MAX_W - MIN_W) * Math.sqrt(r.scope / maxScope);
        return {
            ribbon: r,
            x: LEFT_PAD,
            y: 30 + i * (RIBBON_H + RIBBON_GAP),
            w,
            targetX: LEFT_PAD + MAX_W + TARGET_GAP,
            targetY: 30 + i * (RIBBON_H + RIBBON_GAP) + RIBBON_H / 2,
        };
    });
    $: totalH = ribbons.length === 0 ? 200 : 30 + ribbons.length * (RIBBON_H + RIBBON_GAP) + 40;
    $: totalW = LEFT_PAD + MAX_W + TARGET_GAP + 200;

    function criticalityColor(c: number): string {
        if (c > 0.7) return '#dc2626';
        if (c > 0.4) return '#f59e0b';
        if (c > 0.2) return '#facc15';
        return '#94a3b8';
    }

    function mostUrgentLeaf(prereqs: GraphNode[]): GraphNode | null {
        if (prereqs.length === 0) return null;
        return [...prereqs].sort((a, b) => {
            const ca = isCompleted(a) ? 1 : 0;
            const cb = isCompleted(b) ? 1 : 0;
            if (ca !== cb) return ca - cb;
            return (a.priority ?? 4) - (b.priority ?? 4);
        })[0];
    }
</script>

<div class="ribbon-root" data-component="ribbon-view">
    {#if !$graphData || !multi}
        <div class="empty">Loading…</div>
    {:else if multi.targets.length === 0}
        <div class="empty">No active targets found.</div>
    {:else}
        <div class="caption">
            <strong>Project Ribbons</strong>
            <span class="meta">
                · {multi.targets.length} ribbons (one per target)
                · width = total scope · fill = progress · end-cap = max criticality
            </span>
        </div>
        <div class="canvas-wrap">
            <svg width={totalW} height={totalH}>
                {#each layout as l}
                    {@const r = l.ribbon}
                    <!-- Ribbon -->
                    <g class="ribbon"
                       onclick={(e) => {
                           e.stopPropagation();
                           const next = mostUrgentLeaf(r.prereqs);
                           if (next) toggleSelection(next.id);
                       }}>
                        <rect x={l.x} y={l.y} width={l.w} height={RIBBON_H} rx="6"
                              fill={r.color} opacity="0.18" stroke={r.color} stroke-width="1.5" />
                        <rect x={l.x} y={l.y} width={l.w * r.progress} height={RIBBON_H} rx="6"
                              fill={r.color} opacity="0.55" />
                        <rect x={l.x + l.w - 6} y={l.y + 4} width="6" height={RIBBON_H - 8}
                              fill={criticalityColor(r.criticality)} opacity="0.85" />
                        {#if r.uncertainty > 0.3}
                            <rect x={l.x} y={l.y} width={l.w} height={RIBBON_H} rx="6"
                                  fill="rgba(15,23,42,0.6)" opacity={Math.min(0.5, r.uncertainty * 0.5)} />
                        {/if}

                        <text class="ribbon-meta-tag" x={l.x + 12} y={l.y + 18}>
                            {r.target.label.length > 42 ? r.target.label.slice(0, 41) + '…' : r.target.label}
                        </text>
                        <text class="ribbon-meta" x={l.x + 12} y={l.y + 34}>
                            {r.prereqs.length} prereq{r.prereqs.length === 1 ? '' : 's'}
                            · {Math.round(r.progress * 100)}% done
                            {#if r.criticality > 0.4}· crit {r.criticality.toFixed(2)}{/if}
                            {#if r.uncertainty > 0.3}· uncertain {r.uncertainty.toFixed(2)}{/if}
                        </text>
                        {#if r.sharedCount > 0}
                            <text class="ribbon-shared" x={l.x + 12} y={l.y + 50}>
                                ⇄ {r.sharedCount} shared with other targets
                            </text>
                        {/if}

                        <!-- Task ticks along the ribbon (green=done, blue=in_prog, red=blocked) -->
                        {#each r.prereqs.slice(0, Math.floor(l.w / 6)) as task, ti}
                            {@const tx = l.x + (ti + 0.5) * (l.w / Math.max(1, r.prereqs.length))}
                            <circle cx={tx} cy={l.y + RIBBON_H - 5} r="2"
                                    fill={isCompleted(task) ? '#10b981'
                                        : task.status === 'in_progress' ? '#3b82f6'
                                        : task.status === 'blocked' ? '#ef4444' : '#cbd5e1'}
                                    opacity="0.9" />
                        {/each}
                    </g>

                    <!-- Convergence arrow ribbon-end → target -->
                    <path d={`M ${l.x + l.w} ${l.y + RIBBON_H / 2} Q ${(l.x + l.w + l.targetX) / 2} ${l.y + RIBBON_H / 2}, ${l.targetX - TARGET_R - 2} ${l.targetY}`}
                          fill="none"
                          stroke={r.color}
                          stroke-width="2.5"
                          opacity="0.7" />

                    <!-- Per-target node -->
                    <g class="target-node"
                       onclick={(e) => { e.stopPropagation(); toggleSelection(r.target.id); }}>
                        <circle cx={l.targetX} cy={l.targetY} r={TARGET_R}
                                fill="#fef3c7" stroke={r.color} stroke-width="3" />
                        <text class="target-icon" x={l.targetX} y={l.targetY - 1}>◎</text>
                    </g>
                    <text class="target-label" x={l.targetX + TARGET_R + 8} y={l.targetY + 4}>
                        {r.target.label.length > 22 ? r.target.label.slice(0, 21) + '…' : r.target.label}
                    </text>
                {/each}

                {#if ribbons.length > 1}
                    {@const critical = ribbons.reduce((a, b) => a.scope > b.scope ? a : b)}
                    <text class="annotation" x={LEFT_PAD} y={totalH - 12}>
                        critical chain: <tspan style="fill: #f59e0b">{critical.target.label}</tspan>
                        ({critical.prereqs.length} tasks, scope {critical.scope})
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
    .ribbon-meta-tag {
        font-size: 12px;
        font-weight: 700;
        fill: var(--color-primary);
    }
    .ribbon-meta {
        font-size: 10px;
        fill: color-mix(in srgb, var(--color-primary) 65%, transparent);
    }
    .ribbon-shared {
        font-size: 9.5px;
        fill: #93c5fd;
        font-weight: 600;
    }
    .target-node { cursor: pointer; }
    .target-node:hover circle { fill: #fed7aa; }
    .target-icon {
        font-size: 22px;
        text-anchor: middle;
        fill: #92400e;
        pointer-events: none;
    }
    .target-label {
        font-size: 11px;
        fill: color-mix(in srgb, var(--color-primary) 85%, transparent);
        font-weight: 600;
    }
    .annotation {
        font-size: 11px;
        fill: color-mix(in srgb, var(--color-primary) 55%, transparent);
    }
    .empty { margin: auto; opacity: 0.6; font-size: 12px; }
</style>
