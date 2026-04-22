<script lang="ts">
    import * as d3 from "d3";
    import * as cola from "webcola";
    import { onDestroy } from "svelte";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { viewSettings } from "../../stores/viewSettings";
    import { filters } from "../../stores/filters";
    import { selection, toggleSelection } from "../../stores/selection";
    import { buildTaskCardNode } from "../shared/NodeShapes";
    import { projectHue } from "../../data/projectUtils";
    import { zoomScale } from "../../stores/zoom";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";

    /**
     * COLA PHYSICS ENGINE: ARCHITECTURAL OVERVIEW
     * The physical position of every node is determined by the balance of these constraints and forces:
     *
     * 1. GROUP CONTAINMENT (Hard Constraint):
     *    Every node assigned to a group's `leaves` MUST be contained within that group's
     *    bounding box. The box expands/stretches to wrap its nodes.
     *
     * 2. NON-OVERLAP (Hard Constraint):
     *    Nodes are treated as solid blocks based on their width/height. They can NEVER overlap.
     *    Note: Because nodes (task cards) are wider than they are tall, resolving initial overlaps
     *    typically results in vertical stacking (the shortest path to clear the overlap). Unlike d3-force,
     *    WebCola does NOT have a continuous global repulsive "magnetic" force pushing nodes apart.
     *
     * 3. DEPENDENCY LINKS (Spring Force):
     *    Tries to maintain nodes at exactly the `dist` distance with `weight` strength.
     *
     * 4. 300-TICK FRICTION (Killswitch):
     *    Simulation automatically stops after 300 iterations to save CPU.
     *
     * 5. USER ANCHOR (Manual Constraint):
     *    Dragging a node sets `fixed = 1`, overriding all physics for that node.
     */

    const NODE_AREA_BUDGET = 25_000; // sq px personal space per node

    export let containerGroup: SVGGElement;
    export let running = false;
    export let restartNonce = 0;
    export let randomizeNonce = 0;

    let linksLayer: SVGGElement;
    let nodesLayer: SVGGElement;
    let hullLayer: SVGGElement;

    let colaLayout: (cola.Layout & cola.ID3StyleLayoutAdaptor) | null = null;
    let colaGroups: any[] = [];
    let ticks = 0;
    let lastRestartNonce = 0;
    let prevSimNodeIds = new Set<string>();
    let lastRandomizeNonce = 0;

    // ─── Helpers ──────────────────────────────────────────────────────────────

    function resolveLinkReferences(nodes: GraphNode[], links: GraphEdge[]) {
        const nodeById = new Map(nodes.map(n => [n.id, n]));
        links.forEach((l: any) => {
            if (typeof l.source === 'string') l.source = nodeById.get(l.source) || l.source;
            if (typeof l.target === 'string') l.target = nodeById.get(l.target) || l.target;
        });
    }

    function syncNodeDimensions(nodes: GraphNode[]) {
        nodes.forEach((n: any) => { n.width = n.w + 12; n.height = n.h + 24; });
    }

    function getActiveNodes(baseNodes: GraphNode[]): GraphNode[] {
        let nodes = baseNodes.filter((n: any) => n.type !== 'project');

        if (!$viewSettings.colaHandleDisconnected) {
            const nodeById = new Map(nodes.map(n => [n.id, n]));
            const childrenOf = new Map<string, Set<string>>();

            for (const n of nodes) {
                const pid = (n as any)._safe_parent;
                if (!pid || !nodeById.has(pid)) continue;
                if (!childrenOf.has(pid)) childrenOf.set(pid, new Set());
                childrenOf.get(pid)!.add(n.id);
            }

            const keepIds = new Set<string>();
            for (const [pid, children] of childrenOf.entries()) {
                if (children.size >= 1) {
                    keepIds.add(pid);
                    for (const cid of children) {
                        keepIds.add(cid);
                    }
                }
            }

            nodes = nodes.filter(n => keepIds.has(n.id));
        }

        return nodes;
    }

    function buildPhysicsLinks(nodes: GraphNode[], links: GraphEdge[]) {
        const parentIds = new Set(nodes.map(n => (n as any)._safe_parent).filter(Boolean));
        return links.filter((l: any) => {
            if (typeof l.source !== 'object' || typeof l.target !== 'object') return false;

            const src = l.source as any;
            const tgt = l.target as any;
            const srcParent = src._safe_parent;
            const tgtParent = tgt._safe_parent;

            const isIntraGroup = (srcParent && srcParent === tgtParent) ||
                                 (tgtParent === src.id && !parentIds.has(tgt.id)) ||
                                 (srcParent === tgt.id && !parentIds.has(src.id));

            // Drop non-parent intra-group edges (like depends_on or ref between siblings) as before
            if (l.type !== 'parent' && isIntraGroup) return false;

            l._isIntraGroup = (l.type === 'parent' && isIntraGroup);

            if (l._isIntraGroup && ($filters as any).edgeIntraGroup === 'hidden') return false;
            if (l.type === 'parent' && !l._isIntraGroup && $filters.edgeParent === 'hidden') return false;
            if (l.type === 'depends_on' && $filters.edgeDependencies === 'hidden') return false;
            if (l.type === 'ref' && $filters.edgeReferences === 'hidden') return false;

            return true;
        }).map((l: any) => {
            let length = $viewSettings.colaLinkLength;
            let weight = 0.2;
            let color = undefined;

            if (l.type === 'parent') {
                if (l._isIntraGroup) {
                    length = $viewSettings.colaLinkDistIntraParent;
                    weight = $viewSettings.colaLinkWeightIntraParent;
                    color = '#3b82f6';
                } else {
                    length = $viewSettings.colaLinkDistInterParent;
                    weight = $viewSettings.colaLinkWeightInterParent;
                }
            } else if (l.type === 'depends_on') {
                length = $viewSettings.colaLinkDistDependsOn;
                weight = $viewSettings.colaLinkWeightDependsOn;
            } else if (l.type === 'ref' || l.type === 'soft_depends_on') {
                length = $viewSettings.colaLinkDistRef;
                weight = $viewSettings.colaLinkWeightRef;
            }

            return { ...l, length, weight, color: color || l.color };
        });
    }

    function renderNodes(nodes: GraphNode[]) {
        const activeId = $selection.activeNodeId;
        const nEls = d3.select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .data(nodes, d => d.id)
            .join("g").attr("class", "node")
            .attr("transform", d => `translate(${d.x ?? 0},${d.y ?? 0})`);
        nEls.each(function(d) {
            const g = d3.select(this) as any;
            g.selectAll("*").remove();
            buildTaskCardNode(g, d, d.id === activeId);
            // Tag base font sizes for zoom scaling
            g.selectAll<SVGTextElement, any>('text').each(function() {
                const el = d3.select(this);
                if (!el.attr('data-base-fs')) el.attr('data-base-fs', el.attr('font-size') || '10');
            });
        });
        bindDragAndClick(nEls);
        applyZoomTextScale();
    }

    function applyZoomTextScale() {
        if (!nodesLayer) return;
        d3.select(nodesLayer).selectAll<SVGTextElement, any>('text[data-base-fs]')
            .each(function() {
                const el = d3.select(this);
                const base = parseFloat(el.attr('data-base-fs') || '10');
                el.attr('font-size', `${base}px`);
            });
    }

    function renderLinks(colaLinks: any[]) {
        if (!linksLayer) return;
        d3.select(linksLayer).selectAll("line.link")
            .data(colaLinks)
            .join("line").attr("class", "link")
            .attr("stroke", (d: any) => d.color || "#cbd5e1")
            .attr("stroke-width", (d: any) => d.width || 1.5)
            .attr("stroke-dasharray", (d: any) => d.dash || null)
            .attr("opacity", 0.6);
    }

    // ─── Group building ────────────────────────────────────────────────────────

    /**
     * Builds WebCola flat groups (no nesting).
     * Uses `_safe_parent` (set by +page.svelte before Cola mutates n.parent into a circular ref).
     */
    function buildColaGroups(activeNodes: GraphNode[], _activeLinks: GraphEdge[]): any[] {
        const nodeIndex = new Map(activeNodes.map((n, i) => [n.id, i]));
        const childrenOf = new Map<string, Set<number>>();

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
        const parentIds = new Set(childrenOf.keys());

        for (const pid of parentIds) {
            const pidx = nodeIndex.get(pid);
            if (pidx === undefined) continue;

            const leafIndices = [pidx];
            for (const cidx of childrenOf.get(pid)!) {
                if (!parentIds.has(activeNodes[cidx].id)) {
                    leafIndices.push(cidx);
                }
            }

            if (leafIndices.length <= 1) continue;

            groups.push({
                leaves: leafIndices,
                groups: [],
                padding: $viewSettings.colaGroupPadding,
                containerId: pid,
            });
        }

        const assignedLeaves = new Set<number>();
        for (const g of groups) {
            for (const leafIdx of g.leaves) {
                assignedLeaves.add(leafIdx);
            }
        }

        const unassignedLeaves = [];
        for (let i = 0; i < activeNodes.length; i++) {
            if (!assignedLeaves.has(i)) {
                unassignedLeaves.push(i);
            }
        }

        if (unassignedLeaves.length > 0 && $viewSettings.colaHandleDisconnected) {
            groups.push({
                leaves: unassignedLeaves,
                groups: [], // can also use subGroupIndices to nest
                padding: $viewSettings.colaGroupPadding + 20,
                containerId: "__main__",
            });
        }

        return groups;
    }

    // ─── Drag and click ───────────────────────────────────────────────────────

    function bindDragAndClick(nEls: any) {
        let dragOx = 0, dragOy = 0;
        nEls.style("cursor", "crosshair")
            .on("click", (e: any, d: any) => { e.stopPropagation(); toggleSelection(d.id); })
            .call(
                d3.drag<SVGGElement, GraphNode>()
                    .clickDistance(4)
                    .on("start", (e, d: any) => {
                        d.fixed = 1;
                        dragOx = e.x - (d.x ?? 0);
                        dragOy = e.y - (d.y ?? 0);
                    })
                    .on("drag", (e, d: any) => {
                        d.x = e.x - dragOx;
                        d.y = e.y - dragOy;
                        if (colaLayout) {
                            ticks = 0;
                            colaLayout.resume();
                            running = true;
                        }
                        tickVisuals();
                    })
                    .on("end", (_e, d: any) => { d.fixed = 0; }),
            );
    }

    // ─── Group box rendering ──────────────────────────────────────────────────

    function renderGroupBoxes() {
        if (!hullLayer) return;

        if (colaGroups.length === 0) {
            d3.select(hullLayer).selectAll("rect.cola-group").remove();
            return;
        }

        type GB = { x: number; y: number; w: number; h: number; containerId: string };
        const data: GB[] = [];

        for (const cg of colaGroups) {
            if (!cg.bounds) continue;
            const b = cg.bounds;
            data.push({ x: b.x, y: b.y, w: b.X - b.x, h: b.Y - b.y, containerId: cg.containerId });
        }

        // Bounds not ready yet — keep existing rects until next tick populates them
        if (data.length === 0) return;

        d3.select(hullLayer).selectAll<SVGRectElement, GB>("rect.cola-group")
            .data(data, d => d.containerId).join("rect")
            .attr("class", "cola-group")
            .attr("rx", 8).attr("ry", 8)
            .attr("x", d => d.x).attr("y", d => d.y)
            .attr("width", d => d.w).attr("height", d => d.h)
            .attr("fill", d => `hsla(${projectHue(d.containerId)},40%,50%,0.15)`)
            .attr("stroke", d => `hsla(${projectHue(d.containerId)},40%,50%,0.3)`)
            .attr("stroke-width", 1.5)
            .style("cursor", "crosshair")
            .on("click", (e: any, d) => { e.stopPropagation(); toggleSelection(d.containerId); });
    }

    // ─── Tick + layout control ────────────────────────────────────────────────

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

    function reheatLayout(heat = 0.95) {
        if (!colaLayout) return;
        ticks = 0;
        colaLayout.resume(); // restart timer if stopped
        colaLayout.alpha(heat); // override default 0.1 energy
        running = true;
    }

    function applyLiveLayoutSettings(heat = 0.85) {
        if (!$graphData || !colaLayout) return;

        const nodes = getActiveNodes($graphData.nodes);
        const links = $graphData.links.map((l: any) => ({ ...l })); // Shallow copy to avoid mutating store

        resolveLinkReferences(nodes, links);
        syncNodeDimensions(nodes);
        colaGroups = buildColaGroups(nodes, links);

        const colaLinks = buildPhysicsLinks(nodes, links);
        renderNodes(nodes);
        renderLinks(colaLinks);

        colaLayout
            .nodes(nodes as any)
            .links(colaLinks as any)
            .groups(colaGroups)
            .linkDistance((l: any) => l.length)
            .convergenceThreshold($viewSettings.colaConvergence)
            .avoidOverlaps(true)
            .handleDisconnected($viewSettings.colaHandleDisconnected);

        tickVisuals();
        reheatLayout(heat);
    }

    function rebuild() {
        if (!$graphData) return;
        if (colaLayout) { colaLayout.stop(); colaLayout = null; }

        const nodes = getActiveNodes($graphData.nodes);
        const links = $graphData.links.map((l: any) => ({ ...l })); // Shallow copy

        resolveLinkReferences(nodes, links);
        syncNodeDimensions(nodes);
        colaGroups = buildColaGroups(nodes, links);

        const svg = containerGroup?.ownerSVGElement;
        const vw = svg?.clientWidth || window.innerWidth || 1400;
        const vh = svg?.clientHeight || window.innerHeight || 900;
        const aspect = vw / vh;
        const canvasArea = Math.max(nodes.length * NODE_AREA_BUDGET, vw * vh);
        const ch = Math.round(Math.sqrt(canvasArea / aspect));
        const cw = Math.round(ch * aspect);

        const margin = 50;
        nodes.forEach((n: any) => {
            if (!prevSimNodeIds.has(n.id) || typeof n.x !== 'number' || n.x < -9000) {
                n.x = (Math.random() * cw * 0.8) + (cw * 0.1);
                n.y = (Math.random() * ch * 0.8) + (ch * 0.1);
            } else {
                n.x = Math.max(margin, Math.min(cw - margin, n.x));
                n.y = Math.max(margin, Math.min(ch - margin, n.y));
            }
        });
        prevSimNodeIds = new Set(nodes.map(n => n.id));

        renderNodes(nodes);

        const colaLinks = buildPhysicsLinks(nodes, links);
        renderLinks(colaLinks);

        ticks = 0;

        colaLayout = cola.d3adaptor(d3)
            .size([cw, ch])
            .nodes(nodes as any)
            .links(colaLinks as any)
            .groups(colaGroups)
            .linkDistance((l: any) => l.length)
            .convergenceThreshold($viewSettings.colaConvergence)
            .avoidOverlaps(true)
            .handleDisconnected($viewSettings.colaHandleDisconnected)
            .on("tick", () => {
                ticks++;
                if (ticks > 500) {
                    if (colaLayout) colaLayout.stop();
                    running = false;
                }
                tickVisuals();
            })
            .on("end", () => { running = false; })
            .start(30, 20, 20);
        running = true;

        tickVisuals();
    }

    // ─── Public API ───────────────────────────────────────────────────────────

    export function toggleRunning() {
        if (!colaLayout) return;
        if (running) {
            colaLayout.stop();
            running = false;
        } else {
            ticks = 0;
            if ($graphData?.nodes) {
                $graphData.nodes.forEach((n: any) => {
                    if (!n.fixed) {
                        n.x = (n.x || 0) + (Math.random() * 10 - 5);
                        n.y = (n.y || 0) + (Math.random() * 10 - 5);
                    }
                });
            }
            colaLayout.resume();
            running = true;
        }
    }

    function jiggleNodes(radius = 320) {
        if (!$graphData) return;
        const half = radius / 2;
        for (const n of $graphData.nodes as any[]) {
            if (n.fixed) continue;
            n.x = (n.x ?? 0) + (Math.random() * radius - half);
            n.y = (n.y ?? 0) + (Math.random() * radius - half);
            n.px = n.x;
            n.py = n.y;
        }
    }

    export function randomize() {
        if (!$graphData) return;
        jiggleNodes(320);
        tickVisuals();
        if (colaLayout) {
            reheatLayout(1);
        } else {
            rebuild();
        }
    }

    // ─── Reactivity ───────────────────────────────────────────────────────────

    let lastStructureKey = '';
    let lastColaParams = '';

    $: {
        const sk = $graphStructureKey;
        const cp = `${$viewSettings.colaLinkLength}|${$viewSettings.colaConvergence}|${$viewSettings.colaHandleDisconnected}|${$viewSettings.colaGroupPadding}|${$viewSettings.colaLinkDistIntraParent}|${$viewSettings.colaLinkWeightIntraParent}|${$viewSettings.colaLinkDistInterParent}|${$viewSettings.colaLinkWeightInterParent}|${$viewSettings.colaLinkDistDependsOn}|${$viewSettings.colaLinkWeightDependsOn}|${$viewSettings.colaLinkDistRef}|${$viewSettings.colaLinkWeightRef}|${$filters.edgeDependencies}|${$filters.edgeReferences}|${$filters.edgeParent}|${($filters as any).edgeIntraGroup}`;
        if (containerGroup && $graphData && nodesLayer && hullLayer) {
            if (sk !== lastStructureKey) {
                lastStructureKey = sk;
                lastColaParams = cp;
                rebuild();
            } else if (cp !== lastColaParams) {
                lastColaParams = cp;
                applyLiveLayoutSettings();
            }
        }
    }

    $: if (restartNonce !== lastRestartNonce) {
        lastRestartNonce = restartNonce;
        if (restartNonce > 0 && $graphData && colaLayout) reheatLayout(0.7);
    }

    $: if (randomizeNonce !== lastRandomizeNonce) {
        lastRandomizeNonce = randomizeNonce;
        if (randomizeNonce > 0 && $graphData) randomize();
    }

    $: if ($zoomScale !== undefined) applyZoomTextScale();

    onDestroy(() => {
        if (colaLayout) colaLayout.stop();
        prevSimNodeIds = new Set();
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
        filter: brightness(1.6);
    }
</style>
