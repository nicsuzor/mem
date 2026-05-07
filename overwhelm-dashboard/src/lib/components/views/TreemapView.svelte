<script lang="ts">
    import { untrack } from "svelte";
    import * as d3 from "d3";
    import { graphData } from "../../stores/graph";
    import { filters } from "../../stores/filters";
    import { toggleSelection, selection } from "../../stores/selection";
    import {
        buildTreemapNode,
        treemapHeaderMetrics,
    } from "../shared/NodeShapes";
    import { routeTreemapEdges } from "../shared/EdgeRenderer";
    import { viewSettings } from "../../stores/viewSettings";
    import type { GraphEdge } from "../../data/prepareGraphData";
    import { focusSize, maxFocusOf } from "../../data/nodeSize";

    let {
        containerGroup,
        width = 2000,
        height = 1000,
    } = $props<{
        containerGroup: SVGGElement | null;
        width?: number;
        height?: number;
    }>();

    let nodesLayer = $state<SVGGElement>(undefined!);
    let edgesLayer = $state<SVGGElement>(undefined!);

    const canvasW = 3000;
    const canvasH = $derived(
        canvasW * (height && width ? height / width : 0.5),
    );

    // Reactive state for the computed layout
    let visibleNodes = $state<any[]>([]);
    let links = $state<any[]>([]);

    $effect(() => {
        // Run layout ONLY when graph data changes.
        // untrack() prevents Svelte from auto-tracking reactive reads inside
        // computeLayout (e.g. canvasH, $filters) — those would cause a
        // re-layout when the container resizes (sidebar open/close on click).
        const _data = $graphData;
        if (containerGroup && _data && nodesLayer) {
            untrack(() => computeLayout(_data));
        }
    });

    $effect(() => {
        // Update Highlights ONLY when selection changes
        const activeNodeId = $selection.activeNodeId;
        const hoveredNodeId = $selection.hoveredNodeId;

        if (nodesLayer) {
            d3.select(nodesLayer)
                .selectAll<SVGGElement, any>("g.node")
                .each(function (d) {
                    const g = d3.select(this);
                    const isSelected = d.id === activeNodeId;
                    const isHovered = d.id === hoveredNodeId;
                    const needsHighlight = isSelected || isHovered;

                    const lastState = (d as any)._lastHighlight;
                    if (
                        g.selectAll("*").empty() ||
                        lastState !== needsHighlight
                    ) {
                        g.selectAll("*").remove();
                        buildTreemapNode(g, d, needsHighlight);
                        (d as any)._lastHighlight = needsHighlight;
                    }
                });
        }
    });

    function computeLayout(data: any) {
        const nodes = data.nodes;
        const virtualRootId = "__treemap_root__";
        const nodeIdSet = new Set(nodes.map((n: any) => n.id));
        const projectRootId = ($filters as any).projectFilter as
            | string
            | undefined;

        let stratifyNodes: any[];
        let rootId: string;

        if (projectRootId && nodeIdSet.has(projectRootId)) {
            rootId = projectRootId;
            stratifyNodes = nodes
                .map((n: any) => ({
                    ...n,
                    parent:
                        n.parent && nodeIdSet.has(n.parent) && n.id !== rootId
                            ? n.parent
                            : n.id === rootId
                              ? ""
                              : "__ignore__",
                }))
                .filter((n: any) => n.parent !== "__ignore__");
        } else {
            rootId = virtualRootId;
            stratifyNodes = [
                { id: rootId, parent: "", type: "root", label: "ROOT" },
            ];

            for (const n of nodes) {
                const effectiveParent =
                    n.parent && nodeIdSet.has(n.parent) ? n.parent : rootId;
                stratifyNodes.push({ ...n, parent: effectiveParent });
            }
        }

        let root;
        try {
            root = d3
                .stratify<any>()
                .id((d) => d.id)
                .parentId((d) => d.parent)(stratifyNodes);
        } catch (e) {
            console.error("Stratify failed for Treemap", e);
            return;
        }

        // Weight unit range — d3.treemap allocates area proportional to value.
        // Min=10, max=200 softens the dynamic range slightly from the extreme 5-500,
        // so critical tasks still pop but don't completely swallow normal tasks.
        const MIN_WEIGHT = 10;
        const MAX_WEIGHT = 200;
        const maxFocus = maxFocusOf(nodes);

        // Mark hierarchy parents — d.children on raw data is always undefined
        // since parent-child is defined by parent ID, not nested arrays.
        // Without this, every node (including parents) gets its own weight,
        // inflating containers beyond what their children fill.
        root.each((node: any) => {
            node.data._isHierarchyParent = !!node.children;
        });

        root.sum((d) => {
            if (d._isHierarchyParent) return 0;
            return focusSize(d.focusScore, maxFocus, MIN_WEIGHT, MAX_WEIGHT);
        });

        // STABLE SORT: Tie-break with ID to prevent jumping on re-renders
        root.sort(
            (a, b) =>
                (b.value || 0) - (a.value || 0) || a.id!.localeCompare(b.id!),
        );

        // Use the same header height computation as the renderer
        function estimateHeaderHeight(node: any): number {
            if (node.depth === 0) return 4; // virtual root
            if (!node.children) return 0; // leaves don't need header padding
            const w = (node.x1 ?? canvasW) - (node.x0 ?? 0);
            const h = (node.y1 ?? canvasH) - (node.y0 ?? 0);
            const label = node.data?.label || "";
            if (!label || w < 25) return 14;
            return treemapHeaderMetrics(w, h, label, node.depth).headerH;
        }

        const treemap = d3
            .treemap<any>()
            .size([canvasW, canvasH])
            .paddingInner((node: any) =>
                node.depth <= 1 ? 14 : node.depth <= 2 ? 6 : 5,
            )
            .paddingBottom((node: any) =>
                node.depth <= 1 ? 10 : node.depth <= 2 ? 5 : 4,
            )
            .paddingLeft((node: any) =>
                node.depth <= 1 ? 10 : node.depth <= 2 ? 5 : 4,
            )
            .paddingRight((node: any) =>
                node.depth <= 1 ? 10 : node.depth <= 2 ? 5 : 4,
            )
            .paddingTop((node: any) => estimateHeaderHeight(node))
            .tile(d3.treemapSquarify.ratio(1.618))
            .round(true);

        treemap(root);

        const layoutMap = new Map();
        root.descendants().forEach((d: any) => {
            if (d.data.id === rootId) return;
            // Count leaf descendants (actual tasks inside this container)
            let leafCount = 0;
            if (d.children) {
                d.leaves().forEach(() => leafCount++);
            }
            layoutMap.set(d.data.id, {
                x: d.x0 + (d.x1 - d.x0) / 2,
                y: d.y0 + (d.y1 - d.y0) / 2,
                w: Math.max(14, d.x1 - d.x0),
                h: Math.max(14, d.y1 - d.y0),
                depth: d.depth,
                isLeaf: !d.children || d.children.length === 0,
                leafCount,
            });
        });

        visibleNodes = nodes
            .filter((n: any) => {
                const l = layoutMap.get(n.id);
                if (l) {
                    n.x = l.x;
                    n.y = l.y;
                    n._lw = l.w;
                    n._lh = l.h;
                    n.depth = l.depth;
                    n._isLeaf = l.isLeaf;
                    n._leafCount = l.leafCount;
                    return true;
                }
                n.x = -9999;
                return false;
            })
            .sort((a: any, b: any) => (a.depth || 0) - (b.depth || 0));

        links = data.links;

        renderNodes();
    }

    function renderNodes() {
        if (!nodesLayer) return;

        const nEls = d3
            .select(nodesLayer)
            .selectAll<SVGGElement, any>("g.node")
            .data(visibleNodes, (d: any) => d.id)
            .join("g")
            .attr("class", "node")
            .attr("transform", (d) => `translate(${d.x},${d.y})`)
            .style("cursor", "pointer")
            .on("click", (e, d) => {
                e.stopPropagation();
                toggleSelection(d.id);
            })
            .on("mouseenter", (e, d) => {
                selection.update((s) => ({ ...s, hoveredNodeId: d.id }));
            })
            .on("mouseleave", () => {
                selection.update((s) => ({ ...s, hoveredNodeId: null }));
            });

        // Current selection state for initial build
        const activeNodeId = $selection.activeNodeId;
        const hoveredNodeId = $selection.hoveredNodeId;

        const focusIds: Set<string> =
            ($graphData as any)?.focusIds || new Set();
        const showFocus = $viewSettings.showFocusHighlight && focusIds.size > 0;

        nEls.each(function (d) {
            const g = d3.select(this);
            const needsHighlight =
                d.id === activeNodeId || d.id === hoveredNodeId;
            g.selectAll("*").remove();
            buildTreemapNode(g, d, needsHighlight);
            (d as any)._lastHighlight = needsHighlight;

            // Focus accent: prominent glowing border on priority focus tasks
            if (showFocus && d._isLeaf && focusIds.has(d.id)) {
                const focusW = d._lw || d.w;
                const focusH = d._lh || d.h;
                
                // Outer glow
                g.append("rect")
                    .attr("x", -focusW / 2).attr("y", -focusH / 2)
                    .attr("width", focusW).attr("height", focusH)
                    .attr("fill", "none")
                    .attr("stroke", "#f59e0b")
                    .attr("stroke-width", 6)
                    .attr("stroke-opacity", 0.6)
                    .style("pointer-events", "none")
                    .append("animate")
                    .attr("attributeName", "stroke-opacity")
                    .attr("values", "0.2;0.8;0.2")
                    .attr("dur", "2s")
                    .attr("repeatCount", "indefinite");

                // Sharp inner highlight
                g.append("rect")
                    .attr("x", -focusW / 2 + 1.5).attr("y", -focusH / 2 + 1.5)
                    .attr("width", Math.max(0, focusW - 3)).attr("height", Math.max(0, focusH - 3))
                    .attr("fill", "none")
                    .attr("stroke", "#fbbf24")
                    .attr("stroke-width", 3)
                    .attr("rx", 3)
                    .style("pointer-events", "none");
            }
        });

        // Gentle dimming: non-focus leaf nodes slightly faded, and filter-dimmed nodes heavily faded
        if (showFocus) {
            nEls.style("opacity", (d: any) => {
                const baseOp = d.opacity ?? 1;
                if (d.filter_dimmed) return 0.65 * baseOp;
                if (!d._isLeaf) return baseOp < 1 ? baseOp : null; // Don't dim containers unless baseOp wants to
                if (focusIds.has(d.id)) return baseOp;
                return 0.65 * baseOp;
            });
        } else {
            nEls.style("opacity", (d: any) => {
                const baseOp = d.opacity ?? 1;
                return d.filter_dimmed ? 0.65 * baseOp : (baseOp < 1 ? baseOp : null);
            });
        }

        const eEls = d3
            .select(edgesLayer)
            .selectAll("path")
            .data(links)
            .join("path");
        routeTreemapEdges(eEls);
    }
</script>

{#if containerGroup}
    <g bind:this={edgesLayer}></g>
    <g bind:this={nodesLayer}></g>
{/if}
