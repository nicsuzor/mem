<script lang="ts">
    import { graphData } from '../../stores/graph';
    import { selection, toggleSelection } from '../../stores/selection';
    import { projectColor } from '../../data/projectUtils';
    import {
        extractMultiTargetSubgraph,
        multiAsExtracted,
        pickAllTargets,
        computeDependencyDepth,
        isCompleted,
    } from '../../data/subgraphExtraction';
    import type { GraphNode, GraphEdge } from '../../data/prepareGraphData';

    const NODE_W = 160;
    const NODE_H = 38;
    const COL_GAP = 24;
    const ROW_GAP = 10;
    const LANE_PAD = 10;
    const LANE_HEADER = 28;

    $: targets = pickAllTargets($graphData);
    $: focusOverride = $selection.focusNodeId;
    $: targetIds = focusOverride ? [focusOverride] : targets.map(t => t.id);
    $: multi = $graphData && targetIds.length > 0
        ? extractMultiTargetSubgraph($graphData, targetIds)
        : null;
    $: extracted = multi ? multiAsExtracted(multi) : null;
    $: depth = extracted ? computeDependencyDepth(extracted) : new Map<string, number>();

    interface Placed {
        node: GraphNode;
        x: number; y: number;
        lane: number; col: number;
    }
    interface LaneLayout {
        target: GraphNode;
        y: number;
        height: number;
        nodes: Placed[];
        sharedNodes: Placed[];
        sharedRefs: Map<string, Set<string>>; // sid -> set of other lane targetIds it appears in
    }
    interface Layout {
        lanes: LaneLayout[];
        edges: { x1: number; y1: number; x2: number; y2: number; cross: boolean; type: string }[];
        width: number; height: number;
        sharedBridges: { x1: number; y1: number; x2: number; y2: number }[];
    }

    function buildLayout(
        targets: GraphNode[],
        nodes: GraphNode[],
        edges: GraphEdge[],
        routes: Map<string, Set<string>>,
        depth: Map<string, number>,
    ): Layout {
        // Per-lane: assign each lane the prereq nodes that route to its target.
        // A node belongs to multiple lanes if it serves multiple targets — we
        // place it in the lane of its **lowest-priority** target (the most
        // urgent), and stub it in the others as a "shared" hint, so the user
        // can see the cross-cutting dependency without us inventing edges.
        const targetById = new Map(targets.map(t => [t.id, t]));

        const primaryLaneFor = new Map<string, string>(); // nodeId -> targetId
        for (const [nid, ts] of routes) {
            if (targetById.has(nid)) continue;
            // Pick the highest-priority (lowest p number) target to "own" the node.
            let best: GraphNode | null = null;
            for (const tid of ts) {
                const t = targetById.get(tid);
                if (!t) continue;
                if (!best || (t.priority ?? 4) < (best.priority ?? 4)) best = t;
            }
            if (best) primaryLaneFor.set(nid, best.id);
        }

        // Compute per-lane column for each node, normalised so each lane
        // starts at column 0 (its leaves) and ends at the lane's target.
        const placedById = new Map<string, Placed>();
        let cursorY = LANE_PAD;
        const lanes: LaneLayout[] = [];

        // Determine global max column (so all targets sit in the same right column)
        let globalMaxCol = 0;
        const laneNodes = new Map<string, GraphNode[]>();
        for (const t of targets) laneNodes.set(t.id, []);
        for (const n of nodes) {
            if (targetById.has(n.id)) continue;
            const tid = primaryLaneFor.get(n.id);
            if (tid) laneNodes.get(tid)?.push(n);
        }
        const laneMinDepth = new Map<string, number>();
        for (const [tid, ns] of laneNodes) {
            const minD = ns.length ? Math.min(...ns.map(n => depth.get(n.id) ?? 0)) : 0;
            laneMinDepth.set(tid, minD);
            const maxRel = ns.length
                ? Math.max(...ns.map(n => (depth.get(n.id) ?? 0) - minD))
                : 0;
            if (maxRel > globalMaxCol) globalMaxCol = maxRel;
        }
        const targetCol = globalMaxCol + 1;

        for (const t of targets) {
            const ns = (laneNodes.get(t.id) || []).slice();
            const minD = laneMinDepth.get(t.id) ?? 0;
            const colsByNode = new Map<number, GraphNode[]>();
            for (const n of ns) {
                const c = (depth.get(n.id) ?? 0) - minD;
                const arr = colsByNode.get(c) || [];
                arr.push(n);
                colsByNode.set(c, arr);
            }
            for (const arr of colsByNode.values()) {
                arr.sort((a, b) => (b.criticality - a.criticality) || a.label.localeCompare(b.label));
            }
            const rowsInLane = Math.max(1, ...[...colsByNode.values()].map(a => a.length));
            const laneHeight = LANE_HEADER + rowsInLane * (NODE_H + ROW_GAP) + LANE_PAD + 4;

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
            // Place the lane's target node at the right column
            const targetX = LANE_PAD + targetCol * (NODE_W + COL_GAP);
            const targetY = cursorY + LANE_HEADER + Math.max(0, (rowsInLane - 1) / 2) * (NODE_H + ROW_GAP);
            const tp: Placed = { node: t, x: targetX, y: targetY, lane: lanes.length, col: targetCol };
            placedById.set(t.id, tp);

            const sharedRefs = new Map<string, Set<string>>();
            const sharedNodes: Placed[] = [];
            for (const n of ns) {
                const r = routes.get(n.id);
                if (!r || r.size <= 1) continue;
                sharedRefs.set(n.id, new Set([...r].filter(x => x !== t.id)));
            }

            lanes.push({
                target: t,
                y: cursorY,
                height: laneHeight,
                nodes: [...placed, tp],
                sharedNodes,
                sharedRefs,
            });
            cursorY += laneHeight;
        }

        const drawEdges: Layout['edges'] = [];
        for (const e of edges) {
            const sid = typeof e.source === 'object' ? e.source.id : e.source;
            const tid = typeof e.target === 'object' ? e.target.id : e.target;
            const a = placedById.get(sid);
            const b = placedById.get(tid);
            if (!a || !b) continue;
            // Only draw edges within the same lane to keep visual clarity;
            // cross-lane edges are surfaced via the "+N targets" badges
            // instead of spaghetti lines.
            const cross = a.lane !== b.lane;
            if (cross) continue;
            drawEdges.push({
                x1: a.x + NODE_W,
                y1: a.y + NODE_H / 2,
                x2: b.x,
                y2: b.y + NODE_H / 2,
                cross,
                type: e.type,
            });
        }

        const totalHeight = cursorY + LANE_PAD;
        const totalWidth = LANE_PAD * 2 + (targetCol + 1) * (NODE_W + COL_GAP);
        return { lanes, edges: drawEdges, width: totalWidth, height: totalHeight, sharedBridges: [] };
    }

    $: layout = multi ? buildLayout(multi.targets, multi.nodes, multi.edges, multi.routes, depth) : null;

    function nodeFill(n: GraphNode) {
        if (isCompleted(n)) return '#1e293b';
        if (n.status === 'in_progress') return '#1e40af';
        if (n.status === 'blocked') return '#7f1d1d';
        return n.fill;
    }
    function isTarget(id: string) {
        return multi ? multi.targets.some(t => t.id === id) : false;
    }
    function shareCount(id: string): number {
        const r = multi?.routes.get(id);
        return r ? r.size : 0;
    }
