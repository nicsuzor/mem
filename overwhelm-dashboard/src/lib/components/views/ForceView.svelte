<script lang="ts">
    import * as d3 from "d3";
    import * as cola from "webcola";
    import { onDestroy } from "svelte";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { viewSettings } from "../../stores/viewSettings";
    import { selection, toggleSelection } from "../../stores/selection";
    import { buildTaskCardNode } from "../shared/NodeShapes";
    import { projectHue } from "../../data/projectUtils";
    import { zoomScale } from "../../stores/zoom";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";

    const CANVAS_AREA = 30_000_000;

    export let containerGroup: SVGGElement;

    let nodesLayer: SVGGElement;
    let hullLayer: SVGGElement;

    let colaLayout: (cola.Layout & cola.ID3StyleLayoutAdaptor) | null = null;
    let colaGroups: any[] = [];
    let groupMembers: Map<string, string[]> = new Map();

    // Rebuild when structure or Cola params change (debounced)
    let lastStructureKey = '';
    let lastColaParams = '';
    let rebuildTimer: ReturnType<typeof setTimeout> | null = null;
    $: {
        const sk = $graphStructureKey;
        const cp = `${$viewSettings.colaLinkLength}|${$viewSettings.colaGroupPadding}|${$viewSettings.colaAvoidOverlaps}|${$viewSettings.colaHandleDisconnected}`;
        if (containerGroup && $graphData && nodesLayer && hullLayer && (sk !== lastStructureKey || cp !== lastColaParams)) {
            lastStructureKey = sk;
            lastColaParams = cp;
            if (rebuildTimer) clearTimeout(rebuildTimer);
            rebuildTimer = setTimeout(() => { rebuildTimer = null; drawForceAndStartPhysics(); }, 100);
        }
    }

    // Patch node visuals on property-only updates (no physics restart)
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
                    buildTaskCardNode(g, d, d.id === activeId);
                    (d as any)._lastSelected = d.id === activeId;
                }
            });
    }

    // Dimming + selection highlight
    $: if (nodesLayer && $graphData) {
        const activeId = $selection.activeNodeId;
        d3.select(nodesLayer).selectAll<SVGGElement, GraphNode>("g.node")
            .classed("dimmed", (d: any) => d.filter_dimmed)
            .classed("selected-node", (d: any) => d.id === activeId);
    }

    // ─── Group building ────────────────────────────────────────────────────────

    function buildColaGroups(activeNodes: GraphNode[], activeLinks: GraphEdge[], groupPadding: number): any[] {
        const nodeIndex = new Map(activeNodes.map((n, i) => [n.id, i]));
        const nodeById = new Map(activeNodes.map(n => [n.id, n]));

        // Build parent -> child indices from parent links
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

        // Flat group for every parent with children (no nesting — Cola's recursive
        // constraint generation explodes with nested groups)
        const groups: any[] = [];
        for (const [pid, childIdxs] of childrenOf) {
            const pidx = nodeIndex.get(pid);
            if (pidx === undefined || childIdxs.size === 0) continue;
            const pNode = nodeById.get(pid);
            groups.push({
                leaves: [pidx, ...childIdxs],
                padding: groupPadding + 55,
                label: pNode?.label || (pNode as any)?.fullTitle || pid,
                containerId: pid,
            });
        }

        // Each node in at most one group — group parents stay in their own group only
        const groupParentIdxs = new Set(groups.map((g: any) => nodeIndex.get(g.containerId)!));
        for (const g of groups) {
            const ownIdx = nodeIndex.get(g.containerId);
            g.leaves = g.leaves.filter((l: number) => !groupParentIdxs.has(l) || l === ownIdx);
        }

        // Build member map after dedup for accurate visual boxes
        const members = new Map<string, string[]>();
        for (const g of groups) {
            members.set(g.containerId, g.leaves.map((i: number) => activeNodes[i].id));
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
                    .on("drag", (e, d: any) => { d.x = e.x; d.y = e.y; tickVisuals(); })
                    .on("end", (_e, d: any) => { d.fixed = 0; }),
            );
    }

    // ─── Group box rendering ──────────────────────────────────────────────────

    function renderGroupBoxes() {
        if (!hullLayer) return;

        const nodePos = new Map<string, { x: number; y: number; w: number; h: number }>();
        d3.select(nodesLayer).selectAll<SVGGElement, GraphNode>("g.node")
            .each(function (d) { nodePos.set(d.id, { x: d.x ?? 0, y: d.y ?? 0, w: d.w ?? 0, h: d.h ?? 0 }); });

        type GB = { x: number; y: number; w: number; h: number; label: string; containerId: string };
        const data: GB[] = [];
        const PAD = 30;

        for (const [containerId, memberIds] of groupMembers) {
            const positions = memberIds.map(id => nodePos.get(id)).filter(Boolean) as { x: number; y: number; w: number; h: number }[];
            if (positions.length === 0) continue;
            let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
            for (const p of positions) {
                const hw = p.w / 2, hh = p.h / 2;
                minX = Math.min(minX, p.x - hw - PAD); minY = Math.min(minY, p.y - hh - PAD);
                maxX = Math.max(maxX, p.x + hw + PAD); maxY = Math.max(maxY, p.y + hh + PAD);
            }
            const cg = colaGroups.find((g: any) => g.containerId === containerId);
            data.push({ x: minX, y: minY, w: maxX - minX, h: maxY - minY,
                label: cg?.label || containerId, containerId });
        }

        d3.select(hullLayer).selectAll<SVGRectElement, GB>("rect.cola-group")
            .data(data, d => d.containerId).join("rect")
            .attr("class", "cola-group")
            .attr("rx", 10).attr("ry", 10)
            .attr("x", d => d.x).attr("y", d => d.y)
            .attr("width", d => d.w).attr("height", d => d.h)
            .attr("fill", d => `hsla(${projectHue(d.containerId)},40%,50%,0.08)`)
            .attr("stroke", d => `hsla(${projectHue(d.containerId)},40%,50%,0.3)`)
            .attr("stroke-width", 2)
            .attr("stroke-dasharray", "6,3")
            .style("cursor", "crosshair")
            .on("click", (e: any, d) => { e.stopPropagation(); toggleSelection(d.containerId); });

        d3.select(hullLayer).selectAll<SVGTextElement, GB>("text.cola-group-label")
            .data(data, d => d.containerId).join("text")
            .attr("class", "cola-group-label")
            .attr("x", d => d.x + 12).attr("y", d => d.y + 22)
            .attr("font-size", 18).attr("font-weight", 700)
            .attr("fill", d => `hsla(${projectHue(d.containerId)},60%,80%,0.9)`)
            .style("pointer-events", "none").style("text-transform", "uppercase").style("letter-spacing", "0.05em")
            .text(d => { const l = (d.label || '').toUpperCase(); const mc = Math.max(10, Math.floor(d.w / 10)); return l.length > mc ? l.slice(0, mc - 1) + '\u2026' : l; });
    }

    // ─── Tick + Main draw ─────────────────────────────────────────────────────

    function tickVisuals() {
        const scale = $zoomScale;
        d3.select(nodesLayer).selectAll<SVGGElement, GraphNode>("g.node")
            .attr("transform", d => `translate(${d.x ?? 0},${d.y ?? 0})`)
            .each(function (d) {
                const texts = d3.select(this).selectAll("text, tspan");
                if (!texts.empty()) texts.attr("opacity", (scale > 0.4 || (d.priority <= 1 && scale > 0.2)) ? null : 0);
            });
        renderGroupBoxes();
    }

    function drawForceAndStartPhysics() {
        if (!$graphData) return;
        if (colaLayout) { colaLayout.stop(); colaLayout = null; }

        const data = $graphData;

        // Strip project nodes
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
                while (cur && projectIds.has(cur)) { if (seen.has(cur)) break; seen.add(cur); cur = projParentMap.get(cur) ?? null; }
                return cur !== n.parent ? { ...n, parent: cur } : n;
            }).filter(n => !projectIds.has(n.id));
            activeLinks = data.links.filter((l: any) => {
                const sid = typeof l.source === 'object' ? l.source.id : l.source;
                const tid = typeof l.target === 'object' ? l.target.id : l.target;
                return !projectIds.has(sid) && !projectIds.has(tid);
            });
        }

        // Resolve links + set node dimensions for Cola
        const nodeById = new Map(activeNodes.map(n => [n.id, n]));
        const nodeIndex = new Map(activeNodes.map((n, i) => [n.id, i]));
        activeLinks.forEach((l: any) => {
            if (typeof l.source === 'string') l.source = nodeById.get(l.source) || l.source;
            if (typeof l.target === 'string') l.target = nodeById.get(l.target) || l.target;
        });
        // Inflate dimensions for Cola so constraints account for visual extras
        // (border strokes up to 4px, priority glow rings up to 6px, badges below)
        const COLA_PAD = 14;
        activeNodes.forEach((n: any) => { n.width = n.w + COLA_PAD; n.height = n.h + COLA_PAD; });

        colaGroups = buildColaGroups(activeNodes, activeLinks, $viewSettings.colaGroupPadding);

        const svg = containerGroup?.ownerSVGElement;
        const vw = svg?.clientWidth || window.innerWidth || 1400;
        const vh = svg?.clientHeight || window.innerHeight || 900;
        const aspect = vw / vh;
        const ch = Math.round(Math.sqrt(CANVAS_AREA / aspect));
        const cw = Math.round(ch * aspect);

        // Random initial scatter
        const pad = 200;
        activeNodes.forEach((n: any) => {
            if (typeof n.x !== 'number') n.x = pad + Math.random() * (cw - pad * 2);
            if (typeof n.y !== 'number') n.y = pad + Math.random() * (ch - pad * 2);
        });

        // Render nodes
        const nEls = d3.select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .data(activeNodes, d => d.id)
            .join("g").attr("class", "node")
            .attr("transform", d => `translate(${d.x ?? 0},${d.y ?? 0})`);
        const activeId = $selection.activeNodeId;
        nEls.each(function (d) {
            const g = d3.select(this) as any;
            const sel = d.id === activeId;
            if (g.selectAll("*").empty() || (d as any)._lastSelected !== sel) {
                g.selectAll("*").remove(); buildTaskCardNode(g, d, sel); (d as any)._lastSelected = sel;
            }
        });
        bindDragAndClick(nEls);

        // Parent links give Cola graph structure for stress majorization
        const colaLinks = activeLinks.filter((l: any) =>
            l.type === 'parent' && typeof l.source === 'object' && typeof l.target === 'object');

        // Save parent field — Cola overwrites it with internal group objects
        const savedParents = new Map(activeNodes.map(n => [n.id, n.parent]));

        colaLayout = cola.d3adaptor(d3)
            .size([cw, ch])
            .nodes(activeNodes as any)
            .links(colaLinks as any)
            .groups(colaGroups)
            .avoidOverlaps(true)
            .handleDisconnected(true)
            .start(10, 30, 200, 0, false);

        // Restore parent field after Cola finishes sync iterations
        activeNodes.forEach(n => { (n as any).parent = savedParents.get(n.id); });

        tickVisuals();
    }

    onDestroy(() => {
        if (rebuildTimer) clearTimeout(rebuildTimer);
        if (colaLayout) colaLayout.stop();
    });
</script>

{#if containerGroup}
    <g bind:this={hullLayer} class="hull-layer"></g>
    <g bind:this={nodesLayer}></g>
{/if}

<style>
    :global(g.node) { transition: opacity 0.3s ease, filter 0.3s ease; }
    :global(g.node.dimmed) { opacity: 0.6; filter: grayscale(0.5) brightness(0.75); }
    :global(g.node.selected-node) { opacity: 1 !important; filter: none !important; }
</style>
