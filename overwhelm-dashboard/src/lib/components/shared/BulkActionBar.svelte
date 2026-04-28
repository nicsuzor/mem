<script lang="ts">
    import { selectedTaskIds, multiSelectActive, bulkAction, clearSelectedTasks, toggleMultiSelect, QUICK_ACTION_META, type QuickStatus } from '../../stores/queueActions';

    let busy = $state(false);
    let lastResult = $state<string | null>(null);

    async function runBulk(status: QuickStatus) {
        const ids = Array.from($selectedTaskIds);
        if (!ids.length || busy) return;
        busy = true;
        lastResult = null;
        try {
            const { ok, failed } = await bulkAction(ids, status);
            lastResult = failed > 0
                ? `${QUICK_ACTION_META[status].label}: ${ok} ok, ${failed} failed`
                : `${QUICK_ACTION_META[status].label}: ${ok} updated`;
            clearSelectedTasks();
        } finally {
            busy = false;
        }
    }
</script>

{#if $multiSelectActive && $selectedTaskIds.size > 0}
    <div
        class="fixed bottom-4 left-1/2 -translate-x-1/2 z-50 border border-primary/40 bg-black/90 backdrop-blur px-4 py-3 shadow-2xl flex items-center gap-3 font-mono"
        data-component="bulk-action-bar"
        role="region"
        aria-label="Bulk actions"
    >
        <span class="text-[11px] font-bold uppercase tracking-widest text-primary">
            {$selectedTaskIds.size} selected
        </span>
        <span class="h-4 w-px bg-primary/30"></span>
        {#each ['done', 'cancelled'] as status (status)}
            {@const meta = QUICK_ACTION_META[status as QuickStatus]}
            <button
                type="button"
                class="inline-flex items-center gap-1.5 px-3 py-1.5 border border-primary/30 text-primary text-[10px] font-bold uppercase tracking-widest hover:bg-primary/15 disabled:opacity-50 disabled:pointer-events-none transition-colors"
                disabled={busy}
                onclick={() => runBulk(status as QuickStatus)}
            >
                <span class="material-symbols-outlined text-[14px]">{busy ? 'progress_activity' : meta.icon}</span>
                {meta.label} all
            </button>
        {/each}
        <span class="h-4 w-px bg-primary/30"></span>
        <button
            type="button"
            class="text-[10px] uppercase tracking-widest text-primary/50 hover:text-primary px-2 py-1"
            onclick={clearSelectedTasks}
        >Clear</button>
        <button
            type="button"
            class="text-[10px] uppercase tracking-widest text-primary/40 hover:text-primary px-2 py-1"
            onclick={toggleMultiSelect}
        >Exit</button>
        {#if lastResult}
            <span class="text-[10px] text-primary/60 pl-2">{lastResult}</span>
        {/if}
    </div>
{/if}
