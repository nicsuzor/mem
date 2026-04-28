<script lang="ts">
    import * as d3 from 'd3';
    import { graphData } from '../../stores/graph';
    import { selection, toggleSelection } from '../../stores/selection';
    import { projectColor } from '../../data/projectUtils';
    import {
        extractSubgraph,
        pickDefaultTarget,
        isCompleted,
    } from '../../data/subgraphExtraction';
    import type { GraphNode, GraphEdge } from '../../data/prepareGraphData';

    interface TreeDatum {
        id: string;
        node: GraphNode;
        children?: TreeDatum[];
        _children?: TreeDatum[]; // collapsed
        depth?: number;
    }

    $: targetId = $selection.focusNodeId || pickDefaultTarget($graphData);
    $: subgraph = $graphData && targetId ? extractSubgraph($graphData, targetId) : null;
    $: target = subgraph ? subgraph.nodes.find(n => n.id === subgraph.targetId) : null;

    function endpointId(e: string | GraphNode): string {
        return typeof e === 'object' ? e.id : e;
    }

    /**
     * Build a *tree* projection of the prerequisite DAG, rooted at target.
     * Each prerequisite gets attached at its FIRST encounter (BFS), so
     * shared prerequisites appear once near the top — preserving correctness
     * (we never invent edges) at the cost of hiding redundant attachments.
     */
    function buildTree(sub: ReturnType<typeof extractSubgraph>): TreeDatum {
        const targetNode = sub.nodes.find(n => n.id === sub.targetId)!;
        const nodeById = new Map(sub.nodes.map(n => [n.id, n]));
        const prereqsOf = new Map<string, string[]>();
        for (const n of sub.nodes) prereqsOf.set(n.id, []);
        for (const e of sub.edges) {
            const sid = endpointId(e.source);
            const tid = endpointId(e.target);
            if (e.type === 'depends_on' || e.type === 'soft_depends_on') {
                prereqsOf.get(sid)?.push(tid);
            } else if (e.type === 'parent') {
                // source = parent, target = child; child is prereq for parent
                prereqsOf.get(sid)?.push(tid);
            }
        }

        const placed = new Set<string>([sub.targetId]);
        const root: TreeDatum = { id: sub.targetId, node: targetNode, children: [] };
        const queue: TreeDatum[] = [root];

        while (queue.length > 0) {
            const cur = queue.shift()!;
            const prereqs = prereqsOf.get(cur.id) || [];
            // Stable order: incomplete first, then by criticality desc
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
                const child: TreeDatum = { id: pid, node: pnode, children: [] };
                cur.children!.push(child);
                queue.push(child);
            }
        }
        return root;
    }

    $: treeData = subgraph ? buildTree(subgraph) : null;

    // SVG container
    let svgEl: SVGSVGElement;
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
    }
    interface PositionedLink {
        x1: number; y1: number; x2: number; y2: number;
    }

    interface Render {
        nodes: PositionedNode[];
        links: PositionedLink[];
        width: number;
        height: number;
    }

    function visibleHierarchy(root: TreeDatum, collapsed: Set<string>): TreeDatum {
        function walk(d: TreeDatum): TreeDatum {
            if (collapsed.has(d.id)) return { ...d, children: [] };
            return { ...d, children: (d.children || []).map(walk) };
        }
        return walk(root);
    }

    function buildRender(root: TreeDatum, layout: 'vertical' | 'radial'): Render {
        const visible = visibleHierarchy(root, collapsed);
        const h = d3.hierarchy<TreeDatum>(visible);
        const total = h.descendants().length;

        if (layout === 'radial') {
            const r = Math.min(viewW, viewH) / 2 - 80;
            const tree = d3.tree<TreeDatum>().size([2 * Math.PI, r])
                .separation((a, b) => (a.parent === b.parent ? 1 : 1.6) / Math.max(1, a.depth));
            tree(h);
            const cx = viewW / 2, cy = viewH / 2;
            const nodes: PositionedNode[] = h.descendants().map(n => {
                const angle = (n as any).x;
                const radius = (n as any).y;
                return {
                    d: n.data,
                    x: cx + radius * Math.cos(angle - Math.PI / 2),
                    y: cy + radius * Math.sin(angle - Math.PI / 2),
                    depth: n.depth,
                    collapsed: collapsed.has(n.data.id),
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
            const dx = 28; // vertical spacing per leaf
            const dy = Math.max(180, viewW / Math.max(2, h.height + 1));
            const tree = d3.tree<TreeDatum>().nodeSize([dx, dy]);
            tree(h);
            let minX = Infinity, maxX = -Infinity;
            h.each(n => {
                const x = (n as any).x;
                if (x < minX) minX = x;
                if (x > maxX) maxX = x;
            });
            const offsetX = -minX + 30;
            const nodes: PositionedNode[] = h.descendants().map(n => ({
                d: n.data,
                x: (n as any).y + 30,
                y: (n as any).x + offsetX,
                depth: n.depth,
                collapsed: collapsed.has(n.data.id),
            }));
            const links: PositionedLink[] = h.links().map(l => ({
                x1: (l.source as any).y + 30,
                y1: (l.source as any).x + offsetX,
                x2: (l.target as any).y + 30,
                y2: (l.target as any).x + offsetX,
            }));
            const totalH = (maxX - minX) + 60;
            const totalW = (h.height + 1) * dy + 200;
            return { nodes, links, width: totalW, height: Math.max(viewH, totalH) };
        }
    }

    $: render = treeData ? buildRender(treeData, layoutMode) : null;

    function nodeColor(n: GraphNode): string {
        if (isCompleted(n)) return '#1e293b';
        if (n.status === 'in_progress') return '#1e40af';
        if (n.status === 'blocked') return '#7f1d1d';
        return n.fill;
    }
    function badge(d: TreeDatum) {
        const total = countDescendants(d);
        return total - 1; // exclude self
    }
    function countDescendants(d: TreeDatum): number {
        let c = 1;
        for (const k of d.children || []) c += countDescendants(k);
        return c;
    }
</script>

<div class="hta-root" data-component="hta-tree-view">
    {#if !$graphData || !subgraph || !target || !render || !treeData}
        <div class="empty">Loading…</div>
    {:else}
        <div class="caption">
            <strong>Hierarchical Task Analysis</strong>
            <span class="meta">· {subgraph.nodes.length} nodes · click chevrons to collapse · target = {target.label}</span>
            <div class="controls">
                <button class:active={layoutMode === 'vertical'} on:click={() => layoutMode = 'vertical'}>Vertical</button>
                <button class:active={layoutMode === 'radial'} on:click={() => layoutMode = 'radial'}>Radial</button>
                <button on:click={() => collapsed = new Set()}>Expand all</button>
                <button on:click={() => {
                    const next = new Set<string>();
                    function walk(d: TreeDatum, depth: number) {
                        if (depth >= 1 && (d.children?.length || 0) > 0) next.add(d.id);
                        for (const k of d.children || []) walk(k, depth + 1);
                    }
                    if (treeData) walk(treeData, 0);
                    collapsed = next;
                }}>Collapse to L1</button>
            </div>
        </div>
        <div class="canvas-wrap">
            <svg bind:this={svgEl} width={render.width} height={render.height}>
                <defs>
                    <marker id="hta-arrow" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto">
                        <path d="M0,0 L8,4 L0,8 z" fill="#64748b" />
                    </marker>
                </defs>
                {#each render.links as l}
                    <path d={`M ${l.x1} ${l.y1} C ${(l.x1 + l.x2) / 2} ${l.y1}, ${(l.x1 + l.x2) / 2} ${l.y2}, ${l.x2} ${l.y2}`}
                          fill="none" stroke="#475569" stroke-width="1.2" opacity="0.6" />
                {/each}

                {#each render.nodes as p}
                    {@const n = p.d.node}
                    {@const isTarget = n.id === target.id}
                    {@const stroke = isTarget ? '#f59e0b' : (n.project ? projectColor(n.project) : '#475569')}
                    {@const hasChildren = (p.d.children?.length || 0) > 0 || p.collapsed}
                    {@const r = isTarget ? 14 : 10}
                    <g class="hta-node" transform={`translate(${p.x},${p.y})`}>
                        <circle r={r} fill={isTarget ? '#fef3c7' : nodeColor(n)}
                                stroke={stroke} stroke-width={isTarget ? 3 : 1.5}
                                opacity={isCompleted(n) ? 0.5 : 1}
                                on:click|stopPropagation={() => toggleSelection(n.id)} />
                        {#if n.criticality > 0.5}
                            <circle r={r + 3} fill="none" stroke="#f59e0b" stroke-width="1.5" stroke-dasharray="2,2" opacity="0.7" />
                        {/if}
                        {#if hasChildren}
                            <g class="collapse-toggle" transform={`translate(${r + 4}, ${-r - 2})`}
                               on:click|stopPropagation={() => toggleCollapsed(p.d.id)}>
                                <circle r="7" fill="#0f172a" stroke="#64748b" stroke-width="1" />
                                <text class="toggle-icon" x="0" y="3">{p.collapsed ? '+' : '−'}</text>
                            </g>
                            {#if p.collapsed}
                                <text class="badge-count" x={r + 16} y={4}>+{badge(p.d)} hidden</text>
                            {/if}
                        {/if}
                        <text class="hta-label"
                              x={layoutMode === 'radial' ? 0 : r + 8}
                              y={layoutMode === 'radial' ? r + 14 : 4}
                              fill={isCompleted(n) ? '#64748b' : '#cbd5e1'}
                              text-anchor={layoutMode === 'radial' ? 'middle' : 'start'}
                              opacity={isCompleted(n) ? 0.5 : 1}>
                            {n.label.length > 30 ? n.label.slice(0, 29) + '…' : n.label}
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
