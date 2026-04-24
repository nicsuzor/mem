<script lang="ts">
    import { onMount, onDestroy } from 'svelte';
    import { preparedGraphData } from '../../stores/graph';
    import { selection, toggleSelection } from '../../stores/selection';
    import { INCOMPLETE_STATUSES } from '../../data/constants';
    import { projectColor } from '../../data/projectUtils';
    import type { GraphNode } from '../../data/prepareGraphData';

    // ── Principles (from prompt):
    //   1. Drop topology-as-layout. Straight-ish radial lines from a hub.
    //   4. Lines are timelines. Position along line = time-to-destination.
    //   6. Lines are curated. Auto-picked stops now; user pin/unpin later.
    //   Plus: no duplication, but lines may intersect at shared stops.

    let containerEl: HTMLDivElement;
    let w = 800;
    let h = 800;

    // Tunables
    const HOP_UP = 2;              // ancestor hops above a leaf target
    const HOP_DOWN = 3;            // descendant hops below a container target
    const HOP_DEPS = 4;            // depends_on closure depth
    const MAX_STOPS_PER_LINE = 7;  // cap for readability (incl. terminal)
    const MAX_DESTINATIONS = 8;    // hard cap on terminals to keep map readable
    const NEIGHBOURHOOD_FAN = 6;   // top-N from containing epic's subtree
    const R_HUB = 70;              // hub radius
    const EDGE_PAD = 170;          // padding inside viewport (room for labels)

    // Hover state
    let hoverId: string | null = null;

    // Layout output (reactive)
    interface Line {
        terminalId: string;
        angle: number;
        color: string;
        priority: number;
        label: string;
        stops: string[]; // hub-side → terminal-side
    }

    interface StopPos { x: number; y: number; }

    let lines: Line[] = [];
    let stopPos = new Map<string, StopPos>();
    let stopRoutes = new Map<string, string[]>(); // stopId -> terminalIds serving it
    let hub = { x: 0, y: 0 };

    $: prep = $preparedGraphData;
    $: if (prep) recompute(prep, w, h);

    function isContainer(n: GraphNode, nodes: GraphNode[]): boolean {
        if (['epic','goal','project'].includes((n.type||'').toLowerCase())) return true;
        return nodes.some(c => c.parent === n.id && INCOMPLETE_STATUSES.has(c.status));
    }

    function pickDestinations(nodes: GraphNode[]): GraphNode[] {
        // Prefer the curated set: nodes explicitly marked as `type: target`
        // (the user has already decided these are the destinations).
        // Fall back to P0/P1 incomplete if none exist.
        const byType = nodes.filter(n =>
            INCOMPLETE_STATUSES.has(n.status) && (n.type || '').toLowerCase() === 'target'
        );
        const pool = byType.length > 0
            ? byType
            : nodes.filter(n =>
                INCOMPLETE_STATUSES.has(n.status) && (n.priority === 0 || n.priority === 1)
            );
        const sorted = pool.sort((a, b) =>
            (a.priority ?? 4) - (b.priority ?? 4)
            || (a.project || '').localeCompare(b.project || '')
            || (a.label || '').localeCompare(b.label || '')
        );
        return sorted.slice(0, MAX_DESTINATIONS);
    }

    function recompute(prepared: any, width: number, height: number) {
        const nodes: GraphNode[] = prepared.nodes;
        const edges = prepared.links;
        const byId = new Map(nodes.map((n: GraphNode) => [n.id, n]));

        // Build parent + depends_on indices
        const parentOf = new Map<string, string | null>();
        nodes.forEach(n => parentOf.set(n.id, n.parent));
        const childrenOf = new Map<string, string[]>();
        nodes.forEach(n => {
            if (n.parent) {
                const arr = childrenOf.get(n.parent) || [];
                arr.push(n.id);
                childrenOf.set(n.parent, arr);
            }
        });
        const depsOf = new Map<string, string[]>();
        edges.forEach((e: any) => {
            if (e.type === 'depends_on' || e.type === 'soft_depends_on') {
                const s = typeof e.source === 'string' ? e.source : e.source.id;
                const t = typeof e.target === 'string' ? e.target : e.target.id;
                const arr = depsOf.get(s) || [];
                arr.push(t);
                depsOf.set(s, arr);
            }
        });

        const dests = pickDestinations(nodes);
        const N = dests.length;

        // ── Step 1: curate candidate stops per destination
        const routes = new Map<string, Set<string>>(); // nodeId -> set of destinationIds
        const addRoute = (nid: string, did: string) => {
            if (!byId.has(nid)) return;
            const n = byId.get(nid)!;
            if (!INCOMPLETE_STATUSES.has(n.status)) return;
            const s = routes.get(nid) || new Set();
            s.add(did);
            routes.set(nid, s);
        };

        for (const t of dests) {
            addRoute(t.id, t.id);

            // Ancestors (up HOP_UP)
            let cur = parentOf.get(t.id) || null;
            for (let k = 0; k < HOP_UP && cur; k++) {
                addRoute(cur, t.id);
                cur = parentOf.get(cur) || null;
            }

            // Descendants (for container targets) — BFS down HOP_DOWN
            if (isContainer(t, nodes)) {
                let frontier = [t.id];
                for (let d = 0; d < HOP_DOWN; d++) {
                    const next: string[] = [];
                    for (const id of frontier) {
                        for (const cid of childrenOf.get(id) || []) {
                            addRoute(cid, t.id);
                            next.push(cid);
                        }
                    }
                    frontier = next;
                    if (!frontier.length) break;
                }
            }

            // depends_on closure from the target
            {
                let frontier = [t.id];
                const seen = new Set<string>([t.id]);
                for (let d = 0; d < HOP_DEPS; d++) {
                    const next: string[] = [];
                    for (const id of frontier) {
                        for (const dep of depsOf.get(id) || []) {
                            if (seen.has(dep)) continue;
                            seen.add(dep);
                            addRoute(dep, t.id);
                            next.push(dep);
                        }
                    }
                    frontier = next;
                    if (!frontier.length) break;
                }
            }

            // Neighbourhood fan (NEW): for LEAF targets the line is often
            // empty because the target itself has no deps or children. Pull
            // in top-N "next actions" from the containing epic's subtree so
            // the line has substance. Cap hard to avoid sibling overflow.
            if (!isContainer(t, nodes)) {
                const parentId = t.parent;
                if (parentId) {
                    const bag: GraphNode[] = [];
                    let frontier = [parentId];
                    const seen = new Set<string>(frontier);
                    for (let d = 0; d < 2; d++) {
                        const next: string[] = [];
                        for (const id of frontier) {
                            for (const cid of childrenOf.get(id) || []) {
                                if (seen.has(cid) || cid === t.id) continue;
                                seen.add(cid);
                                const c = byId.get(cid);
                                if (c && INCOMPLETE_STATUSES.has(c.status)
                                    && !['learn', 'goal'].includes((c.type || '').toLowerCase())) {
                                    bag.push(c);
                                }
                                next.push(cid);
                            }
                        }
                        frontier = next;
                        if (!frontier.length) break;
                    }
                    // Rank by next-action-ness: status readiness, priority, recency
                    const statusOrderLocal: Record<string, number> = {
                        in_progress: 0, review: 1, ready: 2, active: 3,
                        todo: 4, inbox: 5, waiting: 6, blocked: 7, dormant: 8,
                    };
                    bag.sort((a, b) => {
                        const sa = statusOrderLocal[a.status] ?? 9;
                        const sb = statusOrderLocal[b.status] ?? 9;
                        if (sa !== sb) return sa - sb;
                        const pa = a.priority ?? 4, pb = b.priority ?? 4;
                        if (pa !== pb) return pa - pb;
                        return (b.modified ?? 0) - (a.modified ?? 0);
                    });
                    for (const c of bag.slice(0, NEIGHBOURHOOD_FAN)) {
                        addRoute(c.id, t.id);
                    }
                }
            }
        }

        // ── Step 2: assign each destination an angle, build Line skeletons
        const newLines: Line[] = dests.map((t, i) => ({
            terminalId: t.id,
            angle: -Math.PI / 2 + (2 * Math.PI * i) / Math.max(N, 1),
            color: projectColor(t.project || '') || '#888',
            priority: t.priority ?? 2,
            label: t.label || t.id,
            stops: [],
        }));

        // ── Step 3: for each line, pick its stops from the route map
        //    (A node belongs to EVERY line whose route set contains it — so a
        //    shared node legitimately appears in multiple lines' stop lists.
        //    That's what produces interchanges.)
        const statusOrder: Record<string, number> = {
            in_progress: 0, review: 1, ready: 2, active: 3, todo: 4,
            inbox: 5, waiting: 6, blocked: 7, dormant: 8,
        };

        for (const line of newLines) {
            const mine: GraphNode[] = [];
            for (const [nid, destSet] of routes) {
                if (destSet.has(line.terminalId) && nid !== line.terminalId) {
                    mine.push(byId.get(nid)!);
                }
            }
            // Timeline ordering: later in journey = closer to terminus.
            // Closer to terminus = more urgent / readier / higher priority.
            // Sort so [first...last] goes from hub-side to terminal-side.
            mine.sort((a, b) => {
                // earlier = less urgent (higher priority number, further from ready)
                const pa = a.priority ?? 4, pb = b.priority ?? 4;
                if (pa !== pb) return pb - pa; // desc
                const sa = statusOrder[a.status] ?? 9;
                const sb = statusOrder[b.status] ?? 9;
                if (sa !== sb) return sb - sa; // desc (ready→terminus end)
                return a.id.localeCompare(b.id);
            });
            // Cap; keep the MOST urgent (terminal-side tail).
            if (mine.length > MAX_STOPS_PER_LINE - 1) {
                mine.splice(0, mine.length - (MAX_STOPS_PER_LINE - 1));
            }
            line.stops = [...mine.map(s => s.id), line.terminalId];
        }

        // ── Step 4: compute positions
        //   Each stop has a preferred position along each line it's on (the
        //   ray at the stop's rank along that line). If a stop is on multiple
        //   lines, its position is the centroid of its preferred positions —
        //   which places it between the rays, making both lines bend through
        //   it (geometric interchange, no duplication).
        const cx = width / 2, cy = height / 2;
        const rTerm = Math.min(width, height) / 2 - EDGE_PAD;
        hub = { x: cx, y: cy };

        const preferred = new Map<string, { x: number; y: number }[]>();
        for (const line of newLines) {
            const K = line.stops.length;
            line.stops.forEach((sid, k) => {
                const t = K > 1 ? k / (K - 1) : 1;
                const r = R_HUB + (rTerm - R_HUB) * (0.12 + 0.88 * t);
                const x = cx + r * Math.cos(line.angle);
                const y = cy + r * Math.sin(line.angle);
                const arr = preferred.get(sid) || [];
                arr.push({ x, y });
                preferred.set(sid, arr);
            });
        }

        // Terminals must stay exactly on their ray — don't average them.
        const terminalIds = new Set(newLines.map(l => l.terminalId));
        const newPositions = new Map<string, StopPos>();
        for (const [sid, prefs] of preferred) {
            if (terminalIds.has(sid) && prefs.length) {
                // Use the one belonging to its own line (first matching angle).
                const myLine = newLines.find(l => l.terminalId === sid)!;
                const r = rTerm;
                newPositions.set(sid, {
                    x: cx + r * Math.cos(myLine.angle),
                    y: cy + r * Math.sin(myLine.angle),
                });
            } else {
                const x = prefs.reduce((s, p) => s + p.x, 0) / prefs.length;
                const y = prefs.reduce((s, p) => s + p.y, 0) / prefs.length;
                newPositions.set(sid, { x, y });
            }
        }

        // Track routes per stop for multi-colour stroke rendering at interchanges
        const sr = new Map<string, string[]>();
        for (const [nid, destSet] of routes) {
            sr.set(nid, [...destSet]);
        }

        lines = newLines;
        stopPos = newPositions;
        stopRoutes = sr;
    }

    // ── Resize observer
    let ro: ResizeObserver | null = null;
    onMount(() => {
        if (containerEl) {
            const rect = containerEl.getBoundingClientRect();
            w = Math.max(rect.width, 600);
            h = Math.max(rect.height, 600);
            ro = new ResizeObserver(entries => {
                for (const e of entries) {
                    w = Math.max(e.contentRect.width, 600);
                    h = Math.max(e.contentRect.height, 600);
                }
            });
            ro.observe(containerEl);
        }
    });
    onDestroy(() => { ro?.disconnect(); });

    // ── Helpers for rendering
    function linePath(line: Line): string {
        const parts = [`M ${hub.x.toFixed(1)} ${hub.y.toFixed(1)}`];
        for (const sid of line.stops) {
            const p = stopPos.get(sid);
            if (p) parts.push(`L ${p.x.toFixed(1)} ${p.y.toFixed(1)}`);
        }
        return parts.join(' ');
    }

    function nodeOf(id: string): GraphNode | undefined {
        return prep?.nodes.find((n: GraphNode) => n.id === id);
    }

    function stopRadius(sid: string, isTerminal: boolean): number {
        if (isTerminal) return 14;
        const routes = stopRoutes.get(sid);
        if (routes && routes.length >= 2) return 9; // interchange
        return 5;
    }

    function isInProgress(n?: GraphNode): boolean {
        return !!n && (n.status === 'in_progress' || n.status === 'active');
    }

    // ── You-are-here: in_progress tasks get a pulse ring
    $: youAreHereIds = new Set(
        (prep?.nodes || [])
            .filter((n: GraphNode) => n.status === 'in_progress')
            .map((n: GraphNode) => n.id)
    );

    // ── Highlight on selection / terminal click: dim non-route stops
    $: highlightedTerminalId = (() => {
        const sel = $selection.activeNodeId;
        if (!sel) return null;
        const asTerminal = lines.find(l => l.terminalId === sel);
        return asTerminal ? asTerminal.terminalId : null;
    })();

    function stopIsOnHighlightedLine(sid: string): boolean {
        if (!highlightedTerminalId) return true;
        const r = stopRoutes.get(sid);
        return !!r && r.includes(highlightedTerminalId);
    }
