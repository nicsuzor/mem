import { writable } from 'svelte/store';

export const VIEW_MODES = ['Treemap', 'Circle Pack', 'SFDP', 'Arc Diagram'] as const;
export type ViewMode = typeof VIEW_MODES[number];

export const viewSettings = writable({
    showSidebar: false, // controls sidebar visibility
    mainTab: 'Dashboard', // 'Dashboard' or 'Task Graph'
    viewMode: 'Treemap',  // "Treemap", "Circle Pack", "SFDP", "Arc Diagram"
    topNLeaves: 80,
    liveSimulation: true,
    chargeStrength: 1.0,
    linkDistance: 1.0,
    collisionRadius: 1.2,
    gravity: 0.05,
    alphaDecay: 0.04,
    velocityDecay: 0.7,
    circleRollupThreshold: 15,
    arcVerticalSpacing: 1.0,
    treemapWeightMode: 'priority' as 'sqrt' | 'priority' | 'dw-bucket' | 'equal',
    showIntentionPath: true,
    showFocusHighlight: true,
    arcFocusedOnly: true,
});

export const getLayoutFromViewSettings = ($settings: any) => {
    switch ($settings.viewMode) {
        case 'Treemap':
            return 'treemap';
        case 'Circle Pack':
            return 'circle_pack';
        case 'SFDP':
            return 'force';
        case 'Arc Diagram':
            return 'arc';
        default:
            return 'force';
    }
}

/** Graph layout key — all views use one graph file since layouts are computed client-side */
export const getGraphLayoutKey = (_$settings: any): string => 'graph';
