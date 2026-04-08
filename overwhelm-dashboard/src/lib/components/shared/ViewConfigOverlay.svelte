<script lang="ts">
    import { viewSettings, getLayoutFromViewSettings } from "../../stores/viewSettings";

    $: layout = getLayoutFromViewSettings($viewSettings);
    $: isForce = layout === "force";
    $: isCircle = layout === "circle_pack";
    $: isArc = layout === "arc";
    $: isTreemap = layout === "treemap";

    // Show live controls when the layout has tunable parameters
    $: hasLiveControls = isForce || isCircle || isArc || isTreemap;

    const WEIGHT_MODES = [
        { value: 'sqrt', label: '√ DW' },
        { value: 'priority', label: 'PRIORITY' },
        { value: 'dw-bucket', label: 'DW BUCKET' },
        { value: 'equal', label: 'EQUAL' },
    ] as const;

    let expanded = false;
</script>

{#if hasLiveControls}
    <div class="absolute bottom-4 left-[200px] z-30 flex flex-col items-start gap-2 font-mono">
        {#if expanded}
            <div class="config-panel">
                <div class="flex items-center justify-between border-b border-primary/10 pb-2">
                    <h3 class="text-[10px] font-bold tracking-[0.2em] text-primary/80 uppercase">
                        {#if isForce}Simulation_Config{:else if isCircle}Circle_Pack_Config{:else if isArc}Arc_Diagram_Config{:else if isTreemap}Treemap_Config{/if}
                    </h3>
                    <button class="text-primary/40 hover:text-primary transition-colors cursor-pointer" onclick={() => expanded = false}>
                        <span class="material-symbols-outlined text-sm">close</span>
                    </button>
                </div>

                {#if isForce}
                    <div class="space-y-3 pt-1 border-t border-primary/5">
                        <div class="space-y-1">
                            <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                                <span>Link_Length</span>
                                <span>{$viewSettings.colaLinkLength}</span>
                            </div>
                            <input type="range" min="30" max="400" step="10" bind:value={$viewSettings.colaLinkLength} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
                        </div>
                        <div class="space-y-1">
                            <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                                <span>Flow_Separation</span>
                                <span>{$viewSettings.colaFlowSep}</span>
                            </div>
                            <input type="range" min="10" max="200" step="5" bind:value={$viewSettings.colaFlowSep} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
                        </div>
                        <div class="space-y-1">
                            <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                                <span>Group_Padding</span>
                                <span>{$viewSettings.colaGroupPadding}</span>
                            </div>
                            <input type="range" min="5" max="80" step="5" bind:value={$viewSettings.colaGroupPadding} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
                        </div>
                    </div>

                    <div class="space-y-1 pt-1 border-t border-primary/5">
                        <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                            <span>Max_Visible_Nodes</span>
                            <span>{$viewSettings.topNLeaves}</span>
                        </div>
                        <input type="range" min="10" max="2000" step="10" bind:value={$viewSettings.topNLeaves} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
                    </div>

                    <div class="space-y-2 pt-2 border-t border-primary/5">
                        <span class="text-[9px] text-primary/50 uppercase font-bold">Cola_Constraints</span>
                        <label class="flex items-center justify-between cursor-pointer">
                            <span class="text-[10px] text-primary/60 uppercase">Avoid_Overlaps</span>
                            <input type="checkbox" bind:checked={$viewSettings.colaAvoidOverlaps} class="text-primary bg-black border-primary/30 focus:ring-primary rounded-sm cursor-pointer" />
                        </label>
                        <label class="flex items-center justify-between cursor-pointer">
                            <span class="text-[10px] text-primary/60 uppercase">Groups</span>
                            <input type="checkbox" bind:checked={$viewSettings.colaGroups} class="text-primary bg-black border-primary/30 focus:ring-primary rounded-sm cursor-pointer" />
                        </label>
                        <label class="flex items-center justify-between cursor-pointer">
                            <span class="text-[10px] text-primary/60 uppercase">Links</span>
                            <input type="checkbox" bind:checked={$viewSettings.colaLinks} class="text-primary bg-black border-primary/30 focus:ring-primary rounded-sm cursor-pointer" />
                        </label>
                        <label class="flex items-center justify-between cursor-pointer">
                            <span class="text-[10px] text-primary/60 uppercase">Handle_Disconnected</span>
                            <input type="checkbox" bind:checked={$viewSettings.colaHandleDisconnected} class="text-primary bg-black border-primary/30 focus:ring-primary rounded-sm cursor-pointer" />
                        </label>
                    </div>

                {/if}

                {#if isCircle}
                    <div class="space-y-1">
                        <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                            <span>Rollup_Threshold</span>
                            <span>{$viewSettings.circleRollupThreshold}px</span>
                        </div>
                        <input type="range" min="5" max="50" step="1" bind:value={$viewSettings.circleRollupThreshold} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
                    </div>
                {/if}

                {#if isArc}
                    <label class="flex items-center justify-between cursor-pointer group">
                        <span class="text-[10px] font-bold text-primary/60 uppercase">Focused_Only</span>
                        <input type="checkbox" bind:checked={$viewSettings.arcFocusedOnly} class="text-primary bg-black border-primary/30 focus:ring-primary rounded-sm cursor-pointer" />
                    </label>
                    <div class="space-y-1">
                        <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                            <span>Vertical_Scale</span>
                            <span>{$viewSettings.arcVerticalSpacing.toFixed(1)}x</span>
                        </div>
                        <input type="range" min="0.5" max="3.0" step="0.1" bind:value={$viewSettings.arcVerticalSpacing} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
                    </div>
                {/if}

                {#if isTreemap}
                    <div class="space-y-2">
                        <span class="text-[9px] text-primary/50 uppercase">Weight_Mode</span>
                        <div class="grid grid-cols-2 gap-1">
                            {#each WEIGHT_MODES as mode}
                                <button
                                    class="px-2 py-1.5 text-[9px] font-bold uppercase tracking-wider border rounded-sm transition-all cursor-pointer
                                        {$viewSettings.treemapWeightMode === mode.value
                                            ? 'bg-primary text-background border-primary'
                                            : 'bg-primary/5 text-primary/60 border-primary/20 hover:border-primary/40'}"
                                    onclick={() => viewSettings.update(s => ({ ...s, treemapWeightMode: mode.value }))}
                                >
                                    {mode.label}
                                </button>
                            {/each}
                        </div>
                    </div>
                {/if}
            </div>
        {/if}

        <button
            class="config-toggle"
            onclick={() => expanded = !expanded}
        >
            <span class="material-symbols-outlined text-primary group-hover:rotate-90 transition-transform duration-300">
                {#if isForce}settings_input_component{:else if isCircle}radio_button_checked{:else if isArc}architecture{:else if isTreemap}grid_view{/if}
            </span>
            <span class="text-[10px] font-black uppercase tracking-widest text-primary">
                {#if isForce}Force{:else if isCircle}Pack{:else if isArc}Arc{:else if isTreemap}Treemap{/if}_Config
            </span>
        </button>
    </div>
{/if}

<style>
    .config-panel {
        background: rgba(10, 10, 10, 0.92);
        border: 1px solid color-mix(in srgb, var(--color-primary) 20%, transparent);
        border-radius: 12px;
        padding: 16px;
        backdrop-filter: blur(12px);
        display: flex;
        flex-direction: column;
        gap: 16px;
        min-width: 280px;
        max-height: 80vh;
        overflow-y: auto;
        box-shadow: 0 8px 32px rgba(0, 0, 0, 0.6);
    }

    .config-toggle {
        background: rgba(10, 10, 10, 0.92);
        border: 1px solid color-mix(in srgb, var(--color-primary) 30%, transparent);
        border-radius: 8px;
        padding: 6px 12px;
        backdrop-filter: blur(12px);
        display: flex;
        align-items: center;
        gap: 8px;
        cursor: pointer;
        transition: background 0.15s;
        box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
    }
    .config-toggle:hover {
        background: color-mix(in srgb, var(--color-primary) 10%, transparent);
    }

    input[type=range]::-webkit-slider-thumb {
        -webkit-appearance: none;
        appearance: none;
        width: 12px;
        height: 12px;
        background: var(--color-primary);
        cursor: pointer;
        border-radius: 50%;
        box-shadow: 0 0 10px rgba(var(--color-primary-rgb), 0.5);
    }
</style>