</script>

<div bind:this={containerEl} class="metro-radial-root">
    <svg viewBox="0 0 {w} {h}" preserveAspectRatio="xMidYMid meet">
        <!-- Faint spoke rails so empty space reads as radial -->
        {#each lines as line}
            <line
                x1={hub.x} y1={hub.y}
                x2={hub.x + (Math.min(w,h)/2 - EDGE_PAD) * Math.cos(line.angle)}
                y2={hub.y + (Math.min(w,h)/2 - EDGE_PAD) * Math.sin(line.angle)}
                stroke={line.color}
                stroke-width="1"
                stroke-opacity={highlightedTerminalId && highlightedTerminalId !== line.terminalId ? 0.05 : 0.15}
                stroke-dasharray="4 6"
            />
        {/each}

        <!-- Lines as thick semi-transparent strokes -->
        {#each lines as line}
            <path
                d={linePath(line)}
                stroke={line.color}
                stroke-width="10"
                fill="none"
                stroke-opacity={highlightedTerminalId && highlightedTerminalId !== line.terminalId ? 0.08 : 0.45}
                stroke-linecap="round"
                stroke-linejoin="round"
            />
        {/each}

        <!-- Hub -->
        <circle cx={hub.x} cy={hub.y} r="10" fill="#1e293b" stroke="#e2e8f0" stroke-width="2" />
        <text x={hub.x} y={hub.y - 18} text-anchor="middle" fill="#94a3b8"
              font-size="11" font-family="monospace" letter-spacing="1.5">NOW</text>

        <!-- Stops -->
        {#each [...stopPos] as [sid, p] (sid)}
            {@const n = nodeOf(sid)}
            {@const isTerm = lines.some(l => l.terminalId === sid)}
            {@const routes = stopRoutes.get(sid) || []}
            {@const dim = !stopIsOnHighlightedLine(sid)}
            {@const isYAH = youAreHereIds.has(sid)}
            <g
                on:click|stopPropagation={() => toggleSelection(sid)}
                on:mouseenter={() => (hoverId = sid)}
                on:mouseleave={() => (hoverId = null)}
                class="stop-group"
                style="cursor: pointer; opacity: {dim ? 0.15 : 1};"
            >
                {#if isYAH}
                    <circle cx={p.x} cy={p.y} r={stopRadius(sid, isTerm) + 6}
                        fill="none" stroke="#fde68a" stroke-width="2" opacity="0.8">
                        <animate attributeName="r" values="{stopRadius(sid,isTerm)+4};{stopRadius(sid,isTerm)+10};{stopRadius(sid,isTerm)+4}"
                                 dur="2s" repeatCount="indefinite" />
                        <animate attributeName="opacity" values="0.8;0.2;0.8" dur="2s" repeatCount="indefinite"/>
                    </circle>
                {/if}
                {#if routes.length >= 2 && !isTerm}
                    <!-- Interchange: white ring over inner circle, metro-map style -->
                    <circle cx={p.x} cy={p.y} r={stopRadius(sid, isTerm) + 2}
                        fill="white" stroke="#334155" stroke-width="2" />
                {/if}
                <circle
                    cx={p.x} cy={p.y}
                    r={stopRadius(sid, isTerm)}
                    fill={isTerm
                        ? (lines.find(l => l.terminalId === sid)?.color || '#888')
                        : (routes.length >= 2 ? '#1e293b' : '#64748b')}
                    stroke={isTerm ? 'white' : '#0f172a'}
                    stroke-width={isTerm ? 3 : 1}
                />
                {#if isTerm}
                    {@const line = lines.find(l => l.terminalId === sid)}
                    {@const labelX = p.x + (p.x - hub.x) * 0.04 + (Math.cos(line?.angle ?? 0) * 22)}
                    {@const labelY = p.y + (p.y - hub.y) * 0.04 + (Math.sin(line?.angle ?? 0) * 22) + 4}
                    {@const ang = line?.angle ?? 0}
                    {@const anchor = Math.cos(ang) > 0.3 ? 'start' : Math.cos(ang) < -0.3 ? 'end' : 'middle'}
                    <text
                        x={labelX} y={labelY}
                        text-anchor={anchor}
                        fill="#e2e8f0"
                        font-size="13"
                        font-weight="600"
                        style="pointer-events:none; text-shadow: 0 0 4px #0f172a;"
                    >{(n?.label || sid).slice(0, 40)}</text>
                {/if}
                {#if hoverId === sid && !isTerm}
                    <text
                        x={p.x + 10} y={p.y - 10}
                        fill="#e2e8f0"
                        font-size="11"
                        style="pointer-events:none; text-shadow: 0 0 3px #0f172a;"
                    >{(n?.label || sid).slice(0, 48)}</text>
                {/if}
            </g>
        {/each}
    </svg>

    {#if lines.length === 0}
        <div class="empty">No P0/P1 destinations to route to.</div>
    {/if}
</div>

<style>
    .metro-radial-root {
        position: relative;
        width: 100%;
        height: 100%;
        background: radial-gradient(ellipse at center, #0b1220 0%, #05070d 85%);
        overflow: hidden;
    }
    svg { width: 100%; height: 100%; display: block; }
    .empty {
        position: absolute; inset: 0;
        display: flex; align-items: center; justify-content: center;
        color: #64748b; font-family: monospace;
    }
    .stop-group:hover circle { filter: brightness(1.25); }
</style>
