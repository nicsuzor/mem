<script lang="ts">
    import { toast } from '../../stores/toast';
</script>

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
