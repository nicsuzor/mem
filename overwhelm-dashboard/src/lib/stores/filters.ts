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
    
    // Default all priorities to bright
    priority0: 'bright' as VisibilityState,
    priority1: 'bright' as VisibilityState,
    priority2: 'bright' as VisibilityState,
    priority3: 'bright' as VisibilityState,
    priority4: 'bright' as VisibilityState,

    hiddenProjects: [] as string[],
    
    edgeDependencies: 'half' as VisibilityState,
    edgeReferences: 'hidden' as VisibilityState,
    edgeParent: 'bright' as VisibilityState,
});
