<script lang="ts">
    import * as d3 from "d3";
    import * as cola from "webcola";
    import { onDestroy } from "svelte";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { selection, toggleSelection } from "../../stores/selection";
    import { buildTaskCardNode } from "../shared/NodeShapes";
    import { projectHue } from "../../data/projectUtils";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";

    const CANVAS_AREA = 30_000_000;
    const GROUP_PADDING = 60;

    export let containerGroup: SVGGElement;

    let nodesLayer: SVGGElement;
    let hullLayer: SVGGElement;

    let colaLayout: (cola.Layout & cola.ID3StyleLayoutAdaptor) | null = null;
    let colaGroups: any[] = [];
    export let running = false;

    export function toggleRunning() {
        if (!colaLayout) return;
        if (running) {
            colaLayout.stop();
            running = false;
        } else {
            colaLayout.resume();
            running = true;
        }
    }

    // Rebuild when graph structure changes
    let lastStructureKey = '';
    $: {
        const sk = $graphStructureKey;
        if (containerGroup && $graphData && nodesLayer && hullLayer && sk !== lastStructureKey) {
            lastStructureKey = sk;
            rebuild();
        }
    }

    // ─── Group building ────────────────────────────────────────────────────────

    function buildColaGroups(activeNodes: GraphNode[], activeLinks: GraphEdge[]): any[] {
        const nodeIndex = new Map(activeNodes.map((n, i) => [n.id, i]));
        const nodeById = new Map(activeNodes.map(n => [n.id, n]));

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

        const groups: any[] = [];
        const groupIndexOf = new Map<string, number>();
        for (const [pid, childIdxs] of childrenOf) {
            const pidx = nodeIndex.get(pid);
            if (pidx === undefined || childIdxs.size === 0) continue;
            groupIndexOf.set(pid, groups.length);
            groups.push({
                leaves: [pidx, ...childIdxs],
                groups: [],
                padding: GROUP_PADDING,
                containerId: pid,
            });
        }

        // Wire nesting
        for (const [pid] of groupIndexOf) {
            const pNode = nodeById.get(pid);
            if (!pNode?.parent) continue;
            const parentGroupIdx = groupIndexOf.get(pNode.parent);
            if (parentGroupIdx === undefined) continue;
            const thisGroupIdx = groupIndexOf.get(pid)!;
            groups[parentGroupIdx].groups.push(groups[thisGroupIdx]);
        }

        // Deduplicate leaves across nested groups
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

        type GB = { x: number; y: number; w: number; h: number; containerId: string };
        const data: GB[] = [];

        for (const cg of colaGroups) {
            if (!cg.bounds) continue;
            const b = cg.bounds;
            data.push({ x: b.x, y: b.y, w: b.X - b.x, h: b.Y - b.y, containerId: cg.containerId });
        }

        d3.select(hullLayer).selectAll<SVGRectElement, GB>("rect.cola-group")
            .data(data, d => d.containerId).join("rect")
            .attr("class", "cola-group")
            .attr("rx", 8).attr("ry", 8)
            .attr("x", d => d.x).attr("y", d => d.y)
            .attr("width", d => d.w).attr("height", d => d.h)
            .attr("fill", d => `hsla(${projectHue(d.containerId)},40%,50%,0.08)`)
            .attr("stroke", d => `hsla(${projectHue(d.containerId)},40%,50%,0.3)`)
            .attr("stroke-width", 1.5)
            .style("cursor", "crosshair")
            .on("click", (e: any, d) => { e.stopPropagation(); toggleSelection(d.containerId); });
    }

    // ─── Tick + rebuild ──────────────────────────────────────────────────────

    function tickVisuals() {
        d3.select(nodesLayer).selectAll<SVGGElement, GraphNode>("g.node")
            .attr("transform", d => `translate(${d.x ?? 0},${d.y ?? 0})`);
        renderGroupBoxes();
    }

    function rebuild() {
        if (!$graphData) return;
        if (colaLayout) { colaLayout.stop(); colaLayout = null; }

        const nodes: GraphNode[] = $graphData.nodes;
        const links: GraphEdge[] = $graphData.links;

        // Resolve link references to node objects
        const nodeById = new Map(nodes.map(n => [n.id, n]));
        links.forEach((l: any) => {
            if (typeof l.source === 'string') l.source = nodeById.get(l.source) || l.source;
            if (typeof l.target === 'string') l.target = nodeById.get(l.target) || l.target;
        });

        // Set Cola dimensions = actual card size
        nodes.forEach((n: any) => { n.width = n.w; n.height = n.h; });

        // Build hierarchical groups
        colaGroups = buildColaGroups(nodes, links);

        // Canvas from CANVAS_AREA
        const svg = containerGroup?.ownerSVGElement;
        const vw = svg?.clientWidth || window.innerWidth || 1400;
        const vh = svg?.clientHeight || window.innerHeight || 900;
        const aspect = vw / vh;
        const ch = Math.round(Math.sqrt(CANVAS_AREA / aspect));
        const cw = Math.round(ch * aspect);

        // Random initial positions
        nodes.forEach((n: any) => {
            if (typeof n.x !== 'number') n.x = Math.random() * cw;
            if (typeof n.y !== 'number') n.y = Math.random() * ch;
        });

        // Render nodes
        const activeId = $selection.activeNodeId;
        const nEls = d3.select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .data(nodes, d => d.id)
            .join("g").attr("class", "node")
            .attr("transform", d => `translate(${d.x ?? 0},${d.y ?? 0})`);
        nEls.each(function (d) {
            const g = d3.select(this) as any;
            g.selectAll("*").remove();
            buildTaskCardNode(g, d, d.id === activeId);
        });
        bindDragAndClick(nEls);

        // Parent links for Cola structure
        const colaLinks = links.filter((l: any) =>
            l.type === 'parent' && typeof l.source === 'object' && typeof l.target === 'object');

        // Bare Cola — async tick
        colaLayout = cola.d3adaptor(d3)
            .size([cw, ch])
            .nodes(nodes as any)
            .links(colaLinks as any)
            .groups(colaGroups)
            .linkDistance(40)
            .avoidOverlaps(true)
            .handleDisconnected(true)
            .on("tick", tickVisuals)
            .on("end", () => { running = false; })
            .start();
        running = true;
    }

    onDestroy(() => {
        if (colaLayout) colaLayout.stop();
    });
</script>

{#if containerGroup}
    <g bind:this={hullLayer} class="hull-layer"></g>
    <g bind:this={nodesLayer}></g>
{/if}
