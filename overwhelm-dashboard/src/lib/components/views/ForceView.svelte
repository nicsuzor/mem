<script lang="ts">
    import * as d3 from "d3";
    import { onMount, onDestroy } from "svelte";
    import { graphData } from "../../stores/graph";
    import { viewSettings } from "../../stores/viewSettings";
    import { filters, type EdgeVisibility } from "../../stores/filters";
    import { selection, toggleSelection } from "../../stores/selection";
    import { buildTaskCardNode } from "../shared/NodeShapes";
    import { routeForceEdges, routeSfdpEdges } from "../shared/EdgeRenderer";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";

    export let containerGroup: SVGGElement;

    let nodesLayer: SVGGElement;
    let edgesLayer: SVGGElement;
    let hullLayer: SVGGElement;

    let simulation: d3.Simulation<GraphNode, GraphEdge> | null = null;
    // Track cleanup and frame loop
    let frameId = 0;

    $: {
        if (
            containerGroup &&
            $graphData &&
            nodesLayer &&
            edgesLayer &&
            hullLayer
        ) {
            drawForceAndStartPhysics();
        }
    }

    // Pre-built adjacency map for O(1) neighbor lookup during hover
    let adjacencyMap = new Map<string, Set<string>>();
    $: if ($graphData) {
        const adj = new Map<string, Set<string>>();
        $graphData.links.forEach((l: any) => {
            const sid = l.source.id || l.source;
            const tid = l.target.id || l.target;
            if (!adj.has(sid)) adj.set(sid, new Set());
            if (!adj.has(tid)) adj.set(tid, new Set());
            adj.get(sid)!.add(tid);
            adj.get(tid)!.add(sid);
        });
        adjacencyMap = adj;
    }

    // Build parent-child maps for hierarchy walking
    let parentOf = new Map<string, string>();   // child → parent
    let childrenOf = new Map<string, Set<string>>(); // parent → children
    $: if ($graphData) {
        const pOf = new Map<string, string>();
        const cOf = new Map<string, Set<string>>();
        $graphData.links.forEach((l: any) => {
            if (l.type !== 'parent') return;
            // parent edges are flipped: source=parent, target=child
            const pid = l.source.id || l.source;
            const cid = l.target.id || l.target;
            pOf.set(cid, pid);
            if (!cOf.has(pid)) cOf.set(pid, new Set());
            cOf.get(pid)!.add(cid);
        });
        parentOf = pOf;
        childrenOf = cOf;
    }

    // Collect full hierarchy (ancestors + descendants) for a node
    function getHierarchy(nodeId: string): Set<string> {
        const result = new Set<string>([nodeId]);
        // Walk up ancestors
        let cur = nodeId;
        while (parentOf.has(cur)) {
            cur = parentOf.get(cur)!;
            result.add(cur);
        }
        // Walk down descendants (BFS)
        const queue = [nodeId];
        while (queue.length > 0) {
            const id = queue.shift()!;
            const kids = childrenOf.get(id);
            if (kids) for (const kid of kids) {
                if (!result.has(kid)) {
                    result.add(kid);
                    queue.push(kid);
                }
            }
        }
        return result;
    }

    // Flashlight Hover Effect Logic — uses pre-built adjacency for O(1) lookups
    $: if (nodesLayer && edgesLayer && $graphData) {
        const hoveredId = $selection.hoveredNodeId;
        const activeId = $selection.activeNodeId;
        const nEls = d3.select(nodesLayer).selectAll(".node");
        const eEls = d3.select(edgesLayer).selectAll("path");

        // Hierarchy + direct neighbors (deps/refs) of selected node — always at full opacity
        const hierarchyIds = activeId ? getHierarchy(activeId) : new Set<string>();
        // Add direct adjacency (dependencies, references) of the selected node
        const selectedNeighbors = new Set<string>(hierarchyIds);
        if (activeId) {
            const adj = adjacencyMap.get(activeId);
            if (adj) adj.forEach(id => selectedNeighbors.add(id));
        }

        nEls.classed("selected-node", (d: any) => selectedNeighbors.has(d.id));

        if (hoveredId) {
            const neighbors = new Set<string>([hoveredId]);
            const adj = adjacencyMap.get(hoveredId);
            if (adj) adj.forEach(id => neighbors.add(id));

            nEls.classed("dimmed", (d: any) => !neighbors.has(d.id) && !selectedNeighbors.has(d.id))
                .classed("illuminated", (d: any) => neighbors.has(d.id) || selectedNeighbors.has(d.id));

            eEls.classed("dimmed", (l: any) => {
                const sid = l.source.id || l.source;
                const tid = l.target.id || l.target;
                const hoverMatch = sid === hoveredId || tid === hoveredId;
                const selMatch = (selectedNeighbors.has(sid) && selectedNeighbors.has(tid));
                // Also highlight edges touching the selected node directly
                const activeMatch = activeId && (sid === activeId || tid === activeId);
                return !hoverMatch && !selMatch && !activeMatch;
            }).classed("illuminated", (l: any) => {
                const sid = l.source.id || l.source;
                const tid = l.target.id || l.target;
                const hoverMatch = sid === hoveredId || tid === hoveredId;
                const selMatch = (selectedNeighbors.has(sid) && selectedNeighbors.has(tid));
                const activeMatch = activeId && (sid === activeId || tid === activeId);
                return hoverMatch || selMatch || activeMatch;
            });
        } else {
            nEls.classed("dimmed", false).classed("illuminated", false);
            eEls.classed("dimmed", false).classed("illuminated", false);
        }
    }

    function projectColor(projectId: string) {
        let hash = 0;
        for (let i = 0; i < projectId.length; i++) {
            hash = (hash << 5) - hash + projectId.charCodeAt(i);
            hash |= 0;
        }
        const hue = Math.abs(hash) % 360;
        return `hsl(${hue}, 55%, 52%)`;
    }

    function edgeVisForType(type: string): EdgeVisibility {
        if (type === 'parent') return $filters.edgeParent;
        if (type === 'depends_on') return $filters.edgeDependencies;
        return $filters.edgeReferences; // ref, soft_depends_on, etc.
    }

    function edgeOpacity(vis: EdgeVisibility): number {
        if (vis === 'bright') return 0.85;
        if (vis === 'half') return 0.25;
        return 0;
    }

    function applyEdgeVisibility(eEls: any) {
        eEls.attr("opacity", (d: any) => edgeOpacity(edgeVisForType(d.type)));
    }

    // Reactively update edge visibility when filters change (no redraw needed)
    $: if (edgesLayer && $filters) {
        const eEls = d3.select(edgesLayer).selectAll("path");
        if (!eEls.empty()) {
            applyEdgeVisibility(eEls);
        }
    }

    function updateHulls() {
        if (!$graphData || !hullLayer) return;

        const projectNodes = new Map<string, [number, number][]>();
        $graphData.nodes.forEach((n) => {
            if (
                typeof n.x !== "number" ||
                typeof n.y !== "number" ||
                n.x < -9000
            )
                return;
            const p = n.project;
            if (!p) return;
            if (!projectNodes.has(p)) projectNodes.set(p, []);
            projectNodes.get(p)!.push([n.x, n.y]);
        });

        const hullData: any[] = [];
        projectNodes.forEach((pts, pid) => {
            if (pts.length < 3) return;
            const hull = d3.polygonHull(pts);
            if (!hull) return;
            // Expand hull
            const cx = d3.mean(hull, (p) => p[0]) || 0;
            const cy = d3.mean(hull, (p) => p[1]) || 0;
            const expanded = hull.map(([x, y]) => {
                const dx = x - cx,
                    dy = y - cy;
                const dist = Math.sqrt(dx * dx + dy * dy) || 1;
                return [x + (dx / dist) * 40, y + (dy / dist) * 40];
            });
            hullData.push({ id: pid, points: expanded, cx, cy });
        });

        d3.select(hullLayer)
            .selectAll(".hull-path")
            .data(hullData, (d: any) => d.id)
            .join("path")
            .attr("class", "hull-path")
            .attr(
                "d",
                (d) =>
                    "M" + d.points.map((p: any) => p.join(",")).join("L") + "Z",
            )
            .attr("fill", (d) => projectColor(d.id))
            .attr("fill-opacity", 0.15)
            .attr("stroke", (d) => projectColor(d.id))
            .attr("stroke-opacity", 0.45)
            .attr("stroke-width", 2)
            .attr("stroke-dasharray", "5,3")
            .style("pointer-events", "none");

        d3.select(hullLayer)
            .selectAll(".hull-label")
            .data(hullData, (d: any) => d.id)
            .join("text")
            .attr("class", "hull-label")
            .attr("x", (d) => d.cx)
            .attr("y", (d) => Number(d3.min(d.points, (p: any) => p[1]) || 0) - 12)
            .attr("text-anchor", "middle")
            .attr("font-size", "14px")
            .attr("font-weight", "700")
            .attr("font-family", "var(--font-mono), monospace")
            .attr("fill", (d) => projectColor(d.id))
            .attr("opacity", 0.7)
            .attr("letter-spacing", "2px")
            .style("pointer-events", "none")
            .style("user-select", "none")
            .style("text-shadow", "0 1px 6px rgba(0,0,0,0.8)")
            .text((d) =>
                d.id.replace(/_/g, " ").toUpperCase().substring(0, 30),
            );
    }

    function bindDragAndClick(nEls: any) {
        nEls.style("cursor", "crosshair")
            .on("click", (e: any, d: any) => {
                e.stopPropagation();
                toggleSelection(d.id);
            })
            .on("mouseenter", (e: any, d: any) => {
                selection.update((s) => ({ ...s, hoveredNodeId: d.id }));
            })
            .on("mouseleave", (e: any, d: any) => {
                selection.update((s) => ({ ...s, hoveredNodeId: null }));
            })
            .call(
                d3
                    .drag<SVGGElement, GraphNode>()
                    .on("start", (e, d) => {
                        if (simulation) simulation.alphaTarget(0.1).restart();
                        d.fx = d.x;
                        d.fy = d.y;
                    })
                    .on("drag", (e, d) => {
                        d.fx = e.x;
                        d.fy = e.y;
                    })
                    .on("end", (e, d) => {
                        if (simulation) simulation.alphaTarget(0);
                        d.fx = null;
                        d.fy = null;
                    }),
            );
    }

    function drawStaticForce() {
        if (!$graphData) return;
        if (simulation) {
            simulation.stop();
            simulation = null;
        }

        const data = $graphData;
        // Map initial precomputed SFDP layout
        data.nodes.forEach((d) => {
            d.x =
                d.layouts?.sfdp?.x ||
                d.x ||
                Math.random() * 500;
            d.y =
                d.layouts?.sfdp?.y ||
                d.y ||
                Math.random() * 500;
        });

        const nEls = d3
            .select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .data(data.nodes, (d) => d.id)
            .join("g")
            .attr("class", "node")
            .attr("transform", (d) => `translate(${d.x},${d.y})`);

        const activeId = $selection.activeNodeId;
        nEls.each(function (d) {
            const g = d3.select(this) as any;
            const isSelected = d.id === activeId;
            const lastSelected = (d as any)._lastSelected;
            const isEmpty = g.selectAll("*").empty();
            // Only rebuild DOM when selection state changes or node is new
            if (isEmpty || lastSelected !== isSelected) {
                g.selectAll("*").remove();
                buildTaskCardNode(g, d, isSelected);
                (d as any)._lastSelected = isSelected;
            }
        });

        bindDragAndClick(nEls);

        const eEls = d3
            .select(edgesLayer)
            .selectAll<SVGPathElement, GraphEdge>("path")
            .data(data.links)
            .join("path")
            .attr("fill", "none")
            .attr("stroke", (d: any) => d.color)
            .attr("stroke-width", (d: any) => d.width)
            .attr("stroke-dasharray", (d: any) => d.dash)
            .attr("marker-end", "url(#ar)")
            .attr("stroke-linecap", "round")
            .attr("stroke-linejoin", "round")
            .attr("class", "force-edge");

        // Apply edge visibility from filters
        applyEdgeVisibility(eEls);

        if ($viewSettings.viewMode === "Force") {
            routeSfdpEdges(eEls);
        } else {
            routeForceEdges(eEls);
        }
        updateHulls();
    }

    function tickVisuals() {
        d3.select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .attr("transform", (d) => `translate(${d.x},${d.y})`);

        const eEls = d3.select(edgesLayer).selectAll<SVGPathElement, GraphEdge>("path");
        if ($viewSettings.viewMode === "Force") {
            // Use straight lines during live simulation; bundles recomputed when it cools
            routeSfdpEdges(eEls);
        } else {
            routeForceEdges(eEls);
        }
        updateHulls();
    }

    function drawForceAndStartPhysics() {
        if (!$graphData) return;

        // Setup elements
        drawStaticForce();

        // Start Simulation
        if (simulation) simulation.stop();

        const fc = $graphData.forceConfig;
        const cw = 1200,
            ch = 800; // Expected center bounds

        // Separate parent (structural) and dependency (cross-link) edges
        const parentLinks = $graphData.links.filter((l: any) => l.type === 'parent');
        const depLinks = $graphData.links.filter((l: any) => l.type !== 'parent');

        simulation = d3
            .forceSimulation<GraphNode, GraphEdge>($graphData.nodes)
            .force(
                "link-parent",
                d3
                    .forceLink<GraphNode, GraphEdge>(parentLinks)
                    .id((d) => d.id)
                    .distance((d) => d.distance * (fc.linkDistMult || 1.0) * $viewSettings.linkDistance)
                    .strength(0.9),  // Strong: keeps hierarchical spine rigid
            )
            .force(
                "link-dep",
                d3
                    .forceLink<GraphNode, GraphEdge>(depLinks)
                    .id((d) => d.id)
                    .distance((d) => d.distance * (fc.linkDistMult || 1.0) * $viewSettings.linkDistance)
                    .strength(0.05), // Weak: renders the line but doesn't pull nodes out of clusters
            )
            .force(
                "charge",
                d3
                    .forceManyBody<GraphNode>()
                    .strength(
                        (d) =>
                            (d.charge || -100) *
                            (fc.chargeMult || 1.0) *
                            $viewSettings.chargeStrength,
                    )
                    .distanceMax(fc.chargeDistanceMax || 280),
            )
            .force(
                "collide",
                d3
                    .forceCollide<GraphNode>()
                    .radius(
                        (d) =>
                            (Math.max(d.w / 2, d.h / 2) +
                            (fc.collisionPadding || 2)) * $viewSettings.collisionRadius,
                    )
                    .strength(fc.collisionStrength || 0.4)
                    .iterations(fc.collisionIterations || 3),
            )
            .force("center", d3.forceCenter(cw / 2, ch / 2).strength($viewSettings.gravity));

        // Radial force — pulls nodes toward concentric rings by hierarchy depth
        {
            const depthMap = new Map<string, number>();
            const childToParent = new Map<string, string>();
            for (const l of parentLinks) {
                const tid = typeof l.target === 'object' ? (l.target as any).id : l.target;
                const sid = typeof l.source === 'object' ? (l.source as any).id : l.source;
                childToParent.set(tid, sid);
            }
            for (const n of $graphData.nodes) {
                let depth = 0;
                let cur = n.id;
                while (childToParent.has(cur) && depth < 20) {
                    cur = childToParent.get(cur)!;
                    depth++;
                }
                depthMap.set(n.id, depth);
            }
            const maxDepth = Math.max(...depthMap.values(), 1);
            const radiusScale = Math.min(cw, ch) * 0.4;

            simulation.force(
                "radial",
                d3.forceRadial<GraphNode>(
                    (d) => ((depthMap.get(d.id) || 0) / maxDepth) * radiusScale + 50,
                    cw / 2,
                    ch / 2,
                ).strength(0.3),
            );
        }

        simulation
            .alphaDecay($viewSettings.alphaDecay)
            .velocityDecay($viewSettings.velocityDecay);

        // Warm up ticks — run longer to let layout stabilize
        const warmup = fc.warmupTicks || 300;
        for (let i = 0; i < warmup; i++) {
            simulation.tick();
        }

        // Live loop
        simulation.on("tick", tickVisuals);
    }

    onDestroy(() => {
        if (simulation) simulation.stop();
        cancelAnimationFrame(frameId);
    });
