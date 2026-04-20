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

    hiddenProjects: [] as string[],
    selectedStatuses: ['inbox', 'ready', 'todo', 'in_progress', 'active', 'review', 'waiting', 'decomposing', 'dormant', 'blocked'] as string[],

    edgeDependencies: 'half' as VisibilityState,
    edgeReferences: 'hidden' as VisibilityState,
    edgeParent: 'bright' as VisibilityState,
});
