<script lang="ts">
    import * as d3 from "d3";
    import { graphData } from "../../stores/graph";
    import { toggleSelection, selection } from "../../stores/selection";
    import { viewSettings } from "../../stores/viewSettings";
    import { buildCirclePackNode } from "../shared/NodeShapes";
    import { routeContainmentEdges } from "../shared/EdgeRenderer";

    export let containerGroup: SVGGElement;

    let nodesLayer: SVGGElement;
    let edgesLayer: SVGGElement;

    let layoutComputed = false;
    let lastGraphData: any = null;

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

        const computeSum = (d: any) => {
            if (d.children?.length) return 0;
            if (["done", "completed", "cancelled"].includes(d.status)) return 0.1;
            // Increase minimum weight for active tasks
            return Math.max(2, d.dw || 1);
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

        const pack = d3.pack<any>().size([2000, 2000]).padding(10);

        pack(root);

        const nodesToRollup = new Set<string>();
        // Rollup logic removed to show all nodes again as per user request

        const layoutMap = new Map();
        root.descendants().forEach((d: any) => {
            if (d.data.id === rootId) return;
            layoutMap.set(d.data.id, {
                x: d.x - 1000, // center at 0
                y: d.y - 1000,
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
        });

        const eEls = d3
            .select(edgesLayer)
            .selectAll("path")
            .data(data.links)
            .join("path");

        routeContainmentEdges(eEls);
    }
</script>

{#if containerGroup}
    <g bind:this={edgesLayer}></g>
    <g bind:this={nodesLayer}></g>
{/if}
