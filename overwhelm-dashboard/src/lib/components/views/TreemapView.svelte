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
    import {
        focusSize,
        maxFocusOf,
        emphasisOpacity,
        computeSelectionMask,
    } from "../../data/focusEmphasis";
    import { FOCUS_PICK } from "../../data/nodeAffordances";
    import { COMPLETED_STATUSES } from "../../data/constants";

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

    let visibleNodes = $state<any[]>([]);
    let links = $state<any[]>([]);

    // Layout — re-tiles when graph data, container size, or project filter
    // changes. Mutations to node x/y/_lw/_lh inside computeLayout would re-fire
    // this effect without untrack, since visibleNodes is reactive $state.
    $effect(() => {
        const data = $graphData;
        const _h = canvasH;
        const _projectFilter = ($filters as any).projectFilter;
        if (containerGroup && data && nodesLayer) {
            untrack(() => computeLayout(data));
        }
    });

    // Selection / focus highlight — repaints rings and opacities without
    // re-tiling. Read selection at top so the effect tracks it; do the DOM
    // work inside untrack so node-data writes don't loop.
    $effect(() => {
        const _active = $selection.activeNodeId;
        const _focusEgo = $selection.focusNodeId;
        const _focusEgoSet = $selection.focusNeighborSet;
        const _showPicks = $viewSettings.showFocusHighlight;
        if (nodesLayer && visibleNodes.length) {
            untrack(() => applyHighlights());
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
        const MIN_WEIGHT = 10;
        const MAX_WEIGHT = 200;
        const maxFocus = maxFocusOf(nodes);

        // d.children is undefined on raw data — parent/child is by ID. Mark
        // hierarchy parents so .sum() knows to ignore their own focus weight.
        root.each((node: any) => {
            node.data._isHierarchyParent = !!node.children;
        });

        root.sum((d) => {
            if (d._isHierarchyParent) return 0;
            return focusSize(d.focusScore, maxFocus, MIN_WEIGHT, MAX_WEIGHT);
        });

        // Stable sort: tie-break with id to prevent jumping on re-renders.
        root.sort(
            (a, b) =>
                (b.value || 0) - (a.value || 0) || a.id!.localeCompare(b.id!),
        );

        function estimateHeaderHeight(node: any): number {
            if (node.depth === 0) return 4;
            if (!node.children) return 0;
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
            let leafCount = 0;
            if (d.children) d.leaves().forEach(() => leafCount++);
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
            });

        nEls.each(function (d) {
            (d as any)._lastSelected = undefined;
            d3.select(this).selectAll("*").remove();
        });

        applyHighlights();

        const eEls = d3
            .select(edgesLayer)
            .selectAll("path")
            .data(links)
            .join("path");
        routeTreemapEdges(eEls);
    }

    function applyHighlights() {
        if (!nodesLayer) return;

        const activeNodeId = $selection.activeNodeId;
        const focusEgoId = $selection.focusNodeId;
        const focusEgoSet = $selection.focusNeighborSet;
        const focusIds: Set<string> =
            ($graphData as any)?.focusIds || new Set();
        const showPicks =
            $viewSettings.showFocusHighlight && focusIds.size > 0;

        // Selection mask trumps focus-pick dimming. Treemap doesn't include
        // siblings in the mask — the layout already groups them by parent.
        const mask = computeSelectionMask(
            visibleNodes,
            links,
            activeNodeId,
            { includeSiblings: false },
        );

        const nEls = d3
            .select(nodesLayer)
            .selectAll<SVGGElement, any>("g.node");

        nEls.each(function (d) {
            const g = d3.select(this);
            const isSelected = d.id === activeNodeId;
            const lastSelected = (d as any)._lastSelected;

            // Rebuild only when selection state for THIS node has changed.
            if (g.selectAll("*").empty() || lastSelected !== isSelected) {
                g.selectAll("*").remove();
                buildTreemapNode(g, d, isSelected);
                (d as any)._lastSelected = isSelected;

                if (showPicks && d._isLeaf && focusIds.has(d.id)) {
                    appendFocusPickRings(g, d);
                }
            }
        });

        nEls.style("opacity", (d: any) =>
            computeNodeOpacity(d, mask, focusEgoId, focusEgoSet),
        );
    }

    function appendFocusPickRings(
        g: d3.Selection<SVGGElement, any, any, any>,
        d: any,
    ) {
        const focusW = d._lw || d.w;
        const focusH = d._lh || d.h;
        g.append("rect")
            .attr("class", "focus-ring")
            .attr("x", -focusW / 2)
            .attr("y", -focusH / 2)
            .attr("width", focusW)
            .attr("height", focusH)
            .attr("fill", "none")
            .attr("stroke", FOCUS_PICK.outerColor)
            .attr("stroke-width", FOCUS_PICK.outerWidth)
            .attr("stroke-opacity", FOCUS_PICK.outerOpacity)
            .style("pointer-events", "none");
        g.append("rect")
            .attr("class", "focus-ring")
            .attr("x", -focusW / 2 + 1.5)
            .attr("y", -focusH / 2 + 1.5)
            .attr("width", Math.max(0, focusW - 3))
            .attr("height", Math.max(0, focusH - 3))
            .attr("fill", "none")
            .attr("stroke", FOCUS_PICK.innerColor)
            .attr("stroke-width", FOCUS_PICK.innerWidth)
            .attr("rx", 3)
            .style("pointer-events", "none");
    }

    function computeNodeOpacity(
        d: any,
        mask: Set<string> | null,
        focusEgoId: string | null,
        focusEgoSet: Set<string> | null,
    ): number | null {
        const inMask = !!mask && mask.has(d.id);

        // Ego mode hides outside-set nodes. Filter-half is suppressed for
        // nodes inside the selection mask: the user has explicitly focused
        // on them, overriding any priority-bucket dim.
        let visibilityState: "bright" | "half" | "hidden" =
            !inMask && d.filter_dimmed ? "half" : "bright";
        if (focusEgoId && focusEgoSet && !focusEgoSet.has(d.id)) {
            visibilityState = "hidden";
        }

        const op = emphasisOpacity({
            prominence: d.prominence ?? 0,
            isCompleted: COMPLETED_STATUSES.has(d.status),
            visibilityState,
            selectionMasked: !!mask && !inMask,
        });
        return op < 1 ? op : null;
    }
</script>

{#if containerGroup}
    <g bind:this={edgesLayer}></g>
    <g bind:this={nodesLayer}></g>
{/if}
