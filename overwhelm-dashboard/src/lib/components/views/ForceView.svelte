<script lang="ts">
    import * as d3 from "d3";
    import * as cola from "webcola";
    import { onMount, onDestroy } from "svelte";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { viewSettings } from "../../stores/viewSettings";
    import { filters, type EdgeVisibility } from "../../stores/filters";
    import { selection, toggleSelection } from "../../stores/selection";
    import { buildTaskCardNode } from "../shared/NodeShapes";
    import { routeSfdpEdges } from "../shared/EdgeRenderer";
    import { FORCE_CONFIG } from "../../data/constants";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";

    export let containerGroup: SVGGElement;

    let nodesLayer: SVGGElement;
    let edgesLayer: SVGGElement;
    let hullLayer: SVGGElement;

    let colaLayout: (cola.Layout & cola.ID3StyleLayoutAdaptor) | null = null;
    // Track cleanup and frame loop
    let frameId = 0;

    // Full physics rebuild only when structure (node/link set) or Cola params change
    let lastStructureKey = '';
    let lastColaParams = '';
    $: {
        const sk = $graphStructureKey;
        const cp = `${$viewSettings.colaLinkLength}|${$viewSettings.colaFlowSep}|${$viewSettings.colaGroupPadding}`;
        if (
            containerGroup &&
            $graphData &&
            nodesLayer &&
            edgesLayer &&
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
                // Sync mutable display properties
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
                const activeMatch = !!(activeId && (sid === activeId || tid === activeId));
                return hoverMatch || selMatch || activeMatch;
            });
        } else {
            nEls.classed("dimmed", false).classed("illuminated", false);
            eEls.classed("dimmed", false).classed("illuminated", false);
        }
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
    $: {
        // Touch specific filter values so Svelte tracks them as dependencies
        const _ep = $filters.edgeParent;
        const _ed = $filters.edgeDependencies;
        const _er = $filters.edgeReferences;
        if (edgesLayer) {
            const eEls = d3.select(edgesLayer).selectAll("path");
            if (!eEls.empty()) {
                applyEdgeVisibility(eEls);
            }
        }
    }

    function updateHulls() {
        // Old polygon hulls removed — Cola group bounding boxes handle this now
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
                    .on("start", (e, d: any) => {
                        cola.Layout.dragStart(d);
                    })
                    .on("drag", (e, d: any) => {
                        d.x = e.x;
                        d.y = e.y;
                        if (colaLayout) colaLayout.resume();
                    })
                    .on("end", (e, d: any) => {
                        d.fixed = 0;
                    }),
            );
    }

    function tickVisuals() {
        // --- Custom Force: Keep epics and child tasks closely packed ---
        if ($graphData && parentOf) {
            const nodeMap = new Map<string, any>();
            $graphData.nodes.forEach((n: any) => nodeMap.set(n.id, n));

            const CONTAINER_TYPES = new Set(['epic', 'project', 'goal']);

            $graphData.nodes.forEach((n: any) => {
                if (CONTAINER_TYPES.has(n.type)) return;
                
                let cur = n.id;
                let targetContainer = null;
                for (let i = 0; i < 20; i++) {
                    const pid = parentOf.get(cur);
                    if (!pid) break;
                    const pNode = nodeMap.get(pid);
                    if (pNode && CONTAINER_TYPES.has(pNode.type)) {
                        targetContainer = pNode;
                        break;
                    }
                    cur = pid;
                }

                if (targetContainer && typeof targetContainer.x === 'number' && typeof targetContainer.y === 'number') {
                    const dx = targetContainer.x - n.x;
                    const dy = targetContainer.y - n.y;
                    // Apply a strong attractive spring force towards the container centre
                    n.x += dx * 0.08;
                    n.y += dy * 0.08;
                }
            });
        }

        d3.select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .attr("transform", (d) => `translate(${d.x ?? 0},${d.y ?? 0})`);

        const eEls = d3.select(edgesLayer).selectAll<SVGPathElement, GraphEdge>("path");
        routeSfdpEdges(eEls);
        applyEdgeVisibility(eEls);

        // Render group bounding boxes from WebCola (skip unlabelled catch-all group)
        if (hullLayer && colaLayout) {
            const groups = (colaLayout.groups() || []).filter((g: any) => g.label);
            const groupEls = d3.select(hullLayer)
                .selectAll<SVGRectElement, any>("rect.cola-group")
                .data(groups);

            groupEls.join("rect")
                .attr("class", "cola-group")
                .attr("rx", 8).attr("ry", 8)
                .attr("x", (d: any) => d.bounds?.x ?? 0)
                .attr("y", (d: any) => d.bounds?.y ?? 0)
                .attr("width", (d: any) => d.bounds?.width() ?? 0)
                .attr("height", (d: any) => d.bounds?.height() ?? 0)
                .attr("fill", (d: any, i: number) => {
                    const hue = (i * 47) % 360;
                    return `hsla(${hue}, 40%, 50%, 0.08)`;
                })
                .attr("stroke", (d: any, i: number) => {
                    const hue = (i * 47) % 360;
                    return `hsla(${hue}, 40%, 50%, 0.3)`;
                })
                .attr("stroke-width", 1.5)
                .attr("stroke-dasharray", "6,3")
                .style("pointer-events", "none");

            // Epic group labels
            const labelEls = d3.select(hullLayer)
                .selectAll<SVGTextElement, any>("text.cola-group-label")
                .data(groups);

            labelEls.join("text")
                .attr("class", "cola-group-label")
                .attr("x", (d: any) => (d.bounds?.x ?? 0) + 12)
                .attr("y", (d: any) => (d.bounds?.y ?? 0) + 24)
                .text((d: any) => d.label || "")
                .attr("font-size", 18)
                .attr("font-weight", 700)
                .attr("fill", (d: any, i: number) => {
                    const hue = (i * 47) % 360;
                    return `hsla(${hue}, 40%, 35%, 0.7)`;
                })
                .style("pointer-events", "none")
                .style("text-transform", "uppercase")
                .style("letter-spacing", "0.05em");
        }
    }

    function drawForceAndStartPhysics() {
        if (!$graphData) return;

        // Setup DOM elements
        if (colaLayout) { colaLayout.stop(); colaLayout = null; }

        const data = $graphData;
        const cw = 5000, ch = 3000;

        const nEls = d3
            .select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .data(data.nodes, (d) => d.id)
            .join("g")
            .attr("class", "node")
            .attr("transform", (d) => `translate(${d.x ?? 0},${d.y ?? 0})`);

        const activeId = $selection.activeNodeId;
        nEls.each(function (d) {
            const g = d3.select(this) as any;
            const isSelected = d.id === activeId;
            const lastSelected = (d as any)._lastSelected;
            const isEmpty = g.selectAll("*").empty();
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

        applyEdgeVisibility(eEls);
        routeSfdpEdges(eEls);
        updateHulls();

        // Start WebCola layout with hierarchical grouping
        const fc = FORCE_CONFIG;

        // Resolve string IDs to node references for all edges
        const nodeById = new Map($graphData.nodes.map(n => [n.id, n]));
        const nodeIndex = new Map($graphData.nodes.map((n, i) => [n.id, i]));
        $graphData.links.forEach((l: any) => {
            if (typeof l.source === 'string') l.source = nodeById.get(l.source) || l.source;
            if (typeof l.target === 'string') l.target = nodeById.get(l.target) || l.target;
        });

        // Set width/height on nodes for avoidOverlaps (generous padding to prevent edges overlapping)
        $graphData.nodes.forEach((n: any) => {
            n.width = n.w + 40;
            n.height = n.h + 30;
        });

        // Build flat groups by project — one group per project containing all its nodes.
        // No nesting, so Cola treats this as a force layout with non-overlapping clusters.
        const parentLinks = $graphData.links.filter((l: any) => l.type === 'parent');

        // Build parent lookup: child ID → parent node
        const parentOf = new Map<string, GraphNode>();
        for (const l of parentLinks) {
            const pid = typeof l.source === 'object' ? l.source.id : l.source;
            const cid = typeof l.target === 'object' ? l.target.id : l.target;
            const pNode = nodeById.get(pid);
            if (pNode) parentOf.set(cid, pNode);
        }

        // Group by nearest epic/project ancestor — ALL nodes must belong to a group
        // so that avoidOverlaps prevents non-descendants from entering epic containers
        const CONTAINER_TYPES = new Set(['epic', 'project', 'goal']);
        function findContainer(nodeId: string): string | null {
            let cur = nodeId;
            let depth = 0;
            while (depth < 20) {
                const p = parentOf.get(cur);
                if (!p) break;
                if (CONTAINER_TYPES.has(p.type)) return p.id;
                cur = p.id;
                depth++;
            }
            return null;
        }

        const containerMembers = new Map<string, number[]>();
        const ungroupedIndices: number[] = [];
        $graphData.nodes.forEach((n, i) => {
            const containerId = CONTAINER_TYPES.has(n.type) ? n.id : findContainer(n.id);
            if (containerId === null) {
                ungroupedIndices.push(i);
                return;
            }
            if (!containerMembers.has(containerId)) containerMembers.set(containerId, []);
            containerMembers.get(containerId)!.push(i);
        });

        const groupPadding = $viewSettings.colaGroupPadding;
        const colaGroups: any[] = [];
        for (const [containerId, members] of containerMembers) {
            if (members.length >= 2) {
                const containerNode = nodeById.get(containerId);
                const label = containerNode?.label || containerNode?.fullTitle || containerId;
                colaGroups.push({ leaves: members, padding: groupPadding, label });
            } else {
                // Single-member groups: add members to ungrouped
                ungroupedIndices.push(...members);
            }
        }
        // Put ungrouped nodes in a catch-all group so Cola keeps them outside epic containers
        if (ungroupedIndices.length > 0) {
            colaGroups.push({ leaves: ungroupedIndices, padding: groupPadding, label: '' });
        }

        // Initial positions: randomize group centers, place all members at their group center.
        // This lets Cola's overlap avoidance expand each cluster outward from a shared origin,
        // producing cleaner separation than fully random initial positions.
        const pad = 200;
        colaGroups.forEach(group => {
            const cx = pad + Math.random() * (cw - 2 * pad);
            const cy = pad + Math.random() * (ch - 2 * pad);
            (group.leaves as number[]).forEach(idx => {
                const n = data.nodes[idx] as any;
                n.x = cx;
                n.y = cy;
            });
        });
        // Any nodes not in any group (shouldn't happen, but safety)
        data.nodes.forEach((d: any) => {
            if (typeof d.x !== 'number') d.x = cw / 2;
            if (typeof d.y !== 'number') d.y = ch / 2;
        });

        // Build cola links from parent edges (index-based)
        const colaLinks = parentLinks.map((l: any) => {
            const si = nodeIndex.get(typeof l.source === 'object' ? l.source.id : l.source)!;
            const ti = nodeIndex.get(typeof l.target === 'object' ? l.target.id : l.target)!;
            return { source: si, target: ti };
        }).filter((l: any) => l.source !== undefined && l.target !== undefined);

        console.log(`[Cola] ${$graphData.nodes.length} nodes, ${colaLinks.length} links, ${colaGroups.length} groups`, colaGroups.map(g => g.leaves.length));

        colaLayout = cola.d3adaptor(d3)
            .size([cw, ch])
            .nodes($graphData.nodes as any)
            .links(colaLinks)
            .groups(colaGroups)
            .avoidOverlaps(true)
            .handleDisconnected(true)
            .symmetricDiffLinkLengths($viewSettings.colaLinkLength, 0.7)
            .on("tick", tickVisuals)
            .start(80, 80, 80);
    }

    onDestroy(() => {
        if (colaLayout) colaLayout.stop();
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
