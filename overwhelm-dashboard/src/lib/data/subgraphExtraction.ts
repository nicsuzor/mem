import type { GraphNode, GraphEdge, PreparedGraph } from './prepareGraphData';
import { COMPLETED_STATUSES, INCOMPLETE_STATUSES } from './constants';

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

export interface MultiTargetSubgraph {
    targets: GraphNode[];
    nodes: GraphNode[];
    edges: GraphEdge[];
    /** node id -> set of target ids this node contributes to */
    routes: Map<string, Set<string>>;
    /** node id -> minimum BFS distance to any target */
    distanceToNearest: Map<string, number>;
    /**
     * Per-node provenance: how the node entered the subgraph.
     *  - "target":     it IS one of the picked targets
     *  - "prereq":     reachable via depends_on / soft_depends_on / parent-down BFS
     *                  from a target (a strict prerequisite)
     *  - "ancestor":   walking parent-up from a target (the containing epic /
     *                  project / area-of-work the target lives under)
     *  - "sibling":    a sibling task under one of the target's ancestors —
     *                  i.e. "other work in the same project that contributes
     *                  to the same goal." Not a hard prerequisite, but the
     *                  practical work that gets the target delivered.
     */
    provenance: Map<string, 'target' | 'prereq' | 'ancestor' | 'sibling'>;
}

const PREREQ_EDGE_TYPES = new Set(['depends_on', 'soft_depends_on', 'parent', 'contributes_to']);

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
 * Discover ALL active targets — incomplete nodes whose `node_type` is
 * `target`, falling back to P0/P1 incomplete nodes if none exist.
 * Mirrors the discovery rule used by MetroRadialView so the
 * experimental views show the same destinations the user is steering by.
 */
