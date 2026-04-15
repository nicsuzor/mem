<script lang="ts">
    import * as d3 from "d3";
    import * as cola from "webcola";
    import { getContext, onDestroy } from "svelte";
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
    const DEFAULT_JIGGLE_RADIUS = 320;
    const DRAG_REHEAT = 0.35;
    const FULL_START_ITERATIONS: readonly [number, number, number] = [30, 30, 30];
    const RESTART_START_ITERATIONS: readonly [number, number, number] = [5, 5, 5];

    export let containerGroup: SVGGElement;
    export let running = false;
    export let restartNonce = 0;
    export let randomizeNonce = 0;

    const zoomContext = getContext<{ autoZoomToFit?: (nodesToFit?: GraphNode[], delay?: number, trimOutliers?: boolean) => void }>("zoom");

    let linksLayer: SVGGElement;
    let nodesLayer: SVGGElement;
    let hullLayer: SVGGElement;

    let colaLayout: (cola.Layout & cola.ID3StyleLayoutAdaptor) | null = null;
    let colaGroups: any[] = [];
    let groupMembers = new Map<string, string[]>();
    let lastRestartNonce = 0;
    let lastRandomizeNonce = 0;
    let lastRunning = running;

    function reheatLayout(heat = 1) {
        if (!colaLayout) return;

        // WebCola resume() only sets alpha to 0.1, which is often too cold to create
        // noticeable motion once the graph has already converged.
        colaLayout.alpha(heat);
        running = true;
    }

    function jiggleNodes(radius = 24) {
        if (!$graphData) return;

        const halfRadius = radius / 2;
        for (const n of $graphData.nodes as any[]) {
            if (n.fixed) continue;

            n.x = (n.x ?? 0) + (Math.random() * radius - halfRadius);
            n.y = (n.y ?? 0) + (Math.random() * radius - halfRadius);
            n.px = n.x;
            n.py = n.y;
        }
    }

    function fitGraph(delay = 150, trimOutliers = false) {
        zoomContext?.autoZoomToFit?.($graphData?.nodes, delay, trimOutliers);
    }

    function seedNodesAcrossCanvas(nodes: GraphNode[], width: number, height: number) {
        if (!nodes.length) return;

        const childrenByParent = new Map<string, GraphNode[]>();
        for (const node of nodes) {
            const parentId = (node as any)._safe_parent;
            if (!parentId) continue;
            if (!childrenByParent.has(parentId)) childrenByParent.set(parentId, []);
            childrenByParent.get(parentId)!.push(node);
        }

        const parentIds = new Set(childrenByParent.keys());
        const roots = nodes.filter((node) => !(node as any)._safe_parent).sort((left, right) => left.id.localeCompare(right.id));
        const placedNodeIds = new Set<string>();
        const horizontalGap = Math.max($viewSettings.colaLinkDistInterParent * 1.15, 220);
        const verticalGap = Math.max(height / Math.max(roots.length + 1, 2), 140);
        const localLeafXGap = Math.max($viewSettings.colaLinkDistIntraParent * 0.7, 80);
        const localLeafYGap = 78;
        const rootStartX = Math.max(120, horizontalGap * 0.7);
        const rootStartY = Math.max(140, verticalGap * 0.7);

        const setNodePosition = (node: GraphNode, x: number, y: number, jitterScale = 1) => {
            const mutableNode = node as GraphNode & { fixed?: number; px?: number; py?: number };
            if (mutableNode.fixed) return;

            const jitterX = (Math.random() - 0.5) * 36 * jitterScale;
            const jitterY = (Math.random() - 0.5) * 36 * jitterScale;
            mutableNode.x = x + jitterX;
            mutableNode.y = y + jitterY;
            mutableNode.px = mutableNode.x;
            mutableNode.py = mutableNode.y;
            placedNodeIds.add(node.id);
        };

        const placeSubtree = (node: GraphNode, depth: number, centerY: number) => {
            const nodeX = Math.min(width - 140, rootStartX + depth * horizontalGap);
            const nodeY = Math.max(80, Math.min(height - 80, centerY));
            setNodePosition(node, nodeX, nodeY, 1);

            const children = (childrenByParent.get(node.id) || []).slice().sort((left, right) => left.id.localeCompare(right.id));
            const childParents = children.filter((child) => parentIds.has(child.id));
            const leafChildren = children.filter((child) => !parentIds.has(child.id));

            if (leafChildren.length > 0) {
                const columns = Math.max(1, Math.ceil(Math.sqrt(leafChildren.length)));
                leafChildren.forEach((leafChild, index) => {
                    const column = index % columns;
                    const row = Math.floor(index / columns);
                    const offsetX = ((column - (columns - 1) / 2) * localLeafXGap) + localLeafXGap;
                    const offsetY = (row - (Math.ceil(leafChildren.length / columns) - 1) / 2) * localLeafYGap;
                    setNodePosition(leafChild, nodeX + offsetX, nodeY + offsetY, 0.6);
                });
            }

            if (childParents.length > 0) {
                const subtreeHeight = Math.max(verticalGap, childParents.length * 120);
                const startY = nodeY - subtreeHeight / 2;
                const stepY = childParents.length === 1 ? 0 : subtreeHeight / (childParents.length - 1);

                childParents.forEach((childParent, index) => {
                    placeSubtree(childParent, depth + 1, startY + stepY * index);
                });
            }
        };

        if (roots.length > 0) {
            roots.forEach((root, index) => {
                placeSubtree(root, 0, rootStartY + index * verticalGap);
            });
        }

        const aspect = width / Math.max(height, 1);
        const columns = Math.max(1, Math.ceil(Math.sqrt(nodes.length * aspect)));
        const rows = Math.max(1, Math.ceil(nodes.length / columns));
        const cellWidth = width / columns;
        const cellHeight = height / rows;
        const marginX = Math.max(24, cellWidth * 0.18);
        const marginY = Math.max(24, cellHeight * 0.18);

        nodes.forEach((node: any, index) => {
            if (placedNodeIds.has(node.id) || node.fixed) return;

            const column = index % columns;
            const row = Math.floor(index / columns);
            const jitterX = (Math.random() - 0.5) * Math.max(12, cellWidth * 0.25);
            const jitterY = (Math.random() - 0.5) * Math.max(12, cellHeight * 0.25);

            node.x = column * cellWidth + marginX + jitterX;
            node.y = row * cellHeight + marginY + jitterY;
            node.px = node.x;
            node.py = node.y;
        });
    }

    export function toggleRunning() {
        if (running) {
            if (colaLayout) colaLayout.stop();
            running = false;
            return;
        }

        jiggleNodes(DEFAULT_JIGGLE_RADIUS);
        rebuild(RESTART_START_ITERATIONS, 1);
    }

    // Full physics rebuild only when structure (node/link set) or Cola params change
    let lastStructureKey = '';
    let lastColaParams = '';
    $: {
        const sk = $graphStructureKey;
        const cp = [
            $viewSettings.colaLinkDistIntraParent,
            $viewSettings.colaLinkWeightIntraParent,
            $viewSettings.colaLinkDistInterParent,
            $viewSettings.colaLinkWeightInterParent,
            $viewSettings.colaLinkDistDependsOn,
            $viewSettings.colaLinkWeightDependsOn,
            $viewSettings.colaLinkDistRef,
            $viewSettings.colaLinkWeightRef,
            $viewSettings.colaConvergence,
            $viewSettings.colaGroupPadding,
            $viewSettings.colaAvoidOverlaps,
            $viewSettings.colaGroups,
            $viewSettings.colaLinks,
            $viewSettings.colaHandleDisconnected,
            $filters.edgeParent,
            $filters.edgeDependencies,
            $filters.edgeReferences,
        ].join('|');
        if (
            containerGroup &&
            $graphData &&
            nodesLayer &&
            hullLayer &&
            (sk !== lastStructureKey || cp !== lastColaParams)
        ) {
            lastStructureKey = sk;
            lastColaParams = cp;
            rebuild(FULL_START_ITERATIONS, 0.1, true);
        }
    }

    // ─── Group building ────────────────────────────────────────────────────────

    function buildColaGroups(activeNodes: GraphNode[], _activeLinks: GraphEdge[]): any[] {
        const nodeIndex = new Map(activeNodes.map((n, i) => [n.id, i]));
        const childrenOf = new Map<string, Set<number>>();
        const childIdsOf = new Map<string, string[]>();

        for (const n of activeNodes) {
            const pid = (n as any)._safe_parent;
            if (!pid) continue;
            const pidx = nodeIndex.get(pid);
            const cidx = nodeIndex.get(n.id);
            if (pidx === undefined || cidx === undefined) continue;

            if (!childrenOf.has(pid)) childrenOf.set(pid, new Set());
            childrenOf.get(pid)!.add(cidx);

            if (!childIdsOf.has(pid)) childIdsOf.set(pid, []);
            childIdsOf.get(pid)!.push(n.id);
        }

        const groups: any[] = [];
        const parentIds = new Set(childrenOf.keys());
        const descendantCache = new Map<string, string[]>();

        const collectDescendants = (parentId: string): string[] => {
            const cached = descendantCache.get(parentId);
            if (cached) return cached;

            const descendants = new Set<string>([parentId]);
            for (const childId of childIdsOf.get(parentId) || []) {
                descendants.add(childId);
                for (const nestedId of collectDescendants(childId)) {
                    descendants.add(nestedId);
                }
            }

            const result = Array.from(descendants);
            descendantCache.set(parentId, result);
            return result;
        };

        groupMembers = new Map();

        for (const pid of parentIds) {
            const pidx = nodeIndex.get(pid);
            if (pidx === undefined) continue;

            groupMembers.set(pid, collectDescendants(pid));

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
                        reheatLayout(DRAG_REHEAT);
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

        if (!$viewSettings.colaGroups) {
            d3.select(hullLayer).selectAll<SVGRectElement, unknown>("rect.cola-group").remove();
            return;
        }

        type GB = { x: number; y: number; w: number; h: number; containerId: string };
        const data: GB[] = [];
        const nodeBounds = new Map<string, { left: number; right: number; top: number; bottom: number }>();

        for (const node of $graphData?.nodes || []) {
            const width = (node as any).width ?? (node.w + 12);
            const height = (node as any).height ?? (node.h + 24);
            const x = node.x ?? 0;
            const y = node.y ?? 0;

            nodeBounds.set(node.id, {
                left: x - width / 2,
                right: x + width / 2,
                top: y - height / 2,
                bottom: y + height / 2,
            });
        }

        const padding = $viewSettings.colaGroupPadding + 10;

        for (const [containerId, memberIds] of groupMembers) {
            const bounds = memberIds.map((memberId) => nodeBounds.get(memberId)).filter(Boolean) as Array<{ left: number; right: number; top: number; bottom: number }>;
            if (bounds.length <= 1) continue;

            let left = Infinity;
            let right = -Infinity;
            let top = Infinity;
            let bottom = -Infinity;

            for (const bound of bounds) {
                left = Math.min(left, bound.left);
                right = Math.max(right, bound.right);
                top = Math.min(top, bound.top);
                bottom = Math.max(bottom, bound.bottom);
            }

            data.push({
                x: left - padding,
                y: top - padding,
                w: (right - left) + padding * 2,
                h: (bottom - top) + padding * 2,
                containerId,
            });
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

    function rebuild(startIterations: readonly [number, number, number] = FULL_START_ITERATIONS, restartHeat = 0.1, resetPositions = false) {
        if (!$graphData) return;
        if (colaLayout) { colaLayout.stop(); colaLayout = null; }
        running = false;

        const [initialUnconstrainedIterations, initialUserConstraintIterations, initialAllConstraintsIterations] = startIterations;

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
        colaGroups = $viewSettings.colaGroups ? buildColaGroups(nodes, links) : [];

        // Canvas from CANVAS_AREA
        const svg = containerGroup?.ownerSVGElement;
        const vw = svg?.clientWidth || window.innerWidth || 1400;
        const vh = svg?.clientHeight || window.innerHeight || 900;
        const aspect = vw / vh;
        const ch = Math.round(Math.sqrt(CANVAS_AREA / aspect));
        const cw = Math.round(ch * aspect);

        if (resetPositions) {
            seedNodesAcrossCanvas(nodes, cw, ch);
        } else {
            // Simple random distribution across the canvas area for initial unconstrained layout
            nodes.forEach((n: any) => {
                if (typeof n.x !== 'number' || n.x < -9000) {
                    n.x = (Math.random() * cw * 0.8) + (cw * 0.1);
                    n.y = (Math.random() * ch * 0.8) + (ch * 0.1);
                }
            });
        }

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
        const parentNodeIds = new Set(nodes.filter((candidate) => nodes.some((node) => (node as any)._safe_parent === candidate.id)).map((node) => node.id));
        const colaLinks = ($viewSettings.colaLinks ? links : []).filter((l: any) => {
            if (typeof l.source !== 'object' || typeof l.target !== 'object') return false;

            if (l.type === 'parent' && $viewSettings.colaGroups) {
                return parentNodeIds.has(l.target.id);
            }

            return true;
        }).map((l: any) => {
            // Apply physics settings
            let length = 1;
            let weight = 0.05;
            let opacity = l.opacity ?? 0.6;
            if (l.type === 'parent') {
                // If the target (child) is a parent itself, it's in its own group (inter-group link).
                // If it's not a parent, it's inside the source's group (intra-group link).
                const isChildParent = parentNodeIds.has(l.target.id);
                if (isChildParent) {
                    length = $viewSettings.colaLinkDistInterParent;
                    weight = $viewSettings.colaLinkWeightInterParent;
                } else {
                    length = $viewSettings.colaLinkDistIntraParent;
                    weight = $viewSettings.colaLinkWeightIntraParent;
                }
            } else if (l.type === 'depends_on' || l.type === 'soft_depends_on') {
                length = $viewSettings.colaLinkDistDependsOn;
                weight = $viewSettings.colaLinkWeightDependsOn;
            } else if (l.type === 'ref') {
                length = $viewSettings.colaLinkDistRef;
                weight = $viewSettings.colaLinkWeightRef;
            }
            return { ...l, length, weight, opacity };
        });

        if (linksLayer) {
            d3.select(linksLayer).selectAll("line.link")
                .data(colaLinks)
                .join("line").attr("class", "link")
                .attr("stroke", (d: any) => d.color || "#cbd5e1")
                .attr("stroke-width", (d: any) => d.width || 1.5)
                .attr("stroke-dasharray", (d: any) => d.dash || null)
                .attr("opacity", (d: any) => d.opacity ?? 0.6);
        }

        colaLayout = cola.d3adaptor(d3)
            .size([cw, ch])
            .nodes(nodes as any)
            .links(colaLinks as any)
            .groups(colaGroups)
            .linkDistance((l: any) => l.length)
            .convergenceThreshold($viewSettings.colaConvergence)
            .avoidOverlaps($viewSettings.colaAvoidOverlaps)
            .handleDisconnected($viewSettings.colaHandleDisconnected)
            .on("tick", tickVisuals)
            .on("end", () => {
                running = false;
                fitGraph(0, false);
            })
            .start(initialUnconstrainedIterations, initialUserConstraintIterations, initialAllConstraintsIterations);

        colaLayout.alpha(restartHeat);

        running = true;

        // Force initial render
        tickVisuals();
        fitGraph();
    }

    export function randomize() {
        if (!$graphData) return;

        jiggleNodes(DEFAULT_JIGGLE_RADIUS);
        rebuild(RESTART_START_ITERATIONS, 1);
    }

    $: if (restartNonce !== lastRestartNonce) {
        lastRestartNonce = restartNonce;
        if (restartNonce > 0 && $graphData) {
            jiggleNodes(DEFAULT_JIGGLE_RADIUS);
            rebuild(RESTART_START_ITERATIONS, 1);
        }
    }

    $: if (randomizeNonce !== lastRandomizeNonce) {
        lastRandomizeNonce = randomizeNonce;
        if (randomizeNonce > 0 && $graphData) {
            randomize();
        }
    }

    $: if (running !== lastRunning) {
        const nextRunning = running;
        lastRunning = running;

        if (!nextRunning && colaLayout) {
            colaLayout.stop();
        }
    }

    onDestroy(() => {
        if (colaLayout) colaLayout.stop();
        running = false;
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
