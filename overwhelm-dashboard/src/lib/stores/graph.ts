import { writable, derived } from 'svelte/store';
import type { PreparedGraph } from '../data/prepareGraphData';
import type { GraphNode } from '../data/prepareGraphData';
import { STATUS_FILLS, STATUS_TEXT } from '../data/constants';

export const graphData = writable<PreparedGraph | null>(null);
// Pre-filter, post-prepareGraphData view of the graph. Views that build
// structural visualizations (Metro) subscribe to this so completed /
// hidden-by-filter nodes still reach route discovery; applying user
// visibility filters on top is done per-view.
export const preparedGraphData = writable<PreparedGraph | null>(null);

export interface TaskNodeUpdates {
    status?: string;
    priority?: number;
    assignee?: string;
    refile?: boolean;
    type?: string;
}

const FADED_STATUSES = new Set(['done', 'cancelled']);

function cloneNodeSnapshot(node: GraphNode) {
    const mutableNode = node as any;

    return {
        ...mutableNode,
        _raw: mutableNode._raw && typeof mutableNode._raw === 'object'
            ? { ...mutableNode._raw }
            : mutableNode._raw,
    };
}

function restoreNodeSnapshot(node: GraphNode, snapshot: Record<string, unknown>) {
    const mutableNode = node as any;

    Object.keys(mutableNode).forEach((key) => {
        if (!(key in snapshot)) {
            delete mutableNode[key];
        }
    });

    Object.assign(mutableNode, snapshot);
}

export function applyTaskNodeUpdates(node: GraphNode, updates: TaskNodeUpdates) {
    const { refile, ...nodeUpdates } = updates;
    Object.assign(node, nodeUpdates);

    if (refile !== undefined) {
        const raw = ((node as any)._raw && typeof (node as any)._raw === 'object')
            ? (node as any)._raw
            : ((node as any)._raw = {});
        raw.refile = refile;
    }

    if (nodeUpdates.status) {
        node.fill = STATUS_FILLS[nodeUpdates.status] ?? node.fill;
        node.textColor = STATUS_TEXT[nodeUpdates.status] ?? node.textColor;
        node.opacity = FADED_STATUSES.has(nodeUpdates.status) ? 0.4 : 0.8;
    }

    (node as any)._lastSelected = undefined;
}

export function updateGraphTaskNode(taskId: string, updates: TaskNodeUpdates) {
    let snapshot: Record<string, unknown> | null = null;

    graphData.update((currentGraph) => {
        if (!currentGraph) return currentGraph;

        const node = currentGraph.nodes.find((candidate) => candidate.id === taskId);
        if (!node) return currentGraph;

        snapshot = cloneNodeSnapshot(node);
        applyTaskNodeUpdates(node, updates);
        return currentGraph;
    });

    return {
        applied: snapshot !== null,
        rollback: () => {
            if (!snapshot) return;

            graphData.update((currentGraph) => {
                if (!currentGraph) return currentGraph;

                const node = currentGraph.nodes.find((candidate) => candidate.id === taskId);
                if (!node) return currentGraph;

                restoreNodeSnapshot(node, snapshot!);
                return currentGraph;
            });
        },
    };
}

/**
 * Structural fingerprint — changes only when the set of node IDs or links changes,
 * NOT when individual node properties (status, priority, etc.) are updated.
 * Views that need full rebuilds (Force, Metro) should key off this.
 */
export const graphStructureKey = derived(graphData, ($gd) => {
    if (!$gd) return '';
    const nodeIds = $gd.nodes.map(n => n.id).sort().join(',');
    const linkKeys = $gd.links.map(l => {
        const sid = typeof l.source === 'object' ? (l.source as any).id : l.source;
        const tid = typeof l.target === 'object' ? (l.target as any).id : l.target;
        return `${sid}>${tid}`;
    }).sort().join(',');
    return `${nodeIds}|${linkKeys}`;
});

/**
 * Structural fingerprint for the pre-filter prepared graph. Metro uses this
 * so user filter changes (which alter $graphData) don't cause unnecessary
 * rebuilds — Metro only rebuilds when the underlying PKB structure changes.
 */
export const preparedStructureKey = derived(preparedGraphData, ($pg) => {
    if (!$pg) return '';
    const nodeIds = $pg.nodes.map(n => n.id).sort().join(',');
    const linkKeys = $pg.links.map(l => {
        const sid = typeof l.source === 'object' ? (l.source as any).id : l.source;
        const tid = typeof l.target === 'object' ? (l.target as any).id : l.target;
        return `${sid}>${tid}`;
    }).sort().join(',');
    return `${nodeIds}|${linkKeys}`;
});
