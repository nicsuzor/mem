import { writable } from 'svelte/store';

export const VIEW_MODES = [
    'Treemap', 'Circle Pack', 'Force', 'Metro', 'Metro V2', 'Arc Diagram', 'Groups',
    // Experimental "path-to-goal" alternatives to ForceView
    'Swimlanes', 'DSM', 'Ribbons', 'HTA Tree', 'Wave Kanban',
] as const;
export type ViewMode = typeof VIEW_MODES[number];

export const viewSettings = writable<Record<string, any>>({
    mainTab: 'Dashboard', // 'Dashboard' or 'Task Graph' or 'Insights'
    viewMode: 'Treemap',  // "Treemap", "Circle Pack", "Force", "Arc Diagram"
    showLegend: true,
    showGraphConfig: false,
    topNLeaves: 80,
    metroAlgorithm: 'force' as 'force' | 'elk' | 'cola',
    colaLinkLength: 600,   // ideal link length (general, used by Metro)
    enableEpicGrouping: false, // shared toggle for epic compound nodes in physics graphs

    // Link-specific forces
    colaLinkDistIntraParent: 100,
    colaLinkWeightIntraParent: 0.4,
    colaLinkDistInterParent: 250,
    colaLinkWeightInterParent: 0.4,
    colaLinkDistDependsOn: 200,
    colaLinkWeightDependsOn: 0.1,
    colaLinkDistSoftDependsOn: 400,
    colaLinkWeightSoftDependsOn: 0.1,
    colaLinkDistContributesTo: 400,
    colaLinkWeightContributesTo: 0.1,
    colaLinkDistSimilarTo: 600,
    colaLinkWeightSimilarTo: 0.1,
    colaLinkDistRef: 600,
    colaLinkWeightRef: 0.1,

    colaConvergence: 0.05, // convergence threshold - must be < 0.1 (Cola's initial alpha)
    colaFlowSep: 40,       // min vertical separation between linked nodes
    colaGroupPadding: 15,  // padding inside epic group hulls - keeps non-descendants out
    // Cola debug toggles - turn on one at a time to isolate layout issues
    colaAvoidOverlaps: true,
    colaGroups: true,
    colaLinks: true,
    colaHandleDisconnected: true,
    circleRollupThreshold: 15,
    arcVerticalSpacing: 1.0,
    treemapWeightMode: 'sqrt' as 'sqrt' | 'priority' | 'focus-bucket' | 'equal',
    arcFocusedOnly: true,
    showFocusHighlight: true,
    activeOverlay: null as string | null, // legacy overlay field; keep until older callers are removed
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
        case 'Metro V2':
            return 'metro_radial';
        case 'Arc Diagram':
            return 'arc';
        case 'Groups':
            return 'groups';
        case 'Swimlanes':
            return 'swimlanes';
        case 'DSM':
            return 'dsm';
        case 'Ribbons':
            return 'ribbons';
        case 'HTA Tree':
            return 'hta';
        case 'Wave Kanban':
            return 'wave_kanban';
        default:
            return 'force';
    }
}

/** Graph layout key - all views use one graph file since layouts are computed client-side */
export const getGraphLayoutKey = (_$settings: any): string => 'graph';
