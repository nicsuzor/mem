<script lang="ts">
  import * as d3 from 'd3';
  import { graphData } from '../../stores/graph';
  import { toggleSelection, selection } from '../../stores/selection';
  import { viewSettings } from '../../stores/viewSettings';
  import { buildArcNode } from '../shared/NodeShapes';
  import { routeArcEdges } from '../shared/EdgeRenderer';
  import type { GraphNode, GraphEdge } from '../../data/prepareGraphData';

  export let containerGroup: SVGGElement;

  let nodesLayer: SVGGElement;
  let edgesLayer: SVGGElement;

  let layoutComputed = false;
  let lastGraphData: any = null;
  let lastArcScale: number | undefined = undefined;
  let lastArcFocused: boolean | undefined = undefined;
  let arcNodeIds = new Set<string>();

  $: {
    if (containerGroup && $graphData && nodesLayer && edgesLayer && $selection && $viewSettings) {
      const dataChanged = $graphData !== lastGraphData;
      const settingsChanged = $viewSettings.arcVerticalSpacing !== lastArcScale
          || $viewSettings.arcFocusedOnly !== lastArcFocused;

      if (dataChanged || settingsChanged) {
        computeArcLayout();
        lastGraphData = $graphData;
        lastArcScale = $viewSettings.arcVerticalSpacing;
        lastArcFocused = $viewSettings.arcFocusedOnly;
        layoutComputed = true;
      }
      if (layoutComputed) {
        renderArcNodes();
      }
    }
  }

  function computeArcLayout() {
    if (!$graphData) return;

    const allNodes = $graphData.nodes;
    const data = $graphData as any;
    const focusIds: Set<string> = data.focusIds || new Set();

    let nodes: GraphNode[];

    if ($viewSettings.arcFocusedOnly) {
        // Filter to server-computed focus set (priority + deadline + staleness + dw)
        const focused = allNodes.filter(n => focusIds.has(n.id));

        // Include ancestor chains for context (with cycle detection)
        const focusedIds = new Set(focused.map(n => n.id));
        const nodeMap = new Map(allNodes.map(n => [n.id, n]));
        const withAncestors = [...focused];
        focused.forEach(n => {
            let parentId = n.parent;
            const visited = new Set<string>();
            while (parentId && !visited.has(parentId)) {
                visited.add(parentId);
                if (!focusedIds.has(parentId)) {
                    const parent = nodeMap.get(parentId);
                    if (parent) { withAncestors.push(parent); focusedIds.add(parentId); }
                }
                const pNode = nodeMap.get(parentId);
                parentId = pNode?.parent || null;
            }
        });

        nodes = withAncestors;
    } else {
        nodes = [...allNodes];
    }

    // Sort by focus score, then depth
    nodes.sort((a, b) => {
        if ((a.depth || 0) !== (b.depth || 0)) return (a.depth || 0) - (b.depth || 0);
        return (b.focusScore || 0) - (a.focusScore || 0);
    });

    // Layout: depth maps to Y bands, x to sorted index within depth
    const maxDepth = nodes.reduce((max, n) => Math.max(max, n.depth || 0), 1);
    const hBand = (1200 / (maxDepth + 1)) * ($viewSettings.arcVerticalSpacing || 1.0);

    const nodesByDepth = new Map<number, GraphNode[]>();
    nodes.forEach(n => {
        const d = n.depth || 0;
        if (!nodesByDepth.has(d)) nodesByDepth.set(d, []);
        nodesByDepth.get(d)!.push(n);
    });

    nodesByDepth.forEach((depthNodes, depth) => {
        const count = depthNodes.length;
        const xStep = 1200 / (count + 1);
        depthNodes.forEach((n, i) => {
            n.x = xStep * (i + 1) + 100;
            n.y = (depth * hBand) + 100 + (Math.random() * 10 - 5);
        });
    });

    // Store focused node set for edge filtering
    arcNodeIds = new Set(nodes.map(n => n.id));
  }

  function renderArcNodes() {
    const data = $graphData;
    if (!data) return;

    // Use only the nodes that survived focus filtering
    const nodes = data.nodes.filter(n => arcNodeIds.has(n.id));
    const links = data.links.filter(l => {
        const sid = typeof l.source === 'object' ? l.source.id : l.source;
        const tid = typeof l.target === 'object' ? l.target.id : l.target;
        return arcNodeIds.has(sid) && arcNodeIds.has(tid);
    });

    const eEls = d3.select(edgesLayer).selectAll<SVGPathElement, GraphEdge>("path")
      .data(links)
      .join("path")
      .attr("fill", "none")
      .attr("stroke", d => d.color)
      .attr("stroke-width", d => d.width)
      .attr("stroke-dasharray", d => d.dash || null)
      .attr("marker-end", "url(#ar)");

    routeArcEdges(eEls);

    const nEls = d3.select(nodesLayer).selectAll<SVGGElement, GraphNode>("g.node")
      .data(nodes, d => d.id)
      .join("g")
      .attr("class", "node")
      .attr("transform", d => `translate(${d.x},${d.y})`)
      .style("cursor", "pointer")
      .on("click", (e, d) => { e.stopPropagation(); toggleSelection(d.id); })
      .on("mouseenter", (e, d) => {
        selection.update(s => ({ ...s, hoveredNodeId: d.id }));
      })
      .on("mouseleave", () => {
        selection.update(s => ({ ...s, hoveredNodeId: null }));
      });

    const activeNodeId = $selection.activeNodeId;

    nEls.each(function(d) {
      const g = d3.select(this);
      const isSelected = d.id === activeNodeId;

      const lastSelected = (d as any)._lastSelected;
      if (g.selectAll("*").empty() || lastSelected !== isSelected) {
          g.selectAll("*").remove();
          buildArcNode(g as any, d, isSelected);
          (d as any)._lastSelected = isSelected;
      }
    });
  }
</script>

{#if containerGroup}
  <g bind:this={edgesLayer}></g>
  <g bind:this={nodesLayer}></g>
  {#if $viewSettings.arcFocusedOnly && $graphData}
    <text x="110" y="60" font-size="12" fill="#94a3b8" font-family="monospace" opacity="0.7">
      Focused: {arcNodeIds.size} of {$graphData.nodes.length} tasks
    </text>
  {/if}
{/if}
