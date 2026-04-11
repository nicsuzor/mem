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

        // Create a group for every parent with children
        const groups: any[] = [];
        const groupIndexOf = new Map<string, number>();
        for (const [pid, childIdxs] of childrenOf) {
            const pidx = nodeIndex.get(pid);
            if (pidx === undefined || childIdxs.size === 0) continue;
            const pNode = nodeById.get(pid);
            groupIndexOf.set(pid, groups.length);
            groups.push({
                leaves: [pidx, ...childIdxs],
                groups: [],
                padding: groupPadding,
                label: pNode?.label || (pNode as any)?.fullTitle || pid,
                containerId: pid,
            });
        }

        // Wire nesting: if a group's container node is itself a child of another
        // group's container, nest it using Cola's native hierarchy support
        for (const [pid] of groupIndexOf) {
            const pNode = nodeById.get(pid);
            if (!pNode?.parent) continue;
            const parentGroupIdx = groupIndexOf.get(pNode.parent);
            if (parentGroupIdx === undefined) continue;
            const thisGroupIdx = groupIndexOf.get(pid)!;
            groups[parentGroupIdx].groups.push(groups[thisGroupIdx]);
            groups[thisGroupIdx].padding = groupPadding;
            groups[thisGroupIdx].nested = true;
        }

        // Remove nested group members from parent leaves — Cola requires
        // each node index in exactly one group's leaves array
        for (const g of groups) {
            if (g.groups.length === 0) continue;
            const nested = new Set<number>();
            for (const child of g.groups) for (const l of child.leaves) nested.add(l);
            g.leaves = g.leaves.filter((l: number) => !nested.has(l));
        }

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

        type GB = { x: number; y: number; w: number; h: number; label: string; containerId: string; nested: boolean };
        const data: GB[] = [];

        // Use Cola's computed group.bounds directly — these are guaranteed
        // non-overlapping by Cola's constraint solver
        for (const cg of colaGroups) {
            if (!cg.bounds) continue;
            const b = cg.bounds;
            data.push({
                x: b.x, y: b.y, w: b.X - b.x, h: b.Y - b.y,
                label: cg.label || cg.containerId, containerId: cg.containerId,
                nested: !!cg.nested,
            });
        }

        d3.select(hullLayer).selectAll<SVGRectElement, GB>("rect.cola-group")
            .data(data, d => d.containerId).join("rect")
            .attr("class", "cola-group")
            .attr("rx", d => d.nested ? 6 : 10).attr("ry", d => d.nested ? 6 : 10)
            .attr("x", d => d.x).attr("y", d => d.y)
            .attr("width", d => d.w).attr("height", d => d.h)
            .attr("fill", d => { const h = projectHue(d.containerId); return d.nested ? `hsla(${h},40%,50%,0.05)` : `hsla(${h},40%,50%,0.08)`; })
            .attr("stroke", d => { const h = projectHue(d.containerId); return d.nested ? `hsla(${h},50%,55%,0.25)` : `hsla(${h},40%,50%,0.3)`; })
            .attr("stroke-width", d => d.nested ? 1 : 2)
            .attr("stroke-dasharray", d => d.nested ? "4,2" : "6,3")
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
        activeLinks.forEach((l: any) => {
            if (typeof l.source === 'string') l.source = nodeById.get(l.source) || l.source;
            if (typeof l.target === 'string') l.target = nodeById.get(l.target) || l.target;
        });
        activeNodes.forEach((n: any) => { n.width = n.w; n.height = n.h; });

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

        colaLayout = cola.d3adaptor(d3)
            .size([cw, ch])
            .nodes(activeNodes as any)
            .links(colaLinks as any)
            .groups(colaGroups)
            .avoidOverlaps(true)
            .handleDisconnected(true)
            .start(10, 30, 200, 0, false);

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
