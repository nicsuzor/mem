<script lang="ts">
    import { viewSettings, getLayoutFromViewSettings } from "../../stores/viewSettings";
    import { EDGE_TYPES } from "../../data/taxonomy";

    $: layout = getLayoutFromViewSettings($viewSettings);
    $: isForce = layout === "force" || layout === "sfdp" || layout === "metro" || layout === "force_v2";
    $: isGroups = layout === "groups";
    $: isMetro = layout === "metro";
    $: isCircle = layout === "circle_pack";
    $: isArc = layout === "arc";
    $: isTreemap = layout === "treemap";

    // Show live controls when the layout has tunable parameters
    $: hasLiveControls = isForce || isGroups || isCircle || isArc || isTreemap;

    const WEIGHT_MODES = [
        { value: 'sqrt', label: '√ FOCUS' },
        { value: 'priority', label: 'PRIORITY' },
        { value: 'focus-bucket', label: 'FOCUS BUCKET' },
        { value: 'equal', label: 'EQUAL' },
    ] as const;

    const METRO_ALGORITHMS = [
        { value: 'force', label: 'Force (Organic)' },
        { value: 'elk', label: 'ELK (Orthogonal Grid)' },
        { value: 'cola', label: 'Cola (Constraint-based)' },
    ] as const;
</script>

{#if hasLiveControls}
    <div class="graph-dock graph-dock-top-right font-mono">
        {#if $viewSettings.showGraphConfig}
            <div class="config-panel graph-control-panel">
                <div class="flex items-center justify-between border-b border-primary/10 pb-2">
                    <h3 class="text-[10px] font-bold tracking-[0.2em] text-primary/80 uppercase">
                        {#if isMetro}Metro_Config{:else if isGroups}Groups_Config{:else if isForce}Simulation_Config{:else if isCircle}Circle_Pack_Config{:else if isArc}Arc_Diagram_Config{:else if isTreemap}Treemap_Config{/if}
                    </h3>
                    <button class="graph-control-icon-button" onclick={() => viewSettings.update(s => ({ ...s, showGraphConfig: false }))}>
                        <span class="material-symbols-outlined text-sm">close</span>
                    </button>
                </div>

                {#if isForce || isGroups}
                    <div class="space-y-3 pt-1 border-t border-primary/5">
                        {#each Object.values(EDGE_TYPES) as edge}
                            <div class="space-y-1">
                                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                                    <span>Dist_{edge.displayName.replace(/ /g, '_')}</span>
                                    <span>{$viewSettings[edge.distKey]}</span>
                                </div>
                                <input type="range" min="10" max="1000" step="10" bind:value={$viewSettings[edge.distKey]} class="slider-{edge.id.split('_')[0]} w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer" />
                            </div>
                            <div class="space-y-1">
                                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                                    <span>Weight_{edge.displayName.replace(/ /g, '_')}</span>
                                    <span>{Number($viewSettings[edge.weightKey]).toFixed(1)}</span>
                                </div>
                                <input type="range" min="0.1" max="1.0" step="0.05" bind:value={$viewSettings[edge.weightKey]} class="slider-{edge.id.split('_')[0]} w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer" />
                            </div>
                        {/each}

                        <div class="space-y-1 pt-2 border-t border-primary/5">
                            <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                                <span>Convergence</span>
                                <span>{$viewSettings.colaConvergence}</span>
                            </div>
                            <input type="range" min="0.001" max="0.09" step="0.001" bind:value={$viewSettings.colaConvergence} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
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
                            <span class="text-[10px] text-primary/60 uppercase">Handle_Disconnected</span>
                            <input type="checkbox" bind:checked={$viewSettings.colaHandleDisconnected} class="text-primary bg-black border-primary/30 focus:ring-primary rounded-sm cursor-pointer" />
                        </label>
                        {#if !isGroups}
                        <label class="flex items-center justify-between cursor-pointer mt-2 pt-2 border-t border-primary/5">
                            <span class="text-[10px] text-primary/80 uppercase font-bold text-accent">Enable Epic Grouping</span>
                            <input type="checkbox" bind:checked={$viewSettings.enableEpicGrouping} class="text-accent bg-black border-accent/50 focus:ring-accent rounded-sm cursor-pointer" />
                        </label>
                        {/if}
                    </div>

                {/if}

                {#if isMetro}
                    <div class="space-y-2 pt-2 border-t border-primary/5">
                        <span class="text-[9px] text-primary/50 uppercase font-bold">Layout_Algorithm</span>
                        <div class="flex flex-col gap-1">
                            {#each METRO_ALGORITHMS as algo}
                                <button
                                    class="px-2 py-1.5 text-[9px] font-bold uppercase tracking-wider border rounded-sm transition-all cursor-pointer text-left
                                        {$viewSettings.metroAlgorithm === algo.value
                                            ? 'bg-primary text-background border-primary'
                                            : 'bg-primary/5 text-primary/60 border-primary/20 hover:border-primary/40'}"
                                    onclick={() => viewSettings.update(s => ({ ...s, metroAlgorithm: algo.value }))}
                                >
                                    {algo.label}
                                </button>
                            {/each}
                        </div>
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
            class="config-toggle graph-control-button {$viewSettings.showGraphConfig ? 'graph-control-button-active' : ''}"
            onclick={() => viewSettings.update(s => ({ ...s, showGraphConfig: !s.showGraphConfig }))}
        >
            <span class="material-symbols-outlined text-primary transition-transform duration-300" class:rotate-90={$viewSettings.showGraphConfig}>
                {#if isMetro}subway{:else if isGroups}hub{:else if isForce}settings_input_component{:else if isCircle}radio_button_checked{:else if isArc}architecture{:else if isTreemap}grid_view{/if}
            </span>
            <span class="text-[10px] font-black uppercase tracking-widest text-primary">
                {#if isMetro}Metro{:else if isGroups}Groups{:else if isForce}Force{:else if isCircle}Pack{:else if isArc}Arc{:else if isTreemap}Treemap{/if}_Config
            </span>
        </button>
    </div>
{/if}

<style>
    .config-panel {
        border-radius: 12px;
        padding: 16px;
        display: flex;
        flex-direction: column;
        gap: 16px;
        min-width: 280px;
        max-width: min(24rem, calc(100vw - 2rem));
        max-height: 80vh;
        overflow-y: auto;
        box-shadow: 0 8px 32px rgba(0, 0, 0, 0.6);
    }

    .config-toggle {
        cursor: pointer;
        box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
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
    .slider-intra::-webkit-slider-thumb { background: #3b82f6 !important; box-shadow: 0 0 8px #3b82f6 !important; }
    .slider-inter::-webkit-slider-thumb { background: #facc15 !important; box-shadow: 0 0 8px #facc15 !important; }
    .slider-depends::-webkit-slider-thumb { background: #ef4444 !important; box-shadow: 0 0 8px #ef4444 !important; }
    .slider-soft::-webkit-slider-thumb { background: #9ca3af !important; box-shadow: 0 0 8px #9ca3af !important; }
    .slider-contrib::-webkit-slider-thumb { background: #10b981 !important; box-shadow: 0 0 8px #10b981 !important; }
    .slider-similar::-webkit-slider-thumb { background: #c4b5fd !important; box-shadow: 0 0 8px #c4b5fd !important; }
    .slider-ref::-webkit-slider-thumb { background: #a3a3a3 !important; box-shadow: 0 0 8px #a3a3a3 !important; }
</style>
