import { writable, get } from 'svelte/store';

export interface SelectionState {
    activeNodeId: string | null;
    activeSessionId: string | null;
    focusNodeId: string | null;  // non-null = ego network focus mode
    focusNeighborSet: Set<string> | null;
    hoveredNodeId: string | null;
}

export const selection = writable<SelectionState>({
    activeNodeId: null,
    activeSessionId: null,
    focusNodeId: null,
    focusNeighborSet: null,
    hoveredNodeId: null
});

export function clearSelection() {
    selection.update(s => ({ ...s, activeNodeId: null, activeSessionId: null }));
}

export function toggleSelection(nodeId: string) {
    selection.update(s => ({
        ...s,
        activeNodeId: s.activeNodeId === nodeId ? null : nodeId,
        activeSessionId: null // clear session when selecting node
    }));
}

export function toggleSessionSelection(sessionId: string) {
    selection.update(s => ({
        ...s,
        activeSessionId: s.activeSessionId === sessionId ? null : sessionId,
        activeNodeId: null // clear node when selecting session
    }));
}

export function setHoveredNode(nodeId: string | null) {
    selection.update(s => ({ ...s, hoveredNodeId: nodeId }));
}
