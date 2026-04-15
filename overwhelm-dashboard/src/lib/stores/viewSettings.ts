import { writable } from 'svelte/store';

export const VIEW_MODES = ['Treemap', 'Circle Pack', 'Force', 'Metro', 'Arc Diagram'] as const;
export type ViewMode = typeof VIEW_MODES[number];

export const viewSettings = writable({
    mainTab: 'Dashboard', // 'Dashboard' or 'Task Graph'
    viewMode: 'Treemap',  // "Treemap", "Circle Pack", "Force", "Arc Diagram"
    topNLeaves: 80,
    colaLinkLength: 600,   // ideal link length (general, used by Metro)

    // Link-specific forces
    colaLinkDistIntraParent: 60,
    colaLinkWeightIntraParent: 1.0,
    colaLinkDistInterParent: 150,
    colaLinkWeightInterParent: 0.8,
    colaLinkDistDependsOn: 150,
    colaLinkWeightDependsOn: 0.5,
    colaLinkDistRef: 300,
    colaLinkWeightRef: 0.2,

    colaConvergence: 0.005, // convergence threshold - must be < 0.1 (Cola's initial alpha)
    colaFlowSep: 40,       // min vertical separation between linked nodes
    colaGroupPadding: 15,  // padding inside epic group hulls - keeps non-descendants out
    // Cola debug toggles - turn on one at a time to isolate layout issues
    colaAvoidOverlaps: true,
    colaGroups: true,
    colaLinks: true,
    colaHandleDisconnected: true,
    circleRollupThreshold: 15,
    arcVerticalSpacing: 1.0,
    treemapWeightMode: 'priority' as 'sqrt' | 'priority' | 'dw-bucket' | 'equal',
    arcFocusedOnly: true,
    showFocusHighlight: true,
    activeOverlay: null as string | null, // e.g., 'legend', 'config'
});

export const getLayoutFromViewSettings = ($settings: any) => {
    switch ($settings.viewMode) {
        case 'Treemap':
            return 'treemap';
        case 'Circle Pack':
            return 'circle_pack';
        case 'Force':
            return 'force';
        case 'Metro':
            return 'metro';
        case 'Arc Diagram':
            return 'arc';
        default:
            return 'force';
    }
}

/** Graph layout key - all views use one graph file since layouts are computed client-side */
export const getGraphLayoutKey = (_$settings: any): string => 'graph';
