<script lang="ts">
    import { graphData } from '../../stores/graph';
    import { selection, toggleSelection } from '../../stores/selection';
    import { projectColor } from '../../data/projectUtils';
    import {
        extractSubgraph,
        pickDefaultTarget,
        findClusters,
        computeDependencyDepth,
        clusterLabel,
        isCompleted,
    } from '../../data/subgraphExtraction';
    import type { GraphNode, GraphEdge } from '../../data/prepareGraphData';

    const NODE_W = 150;
    const NODE_H = 36;
    const COL_GAP = 28;
    const ROW_GAP = 14;
    const LANE_PAD = 10;
    const LANE_HEADER = 22;

    $: targetId = $selection.focusNodeId || pickDefaultTarget($graphData);
    $: subgraph = $graphData && targetId ? extractSubgraph($graphData, targetId) : null;
    $: clusters = subgraph ? findClusters(subgraph) : [];
    $: depthMap = subgraph ? computeDependencyDepth(subgraph) : new Map<string, number>();
    $: target = subgraph ? subgraph.nodes.find(n => n.id === subgraph.targetId) : null;

    interface Placed {
        node: GraphNode;
        x: number; y: number;
        lane: number; col: number;
    }

    interface LaneLayout {
        cluster: GraphNode[];
        label: string;
        y: number;
        height: number;
        nodes: Placed[];
        maxCol: number;
    }

    interface Layout {
        lanes: LaneLayout[];
        target: Placed | null;
        edges: { e: GraphEdge; x1: number; y1: number; x2: number; y2: number; cross: boolean }[];
        width: number; height: number;
    }

    function buildLayout(
        clusters: GraphNode[][],
        depths: Map<string, number>,
        target: GraphNode | null,
        edges: GraphEdge[],
    ): Layout {
        const placedById = new Map<string, Placed>();
        let cursorY = LANE_PAD;
        const lanes: LaneLayout[] = [];

        let maxCol = 0;
        for (const cluster of clusters) {
            const minD = Math.min(...cluster.map(n => depths.get(n.id) ?? 0));
            const colsByNode = cluster.map(n => ({
                n, col: (depths.get(n.id) ?? 0) - minD
            }));
            const lcMax = Math.max(0, ...colsByNode.map(c => c.col));
            maxCol = Math.max(maxCol, lcMax);
        }
        // Reserve target column at maxCol + 1
        const targetCol = maxCol + 1;

        for (const cluster of clusters) {
            const minD = Math.min(...cluster.map(n => depths.get(n.id) ?? 0));
            const colsByNode = new Map<number, GraphNode[]>();
            for (const n of cluster) {
                const c = (depths.get(n.id) ?? 0) - minD;
                const arr = colsByNode.get(c) || [];
                arr.push(n);
                colsByNode.set(c, arr);
            }
            // Sort each column: criticality desc, then label
            for (const arr of colsByNode.values()) {
                arr.sort((a, b) => (b.criticality - a.criticality) || a.label.localeCompare(b.label));
            }
            const rowsInLane = Math.max(...[...colsByNode.values()].map(a => a.length));
            const laneHeight = LANE_HEADER + rowsInLane * (NODE_H + ROW_GAP) + LANE_PAD;

            const placed: Placed[] = [];
            for (const [col, nodesInCol] of colsByNode) {
                nodesInCol.forEach((n, i) => {
                    const x = LANE_PAD + col * (NODE_W + COL_GAP);
                    const y = cursorY + LANE_HEADER + i * (NODE_H + ROW_GAP);
                    const p: Placed = { node: n, x, y, lane: lanes.length, col };
                    placed.push(p);
                    placedById.set(n.id, p);
                });
            }
            const lcMax = Math.max(0, ...[...colsByNode.keys()]);
            lanes.push({
                cluster,
                label: clusterLabel(cluster),
                y: cursorY,
                height: laneHeight,
                nodes: placed,
                maxCol: lcMax,
            });
            cursorY += laneHeight;
        }

        let targetPlaced: Placed | null = null;
        if (target) {
            const targetX = LANE_PAD + targetCol * (NODE_W + COL_GAP);
            const targetY = LANE_PAD + (cursorY - LANE_PAD) / 2 - NODE_H / 2;
            targetPlaced = { node: target, x: targetX, y: targetY, lane: -1, col: targetCol };
            placedById.set(target.id, targetPlaced);
        }

        const drawEdges: Layout['edges'] = [];
        for (const e of edges) {
            const sid = typeof e.source === 'object' ? e.source.id : e.source;
            const tid = typeof e.target === 'object' ? e.target.id : e.target;
            const a = placedById.get(sid);
            const b = placedById.get(tid);
            if (!a || !b) continue;
            const cross = a.lane !== b.lane && b.lane === -1 || a.lane === -1;
            drawEdges.push({
                e,
                x1: a.x + NODE_W,
                y1: a.y + NODE_H / 2,
                x2: b.x,
                y2: b.y + NODE_H / 2,
                cross: a.lane !== b.lane,
            });
        }

        const totalHeight = cursorY + LANE_PAD;
        const totalWidth = LANE_PAD * 2 + (targetCol + 1) * (NODE_W + COL_GAP);
        return {
            lanes,
            target: targetPlaced,
            edges: drawEdges,
            width: totalWidth,
            height: totalHeight,
        };
    }

    $: layout = subgraph ? buildLayout(clusters, depthMap, target ?? null, subgraph.edges) : null;

    function nodeFill(n: GraphNode) {
        if (isCompleted(n)) return '#1e293b';
        if (n.status === 'in_progress') return '#1e40af';
        if (n.status === 'blocked') return '#7f1d1d';
        return n.fill;
    }
