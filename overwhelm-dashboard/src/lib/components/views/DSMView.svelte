<script lang="ts">
    import { graphData } from '../../stores/graph';
    import { selection, toggleSelection } from '../../stores/selection';
    import {
        extractSubgraph,
        pickDefaultTarget,
        computeDependencyDepth,
        findClusters,
        isCompleted,
    } from '../../data/subgraphExtraction';
    import type { GraphNode } from '../../data/prepareGraphData';

    $: targetId = $selection.focusNodeId || pickDefaultTarget($graphData);
    $: subgraph = $graphData && targetId ? extractSubgraph($graphData, targetId) : null;
    $: depth = subgraph ? computeDependencyDepth(subgraph) : new Map<string, number>();
    $: clusters = subgraph ? findClusters(subgraph) : [];

    let hovered: { row: number; col: number; type: string } | null = null;

    interface Cell {
        type: 'depends_on' | 'soft_depends_on' | 'parent';
        // we render at (row=src, col=dst) — meaning "row depends on col"
    }

    interface Matrix {
        order: GraphNode[];
        clusterIdx: number[];   // cluster index per row (-1 for target)
        cells: Map<string, Cell>; // key: `${row},${col}`
        size: number;
    }

    function buildMatrix(
        clusters: GraphNode[][],
        depth: Map<string, number>,
        sub: ReturnType<typeof extractSubgraph>,
    ): Matrix {
        // Order: target last; otherwise group by cluster, within cluster by depth
        const order: GraphNode[] = [];
        const clusterIdx: number[] = [];
        clusters.forEach((cluster, ci) => {
            const sorted = [...cluster].sort((a, b) =>
                (depth.get(a.id) ?? 0) - (depth.get(b.id) ?? 0) ||
                a.label.localeCompare(b.label));
            for (const n of sorted) {
                order.push(n);
                clusterIdx.push(ci);
            }
        });
        const target = sub.nodes.find(n => n.id === sub.targetId);
        if (target) {
            order.push(target);
            clusterIdx.push(-1);
        }

        const idIndex = new Map(order.map((n, i) => [n.id, i]));
        const cells = new Map<string, Cell>();
        for (const e of sub.edges) {
            const sid = typeof e.source === 'object' ? e.source.id : e.source;
            const tid = typeof e.target === 'object' ? e.target.id : e.target;
            let row: number | undefined;
            let col: number | undefined;
            let type: Cell['type'];
            if (e.type === 'depends_on' || e.type === 'soft_depends_on') {
                // sid depends on tid -> mark row=sid, col=tid (below diagonal after topo sort)
                row = idIndex.get(sid);
                col = idIndex.get(tid);
                type = e.type as Cell['type'];
            } else {
                // parent edge: parent (source after flip) depends on child completing
                row = idIndex.get(sid); // parent
                col = idIndex.get(tid); // child
                type = 'parent';
            }
            if (row != null && col != null) cells.set(`${row},${col}`, { type });
        }
        return { order, clusterIdx, cells, size: order.length };
    }

    $: matrix = subgraph ? buildMatrix(clusters, depth, subgraph) : null;

    // Cell sizing based on matrix size — keep readable for small N, dense for large
    $: cellSize = matrix ? Math.max(8, Math.min(28, Math.floor(640 / Math.max(1, matrix.size)))) : 16;
    $: labelW = 220;
    $: labelH = 220;

    function cellFill(c: Cell | undefined): string {
        if (!c) return 'transparent';
        if (c.type === 'depends_on') return '#ef4444';
        if (c.type === 'soft_depends_on') return '#b91c1c';
        return '#facc15'; // parent
    }
</script>

