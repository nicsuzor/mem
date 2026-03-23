<script lang="ts">
    import * as d3 from "d3";
    import { graphData } from "../../stores/graph";
    import { filters } from "../../stores/filters";
    import { toggleSelection, selection } from "../../stores/selection";
    import { buildTreemapNode } from "../shared/NodeShapes";
    import { routeTreemapEdges } from "../shared/EdgeRenderer";
    import { viewSettings } from "../../stores/viewSettings";
    import type { GraphEdge } from "../../data/prepareGraphData";

    let { containerGroup, width = 2000, height = 1000 } = $props<{ containerGroup: SVGGElement | null; width?: number; height?: number }>();

    let nodesLayer: SVGGElement;
    let edgesLayer: SVGGElement;

    const canvasW = 3000;
    const canvasH = $derived(canvasW * (height && width ? height / width : 0.5));

    $effect(() => {
        const _weightMode = $viewSettings.treemapWeightMode;
        if (containerGroup && $graphData && nodesLayer) {
            updateLayoutAndRender();
        }
    });

    // Types that should be preserved as grouping containers (never collapsed)
    const CONTAINER_TYPES = new Set(['epic', 'goal', 'project', 'project_container']);

    function collapseSingleChildParents(nodes: any[], rootId: string): any[] {
        const DONE_STATUSES = new Set(['done', 'completed', 'cancelled']);
        let result = [...nodes];
        let changed = true;
        while (changed) {
            changed = false;
            const childrenOf = new Map<string, any[]>();
            for (const n of result) {
                if (n.parent) {
                    if (!childrenOf.has(n.parent)) childrenOf.set(n.parent, []);
                    childrenOf.get(n.parent)!.push(n);
                }
            }

            for (const [parentId, children] of childrenOf) {
                if (parentId === '' || parentId === rootId) continue;
                const parent = result.find(n => n.id === parentId);
                if (!parent) continue;

                // Never collapse epics/goals/projects — they form the visual hierarchy
                if (CONTAINER_TYPES.has(parent.type)) continue;

                const activeChildren = children.filter(c => !DONE_STATUSES.has(c.status));
                if (activeChildren.length !== 1) continue;

                const child = activeChildren[0];
                // Concatenate labels, cap at 2 segments
                if (child.label && parent.label && !child.label.includes('›')) {
                    child.label = parent.label + ' › ' + child.label;
                }
                // Inherit project color if child lacks one
                if (!child.project && parent.project) child.project = parent.project;
                // Reparent child to grandparent
                child.parent = parent.parent;

                // Remove parent + completed siblings (visual noise)
                const removeIds = new Set([parentId, ...children.filter(c => c.id !== child.id).map(c => c.id)]);
                result = result.filter(n => !removeIds.has(n.id));
                changed = true;
                break; // restart after mutation
            }
        }
        return result;
    }

    function updateLayoutAndRender() {
        const data = $graphData;
        if (!data) return;
        const nodes = data.nodes;

        const virtualRootId = "__treemap_root__";
        const nodeIdSet = new Set(nodes.map(n => n.id));
        const projectRootId = ($filters as any).projectFilter as string | undefined;

        let stratifyNodes: any[];
        let rootId: string;

        if (projectRootId && nodeIdSet.has(projectRootId)) {
            rootId = projectRootId;
            stratifyNodes = nodes.map(n => ({
                ...n,
                parent: (n.parent && nodeIdSet.has(n.parent) && n.id !== rootId) ? n.parent : (n.id === rootId ? "" : "__ignore__")
            })).filter(n => n.parent !== "__ignore__");
        } else {
            rootId = virtualRootId;

            // Build project-grouped hierarchy:
            // root → project containers → (existing parent hierarchy within project)
            const projects = new Set(nodes.map(n => n.project).filter((p): p is string => Boolean(p)));
            const projectContainerIds = new Map<string, string>();

            stratifyNodes = [{ id: rootId, parent: "", type: "root" }];

            // Create synthetic project container nodes
            for (const proj of projects) {
                const containerId = `__project_${proj}__`;
                projectContainerIds.set(proj, containerId);
                stratifyNodes.push({
                    id: containerId,
                    parent: rootId,
                    label: proj,
                    type: 'project_container',
                    status: '',
                    project: proj,
                    _isProjectContainer: true,
                });
            }

            // Reparent nodes: if a node's parent is in the graph AND shares the same project,
            // keep it. Otherwise, reparent to the project container (or root for no-project).
            for (const n of nodes) {
                const projContainer = n.project ? projectContainerIds.get(n.project) : undefined;
                let effectiveParent: string;

                if (n.parent && nodeIdSet.has(n.parent)) {
                    // Check if parent is in the same project
                    const parentNode = nodes.find(p => p.id === n.parent);
                    if (parentNode && parentNode.project === n.project) {
                        effectiveParent = n.parent;
                    } else {
                        // Cross-project parent — reparent to project container
                        effectiveParent = projContainer || rootId;
                    }
                } else {
                    effectiveParent = projContainer || rootId;
                }

                stratifyNodes.push({ ...n, parent: effectiveParent });
            }

            // Also add a container for orphan nodes (no project)
            const orphans = stratifyNodes.filter(n => n.parent === rootId && n.id !== rootId && n.type !== 'project_container');
            if (orphans.length > 0) {
                const orphanId = '__project_uncategorized__';
                stratifyNodes.push({
                    id: orphanId,
                    parent: rootId,
                    label: 'uncategorized',
                    type: 'project_container',
                    status: '',
                    project: '',
                    _isProjectContainer: true,
                });
                for (const o of orphans) {
                    o.parent = orphanId;
                }
            }
        }

        // Collapse single-child intermediate parents to reduce wasted nesting
        stratifyNodes = collapseSingleChildParents(stratifyNodes, rootId);

        let root;
        let filteredNodes: any[] = [];
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
            filteredNodes = [...rolledUpNodes, ...syntheticNodes];
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

        const weightMode = $viewSettings.treemapWeightMode || 'sqrt';
        root.sum(d => {
            if (d.children?.length) return 0;
            switch (weightMode) {
                case 'priority': {
                    if (d.status === 'done' || d.status === 'completed') return 1;
                    if (d.priority <= 1) return 3;
                    return 2;
                }
                case 'dw-bucket': {
                    const s = Math.sqrt(d.dw || 1);
                    if (s > 5) return 4;
                    if (s > 2) return 3;
                    if (s > 1) return 2;
                    return 1;
                }
                case 'equal':
                    return 1;
                case 'sqrt':
                default:
                    return Math.max(MIN_NODE_WEIGHT, Math.sqrt(d.dw || MIN_NODE_WEIGHT));
            }
        }).sort((a, b) => (b.value || 0) - (a.value || 0));

        // Estimate header height based on label length and node width
        function estimateHeaderHeight(node: any): number {
            if (node.depth === 0) return 4; // virtual root
            // Project containers: space for the large label + breathing room
            if (node.data?._isProjectContainer) return 34;
            const w = (node.x1 ?? canvasW) - (node.x0 ?? 0);
            const label = node.data?.label || '';
            if (!label || w < 20) return node.depth <= 1 ? 38 : 20;
            const fontSize = node.depth <= 1 ? 11 : 9;
            const charWidth = fontSize * 0.56;
            const availableWidth = Math.max(20, w - 12); // pad
            const charsPerLine = Math.max(4, Math.floor(availableWidth / charWidth));
            const lines = Math.min(3, Math.ceil(label.length / charsPerLine));
            const lineHeight = fontSize * 1.3;
            const basePad = node.depth <= 1 ? 10 : 6;
            return Math.max(node.depth <= 1 ? 24 : 16, Math.min(60, lines * lineHeight + basePad));
        }

        // 3-tier spacing: projects (very generous) → epics (moderate) → task cards (breathing room)
        const treemap = d3.treemap<any>()
            .size([canvasW, canvasH])
            .paddingInner((node: any) => node.depth <= 1 ? 14 : node.depth <= 2 ? 6 : 5)
            .paddingBottom((node: any) => node.depth <= 1 ? 10 : node.depth <= 2 ? 5 : 4)
            .paddingLeft((node: any) => node.depth <= 1 ? 10 : node.depth <= 2 ? 5 : 4)
            .paddingRight((node: any) => node.depth <= 1 ? 10 : node.depth <= 2 ? 5 : 4)
            .paddingTop((node: any) => estimateHeaderHeight(node))
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

        // Apply layout positions to real nodes
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

        // Build synthetic nodes (project containers, overflow) with layout data
        const syntheticVisible: any[] = [];
        for (const sn of filteredNodes) {
            if (sn._isProjectContainer || sn._isOverflow) {
                const l = layoutMap.get(sn.id);
                if (l) {
                    syntheticVisible.push({
                        ...sn,
                        x: l.x, y: l.y, w: l.w, h: l.h,
                        depth: l.depth, _isLeaf: l.isLeaf, isLeaf: l.isLeaf,
                    });
                }
            }
        }

        const visibleNodes = [...syntheticVisible, ...nodes]
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
