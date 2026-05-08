<script lang="ts">
    import * as d3 from "d3";
    import { graphData } from "../../stores/graph";
    import { toggleSelection, selection } from "../../stores/selection";
    import { viewSettings } from "../../stores/viewSettings";
    import { buildCirclePackNode } from "../shared/NodeShapes";
    import { routeContainmentEdges } from "../shared/EdgeRenderer";
    import {
        focusSize,
        maxFocusOf,
        emphasisOpacity,
        computeSelectionMask,
    } from "../../data/focusEmphasis";
    import { FOCUS_PICK } from "../../data/nodeAffordances";
    import { COMPLETED_STATUSES } from "../../data/constants";

    import { onMount } from "svelte";

    export let containerGroup: SVGGElement;

    let nodesLayer: SVGGElement;
    let edgesLayer: SVGGElement;

    let layoutComputed = false;
    let lastGraphData: any = null;

    // Minimum pixel-space radius for showing text labels
    const MIN_TEXT_PIXEL_RADIUS = 10;

    onMount(() => {
        // Watch the zoom transform on the parent SVG to toggle text visibility
        const svg = containerGroup?.closest("svg");
        if (!svg) return;

        const observer = new MutationObserver(() => updateTextVisibility());
        observer.observe(containerGroup, {
            attributes: true,
            attributeFilter: ["transform"],
        });

        // Initial visibility pass after layout settles
        setTimeout(updateTextVisibility, 200);

        return () => observer.disconnect();
    });

    function getZoomScale(): number {
        if (!containerGroup) return 1;
        const transform = containerGroup.getAttribute("transform") || "";
        const m = transform.match(/scale\(([^)]+)\)/);
        if (m) return parseFloat(m[1]);
        // d3 uses matrix form: translate(x,y) scale(k)
        // or matrix(a,b,c,d,e,f) where a=k
        const mat = transform.match(/matrix\(([^,]+)/);
        if (mat) return parseFloat(mat[1]);
        return 1;
    }

    // Minimum pixel radius for parent labels to be visible
    const MIN_PARENT_LABEL_PIXEL_RADIUS = 48;

    function updateTextVisibility() {
        if (!nodesLayer) return;
        const k = getZoomScale();
        const nodes = nodesLayer.querySelectorAll<SVGGElement>("g.node");
        nodes.forEach((g) => {
            const d = d3.select(g).datum() as any;
            if (!d) return;
            const r = d._lr || 0;
            const pixelR = r * k;

            if (d._isLeaf) {
                // Leaf nodes: hide text when circle too small on screen
                g.querySelectorAll("text, foreignObject").forEach((el) => {
                    (el as SVGElement).style.display =
                        pixelR < MIN_TEXT_PIXEL_RADIUS ? "none" : "";
                });
            } else {
                // Parent nodes: hide label + pill when container too small
                g.querySelectorAll(".parent-label, .parent-label-bg").forEach(
                    (el) => {
                        (el as SVGElement).style.display =
                            pixelR < MIN_PARENT_LABEL_PIXEL_RADIUS
                                ? "none"
                                : "";
                    },
                );
            }
        });
    }

    $: {
        if (
            containerGroup &&
            $graphData &&
            nodesLayer &&
            edgesLayer &&
            $selection &&
            $viewSettings.circleRollupThreshold
        ) {
            const dataChanged = $graphData !== lastGraphData;
            if (dataChanged) {
                computeCirclePackLayout();
                lastGraphData = $graphData;
                layoutComputed = true;
            }
            if (layoutComputed) {
                renderCirclePackNodes();
            }
        }
    }

    function computeCirclePackLayout() {
        if (!$graphData) return;

        const data = $graphData;
        const nodes = data.nodes;

        const rootId = "__root__";
        const nodeIdSet = new Set(nodes.map((n) => n.id));
        const packNodes = [
            { id: rootId, parent: "", type: "root" },
            ...nodes.map((n) => ({
                ...n,
                parent: n.parent && nodeIdSet.has(n.parent) ? n.parent : rootId,
            })),
        ];

        let root;
        try {
            root = d3
                .stratify<any>()
                .id((d) => d.id)
                .parentId((d) => d.parent)(packNodes);
        } catch (e) {
            console.warn("Stratify failed for Circle Pack", e);
            return;
        }

        // Use .radius() so leaves get explicit radii and parents auto-size
        // as minimum enclosing circles — no .size() inflation.
        root.each((node: any) => {
            node.data._isHierarchyParent = !!node.children;
        });

        // Leaves sized by central focus → radius mapping.
        const MIN_R = 6;
        const MAX_R = 70;
        const maxFocus = maxFocusOf(nodes);
        const leafRadius = (d: any) =>
            focusSize(d.focusScore, maxFocus, MIN_R, MAX_R);

        // value (used only for sort ordering) is area ∝ r²
        root.sum((d: any) => {
            if (d._isHierarchyParent) return 0;
            const r = leafRadius(d);
            return r * r;
        });
        root.sort((a, b) => (b.value || 0) - (a.value || 0));

        const pack = d3
            .pack<any>()
            .radius((d: any) => leafRadius(d.data))
            .padding((d: any) => {
                if (!d.children) return 0.5;
                return 1;
            });

        pack(root);

        // Center on the root node's position
        const rootX = root.x ?? 0;
        const rootY = root.y ?? 0;

        const layoutMap = new Map();
        root.descendants().forEach((d: any) => {
            if (d.data.id === rootId) return;
            layoutMap.set(d.data.id, {
                x: d.x - rootX,
                y: d.y - rootY,
                r: d.r,
                depth: d.depth,
                isLeaf: !d.children,
                d3Node: d,
            });
        });

        nodes.forEach((n) => {
            const l = layoutMap.get(n.id);
            if (l) {
                n.x = l.x;
                n.y = l.y;
                n.depth = l.depth;
                n._lr = l.r;
                n._isLeaf = l.isLeaf;
            } else {
                n.x = -9999;
                n.y = -9999;
            }
        });
    }

    function renderCirclePackNodes() {
        const data = $graphData;
        if (!data) return;

        // Sort by depth for correct z-order (parents behind children)
        const visibleNodes = data.nodes
            .filter((n) => (n.x || 0) > -9000)
            .sort((a, b) => (a.depth || 0) - (b.depth || 0));

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

        const activeNodeId = $selection.activeNodeId;
        const focusEgoId = $selection.focusNodeId;
        const focusEgoSet = $selection.focusNeighborSet;
        const focusIds: Set<string> =
            ($graphData as any)?.focusIds || new Set();
        const showPicks =
            $viewSettings.showFocusHighlight && focusIds.size > 0;

        const mask = computeSelectionMask(
            visibleNodes,
            data.links,
            activeNodeId,
            { includeSiblings: false },
        );

        nEls.each(function (d) {
            const g = d3.select(this);
            const isSelected = d.id === activeNodeId;

            const lastSelected = d._lastSelected;
            if (g.selectAll("*").empty() || lastSelected !== isSelected) {
                g.selectAll("*").remove();
                const tempD = { ...d, _lr: d._lr, isLeaf: d._isLeaf };
                buildCirclePackNode(g as any, tempD, isSelected);
                d._lastSelected = isSelected;

                if (showPicks && d._isLeaf && focusIds.has(d.id)) {
                    g.insert("circle", ":first-child")
                        .attr("class", "focus-ring")
                        .attr("cx", 0).attr("cy", 0)
                        .attr("r", (d._lr || 5))
                        .attr("fill", "none")
                        .attr("stroke", FOCUS_PICK.outerColor)
                        .attr("stroke-width", FOCUS_PICK.outerWidth)
                        .attr("stroke-opacity", FOCUS_PICK.outerOpacity)
                        .style("pointer-events", "none");
                    g.insert("circle", ":first-child")
                        .attr("class", "focus-ring")
                        .attr("cx", 0).attr("cy", 0)
                        .attr("r", (d._lr || 5) - 1.5)
                        .attr("fill", "none")
                        .attr("stroke", FOCUS_PICK.innerColor)
                        .attr("stroke-width", FOCUS_PICK.innerWidth)
                        .style("pointer-events", "none");
                }
            }
        });

        nEls.style("opacity", (d: any) => {
            const inMask = !!mask && mask.has(d.id);
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
        });

        const eEls = d3
            .select(edgesLayer)
            .selectAll("path")
            .data(data.links)
            .join("path");

        routeContainmentEdges(eEls);

        setTimeout(updateTextVisibility, 50);
    }
</script>

{#if containerGroup}
    <g bind:this={edgesLayer}></g>
    <g bind:this={nodesLayer}></g>
{/if}
