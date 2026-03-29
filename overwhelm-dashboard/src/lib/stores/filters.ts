import { writable } from 'svelte/store';

/** Edge visibility: 'bright' (full), 'half' (dim), 'hidden' (invisible) */
export type EdgeVisibility = 'bright' | 'half' | 'hidden';

export function cycleEdgeVisibility(current: EdgeVisibility): EdgeVisibility {
    if (current === 'bright') return 'half';
    if (current === 'half') return 'hidden';
    return 'bright';
}

export const filters = writable({
    project: 'ALL',
    showActive: true,
    showBlocked: true,
    showCompleted: false,
    showOrphans: false,
    edgeDependencies: 'half' as EdgeVisibility,
    edgeReferences: 'hidden' as EdgeVisibility,
    edgeParent: 'bright' as EdgeVisibility,
});
