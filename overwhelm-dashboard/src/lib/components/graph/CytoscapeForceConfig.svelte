<script lang="ts">
    import { viewSettings } from "../../stores/viewSettings";

    export let showConfig: boolean = true;
    export let onClose: () => void = () => {};
</script>

{#if showConfig}
    <div class="config-panel graph-control-panel">
        <div class="flex items-center justify-between border-b border-primary/10 pb-2">
            <h3 class="text-[10px] font-bold tracking-[0.2em] text-primary/80 uppercase">
                Force_Simulation_Config
            </h3>
            <button class="graph-control-icon-button" on:click={onClose}>
                <span class="material-symbols-outlined text-sm">close</span>
            </button>
        </div>

        <div class="space-y-3 pt-1 border-t border-primary/5">
            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Dist_Intra-Group</span>
                    <span>{$viewSettings.colaLinkDistIntraParent}</span>
                </div>
                <input type="range" min="10" max="1000" step="10" bind:value={$viewSettings.colaLinkDistIntraParent} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>
            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Weight_Intra-Group</span>
                    <span>{$viewSettings.colaLinkWeightIntraParent.toFixed(1)}</span>
                </div>
                <input type="range" min="0.1" max="1.0" step="0.05" bind:value={$viewSettings.colaLinkWeightIntraParent} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>

            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Dist_Parent_(Inter-Group)</span>
                    <span>{$viewSettings.colaLinkDistInterParent}</span>
                </div>
                <input type="range" min="10" max="1000" step="10" bind:value={$viewSettings.colaLinkDistInterParent} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>
            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Weight_Parent_(Inter-Group)</span>
                    <span>{$viewSettings.colaLinkWeightInterParent.toFixed(1)}</span>
                </div>
                <input type="range" min="0.1" max="1.0" step="0.05" bind:value={$viewSettings.colaLinkWeightInterParent} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>

            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Dist_Depends_On</span>
                    <span>{$viewSettings.colaLinkDistDependsOn}</span>
                </div>
                <input type="range" min="10" max="1000" step="10" bind:value={$viewSettings.colaLinkDistDependsOn} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>
            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Weight_Depends_On</span>
                    <span>{$viewSettings.colaLinkWeightDependsOn.toFixed(1)}</span>
                </div>
                <input type="range" min="0.1" max="1.0" step="0.05" bind:value={$viewSettings.colaLinkWeightDependsOn} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>

            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Dist_Soft_Depends</span>
                    <span>{$viewSettings.colaLinkDistSoftDependsOn}</span>
                </div>
                <input type="range" min="10" max="1000" step="10" bind:value={$viewSettings.colaLinkDistSoftDependsOn} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>
            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Weight_Soft_Depends</span>
                    <span>{$viewSettings.colaLinkWeightSoftDependsOn.toFixed(1)}</span>
                </div>
                <input type="range" min="0.1" max="1.0" step="0.05" bind:value={$viewSettings.colaLinkWeightSoftDependsOn} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>

            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Dist_Contributes_To</span>
                    <span>{$viewSettings.colaLinkDistContributesTo}</span>
                </div>
                <input type="range" min="10" max="1000" step="10" bind:value={$viewSettings.colaLinkDistContributesTo} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>
            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Weight_Contributes_To</span>
                    <span>{$viewSettings.colaLinkWeightContributesTo.toFixed(1)}</span>
                </div>
                <input type="range" min="0.1" max="1.0" step="0.05" bind:value={$viewSettings.colaLinkWeightContributesTo} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>

            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Dist_Similar_To</span>
                    <span>{$viewSettings.colaLinkDistSimilarTo}</span>
                </div>
                <input type="range" min="10" max="1000" step="10" bind:value={$viewSettings.colaLinkDistSimilarTo} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>
            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Weight_Similar_To</span>
                    <span>{$viewSettings.colaLinkWeightSimilarTo.toFixed(1)}</span>
                </div>
                <input type="range" min="0.1" max="1.0" step="0.05" bind:value={$viewSettings.colaLinkWeightSimilarTo} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>

            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Dist_References</span>
                    <span>{$viewSettings.colaLinkDistRef}</span>
                </div>
                <input type="range" min="10" max="1000" step="10" bind:value={$viewSettings.colaLinkDistRef} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>
            <div class="space-y-1">
                <div class="flex justify-between text-[9px] text-primary/50 uppercase">
                    <span>Weight_References</span>
                    <span>{$viewSettings.colaLinkWeightRef.toFixed(1)}</span>
                </div>
                <input type="range" min="0.1" max="1.0" step="0.05" bind:value={$viewSettings.colaLinkWeightRef} class="w-full h-1 bg-primary/10 rounded-lg appearance-none cursor-pointer accent-primary" />
            </div>

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
        </div>
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
        background: rgba(10, 14, 20, 0.95);
        backdrop-filter: blur(12px);
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
