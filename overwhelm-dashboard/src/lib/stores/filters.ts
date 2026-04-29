import { writable } from 'svelte/store';

/** Visibility state: 'bright' (full), 'half' (dim), 'hidden' (invisible) */
export type VisibilityState = 'bright' | 'half' | 'hidden';

export function cycleVisibility(current: VisibilityState): VisibilityState {
    if (current === 'bright') return 'half';
    if (current === 'half') return 'hidden';
    return 'bright';
}

export const filters = writable({
    statusActive: 'bright' as VisibilityState,
    statusBlocked: 'bright' as VisibilityState,
    statusCompleted: 'hidden' as VisibilityState,
    statusOrphans: 'hidden' as VisibilityState,

    // Default critical priorities to full visibility; the rest start half-visible.
    priority0: 'bright' as VisibilityState,
    priority1: 'bright' as VisibilityState,
    priority2: 'half' as VisibilityState,
    priority3: 'half' as VisibilityState,
    priority4: 'half' as VisibilityState,

    minCriticality: 0 as number,
    hiddenProjects: [] as string[],
    selectedStatuses: ['inbox', 'ready', 'queued', 'in_progress', 'merge_ready', 'review', 'blocked', 'paused', 'someday'] as string[],

    edgeParent: 'bright' as VisibilityState,
    edgeDependencies: 'bright' as VisibilityState,        // depends_on (hard)
    edgeSoftDependencies: 'bright' as VisibilityState,    // soft_depends_on
    edgeContributes: 'bright' as VisibilityState,         // contributes_to
    edgeSimilar: 'hidden' as VisibilityState,             // similar_to (noisy by default)
    edgeReferences: 'hidden' as VisibilityState,          // link / wikilink
    edgeIntraGroup: 'bright' as VisibilityState,
});
