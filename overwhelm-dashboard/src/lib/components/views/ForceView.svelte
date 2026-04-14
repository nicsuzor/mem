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
    const GROUP_PADDING = 60;

    export let containerGroup: SVGGElement;

    let linksLayer: SVGGElement;
    let nodesLayer: SVGGElement;
    let hullLayer: SVGGElement;

    let colaLayout: (cola.Layout & cola.ID3StyleLayoutAdaptor) | null = null;
    let colaGroups: any[] = [];
    export let running = false;
    let ticks = 0;

    export function toggleRunning() {
        if (!colaLayout) return;
        if (running) {
            colaLayout.stop();
            running = false;
        } else {
            ticks = 0;
            colaLayout.resume();
            running = true;
        }
    }

    // Rebuild when graph structure changes
    let lastStructureKey = '';
    let lastColaParams = '';
    $: {
        const sk = $graphStructureKey;
        const cp = `${$viewSettings.colaLinkLength}|${$viewSettings.colaConvergence}|${$viewSettings.colaHandleDisconnected}`;
        if (containerGroup && $graphData && nodesLayer && hullLayer && (sk !== lastStructureKey || cp !== lastColaParams)) {
            lastStructureKey = sk;
            lastColaParams = cp;
            rebuild();
        }
    }

    // ─── Group building ────────────────────────────────────────────────────────

    /**
     * Builds WebCola hierarchical groups.
     * 
     * ARCHITECTURE NOTES:
     * 1. Source of Truth: We MUST use `_safe_parent` from the node objects. The 
     *    `n.parent` string is mutated by WebCola into a circular Group object reference 
     *    during the first physics tick, which destroys Svelte's ability to rebuild the hierarchy.
     * 
     * 2. Integer Array Indices: WebCola strictly requires `leaves` to be integer 
     *    indices into the `activeNodes` array, NOT object references.
     */
    function buildColaGroups(activeNodes: GraphNode[], _activeLinks: GraphEdge[]): any[] {
        const nodeIndex = new Map(activeNodes.map((n, i) => [n.id, i]));
        const nodeById = new Map(activeNodes.map(n => [n.id, n]));

        const childrenOf = new Map<string, Set<number>>();
        
        // Use n._safe_parent as the single source of truth for group hierarchies
        for (const n of activeNodes) {
            const pid = (n as any)._safe_parent;
            if (!pid) continue;
            const pidx = nodeIndex.get(pid);
            const cidx = nodeIndex.get(n.id);
            if (pidx === undefined || cidx === undefined) continue;
            
            if (!childrenOf.has(pid)) childrenOf.set(pid, new Set());
            childrenOf.get(pid)!.add(cidx);
        }

        const groups: any[] = [];
        const groupIndexOf = new Map<string, number>();

        // 1. Create empty groups for valid parents
        for (const [pid, childIdxs] of childrenOf) {
            const pidx = nodeIndex.get(pid);
            if (pidx === undefined) continue;
            
            groupIndexOf.set(pid, groups.length);
            groups.push({
                // Include both parent and children in the same group box
                leaves: [pidx, ...Array.from(childIdxs)], 
                groups: [], // Will hold references to child groups
                padding: GROUP_PADDING,
                containerId: pid,
            });
        }

        // 2. Nest groups inside their nearest ancestor group
        for (const [pid, groupIdx] of groupIndexOf) {
            const pNode = nodeById.get(pid);
            let curr = (pNode as any)?._safe_parent;
            while (curr) {
                const parentGroupIdx = groupIndexOf.get(curr);
                if (parentGroupIdx !== undefined) {
                    // Tell the parent group that this group is nested inside it
                    groups[parentGroupIdx].groups.push(groupIdx);
                    break;
                }
                const currNode = nodeById.get(curr);
                curr = (currNode as any)?._safe_parent;
            }
        }

        // 3. Important: Cola expects nested groups to be object references, not index numbers.
        for (const g of groups) {
            g.groups = g.groups.map((idx: number) => groups[idx]);
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
                    .on("start", (_e, d: any) => { 
                        d.fixed = 1; 
                    })
                    .on("drag", (e, d: any) => { 
                        d.x = e.x; 
                        d.y = e.y; 
                        
                        // Resume layout on drag so bounding boxes follow the node
                        if (colaLayout && !running) {
                            ticks = 0;
                            colaLayout.resume();
                            running = true;
                        } else if (colaLayout) {
                            ticks = 0;
                            colaLayout.resume();
                        }
                        tickVisuals(); 
                    })
                    .on("end", (_e, d: any) => { 
                        d.fixed = 0; 
                    }),
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
        if (linksLayer) {
            d3.select(linksLayer).selectAll<SVGLineElement, any>("line.link")
                .attr("x1", d => d.source.x).attr("y1", d => d.source.y)
                .attr("x2", d => d.target.x).attr("y2", d => d.target.y);
        }
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

        // Set Cola dimensions = actual card size + visual buffer for badges/glows
        nodes.forEach((n: any) => { n.width = n.w + 12; n.height = n.h + 24; });

        // Build hierarchical groups
        colaGroups = buildColaGroups(nodes, links);

        // Canvas from CANVAS_AREA
        const svg = containerGroup?.ownerSVGElement;
        const vw = svg?.clientWidth || window.innerWidth || 1400;
        const vh = svg?.clientHeight || window.innerHeight || 900;
        const aspect = vw / vh;
        const ch = Math.round(Math.sqrt(CANVAS_AREA / aspect));
        const cw = Math.round(ch * aspect);

        // ARCHITECTURE NOTE (Physics Timeout & Disconnected Components):
        // Assign each root container (e.g. Project) a random center point on the canvas, 
        // and spawn all its descendants tightly around that point.
        const rootCenters = new Map<string, {x: number, y: number}>();
        
        nodes.forEach((n: any) => {
            if (typeof n.x !== 'number' || n.x < -9000) {
                let rootId = n.id;
                let curr = n._safe_parent;
                while (curr) {
                    rootId = curr;
                    const parentNode = nodeById.get(curr);
                    curr = parentNode ? parentNode._safe_parent : null;
                }
                
                if (!rootCenters.has(rootId)) {
                    rootCenters.set(rootId, {
                        x: (cw * 0.1) + Math.random() * (cw * 0.8),
                        y: (ch * 0.1) + Math.random() * (ch * 0.8)
                    });
                }
                
                const center = rootCenters.get(rootId)!;
                n.x = center.x + (Math.random() * 200 - 100);
                n.y = center.y + (Math.random() * 200 - 100);
            }
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

        // Physics links (EXCLUDING parent links as per user instruction)
        const colaLinks = links.filter((l: any) =>
            l.type !== 'parent' && typeof l.source === 'object' && typeof l.target === 'object');

        if (linksLayer) {
            d3.select(linksLayer).selectAll("line.link")
                .data(colaLinks)
                .join("line").attr("class", "link")
                .attr("stroke", (d: any) => d.color || "#cbd5e1")
                .attr("stroke-width", (d: any) => d.width || 1.5)
                .attr("stroke-dasharray", (d: any) => d.dash || null)
                .attr("opacity", 0.6);
        }

        ticks = 0;

        colaLayout = cola.d3adaptor(d3)
            .size([cw, ch])
            .nodes(nodes as any)
            .links(colaLinks as any)
            .groups(colaGroups)
            .linkDistance((d: any) => {
                // Use _safe_parent to check if nodes share the same structural parent
                if (d.source._safe_parent && d.target._safe_parent && d.source._safe_parent === d.target._safe_parent) {
                    return 50; // Short intra-group dependency links
                }
                return $viewSettings.colaLinkLength; // Long inter-group dependency links
            })
            .convergenceThreshold($viewSettings.colaConvergence)
            .avoidOverlaps(true)
            .handleDisconnected($viewSettings.colaHandleDisconnected)
            .on("tick", () => {
                ticks++;
                if (ticks > 300) {
                    if (colaLayout) colaLayout.stop();
                    running = false;
                }
                tickVisuals();
            })
            .on("end", () => { running = false; })
            .start(5, 5, 5); 
        running = true;
    }

    onDestroy(() => {
        if (colaLayout) colaLayout.stop();
    });
</script>

{#if containerGroup}
    <g bind:this={hullLayer} class="hull-layer"></g>
    <g bind:this={linksLayer} class="links-layer"></g>
    <g bind:this={nodesLayer}></g>
{/if}

<style>
    :global(rect.cola-group) {
        transition: fill 0.3s, stroke 0.3s;
    }
    :global(rect.cola-group:hover) {
        fill-opacity: 0.15;
        stroke-opacity: 0.6;
    }
</style>
