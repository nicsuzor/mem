<script lang="ts">
    import { filters } from '../../stores/filters';
    import { graphData } from '../../stores/graph';

    let showLegend = true;

    // Status = fill color (muted defaults, saturated for attention)
    const statusGroups = [
        { key: 'showActive', label: 'ACTIVE', statuses: ['active', 'inbox', 'todo', 'in_progress', 'review'], color: '#2C4A88' },
        { key: 'showBlocked', label: 'BLOCKED', statuses: ['blocked'], color: '#dc2626' },
        { key: 'showCompleted', label: 'COMPLETED', statuses: ['done', 'completed', 'cancelled'], color: '#1E1E24' },
    ] as const;

    // Priority = border color (only P0/P1 pop)
    const priorityColors = [
        { label: 'P0 CRITICAL', color: '#f59e0b', border: true },
        { label: 'P1 HIGH', color: '#d97706', border: true },
        { label: 'P2 MEDIUM', color: '#4A5568', border: true },
        { label: 'P3 LOW', color: '#3A4250', border: true },
        { label: 'P4 BACKLOG', color: '#2D3340', border: true },
    ] as const;

    const edgeTypes = [
        { key: 'showDependencies', label: 'DEPENDENCIES', color: '#ef4444', dash: false },
        { key: 'showReferences', label: 'REFERENCES', color: '#a3a3a3', dash: true },
    ] as const;

    function toggleStatus(key: string) {
        filters.update(f => ({ ...f, [key]: !f[key as keyof typeof f] }));
    }

    $: availableProjects = $graphData
        ? Array.from(new Set($graphData.nodes.map((n) => n.project).filter((p) => p))).sort()
        : [];
</script>

{#if showLegend}
    <div class="legend" role="complementary" aria-label="Filters & Legend">
        <div class="legend-header">
            <span class="legend-title">VISIBILITY</span>
            <button class="legend-close" on:click={() => showLegend = false}>
                <span class="material-symbols-outlined" style="font-size: 14px;">close</span>
            </button>
        </div>

        <!-- Status filters -->
        <div class="legend-section">
            {#each statusGroups as group}
                <button
                    class="legend-item"
                    class:dimmed={!$filters[group.key]}
                    on:click={() => toggleStatus(group.key)}
                >
                    <div class="legend-box" style="background:{group.color}; opacity:{$filters[group.key] ? 1 : 0.2}"></div>
                    <span class="legend-label">{group.label}</span>
                </button>
            {/each}
        </div>

        <!-- Priority legend (shown as border strokes) -->
        <div class="legend-section">
            <span class="legend-section-title">PRIORITY (BORDER)</span>
            {#each priorityColors as p}
                <div class="legend-item" style="cursor: default;">
                    <div class="legend-box" style="background:transparent; border: 2px solid {p.color};"></div>
                    <span class="legend-label">{p.label}</span>
                </div>
            {/each}
        </div>

        <!-- Edge filters -->
        <div class="legend-section">
            <span class="legend-section-title">EDGES</span>
            {#each edgeTypes as edge}
                <button
                    class="legend-item"
                    class:dimmed={!$filters[edge.key]}
                    on:click={() => toggleStatus(edge.key)}
                >
                    <div class="legend-line" style="background:{edge.color}; opacity:{$filters[edge.key] ? 1 : 0.2};"
                        class:dashed={edge.dash}></div>
                    <span class="legend-label">{edge.label}</span>
                </button>
            {/each}
        </div>

        <!-- Project filter -->
        <div class="legend-section">
            <span class="legend-section-title">PROJECT</span>
            <select
                class="legend-select"
                bind:value={$filters.project}
            >
                <option value="ALL">ALL</option>
                {#each availableProjects as project}
                    <option value={project}>{(project || '').toUpperCase()}</option>
                {/each}
            </select>
        </div>
    </div>
{:else}
    <button class="legend-toggle" on:click={() => showLegend = true}>
        <span class="material-symbols-outlined" style="font-size: 14px;">visibility</span>
        <span>Filters</span>
    </button>
{/if}

<style>
    .legend {
        position: absolute;
        bottom: 16px;
        left: 16px;
        z-index: 10;
        background: rgba(10, 10, 10, 0.92);
        border: 1px solid color-mix(in srgb, var(--color-primary) 20%, transparent);
        border-radius: 12px;
        padding: 10px 12px;
        font-size: 10px;
        color: var(--color-primary);
        backdrop-filter: blur(12px);
        display: flex;
        flex-direction: column;
        gap: 8px;
        font-family: var(--font-mono);
        min-width: 160px;
    }

    .legend-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        border-bottom: 1px solid color-mix(in srgb, var(--color-primary) 10%, transparent);
        padding-bottom: 6px;
    }

    .legend-title {
        font-size: 9px;
        font-weight: 900;
        letter-spacing: 0.2em;
        color: color-mix(in srgb, var(--color-primary) 80%, transparent);
        text-transform: uppercase;
    }

    .legend-close {
        color: color-mix(in srgb, var(--color-primary) 40%, transparent);
        background: none;
        border: none;
        cursor: pointer;
        padding: 0;
        line-height: 1;
    }
    .legend-close:hover { color: var(--color-primary); }

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

    .legend-select {
        width: 100%;
        background: rgba(0, 0, 0, 0.5);
        border: 1px solid color-mix(in srgb, var(--color-primary) 20%, transparent);
        color: var(--color-primary);
        font-size: 10px;
        font-family: var(--font-mono);
        font-weight: 700;
        padding: 4px 6px;
        border-radius: 4px;
        outline: none;
        cursor: pointer;
    }
    .legend-select:focus {
        border-color: color-mix(in srgb, var(--color-primary) 50%, transparent);
    }

    .legend-toggle {
        position: absolute;
        bottom: 16px;
        left: 16px;
        z-index: 10;
        background: rgba(10, 10, 10, 0.92);
        border: 1px solid color-mix(in srgb, var(--color-primary) 30%, transparent);
        border-radius: 8px;
        padding: 6px 12px;
        font-size: 10px;
        cursor: pointer;
        color: var(--color-primary);
        font-weight: 900;
        font-family: var(--font-mono);
        display: flex;
        align-items: center;
        gap: 6px;
        letter-spacing: 0.15em;
        text-transform: uppercase;
        backdrop-filter: blur(12px);
    }
    .legend-toggle:hover { background: color-mix(in srgb, var(--color-primary) 10%, transparent); }
</style>
