<script lang="ts">
    import * as d3 from 'd3';
    import { graphData } from '../../stores/graph';
    import { selection, toggleSelection } from '../../stores/selection';
    import { projectColor } from '../../data/projectUtils';
    import {
        extractMultiTargetSubgraph,
        pickAllTargets,
        isCompleted,
    } from '../../data/subgraphExtraction';
    import type { GraphNode } from '../../data/prepareGraphData';

    interface TreeDatum {
        id: string;
        label: string;
        node: GraphNode | null;
        children?: TreeDatum[];
    }

    $: targets = pickAllTargets($graphData);
    $: focusOverride = $selection.focusNodeId;
    $: targetIds = focusOverride ? [focusOverride] : targets.map(t => t.id);
    $: multi = $graphData && targetIds.length > 0
        ? extractMultiTargetSubgraph($graphData, targetIds)
        : null;

    function endpointId(e: string | GraphNode): string {
        return typeof e === 'object' ? e.id : e;
    }

    function buildTree(multi: NonNullable<ReturnType<typeof extractMultiTargetSubgraph>>): TreeDatum {
        const nodeById = new Map(multi.nodes.map(n => [n.id, n]));
        const prereqsOf = new Map<string, string[]>();
        for (const n of multi.nodes) prereqsOf.set(n.id, []);
        for (const e of multi.edges) {
            const sid = endpointId(e.source);
            const tid = endpointId(e.target);
            if (e.type === 'depends_on' || e.type === 'soft_depends_on' || e.type === 'parent') {
                prereqsOf.get(sid)?.push(tid);
            }
        }

        const root: TreeDatum = {
            id: '__root__',
            label: `${multi.targets.length} active targets`,
            node: null,
            children: [],
        };
        const placed = new Set<string>(['__root__']);
        const queue: TreeDatum[] = [];

        const orderedTargets = [...multi.targets].sort((a, b) =>
            (a.priority ?? 4) - (b.priority ?? 4) || a.label.localeCompare(b.label));
        for (const t of orderedTargets) {
            const td: TreeDatum = { id: t.id, label: t.label, node: t, children: [] };
            root.children!.push(td);
            placed.add(t.id);
            queue.push(td);
        }

        while (queue.length > 0) {
            const cur = queue.shift()!;
            const prereqs = prereqsOf.get(cur.id) || [];
            prereqs.sort((a, b) => {
                const na = nodeById.get(a)!;
                const nb = nodeById.get(b)!;
                const ca = isCompleted(na) ? 1 : 0;
                const cb = isCompleted(nb) ? 1 : 0;
                if (ca !== cb) return ca - cb;
                return (nb.criticality || 0) - (na.criticality || 0);
            });
            for (const pid of prereqs) {
                if (placed.has(pid)) continue;
                placed.add(pid);
                const pnode = nodeById.get(pid);
                if (!pnode) continue;
                const child: TreeDatum = { id: pid, label: pnode.label, node: pnode, children: [] };
                cur.children!.push(child);
                queue.push(child);
            }
        }
        return root;
    }

    $: treeData = multi ? buildTree(multi) : null;

    let collapsed = new Set<string>();
    let layoutMode: 'vertical' | 'radial' = 'vertical';
    let viewW = 1200;
    let viewH = 800;

    function toggleCollapsed(id: string) {
        const next = new Set(collapsed);
        if (next.has(id)) next.delete(id);
        else next.add(id);
        collapsed = next;
    }

    interface PositionedNode {
        d: TreeDatum;
        x: number; y: number;
        depth: number;
        collapsed: boolean;
        hidden: number;
    }
    interface PositionedLink { x1: number; y1: number; x2: number; y2: number; }
    interface Render { nodes: PositionedNode[]; links: PositionedLink[]; width: number; height: number; }

    function visibleHierarchy(root: TreeDatum, collapsed: Set<string>): TreeDatum {
        function walk(d: TreeDatum): TreeDatum {
            if (collapsed.has(d.id)) return { ...d, children: [] };
            return { ...d, children: (d.children || []).map(walk) };
        }
        return walk(root);
    }
    function countDescendants(d: TreeDatum): number {
        let c = 0;
        for (const k of d.children || []) c += 1 + countDescendants(k);
        return c;
    }

    function buildRender(root: TreeDatum, layout: 'vertical' | 'radial'): Render {
        const visible = visibleHierarchy(root, collapsed);
        const h = d3.hierarchy<TreeDatum>(visible);
        const idIndex = new Map<string, TreeDatum>();
        function indexAll(d: TreeDatum) { idIndex.set(d.id, d); for (const k of d.children || []) indexAll(k); }
        indexAll(root);

        if (layout === 'radial') {
            const r = Math.min(viewW, viewH) / 2 - 90;
            const tree = d3.tree<TreeDatum>().size([2 * Math.PI, r])
                .separation((a, b) => (a.parent === b.parent ? 1 : 1.6) / Math.max(1, a.depth));
            tree(h);
            const cx = viewW / 2, cy = viewH / 2;
            const nodes: PositionedNode[] = h.descendants().map(n => {
                const angle = (n as any).x;
                const radius = (n as any).y;
                const original = idIndex.get(n.data.id) || n.data;
                return {
                    d: n.data,
                    x: cx + radius * Math.cos(angle - Math.PI / 2),
                    y: cy + radius * Math.sin(angle - Math.PI / 2),
                    depth: n.depth,
                    collapsed: collapsed.has(n.data.id),
                    hidden: collapsed.has(n.data.id) ? countDescendants(original) : 0,
                };
            });
            const links: PositionedLink[] = h.links().map(l => {
                const sa = (l.source as any).x; const sr = (l.source as any).y;
                const ta = (l.target as any).x; const tr = (l.target as any).y;
                return {
                    x1: cx + sr * Math.cos(sa - Math.PI / 2),
                    y1: cy + sr * Math.sin(sa - Math.PI / 2),
                    x2: cx + tr * Math.cos(ta - Math.PI / 2),
                    y2: cy + tr * Math.sin(ta - Math.PI / 2),
                };
            });
            return { nodes, links, width: viewW, height: viewH };
        } else {
            const dx = 26;
            const dy = Math.max(220, viewW / Math.max(2, h.height + 1));
            const tree = d3.tree<TreeDatum>().nodeSize([dx, dy]);
            tree(h);
            let minX = Infinity, maxX = -Infinity;
            h.each(n => {
                const x = (n as any).x;
                if (x < minX) minX = x;
                if (x > maxX) maxX = x;
            });
            const offsetX = -minX + 30;
            const nodes: PositionedNode[] = h.descendants().map(n => {
                const original = idIndex.get(n.data.id) || n.data;
                return {
                    d: n.data,
                    x: (n as any).y + 30,
                    y: (n as any).x + offsetX,
                    depth: n.depth,
                    collapsed: collapsed.has(n.data.id),
                    hidden: collapsed.has(n.data.id) ? countDescendants(original) : 0,
                };
            });
            const links: PositionedLink[] = h.links().map(l => ({
                x1: (l.source as any).y + 30,
                y1: (l.source as any).x + offsetX,
                x2: (l.target as any).y + 30,
                y2: (l.target as any).x + offsetX,
            }));
            const totalH = (maxX - minX) + 80;
            const totalW = (h.height + 1) * dy + 240;
            return { nodes, links, width: totalW, height: Math.max(viewH, totalH) };
        }
    }

    $: render = treeData ? buildRender(treeData, layoutMode) : null;

    function nodeColor(n: GraphNode | null): string {
        if (!n) return '#0f172a';
        if (isCompleted(n)) return '#1e293b';
        if (n.status === 'in_progress') return '#1e40af';
        if (n.status === 'blocked') return '#7f1d1d';
        return n.fill;
    }
    function isTarget(d: TreeDatum) {
        return !!multi && multi.targets.some(t => t.id === d.id);
    }
    function isRoot(d: TreeDatum) { return d.id === '__root__'; }
