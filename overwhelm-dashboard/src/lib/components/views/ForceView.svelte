<script lang="ts">
    import * as d3 from "d3";
    import { onMount, onDestroy } from "svelte";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { viewSettings } from "../../stores/viewSettings";
    import { filters, type VisibilityState } from "../../stores/filters";
    import { selection, toggleSelection } from "../../stores/selection";
    import { buildTaskCardNode } from "../shared/NodeShapes";
    import { projectHue } from "../../data/projectUtils";
    import { routeSfdpEdges, setEdgeObstacles } from "../shared/EdgeRenderer";
    import { zoomScale } from "../../stores/zoom";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";

    // Module-level constant — avoids allocating a new Set on every tick
    const CONTAINER_TYPES = new Set(['epic']);

    export let containerGroup: SVGGElement;

    let nodesLayer: SVGGElement;
    let edgesLayer: SVGGElement;
    let hullLayer: SVGGElement;

    let simulation: d3.Simulation<d3.SimulationNodeDatum, undefined> | null = null;
    let layoutGroups: any[] = [];
    // Container nodes that have a Cola group box — their hexagon is hidden (box replaces it)
    let containerGroupNodeIds = new Set<string>();
    // Track cleanup and frame loop
    let frameId = 0;

    // Pre-computed per-layout data — rebuilt in drawForceAndStartPhysics(), stable across ticks
    let layoutNodeMap: Map<string, any> = new Map();
    let layoutNodeGroupSets: Map<string, Set<string>> = new Map();
    let layoutNodes: GraphNode[] = [];
    // layoutHighPriRelatedIds removed — Metro view owns priority paths.
    let layoutNestedGroupSet: Set<any> = new Set();

    // Full physics rebuild only when structure (node/link set) or Cola params change
    let lastStructureKey = '';
    let lastColaParams = '';
    $: {
        const sk = $graphStructureKey;
        const cp = `${$viewSettings.colaLinkLength}|${$viewSettings.colaFlowSep}|${$viewSettings.colaGroupPadding}|${$viewSettings.colaAvoidOverlaps}|${$viewSettings.colaGroups}|${$viewSettings.colaLinks}|${$viewSettings.colaHandleDisconnected}`;
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

            nEls.classed("dimmed", (d: any) => (!neighbors.has(d.id) && !selectedNeighbors.has(d.id)) || d.filter_dimmed)
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
            nEls.classed("dimmed", (d: any) => d.filter_dimmed).classed("illuminated", false);
            eEls.classed("dimmed", false).classed("illuminated", false);
        }
    }

    function edgeVisForType(type: string): VisibilityState {
        if (type === 'parent') return $filters.edgeParent;
        if (type === 'depends_on') return $filters.edgeDependencies;
        return $filters.edgeReferences; // ref, soft_depends_on, etc.
    }

    function edgeOpacity(vis: VisibilityState): number {
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
                    .clickDistance(4) // px threshold — prevents clicks from triggering drag
                    .on("start", () => {
                        // Defer dragStart to first real drag movement (see "drag" handler)
                    })
                    .on("drag", (e, d: any) => {
                        // Lazy init: only pin/wake Cola once actual movement occurs
                        if (!(d as any)._dragging) {
                            if (simulation) simulation.alphaTarget(0.3).restart();
                            (d as any)._dragging = true;
                        }
                        d.fx = e.x;
                        d.fy = e.y;
                    })
                    .on("end", (e, d: any) => {
                        if ((d as any)._dragging) {
                            d.fx = null;
                            d.fy = null;
                            if (simulation) simulation.alphaTarget(0);
                            (d as any)._dragging = false;
                        }
                    }),
            );
    }

    function tickVisuals() {
        if (!simulation) return;

        // Compute group bounding boxes manually bottom-up
        if (layoutGroups && layoutGroups.length > 0) {
            const computed = new Set();
            function getBounds(g: any) {
                if (computed.has(g)) return g.bounds;
                let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
                
                (g.leaves || []).forEach((idx: number) => {
                    const n = layoutNodes[idx];
                    if (!n) return;
                    const halfW = (n.w || 0) / 2;
                    const halfH = (n.h || 0) / 2;
                    const nx = n.x ?? 0;
                    const ny = n.y ?? 0;
                    if (nx - halfW < minX) minX = nx - halfW;
                    if (ny - halfH < minY) minY = ny - halfH;
                    if (nx + halfW > maxX) maxX = nx + halfW;
                    if (ny + halfH > maxY) maxY = ny + halfH;
                });
                
                (g.groups || []).forEach((child: any) => {
                    const cb = getBounds(child);
                    if (cb) {
                        if (cb.x < minX) minX = cb.x;
                        if (cb.y < minY) minY = cb.y;
                        if (cb.X > maxX) maxX = cb.X;
                        if (cb.Y > maxY) maxY = cb.Y;
                    }
                });

                if (minX !== Infinity) {
                    const pad = g.padding || 0;
                    minX -= pad;
                    minY -= pad;
                    maxX += pad;
                    maxY += pad;
                    g.bounds = {
                        x: minX, y: minY, X: maxX, Y: maxY,
                        width: () => maxX - minX,
                        height: () => maxY - minY
                    };
                } else {
                    g.bounds = null;
                }
                computed.add(g);
                return g.bounds;
            }

            layoutGroups.forEach((g: any) => getBounds(g));
        }

        const scale = $zoomScale;
        d3.select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .attr("transform", (d) => `translate(${d.x ?? 0},${d.y ?? 0})`)
            .each(function (d) {
                // Progressive label reveal: hide text at low zoom, show at high zoom.
                // Epic group labels (rendered separately) are always visible.
                // P0/P1 nodes show labels at moderate zoom; others only when zoomed in.
                const texts = d3.select(this).selectAll("text, tspan");
                if (texts.empty()) return;
                const isHighPri = d.priority <= 1;
                const showLabel = scale > 0.4 || (isHighPri && scale > 0.2);
                texts.attr("opacity", showLabel ? null : 0);
            });

        // Update obstacle data for edge routing from group bounding boxes.
        if (layoutGroups) {
            const groups = layoutGroups.filter((g: any) => g.label && g.bounds);
            const obstacles = groups.map((g: any) => ({
                x: g.bounds.x,
                y: g.bounds.y,
                X: g.bounds.X,
                Y: g.bounds.Y,
                containerId: g.containerId || g.label || '',
            }));
            setEdgeObstacles(obstacles, layoutNodeGroupSets);
        }

        const eEls = d3.select(edgesLayer).selectAll<SVGPathElement, GraphEdge>("path");
        routeSfdpEdges(eEls);
        applyEdgeVisibility(eEls);

        // Priority path highlighting removed — Metro view owns "what are the priority paths?"
        // Force view shows pure topology only.

        // Render group bounding boxes
        if (hullLayer && layoutGroups) {
            const allGroups: any[] = [];
            function extractGroups(gList: any[]) {
                for (const g of gList) {
                    if (g.label) allGroups.push(g);
                    if (g.groups && g.groups.length > 0) extractGroups(g.groups);
                }
            }
            extractGroups(layoutGroups);
            
            // Deduplicate by containerId or label
            const uniqueGroups = Array.from(new Map(allGroups.map(g => [g.containerId || g.label, g])).values());
            
            const TOP_PAD = 60; // Extra visual padding extending upwards for titles

            const groupEls = d3.select(hullLayer)
                .selectAll<SVGRectElement, any>("rect.cola-group")
                .data(uniqueGroups, (d: any) => d.containerId || d.label);

            // layoutNestedGroupSet is pre-computed in drawForceAndStartPhysics — stable per layout.
            groupEls.join("rect")
                .attr("class", "cola-group")
                .attr("rx", (d: any) => layoutNestedGroupSet.has(d) ? 6 : 10)
                .attr("ry", (d: any) => layoutNestedGroupSet.has(d) ? 6 : 10)
                .attr("x", (d: any) => d.bounds?.x ?? 0)
                .attr("y", (d: any) => (d.bounds?.y ?? 0) - TOP_PAD)
                .attr("width", (d: any) => d.bounds?.width() ?? 0)
                .attr("height", (d: any) => (d.bounds?.height() ?? 0) + TOP_PAD)
                .attr("fill", (d: any) => {
                    const hue = projectHue(d.containerId || d.label || '');
                    return layoutNestedGroupSet.has(d)
                        ? `hsla(${hue}, 40%, 50%, 0.05)`
                        : `hsla(${hue}, 40%, 50%, 0.08)`;
                })
                .attr("stroke", (d: any) => {
                    const hue = projectHue(d.containerId || d.label || '');
                    return layoutNestedGroupSet.has(d)
                        ? `hsla(${hue}, 50%, 55%, 0.25)`
                        : `hsla(${hue}, 40%, 50%, 0.3)`;
                })
                .attr("stroke-width", (d: any) => layoutNestedGroupSet.has(d) ? 1 : 2)
                .attr("stroke-dasharray", (d: any) => layoutNestedGroupSet.has(d) ? "4,2" : "6,3")
                .style("cursor", "crosshair")
                .on("click", (e: any, d: any) => {
                    e.stopPropagation();
                    if (d.containerId) toggleSelection(d.containerId);
                })
                .on("mouseenter", (e: any, d: any) => {
                    if (d.containerId) selection.update(s => ({ ...s, hoveredNodeId: d.containerId }));
                })
                .on("mouseleave", () => {
                    selection.update(s => ({ ...s, hoveredNodeId: null }));
                });

            // Header shading rect
            const headerEls = d3.select(hullLayer)
                .selectAll<SVGRectElement, any>("rect.cola-group-header")
                .data(uniqueGroups, (d: any) => d.containerId || d.label);

            headerEls.join("rect")
                .attr("class", "cola-group-header")
                .style("pointer-events", "none")
                .attr("rx", (d: any) => layoutNestedGroupSet.has(d) ? 6 : 10)
                .attr("ry", (d: any) => layoutNestedGroupSet.has(d) ? 6 : 10)
                // Draw a rectangle covering just the top section of the group
                .attr("x", (d: any) => d.bounds?.x ?? 0)
                .attr("y", (d: any) => (d.bounds?.y ?? 0) - TOP_PAD)
                .attr("width", (d: any) => d.bounds?.width() ?? 0)
                .attr("height", Math.max(TOP_PAD + 10, 0))
                .attr("fill", (d: any) => {
                    const hue = projectHue(d.containerId || d.label || '');
                    return `hsla(${hue}, 40%, 15%, 0.6)`; // Darker semi-transparent header
                });

            // Epic group labels
            const labelEls = d3.select(hullLayer)
                .selectAll<SVGTextElement, any>("text.cola-group-label")
                .data(uniqueGroups, (d: any) => d.containerId || d.label);

            labelEls.join("text")
                .attr("class", "cola-group-label")
                .attr("font-size", 18)
                .attr("font-weight", 700)
                .attr("fill", (d: any) => {
                    const hue = projectHue(d.containerId || d.label || '');
                    return `hsla(${hue}, 60%, 80%, 0.9)`; // Brighter text over the dark header
                })
                .style("pointer-events", "none")
                .style("text-transform", "uppercase")
                .style("letter-spacing", "0.05em")
                .each(function (d: any) {
                    const el = d3.select(this);
                    el.selectAll("tspan").remove();
                    const label = (d.label || "").toUpperCase();
                    const bx = (d.bounds?.x ?? 0) + 12;
                    const by = (d.bounds?.y ?? 0) - TOP_PAD + 22;
                    const availW = Math.max(40, (d.bounds?.width() ?? 200) - 24);
                    const charW = 10; // approximate char width at font-size 18
                    const charsPerLine = Math.max(4, Math.floor(availW / charW));
                    const words = label.split(/\s+/);
                    const lines: string[] = [];
                    let current = "";
                    for (const word of words) {
                        const test = current ? `${current} ${word}` : word;
                        if (test.length > charsPerLine && current) {
                            lines.push(current);
                            current = word;
                        } else {
                            current = test;
                        }
                    }
                    if (current) lines.push(current);
                    // Cap at 2 lines with ellipsis
                    const maxLines = 2;
                    const display = lines.slice(0, maxLines);
                    if (lines.length > maxLines) {
                        display[maxLines - 1] = display[maxLines - 1].slice(0, -1) + "…";
                    }
                    display.forEach((line, i) => {
                        el.append("tspan")
                            .attr("x", bx)
                            .attr("y", by + i * 22)
                            .text(line);
                    });
                });
        }
    }

    function drawForceAndStartPhysics() {
        if (!$graphData) return;

        // Setup DOM elements
        if (simulation) { simulation.stop(); simulation = null; }

        const data = $graphData;

        // ForceView-local: strip project nodes — epic group boxes handle visual hierarchy.
        // Reparent children of removed projects to the project's own parent (if any).
        const forceProjectIds = new Set(data.nodes.filter(n => n.type === 'project').map(n => n.id));
        let activeNodes: GraphNode[] = data.nodes;
        let activeLinks: GraphEdge[] = data.links;
        if (forceProjectIds.size > 0) {
            const projParentMap = new Map<string, string | null>(
                data.nodes.filter(n => forceProjectIds.has(n.id)).map(n => [n.id, n.parent])
            );
            activeNodes = data.nodes.map(n => {
                if (forceProjectIds.has(n.id)) return n; // will be removed
                let cur = n.parent;
                const seen = new Set<string>();
                while (cur && forceProjectIds.has(cur)) {
                    if (seen.has(cur)) break;
                    seen.add(cur);
                    cur = projParentMap.get(cur) ?? null;
                }
                return cur !== n.parent ? { ...n, parent: cur } : n;
            }).filter(n => !forceProjectIds.has(n.id));
            activeLinks = data.links.filter((l: any) => {
                const sid = typeof l.source === 'object' ? l.source.id : l.source;
                const tid = typeof l.target === 'object' ? l.target.id : l.target;
                return !forceProjectIds.has(sid) && !forceProjectIds.has(tid);
            });
        }

        layoutNodes = activeNodes;
        layoutNodeMap = new Map(activeNodes.map((n: any) => [n.id, n]));
        // Match canvas aspect ratio to viewport so the layout fills the screen naturally.
        // Read viewport once at layout start — resize is handled by ZoomContainer's fit-to-view.
        const svg = containerGroup?.ownerSVGElement;
        const vw = svg?.clientWidth || window.innerWidth || 1400;
        const vh = svg?.clientHeight || window.innerHeight || 900;
        const aspect = vw / vh;
        const canvasArea = 12_000_000; // total canvas pixels (constant area regardless of ratio)
        const ch = Math.round(Math.sqrt(canvasArea / aspect));
        const cw = Math.round(ch * aspect);

        // --- Phase 1: Resolve IDs and build groups FIRST so we know which nodes are containers ---
        const nodeById = new Map(activeNodes.map(n => [n.id, n]));
        const nodeIndex = new Map(activeNodes.map((n, i) => [n.id, i]));
        activeLinks.forEach((l: any) => {
            if (typeof l.source === 'string') l.source = nodeById.get(l.source) || l.source;
            if (typeof l.target === 'string') l.target = nodeById.get(l.target) || l.target;
        });

        // Set width/height on nodes for avoidOverlaps.
        activeNodes.forEach((n: any) => {
            n.width = n.w;
            n.height = n.h;
        });

        // Build hierarchical nested groups — epics inside projects, sub-epics inside epics, etc.
        // WebCola supports nested groups via the `groups` property on parent groups.
        const parentLinks = activeLinks.filter((l: any) => l.type === 'parent');

        // Build parent lookup: child ID → parent node
        const parentOf = new Map<string, GraphNode>();
        for (const l of parentLinks) {
            const pid = typeof l.source === 'object' ? l.source.id : l.source;
            const cid = typeof l.target === 'object' ? l.target.id : l.target;
            const pNode = nodeById.get(pid);
            if (pNode) parentOf.set(cid, pNode);
        }

        // Find nearest container ancestor for a node
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

        // Identify all container nodes
        const containerNodeIds = new Set<string>();
        activeNodes.forEach(n => {
            if (CONTAINER_TYPES.has(n.type)) containerNodeIds.add(n.id);
        });

        // For each container, find its parent container (for nesting)
        const containerParent = new Map<string, string | null>();
        for (const cid of containerNodeIds) {
            containerParent.set(cid, findContainer(cid));
        }

        // Direct leaves per container: non-container nodes whose nearest container is this one
        // PLUS the container node itself (so it stays inside its own group box)
        const containerLeaves = new Map<string, number[]>();
        const containerChildGroups = new Map<string, string[]>();
        for (const cid of containerNodeIds) {
            containerLeaves.set(cid, []);
            containerChildGroups.set(cid, []);
        }

        // Register container→container parent relationships
        for (const cid of containerNodeIds) {
            const pcid = containerParent.get(cid);
            if (pcid && containerChildGroups.has(pcid)) {
                containerChildGroups.get(pcid)!.push(cid);
            }
        }

        // Assign each node to its nearest container
        const ungroupedIndices: number[] = [];
        activeNodes.forEach((n, i) => {
            if (containerNodeIds.has(n.id)) {
                // Container node is a leaf in its own group
                containerLeaves.get(n.id)!.push(i);
                return;
            }
            const containerId = findContainer(n.id);
            if (containerId && containerLeaves.has(containerId)) {
                containerLeaves.get(containerId)!.push(i);
            } else {
                ungroupedIndices.push(i);
            }
        });

        const groupPadding = $viewSettings.colaGroupPadding;
        const d3Groups: any[] = [];
        const groupIndexMap = new Map<string, number>(); // containerId → index in d3Groups

        // Pass 1: Create all container groups (leaves only, groups wired in pass 2)
        for (const cid of containerNodeIds) {
            const leaves = containerLeaves.get(cid) || [];
            const childContainers = containerChildGroups.get(cid) || [];
            // Skip empty containers (no leaves beyond itself, no child containers)
            if (leaves.length <= 1 && childContainers.length === 0) {
                // Just the container node alone with no children — treat as ungrouped
                ungroupedIndices.push(...leaves);
                continue;
            }
            const containerNode = nodeById.get(cid);
            const label = containerNode?.label || containerNode?.fullTitle || cid;
            // Cola padding is symmetric — must be large enough to cover the visual header
            // (TOP_PAD=60) extending above g.bounds.y. Without this, Cola places non-member
            // nodes in the header zone, causing visual overlap.
            const isNested = containerParent.get(cid) !== null;
            const nestPadding = isNested ? Math.max(40, groupPadding + 30) : Math.max(65, groupPadding + 55);
            const groupIdx = d3Groups.length;
            groupIndexMap.set(cid, groupIdx);
            d3Groups.push({
                leaves,
                groups: [],  // filled in pass 2
                padding: nestPadding,
                label,
                containerId: cid
            });
        }

        // Pass 2: Wire up nested group references (child container groups → parent group)
        for (const cid of containerNodeIds) {
            const pcid = containerParent.get(cid);
            if (pcid && groupIndexMap.has(pcid) && groupIndexMap.has(cid)) {
                const parentGroupIdx = groupIndexMap.get(pcid)!;
                const childGroupIdx = groupIndexMap.get(cid)!;
                d3Groups[parentGroupIdx].groups.push(childGroupIdx);
            }
        }

        // Convert group indices to actual object references for layoutGroups
        layoutGroups = d3Groups.map(g => ({ ...g, groups: [] })); // shadow copy
        d3Groups.forEach((g, i) => {
            layoutGroups[i].groups = (g.groups as number[]).map(idx => layoutGroups[idx]);
        });

        // Mark which container nodes have a group box — their node visual will be hidden
        containerGroupNodeIds = new Set(groupIndexMap.keys());

        // Put ungrouped nodes in a catch-all group
        if (ungroupedIndices.length > 0) {
            layoutGroups.push({ leaves: ungroupedIndices, groups: [], padding: groupPadding, label: '' });
        }

        // Pre-compute node→group membership for edge routing (avoids per-tick allocation).
        layoutNodeGroupSets = new Map();
        {
            function addNGS(nodeId: string, gId: string) {
                if (!layoutNodeGroupSets.has(nodeId)) layoutNodeGroupSets.set(nodeId, new Set());
                layoutNodeGroupSets.get(nodeId)!.add(gId);
            }
            function buildGroupMembership(g: any, ancestorIds: string[]) {
                const gId = g.containerId || g.label || '';
                (g.leaves || []).forEach((leaf: any) => {
                    const nodeId = typeof leaf === 'number' ? activeNodes[leaf]?.id : (leaf.id || leaf);
                    if (nodeId) {
                        addNGS(nodeId, gId);
                        for (const aid of ancestorIds) addNGS(nodeId, aid);
                    }
                });
                (g.groups || []).forEach((childGroup: any) => {
                    buildGroupMembership(childGroup, [...ancestorIds, gId]);
                });
            }
            const nestedResolved = new Set<any>();
            layoutGroups.forEach(g => (g.groups || []).forEach((c: any) => nestedResolved.add(c)));
            layoutGroups.forEach(g => {
                if (!nestedResolved.has(g) && g.label) buildGroupMembership(g, []);
            });
        }

        // Priority path computation removed — Metro view owns priority paths.

        // Pre-compute nested-group set for hull rendering
        layoutNestedGroupSet = new Set<any>();
        layoutGroups.forEach((g: any) => {
            (g.groups as any[]).forEach((child: any) => {
                layoutNestedGroupSet.add(child);
            });
        });

        // Initial positions: lay out top-level groups in a wide horizontal grid,
        // then scatter child groups and leaves near their parent's center.
        const pad = 200;
        const groupSeeded = new Set<any>();

        function seedGroupPositions(group: any, cx: number, cy: number, spreadX: number, spreadY: number) {
            if (groupSeeded.has(group)) return;
            groupSeeded.add(group);
            
            // Place the container node exactly at the center
            const leaves = group.leaves as number[];
            const containerIdx = leaves.find((idx: number) => activeNodes[idx].id === group.containerId);
            if (containerIdx !== undefined) {
                const n = activeNodes[containerIdx] as any;
                n.x = cx;
                n.y = cy;
            }

            const childLeaves = leaves.filter((idx: number) => activeNodes[idx].id !== group.containerId);
            childLeaves.forEach((idx: number) => {
                const n = activeNodes[idx] as any;
                n.x = cx + (Math.random() - 0.5) * 5;
                n.y = cy + (Math.random() - 0.5) * 5;
            });

            const childGroupCount = (group.groups as any[]).length;
            if (childGroupCount > 0) {
                const angleStep = (Math.PI * 2) / childGroupCount;
                const radius = Math.max(spreadX, spreadY) * 1.5; 
                (group.groups as any[]).forEach((child: any, i: number) => {
                    const angle = i * angleStep;
                    const childCx = cx + Math.cos(angle) * radius;
                    const childCy = cy + Math.sin(angle) * radius;
                    seedGroupPositions(child, childCx, childCy, spreadX * 0.8, spreadY * 0.8);
                });
            }
        }

        const topGroups = layoutGroups.filter(g => !layoutNestedGroupSet.has(g));
        // Arrange top-level groups in a horizontal row with wrapping
        const cols = Math.max(1, Math.ceil(Math.sqrt(topGroups.length * (cw / ch))));
        const rows = Math.ceil(topGroups.length / cols);
        const cellW = (cw - pad * 2) / cols;
        const cellH = (ch - pad * 2) / rows;

        topGroups.forEach((group, i) => {
            const col = i % cols;
            const row = Math.floor(i / cols);
            const cx = pad + cellW * (col + 0.5) + (Math.random() - 0.5) * cellW * 0.3;
            const cy = pad + cellH * (row + 0.5) + (Math.random() - 0.5) * cellH * 0.3;
            seedGroupPositions(group, cx, cy, cellW * 0.7, cellH * 0.6);
        });
        // Any nodes not in any group (shouldn't happen, but safety)
        activeNodes.forEach((d: any) => {
            if (typeof d.x !== 'number') d.x = pad + Math.random() * (cw - pad * 2);
            if (typeof d.y !== 'number') d.y = ch / 2 + (Math.random() - 0.5) * cellH;
        });

        // --- Phase 2: Render nodes and edges (now that containerGroupNodeIds is populated) ---
        const nEls = d3
            .select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .data(activeNodes, (d) => d.id)
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

        // Filter out parent edges — the bounding boxes show the hierarchy.
        const visualLinks = activeLinks.filter((l: any) => l.type !== 'parent');

        const eEls = d3
            .select(edgesLayer)
            .selectAll<SVGPathElement, GraphEdge>("path")
            .data(visualLinks)
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

        // --- Phase 3: Start D3 layout ---
        const d3Links = activeLinks.map((l: any) => {
            const sid = typeof l.source === 'object' ? l.source.id : l.source;
            const tid = typeof l.target === 'object' ? l.target.id : l.target;
            if (!sid || !tid) return null;
            
            let length = $viewSettings.colaLinkLength || 35;
            let strength = 1.0;
            
            if (l.type === 'parent') {
                length = length * 0.8;
                strength = 2.0; 
            } else if (l.type === 'depends_on') {
                length = length * 1.2; 
                strength = 0.5;
            } else {
                length = length * 1.4; 
                strength = 0.2; 
            }
            return { source: sid, target: tid, length, strength, type: l.type };
        }).filter((l: any) => l !== null) as any[];

        const nestedCount = layoutGroups.filter(g => (g.groups || []).length > 0).length;
        console.log(`[D3 Force] ${activeNodes.length} nodes, ${d3Links.length} links, ${layoutGroups.length} groups (${nestedCount} with children)`);

        simulation = d3.forceSimulation(activeNodes as any)
            .force("link", d3.forceLink(d3Links).id((d: any) => d.id).distance((d: any) => d.length).strength((d: any) => d.strength))
            .force("charge", d3.forceManyBody().strength(-300))
            .force("center", d3.forceCenter(cw / 2, ch / 2))
            .force("collide", d3.forceCollide().radius((d: any) => Math.max(d.w || 0, d.h || 0) / 2 + 10))
            .on("tick", tickVisuals);
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