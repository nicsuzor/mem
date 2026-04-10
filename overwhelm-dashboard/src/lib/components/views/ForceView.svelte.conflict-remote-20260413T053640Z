<script lang="ts">
    import * as d3 from "d3";
    import * as cola from "webcola";
    import { onDestroy } from "svelte";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { viewSettings } from "../../stores/viewSettings";
    import { selection, toggleSelection } from "../../stores/selection";
    import { buildTaskCardNode } from "../shared/NodeShapes";
    import { projectHue } from "../../data/projectUtils";
    import { writable } from "svelte/store";
    import { zoomScale } from "../../stores/zoom";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";

    const TOP_PAD = 60;
    const CANVAS_AREA = 12_000_000;

    export let containerGroup: SVGGElement;

    let nodesLayer: SVGGElement;
    let hullLayer: SVGGElement;

    let colaLayout: (cola.Layout & cola.ID3StyleLayoutAdaptor) | null = null;
    let colaGroups: any[] = [];
    let groupMembers: Map<string, string[]> = new Map(); // containerId -> member node IDs

    // Layout control state
    const tickCount = writable(0);
    const running = writable(false);
    let controlsEl: HTMLDivElement;

    function toggleRunning() {
        if (!colaLayout) return;
        if ($running) {
            colaLayout.stop();
            $running = false;
        } else {
            $running = true;
            colaLayout.resume();
        }
    }

    // Portal: mount HTML controls outside SVG so they render at screen coordinates
    function portalControls(node: HTMLDivElement) {
        const target = document.querySelector('main') || document.body;
        target.appendChild(node);
        return { destroy() { node.remove(); } };
    }

    // Full physics rebuild only when structure (node/link set) or Cola params change
    let lastStructureKey = '';
    let lastColaParams = '';
    $: {
        const sk = $graphStructureKey;
        const cp = `${$viewSettings.colaLinkLength}|${$viewSettings.colaGroupPadding}|${$viewSettings.colaAvoidOverlaps}|${$viewSettings.colaHandleDisconnected}`;
        if (
            containerGroup &&
            $graphData &&
            nodesLayer &&
            hullLayer &&
            (sk !== lastStructureKey || cp !== lastColaParams)
        ) {
            lastStructureKey = sk;
            lastColaParams = cp;
            drawForceAndStartPhysics();
        }
    }

    // Property-only updates (status, priority, etc.) — patch node visuals without restarting physics
    $: if ($graphData && nodesLayer && lastStructureKey && lastStructureKey === $graphStructureKey) {
        const activeId = $selection.activeNodeId;
        d3.select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .each(function (d) {
                const fresh = $graphData!.nodes.find(n => n.id === d.id);
                if (!fresh) return;
                if (d.status !== fresh.status || d.fill !== fresh.fill || d.opacity !== fresh.opacity) {
                    Object.assign(d, fresh);
                    const g = d3.select(this) as any;
                    g.selectAll("*").remove();
                    const isSelected = d.id === activeId;
                    buildTaskCardNode(g, d, isSelected);
                    (d as any)._lastSelected = isSelected;
                }
            });
    }

    // Simple dimming for filtered nodes + selection highlight
    $: if (nodesLayer && $graphData) {
        const activeId = $selection.activeNodeId;
        d3.select(nodesLayer).selectAll<SVGGElement, GraphNode>("g.node")
            .classed("dimmed", (d: any) => d.filter_dimmed)
            .classed("selected-node", (d: any) => d.id === activeId);
    }

    // ─── Group building ────────────────────────────────────────────────────────

    function buildColaGroups(
        activeNodes: GraphNode[],
        activeLinks: GraphEdge[],
        groupPadding: number,
    ): any[] {
        const nodeIndex = new Map(activeNodes.map((n, i) => [n.id, i]));
        const nodeById = new Map(activeNodes.map(n => [n.id, n]));

        // Build parent -> children map from parent links
        const childrenOf = new Map<string, Set<number>>();
        for (const l of activeLinks) {
            if ((l as any).type !== 'parent') continue;
            const pid = typeof l.source === 'object' ? (l.source as any).id : l.source;
            const cid = typeof l.target === 'object' ? (l.target as any).id : l.target;
            const pidx = nodeIndex.get(pid);
            const cidx = nodeIndex.get(cid);
            if (pidx === undefined || cidx === undefined) continue;
            if (!childrenOf.has(pid)) childrenOf.set(pid, new Set());
            childrenOf.get(pid)!.add(cidx);
        }

        // Create a group for every parent that has children
        const groups: any[] = [];
        const groupIndexOf = new Map<string, number>();
        const members = new Map<string, string[]>(); // containerId -> node IDs

        for (const [pid, childIdxs] of childrenOf) {
            const pidx = nodeIndex.get(pid);
            if (pidx === undefined) continue;
            if (childIdxs.size === 0) continue;

            const pNode = nodeById.get(pid);
            const label = pNode?.label || (pNode as any)?.fullTitle || pid;
            groupIndexOf.set(pid, groups.length);

            const leafIndices = [pidx, ...childIdxs];
            groups.push({
                leaves: leafIndices,
                groups: [],
                padding: groupPadding + 55,
                label,
                containerId: pid,
            });

            // Track member node IDs for position-based bounds
            members.set(pid, leafIndices.map(i => activeNodes[i].id));
        }

        // Wire nesting: if a group's parent node is itself in another group, nest it
        for (const [pid] of groupIndexOf) {
            const pNode = nodeById.get(pid);
            if (!pNode?.parent) continue;
            const parentGroupIdx = groupIndexOf.get(pNode.parent);
            if (parentGroupIdx === undefined) continue;
            const thisGroupIdx = groupIndexOf.get(pid)!;
            groups[parentGroupIdx].groups.push(groups[thisGroupIdx]);
            groups[thisGroupIdx].padding = groupPadding + 30;
        }

        groupMembers = members;
        return groups;
    }

    // ─── Drag and click ───────────────────────────────────────────────────────

    function bindDragAndClick(nEls: any) {
        nEls.style("cursor", "crosshair")
            .on("click", (e: any, d: any) => { e.stopPropagation(); toggleSelection(d.id); })
            .call(
                d3.drag<SVGGElement, GraphNode>()
                    .clickDistance(4)
                    .on("start", (_e, d: any) => { d.fixed = 1; })
                    .on("drag", (e, d: any) => {
                        d.x = e.x; d.y = e.y;
                        tickVisuals();
                    })
                    .on("end", (_e, d: any) => { d.fixed = 0; }),
            );
    }

    // ─── Group box rendering ──────────────────────────────────────────────────

    function renderGroupBoxes() {
        if (!hullLayer || !colaLayout) return;

        // Build node position lookup from actual DOM positions
        const nodePos = new Map<string, { x: number; y: number; w: number; h: number }>();
        d3.select(nodesLayer).selectAll<SVGGElement, GraphNode>("g.node")
            .each(function (d) {
                nodePos.set(d.id, { x: d.x ?? 0, y: d.y ?? 0, w: d.w ?? 0, h: d.h ?? 0 });
            });

        // Compute bounds from actual member node positions (not Cola's stale bounds)
        type GroupBounds = { x: number; y: number; w: number; h: number; label: string; containerId: string; nested: boolean };
        const groupBoundsData: GroupBounds[] = [];
        const PAD = 30;

        for (const [containerId, memberIds] of groupMembers) {
            const positions = memberIds.map(id => nodePos.get(id)).filter(Boolean) as { x: number; y: number; w: number; h: number }[];
            if (positions.length === 0) continue;

            let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
            for (const p of positions) {
                minX = Math.min(minX, p.x - PAD);
                minY = Math.min(minY, p.y - PAD);
                maxX = Math.max(maxX, p.x + p.w + PAD);
                maxY = Math.max(maxY, p.y + p.h + PAD);
            }

            const colaGroup = colaGroups.find((g: any) => g.containerId === containerId);
            const label = colaGroup?.label || containerId;
            const nested = !!colaGroup?.parent;

            groupBoundsData.push({
                x: minX, y: minY - TOP_PAD, w: maxX - minX, h: maxY - minY + TOP_PAD,
                label, containerId, nested,
            });
        }

        // Background rectangles
        d3.select(hullLayer)
            .selectAll<SVGRectElement, GroupBounds>("rect.cola-group")
            .data(groupBoundsData, (d) => d.containerId)
            .join("rect")
            .attr("class", "cola-group")
            .attr("rx", (d) => d.nested ? 6 : 10)
            .attr("ry", (d) => d.nested ? 6 : 10)
            .attr("x", (d) => d.x)
            .attr("y", (d) => d.y)
            .attr("width", (d) => d.w)
            .attr("height", (d) => d.h)
            .attr("fill", (d) => {
                const hue = projectHue(d.containerId);
                return d.nested
                    ? `hsla(${hue}, 40%, 50%, 0.05)`
                    : `hsla(${hue}, 40%, 50%, 0.08)`;
            })
            .attr("stroke", (d) => {
                const hue = projectHue(d.containerId);
                return d.nested
                    ? `hsla(${hue}, 50%, 55%, 0.25)`
                    : `hsla(${hue}, 40%, 50%, 0.3)`;
            })
            .attr("stroke-width", (d) => d.nested ? 1 : 2)
            .attr("stroke-dasharray", (d) => d.nested ? "4,2" : "6,3")
            .style("cursor", "crosshair")
            .on("click", (e: any, d) => { e.stopPropagation(); toggleSelection(d.containerId); });

        // Group labels (simple truncated text)
        d3.select(hullLayer)
            .selectAll<SVGTextElement, GroupBounds>("text.cola-group-label")
            .data(groupBoundsData, (d) => d.containerId)
            .join("text")
            .attr("class", "cola-group-label")
            .attr("x", (d) => d.x + 12)
            .attr("y", (d) => d.y + 22)
            .attr("font-size", 18)
            .attr("font-weight", 700)
            .attr("fill", (d) => {
                const hue = projectHue(d.containerId);
                return `hsla(${hue}, 60%, 80%, 0.9)`;
            })
            .style("pointer-events", "none")
            .style("text-transform", "uppercase")
            .style("letter-spacing", "0.05em")
            .text((d) => {
                const label = (d.label || '').toUpperCase();
                const maxChars = Math.max(10, Math.floor(d.w / 10));
                return label.length > maxChars ? label.slice(0, maxChars - 1) + '\u2026' : label;
            });
    }

    // ─── Tick ─────────────────────────────────────────────────────────────────

    function tickVisuals() {
        if (!colaLayout) return;
        $tickCount++;

        const scale = $zoomScale;
        d3.select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .attr("transform", (d) => `translate(${d.x ?? 0},${d.y ?? 0})`)
            .each(function (d) {
                const texts = d3.select(this).selectAll("text, tspan");
                if (texts.empty()) return;
                const isHighPri = d.priority <= 1;
                texts.attr("opacity", (scale > 0.4 || (isHighPri && scale > 0.2)) ? null : 0);
            });

        renderGroupBoxes();
    }

    // ─── Main draw ────────────────────────────────────────────────────────────

    function drawForceAndStartPhysics() {
        if (!$graphData) return;
        if (colaLayout) { colaLayout.stop(); colaLayout = null; }
        $tickCount = 0;
        $running = false;

        const data = $graphData;

        // Strip project nodes — epic group boxes handle visual hierarchy
        const projectIds = new Set(data.nodes.filter(n => n.type === 'project').map(n => n.id));
        let activeNodes: GraphNode[] = data.nodes;
        let activeLinks: GraphEdge[] = data.links;
        if (projectIds.size > 0) {
            const projParentMap = new Map<string, string | null>(
                data.nodes.filter(n => projectIds.has(n.id)).map(n => [n.id, n.parent])
            );
            activeNodes = data.nodes.map(n => {
                if (projectIds.has(n.id)) return n;
                let cur = n.parent;
                const seen = new Set<string>();
                while (cur && projectIds.has(cur)) {
                    if (seen.has(cur)) break;
                    seen.add(cur);
                    cur = projParentMap.get(cur) ?? null;
                }
                return cur !== n.parent ? { ...n, parent: cur } : n;
            }).filter(n => !projectIds.has(n.id));
            activeLinks = data.links.filter((l: any) => {
                const sid = typeof l.source === 'object' ? l.source.id : l.source;
                const tid = typeof l.target === 'object' ? l.target.id : l.target;
                return !projectIds.has(sid) && !projectIds.has(tid);
            });
        }

        // Resolve link IDs to node references
        const nodeById = new Map(activeNodes.map(n => [n.id, n]));
        const nodeIndex = new Map(activeNodes.map((n, i) => [n.id, i]));
        activeLinks.forEach((l: any) => {
            if (typeof l.source === 'string') l.source = nodeById.get(l.source) || l.source;
            if (typeof l.target === 'string') l.target = nodeById.get(l.target) || l.target;
        });
        activeNodes.forEach((n: any) => { n.width = n.w; n.height = n.h; });

        // Build groups
        colaGroups = buildColaGroups(activeNodes, activeLinks, $viewSettings.colaGroupPadding);

        const svg = containerGroup?.ownerSVGElement;
        const vw = svg?.clientWidth || window.innerWidth || 1400;
        const vh = svg?.clientHeight || window.innerHeight || 900;
        const aspect = vw / vh;
        const ch = Math.round(Math.sqrt(CANVAS_AREA / aspect));
        const cw = Math.round(ch * aspect);

        // Scatter nodes randomly so Cola has something to work with
        const pad = 200;
        activeNodes.forEach((n: any) => {
            if (typeof n.x !== 'number') n.x = pad + Math.random() * (cw - pad * 2);
            if (typeof n.y !== 'number') n.y = pad + Math.random() * (ch - pad * 2);
        });

        // Render nodes
        const nEls = d3.select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .data(activeNodes, (d) => d.id)
            .join("g")
            .attr("class", "node")
            .attr("transform", (d) => `translate(${d.x ?? 0},${d.y ?? 0})`);

        const activeId = $selection.activeNodeId;
        nEls.each(function (d) {
            const g = d3.select(this) as any;
            const isSelected = d.id === activeId;
            if (g.selectAll("*").empty() || (d as any)._lastSelected !== isSelected) {
                g.selectAll("*").remove();
                buildTaskCardNode(g, d, isSelected);
                (d as any)._lastSelected = isSelected;
            }
        });
        bindDragAndClick(nEls);

        // Build Cola links (parent links omitted — groups handle hierarchy)
        const colaLinks = activeLinks
            .filter((l: any) => l.type !== 'parent')
            .map((l: any) => {
                const si = nodeIndex.get(typeof l.source === 'object' ? l.source.id : l.source);
                const ti = nodeIndex.get(typeof l.target === 'object' ? l.target.id : l.target);
                if (si === undefined || ti === undefined) return null;
                const length = ($viewSettings.colaLinkLength || 35) *
                    (l.type === 'depends_on' ? 1.2 : 1.4);
                const weight = l.type === 'depends_on' ? 1.0 : 0.5;
                return { source: si, target: ti, length, weight };
            })
            .filter((l: any) => l !== null) as any[];

        // Start Cola layout — sync only, no async ticks (5th param = false)
        colaLayout = cola.d3adaptor(d3)
            .size([cw, ch])
            .nodes(activeNodes as any)
            .links(colaLinks)
            .groups(colaGroups)
            .avoidOverlaps(true)
            .handleDisconnected(true)
            .linkDistance((d: any) => d.length)
            .on("tick", tickVisuals)
            .on("end", () => { $running = false; })
            .start(10, 15, 20, 0, false);

        // Render final positions
        tickVisuals();
    }

    onDestroy(() => {
        if (colaLayout) colaLayout.stop();
    });
