<script lang="ts">
    import * as d3 from "d3";
    import { graphData } from "../../stores/graph";
    import { toggleSelection, selection } from "../../stores/selection";
    import { viewSettings } from "../../stores/viewSettings";
    import { buildCirclePackNode } from "../shared/NodeShapes";
    import { routeContainmentEdges } from "../shared/EdgeRenderer";
    import { focusSize, maxFocusOf } from "../../data/nodeSize";

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
            })
            .on("mouseenter", (e, d) => {
                selection.update((s) => ({ ...s, hoveredNodeId: d.id }));
            })
            .on("mouseleave", () => {
                selection.update((s) => ({ ...s, hoveredNodeId: null }));
            });

        const activeNodeId = $selection.activeNodeId;
        const hoveredNodeId = $selection.hoveredNodeId;
        const focusIds: Set<string> =
            ($graphData as any)?.focusIds || new Set();
        const showFocus = $viewSettings.showFocusHighlight && focusIds.size > 0;

        nEls.each(function (d) {
            const g = d3.select(this);
            const isSelected = d.id === activeNodeId;
            const isHovered = d.id === hoveredNodeId;

            const lastSelected = d._lastSelected;
            if (g.selectAll("*").empty() || lastSelected !== isSelected) {
                g.selectAll("*").remove();
                // Pass _isLeaf from mutated state
                const tempD = { ...d, _lr: d._lr, isLeaf: d._isLeaf };
                buildCirclePackNode(g as any, tempD, isSelected);
                d._lastSelected = isSelected;
            }

            g.classed("hovered-node", isHovered);

            // Focus highlight: glowing ring on priority focus leaf nodes
            if (showFocus && d._isLeaf) {
                g.selectAll(".focus-ring").remove();
                if (focusIds.has(d.id)) {
                    // Outer glow
                    g.insert("circle", ":first-child")
                        .attr("class", "focus-ring")
                        .attr("cx", 0)
                        .attr("cy", 0)
                        .attr("r", (d._lr || 5))
                        .attr("fill", "none")
                        .attr("stroke", "#f59e0b")
                        .attr("stroke-width", 8)
                        .attr("stroke-opacity", 0.6)
                        .style("pointer-events", "none")
                        .append("animate")
                        .attr("attributeName", "stroke-opacity")
                        .attr("values", "0.2;0.8;0.2")
                        .attr("dur", "2s")
                        .attr("repeatCount", "indefinite");

                    // Sharp inner highlight
                    g.insert("circle", ":first-child")
                        .attr("class", "focus-ring")
                        .attr("cx", 0)
                        .attr("cy", 0)
                        .attr("r", (d._lr || 5) - 1.5)
                        .attr("fill", "none")
                        .attr("stroke", "#fbbf24")
                        .attr("stroke-width", 3)
                        .style("pointer-events", "none");
                }
            }
        });

        // Gentle dimming: non-focus leaves slightly faded, and filter-dimmed nodes heavily faded
        if (showFocus) {
            nEls.style("opacity", (d: any) => {
                const baseOp = d.opacity ?? 1;
                if (d.filter_dimmed) return 0.2 * baseOp;
                if (!d._isLeaf) return baseOp < 1 ? baseOp : null;
                if (focusIds.has(d.id)) return baseOp;
                return 0.6 * baseOp;
            });
        } else {
            nEls.style("opacity", (d: any) => {
                const baseOp = d.opacity ?? 1;
                return d.filter_dimmed ? 0.2 * baseOp : (baseOp < 1 ? baseOp : null);
            });
        }

        const eEls = d3
            .select(edgesLayer)
            .selectAll("path")
            .data(data.links)
            .join("path");

        routeContainmentEdges(eEls);

        // Apply zoom-responsive text visibility after render
        setTimeout(updateTextVisibility, 50);
    }
</script>

{#if containerGroup}
    <g bind:this={edgesLayer}></g>
    <g bind:this={nodesLayer}></g>
{/if}
