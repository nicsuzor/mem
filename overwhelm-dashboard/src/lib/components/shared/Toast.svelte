<script lang="ts">
    import { toast } from '../../stores/toast';
    import { taskOperations } from '../../stores/taskOperations';

    let visibleOperations = $derived.by(() => {
        const entries = $taskOperations;
        const pending = entries.filter((entry) => entry.status === 'pending').reverse();
        const errors = entries.filter((entry) => entry.status === 'error').reverse();
        const successes = entries.filter((entry) => entry.status === 'success')
            .slice(-Math.max(0, 4 - pending.length - errors.length))
            .reverse();
        return [...errors, ...pending, ...successes].slice(0, 6);
    });

    function handleRetry(operation: typeof visibleOperations[number]) {
        const retry = operation.retry;
        taskOperations.remove(operation.id);
        retry?.();
    }
</script>

{#if visibleOperations.length > 0}
    <div class="fixed right-4 top-20 z-[980] flex w-[min(22rem,calc(100vw-1.5rem))] flex-col gap-2 pointer-events-none">
        {#each visibleOperations as operation (operation.id)}
            <div
                class={`rounded-lg border bg-black/80 px-3 py-2 shadow-[0_10px_30px_rgba(0,0,0,0.35)] backdrop-blur-sm ${
                    operation.status === 'pending'
                        ? 'border-primary/30 text-primary'
                        : operation.status === 'success'
                            ? 'border-emerald-500/50 text-emerald-100'
                            : 'border-red-500/50 text-red-100 pointer-events-auto'
                }`}
            >
                <div class="flex items-start gap-2">
                    <span class="material-symbols-outlined mt-0.5 text-[15px]" class:animate-pulse={operation.status === 'pending'}>
                        {#if operation.status === 'pending'}
                            sync
                        {:else if operation.status === 'success'}
                            check_circle
                        {:else}
                            error
                        {/if}
                    </span>
                    <div class="min-w-0 flex-1">
                        <div class="truncate text-[10px] font-black uppercase tracking-[0.16em]">{operation.label}</div>
                        <div class="mt-1 flex items-center gap-2 text-[9px] font-mono opacity-80">
                            <span class="truncate">{operation.taskId}</span>
                            <span class="opacity-40">/</span>
                            <span class="truncate">{operation.detail}</span>
                        </div>
                        {#if operation.status === 'error'}
                            <div class="mt-1.5 flex items-center gap-1.5">
                                {#if operation.retry}
                                    <button
                                        type="button"
                                        class="inline-flex items-center gap-1 rounded-sm border border-red-400/50 bg-red-500/10 px-2 py-0.5 text-[9px] font-bold uppercase tracking-[0.14em] text-red-100 hover:bg-red-500/20 transition-colors"
                                        onclick={() => handleRetry(operation)}
                                    >
                                        <span class="material-symbols-outlined text-[11px]">refresh</span>
                                        Retry
                                    </button>
                                {/if}
                                <button
                                    type="button"
                                    class="inline-flex items-center gap-1 rounded-sm border border-red-400/30 bg-transparent px-2 py-0.5 text-[9px] font-bold uppercase tracking-[0.14em] text-red-100/80 hover:bg-red-500/10 transition-colors"
                                    onclick={() => taskOperations.remove(operation.id)}
                                >
                                    Dismiss
                                </button>
                            </div>
                        {/if}
                    </div>
                </div>
            </div>
        {/each}
    </div>
{/if}

{#if $toast.length > 0}
    <div class="fixed top-4 left-1/2 -translate-x-1/2 z-[1000] flex flex-col items-center gap-2 pointer-events-none">
        {#each $toast as t (t.id)}
            <div
                class="px-4 py-2 rounded-md font-mono text-sm font-bold shadow-lg flex items-center gap-2 transition-all"
                class:bg-emerald-900={t.type === 'success'}
                class:text-emerald-100={t.type === 'success'}
                class:border={true}
                class:border-emerald-500={t.type === 'success'}
                class:bg-red-900={t.type === 'error'}
                class:text-red-100={t.type === 'error'}
                class:border-red-500={t.type === 'error'}
                class:bg-blue-900={t.type === 'info'}
                class:text-blue-100={t.type === 'info'}
                class:border-blue-500={t.type === 'info'}
            >
                <span class="material-symbols-outlined text-base">
                    {#if t.type === 'success'}
                        check_circle
                    {:else if t.type === 'error'}
                        error
                    {:else}
                        info
                    {/if}
                </span>
                {t.message}
            </div>
        {/each}
    </div>
{/if}