export function pickAllTargets(graph: PreparedGraph | null): GraphNode[] {
    if (!graph) return [];
    const incomplete = graph.nodes.filter(n => INCOMPLETE_STATUSES.has(n.status));
    const explicit = incomplete.filter(n => (n.type || '').toLowerCase() === 'target');
    const pool = explicit.length > 0
        ? explicit
        : incomplete.filter(n => n.priority === 0 || n.priority === 1);
    return pool.sort((a, b) =>
        (a.priority ?? 4) - (b.priority ?? 4)
        || (a.project || '').localeCompare(b.project || '')
        || (a.label || '').localeCompare(b.label || '')
    );
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
        if (e.type === 'depends_on' || e.type === 'soft_depends_on' || e.type === 'contributes_to') {
            // sid depends on / contributes to tid -> tid is a prereq for sid
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
            // For both parent and depends_on edges, target endpoint is the
            // node we want to walk into next (child for parent-down, prereq
            // for depends_on).
            const next = endpointId(e.target);
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

export interface MultiTargetExtractionOptions {
    /** How many parent levels to walk up from each target (default 2). */
    hopUp?: number;
    /**
     * Whether to fold sibling tasks under each ancestor into the
     * subgraph as "contributing" work. Targets in the PKB tend to be
     * leaf-shaped — without this, the prerequisite subgraph is almost
     * empty, even when the project has dozens of incomplete tasks. The
     * sibling fan surfaces the practical work that delivers the target.
     * (default true)
     */
    includeSiblings?: boolean;
}

/**
 * Reverse-BFS from MULTIPLE targets at once. The result is the union of
 * every target's prerequisite subgraph; `routes` records, for each node,
 * which targets it contributes to (a node may serve multiple targets if
 * it's a shared prerequisite).
 *
 * Expansion rules per target (mirrors MetroRadialView's discovery so
 * the views agree on what "the work to reach this goal" means):
 *
 *   1. The target itself.
 *   2. Walk DOWN parent-edges (parent → child) and depends_on / soft_depends_on
 *      from the target — its strict prerequisites.
 *   3. Walk UP parent-edges to ancestors, up to `hopUp` levels — the
 *      epic / project / area the target lives under.
 *   4. From each ancestor, fold in its **sibling subtree**: incomplete
 *      tasks that share an ancestor with the target. These are not
 *      hard prerequisites in the depends_on sense, but they ARE the
 *      practical work that delivers the goal (provenance: 'sibling').
 *      Re-run prereq BFS from the siblings so their depends_on chains
 *      come along too.
 */
export function extractMultiTargetSubgraph(
    graph: PreparedGraph,
    targetIds: string[],
    options: MultiTargetExtractionOptions = {},
): MultiTargetSubgraph {
    const HOP_UP = options.hopUp ?? 2;
    const INCLUDE_SIBLINGS = options.includeSiblings ?? true;

    // ── Edge indexes
    const prereqOut = new Map<string, GraphEdge[]>(); // node -> edges from it to its prereqs
    const childrenOf = new Map<string, string[]>();   // parent -> children (from parent edges)
    for (const e of graph.links) {
        const sid = endpointId(e.source);
        const tid = endpointId(e.target);
        if (e.type === 'parent') {
            // After prepareGraphData flip: source = parent, target = child
            const arr = prereqOut.get(sid) || [];
            arr.push(e);
            prereqOut.set(sid, arr);
            const c = childrenOf.get(sid) || [];
            c.push(tid);
            childrenOf.set(sid, c);
        } else if (e.type === 'depends_on' || e.type === 'soft_depends_on' || e.type === 'contributes_to') {
            // sid depends on / contributes to tid — tid is a prereq for sid
            const arr = prereqOut.get(sid) || [];
            arr.push(e);
            prereqOut.set(sid, arr);
        }
    }
    const parentOf = new Map<string, string | null>();
    for (const n of graph.nodes) parentOf.set(n.id, n.parent);

    const nodeById = new Map(graph.nodes.map(n => [n.id, n]));
    const routes = new Map<string, Set<string>>();
    const distanceToNearest = new Map<string, number>();
    const provenance = new Map<string, 'target' | 'prereq' | 'ancestor' | 'sibling'>();
    const validTargets: GraphNode[] = [];

    const ensureRoute = (nid: string, tid: string) => {
        const s = routes.get(nid) || new Set<string>();
        s.add(tid);
        routes.set(nid, s);
    };
    const setProv = (id: string, p: 'target' | 'prereq' | 'ancestor' | 'sibling') => {
        // Provenance precedence: target > prereq > sibling > ancestor.
        // (a node both directly required and inherited as ancestor should
        // read as the stricter "prereq".)
        const order = { target: 4, prereq: 3, sibling: 2, ancestor: 1 } as const;
        const cur = provenance.get(id);
        if (!cur || order[p] > order[cur]) provenance.set(id, p);
    };

    // BFS prereq closure from a seed set, attributing routes to a target id
    const prereqClosure = (seeds: string[], tid: string, seedProv: 'target' | 'sibling') => {
        const dist = new Map<string, number>();
        const queue: string[] = [];
        for (const s of seeds) {
            if (!nodeById.has(s)) continue;
            if (!dist.has(s)) {
                dist.set(s, 0);
                queue.push(s);
                setProv(s, seedProv);
                ensureRoute(s, tid);
            }
        }
        while (queue.length > 0) {
            const cur = queue.shift()!;
            const d = dist.get(cur)!;
            for (const e of prereqOut.get(cur) || []) {
                const next = endpointId(e.target);
                if (dist.has(next)) continue;
                dist.set(next, d + 1);
                ensureRoute(next, tid);
                if (provenance.get(next) !== 'target') setProv(next, 'prereq');
                queue.push(next);
            }
        }
        for (const [id, d] of dist) {
            const prev = distanceToNearest.get(id);
            if (prev === undefined || d < prev) distanceToNearest.set(id, d);
        }
    };

    for (const tid of targetIds) {
        const t = nodeById.get(tid);
        if (!t) continue;
        validTargets.push(t);
        setProv(tid, 'target');

        // 1+2. Strict prereq closure starting at the target itself
        prereqClosure([tid], tid, 'target');

        // 3+4. Ancestor walk and sibling fan
        if (HOP_UP > 0 || INCLUDE_SIBLINGS) {
            let cur = parentOf.get(tid) || null;
            const siblingSeeds = new Set<string>();
            for (let hop = 0; hop < HOP_UP && cur; hop++) {
                ensureRoute(cur, tid);
                if (provenance.get(cur) !== 'target' && provenance.get(cur) !== 'prereq') {
                    setProv(cur, 'ancestor');
                }
                if (INCLUDE_SIBLINGS) {
                    for (const sib of childrenOf.get(cur) || []) {
                        if (sib === tid) continue;
                        const sn = nodeById.get(sib);
                        if (!sn) continue;
                        // Skip noise: dailies, contacts, knowledge notes,
                        // memories — they're not "the work."
                        const skipTypes = new Set(['daily', 'contact', 'knowledge', 'memory', 'reference', 'note', 'index', 'session-log']);
                        if (skipTypes.has((sn.type || '').toLowerCase())) continue;
                        siblingSeeds.add(sib);
                    }
                }
                cur = parentOf.get(cur) || null;
            }
            if (siblingSeeds.size > 0) {
                prereqClosure([...siblingSeeds], tid, 'sibling');
            }
        }
    }

    const nodes = [...routes.keys()].map(id => nodeById.get(id)).filter((n): n is GraphNode => !!n);
    const visited = new Set(routes.keys());
    // Edges: include parent + depends_on edges among visited nodes. We
    // intentionally include parent edges where one endpoint is an ancestor
    // — those are what visualise the project containment.
    const edges = graph.links.filter(e => {
        if (!PREREQ_EDGE_TYPES.has(e.type)) return false;
        return visited.has(endpointId(e.source)) && visited.has(endpointId(e.target));
    });

    return { targets: validTargets, nodes, edges, routes, distanceToNearest, provenance };
}

/**
 * Convenience wrapper: take a multi-target subgraph and turn it into a
 * single-target shape suitable for the existing `computeDependencyDepth`
 * and `findClusters` helpers — just by picking one of the targets to act
 * as the centroid for distance bookkeeping. Cluster detection is unchanged
 * since it ignores the target.
 */
export function multiAsExtracted(multi: MultiTargetSubgraph): ExtractedSubgraph {
    return {
        targetId: multi.targets[0]?.id ?? '',
        nodes: multi.nodes,
        edges: multi.edges,
        distanceFromTarget: multi.distanceToNearest,
    };
}

/**
 * Connected components on a multi-target subgraph, treating ALL target
 * nodes as cut points (so clusters are the truly independent paths
 * upstream of the targets, never bridged through a target).
 */
export function findMultiClusters(multi: MultiTargetSubgraph): GraphNode[][] {
    const targetSet = new Set(multi.targets.map(t => t.id));
    const adj = new Map<string, Set<string>>();
    multi.nodes.forEach(n => adj.set(n.id, new Set()));
    for (const e of multi.edges) {
        const sid = endpointId(e.source);
        const tid = endpointId(e.target);
        if (targetSet.has(sid) || targetSet.has(tid)) continue;
        adj.get(sid)?.add(tid);
        adj.get(tid)?.add(sid);
    }
    const seen = new Set<string>(targetSet);
    const clusters: GraphNode[][] = [];
    const nodeById = new Map(multi.nodes.map(n => [n.id, n]));
    for (const n of multi.nodes) {
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
    clusters.sort((a, b) => {
        if (b.length !== a.length) return b.length - a.length;
        const fa = a.reduce((s, n) => s + (n.focusScore || 0), 0);
        const fb = b.reduce((s, n) => s + (n.focusScore || 0), 0);
        return fb - fa;
    });
    return clusters;
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
