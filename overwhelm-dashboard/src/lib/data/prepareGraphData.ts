import {
    STATUS_FILLS,
    TYPE_BASE_SCALE,
    STATUS_TEXT,
    MUTED_FILL,
    MUTED_TEXT,
    INCOMPLETE_STATUSES,
    COMPLETED_STATUSES,
    TYPE_SHAPE,
    TYPE_BADGE,
} from './constants';
import { projectBorderColor } from './projectUtils';
import { getEdgeTypeDef } from './taxonomy';

export interface GraphNode {
    id: string;
    label: string;
    lines: string[];
    type: string;
    shape: string;
    status: string;
    priority: number;
    depth: number;
    maxDepth: number;
    w: number;
    h: number;
    fontSize: number;
    fill: string;
    textColor: string;
    borderColor: string;
    borderWidth: number;
    stakeholder: boolean;
    structural: boolean;
    dw: number;
    scope: number;
    uncertainty: number;
    criticality: number;
    totalLeafCount: number;
    modified: number | null;
    badge: string;
    parent: string | null;
    project: string | null;
    assignee: string | null;
    path: string | null;
    opacity: number;
    isLeaf: boolean;
    spotlight: boolean;
    x?: number;
    y?: number;
    layouts: Record<string, any>;
    fullTitle: string;
    focusScore: number;
    _raw: any;

    // D3 physics mutation state
    fx?: number | null;
    fy?: number | null;
    vx?: number;
    vy?: number;

    // Layout-computed properties (mutated during rendering)
    _lr?: number;        // circle pack layout radius
    _lw?: number;        // treemap layout width
    _lh?: number;        // treemap layout height
    _isLeaf?: boolean;   // layout leaf state
    _isOverflow?: boolean; // layout overflow state
    _lastSelected?: boolean; // previous selection state for dirty-check optimisation
}

export interface GraphEdge {
    source: string | GraphNode;
    target: string | GraphNode;
    type: string;
    color: string;
    width: number;
    dash: string;
}

export interface PreparedGraph {
    nodes: GraphNode[];
    links: GraphEdge[];
    hasLayout: boolean;
    availableLayouts: string[];
    readyIds: Set<string>;
    blockedIds: Set<string>;
    focusIds: Set<string>;
    allProjects: string[];
}

function estimateTextWidth(text: string, fontSize: number): number {
    return text.length * fontSize * 0.56;
}

function wrapText(label: string, fontSize: number, maxWidth: number): string[] {
    if (estimateTextWidth(label, fontSize) <= maxWidth) {
        return [label];
    }
    const charsPerLine = Math.max(10, Math.floor(maxWidth / (fontSize * 0.56)));
    const lines: string[] = [];
    let current = "";

    for (const word of label.split(/\s+/)) {
        const test = current ? `${current} ${word}` : word;
        if (test.length > charsPerLine && current) {
            lines.push(current);
            current = word;
        } else {
            current = test;
        }
    }
    if (current) {
        lines.push(current);
    }
    return lines.slice(0, 3);
}

