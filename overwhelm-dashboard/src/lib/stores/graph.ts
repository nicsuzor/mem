import { writable, derived } from 'svelte/store';
import type { PreparedGraph } from '../data/prepareGraphData';

export const graphData = writable<PreparedGraph | null>(null);

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
