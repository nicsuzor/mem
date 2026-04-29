<script lang="ts">
    import { graphData } from '../../stores/graph';
    import { selection, toggleSelection } from '../../stores/selection';
    import {
        extractMultiTargetSubgraph,
        multiAsExtracted,
        pickAllTargets,
        computeDependencyDepth,
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

    let hovered: { row: number; col: number; type: string } | null = null;

    interface Cell {
        type: 'depends_on' | 'soft_depends_on' | 'parent';
    }

    interface Block {
        targetId: string;
        label: string;
        start: number;
        end: number; // inclusive
    }

    interface Matrix {
        order: GraphNode[];
        block: number[];   // block index per row (which target band)
        blocks: Block[];
        cells: Map<string, Cell>;
        size: number;
    }

    function buildMatrix(
        multi: NonNullable<ReturnType<typeof extractMultiTargetSubgraph>>,
        depth: Map<string, number>,
    ): Matrix {
        // Each prereq node is assigned to exactly one block, owned by the
        // highest-priority target it serves. Targets sit at the end of their
        // own block as the diagonal terminus.
        const targetById = new Map(multi.targets.map(t => [t.id, t]));
        const ownerByNode = new Map<string, string>();
        for (const [nid, ts] of multi.routes) {
            if (targetById.has(nid)) continue;
            let best: GraphNode | null = null;
            for (const tid of ts) {
                const t = targetById.get(tid);
                if (!t) continue;
                if (!best || (t.priority ?? 4) < (best.priority ?? 4)) best = t;
            }
            if (best) ownerByNode.set(nid, best.id);
        }

        const orderedTargets = [...multi.targets].sort((a, b) =>
            (a.priority ?? 4) - (b.priority ?? 4) || a.label.localeCompare(b.label));

        const order: GraphNode[] = [];
        const blockIdx: number[] = [];
        const blocks: Block[] = [];

        orderedTargets.forEach((t, ti) => {
            const start = order.length;
            const own = multi.nodes.filter(n => ownerByNode.get(n.id) === t.id);
            own.sort((a, b) =>
                (depth.get(a.id) ?? 0) - (depth.get(b.id) ?? 0)
                || a.label.localeCompare(b.label));
            for (const n of own) {
                order.push(n);
                blockIdx.push(ti);
            }
            order.push(t);
            blockIdx.push(ti);
            const end = order.length - 1;
            blocks.push({ targetId: t.id, label: t.label, start, end });
        });

        const idIndex = new Map(order.map((n, i) => [n.id, i]));
        const cells = new Map<string, Cell>();
        for (const e of multi.edges) {
            const sid = typeof e.source === 'object' ? e.source.id : e.source;
            const tid = typeof e.target === 'object' ? e.target.id : e.target;
            const row = idIndex.get(sid);
            const col = idIndex.get(tid);
            if (row == null || col == null) continue;
            let type: Cell['type'];
            if (e.type === 'depends_on') type = 'depends_on';
            else if (e.type === 'soft_depends_on') type = 'soft_depends_on';
            else type = 'parent';
            cells.set(`${row},${col}`, { type });
        }

        return { order, block: blockIdx, blocks, cells, size: order.length };
    }

    $: matrix = multi ? buildMatrix(multi, depth) : null;

    // Cell size: when there are many nodes we still want each cell clickable.
    // Floor at 9px (clickable) and cap at 22px. Labels become noisy when the
    // cell is smaller than the label's natural line height — we hide non-target
    // labels below that threshold and only show targets + every Nth tick.
    $: cellSize = matrix ? Math.max(9, Math.min(22, Math.floor(900 / Math.max(1, matrix.size)))) : 12;
    $: labelStride = cellSize >= 12 ? 1 : Math.ceil(12 / cellSize);
    $: showAllLabels = cellSize >= 12;
    const labelW = 240;
    const labelH = 240;

    function cellFill(c: Cell | undefined): string {
        if (!c) return 'transparent';
        if (c.type === 'depends_on') return '#ef4444';
        if (c.type === 'soft_depends_on') return '#b91c1c';
        return '#facc15';
    }

    function isTarget(id: string) {
        return multi ? multi.targets.some(t => t.id === id) : false;
    }
</script>

<div class="dsm-root" data-component="dsm-view">
    {#if !$graphData || !multi || !matrix}
        <div class="empty">Loading…</div>
    {:else if multi.targets.length === 0}
        <div class="empty">No active targets found.</div>
    {:else}
        <div class="caption">
            <strong>Dependency Structure Matrix</strong>
            <span class="meta">
                · {multi.targets.length} targets · {matrix.size} nodes · {matrix.cells.size} edges
                · one diagonal block per target
            </span>
        </div>
        <div class="legend">
            <span><span class="swatch" style="background: #ef4444"></span> hard depends_on</span>
            <span><span class="swatch" style="background: #b91c1c"></span> soft depends_on</span>
            <span><span class="swatch" style="background: #facc15"></span> parent / contains</span>
            <span class="hint">cells above the diagonal = backward edges (cycles or bypasses)</span>
        </div>
        <div class="canvas-wrap">
            <svg width={labelW + matrix.size * cellSize + 12}
                 height={labelH + matrix.size * cellSize + 12}>
                <!-- Per-target blocks -->
                {#each matrix.blocks as b, bi}
                    <rect class="cluster-band"
                          x={labelW + b.start * cellSize}
                          y={labelH + b.start * cellSize}
                          width={(b.end - b.start + 1) * cellSize}
                          height={(b.end - b.start + 1) * cellSize}
                          fill={`hsla(${(bi * 67) % 360}, 70%, 60%, 0.08)`}
                          stroke={`hsla(${(bi * 67) % 360}, 70%, 60%, 0.65)`}
                          stroke-width="1.5" />
                    <text class="block-label"
                          x={labelW + b.start * cellSize + 2}
                          y={labelH + b.start * cellSize - 4}
                          fill={`hsl(${(bi * 67) % 360}, 60%, 70%)`}>
                        ◎ {b.label.length > 30 ? b.label.slice(0, 29) + '…' : b.label}
                    </text>
                {/each}

                <!-- Diagonal -->
                <line x1={labelW} y1={labelH}
                      x2={labelW + matrix.size * cellSize}
                      y2={labelH + matrix.size * cellSize}
                      stroke="rgba(148,163,184,0.3)" stroke-dasharray="3,3" />

                <!-- Row labels: always show targets, others only if cells are big enough or every Nth -->
                {#each matrix.order as n, i}
                    {@const tgt = isTarget(n.id)}
                    {#if tgt || showAllLabels || i % labelStride === 0}
                        <text class="row-label" class:target-row={tgt}
                              role="button" tabindex="0"
                              aria-label={`Row: ${n.label}`}
                              x={labelW - 6} y={labelH + i * cellSize + cellSize / 2 + 3}
                              onclick={(e) => { e.stopPropagation(); toggleSelection(n.id); }}
                              onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggleSelection(n.id); } }}>
                            <title>{n.label} — {n.type} · P{n.priority ?? '?'} · {n.status}</title>
                            {(tgt ? '◎ ' : '') + (n.label.length > 30 ? n.label.slice(0, 29) + '…' : n.label)}
                        </text>
                    {/if}
                {/each}

                <!-- Column labels: same density rule -->
                {#each matrix.order as n, i}
                    {@const tgt = isTarget(n.id)}
                    {#if tgt || showAllLabels || i % labelStride === 0}
                        <g transform={`translate(${labelW + i * cellSize + cellSize / 2}, ${labelH - 6}) rotate(-55)`}>
                            <text class="col-label" class:target-row={tgt}
                                  role="button" tabindex="0"
                                  aria-label={`Column: ${n.label}`}
                                  onclick={(e) => { e.stopPropagation(); toggleSelection(n.id); }}
                                  onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggleSelection(n.id); } }}>
                                <title>{n.label} — {n.type} · P{n.priority ?? '?'} · {n.status}</title>
                                {(tgt ? '◎ ' : '') + (n.label.length > 30 ? n.label.slice(0, 29) + '…' : n.label)}
                            </text>
                        </g>
                    {/if}
                {/each}

                <!-- Cells -->
                {#each matrix.order as rowNode, r}
                    {#each matrix.order as colNode, c}
                        {@const cell = matrix.cells.get(`${r},${c}`)}
                        <rect class="cell"
                              class:hover={hovered && hovered.row === r && hovered.col === c}
                              role={cell ? 'button' : undefined}
                              tabindex={cell ? 0 : undefined}
                              x={labelW + c * cellSize}
                              y={labelH + r * cellSize}
                              width={cellSize - 1}
                              height={cellSize - 1}
                              fill={cellFill(cell)}
                              opacity={cell ? (isCompleted(rowNode) || isCompleted(colNode) ? 0.35 : 0.9) : 0}
                              onmouseenter={() => cell && (hovered = { row: r, col: c, type: cell.type })}
                              onmouseleave={() => hovered = null}
                              onclick={(e) => { e.stopPropagation(); if (cell) toggleSelection(rowNode.id); }}
                              onkeydown={(e) => { if (cell && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); toggleSelection(rowNode.id); } }}>
                            {#if cell}
                                <title>{rowNode.label} {cell.type === 'parent' ? '⊃ contains' : '→ depends on'} {colNode.label}</title>
                            {/if}
                        </rect>
                    {/each}
                {/each}
            </svg>
            {#if hovered}
                {@const r = matrix.order[hovered.row]}
                {@const c = matrix.order[hovered.col]}
                <div class="tooltip">
                    <div><strong>{r.label}</strong></div>
                    <div class="tip-arrow">{hovered.type === 'parent' ? '⊃ contains' : '→ depends on'}</div>
                    <div><strong>{c.label}</strong></div>
                </div>
            {/if}
        </div>
    {/if}
</div>

<style>
    .dsm-root {
        width: 100%; height: 100%;
        display: flex; flex-direction: column;
        background: var(--color-surface, #0f172a);
        color: var(--color-primary, #cbd5e1);
        font-family: ui-monospace, monospace;
        overflow: hidden;
    }
    .caption {
        padding: 8px 14px; font-size: 11px;
        border-bottom: 1px solid color-mix(in srgb, var(--color-primary) 12%, transparent);
    }
    .meta { opacity: 0.6; margin-left: 6px; }
    .legend {
        display: flex; gap: 16px; padding: 6px 14px;
        font-size: 10px; flex-wrap: wrap;
        border-bottom: 1px solid color-mix(in srgb, var(--color-primary) 8%, transparent);
        align-items: center;
    }
    .swatch {
        display: inline-block; width: 10px; height: 10px;
        border-radius: 2px; vertical-align: middle; margin-right: 4px;
    }
    .hint { opacity: 0.5; font-style: italic; margin-left: auto; }
    .canvas-wrap { flex: 1; overflow: auto; padding: 12px; position: relative; }
    .row-label, .col-label {
        font-size: 9.5px;
        fill: color-mix(in srgb, var(--color-primary) 75%, transparent);
        cursor: pointer;
    }
    .row-label { text-anchor: end; }
    .col-label { text-anchor: start; }
    .row-label:hover, .col-label:hover { fill: var(--color-primary); }
    .target-row { fill: #f59e0b !important; font-weight: 700; }
    .block-label {
        font-size: 10px;
        font-weight: 700;
        letter-spacing: 0.04em;
    }
    .cell {
        stroke: rgba(148,163,184,0.06);
        stroke-width: 0.5;
        transition: opacity 0.1s;
    }
    .cell.hover { stroke: #fff; stroke-width: 1.5; }
    .tooltip {
        position: absolute; top: 14px; right: 14px;
        background: rgba(15,23,42,0.95);
        border: 1px solid rgba(245,158,11,0.5);
        padding: 8px 10px; border-radius: 4px;
        font-size: 11px; max-width: 320px; pointer-events: none;
    }
    .tip-arrow { opacity: 0.6; font-size: 10px; margin: 3px 0; }
    .empty { margin: auto; opacity: 0.6; font-size: 12px; }
</style>
