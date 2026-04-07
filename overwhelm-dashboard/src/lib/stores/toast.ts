import { writable } from 'svelte/store';

export type ToastType = 'success' | 'error' | 'info';

export interface ToastMessage {
    id: number;
    message: string;
    type: ToastType;
}

function createToastStore() {
    const { subscribe, update } = writable<ToastMessage[]>([]);
    let nextId = 0;

    return {
        subscribe,
        show: (message: string, type: ToastType = 'info', timeoutMs: number = 3000) => {
            const id = nextId++;
            update(msgs => [...msgs, { id, message, type }]);
            setTimeout(() => {
                update(msgs => msgs.filter(m => m.id !== id));
            }, timeoutMs);
        },
        remove: (id: number) => {
            update(msgs => msgs.filter(m => m.id !== id));
        }
    };
}

export const toast = createToastStore();
