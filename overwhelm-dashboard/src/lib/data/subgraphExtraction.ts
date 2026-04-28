import type { GraphNode, GraphEdge, PreparedGraph } from './prepareGraphData';
import { COMPLETED_STATUSES } from './constants';

/**
 * Subgraph extraction & layout helpers shared by experimental
 * "path-to-goal" visualisation views (Swimlanes, DSM, Ribbon,
 * HTA Tree, Wave Kanban).
 *
 * The model: given a target/goal node, the subgraph that "needs to
 * be done to reach it" is everything reachable by walking BACKWARDS
 * along edges that represent prerequisites:
 *   - depends_on (and soft_depends_on)  — direct blockers
 *   - parent     — children must complete for the parent to complete
 *
 * Cross-references (`ref`) are NOT prerequisites and are excluded.
 */

export interface ExtractedSubgraph {
    targetId: string;
    nodes: GraphNode[];
    edges: GraphEdge[];
    /** depth from target, 0 = target itself, growing as we move away */
    distanceFromTarget: Map<string, number>;
}

const PREREQ_EDGE_TYPES = new Set(['depends_on', 'soft_depends_on', 'parent']);

function endpointId(endpoint: string | GraphNode): string {
    return typeof endpoint === 'object' ? endpoint.id : endpoint;
}

/**
 * Pick a sensible default target if the user hasn't focused one.
 * Preference: explicit goal/target/project nodes by focusScore desc.
 */
export function pickDefaultTarget(graph: PreparedGraph | null): string | null {
    if (!graph || graph.nodes.length === 0) return null;
    const ranked = [...graph.nodes].sort((a, b) => (b.focusScore || 0) - (a.focusScore || 0));
    const goal = ranked.find(n => ['goal', 'target', 'project'].includes(n.type));
    if (goal) return goal.id;
    const epic = ranked.find(n => n.type === 'epic');
    if (epic) return epic.id;
    return ranked[0]?.id ?? null;
}

/**
 * Reverse-BFS extraction of the subgraph that must be completed to
 * reach `targetId`. Walks any incoming prerequisite edge into the
 * frontier — i.e. the prerequisite set of the target.
 */
export function extractSubgraph(graph: PreparedGraph, targetId: string): ExtractedSubgraph {
    const incoming = new Map<string, GraphEdge[]>();
    for (const e of graph.links) {
        if (!PREREQ_EDGE_TYPES.has(e.type)) continue;
        // edge meaning:
        //   depends_on:   source depends on target  -> target is prereq for source
        //   parent:       (after prepareGraphData flip) source = parent, target = child
        //                 -> child is a prereq for parent (must complete to bubble up)
        // In both cases, we want to walk from a node to its prereqs.
        // So we index by the node that "needs" something else.
        if (e.type === 'depends_on' || e.type === 'soft_depends_on') {
            const sid = endpointId(e.source);
            const arr = incoming.get(sid) || [];
            arr.push(e);
            incoming.set(sid, arr);
        } else if (e.type === 'parent') {
            // After prepareGraphData: source = parent, target = child
            const parent = endpointId(e.source);
            const arr = incoming.get(parent) || [];
            arr.push(e);
            incoming.set(parent, arr);
        }
    }

    const visited = new Set<string>([targetId]);
    const distanceFromTarget = new Map<string, number>([[targetId, 0]]);
    const queue: string[] = [targetId];

    while (queue.length > 0) {
        const cur = queue.shift()!;
        const dist = distanceFromTarget.get(cur)!;
        const out = incoming.get(cur) || [];
        for (const e of out) {
            const next =
                e.type === 'parent'
                    ? endpointId(e.target) // child of cur
                    : endpointId(e.target); // dependency of cur
            if (visited.has(next)) continue;
            visited.add(next);
            distanceFromTarget.set(next, dist + 1);
            queue.push(next);
        }
    }

    const nodeById = new Map(graph.nodes.map(n => [n.id, n]));
    const nodes = [...visited].map(id => nodeById.get(id)).filter((n): n is GraphNode => !!n);
    const edges = graph.links.filter(e => {
        if (!PREREQ_EDGE_TYPES.has(e.type)) return false;
        return visited.has(endpointId(e.source)) && visited.has(endpointId(e.target));
    });

    return { targetId, nodes, edges, distanceFromTarget };
}

