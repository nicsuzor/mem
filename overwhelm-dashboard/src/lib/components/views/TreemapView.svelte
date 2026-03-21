<script lang="ts">
    import * as d3 from "d3";
    import { graphData } from "../../stores/graph";
    import { filters } from "../../stores/filters";
    import { toggleSelection, selection } from "../../stores/selection";
    import { buildTreemapNode } from "../shared/NodeShapes";
    import { routeTreemapEdges } from "../shared/EdgeRenderer";
    import type { GraphEdge } from "../../data/prepareGraphData";

    let { containerGroup, width = 2000, height = 1000 } = $props<{ containerGroup: SVGGElement | null; width?: number; height?: number }>();

    let nodesLayer: SVGGElement;
    let edgesLayer: SVGGElement;

    const canvasW = 3000;
    const canvasH = $derived(canvasW * (height && width ? height / width : 0.5));

    $effect(() => {
        if (containerGroup && $graphData && nodesLayer) {
            updateLayoutAndRender();
        }
    });

    function updateLayoutAndRender() {
        const data = $graphData;
        if (!data) return;
        const nodes = data.nodes;

        const virtualRootId = "__treemap_root__";
        const nodeIdSet = new Set(nodes.map(n => n.id));
        const projectRootId = ($filters as any).projectFilter as string | undefined;

        let stratifyNodes;
        let rootId: string;

        if (projectRootId && nodeIdSet.has(projectRootId)) {
            rootId = projectRootId;
            stratifyNodes = nodes.map(n => ({
                ...n,
                parent: (n.parent && nodeIdSet.has(n.parent) && n.id !== rootId) ? n.parent : (n.id === rootId ? "" : "__ignore__")
            })).filter(n => n.parent !== "__ignore__");
        } else {
            rootId = virtualRootId;
            stratifyNodes = [
                { id: rootId, parent: "", type: "root" },
                ...nodes.map((n) => ({
                    ...n,
                    parent: (n.parent && nodeIdSet.has(n.parent)) ? n.parent : rootId,
                })),
            ];
        }

        let root;
        try {
            // Pre-stratification Rollup Strategy:
            // For any parent node, if it has more than MAX_NODES_PER_PARENT children,
            // we group the lowest priority ones into a synthetic overflow node.
            const MAX_NODES_PER_PARENT = 30;
            
            const parentMap = new Map<string, any[]>();
            stratifyNodes.forEach(n => {
                if (!parentMap.has(n.parent)) parentMap.set(n.parent, []);
                parentMap.get(n.parent)!.push(n);
            });

            const rolledUpNodes: any[] = [];
            const syntheticNodes: any[] = [];
            
            // To safely prune, if we prune a node, we must also discard its descendants.
            // We'll track discarded IDs to filter them out later.
            const discardedIds = new Set<string>();
            
            parentMap.forEach((children, parentId) => {
                if (children.length > MAX_NODES_PER_PARENT) {
                    children.sort((a, b) => {
                        const pa = a.priority ?? 5;
                        const pb = b.priority ?? 5;
                        if (pa !== pb) return pa - pb;
                        return (b.value || 0) - (a.value || 0);
                    });

                    const keep = children.slice(0, MAX_NODES_PER_PARENT - 1);
                    const rollup = children.slice(MAX_NODES_PER_PARENT - 1);
                    
                    rolledUpNodes.push(...keep);
                    
                    if (rollup.length > 0) {
                        const synthId = `__rollup_${parentId}__`;
                        syntheticNodes.push({
                            id: synthId,
                            parent: parentId,
                            label: `+ ${rollup.length} more tasks...`,
                            status: 'active',
                            priority: 4,
                            type: 'synthetic',
                            value: rollup.reduce((sum, n) => sum + (n.value || 1), 0),
                            _isOverflow: true
                        });
                        
                        rollup.forEach(r => discardedIds.add(r.id));
                    }
                } else {
                    rolledUpNodes.push(...children);
                }
            });

            const rootItem = stratifyNodes.find(n => !n.parent);
            if (rootItem && !rolledUpNodes.find(n => n.id === rootItem.id)) {
                rolledUpNodes.push(rootItem);
            }

            // We must now recursively remove any node whose ancestor was discarded
            let filteredNodes = [...rolledUpNodes, ...syntheticNodes];
            let changed = true;
            while (changed) {
                changed = false;
                const newFiltered = [];
                for (const n of filteredNodes) {
                    if (discardedIds.has(n.parent)) {
                        discardedIds.add(n.id);
                        changed = true;
                    } else {
                        newFiltered.push(n);
                    }
                }
                filteredNodes = newFiltered;
            }

            root = d3
                .stratify<any>()
                .id((d) => d.id)
                .parentId((d) => d.parent)(filteredNodes);
        } catch (e) {
            console.error("Stratify failed for Treemap", e);
            return;
        }

        const MIN_NODE_WEIGHT = 1;
        const TREEMAP_PADDING_INNER = 1;
        const TREEMAP_PADDING_OUTER = 2;
        const TREEMAP_PADDING_TOP = 14;

        root.sum(d => {
            if (d.children?.length) return 0;
            return Math.max(MIN_NODE_WEIGHT, d.dw || MIN_NODE_WEIGHT);
        }).sort((a, b) => (b.value || 0) - (a.value || 0));

        const treemap = d3.treemap<any>()
            .size([canvasW, canvasH])
            .paddingInner(TREEMAP_PADDING_INNER)
            .paddingOuter(TREEMAP_PADDING_OUTER)
            .paddingTop(TREEMAP_PADDING_TOP)
            .tile(d3.treemapSquarify.ratio(1.618))
            .round(true);

        treemap(root);

        const layoutMap = new Map();
        root.descendants().forEach((d: any) => {
            if (d.data.id === rootId) return;
            layoutMap.set(d.data.id, {
                x: d.x0 + (d.x1 - d.x0) / 2,
                y: d.y0 + (d.y1 - d.y0) / 2,
                w: d.x1 - d.x0,
                h: d.y1 - d.y0,
                depth: d.depth,
                isLeaf: !d.children || d.children.length === 0,
            });
        });

        nodes.forEach((n) => {
            const l = layoutMap.get(n.id);
            if (l) {
                n.x = l.x; n.y = l.y; n.w = l.w; n.h = l.h;
                n.depth = l.depth;
                n._isLeaf = l.isLeaf;
            } else {
                n.x = -9999; n.y = -9999;
            }
        });

        const visibleNodes = [...nodes]
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
            const needsHighlight = isSelected || isHovered;
            // Only rebuild DOM when selection state actually changes
            const lastState = (d as any)._lastHighlight;
            if (g.selectAll("*").empty() || lastState !== needsHighlight) {
                g.selectAll("*").remove();
                buildTreemapNode(g, d, needsHighlight);
                (d as any)._lastHighlight = needsHighlight;
            }
        });

        if (!$filters.showDependencies) {
            d3.select(edgesLayer).selectAll("path").remove();
        } else {
            const eEls = d3
                .select(edgesLayer)
                .selectAll("path")
                .data(data.links)
                .join("path");
            routeTreemapEdges(eEls);
        }
    }
</script>

{#if containerGroup}
    <g bind:this={edgesLayer}></g>
    <g bind:this={nodesLayer}></g>
{/if}
