<script lang="ts">
    import { copyToClipboard, formatText } from "../../data/utils";

    let { sessionId, sessionData, onclose }: { sessionId: string; sessionData: any; onclose: () => void } = $props();

    let session = $derived(sessionData.find((s: any) => s.session_id === sessionId) || null);
    let title = $derived(session?.description || "Unknown Session");
    let metrics = $derived(session?.token_metrics || null);
    
    function formatNumber(num: number | undefined): string {
        if (num === undefined || num === null) return '0';
        return new Intl.NumberFormat('en-US').format(num);
    }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && onclose()} />

{#if !session}
    <div class="flex flex-col items-center justify-center h-full text-primary/30 p-8 text-center bg-background border-l border-primary-border">
        <span class="material-symbols-outlined text-3xl mb-2 text-destructive opacity-50">warning</span>
        <span class="text-[10px] tracking-widest uppercase font-bold text-destructive/80">SESSION_NOT_FOUND</span>
        <button class="mt-4 px-3 py-1.5 border border-primary/20 text-[9px] hover:text-primary hover:border-primary transition-colors uppercase tracking-widest" onclick={onclose}>CLOSE</button>
    </div>
{:else}
    <div class="flex flex-col h-full bg-background overflow-hidden font-mono border-l border-primary/20" data-component="session-pane">
        <!-- Header -->
        <div class="flex flex-col gap-1 p-3 border-b border-primary/20 bg-background shrink-0">
            <div class="flex items-center justify-between">
                <div class="flex items-center gap-1.5 text-[9px] font-mono opacity-60">
                    <span class="text-[7px] italic opacity-30 mr-1">session-pane</span>
                    <span class="uppercase">{session.project || 'VOID'}</span>
                    <span class="text-primary/30">/</span>
                    <button class="text-primary hover:underline flex items-center gap-1" onclick={() => copyToClipboard(session.session_id)}>
                        {session.session_id}
                        <span class="material-symbols-outlined text-[10px]">content_copy</span>
                    </button>
                </div>
                <button class="text-primary/40 hover:text-primary transition-colors" onclick={onclose}>
                    <span class="material-symbols-outlined text-base">close</span>
                </button>
            </div>

            <div class="flex flex-col gap-2 mt-1">
                <div class="group relative">
                    <h1 class="text-base font-black tracking-tight uppercase text-primary leading-tight pr-6 line-clamp-3">
                        {@html formatText(title)}
                    </h1>
                    <button class="absolute top-0 right-0 text-primary/30 hover:text-primary opacity-0 group-hover:opacity-100 transition-all" onclick={() => copyToClipboard(title)} title="Copy Title">
                        <span class="material-symbols-outlined text-sm">content_copy</span>
                    </button>
                </div>
                
                <div class="flex flex-wrap items-center gap-2 text-[9px] font-mono uppercase tracking-[0.14em] text-primary/70 mt-1">
                    <span class="inline-flex items-center gap-1 rounded-full border border-primary/15 bg-primary/5 px-2 py-1 text-primary/85">
                        <span class="opacity-55">Type</span>
                        <span class="font-bold">{session.session_type}</span>
                    </span>
                    {#if session.task_id}
                        <button class="inline-flex items-center gap-1 rounded-full border border-primary/15 bg-primary/5 px-2 py-1 text-primary/85 hover:border-primary/40 transition-colors" onclick={() => copyToClipboard(session.task_id)}>
                            <span class="opacity-55">Task</span>
                            <span class="font-bold">{session.task_id.slice(-8)}</span>
                        </button>
                    {/if}
                </div>
            </div>
        </div>

        <!-- Scrollable content -->
        <div class="flex-1 overflow-y-auto custom-scrollbar">
            <div class="flex flex-col p-3 space-y-4">
                
                <!-- Environment details -->
                <section class="rounded-sm border border-primary/15 bg-black/15 p-3">
                    <div class="text-[9px] font-bold uppercase tracking-[0.18em] text-primary/45 border-b border-primary/10 pb-2 mb-2">Environment</div>
                    <div class="grid grid-cols-2 gap-2">
                        <div class="flex flex-col">
                            <span class="text-[8px] uppercase tracking-[0.1em] text-primary/40">Provider</span>
                            <span class="text-[10px] font-bold text-primary/80">{session.provider || 'unknown'}</span>
                        </div>
                        <div class="flex flex-col">
                            <span class="text-[8px] uppercase tracking-[0.1em] text-primary/40">Host</span>
                            <span class="text-[10px] font-bold text-primary/80">{session.hostname || session.machine || 'unknown'}</span>
                        </div>
                        <div class="flex flex-col">
                            <span class="text-[8px] uppercase tracking-[0.1em] text-primary/40">Started</span>
                            <span class="text-[10px] font-bold text-primary/80">{new Date(session.started_at).toLocaleString()}</span>
                        </div>
                        <div class="flex flex-col">
                            <span class="text-[8px] uppercase tracking-[0.1em] text-primary/40">Duration</span>
                            <span class="text-[10px] font-bold text-primary/80">{session.duration_min ? `${session.duration_min.toFixed(1)} min` : 'Unknown'}</span>
                        </div>
                    </div>
                </section>

                {#if metrics}
                    <!-- Efficiency -->
                    <section class="rounded-sm border border-primary/15 bg-black/15 p-3">
                        <div class="text-[9px] font-bold uppercase tracking-[0.18em] text-primary/45 border-b border-primary/10 pb-2 mb-2">Efficiency</div>
                        <div class="grid grid-cols-2 gap-2">
                            <div class="flex flex-col">
                                <span class="text-[8px] uppercase tracking-[0.1em] text-primary/40">Cache Hit Rate</span>
                                <span class="text-sm font-black text-[#42d4f4]">{metrics.efficiency?.cache_hit_rate ? Math.round(metrics.efficiency.cache_hit_rate * 100) : 0}%</span>
                            </div>
                            <div class="flex flex-col">
                                <span class="text-[8px] uppercase tracking-[0.1em] text-primary/40">Processing Speed</span>
                                <span class="text-sm font-black text-primary/80">{formatNumber(Math.round(metrics.efficiency?.tokens_per_minute || 0))} <span class="text-[9px] font-normal">TPM</span></span>
                            </div>
                        </div>
                    </section>

                    <!-- Token Usage -->
                    <section class="rounded-sm border border-primary/15 bg-black/15 p-3">
                        <div class="text-[9px] font-bold uppercase tracking-[0.18em] text-primary/45 border-b border-primary/10 pb-2 mb-2">Token Usage</div>
                        <div class="grid grid-cols-2 gap-y-3 gap-x-2">
                            <div class="flex flex-col">
                                <span class="text-[8px] uppercase tracking-[0.1em] text-primary/40 flex items-center gap-1"><span class="material-symbols-outlined text-[10px] text-primary/50">login</span> Input Tokens</span>
                                <span class="text-[11px] font-bold text-primary/80">{formatNumber(metrics.totals?.input_tokens)}</span>
                            </div>
                            <div class="flex flex-col">
                                <span class="text-[8px] uppercase tracking-[0.1em] text-primary/40 flex items-center gap-1"><span class="material-symbols-outlined text-[10px] text-primary/50">logout</span> Output Tokens</span>
                                <span class="text-[11px] font-bold text-primary/80">{formatNumber(metrics.totals?.output_tokens)}</span>
                            </div>
                            <div class="flex flex-col">
                                <span class="text-[8px] uppercase tracking-[0.1em] text-primary/40 flex items-center gap-1"><span class="material-symbols-outlined text-[10px] text-green-500/50">save</span> Cache Created</span>
                                <span class="text-[11px] font-bold text-primary/80">{formatNumber(metrics.totals?.cache_create_tokens)}</span>
                            </div>
                            <div class="flex flex-col">
                                <span class="text-[8px] uppercase tracking-[0.1em] text-primary/40 flex items-center gap-1"><span class="material-symbols-outlined text-[10px] text-green-500/80">bolt</span> Cache Reads</span>
                                <span class="text-[11px] font-bold text-[#42d4f4]">{formatNumber(metrics.totals?.cache_read_tokens)}</span>
                            </div>
                        </div>
                    </section>
                {:else}
                    <section class="rounded-sm border border-primary/15 bg-black/15 p-3 text-center">
                        <span class="text-[10px] text-primary/40 italic">No token telemetry available for this session.</span>
                    </section>
                {/if}

                {#if session.accomplishments?.length > 0 || session.friction_points?.length > 0}
                    <section class="rounded-sm border border-primary/15 bg-black/15 p-3 space-y-3">
                        {#if session.accomplishments?.length > 0}
                            <div>
                                <div class="text-[9px] font-bold uppercase tracking-[0.18em] text-green-500/70 border-b border-primary/10 pb-1 mb-2">Accomplishments</div>
                                <ul class="space-y-1">
                                    {#each session.accomplishments as acc}
                                        <li class="text-[10px] text-primary/80 flex gap-2"><span class="text-green-500 shrink-0">›</span> <span>{@html formatText(acc)}</span></li>
                                    {/each}
                                </ul>
                            </div>
                        {/if}
                        {#if session.friction_points?.length > 0}
                            <div>
                                <div class="text-[9px] font-bold uppercase tracking-[0.18em] text-yellow-500/70 border-b border-primary/10 pb-1 mb-2">Friction Points</div>
                                <ul class="space-y-1">
                                    {#each session.friction_points as fp}
                                        <li class="text-[10px] text-primary/80 flex gap-2"><span class="text-yellow-500 shrink-0">!</span> <span>{@html formatText(fp)}</span></li>
                                    {/each}
                                </ul>
                            </div>
                        {/if}
                    </section>
                {/if}
            </div>
        </div>
    </div>
{/if}