/**
 * Compute the longest-path "dependency depth" of every node — the
 * length of the longest chain of prerequisites that must complete
 * before this node can start. Leaves (no prereqs) get depth 0.
 *
 * Uses Kahn-style topological order. Cycles short-circuit at the
 * cycle's entry depth (defensive — the PKB should be a DAG).
 */
export function computeDependencyDepth(sub: ExtractedSubgraph): Map<string, number> {
    const prereqs = new Map<string, string[]>(); // id -> ids it depends on
    const dependents = new Map<string, string[]>(); // id -> ids that depend on it
    sub.nodes.forEach(n => {
        prereqs.set(n.id, []);
        dependents.set(n.id, []);
    });

    for (const e of sub.edges) {
        const sid = endpointId(e.source);
        const tid = endpointId(e.target);
        if (e.type === 'depends_on' || e.type === 'soft_depends_on') {
            // sid depends on tid
            prereqs.get(sid)?.push(tid);
            dependents.get(tid)?.push(sid);
        } else if (e.type === 'parent') {
            // child must complete for parent — child is prereq for parent
            const parent = sid;
            const child = tid;
            prereqs.get(parent)?.push(child);
            dependents.get(child)?.push(parent);
        }
    }

    const depth = new Map<string, number>();
    const indegree = new Map<string, number>();
    sub.nodes.forEach(n => indegree.set(n.id, prereqs.get(n.id)!.length));

    const queue: string[] = [];
    for (const [id, deg] of indegree) if (deg === 0) {
        queue.push(id);
        depth.set(id, 0);
    }

    while (queue.length > 0) {
        const cur = queue.shift()!;
        const d = depth.get(cur)!;
        for (const dep of dependents.get(cur) || []) {
            const nd = Math.max(depth.get(dep) ?? 0, d + 1);
            depth.set(dep, nd);
            const newDeg = (indegree.get(dep) ?? 1) - 1;
            indegree.set(dep, newDeg);
            if (newDeg === 0) queue.push(dep);
        }
    }

    // Cycle survivors: assign max known depth
    for (const n of sub.nodes) if (!depth.has(n.id)) depth.set(n.id, 0);
    return depth;
}

/**
 * Connected components on the subgraph (ignoring direction). Used
 * for swimlane/cluster detection. The target node is excluded so
 * clusters are the independent paths converging at it.
 */
export function findClusters(sub: ExtractedSubgraph): GraphNode[][] {
    const adj = new Map<string, Set<string>>();
    sub.nodes.forEach(n => adj.set(n.id, new Set()));
    for (const e of sub.edges) {
        const sid = endpointId(e.source);
        const tid = endpointId(e.target);
        if (sid === sub.targetId || tid === sub.targetId) continue;
        adj.get(sid)?.add(tid);
        adj.get(tid)?.add(sid);
    }

    const seen = new Set<string>([sub.targetId]);
    const clusters: GraphNode[][] = [];
    const nodeById = new Map(sub.nodes.map(n => [n.id, n]));

    for (const n of sub.nodes) {
        if (seen.has(n.id)) continue;
        const stack = [n.id];
        const comp: GraphNode[] = [];
        while (stack.length) {
            const cur = stack.pop()!;
            if (seen.has(cur)) continue;
            seen.add(cur);
            const node = nodeById.get(cur);
            if (node) comp.push(node);
            for (const nb of adj.get(cur) || []) if (!seen.has(nb)) stack.push(nb);
        }
        if (comp.length > 0) clusters.push(comp);
    }

    // Stable order: largest first, then highest focusScore
    clusters.sort((a, b) => {
        if (b.length !== a.length) return b.length - a.length;
        const fa = a.reduce((s, n) => s + (n.focusScore || 0), 0);
        const fb = b.reduce((s, n) => s + (n.focusScore || 0), 0);
        return fb - fa;
    });
    return clusters;
}

export function isCompleted(n: GraphNode): boolean {
    return COMPLETED_STATUSES.has(n.status);
}

export function clusterLabel(cluster: GraphNode[]): string {
    // Heuristic: the cluster's structural ancestor (epic/project/goal) if all
    // share one, otherwise use the highest-focus node label.
    const projects = new Set(cluster.map(n => n.project).filter(Boolean));
    if (projects.size === 1) return [...projects][0]!;
    const top = [...cluster].sort((a, b) => (b.focusScore || 0) - (a.focusScore || 0))[0];
    return top?.label || top?.id || 'cluster';
}