<div class="dsm-root" data-component="dsm-view">
    {#if !$graphData || !subgraph || !matrix}
        <div class="empty">Loading…</div>
    {:else}
        <div class="caption">
            <strong>Dependency Structure Matrix</strong>
            <span class="meta">· {matrix.size} nodes · {matrix.cells.size} edges · clusters along the diagonal</span>
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

                <!-- Cluster bands along diagonal -->
                {#each clusters as cluster, ci}
                    {@const start = matrix.clusterIdx.indexOf(ci)}
                    {@const end = matrix.clusterIdx.lastIndexOf(ci)}
                    {#if start >= 0}
                        <rect class="cluster-band"
                              x={labelW + start * cellSize}
                              y={labelH + start * cellSize}
                              width={(end - start + 1) * cellSize}
                              height={(end - start + 1) * cellSize}
                              fill={`hsla(${(ci * 67) % 360}, 70%, 60%, 0.08)`}
                              stroke={`hsla(${(ci * 67) % 360}, 70%, 60%, 0.5)`}
                              stroke-width="1.5" />
                    {/if}
                {/each}

                <!-- Diagonal -->
                <line x1={labelW} y1={labelH}
                      x2={labelW + matrix.size * cellSize}
                      y2={labelH + matrix.size * cellSize}
                      stroke="rgba(148,163,184,0.35)" stroke-dasharray="3,3" />

                <!-- Row labels (left) -->
                {#each matrix.order as n, i}
                    <text class="row-label" class:target-row={n.id === subgraph.targetId}
                          x={labelW - 6} y={labelH + i * cellSize + cellSize / 2 + 3}
                          on:click|stopPropagation={() => toggleSelection(n.id)}>
                        {n.label.length > 28 ? n.label.slice(0, 27) + '…' : n.label}
                    </text>
                {/each}

                <!-- Column labels (top, rotated) -->
                {#each matrix.order as n, i}
                    <g transform={`translate(${labelW + i * cellSize + cellSize / 2}, ${labelH - 6}) rotate(-55)`}>
                        <text class="col-label" class:target-row={n.id === subgraph.targetId}
                              on:click|stopPropagation={() => toggleSelection(n.id)}>
                            {n.label.length > 28 ? n.label.slice(0, 27) + '…' : n.label}
                        </text>
                    </g>
                {/each}

                <!-- Cells -->
                {#each matrix.order as rowNode, r}
                    {#each matrix.order as colNode, c}
                        {@const cell = matrix.cells.get(`${r},${c}`)}
                        <rect class="cell"
                              class:hover={hovered && hovered.row === r && hovered.col === c}
                              x={labelW + c * cellSize}
                              y={labelH + r * cellSize}
                              width={cellSize - 1}
                              height={cellSize - 1}
                              fill={cellFill(cell)}
                              opacity={cell ? (isCompleted(rowNode) || isCompleted(colNode) ? 0.35 : 0.9) : 0}
                              on:mouseenter={() => cell && (hovered = { row: r, col: c, type: cell.type })}
                              on:mouseleave={() => hovered = null}
                              on:click|stopPropagation={() => cell && toggleSelection(rowNode.id)} />
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
    .legend {
        display: flex;
        gap: 16px;
        padding: 6px 14px;
        font-size: 10px;
        flex-wrap: wrap;
        border-bottom: 1px solid color-mix(in srgb, var(--color-primary) 8%, transparent);
        align-items: center;
    }
    .swatch {
        display: inline-block;
        width: 10px;
        height: 10px;
        border-radius: 2px;
        vertical-align: middle;
        margin-right: 4px;
    }
    .hint {
        opacity: 0.5;
        font-style: italic;
        margin-left: auto;
    }
    .canvas-wrap {
        flex: 1;
        overflow: auto;
        padding: 12px;
        position: relative;
    }
    .row-label, .col-label {
        font-size: 9.5px;
        fill: color-mix(in srgb, var(--color-primary) 75%, transparent);
        cursor: pointer;
    }
    .row-label { text-anchor: end; }
    .col-label { text-anchor: start; }
    .row-label:hover, .col-label:hover { fill: var(--color-primary); }
    .target-row { fill: #f59e0b !important; font-weight: 700; }
    .cell {
        stroke: rgba(148,163,184,0.08);
        stroke-width: 0.5;
        transition: opacity 0.1s;
    }
    .cell.hover { stroke: #fff; stroke-width: 1.5; }
    .tooltip {
        position: absolute;
        top: 14px;
        right: 14px;
        background: rgba(15,23,42,0.95);
        border: 1px solid rgba(245,158,11,0.5);
        padding: 8px 10px;
        border-radius: 4px;
        font-size: 11px;
        max-width: 280px;
        pointer-events: none;
    }
    .tip-arrow {
        opacity: 0.6;
        font-size: 10px;
        margin: 3px 0;
    }
    .empty { margin: auto; opacity: 0.6; font-size: 12px; }
</style>
