<script lang="ts">
    import { viewSettings, getLayoutFromViewSettings } from "../../stores/viewSettings";

    $: layout = getLayoutFromViewSettings($viewSettings);
    $: isForce = layout === "force";
    $: isCircle = layout === "circle_pack";
    $: isArc = layout === "arc";
    $: isTreemap = layout === "treemap";

    // Show live controls when the layout has tunable parameters
    $: hasLiveControls = isForce || isCircle || isArc;

    let expanded = false;
</script>

{#if hasLiveControls}
    <div class="absolute bottom-4 left-[200px] z-30 flex flex-col items-start gap-2 font-mono">
        {#if expanded}
            <div class="config-panel">
                <div class="flex items-center justify-between border-b border-primary/10 pb-2">
                    <h3 class="text-[10px] font-bold tracking-[0.2em] text-primary/80 uppercase">
                        {#if isForce}Simulation_Config{:else if isCircle}Circle_Pack_Config{:else if isArc}Arc_Diagram_Config{/if}
                    </h3>
                    <button class="text-primary/40 hover:text-primary transition-colors cursor-pointer" onclick={() => expanded = false}>
                        <span class="material-symbols-outlined text-sm">close</span>
                    </button>
                </div>

                {#if isForce}
                    <label class="flex items-center justify-between cursor-pointer group">
                        <span class="text-[10px] font-bold text-primary/60 uppercase">Live_Simulation</span>
                        <input type="checkbox" bind:checked={$viewSettings.liveSimulation} class="text-primary bg-black border-primary/30 focus:ring-primary rounded-sm cursor-pointer" />
                    </label>

                    {#if $viewSettings.liveSimulation}
                        <div class="space-y-3 pt-1 border-t border-primary/5">
                            <div class="space-y-1">
                                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                                    <span>Repulsion</span>
                                    <span>{$viewSettings.chargeStrength.toFixed(1)}x</span>
                                </div>
                                <input type="range" min="0.1" max="3.0" step="0.1" bind:value={$viewSettings.chargeStrength} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
                            </div>
                            <div class="space-y-1">
                                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                                    <span>Link_Distance</span>
                                    <span>{$viewSettings.linkDistance.toFixed(1)}x</span>
                                </div>
                                <input type="range" min="0.1" max="3.0" step="0.1" bind:value={$viewSettings.linkDistance} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
                            </div>
                            <div class="space-y-1">
                                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                                    <span>Gravity</span>
                                    <span>{$viewSettings.gravity.toFixed(2)}</span>
                                </div>
                                <input type="range" min="0.01" max="0.5" step="0.01" bind:value={$viewSettings.gravity} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
                            </div>
                        </div>
                    {/if}

                    <div class="space-y-1 pt-1 border-t border-primary/5">
                        <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                            <span>Max_Visible_Nodes</span>
                            <span>{$viewSettings.topNLeaves}</span>
                        </div>
                        <input type="range" min="10" max="2000" step="10" bind:value={$viewSettings.topNLeaves} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
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
                    <div class="space-y-1">
                        <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                            <span>Vertical_Scale</span>
                            <span>{$viewSettings.arcVerticalSpacing.toFixed(1)}x</span>
                        </div>
                        <input type="range" min="0.5" max="3.0" step="0.1" bind:value={$viewSettings.arcVerticalSpacing} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
                    </div>
                {/if}
            </div>
        {/if}

        <button
            class="config-toggle"
            onclick={() => expanded = !expanded}
        >
            <span class="material-symbols-outlined text-primary group-hover:rotate-90 transition-transform duration-300">
                {#if isForce}settings_input_component{:else if isCircle}radio_button_checked{:else if isArc}architecture{/if}
            </span>
            <span class="text-[10px] font-black uppercase tracking-widest text-primary">
                {#if isForce}Force{:else if isCircle}Pack{:else if isArc}Arc{/if}_Config
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
