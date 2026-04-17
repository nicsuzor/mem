<script lang="ts">
    import { filters, cycleVisibility, type VisibilityState } from '../../stores/filters';
    import { graphData } from '../../stores/graph';
    import { viewSettings } from '../../stores/viewSettings';
    import { PRIORITIES } from '../../data/constants';
    import { projectColor } from '../../data/projectUtils';

    let showAllProjects = false;
    const MAX_VISIBLE_PROJECTS = 6;

    // Status = fill color (muted defaults, saturated for attention)
    const statusGroups = [
        { key: 'statusActive', label: 'ACTIVE', color: '#2C4A88' },
        { key: 'statusBlocked', label: 'BLOCKED', color: '#6B3A3A' },
        { key: 'statusCompleted', label: 'COMPLETED', color: '#1E1E24' },
    ] as const;

    // Priority = border color — now interactive (click to cycle)
    const priorityItems = PRIORITIES.map(p => ({
        key: `priority${p.value}`,
        value: p.value,
        label: `P${p.value} ${p.label}`,
        color: p.color,
    }));

    const edgeTypes = [
        { key: 'edgeParent', label: 'PARENT', color: '#facc15', dash: false },
        { key: 'edgeDependencies', label: 'DEPENDENCIES', color: '#ef4444', dash: false },
        { key: 'edgeReferences', label: 'REFERENCES', color: '#a3a3a3', dash: true },
    ] as const;

    function cycleFilter(key: string) {
        filters.update(f => ({
            ...f,
            [key]: cycleVisibility(f[key as keyof typeof f] as VisibilityState),
        }));
    }

    function toggleProject(project: string) {
        filters.update(f => {
            const hidden = f.hiddenProjects || [];
            if (hidden.includes(project)) {
                return { ...f, hiddenProjects: hidden.filter(p => p !== project) };
            } else {
                return { ...f, hiddenProjects: [...hidden, project] };
            }
        });
    }

    function toggleAllProjects() {
        filters.update(f => ({ ...f, hiddenProjects: [] }));
    }

    function edgeOpacityForLegend(vis: VisibilityState): number {
        if (vis === 'bright') return 1;
        if (vis === 'half') return 0.4;
        return 0.1;
    }

    function stateLabel(vis: VisibilityState): string {
        if (vis === 'bright') return '●';
        if (vis === 'half') return '◐';
        return '○';
    }

    $: availableProjects = $graphData
        ? Array.from(new Set($graphData.nodes.map((n) => n.project).filter((p): p is string => !!p))).sort()
        : [];

    $: visibleProjects = showAllProjects
        ? availableProjects
        : availableProjects.slice(0, MAX_VISIBLE_PROJECTS);

    $: hasOverflow = availableProjects.length > MAX_VISIBLE_PROJECTS;
</script>

