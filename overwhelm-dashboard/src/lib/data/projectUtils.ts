import type { GraphNode } from './prepareGraphData';

/**
 * Compute a stable hue for a project name.
 */
export function projectHue(projectId: string): number {
    let hash = 0;
    const id = projectId || 'default';
    for (let i = 0; i < id.length; i++) {
        hash = (hash << 5) - hash + id.charCodeAt(i);
        hash |= 0;
    }
    return Math.abs(hash) % 360;
}

/** HSL color string for a project name. */
export function projectColor(name: string): string {
    const hue = projectHue(name);
    return `hsl(${hue}, 55%, 52%)`;
}

/** Darker background tint for a project. */
export function projectBgTint(name: string): string {
    const hue = projectHue(name);
    return `hsla(${hue}, 40%, 20%, 0.3)`;
}

/** Border color for a project. */
export function projectBorderColor(name: string): string {
    const hue = projectHue(name);
    return `hsl(${hue}, 40%, 40%)`;
}

// Patterns that indicate a meaningless/generated project name
const MEANINGLESS_PATTERNS = [
    /^wt-/i,                       // worktree prefix
    /^polecat-/i,                  // polecat crew prefix
    /^crew-/i,                     // crew prefix
    /^swarm-/i,                    // swarm prefix
    /^worker-/i,                   // worker prefix
    /^[a-f0-9]{8,}$/i,            // hex hash
    /^tmp-/i,                      // temp prefix
    /-worktree-/i,                 // worktree in name
    /-wt\d+$/i,                   // worktree suffix
    /^burst-/i,                    // burst prefix
];

/**
 * Detect whether a project name is "meaningless" (worktree, polecat crew, hash, etc.)
 */
export function isMeaninglessName(name: string): boolean {
    return MEANINGLESS_PATTERNS.some(p => p.test(name));
}

/**
 * Build a map from all project names (including sub-projects) to their
 * top-level "major project" name, using the graph hierarchy.
 *
 * Also maps meaningless session project names to their best graph match.
 */
export function buildProjectRollupMap(nodes: GraphNode[]): Map<string, string> {
    const rollup = new Map<string, string>();

    // Build parent lookup
    const nodeById = new Map(nodes.map(n => [n.id, n]));

    // Walk to the root of the parent chain and return the topmost ancestor's
    // project field. With 'project' type nodes filtered out, this stops at
    // whichever node sits closest to the root and still carries a project tag
    // (typically a goal or top-level epic).
    function findMajorProject(node: GraphNode): string | null {
        let topProject: string | null = null;
        let cur: GraphNode | undefined = node;
        const visited = new Set<string>();
        while (cur) {
            if (visited.has(cur.id)) break;
            visited.add(cur.id);
            if (cur.project) topProject = cur.project;
            cur = cur.parent ? nodeById.get(cur.parent) : undefined;
        }
        return topProject;
    }

    // Map every distinct project field value to its major project
    for (const n of nodes) {
        if (!n.project) continue;
        if (rollup.has(n.project)) continue;

        const major = findMajorProject(n);
        if (major && major !== n.project) {
            rollup.set(n.project, major);
        }
    }

    return rollup;
}

/**
 * Resolve a project name to its major project.
 * Falls back to the original name if no rollup exists.
 */
export function resolveMajorProject(name: string, rollupMap: Map<string, string>): string {
    // Direct rollup match
    if (rollupMap.has(name)) return rollupMap.get(name)!;

    // Try matching against known projects by substring
    if (isMeaninglessName(name)) {
        // Find best match from rollup values (major projects)
        const majors = new Set(rollupMap.values());
        for (const major of majors) {
            if (name.includes(major) || major.includes(name)) return major;
        }
    }

    return name;
}

/**
 * Given a label that might be a worktree/crew name, return a human-readable summary.
 * For meaningful names, returns as-is. For meaningless ones, returns the major project or cleans up.
 */
export function summarizeProjectName(name: string, rollupMap: Map<string, string>): string {
    const major = resolveMajorProject(name, rollupMap);
    if (major !== name) return major;

    // Clean up remaining patterns
    if (isMeaninglessName(name)) {
        // Strip common prefixes/suffixes
        return name
            .replace(/^(wt|worktree|polecat|crew|swarm|worker|burst|tmp)-/i, '')
            .replace(/-wt\d+$/i, '')
            .replace(/-worktree-[a-f0-9]+$/i, '')
            .replace(/^[a-f0-9]{8,}$/i, 'unnamed')
            || name;
    }

    return name;
}
