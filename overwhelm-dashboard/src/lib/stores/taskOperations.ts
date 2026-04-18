import { writable } from 'svelte/store';

export type TaskOperationStatus = 'pending' | 'success' | 'error';

export interface TaskOperationEntry {
    id: number;
    taskId: string;
    label: string;
    detail: string;
    status: TaskOperationStatus;
    startedAt: number;
    completedAt?: number;
}

const MAX_OPERATIONS = 6;
const SUCCESS_TIMEOUT_MS = 2600;
const ERROR_TIMEOUT_MS = 8000;

function capOperations(entries: TaskOperationEntry[]) {
    if (entries.length <= MAX_OPERATIONS) return entries;

    const pending = entries.filter((entry) => entry.status === 'pending');
    const resolved = entries.filter((entry) => entry.status !== 'pending');
    const keepResolved = Math.max(0, MAX_OPERATIONS - pending.length);

    return [...pending, ...resolved.slice(-keepResolved)];
}

function createTaskOperationsStore() {
    const { subscribe, update } = writable<TaskOperationEntry[]>([]);
    const removalTimers = new Map<number, ReturnType<typeof setTimeout>>();
    let nextId = 0;

    const clearRemovalTimer = (id: number) => {
        const timer = removalTimers.get(id);
        if (!timer) return;

        clearTimeout(timer);
        removalTimers.delete(id);
    };

    const scheduleRemoval = (id: number, timeoutMs: number) => {
        clearRemovalTimer(id);
        const timer = setTimeout(() => {
            update((entries) => entries.filter((entry) => entry.id !== id));
            removalTimers.delete(id);
        }, timeoutMs);
        removalTimers.set(id, timer);
    };

    return {
        subscribe,
        start: (taskId: string, label: string, detail = 'Saving…') => {
            const id = nextId++;
            update((entries) => capOperations([
                ...entries,
                {
                    id,
                    taskId,
                    label,
                    detail,
                    status: 'pending',
                    startedAt: Date.now(),
                },
            ]));
            return id;
        },
        succeed: (id: number, detail = 'Saved') => {
            update((entries) => entries.map((entry) => (
                entry.id === id
                    ? { ...entry, status: 'success', detail, completedAt: Date.now() }
                    : entry
            )));
            scheduleRemoval(id, SUCCESS_TIMEOUT_MS);
        },
        fail: (id: number, detail = 'Failed') => {
            update((entries) => entries.map((entry) => (
                entry.id === id
                    ? { ...entry, status: 'error', detail, completedAt: Date.now() }
                    : entry
            )));
            scheduleRemoval(id, ERROR_TIMEOUT_MS);
        },
        remove: (id: number) => {
            clearRemovalTimer(id);
            update((entries) => entries.filter((entry) => entry.id !== id));
        },
    };
}

export function describeTaskMutation(updates: {
    status?: string;
    priority?: number;
    assignee?: string;
    refile?: boolean;
    type?: string;
}) {
    if (updates.refile) return 'Mark for refile';
    if (updates.status) return `Set status to ${updates.status}`;
    if (updates.priority !== undefined) return `Set priority to P${updates.priority}`;
    if (updates.assignee !== undefined) return updates.assignee ? `Assign to ${updates.assignee}` : 'Clear assignee';
    if (updates.type) return `Set type to ${updates.type}`;
    return 'Update task';
}

export const taskOperations = createTaskOperationsStore();
