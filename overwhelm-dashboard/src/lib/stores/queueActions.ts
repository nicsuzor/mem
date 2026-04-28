/**
 * Shared task status-mutation helpers for queue views.
 *
 * Single-row quick actions (done / archive / cancel) and bulk actions
 * both go through here so rollback + toast semantics stay identical.
 */

import { writable, get } from 'svelte/store';
import { updateGraphTaskNode } from './graph';
import { describeTaskMutation, taskOperations } from './taskOperations';

export type QuickStatus = 'done' | 'cancelled';

export const QUICK_ACTION_META: Record<QuickStatus, { label: string; icon: string; tone: 'success' | 'neutral' | 'danger' }> = {
    done: { label: 'Done', icon: 'check_circle', tone: 'success' },
    cancelled: { label: 'Cancel', icon: 'cancel', tone: 'danger' },
};

async function applyStatusToTask(taskId: string, status: QuickStatus): Promise<boolean> {
    const { rollback } = updateGraphTaskNode(taskId, { status });
    const operationId = taskOperations.start(taskId, describeTaskMutation({ status }));
    try {
        const res = await fetch('/api/task/status', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ id: taskId, status }),
        });
        if (!res.ok) {
            const data = await res.json().catch(() => ({}));
            rollback();
            taskOperations.fail(operationId, data.error ?? `HTTP ${res.status}`);
            return false;
        }
        taskOperations.succeed(operationId);
        return true;
    } catch (e: any) {
        rollback();
        taskOperations.fail(operationId, e?.message ?? 'Network error');
        return false;
    }
}

export async function quickAction(taskId: string, status: QuickStatus): Promise<boolean> {
    return applyStatusToTask(taskId, status);
}

export async function bulkAction(taskIds: string[], status: QuickStatus): Promise<{ ok: number; failed: number }> {
    const results = await Promise.all(taskIds.map((id) => applyStatusToTask(id, status)));
    const ok = results.filter(Boolean).length;
    return { ok, failed: results.length - ok };
}

/** Multi-select state for the queue views. Toggle-based, survives view switches within a session. */
export const multiSelectActive = writable<boolean>(false);
export const selectedTaskIds = writable<Set<string>>(new Set());

export function toggleMultiSelect(): void {
    multiSelectActive.update((v) => {
        if (v) selectedTaskIds.set(new Set());
        return !v;
    });
}

export function toggleSelectedTask(id: string): void {
    selectedTaskIds.update((set) => {
        const next = new Set(set);
        if (next.has(id)) next.delete(id);
        else next.add(id);
        return next;
    });
}

export function clearSelectedTasks(): void {
    selectedTaskIds.set(new Set());
}

export function selectedCount(): number {
    return get(selectedTaskIds).size;
}
