<script lang="ts">
    import * as d3 from "d3";
    import * as cola from "webcola";
    import { onMount, onDestroy } from "svelte";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { viewSettings } from "../../stores/viewSettings";
    import { filters, type EdgeVisibility } from "../../stores/filters";
    import { selection, toggleSelection } from "../../stores/selection";
    import { buildTaskCardNode } from "../shared/NodeShapes";
    import { projectHue } from "../../data/projectUtils";
    import { routeSfdpEdges, setEdgeObstacles } from "../shared/EdgeRenderer";
    import { FORCE_CONFIG } from "../../data/constants";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";

    export let containerGroup: SVGGElement;

    let nodesLayer: SVGGElement;
    let edgesLayer: SVGGElement;
    let hullLayer: SVGGElement;

    let colaLayout: (cola.Layout & cola.ID3StyleLayoutAdaptor) | null = null;
    // Container nodes that have a Cola group box — their hexagon is hidden (box replaces it)
    let containerGroupNodeIds = new Set<string>();
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
                    .clickDistance(4) // px threshold — prevents clicks from triggering drag
                    .on("start", () => {
                        // Defer dragStart to first real drag movement (see "drag" handler)
                    })
                    .on("drag", (e, d: any) => {
                        // Lazy init: only pin/wake Cola once actual movement occurs
                        if (!(d as any)._dragging) {
                            cola.Layout.dragStart(d);
                            (d as any)._dragging = true;
                        }
                        d.x = e.x;
                        d.y = e.y;
                        if (colaLayout) colaLayout.resume();
                    })
                    .on("end", (e, d: any) => {
                        if ((d as any)._dragging) {
                            d.fixed = 0;
                            (d as any)._dragging = false;
                        }
                    }),
            );
    }

    function tickVisuals() {
        // --- Custom Force: Keep epics and child tasks closely packed ---
        if ($graphData && parentOf) {
            const nodeMap = new Map<string, any>();
            $graphData.nodes.forEach((n: any) => nodeMap.set(n.id, n));

            const CONTAINER_TYPES = new Set(['epic']);

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
                    n.x += dx * 0.25;
                    n.y += dy * 0.25;
                }
            });
        }

        // Anchor epic nodes to the center of their group bounding box
        if (colaLayout) {
            const groups = colaLayout.groups() || [];
            groups.forEach((g: any) => {
                if (!g.containerId || !g.bounds) return;
                const nodeMap = new Map<string, any>();
                $graphData?.nodes.forEach((n: any) => nodeMap.set(n.id, n));
                const epicNode = nodeMap.get(g.containerId);
                if (epicNode) {
                    epicNode.x = (g.bounds.x + g.bounds.X) / 2;
                    epicNode.y = (g.bounds.y + g.bounds.Y) / 2;
                }
            });
        }

        d3.select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .attr("transform", (d) => `translate(${d.x ?? 0},${d.y ?? 0})`);

        // Update obstacle data for edge routing from Cola group bounding boxes
        if (colaLayout) {
            const groups = (colaLayout.groups() || []).filter((g: any) => g.label && g.bounds);
            const obstacles = groups.map((g: any) => ({
                x: g.bounds.x,
                y: g.bounds.y,
                X: g.bounds.X,
                Y: g.bounds.Y,
                containerId: g.containerId || g.label || '',
            }));
            // Build node → ALL ancestor group IDs so edges between nodes in the
            // same container (or nested child containers) skip all enclosing boxes.
            const nodeGroupSets = new Map<string, Set<string>>();

            // Helper: register a node as belonging to a group
            function addNodeToGroup(nodeId: string, gId: string) {
                if (!nodeGroupSets.has(nodeId)) nodeGroupSets.set(nodeId, new Set());
                nodeGroupSets.get(nodeId)!.add(gId);
            }

            // Direct leaf membership
            groups.forEach((g: any) => {
                const gId = g.containerId || g.label || '';
                (g.leaves || []).forEach((leaf: any) => {
                    const nodeId = typeof leaf === 'number'
                        ? $graphData?.nodes[leaf]?.id
                        : (leaf.id || leaf);
                    if (nodeId) addNodeToGroup(nodeId, gId);
                });
            });

            // Propagate: nodes in child groups also belong to all ancestor groups.
            // Walk the group nesting tree and propagate membership upward.
            function propagateGroupMembership(g: any, ancestorIds: string[]) {
                const gId = g.containerId || g.label || '';
                // All nodes directly in this group get all ancestor group IDs too
                (g.leaves || []).forEach((leaf: any) => {
                    const nodeId = typeof leaf === 'number'
                        ? $graphData?.nodes[leaf]?.id
                        : (leaf.id || leaf);
                    if (nodeId) {
                        for (const aid of ancestorIds) addNodeToGroup(nodeId, aid);
                    }
                });
                // Recurse into child groups
                (g.groups || []).forEach((childGroup: any) => {
                    propagateGroupMembership(childGroup, [...ancestorIds, gId]);
                });
            }
            // Start from top-level groups (those not nested in any other)
            const nestedSet = new Set<any>();
            groups.forEach((g: any) => (g.groups || []).forEach((c: any) => nestedSet.add(c)));
            groups.forEach((g: any) => {
                if (!nestedSet.has(g)) {
                    propagateGroupMembership(g, []);
                }
            });

            setEdgeObstacles(obstacles, nodeGroupSets);
        }

        const eEls = d3.select(edgesLayer).selectAll<SVGPathElement, GraphEdge>("path");
        routeSfdpEdges(eEls);
        applyEdgeVisibility(eEls);

        // P0/P1 edge glow: highlight edges along ancestor/descendant paths of high-priority nodes
        if ($graphData && parentOf && childrenOf) {
            const INCOMPLETE = new Set(['inbox', 'active', 'in_progress', 'blocked', 'waiting', 'todo', 'pending']);
            const highPriIds = new Set<string>();
            $graphData.nodes.forEach(n => {
                if (n.priority <= 1 && INCOMPLETE.has(n.status)) highPriIds.add(n.id);
            });
            // Walk ancestors + descendants of each P0/P1 node
            const relatedIds = new Set<string>(highPriIds);
            for (const id of highPriIds) {
                // Ancestors
                let cur = id;
                while (parentOf.has(cur)) { cur = parentOf.get(cur)!; relatedIds.add(cur); }
                // Descendants (BFS)
                const queue = [id];
                while (queue.length > 0) {
                    const nid = queue.shift()!;
                    const kids = childrenOf.get(nid);
                    if (kids) for (const kid of kids) {
                        if (!relatedIds.has(kid)) { relatedIds.add(kid); queue.push(kid); }
                    }
                }
            }
            eEls.classed("high-priority-edge", (l: any) => {
                const sid = l.source?.id || l.source;
                const tid = l.target?.id || l.target;
                return relatedIds.has(sid) && relatedIds.has(tid);
            });
        }

        // Render group bounding boxes from WebCola (skip unlabelled catch-all group)
        if (hullLayer && colaLayout) {
            const groups = (colaLayout.groups() || []).filter((g: any) => g.label);
            const groupEls = d3.select(hullLayer)
                .selectAll<SVGRectElement, any>("rect.cola-group")
                .data(groups);

            // Detect which groups are nested (have a parent group)
            const nestedGroupSet = new Set<any>();
            groups.forEach((g: any) => {
                (g.groups || []).forEach((child: any) => nestedGroupSet.add(child));
            });

            groupEls.join("rect")
                .attr("class", "cola-group")
                .attr("rx", (d: any) => nestedGroupSet.has(d) ? 6 : 10)
                .attr("ry", (d: any) => nestedGroupSet.has(d) ? 6 : 10)
                .attr("x", (d: any) => d.bounds?.x ?? 0)
                .attr("y", (d: any) => d.bounds?.y ?? 0)
                .attr("width", (d: any) => d.bounds?.width() ?? 0)
                .attr("height", (d: any) => d.bounds?.height() ?? 0)
                .attr("fill", (d: any) => {
                    const hue = projectHue(d.containerId || d.label || '');
                    const isNested = nestedGroupSet.has(d);
                    return isNested
                        ? `hsla(${hue}, 40%, 50%, 0.05)`
                        : `hsla(${hue}, 40%, 50%, 0.08)`;
                })
                .attr("stroke", (d: any) => {
                    const hue = projectHue(d.containerId || d.label || '');
                    const isNested = nestedGroupSet.has(d);
                    return isNested
                        ? `hsla(${hue}, 50%, 55%, 0.25)`
                        : `hsla(${hue}, 40%, 50%, 0.3)`;
                })
                .attr("stroke-width", (d: any) => nestedGroupSet.has(d) ? 1 : 2)
                .attr("stroke-dasharray", (d: any) => nestedGroupSet.has(d) ? "4,2" : "6,3")
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

            // Epic group labels
            const labelEls = d3.select(hullLayer)
                .selectAll<SVGTextElement, any>("text.cola-group-label")
                .data(groups);

            labelEls.join("text")
                .attr("class", "cola-group-label")
                .attr("font-size", 18)
                .attr("font-weight", 700)
                .attr("fill", (d: any) => {
                    const hue = projectHue(d.containerId || d.label || '');
                    return `hsla(${hue}, 40%, 35%, 0.7)`;
                })
                .style("pointer-events", "none")
                .style("text-transform", "uppercase")
                .style("letter-spacing", "0.05em")
                .each(function (d: any) {
                    const el = d3.select(this);
                    el.selectAll("tspan").remove();
                    const label = (d.label || "").toUpperCase();
                    const bx = (d.bounds?.x ?? 0) + 12;
                    const by = (d.bounds?.y ?? 0) + 24;
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
        if (colaLayout) { colaLayout.stop(); colaLayout = null; }

        const data = $graphData;
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
        const fc = FORCE_CONFIG;
        const nodeById = new Map($graphData.nodes.map(n => [n.id, n]));
        const nodeIndex = new Map($graphData.nodes.map((n, i) => [n.id, i]));
        $graphData.links.forEach((l: any) => {
            if (typeof l.source === 'string') l.source = nodeById.get(l.source) || l.source;
            if (typeof l.target === 'string') l.target = nodeById.get(l.target) || l.target;
        });

        const CONTAINER_TYPES = new Set(['epic']);

        // Set width/height on nodes for avoidOverlaps — account for epic scaling.
        // IMPORTANT: Cola's overlap solver pushes nodes apart by the minimum displacement.
        // If nodes are wide+short, it always pushes vertically → vertical stacking.
        // We inflate height padding so the collision box is closer to square,
        // making overlap resolution direction-neutral.
        const CONTAINER_SCALE = 1.3;
        $graphData.nodes.forEach((n: any) => {
            if (CONTAINER_TYPES.has(n.type)) {
                // Epic nodes: full-size collision box, anchored to group center in tickVisuals
                const scale = CONTAINER_SCALE;
                n.width = n.w * scale + 40;
                n.height = n.h * scale + 40;
                return;
            }
            const rawW = n.w;
            const rawH = n.h;
            // Use natural dimensions + generous padding.
            // Wide+short boxes → overlap resolution pushes vertically within groups,
            // creating compact 2D packing. Global horizontal layout from seeding.
            // Extra padding for high-priority nodes (glow rings, thicker borders)
            const priPad = n.priority <= 1 ? 20 : 0;
            n.width = rawW + 40 + priPad;
            n.height = rawH + 40 + priPad;
        });

        // Build hierarchical nested groups — epics inside projects, sub-epics inside epics, etc.
        // WebCola supports nested groups via the `groups` property on parent groups.
        const parentLinks = $graphData.links.filter((l: any) => l.type === 'parent');

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
        $graphData.nodes.forEach(n => {
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
        $graphData.nodes.forEach((n, i) => {
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
        const colaGroups: any[] = [];
        const groupIndexMap = new Map<string, number>(); // containerId → index in colaGroups

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
            // Nested groups get slightly more padding so the visual hierarchy is clear
            const isNested = containerParent.get(cid) !== null;
            const nestPadding = isNested ? groupPadding : groupPadding + 4;
            const groupIdx = colaGroups.length;
            groupIndexMap.set(cid, groupIdx);
            colaGroups.push({
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
                colaGroups[parentGroupIdx].groups.push(childGroupIdx);
            }
        }

        // Mark which container nodes have a group box — their node visual will be hidden
        containerGroupNodeIds = new Set(groupIndexMap.keys());

        // Put ungrouped nodes in a catch-all group so Cola keeps them outside epic containers
        if (ungroupedIndices.length > 0) {
            colaGroups.push({ leaves: ungroupedIndices, groups: [], padding: groupPadding, label: '' });
        }

        // Initial positions: lay out top-level groups in a wide horizontal grid,
        // then scatter child groups and leaves near their parent's center.
        // Bias toward horizontal spread so the graph doesn't stack vertically.
        const pad = 200;
        const groupSeeded = new Set<number>();

        function seedGroupPositions(groupIdx: number, cx: number, cy: number, spreadX: number, spreadY: number) {
            if (groupSeeded.has(groupIdx)) return;
            groupSeeded.add(groupIdx);
            const group = colaGroups[groupIdx];
            // Place direct leaf nodes near group center — wider horizontal scatter
            (group.leaves as number[]).forEach((idx: number) => {
                const n = data.nodes[idx] as any;
                n.x = cx + (Math.random() - 0.5) * spreadX * 0.4;
                n.y = cy + (Math.random() - 0.5) * spreadY * 0.3;
            });
            // Recursively seed child groups near this center
            (group.groups as number[]).forEach((childIdx: number) => {
                const childCx = cx + (Math.random() - 0.5) * spreadX * 0.5;
                const childCy = cy + (Math.random() - 0.5) * spreadY * 0.4;
                seedGroupPositions(childIdx, childCx, childCy, spreadX * 0.6, spreadY * 0.6);
            });
        }

        // Find top-level groups (not nested inside any other group)
        const nestedGroupIndices = new Set<number>();
        colaGroups.forEach(g => (g.groups as number[]).forEach((ci: number) => nestedGroupIndices.add(ci)));

        const topGroups = colaGroups.map((g, i) => i).filter(i => !nestedGroupIndices.has(i));
        // Arrange top-level groups in a horizontal row with wrapping
        const cols = Math.max(1, Math.ceil(Math.sqrt(topGroups.length * (cw / ch))));
        const rows = Math.ceil(topGroups.length / cols);
        const cellW = (cw - pad * 2) / cols;
        const cellH = (ch - pad * 2) / rows;

        topGroups.forEach((groupIdx, i) => {
            const col = i % cols;
            const row = Math.floor(i / cols);
            const cx = pad + cellW * (col + 0.5) + (Math.random() - 0.5) * cellW * 0.3;
            const cy = pad + cellH * (row + 0.5) + (Math.random() - 0.5) * cellH * 0.3;
            seedGroupPositions(groupIdx, cx, cy, cellW * 0.7, cellH * 0.6);
        });
        // Any nodes not in any group (shouldn't happen, but safety)
        data.nodes.forEach((d: any) => {
            if (typeof d.x !== 'number') d.x = pad + Math.random() * (cw - pad * 2);
            if (typeof d.y !== 'number') d.y = ch / 2 + (Math.random() - 0.5) * cellH;
        });

        // --- Phase 2: Render nodes and edges (now that containerGroupNodeIds is populated) ---
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

        // Filter out parent edges involving container-group nodes — the box shows hierarchy.
        const visualLinks = data.links.filter((l: any) => {
            if (l.type !== 'parent') return true;
            const sid = typeof l.source === 'object' ? l.source.id : l.source;
            const tid = typeof l.target === 'object' ? l.target.id : l.target;
            return !containerGroupNodeIds.has(sid) && !containerGroupNodeIds.has(tid);
        });

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

        // --- Phase 3: Start Cola layout ---
        const colaLinks = parentLinks.map((l: any) => {
            const si = nodeIndex.get(typeof l.source === 'object' ? l.source.id : l.source)!;
            const ti = nodeIndex.get(typeof l.target === 'object' ? l.target.id : l.target)!;
            return { source: si, target: ti };
        }).filter((l: any) => l.source !== undefined && l.target !== undefined);

        const nestedCount = colaGroups.filter(g => (g.groups || []).length > 0).length;
        console.log(`[Cola] ${$graphData.nodes.length} nodes, ${colaLinks.length} links, ${colaGroups.length} groups (${nestedCount} with children)`, colaGroups.map(g => `${g.leaves.length}L${(g.groups||[]).length ? '+' + (g.groups||[]).length + 'G' : ''}`));

        colaLayout = cola.d3adaptor(d3)
            .size([cw, ch])
            .nodes($graphData.nodes as any)
            .links(colaLinks)
            .groups(colaGroups)
            .avoidOverlaps(true)
            .handleDisconnected(true)
            .symmetricDiffLinkLengths($viewSettings.colaLinkLength, 0.7)
            .on("tick", tickVisuals)
            .start(30, 30, 30);
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
    /* P0/P1 ancestry/descendancy edge glow */
    :global(path.force-edge.high-priority-edge) {
        filter: drop-shadow(0 0 5px rgba(245, 158, 11, 0.5));
        stroke-width: 3px !important;
        opacity: 0.9 !important;
    }
</style>