</script>

<div class="hta-root" data-component="hta-tree-view">
    {#if !$graphData || !multi || !render || !treeData}
        <div class="empty">Loading…</div>
    {:else if multi.targets.length === 0}
        <div class="empty">No active targets found.</div>
    {:else}
        <div class="caption">
            <strong>Hierarchical Task Analysis</strong>
            <span class="meta">
                · {multi.targets.length} targets · {multi.nodes.length - multi.targets.length} prerequisite tasks
                · click chevrons to collapse/expand
            </span>
            <div class="controls">
                <button class:active={layoutMode === 'vertical'} onclick={() => layoutMode = 'vertical'}>Vertical</button>
                <button class:active={layoutMode === 'radial'} onclick={() => layoutMode = 'radial'}>Radial</button>
                <button onclick={() => collapsed = new Set()}>Expand all</button>
                <button onclick={() => {
                    const next = new Set<string>();
                    if (multi) for (const t of multi.targets) next.add(t.id);
                    collapsed = next;
                }}>Targets only</button>
                <button onclick={() => {
                    const next = new Set<string>();
                    function walk(d: TreeDatum, depth: number) {
                        if (depth >= 2 && (d.children?.length || 0) > 0) next.add(d.id);
                        for (const k of d.children || []) walk(k, depth + 1);
                    }
                    if (treeData) walk(treeData, 0);
                    collapsed = next;
                }}>Collapse to L2</button>
            </div>
        </div>
        <div class="canvas-wrap">
            <svg width={render.width} height={render.height}>
                {#each render.links as l}
                    <path d={`M ${l.x1} ${l.y1} C ${(l.x1 + l.x2) / 2} ${l.y1}, ${(l.x1 + l.x2) / 2} ${l.y2}, ${l.x2} ${l.y2}`}
                          fill="none" stroke="#475569" stroke-width="1.2" opacity="0.55" />
                {/each}

                {#each render.nodes as p}
                    {@const root = isRoot(p.d)}
                    {@const target = isTarget(p.d)}
                    {@const stroke = root ? '#cbd5e1'
                        : target ? '#f59e0b'
                        : (p.d.node?.project ? projectColor(p.d.node.project) : '#475569')}
                    {@const hasChildren = (p.d.children?.length || 0) > 0 || p.collapsed}
                    {@const r = root ? 16 : target ? 14 : 10}
                    <g class="hta-node" transform={`translate(${p.x},${p.y})`}>
                        <circle r={r}
                                fill={target ? '#fef3c7' : root ? '#1e293b' : nodeColor(p.d.node)}
                                stroke={stroke} stroke-width={target ? 3 : root ? 2.5 : 1.5}
                                opacity={p.d.node && isCompleted(p.d.node) ? 0.5 : 1}
                                onclick={(e) => { e.stopPropagation(); if (p.d.node) toggleSelection(p.d.id); }}>
                        </circle>
                        {#if p.d.node && p.d.node.criticality > 0.5}
                            <circle r={r + 3} fill="none" stroke="#f59e0b" stroke-width="1.5" stroke-dasharray="2,2" opacity="0.7" />
                        {/if}
                        {#if hasChildren}
                            <g class="collapse-toggle" transform={`translate(${r + 4}, ${-r - 2})`}
                               onclick={(e) => { e.stopPropagation(); toggleCollapsed(p.d.id); }}>
                                <circle r="7" fill="#0f172a" stroke="#64748b" stroke-width="1" />
                                <text class="toggle-icon" x="0" y="3">{p.collapsed ? '+' : '−'}</text>
                            </g>
                            {#if p.collapsed && p.hidden > 0}
                                <text class="badge-count" x={r + 16} y={4}>+{p.hidden} hidden</text>
                            {/if}
                        {/if}
                        <text class="hta-label"
                              x={layoutMode === 'radial' ? 0 : r + 8}
                              y={layoutMode === 'radial' ? r + 14 : 4}
                              fill={target ? '#fef3c7' : (p.d.node && isCompleted(p.d.node)) ? '#64748b' : '#cbd5e1'}
                              text-anchor={layoutMode === 'radial' ? 'middle' : 'start'}
                              font-weight={target || root ? '700' : '500'}
                              opacity={p.d.node && isCompleted(p.d.node) ? 0.5 : 1}>
                            {target ? '◎ ' : ''}{p.d.label.length > 32 ? p.d.label.slice(0, 31) + '…' : p.d.label}
                        </text>
                    </g>
                {/each}
            </svg>
        </div>
    {/if}
</div>

<style>
    .hta-root {
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
        display: flex;
        align-items: center;
        gap: 12px;
        flex-wrap: wrap;
    }
    .meta { opacity: 0.6; }
    .controls { margin-left: auto; display: flex; gap: 4px; }
    .controls button {
        padding: 3px 8px;
        font-size: 10px;
        font-family: inherit;
        background: rgba(148,163,184,0.05);
        border: 1px solid rgba(148,163,184,0.2);
        color: inherit;
        border-radius: 3px;
        cursor: pointer;
    }
    .controls button.active { background: rgba(245,158,11,0.18); border-color: #f59e0b; color: #fef3c7; }
    .controls button:hover { background: rgba(148,163,184,0.15); }
    .canvas-wrap {
        flex: 1;
        overflow: auto;
        padding: 14px;
    }
    .hta-node circle:first-child { cursor: pointer; }
    .hta-node circle:first-child:hover { filter: brightness(1.3); }
    .collapse-toggle { cursor: pointer; }
    .collapse-toggle:hover circle { fill: #1e293b; stroke: #cbd5e1; }
    .toggle-icon {
        text-anchor: middle;
        font-size: 11px;
        font-weight: 700;
        fill: #cbd5e1;
        pointer-events: none;
    }
    .hta-label {
        font-size: 11px;
        pointer-events: none;
    }
    .badge-count {
        font-size: 9px;
        fill: #f59e0b;
        font-style: italic;
    }
    .empty { margin: auto; opacity: 0.6; font-size: 12px; }
</style>
