<script lang="ts">
    import * as d3 from "d3";
    import { graphData } from "../../stores/graph";
    import { toggleSelection, selection } from "../../stores/selection";
    import { viewSettings } from "../../stores/viewSettings";
    import { buildCirclePackNode } from "../shared/NodeShapes";
    import { routeContainmentEdges } from "../shared/EdgeRenderer";

    import { onMount } from "svelte";

    export let containerGroup: SVGGElement;

    let nodesLayer: SVGGElement;
    let edgesLayer: SVGGElement;

    let layoutComputed = false;
    let lastGraphData: any = null;

    // Minimum pixel-space radius for showing text labels
    const MIN_TEXT_PIXEL_RADIUS = 12;

    onMount(() => {
        // Watch the zoom transform on the parent SVG to toggle text visibility
        const svg = containerGroup?.closest('svg');
        if (!svg) return;

        const observer = new MutationObserver(() => updateTextVisibility());
        observer.observe(containerGroup, { attributes: true, attributeFilter: ['transform'] });

        // Initial visibility pass after layout settles
        setTimeout(updateTextVisibility, 200);

        return () => observer.disconnect();
    });

    function getZoomScale(): number {
        if (!containerGroup) return 1;
        const transform = containerGroup.getAttribute('transform') || '';
        const m = transform.match(/scale\(([^)]+)\)/);
        if (m) return parseFloat(m[1]);
        // d3 uses matrix form: translate(x,y) scale(k)
        // or matrix(a,b,c,d,e,f) where a=k
        const mat = transform.match(/matrix\(([^,]+)/);
        if (mat) return parseFloat(mat[1]);
        return 1;
    }

    // Minimum pixel radius for parent labels to be visible
    const MIN_PARENT_LABEL_PIXEL_RADIUS = 25;

    function updateTextVisibility() {
        if (!nodesLayer) return;
        const k = getZoomScale();
        const nodes = nodesLayer.querySelectorAll<SVGGElement>('g.node');
        nodes.forEach(g => {
            const d = (d3.select(g).datum() as any);
            if (!d) return;
            const r = d._lr || 0;
            const pixelR = r * k;

            if (d._isLeaf) {
                // Leaf nodes: hide text when circle too small on screen
                g.querySelectorAll('text, foreignObject').forEach(el => {
                    (el as SVGElement).style.display = pixelR < MIN_TEXT_PIXEL_RADIUS ? 'none' : '';
                });
            } else {
                // Parent nodes: hide label + pill when container too small
                g.querySelectorAll('.parent-label, .parent-label-bg').forEach(el => {
                    (el as SVGElement).style.display = pixelR < MIN_PARENT_LABEL_PIXEL_RADIUS ? 'none' : '';
                });
            }
        });
    }

    $: {
        if (containerGroup && $graphData && nodesLayer && edgesLayer && $selection && $viewSettings.circleRollupThreshold) {
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
        const nodeIdSet = new Set(nodes.map(n => n.id));
        const packNodes = [
            { id: rootId, parent: "", type: "root" },
            ...nodes.map((n) => ({
                ...n,
                parent: (n.parent && nodeIdSet.has(n.parent)) ? n.parent : rootId,
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

        const COMPLETED_NODE_WEIGHT = 1;
        const MIN_ACTIVE_NODE_WEIGHT = 10;
        const DEFAULT_NODE_WEIGHT = 6;

        const computeSum = (d: any) => {
            if (d.children?.length) return 0;
            if (["done", "completed", "cancelled"].includes(d.status)) return COMPLETED_NODE_WEIGHT;
            // Ensure all active nodes get a meaningful minimum size
            return Math.max(MIN_ACTIVE_NODE_WEIGHT, d.dw || DEFAULT_NODE_WEIGHT);
        };

        root.sum(computeSum).sort((a, b) => {
            const pa = a.data.priority ?? 5;
            const pb = b.data.priority ?? 5;
            if (pa !== pb) return pa - pb;
            const statusOrder: Record<string, number> = { "active": 0, "blocked": 1, "waiting": 2, "review": 3, "done": 4, "completed": 4, "cancelled": 5 };
            const sa = statusOrder[a.data.status] ?? 10;
            const sb = statusOrder[b.data.status] ?? 10;
            if (sa !== sb) return sa - sb;
            return (b.value || 0) - (a.value || 0);
        });

        const PACK_SIZE = 10000;
        const pack = d3.pack<any>().size([PACK_SIZE, PACK_SIZE]).padding(4);

        pack(root);

        const nodesToRollup = new Set<string>();
        // Rollup logic removed to show all nodes again as per user request

        const layoutMap = new Map();
        root.descendants().forEach((d: any) => {
            if (d.data.id === rootId) return;
            layoutMap.set(d.data.id, {
                x: d.x - PACK_SIZE / 2, // center at 0
                y: d.y - PACK_SIZE / 2,
                r: d.r,
                depth: d.depth,
                isLeaf: !d.children,
                d3Node: d,
            });
        });

        nodes.forEach((n) => {
            const l = layoutMap.get(n.id);
            if (l) {
                n.x = l.x; n.y = l.y;
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
            .filter(n => (n.x || 0) > -9000)
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
                selection.update(s => ({ ...s, hoveredNodeId: d.id }));
            })
            .on("mouseleave", () => {
                selection.update(s => ({ ...s, hoveredNodeId: null }));
            });

        const activeNodeId = $selection.activeNodeId;
        const hoveredNodeId = $selection.hoveredNodeId;
        const focusIds: Set<string> = ($graphData as any)?.focusIds || new Set();
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

            // Focus highlight: gold ring on priority focus leaf nodes
            if (showFocus && d._isLeaf) {
                g.select('.focus-ring').remove();
                if (focusIds.has(d.id)) {
                    g.insert('circle', ':first-child')
                        .attr('class', 'focus-ring')
                        .attr('cx', 0).attr('cy', 0).attr('r', (d._lr || 5) + 3)
                        .attr('fill', 'none').attr('stroke', '#f59e0b')
                        .attr('stroke-width', 2).attr('opacity', 0.8);
                }
            }
        });

        // Gentle dimming: non-focus leaves slightly faded
        if (showFocus) {
            nEls.style("opacity", (d: any) => {
                if (!d._isLeaf) return null;
                if (focusIds.has(d.id)) return 1;
                return 0.6;
            });
        } else {
            nEls.style("opacity", null);
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