</script>

<div class="swimlane-root" data-component="swimlane-view">
    {#if !$graphData || !subgraph}
        <div class="empty">Loading…</div>
    {:else if !target}
        <div class="empty">Select a target node to view its swimlane breakdown.</div>
    {:else}
        <div class="caption">
            <strong>Target:</strong> {target.label}
            <span class="meta">· {subgraph.nodes.length - 1} prerequisite tasks · {clusters.length} independent paths</span>
        </div>
        {#if layout}
        <div class="canvas-wrap">
            <svg width={layout.width} height={layout.height} class="canvas">
                <defs>
                    <marker id="sw-arrow" markerWidth="9" markerHeight="9" refX="8" refY="4.5" orient="auto">
                        <path d="M0,0 L9,4.5 L0,9 z" fill="#94a3b8" />
                    </marker>
                    <marker id="sw-arrow-cross" markerWidth="9" markerHeight="9" refX="8" refY="4.5" orient="auto">
                        <path d="M0,0 L9,4.5 L0,9 z" fill="#f59e0b" />
                    </marker>
                </defs>

                {#each layout.lanes as lane, i}
                    <rect class="lane-bg" class:alt={i % 2 === 1}
                          x="0" y={lane.y} width={layout.width} height={lane.height} />
                    <text class="lane-label" x={8} y={lane.y + 14}>{lane.label.toUpperCase()}</text>
                {/each}

                {#each layout.edges as edge}
                    {@const dx = (edge.x2 - edge.x1) / 2}
                    <path
                        class="edge"
                        class:cross={edge.cross}
                        d={`M ${edge.x1} ${edge.y1} C ${edge.x1 + dx} ${edge.y1}, ${edge.x2 - dx} ${edge.y2}, ${edge.x2} ${edge.y2}`}
                        marker-end={edge.cross ? 'url(#sw-arrow-cross)' : 'url(#sw-arrow)'}
                    />
                {/each}

                {#each layout.lanes as lane}
                    {#each lane.nodes as p}
                        {@const stroke = p.node.project ? projectColor(p.node.project) : '#475569'}
                        <g class="node" transform={`translate(${p.x},${p.y})`}
                           on:click|stopPropagation={() => toggleSelection(p.node.id)}>
                            <rect width={NODE_W} height={NODE_H} rx="5"
                                  fill={nodeFill(p.node)} stroke={stroke} stroke-width="1.5"
                                  opacity={isCompleted(p.node) ? 0.5 : 1} />
                            {#if p.node.criticality > 0.5}
                                <circle cx={NODE_W - 8} cy="8" r="3" fill="#f59e0b" />
                            {/if}
                            <text class="node-text" x="8" y="14" fill={p.node.textColor}>
                                {p.node.label.length > 22 ? p.node.label.slice(0, 21) + '…' : p.node.label}
                            </text>
                            <text class="node-meta" x="8" y="28" fill={p.node.textColor} opacity="0.6">
                                {p.node.type} · P{p.node.priority ?? '?'} · {p.node.status}
                            </text>
                        </g>
                    {/each}
                {/each}

                {#if layout.target}
                    <g class="node target-node" transform={`translate(${layout.target.x},${layout.target.y})`}
                       on:click|stopPropagation={() => toggleSelection(layout.target.node.id)}>
                        <rect width={NODE_W} height={NODE_H} rx="5"
                              fill="#fef3c7" stroke="#f59e0b" stroke-width="3" />
                        <text class="node-text" x="8" y="14" fill="#78350f" font-weight="700">
                            ◎ {layout.target.node.label.length > 20 ? layout.target.node.label.slice(0, 19) + '…' : layout.target.node.label}
                        </text>
                        <text class="node-meta" x="8" y="28" fill="#92400e">
                            TARGET · {layout.target.node.type}
                        </text>
                    </g>
                {/if}
            </svg>
        </div>
        {/if}
    {/if}
</div>

<style>
    .swimlane-root {
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
        flex-shrink: 0;
    }
    .meta {
        opacity: 0.6;
        margin-left: 8px;
        font-size: 10px;
    }
    .canvas-wrap {
        flex: 1;
        overflow: auto;
        padding: 10px;
    }
    .canvas {
        display: block;
    }
    .lane-bg {
        fill: color-mix(in srgb, var(--color-primary) 4%, transparent);
        stroke: color-mix(in srgb, var(--color-primary) 10%, transparent);
        stroke-dasharray: 3,4;
    }
    .lane-bg.alt {
        fill: color-mix(in srgb, var(--color-primary) 7%, transparent);
    }
    .lane-label {
        font-size: 10px;
        font-weight: 700;
        letter-spacing: 0.16em;
        fill: color-mix(in srgb, var(--color-primary) 50%, transparent);
    }
    .edge {
        fill: none;
        stroke: #94a3b8;
        stroke-width: 1.4;
        opacity: 0.55;
    }
    .edge.cross {
        stroke: #f59e0b;
        stroke-width: 1.8;
        opacity: 0.85;
        stroke-dasharray: 5,3;
    }
    .node {
        cursor: pointer;
    }
    .node:hover rect {
        filter: brightness(1.2);
    }
    .node-text {
        font-size: 11px;
        font-weight: 600;
    }
    .node-meta {
        font-size: 8.5px;
        text-transform: uppercase;
        letter-spacing: 0.06em;
    }
    .empty {
        margin: auto;
        font-size: 12px;
        opacity: 0.6;
    }
</style>