</script>

<div class="swimlane-root" data-component="swimlane-view">
    {#if !$graphData || !multi}
        <div class="empty">Loading…</div>
    {:else if multi.targets.length === 0}
        <div class="empty">No active targets found.</div>
    {:else if layout}
        <div class="caption">
            <strong>Swimlane DAG</strong>
            <span class="meta">· {multi.targets.length} target lanes · {multi.nodes.length - multi.targets.length} prerequisite tasks · ●n on a card = it serves N targets</span>
        </div>
        <div class="canvas-wrap">
            <svg width={layout.width} height={layout.height} class="canvas">
                <defs>
                    <marker id="sw-arrow" markerWidth="9" markerHeight="9" refX="8" refY="4.5" orient="auto">
                        <path d="M0,0 L9,4.5 L0,9 z" fill="#94a3b8" />
                    </marker>
                    <marker id="sw-arrow-parent" markerWidth="9" markerHeight="9" refX="8" refY="4.5" orient="auto">
                        <path d="M0,0 L9,4.5 L0,9 z" fill="#facc15" />
                    </marker>
                </defs>

                {#each layout.lanes as lane, i}
                    {@const projColor = lane.target.project ? projectColor(lane.target.project) : '#f59e0b'}
                    <rect class="lane-bg" class:alt={i % 2 === 1}
                          x="0" y={lane.y} width={layout.width} height={lane.height} />
                    <rect x="0" y={lane.y} width="6" height={lane.height} fill={projColor} opacity="0.7" />
                    <text class="lane-label" x={14} y={lane.y + 16}>
                        {lane.target.label.toUpperCase()}
                    </text>
                    <text class="lane-sub" x={14} y={lane.y + 28}>
                        {lane.nodes.length - 1} prereqs · P{lane.target.priority ?? '?'} · {lane.target.status}
                    </text>
                {/each}

                {#each layout.edges as edge}
                    {@const dx = (edge.x2 - edge.x1) / 2}
                    <path
                        class="edge"
                        class:parent={edge.type === 'parent'}
                        d={`M ${edge.x1} ${edge.y1} C ${edge.x1 + dx} ${edge.y1}, ${edge.x2 - dx} ${edge.y2}, ${edge.x2} ${edge.y2}`}
                        marker-end={edge.type === 'parent' ? 'url(#sw-arrow-parent)' : 'url(#sw-arrow)'}
                    />
                {/each}

                {#each layout.lanes as lane}
                    {#each lane.nodes as p}
                        {@const target = isTarget(p.node.id)}
                        {@const stroke = target ? '#f59e0b' : (p.node.project ? projectColor(p.node.project) : '#475569')}
                        {@const shared = shareCount(p.node.id)}
                        <g class="node" class:target={target}
                           transform={`translate(${p.x},${p.y})`}
                           onclick={(e) => { e.stopPropagation(); toggleSelection(p.node.id); }}>
                            <rect width={NODE_W} height={NODE_H} rx="5"
                                  fill={target ? '#fef3c7' : nodeFill(p.node)}
                                  stroke={stroke} stroke-width={target ? 2.5 : 1.5}
                                  opacity={isCompleted(p.node) ? 0.5 : 1} />
                            {#if !target && shared > 1}
                                <g transform={`translate(${NODE_W - 24}, 4)`}>
                                    <rect width="20" height="14" rx="7" fill="#1e3a8a" stroke="#3b82f6" stroke-width="0.8" />
                                    <text x="10" y="11" text-anchor="middle" fill="#dbeafe" font-size="9" font-weight="700">●{shared}</text>
                                </g>
                            {/if}
                            {#if !target && p.node.criticality > 0.5}
                                <circle cx={6} cy={6} r="3" fill="#f59e0b" />
                            {/if}
                            <text class="node-text" x="8" y="15"
                                  fill={target ? '#78350f' : p.node.textColor}
                                  font-weight={target ? '700' : '600'}>
                                {target ? '◎ ' : ''}{p.node.label.length > 22 ? p.node.label.slice(0, 21) + '…' : p.node.label}
                            </text>
                            <text class="node-meta" x="8" y="29"
                                  fill={target ? '#92400e' : p.node.textColor}
                                  opacity={target ? 0.85 : 0.6}>
                                {p.node.type} · P{p.node.priority ?? '?'} · {p.node.status}
                            </text>
                        </g>
                    {/each}
                {/each}
            </svg>
        </div>
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
    .meta { opacity: 0.6; margin-left: 6px; }
    .canvas-wrap {
        flex: 1;
        overflow: auto;
        padding: 10px;
    }
    .canvas { display: block; }
    .lane-bg {
        fill: color-mix(in srgb, var(--color-primary) 4%, transparent);
        stroke: color-mix(in srgb, var(--color-primary) 10%, transparent);
        stroke-dasharray: 3,4;
    }
    .lane-bg.alt {
        fill: color-mix(in srgb, var(--color-primary) 7%, transparent);
    }
    .lane-label {
        font-size: 11px;
        font-weight: 700;
        letter-spacing: 0.12em;
        fill: color-mix(in srgb, var(--color-primary) 80%, transparent);
    }
    .lane-sub {
        font-size: 9px;
        fill: color-mix(in srgb, var(--color-primary) 50%, transparent);
    }
    .edge {
        fill: none;
        stroke: #94a3b8;
        stroke-width: 1.4;
        opacity: 0.55;
    }
    .edge.parent {
        stroke: #facc15;
        opacity: 0.45;
        stroke-dasharray: 1,4;
    }
    .node { cursor: pointer; }
    .node:hover rect { filter: brightness(1.2); }
    .node-text { font-size: 11px; font-weight: 600; }
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