</script>

{#if containerGroup}
    <g bind:this={hullLayer} class="hull-layer"></g>
    <g bind:this={nodesLayer}></g>
{/if}

<!-- Layout controls portaled outside SVG -->
<div use:portalControls class="layout-controls">
    <button class="ctrl-btn" on:click={toggleRunning}>
        {$running ? 'STOP' : 'RUN'}
    </button>
    <span class="tick-count">{$tickCount} ticks</span>
</div>

<style>
    .layout-controls {
        position: fixed;
        bottom: 80px;
        right: 20px;
        display: flex;
        align-items: center;
        gap: 8px;
        z-index: 50;
        font-family: monospace;
    }
    .ctrl-btn {
        background: hsla(40, 100%, 50%, 0.15);
        border: 1px solid hsla(40, 100%, 50%, 0.5);
        color: hsla(40, 100%, 50%, 0.9);
        padding: 4px 12px;
        font-size: 11px;
        font-weight: 700;
        font-family: monospace;
        text-transform: uppercase;
        letter-spacing: 0.1em;
        cursor: pointer;
        transition: background 0.2s;
    }
    .ctrl-btn:hover {
        background: hsla(40, 100%, 50%, 0.3);
    }
    .tick-count {
        color: hsla(40, 100%, 50%, 0.7);
        font-size: 11px;
        letter-spacing: 0.05em;
    }
    :global(g.node) {
        transition:
            opacity 0.3s cubic-bezier(0.4, 0, 0.2, 1),
            filter 0.3s ease;
    }
    :global(g.node.dimmed) {
        opacity: 0.6;
        filter: grayscale(0.5) brightness(0.75);
    }
    :global(g.node.selected-node) {
        opacity: 1 !important;
        filter: none !important;
    }
</style>
