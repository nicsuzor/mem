import { writable } from 'svelte/store';

export const VIEW_MODES = ['Treemap', 'Circle Pack', 'Force', 'Arc Diagram'] as const;
export type ViewMode = typeof VIEW_MODES[number];

export const viewSettings = writable({
    mainTab: 'Dashboard', // 'Dashboard' or 'Task Graph'
    viewMode: 'Treemap',  // "Treemap", "Circle Pack", "Force", "Arc Diagram"
    topNLeaves: 80,
    chargeStrength: 1.0,  // Multiplier on FORCE_CONFIG.chargeMult
    linkDistance: 1.0,     // Multiplier on EDGE_FORCE distances
    gravity: 0.05,        // Center gravity strength
    circleRollupThreshold: 15,
    arcVerticalSpacing: 1.0,
    treemapWeightMode: 'priority' as 'sqrt' | 'priority' | 'dw-bucket' | 'equal',
    arcFocusedOnly: true,
});

export const getLayoutFromViewSettings = ($settings: any) => {
    switch ($settings.viewMode) {
        case 'Treemap':
            return 'treemap';
        case 'Circle Pack':
            return 'circle_pack';
        case 'Force':
            return 'force';
        case 'Arc Diagram':
            return 'arc';
        default:
            return 'force';
    }
}

/** Graph layout key — all views use one graph file since layouts are computed client-side */
export const getGraphLayoutKey = (_$settings: any): string => 'graph';
