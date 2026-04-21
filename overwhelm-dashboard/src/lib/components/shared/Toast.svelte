<script lang="ts">
    import { toast } from '../../stores/toast';
    import { taskOperations } from '../../stores/taskOperations';

    let visibleOperations = $derived.by(() => {
        const entries = $taskOperations;
        const pending = entries.filter((entry) => entry.status === 'pending').reverse();
        const resolved = entries.filter((entry) => entry.status !== 'pending').slice(-Math.max(0, 4 - pending.length)).reverse();
        return [...pending, ...resolved].slice(0, 4);
    });
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
                            : 'border-red-500/50 text-red-100'
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