</script>

{#if containerGroup}
    <g bind:this={hullLayer} class="hull-layer"></g>
    <g bind:this={edgesLayer}></g>
    <g bind:this={nodesLayer}></g>
{/if}

<style>
    /* Flashlight depth-of-field state classes */
    :global(g.node) {
        transition:
            opacity 0.3s cubic-bezier(0.4, 0, 0.2, 1),
            filter 0.3s ease;
    }
    :global(g.node.dimmed) {
        opacity: 0.6;
        filter: grayscale(0.5) brightness(0.75);
    }
    :global(g.node.illuminated) {
        opacity: 1;
        filter: drop-shadow(0 0 16px var(--color-primary));
    }
    :global(path.force-edge) {
        transition:
            opacity 0.3s ease,
            stroke-width 0.3s ease,
            stroke 0.3s ease;
    }
    :global(path.force-edge.dimmed) {
        opacity: 0.3;
    }
    :global(path.force-edge.illuminated) {
        opacity: 1;
        stroke: var(--color-primary);
        stroke-width: 2px;
    }
    /* Selected (clicked) node always at full opacity */
    :global(g.node.selected-node) {
        opacity: 1 !important;
        filter: none !important;
    }
    :global(path.force-edge.intent-edge-dim) {
        opacity: 0.15;
    }
</style>
