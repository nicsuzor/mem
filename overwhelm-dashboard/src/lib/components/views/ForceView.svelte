<script lang="ts">
    import * as d3 from "d3";
    import * as cola from "webcola";
    import { onDestroy } from "svelte";
    import { graphData, graphStructureKey } from "../../stores/graph";
    import { viewSettings } from "../../stores/viewSettings";
    import { filters, type VisibilityState } from "../../stores/filters";
    import { selection, toggleSelection } from "../../stores/selection";
    import { buildTaskCardNode } from "../shared/NodeShapes";
    import { projectHue } from "../../data/projectUtils";
    import { routeSfdpEdges, setEdgeObstacles } from "../shared/EdgeRenderer";
    import { zoomScale } from "../../stores/zoom";
    import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";

    const CONTAINER_TYPES = new Set(['epic']);
    const TOP_PAD = 60;              // Visual padding above group boxes for title area
    const MAX_ANCESTOR_DEPTH = 20;   // Cycle guard for parent-chain traversal
    const CHILD_GROUP_SPREAD = 1.5;  // Multiplier for radial spread of nested child groups
    const APPROX_CHAR_WIDTH = 10;    // Approximate char width at font-size 18 for label wrapping
    const CANVAS_AREA = 12_000_000;  // Total canvas pixels (constant area regardless of aspect ratio)

    export let containerGroup: SVGGElement;

    let nodesLayer: SVGGElement;
    let edgesLayer: SVGGElement;
    let hullLayer: SVGGElement;

    let colaLayout: (cola.Layout & cola.ID3StyleLayoutAdaptor) | null = null;
    let colaGroups: any[] = [];
    let layoutNodeGroupSets: Map<string, Set<string>> = new Map();
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

    // Parent-child maps for hierarchy walking
    let parentOf = new Map<string, string>();
    let childrenOf = new Map<string, Set<string>>();
    $: if ($graphData) {
        const pOf = new Map<string, string>();
        const cOf = new Map<string, Set<string>>();
        $graphData.links.forEach((l: any) => {
            if (l.type !== 'parent') return;
            const pid = l.source.id || l.source;
            const cid = l.target.id || l.target;
            pOf.set(cid, pid);
            if (!cOf.has(pid)) cOf.set(pid, new Set());
            cOf.get(pid)!.add(cid);
        });
        parentOf = pOf;
        childrenOf = cOf;
    }

    function getHierarchy(nodeId: string): Set<string> {
        const result = new Set<string>([nodeId]);
        let cur = nodeId;
        while (parentOf.has(cur)) {
            cur = parentOf.get(cur)!;
            result.add(cur);
        }
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

    // Flashlight hover effect
    $: if (nodesLayer && edgesLayer && $graphData) {
        const hoveredId = $selection.hoveredNodeId;
        const activeId = $selection.activeNodeId;
        const nEls = d3.select(nodesLayer).selectAll(".node");
        const eEls = d3.select(edgesLayer).selectAll("path");

        const hierarchyIds = activeId ? getHierarchy(activeId) : new Set<string>();
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
                const selMatch = selectedNeighbors.has(sid) && selectedNeighbors.has(tid);
                const activeMatch = activeId && (sid === activeId || tid === activeId);
                return !hoverMatch && !selMatch && !activeMatch;
            }).classed("illuminated", (l: any) => {
                const sid = l.source.id || l.source;
                const tid = l.target.id || l.target;
                const hoverMatch = sid === hoveredId || tid === hoveredId;
                const selMatch = selectedNeighbors.has(sid) && selectedNeighbors.has(tid);
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
        return $filters.edgeReferences;
    }

    function edgeOpacity(vis: VisibilityState): number {
        if (vis === 'bright') return 0.85;
        if (vis === 'half') return 0.25;
        return 0;
    }

    function applyEdgeVisibility(eEls: any) {
        eEls.attr("opacity", (d: any) => edgeOpacity(edgeVisForType(d.type)));
    }

    $: {
        const _ep = $filters.edgeParent;
        const _ed = $filters.edgeDependencies;
        const _er = $filters.edgeReferences;
        if (edgesLayer) {
            const eEls = d3.select(edgesLayer).selectAll("path");
            if (!eEls.empty()) applyEdgeVisibility(eEls);
        }
    }

    // ─── Group building ────────────────────────────────────────────────────────

    interface ColaGroupResult {
        colaGroups: any[];
        layoutNodeGroupSets: Map<string, Set<string>>;
        layoutNestedGroupSet: Set<any>;
    }

    function buildColaGroups(
        activeNodes: GraphNode[],
        activeLinks: GraphEdge[],
        groupPadding: number,
    ): ColaGroupResult {
        const nodeById = new Map(activeNodes.map(n => [n.id, n]));

        const containerNodeIds = new Set<string>();
        activeNodes.forEach(n => { if (CONTAINER_TYPES.has(n.type)) containerNodeIds.add(n.id); });

        // Build parent lookup for container ancestry traversal
        const parentLinks = activeLinks.filter((l: any) => l.type === 'parent');
        const parentOfNode = new Map<string, GraphNode>();
        for (const l of parentLinks) {
            const pid = typeof l.source === 'object' ? (l.source as any).id : l.source;
            const cid = typeof l.target === 'object' ? (l.target as any).id : l.target;
            const pNode = nodeById.get(pid);
            if (pNode) parentOfNode.set(cid, pNode);
        }

        function findContainer(nodeId: string): string | null {
            let cur = nodeId;
            for (let depth = 0; depth < MAX_ANCESTOR_DEPTH; depth++) {
                const p = parentOfNode.get(cur);
                if (!p) break;
                if (CONTAINER_TYPES.has(p.type)) return p.id;
                cur = p.id;
            }
            return null;
        }

        // Find each container's parent container (for nesting)
        const containerParent = new Map<string, string | null>();
        for (const cid of containerNodeIds) {
            containerParent.set(cid, findContainer(cid));
        }

        // Build leaf and child-group lists per container
        const containerLeaves = new Map<string, number[]>();
        const containerChildGroups = new Map<string, string[]>();
        for (const cid of containerNodeIds) {
            containerLeaves.set(cid, []);
            containerChildGroups.set(cid, []);
        }
        for (const cid of containerNodeIds) {
            const pcid = containerParent.get(cid);
            if (pcid && containerChildGroups.has(pcid)) {
                containerChildGroups.get(pcid)!.push(cid);
            }
        }

        const ungroupedIndices: number[] = [];
        activeNodes.forEach((n, i) => {
            if (containerNodeIds.has(n.id)) {
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

        const d3Groups: any[] = [];
        const groupIndexMap = new Map<string, number>();

        // Pass 1: Create all container groups (leaves only)
        for (const cid of containerNodeIds) {
            const leaves = containerLeaves.get(cid) || [];
            const childContainers = containerChildGroups.get(cid) || [];
            if (leaves.length <= 1 && childContainers.length === 0) {
                ungroupedIndices.push(...leaves);
                continue;
            }
            const containerNode = nodeById.get(cid);
            const label = containerNode?.label || containerNode?.fullTitle || cid;
            const isNested = containerParent.get(cid) !== null;
            const nestPadding = isNested ? Math.max(40, groupPadding + 30) : Math.max(65, groupPadding + 55);
            groupIndexMap.set(cid, d3Groups.length);
            d3Groups.push({ leaves, groups: [], padding: nestPadding, label, containerId: cid });
        }

        // Pass 2: Wire nested group references
        for (const cid of containerNodeIds) {
            const pcid = containerParent.get(cid);
            if (pcid && groupIndexMap.has(pcid) && groupIndexMap.has(cid)) {
                d3Groups[groupIndexMap.get(pcid)!].groups.push(groupIndexMap.get(cid)!);
            }
        }

        // Convert indices to object references
        const groups: any[] = d3Groups.map(g => ({ ...g, groups: [] }));
        d3Groups.forEach((g, i) => {
            groups[i].groups = (g.groups as number[]).map(idx => groups[idx]);
        });

        if (ungroupedIndices.length > 0) {
            groups.push({ leaves: ungroupedIndices, groups: [], padding: groupPadding, label: '' });
        }

        // Pre-compute node→group membership for edge routing
        const nodeGroupSets = new Map<string, Set<string>>();
        function addNGS(nodeId: string, gId: string) {
            if (!nodeGroupSets.has(nodeId)) nodeGroupSets.set(nodeId, new Set());
            nodeGroupSets.get(nodeId)!.add(gId);
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
            (g.groups || []).forEach((child: any) => buildGroupMembership(child, [...ancestorIds, gId]));
        }
        const nestedResolved = new Set<any>();
        groups.forEach(g => (g.groups || []).forEach((c: any) => nestedResolved.add(c)));
        groups.forEach(g => { if (!nestedResolved.has(g) && g.label) buildGroupMembership(g, []); });

        // Pre-compute nested-group set for box rendering
        const nestedGroupSet = new Set<any>();
        groups.forEach((g: any) => (g.groups as any[]).forEach(child => nestedGroupSet.add(child)));

        return { colaGroups: groups, layoutNodeGroupSets: nodeGroupSets, layoutNestedGroupSet: nestedGroupSet };
    }

    // ─── Position seeding ─────────────────────────────────────────────────────

    function seedPositions(
        groups: any[],
        activeNodes: GraphNode[],
        nestedGroupSet: Set<any>,
        cw: number,
        ch: number,
    ) {
        const pad = 200;
        const topGroups = groups.filter(g => !nestedGroupSet.has(g));
        const cols = Math.max(1, Math.ceil(Math.sqrt(topGroups.length * (cw / ch))));
        const rows = Math.ceil(topGroups.length / cols);
        const cellW = (cw - pad * 2) / cols;
        const cellH = (ch - pad * 2) / rows;

        const seeded = new Set<any>();

        function seedGroup(group: any, cx: number, cy: number, spreadX: number, spreadY: number) {
            if (seeded.has(group)) return;
            seeded.add(group);
            const leaves = group.leaves as number[];
            const containerIdx = leaves.find((idx: number) => activeNodes[idx].id === group.containerId);
            if (containerIdx !== undefined) {
                const n = activeNodes[containerIdx] as any;
                n.x = cx; n.y = cy;
            }
            leaves.filter((idx: number) => activeNodes[idx].id !== group.containerId)
                .forEach((idx: number) => {
                    const n = activeNodes[idx] as any;
                    n.x = cx + (Math.random() - 0.5) * 5;
                    n.y = cy + (Math.random() - 0.5) * 5;
                });
            const childGroupCount = (group.groups as any[]).length;
            if (childGroupCount > 0) {
                const angleStep = (Math.PI * 2) / childGroupCount;
                const radius = Math.max(spreadX, spreadY) * CHILD_GROUP_SPREAD;
                (group.groups as any[]).forEach((child: any, i: number) => {
                    const angle = i * angleStep;
                    seedGroup(child, cx + Math.cos(angle) * radius, cy + Math.sin(angle) * radius, spreadX * 0.8, spreadY * 0.8);
                });
            }
        }

        topGroups.forEach((group, i) => {
            const col = i % cols;
            const row = Math.floor(i / cols);
            const cx = pad + cellW * (col + 0.5) + (Math.random() - 0.5) * cellW * 0.3;
            const cy = pad + cellH * (row + 0.5) + (Math.random() - 0.5) * cellH * 0.3;
            seedGroup(group, cx, cy, cellW * 0.7, cellH * 0.6);
        });

        activeNodes.forEach((d: any) => {
            if (typeof d.x !== 'number') d.x = pad + Math.random() * (cw - pad * 2);
            if (typeof d.y !== 'number') d.y = ch / 2 + (Math.random() - 0.5) * cellH;
        });
    }

    // ─── Drag and click ───────────────────────────────────────────────────────

    function bindDragAndClick(nEls: any) {
        nEls.style("cursor", "crosshair")
            .on("click", (e: any, d: any) => { e.stopPropagation(); toggleSelection(d.id); })
            .on("mouseenter", (e: any, d: any) => { selection.update(s => ({ ...s, hoveredNodeId: d.id })); })
            .on("mouseleave", () => { selection.update(s => ({ ...s, hoveredNodeId: null })); })
            .call(
                d3.drag<SVGGElement, GraphNode>()
                    .clickDistance(4)
                    .on("start", () => { /* defer until actual movement */ })
                    .on("drag", (e, d: any) => {
                        if (!(d as any)._dragging) {
                            cola.Layout.dragStart(d);
                            (d as any)._dragging = true;
                        }
                        d.x = e.x; d.y = e.y;
                        if (colaLayout) colaLayout.resume();
                    })
                    .on("end", (_e, d: any) => {
                        if ((d as any)._dragging) {
                            d.fixed = 0;
                            (d as any)._dragging = false;
                        }
                    }),
            );
    }

    // ─── Group box rendering ──────────────────────────────────────────────────

    function renderGroupBoxes() {
        if (!hullLayer || !colaLayout) return;

        const allGroups: any[] = [];
        function extractGroups(gList: any[]) {
            for (const g of gList) {
                if (g.label) allGroups.push(g);
                if (g.groups?.length > 0) extractGroups(g.groups);
            }
        }
        extractGroups(colaLayout.groups() || []);
        const uniqueGroups = Array.from(
            new Map(allGroups.map(g => [g.containerId || g.label, g])).values()
        );

        // Background rectangles
        d3.select(hullLayer)
            .selectAll<SVGRectElement, any>("rect.cola-group")
            .data(uniqueGroups, (d: any) => d.containerId || d.label)
            .join("rect")
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
            .on("click", (e: any, d: any) => { e.stopPropagation(); if (d.containerId) toggleSelection(d.containerId); })
            .on("mouseenter", (e: any, d: any) => { if (d.containerId) selection.update(s => ({ ...s, hoveredNodeId: d.containerId })); })
            .on("mouseleave", () => { selection.update(s => ({ ...s, hoveredNodeId: null })); });

        // Header shading
        d3.select(hullLayer)
            .selectAll<SVGRectElement, any>("rect.cola-group-header")
            .data(uniqueGroups, (d: any) => d.containerId || d.label)
            .join("rect")
            .attr("class", "cola-group-header")
            .style("pointer-events", "none")
            .attr("rx", (d: any) => layoutNestedGroupSet.has(d) ? 6 : 10)
            .attr("ry", (d: any) => layoutNestedGroupSet.has(d) ? 6 : 10)
            .attr("x", (d: any) => d.bounds?.x ?? 0)
            .attr("y", (d: any) => (d.bounds?.y ?? 0) - TOP_PAD)
            .attr("width", (d: any) => d.bounds?.width() ?? 0)
            .attr("height", Math.max(TOP_PAD + 10, 0))
            .attr("fill", (d: any) => {
                const hue = projectHue(d.containerId || d.label || '');
                return `hsla(${hue}, 40%, 15%, 0.6)`;
            });

        // Group labels
        d3.select(hullLayer)
            .selectAll<SVGTextElement, any>("text.cola-group-label")
            .data(uniqueGroups, (d: any) => d.containerId || d.label)
            .join("text")
            .attr("class", "cola-group-label")
            .attr("font-size", 18)
            .attr("font-weight", 700)
            .attr("fill", (d: any) => {
                const hue = projectHue(d.containerId || d.label || '');
                return `hsla(${hue}, 60%, 80%, 0.9)`;
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
                const charW = APPROX_CHAR_WIDTH;
                const charsPerLine = Math.max(4, Math.floor(availW / charW));
                const words = label.split(/\s+/);
                const lines: string[] = [];
                let current = "";
                for (const word of words) {
                    const test = current ? `${current} ${word}` : word;
                    if (test.length > charsPerLine && current) { lines.push(current); current = word; }
                    else { current = test; }
                }
                if (current) lines.push(current);
                const display = lines.slice(0, 2);
                if (lines.length > 2) display[1] = display[1].slice(0, -1) + "…";
                display.forEach((line, i) => {
                    el.append("tspan").attr("x", bx).attr("y", by + i * 22).text(line);
                });
            });
    }

    // ─── Tick ─────────────────────────────────────────────────────────────────

    function tickVisuals() {
        if (!colaLayout) return;

        const scale = $zoomScale;
        d3.select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .attr("transform", (d) => `translate(${d.x ?? 0},${d.y ?? 0})`)
            .each(function (d) {
                const texts = d3.select(this).selectAll("text, tspan");
                if (texts.empty()) return;
                const isHighPri = d.priority <= 1;
                texts.attr("opacity", (scale > 0.4 || (isHighPri && scale > 0.2)) ? null : 0);
            });

        // Update obstacle data for edge routing
        const groups = (colaLayout.groups() || []).filter((g: any) => g.label && g.bounds);
        setEdgeObstacles(
            groups.map((g: any) => ({
                x: g.bounds.x, y: g.bounds.y,
                X: g.bounds.X, Y: g.bounds.Y,
                containerId: g.containerId || g.label || '',
            })),
            layoutNodeGroupSets,
        );

        const eEls = d3.select(edgesLayer).selectAll<SVGPathElement, GraphEdge>("path");
        routeSfdpEdges(eEls);
        applyEdgeVisibility(eEls);

        renderGroupBoxes();
    }

    // ─── Main draw ────────────────────────────────────────────────────────────

    function drawForceAndStartPhysics() {
        if (!$graphData) return;
        if (colaLayout) { colaLayout.stop(); colaLayout = null; }

        const data = $graphData;

        // Strip project nodes — epic group boxes handle visual hierarchy
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
                while (cur && projectIds.has(cur)) {
                    if (seen.has(cur)) break;
                    seen.add(cur);
                    cur = projParentMap.get(cur) ?? null;
                }
                return cur !== n.parent ? { ...n, parent: cur } : n;
            }).filter(n => !projectIds.has(n.id));
            activeLinks = data.links.filter((l: any) => {
                const sid = typeof l.source === 'object' ? l.source.id : l.source;
                const tid = typeof l.target === 'object' ? l.target.id : l.target;
                return !projectIds.has(sid) && !projectIds.has(tid);
            });
        }

        // Resolve link IDs to node references
        const nodeById = new Map(activeNodes.map(n => [n.id, n]));
        const nodeIndex = new Map(activeNodes.map((n, i) => [n.id, i]));
        activeLinks.forEach((l: any) => {
            if (typeof l.source === 'string') l.source = nodeById.get(l.source) || l.source;
            if (typeof l.target === 'string') l.target = nodeById.get(l.target) || l.target;
        });
        activeNodes.forEach((n: any) => { n.width = n.w; n.height = n.h; });

        // Build groups and seed positions
        const groupResult = buildColaGroups(activeNodes, activeLinks, $viewSettings.colaGroupPadding);
        colaGroups = groupResult.colaGroups;
        layoutNodeGroupSets = groupResult.layoutNodeGroupSets;
        layoutNestedGroupSet = groupResult.layoutNestedGroupSet;

        const svg = containerGroup?.ownerSVGElement;
        const vw = svg?.clientWidth || window.innerWidth || 1400;
        const vh = svg?.clientHeight || window.innerHeight || 900;
        const aspect = vw / vh;
        const ch = Math.round(Math.sqrt(CANVAS_AREA / aspect));
        const cw = Math.round(ch * aspect);

        seedPositions(colaGroups, activeNodes, layoutNestedGroupSet, cw, ch);

        // Render nodes
        const nEls = d3.select(nodesLayer)
            .selectAll<SVGGElement, GraphNode>("g.node")
            .data(activeNodes, (d) => d.id)
            .join("g")
            .attr("class", "node")
            .attr("transform", (d) => `translate(${d.x ?? 0},${d.y ?? 0})`);

        const activeId = $selection.activeNodeId;
        nEls.each(function (d) {
            const g = d3.select(this) as any;
            const isSelected = d.id === activeId;
            if (g.selectAll("*").empty() || (d as any)._lastSelected !== isSelected) {
                g.selectAll("*").remove();
                buildTaskCardNode(g, d, isSelected);
                (d as any)._lastSelected = isSelected;
            }
        });
        bindDragAndClick(nEls);

        // Render edges (excluding parent edges — group boxes show hierarchy)
        const visualLinks = activeLinks.filter((l: any) => l.type !== 'parent');
        const eEls = d3.select(edgesLayer)
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

        // Build Cola links (parent links omitted — groups handle hierarchy)
        const colaLinks = activeLinks
            .filter((l: any) => l.type !== 'parent')
            .map((l: any) => {
                const si = nodeIndex.get(typeof l.source === 'object' ? l.source.id : l.source);
                const ti = nodeIndex.get(typeof l.target === 'object' ? l.target.id : l.target);
                if (si === undefined || ti === undefined) return null;
                const length = ($viewSettings.colaLinkLength || 35) *
                    (l.type === 'depends_on' ? 1.2 : 1.4);
                const weight = l.type === 'depends_on' ? 1.0 : 0.5;
                return { source: si, target: ti, length, weight };
            })
            .filter((l: any) => l !== null) as any[];

        // Start Cola layout
        colaLayout = cola.d3adaptor(d3)
            .size([cw, ch])
            .nodes(activeNodes as any)
            .links(colaLinks)
            .groups(colaGroups)
            .avoidOverlaps(true)
            .handleDisconnected(true)
            .linkDistance((d: any) => d.length)
            .on("tick", tickVisuals)
            .start(100, 100, 200);
    }

    onDestroy(() => {
        if (colaLayout) colaLayout.stop();
    });
</script>

{#if containerGroup}
    <g bind:this={hullLayer} class="hull-layer"></g>
    <g bind:this={edgesLayer}></g>
    <g bind:this={nodesLayer}></g>
{/if}

<style>
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
    :global(g.node.selected-node) {
        opacity: 1 !important;
        filter: none !important;
    }
    :global(path.force-edge.intent-edge-dim) {
        opacity: 0.15;
    }
</style>
