<script lang="ts">
    import { quickAction, QUICK_ACTION_META, type QuickStatus } from '../../stores/queueActions';

    type Props = {
        taskId: string;
        // Which of the three quick actions to expose; default = all three
        actions?: QuickStatus[];
        // Hover-reveal variant (e.g., within a table row) vs always-visible variant (drawers)
        hoverReveal?: boolean;
        size?: 'sm' | 'md';
    };

    let { taskId, actions = ['done', 'archived', 'cancelled'], hoverReveal = true, size = 'sm' }: Props = $props();

    let busy = $state<QuickStatus | null>(null);

    async function handle(event: MouseEvent, status: QuickStatus) {
        event.stopPropagation();
        event.preventDefault();
        if (busy) return;
        busy = status;
        try {
            await quickAction(taskId, status);
        } finally {
            busy = null;
        }
    }

    const toneClass: Record<string, string> = {
        success: 'hover:bg-green-900/30 hover:text-green-400 hover:border-green-500/40',
        neutral: 'hover:bg-primary/10 hover:text-primary hover:border-primary/40',
        danger: 'hover:bg-red-900/30 hover:text-red-400 hover:border-red-500/40',
    };

    let iconSize = $derived(size === 'md' ? 'text-[16px]' : 'text-[13px]');
    let padding = $derived(size === 'md' ? 'px-2 py-1' : 'px-1.5 py-0.5');
</script>

<div
    class="inline-flex items-center gap-1 {hoverReveal ? 'opacity-0 group-hover:opacity-100 focus-within:opacity-100 transition-opacity' : ''}"
    data-component="task-actions"
    role="group"
    aria-label="Quick actions"
>
    {#each actions as status (status)}
        {@const meta = QUICK_ACTION_META[status]}
        <button
            type="button"
            class="inline-flex items-center gap-1 border border-primary/15 bg-black/40 text-primary/60 {padding} text-[9px] font-bold uppercase tracking-widest rounded-sm transition-colors disabled:opacity-50 disabled:pointer-events-none {toneClass[meta.tone]}"
            title={`${meta.label} (no drill-down)`}
            aria-label={meta.label}
            disabled={busy !== null}
            onclick={(e) => handle(e, status)}
        >
            <span class="material-symbols-outlined {iconSize}">{busy === status ? 'progress_activity' : meta.icon}</span>
            {#if size === 'md'}
                <span>{meta.label}</span>
            {/if}
        </button>
    {/each}
</div>