{#if $viewSettings.showLegend}
    <div class="graph-dock graph-dock-bottom-left">
        <div class="legend graph-control-panel" role="complementary" aria-label="Filters & Legend">
            <div class="legend-header">
                <span class="legend-title">VISIBILITY</span>
                <span class="component-name">filter-panel</span>
                <button class="legend-close graph-control-icon-button" on:click={() => viewSettings.update(s => ({ ...s, showLegend: false }))}>
                    <span class="material-symbols-outlined" style="font-size: 14px;">close</span>
                </button>
            </div>

            <!-- Status filters (click to cycle: bright → half → hidden) -->
            <div class="legend-section">
                <span class="legend-section-title">STATUS</span>
                {#each statusGroups as group}
                    {@const vis = $filters[group.key as keyof typeof $filters] as VisibilityState}
                    <button
                        class="legend-item"
                        class:dimmed={vis === 'hidden'}
                        on:click={() => cycleFilter(group.key)}
                        title="Click to cycle: bright → half → hidden"
                    >
                        <div class="legend-box" style="background:{group.color}; opacity:{edgeOpacityForLegend(vis)};"></div>
                        <span class="legend-label">{group.label}</span>
                        <span class="edge-state">{stateLabel(vis)}</span>
                    </button>
                {/each}
            </div>

            <!-- Priority filter (click to cycle: bright → half → hidden) -->
            <div class="legend-section">
                <span class="legend-section-title">PRIORITY</span>
                {#each priorityItems as p}
                    {@const vis = $filters[p.key as keyof typeof $filters] as VisibilityState}
                    <button
                        class="legend-item"
                        class:dimmed={vis === 'hidden'}
                        on:click={() => cycleFilter(p.key)}
                        title="Click to cycle: bright → half → hidden"
                    >
                        <div class="legend-box" style="background:transparent; border: 2px solid {p.color}; opacity:{edgeOpacityForLegend(vis)};"></div>
                        <span class="legend-label">{p.label}</span>
                        <span class="edge-state">{stateLabel(vis)}</span>
                    </button>
                {/each}
            </div>

            <!-- Edge visibility (click to cycle: bright → half → hidden) -->
            <div class="legend-section">
                <span class="legend-section-title">EDGES</span>
                {#each edgeTypes as edge}
                    {@const vis = $filters[edge.key as keyof typeof $filters] as VisibilityState}
                    <button
                        class="legend-item"
                        class:dimmed={vis === 'hidden'}
                        on:click={() => cycleFilter(edge.key)}
                        title="Click to cycle: bright → half → hidden"
                    >
                        <div class="legend-line" style="background:{edge.color}; opacity:{edgeOpacityForLegend(vis)};"
                            class:dashed={edge.dash}></div>
                        <span class="legend-label">{edge.label}</span>
                        <span class="edge-state">{stateLabel(vis)}</span>
                    </button>
                {/each}
            </div>

            <!-- Project filter with color swatches -->
            <div class="legend-section">
                <span class="legend-section-title">PROJECTS (CLICK TO TOGGLE)</span>
                <button
                    class="legend-item"
                    class:dimmed={($filters.hiddenProjects?.length ?? 0) > 0}
                    on:click={toggleAllProjects}
                >
                    <div class="legend-box" style="background: #666; border-radius: 50%;"></div>
                    <span class="legend-label">ALL PROJECTS</span>
                    {#if ($filters.hiddenProjects?.length ?? 0) === 0}
                        <span class="filter-badge">ON</span>
                    {/if}
                </button>
                <div class="project-list" class:expanded={showAllProjects}>
                    {#each visibleProjects as proj}
                        <button
                            class="legend-item"
                            class:dimmed={$filters.hiddenProjects?.includes(proj)}
                            on:click={() => toggleProject(proj)}
                            title="Toggle {proj}"
                        >
                            <div class="legend-box project-swatch" style="background: {projectColor(proj)};"></div>
                            <span class="legend-label">{(proj || '').toUpperCase()}</span>
                            {#if !($filters.hiddenProjects?.includes(proj))}
                                <span class="filter-badge">ON</span>
                            {/if}
                        </button>
                    {/each}
                </div>
                {#if hasOverflow}
                    <button
                        class="legend-item overflow-toggle"
                        on:click={() => showAllProjects = !showAllProjects}
                    >
                        <span class="legend-label overflow-label">
                            {showAllProjects ? '▲ LESS' : `▼ +${availableProjects.length - MAX_VISIBLE_PROJECTS} MORE`}
                        </span>
                    </button>
                {/if}
            </div>
        </div>
    </div>
{:else}
    <div class="graph-dock graph-dock-bottom-left">
        <button class="legend-toggle graph-control-button graph-control-button-active" on:click={() => viewSettings.update(s => ({ ...s, showLegend: true }))}>
            <span class="material-symbols-outlined" style="font-size: 14px;">visibility</span>
            <span>Legend</span>
        </button>
    </div>
{/if}

<style>
    .legend {
        border-radius: 12px;
        padding: 10px 12px;
        font-size: 10px;
        color: var(--color-primary);
        display: flex;
        flex-direction: column;
        gap: 8px;
        font-family: var(--font-mono);
        min-width: 180px;
        max-height: calc(100vh - 120px);
        overflow-y: auto;
    }

    .legend-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        border-bottom: 1px solid color-mix(in srgb, var(--color-primary) 10%, transparent);
        padding-bottom: 6px;
        gap: 6px;
    }

    .legend-title {
        font-size: 9px;
        font-weight: 900;
        letter-spacing: 0.2em;
        color: color-mix(in srgb, var(--color-primary) 80%, transparent);
        text-transform: uppercase;
    }

    .component-name {
        font-size: 7px;
        color: color-mix(in srgb, var(--color-primary) 20%, transparent);
        letter-spacing: 0.1em;
        font-style: italic;
    }

    .legend-close {
        cursor: pointer;
        line-height: 1;
    }

    .legend-section {
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    .legend-section-title {
        font-size: 9px;
        font-weight: 900;
        letter-spacing: 0.15em;
        color: color-mix(in srgb, var(--color-primary) 40%, transparent);
        text-transform: uppercase;
        padding-top: 4px;
        border-top: 1px solid color-mix(in srgb, var(--color-primary) 10%, transparent);
    }

    .legend-item {
        display: flex;
        align-items: center;
        gap: 8px;
        cursor: pointer;
        background: none;
        border: none;
        padding: 3px 4px;
        border-radius: 4px;
        transition: background 0.15s;
        text-align: left;
    }
    .legend-item:hover { background: color-mix(in srgb, var(--color-primary) 10%, transparent); }

    .legend-item.dimmed .legend-label {
        opacity: 0.35;
        text-decoration: line-through;
    }

    .legend-label {
        font-size: 10px;
        font-weight: 700;
        color: var(--color-primary);
        transition: opacity 0.15s;
    }

    .legend-box {
        width: 12px;
        height: 12px;
        border-radius: 2px;
        flex-shrink: 0;
        transition: opacity 0.15s;
    }

    .project-swatch {
        border-radius: 50%;
        width: 10px;
        height: 10px;
    }

    .legend-line {
        width: 16px;
        height: 3px;
        border-radius: 1.5px;
        flex-shrink: 0;
        transition: opacity 0.15s;
    }
    .legend-line.dashed {
        background: transparent;
        border-top: 3px dashed;
        border-color: inherit;
        height: 0;
    }

    .edge-state {
        margin-left: auto;
        font-size: 10px;
        opacity: 0.6;
    }

    .filter-badge {
        margin-left: auto;
        font-size: 8px;
        font-weight: 900;
        color: var(--color-primary);
        background: color-mix(in srgb, var(--color-primary) 15%, transparent);
        padding: 1px 4px;
        border-radius: 2px;
        letter-spacing: 0.1em;
    }

    .project-list {
        display: flex;
        flex-direction: column;
        gap: 2px;
        max-height: 200px;
        overflow-y: auto;
    }
    .project-list.expanded {
        max-height: 400px;
    }

    .overflow-toggle {
        justify-content: center;
        padding: 2px 4px;
    }
    .overflow-label {
        font-size: 9px;
        opacity: 0.5;
    }

    .legend-toggle {
        cursor: pointer;
    }
</style>
