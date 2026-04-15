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

    const CANVAS_AREA = 30_000_000;

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
            // Add a tiny jitter to unstick nodes that perfectly hit local constraint minima
            if ($graphData && $graphData.nodes) {
                $graphData.nodes.forEach((n: any) => {
                    if (!n.fixed) {
                        n.x = (n.x || 0) + (Math.random() * 10 - 5);
                        n.y = (n.y || 0) + (Math.random() * 10 - 5);
                    }
                });
            }
            // Just resume the async loop rather than completely rebuilding the physics solver
            colaLayout.resume();
            running = true;
        }
    }

    // Rebuild when graph structure changes
    let lastStructureKey = '';
    let lastColaParams = '';
    $: {
        const sk = $graphStructureKey;
        const cp = `${$viewSettings.colaLinkLength}|${$viewSettings.colaConvergence}|${$viewSettings.colaHandleDisconnected}|${$viewSettings.colaGroupPadding}|${$viewSettings.colaLinkDistIntraParent}|${$viewSettings.colaLinkWeightIntraParent}|${$viewSettings.colaLinkDistInterParent}|${$viewSettings.colaLinkWeightInterParent}|${$viewSettings.colaLinkDistDependsOn}|${$viewSettings.colaLinkWeightDependsOn}|${$viewSettings.colaLinkDistRef}|${$viewSettings.colaLinkWeightRef}|${$filters.edgeDependencies}|${$filters.edgeReferences}|${$filters.edgeParent}`;
        if (containerGroup && $graphData && nodesLayer && hullLayer && (sk !== lastStructureKey || cp !== lastColaParams)) {
            lastStructureKey = sk;
            lastColaParams = cp;
            rebuild();
        }
    }

    // ─── Group building ────────────────────────────────────────────────────────

    /**
     * Builds WebCola flat groups (no nesting).
     * 
     * ARCHITECTURE NOTES:
     * 1. Source of Truth: We MUST use `_safe_parent` from the node objects. The 
     *    `n.parent` string is mutated by WebCola into a circular Group object reference 
     *    during the first physics tick, which destroys Svelte's ability to rebuild the hierarchy.
     * 
     * 2. Integer Array Indices: WebCola strictly requires `leaves` to be integer 
     *    indices into the `activeNodes` array, NOT object references.
     * 
     * 3. Single Group Membership: A leaf node can only be a member of AT MOST one group.
     *    To simplify and prevent impossible constraints, we assign every node to exactly one group:
     *    - If a node is a parent -> it is a leaf in its OWN group.
     *    - If a node is NOT a parent -> it is a leaf in its DIRECT parent's group.
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
            
            // Collect leaves for this group:
            // 1. The parent node itself
            // 2. Direct children that are NOT parents themselves (to satisfy Single Group Membership)
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
            // WebCola's bounds are calculated asynchronously during the simulation.
            // If they are missing, we skip drawing until the next tick.
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

        // Build flat groups
        colaGroups = buildColaGroups(nodes, links);

        // Canvas from CANVAS_AREA
        const svg = containerGroup?.ownerSVGElement;
        const vw = svg?.clientWidth || window.innerWidth || 1400;
        const vh = svg?.clientHeight || window.innerHeight || 900;
        const aspect = vw / vh;
        const ch = Math.round(Math.sqrt(CANVAS_AREA / aspect));
        const cw = Math.round(ch * aspect);

        // Simple random distribution across the canvas area for initial unconstrained layout
        nodes.forEach((n: any) => {
            if (typeof n.x !== 'number' || n.x < -9000) {
                n.x = (Math.random() * cw * 0.8) + (cw * 0.1);
                n.y = (Math.random() * ch * 0.8) + (ch * 0.1);
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

        // Physics links: apply visibility filters and physics settings per link type
        const colaLinks = links.filter((l: any) => {
            if (typeof l.source !== 'object' || typeof l.target !== 'object') return false;
            
            // Check visibility filters from the legend
            if (l.type === 'parent' && $filters.edgeParent === 'hidden') return false;
            if (l.type === 'depends_on' && $filters.edgeDependencies === 'hidden') return false;
            if (l.type === 'ref' && $filters.edgeReferences === 'hidden') return false;

            return true;
        }).map((l: any) => {
            // Apply physics settings
            let length = 1000;
            let weight = 1.0;
            if (l.type === 'parent') {
                // If the target (child) is a parent itself, it's in its own group (inter-group link).
                // If it's not a parent, it's inside the source's group (intra-group link).
                const isChildParent = nodes.some((n: any) => n._safe_parent === l.target.id);
                if (isChildParent) {
                    length = $viewSettings.colaLinkDistInterParent;
                    weight = $viewSettings.colaLinkWeightInterParent;
                } else {
                    length = $viewSettings.colaLinkDistIntraParent;
                    weight = $viewSettings.colaLinkWeightIntraParent;
                }
            } else if (l.type === 'depends_on') {
                length = $viewSettings.colaLinkDistDependsOn;
                weight = $viewSettings.colaLinkWeightDependsOn;
            } else if (l.type === 'ref') {
                length = $viewSettings.colaLinkDistRef;
                weight = $viewSettings.colaLinkWeightRef;
            }
            return { ...l, length, weight };
        });

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
            .linkDistance((l: any) => l.length)
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
            .start(0, 0, 0); 
        running = true;

        // Force initial render
        tickVisuals();
    }

    export function randomize() {
        if (!$graphData || !colaLayout) return;
        
        $graphData.nodes.forEach((n: any) => {
            if (!n.fixed) {
                // Strong jitter rather than complete random repositioning
                n.x = (n.x || 0) + (Math.random() * 200 - 100);
                n.y = (n.y || 0) + (Math.random() * 200 - 100);
                n.px = n.x;
                n.py = n.y;
            }
        });
        tickVisuals();
        
        if (running) {
            ticks = 0;
            colaLayout.alpha(1); // Inject high heat instead of the standard 0.1 from resume()
        } else {
            colaLayout.alpha(1);
            running = true;
        }
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