function hexToRgb(hex: string): [number, number, number] {
    const h = hex.replace(/^#/, '');
    if (h.length === 3) {
        return [
            parseInt(h[0] + h[0], 16),
            parseInt(h[1] + h[1], 16),
            parseInt(h[2] + h[2], 16)
        ];
    }
    return [
        parseInt(h.substring(0, 2), 16),
        parseInt(h.substring(2, 4), 16),
        parseInt(h.substring(4, 6), 16)
    ];
}

function rgbToHex(r: number, g: number, b: number): string {
    return '#' + [r, g, b].map(x => {
        const hex = x.toString(16);
        return hex.length === 1 ? '0' + hex : hex;
    }).join('');
}

function interpolateColor(colorA: string, colorB: string, t: number): string {
    t = Math.max(0, Math.min(1, t));
    const [ra, ga, ba] = hexToRgb(colorA);
    const [rb, gb, bb] = hexToRgb(colorB);

    const r = Math.round(ra + (rb - ra) * t);
    const g = Math.round(ga + (gb - ga) * t);
    const b = Math.round(ba + (bb - ba) * t);

    return rgbToHex(r, g, b);
}

function classifyEdge(sourceId: string, targetId: string, nodeById: Map<string, any>): string {
    const source = nodeById.get(sourceId) || {};
    if (source.parent === targetId) return 'parent';
    if (source.depends_on?.includes(targetId)) return 'depends_on';
    if (source.soft_depends_on?.includes(targetId)) return 'soft_depends_on';
    if (Array.isArray(source.contributes_to) && source.contributes_to.some((c: any) =>
        (typeof c === 'string' ? c : c?.to) === targetId)) return 'contributes_to';
    throw new Error(
        `classifyEdge: cannot classify edge ${sourceId} → ${targetId}; ` +
        `no parent / depends_on / soft_depends_on / contributes_to relation found ` +
        `on the source node. The MCP server should always supply edge.type — ` +
        `if this fired, the upstream graph_json payload is missing a type field.`
    );
}

/**
 * Style a graph edge by its semantic type. Returns d3 stroke attributes plus a
 * canonicalised type label used by the views and the filter store.
 *
 * Canonical types after this function:
 *   parent | depends_on | soft_depends_on | contributes_to | similar_to | ref
 *
 * `link` and `wikilink` are folded into `ref` (the existing convention used
 * across the dashboard for "wikilink-style references").
 */
export function styleEdge(rawType: string, ctx?: {
    targetFocusScore?: number;
    maxFocusScore?: number;
}): { type: string; color: string; width: number; dash: string } {
    let t = (rawType || '').toLowerCase();
    if (t === 'link' || t === 'wikilink' || t === 'supersedes') {
        t = 'ref';
    }

    const def = getEdgeTypeDef(t, false);

    let width = 2.0;
    if (t === 'parent') width = 3.5;
    else if (t === 'depends_on') {
        width = 3.5;
        const tw = ctx?.targetFocusScore ?? 0;
        const mw = ctx?.maxFocusScore ?? 0;
        if (tw > 0 && mw > 0) {
            const critRatio = Math.min(Math.log1p(tw) / Math.log1p(mw), 1.0);
            if (critRatio > 0.5) width = 3.0 + critRatio * 2.0;
        }
    } else if (t === 'contributes_to') width = 2.5;
    else if (t === 'similar_to') width = 1.0;
    else if (t === 'ref') width = 1.5;

    return { type: t, color: def.color, width, dash: def.dashStyle === 'solid' ? '' : def.dashStyle };
}

/** Filter store key for an edge type. Returns null if no filter applies. */
export function edgeFilterKey(type: string): string | null {
    let t = (type || '').toLowerCase();
    if (t === 'link' || t === 'wikilink' || t === 'supersedes') {
        t = 'ref';
    }
    const def = getEdgeTypeDef(t, false);
    return def.filterKey || null;
}

export function prepareGraphData(
    graph: { nodes?: any[]; edges?: any[]; ready?: string[]; blocked?: string[]; focus?: string[] },
    structuralIds: Set<string> = new Set(),
    options: {
        hiddenProjects?: string[];
    } = {}
): PreparedGraph {
    let rawNodes = (graph.nodes || []).map(n => ({ ...n }));
    let rawEdges = (graph.edges || []).map(e => ({ ...e }));

    const allProjects = Array.from(new Set(rawNodes.map(n => n.project).filter((p): p is string => !!p))).sort();

    // Apply project filters at the source
    if (options.hiddenProjects && options.hiddenProjects.length > 0) {
        rawNodes = rawNodes.filter(n => !options.hiddenProjects!.includes(n.project || ""));
    }

    // Drop 'project' type nodes from the graph. Children get re-parented to the
    // project's parent (walking the chain so project-under-project collapses cleanly).
    const projectTypeIds = new Set(
        rawNodes.filter(n => (n.node_type || '').toLowerCase() === 'project').map(n => n.id)
    );
    if (projectTypeIds.size > 0) {
        const parentOf = new Map<string, string | null>();
        rawNodes.forEach(n => parentOf.set(n.id, n.parent || null));

        const resolveAncestor = (parentId: string | null): string | null => {
            const seen = new Set<string>();
            let cur = parentId;
            while (cur && projectTypeIds.has(cur)) {
                if (seen.has(cur)) return null;
                seen.add(cur);
                cur = parentOf.get(cur) ?? null;
            }
            return cur;
        };

        rawNodes.forEach(n => {
            if (n.parent && projectTypeIds.has(n.parent)) {
                n.parent = resolveAncestor(n.parent);
            }
        });
        rawNodes = rawNodes.filter(n => !projectTypeIds.has(n.id));
    }

    // Prune edges referencing filtered-out nodes
    const filteredNodeIds = new Set(rawNodes.map(n => n.id));
    rawEdges = rawEdges.filter(e => filteredNodeIds.has(e.source) && filteredNodeIds.has(e.target));

    const initialNodeById = new Map<string, any>(rawNodes.map(n => [n.id, n]));
    const initialNodeIds = new Set(rawNodes.map(n => n.id));

    const initialChildrenMap = new Map<string, string[]>();
    rawNodes.forEach(n => {
        if (n.parent && initialNodeIds.has(n.parent)) {
            const kids = initialChildrenMap.get(n.parent) || [];
            kids.push(n.id);
            initialChildrenMap.set(n.parent, kids);
        }
    });

    const CONTAINER_TYPES = new Set(['goal', 'project', 'epic', 'task']);
    const collapseMap = new Map<string, string>();

    let changed = true;
    while (changed) {
        changed = false;
        for (const n of rawNodes) {
            if (CONTAINER_TYPES.has((n.node_type || '').toLowerCase())) {
                const kids = initialChildrenMap.get(n.id) || [];
                if (kids.length === 1) {
                    const childId = kids[0];
                    if (!collapseMap.has(n.id)) {
                        collapseMap.set(n.id, childId);
                        changed = true;
                    }
                }
            }
        }
        for (const [k, v] of collapseMap.entries()) {
            if (collapseMap.has(v)) {
                collapseMap.set(k, collapseMap.get(v)!);
                changed = true;
            }
        }
    }

    rawNodes = rawNodes.filter(n => !collapseMap.has(n.id));
    rawNodes.forEach(n => {
        let curParent = n.parent;
        const seen = new Set<string>();
        while (curParent && collapseMap.has(curParent)) {
            if (seen.has(curParent)) break;
            seen.add(curParent);
            const pNode = initialNodeById.get(curParent);
            curParent = pNode ? pNode.parent : null;
        }
        n.parent = curParent;
    });

    rawEdges = rawEdges.map(e => ({
        ...e,
        source: collapseMap.get(e.source) || e.source,
        target: collapseMap.get(e.target) || e.target
    })).filter(e => e.source !== e.target);

    const uniqueEdges = new Map<string, any>();
    rawEdges.forEach(e => {
        const key = `${e.source}-${e.target}-${e.type || ''}`;
        uniqueEdges.set(key, e);
    });
    rawEdges = Array.from(uniqueEdges.values());

    const nodeById = new Map<string, any>(rawNodes.map(n => [n.id, n]));
    const nodeIds = new Set(rawNodes.map(n => n.id));

    // Sanitize orphan parents: if a node's parent isn't in the graph, clear it
    rawNodes.forEach(n => {
        if (n.parent && !nodeIds.has(n.parent)) {
            n.parent = null;
        }
    });

    const parentIdsInGraph = new Set<string>();
    rawNodes.forEach(n => {
        if (n.parent) parentIdsInGraph.add(n.parent);
    });

    // Compute total leaf descendant counts from parent hierarchy
    const childrenMap = new Map<string, string[]>();
    rawNodes.forEach(n => {
        if (n.parent && nodeIds.has(n.parent)) {
            const kids = childrenMap.get(n.parent) || [];
            kids.push(n.id);
            childrenMap.set(n.parent, kids);
        }
    });

    const totalLeafCountCache = new Map<string, number>();
    const countingSeen = new Set<string>();
    function countLeaves(id: string): number {
        if (totalLeafCountCache.has(id)) return totalLeafCountCache.get(id)!;
        if (countingSeen.has(id)) return 0; // cycle detection
        countingSeen.add(id);

        const kids = childrenMap.get(id);
        if (!kids || kids.length === 0) {
            totalLeafCountCache.set(id, 1); // leaf counts as 1
            return 1;
        }
        let total = 0;
        for (const kid of kids) total += countLeaves(kid);
        totalLeafCountCache.set(id, total);
        return total;
    }
    rawNodes.forEach(n => countLeaves(n.id));

    // Intent/focus nodes get promoted to priority above P0
    const validEdges = rawEdges.filter(e => nodeIds.has(e.source) && nodeIds.has(e.target));
    const maxDepth = Math.max(0, ...rawNodes.map(n => n.depth || 0));

    const focusScores = rawNodes.map(n => n.focus_score || 0);
    let maxFocusScore = Math.max(1, ...focusScores);
    if (maxFocusScore === 0) maxFocusScore = 1;

    const targetFocusScore = new Map<string, number>(
        rawNodes.map(n => [n.id, n.focus_score || 0])
    );

    const d3Nodes: GraphNode[] = [];
    for (const node of rawNodes) {
        const nid = node.id;
        // Canonicalise: nodes with no explicit node_type frontmatter
        // (typical for archived markdown drops) become "note", which the
        // taxonomy already defines. Same pattern styleEdge uses to fold
        // link/wikilink/supersedes into "ref".
        const nodeType = (node.node_type || "note").toLowerCase();
        const status = (node.status || "inbox").toLowerCase();

        const priority = typeof node.priority === 'number' ? node.priority : 2;
        const dw = node.downstream_weight || 0;
        const focusScore = node.focus_score || 0;
        const scope = typeof node.scope === 'number' ? node.scope : 0;
        const uncertainty = typeof node.uncertainty === 'number' ? node.uncertainty : 0;
        const criticality = typeof node.criticality === 'number' ? node.criticality : 0;
        const stakeholder = node.stakeholder_exposure || false;
        const depth = node.depth || 0;
        const isStructural = structuralIds.has(nid);

        let label = node.title || node.label || nid;
        // Strip redundant type prefixes (e.g. "Epic: ...", "Project: ...")
        label = label.replace(/^(Epic|Project|Task|Goal|Note|Memory):\s*/i, '');
        const fullTitle = label;

        // Skip file-system recency extraction as we are in browser JS.
        // Must rely on node.modified if passed in.
        const modified = node.modified || null;

        let typeScale = TYPE_BASE_SCALE[nodeType] ?? 1.0;
        if (COMPLETED_STATUSES.has(status)) {
            typeScale *= 0.6;
        }
        // Focus score ranges from 0 up to 10000+. We use log1p to compress the scale so it doesn't blow up nodes.
        // A focus score of 10000 -> log1p(10000) ~= 9.2. Scaling by 0.15 makes the max boost around 2.38x.
        const weightFactor = focusScore > 0 ? 1 + Math.log1p(focusScore) * 0.15 : 1.0;
        const scale = typeScale * weightFactor;

        const baseFont = 10;
        const fontSize = Math.max(8, Math.min(16, Math.round(baseFont * scale)));

        const maxTextW = 160 * scale;
        const lines = wrapText(label, fontSize, maxTextW);
        const lineWidths = lines.map(line => estimateTextWidth(line, fontSize));
        const textW = lineWidths.length > 0 ? Math.max(...lineWidths) : 40;

        const padX = 16 * typeScale;
        const padY = 10 * typeScale;
        const nodeW = Math.max(textW + padX * 2, 55 * typeScale);
        const nodeH = Math.max(lines.length * (fontSize + 4) + padY * 2, 30 * typeScale);

        let fill: string;
        let textCol: string;

        if (isStructural) {
            fill = "#e2e8f0";
            textCol = "#94a3b8";
        } else {
            const weightNorm = Math.min(Math.log1p(focusScore) / Math.log1p(maxFocusScore), 1.0);
            const baseFill = STATUS_FILLS[status];
            if (!baseFill) {
                if (CONTAINER_TYPES.has(nodeType)) {
                    throw new Error(
                        `prepareGraphData: unknown status "${status}" on node ${node.id} (type: ${nodeType}). ` +
                        `Known statuses: ${Object.keys(STATUS_FILLS).join(", ")}. ` +
                        `The MCP server normalises status aliases for tasks — if this fired, ` +
                        `either the canonical set drifted or the server is shipping a stale alias.`
                    );
                }
            }
            // Weight-based desaturation: aggressively dim the bottom 80% of tasks by focus
            // We use the linear focus ratio for a sharper cutoff than the log-compressed weightNorm
            const linearFocusRatio = maxFocusScore > 1 ? focusScore / maxFocusScore : 1.0;
            let desaturation = 0;
            
            if (linearFocusRatio < 0.20) {
                // Bottom ~80% of tasks get heavily desaturated (up to 85% grey for zero focus)
                desaturation = 0.85 - (linearFocusRatio / 0.20) * 0.45; 
            } else {
                // Top ~20% of tasks get full color or mild desaturation
                desaturation = Math.max(0, 0.4 - weightNorm * 0.4);
            }

            // Recency emphasis: stale nodes desaturate further
            if (modified) {
                const daysSinceModified = (Date.now() - modified) / 86400000;
                if (daysSinceModified > 30) {
                    desaturation = Math.min(1.0, desaturation + 0.5);
                } else if (daysSinceModified > 14) {
                    desaturation = Math.min(1.0, desaturation + 0.3);
                } else if (daysSinceModified > 7) {
                    desaturation = Math.min(1.0, desaturation + 0.1);
                }
            }
            fill = interpolateColor(baseFill || MUTED_FILL, MUTED_FILL, desaturation);
            const baseText = STATUS_TEXT[status];
            if (!baseText && CONTAINER_TYPES.has(nodeType)) {
                throw new Error(
                    `prepareGraphData: STATUS_TEXT missing entry for status "${status}". ` +
                    `STATUS_FILLS and STATUS_TEXT must be kept in sync.`
                );
            }
            textCol = interpolateColor(baseText || MUTED_TEXT, MUTED_TEXT, desaturation);
        }

        // Criticality: blend fill toward amber to signal high-impact nodes
        if (!isStructural && criticality > 0) {
            fill = interpolateColor(fill, '#f59e0b', criticality * 0.30);
            textCol = interpolateColor(textCol, '#92400e', criticality * 0.25);
        }

        let opacity = 1.0;
        if (!isStructural && dw === 0) {
            const hasEdges = validEdges.some(e => e.source === nid || e.target === nid);
            if (!hasEdges) opacity = 0.5;
        }
        
        // Dim low-focus nodes globally
        if (!isStructural) {
            const linearFocusRatio = maxFocusScore > 1 ? focusScore / maxFocusScore : 1.0;
            if (linearFocusRatio < 0.20) {
                // Dim up to 40% opacity for zero focus, smoothly scaling up to full opacity
                opacity = Math.min(opacity, 0.4 + (linearFocusRatio / 0.20) * 0.6);
            }
        }

        // Uncertainty: dim nodes proportionally to how uncertain they are
        if (!isStructural && uncertainty > 0) {
            opacity = Math.max(0.3, opacity - uncertainty * 0.35);
        }

        const isIncomplete = INCOMPLETE_STATUSES.has(status);
        const project = node.project || '';
        const borderColor = isStructural ? '#475569' : projectBorderColor(project);

        let borderWidth = 1.5 + Math.min(Math.log1p(dw) * 0.5, 2.5);
        if (priority <= 1 && isIncomplete) {
            borderWidth = Math.max(borderWidth, 3);
        }
        // Criticality: widen border to draw visual attention
        if (!isStructural && criticality > 0) {
            borderWidth = Math.min(borderWidth + criticality * 2.5, 6);
        }

        const shape = TYPE_SHAPE[nodeType];
        if (shape === undefined) {
            throw new Error(
                `prepareGraphData: unknown node type "${nodeType}" on node ${nid} ` +
                `(no TYPE_SHAPE entry). Known: ${Object.keys(TYPE_SHAPE).join(", ")}.`
            );
        }
        const badge = TYPE_BADGE[nodeType];
        if (badge === undefined) {
            throw new Error(
                `prepareGraphData: unknown node type "${nodeType}" on node ${nid} ` +
                `(no TYPE_BADGE entry). Known: ${Object.keys(TYPE_BADGE).join(", ")}.`
            );
        }

        d3Nodes.push({
            id: nid,
            label,
            fullTitle,
            lines,
            type: nodeType,
            shape,
            status,
            priority,
            depth,
            maxDepth,
            w: Math.round(nodeW * 10) / 10,
            h: Math.round(nodeH * 10) / 10,
            fontSize,
            fill,
            textColor: textCol,
            borderColor,
            borderWidth: Math.round(borderWidth * 10) / 10,
            stakeholder,
            structural: isStructural,
            dw: Math.round(dw * 10) / 10,
            scope,
            uncertainty: Math.round(uncertainty * 100) / 100,
            criticality: Math.round(criticality * 100) / 100,
            totalLeafCount: parentIdsInGraph.has(nid) ? (totalLeafCountCache.get(nid) || 0) : 0,
            modified,
            badge,
            parent: node.parent || null,
            project: node.project || null,
            assignee: node.assignee || null,
            path: node.path || null,
            opacity,
            isLeaf: !parentIdsInGraph.has(nid),
            spotlight: Boolean(node.spotlight),
            x: node.x,
            y: node.y,
            layouts: node.layouts || {},
            focusScore: node.focus_score || 0,
            _raw: node
        });
    }

    const d3Links: GraphEdge[] = [];
    for (const edge of validEdges) {
        const rawType = edge.type || classifyEdge(edge.source, edge.target, nodeById);
        const styled = styleEdge(rawType, {
            targetFocusScore: targetFocusScore.get(edge.target) || 0,
            maxFocusScore,
        });

        // Flip parent direction so arrows point parent -> child
        let linkSource = edge.source;
        let linkTarget = edge.target;
        if (styled.type === 'parent') {
            linkSource = edge.target;
            linkTarget = edge.source;
        }

        d3Links.push({
            source: linkSource,
            target: linkTarget,
            type: styled.type,
            color: styled.color,
            width: Math.round(styled.width * 10) / 10,
            dash: styled.dash,
        });
    }

    const hasLayout = rawNodes.some(n =>
        n.x !== undefined ||
        n.layouts?.sfdp
    );

    const availableLayouts = new Set<string>();
    rawNodes.forEach(n => {
        if (n.layouts) {
            Object.keys(n.layouts).forEach(k => availableLayouts.add(k));
        }
    });

    return {
        nodes: d3Nodes,
        links: d3Links,
        hasLayout,
        availableLayouts: Array.from(availableLayouts).sort(),
        readyIds: new Set(graph.ready || []),
        blockedIds: new Set(graph.blocked || []),
        focusIds: new Set(graph.focus || []),
        allProjects,
    };
}
