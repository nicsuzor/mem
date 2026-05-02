<script lang="ts">
    import { filters, cycleVisibility, type VisibilityState } from '../../stores/filters';
    import { graphData } from '../../stores/graph';
    import { viewSettings } from '../../stores/viewSettings';
    import { PRIORITIES } from '../../data/constants';
    import { projectColor } from '../../data/projectUtils';

    let showAllProjects = false;
    const MAX_VISIBLE_PROJECTS = 6;

    // Priority = border color — now interactive (click to cycle)
    const priorityItems = PRIORITIES.map(p => ({
        key: `priority${p.value}`,
        value: p.value,
        label: `P${p.value} ${p.label}`,
        color: p.color,
    }));

    import { EDGE_TYPES } from '../../data/taxonomy';

    const edgeTypes = Object.values(EDGE_TYPES).map(e => ({
        key: e.filterKey,
        label: e.displayName.toUpperCase(),
        color: e.color,
        dash: e.dashStyle !== 'solid',
        rawKey: e.id // to match edgeCounts mapping if needed
    }));

    const metroNodeTypes = [
        { label: 'TERMINAL', description: 'P0/P1 destination. Always labelled, priority-coloured border, bottom-row anchor.', sampleClass: 'sample-terminal' },
        { label: 'INTERCHANGE', description: 'Serves two or more destinations. Always labelled — highest-leverage work.', sampleClass: 'sample-interchange' },
        { label: 'ROUTE STATION', description: 'On the route to one destination. Size tracks downstream weight.', sampleClass: 'sample-task' },
        { label: 'CONTEXT STATION', description: 'Not on any route. Hidden by default; small dot when shown.', sampleClass: 'sample-context' },
        { label: 'COMPLETED', description: 'Desaturated (already-traversed track).', sampleClass: 'sample-completed' },
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

    function toggleSolo(project: string) {
        filters.update(f => ({
            ...f,
            hiddenProjects: availableProjects.filter(p => p !== project)
        }));
    }

    function toggleAllProjects() {
        filters.update(f => {
            if ((f.hiddenProjects?.length ?? 0) > 0) {
                return { ...f, hiddenProjects: [] };
            } else {
                return { ...f, hiddenProjects: [...availableProjects] };
            }
        });
    }

    function hideAllProjects() {
        filters.update(f => ({ ...f, hiddenProjects: [...availableProjects] }));
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

    $: isMetroLegend = $viewSettings.viewMode === 'Metro';

    $: nodeCounts = $graphData ? (() => {
        const nodes = $graphData.nodes;
        return {
            priority0: nodes.filter(n => n.priority === 0).length,
            priority1: nodes.filter(n => n.priority === 1).length,
            priority2: nodes.filter(n => n.priority === 2).length,
            priority3: nodes.filter(n => n.priority === 3).length,
            priority4: nodes.filter(n => n.priority === 4).length,
        };
    })() : null;

    $: edgeCounts = $graphData ? (() => {
        const links = $graphData.links;
        return {
            parent_inter: links.filter((l: any) => l.type === 'parent').length,
            parent_intra: links.filter((l: any) => l.type === 'edgeIntraGroup').length,
            depends_on: links.filter((l: any) => l.type === 'depends_on').length,
            soft_depends_on: links.filter((l: any) => l.type === 'soft_depends_on').length,
            contributes_to: links.filter((l: any) => l.type === 'contributes_to').length,
            similar_to: links.filter((l: any) => l.type === 'similar_to').length,
            ref: links.filter((l: any) => l.type === 'ref').length,
        };
    })() : null;

    $: availableProjects = $graphData?.allProjects || [];

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



            <!-- Priority filter (click to cycle: bright → half → hidden) -->
            <div class="legend-section">
                <span class="legend-section-title">PRIORITY (BORDER)</span>
                {#each priorityItems as p}
                    {@const vis = $filters[p.key as keyof typeof $filters] as VisibilityState}
                    <button
                        class="legend-item"
                        class:dimmed={vis === 'hidden'}
                        on:click={() => cycleFilter(p.key)}
                        title="Click to cycle: bright → half → hidden"
                    >
                        <div class="legend-box" style="background:rgba(10, 14, 20, 0.92); border: 2px solid {p.color}; opacity:{edgeOpacityForLegend(vis)};"></div>
                        <span class="legend-label">{p.label}{nodeCounts ? ` [${nodeCounts[p.key as keyof typeof nodeCounts]}]` : ''}</span>
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
                        <span class="legend-label">{edge.label}{!isMetroLegend && edgeCounts ? ` [${edgeCounts[edge.rawKey as keyof typeof edgeCounts]}]` : ''}</span>
                        <span class="edge-state">{stateLabel(vis)}</span>
                    </button>

                {/each}
            </div>

            {#if isMetroLegend}
                <div class="legend-section">
                    <span class="legend-section-title">NODES</span>
                    {#each metroNodeTypes as station}
                        <div class="legend-item legend-static-item">
                            <div class={`legend-node-sample ${station.sampleClass}`}></div>
                            <div class="legend-copy">
                                <span class="legend-label">{station.label}</span>
                                <span class="legend-note legend-note-inline">{station.description}</span>
                            </div>
                        </div>
                    {/each}
                </div>
            {/if}

            <!-- Project filter with color swatches -->
            <div class="legend-section">
                <span class="legend-section-title">PROJECTS (CLICK TO TOGGLE)</span>
                <div class="project-row" class:dimmed={($filters.hiddenProjects?.length ?? 0) === availableProjects.length}>
                    <button
                        class="legend-item project-main"
                        on:click={toggleAllProjects}
                    >
                        <div class="legend-box" style="background: #666; border-radius: 50%;"></div>
                        <span class="legend-label">ALL PROJECTS</span>
                        {#if ($filters.hiddenProjects?.length ?? 0) === 0}
                            <span class="filter-badge">ON</span>
                        {/if}
                    </button>
                    <button 
                        class="solo-action" 
                        on:click={hideAllProjects}
                        title="Hide all projects"
                    >
                        NONE
                    </button>
                </div>
                <div class="project-list" class:expanded={showAllProjects}>
                    {#each visibleProjects as proj}
                        <div class="project-row" class:dimmed={$filters.hiddenProjects?.includes(proj)}>
                            <button
                                class="legend-item project-main"
                                on:click={() => toggleProject(proj)}
                                title="Toggle {proj}"
                            >
                                <div class="legend-box project-swatch" style="background: {projectColor(proj)};"></div>
                                <span class="legend-label">{(proj || '').toUpperCase()}</span>
                                {#if !($filters.hiddenProjects?.includes(proj))}
                                    <span class="filter-badge">ON</span>
                                {/if}
                            </button>
                            <button 
                                class="solo-action" 
                                on:click={() => toggleSolo(proj)}
                                title="Solo this project"
                            >
                                SOLO
                            </button>
                        </div>
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

    .project-row {
        display: flex;
        align-items: center;
        gap: 4px;
        transition: opacity 0.15s;
    }

    .project-row.dimmed {
        opacity: 0.4;
    }

    .project-main {
        flex: 1;
        min-width: 0;
    }

    .solo-action {
        font-size: 8px;
        font-weight: 900;
        color: color-mix(in srgb, var(--color-primary) 40%, transparent);
        background: color-mix(in srgb, var(--color-primary) 5%, transparent);
        padding: 2px 5px;
        border-radius: 4px;
        cursor: pointer;
        border: 1px solid transparent;
        transition: all 0.15s;
        flex-shrink: 0;
    }

    .solo-action:hover {
        background: color-mix(in srgb, var(--color-primary) 15%, transparent);
        color: var(--color-primary);
    }

    .solo-action.active {
        background: #f59e0b;
        color: #fff;
        border-color: #d97706;
    }

    .legend-label {
        font-size: 10px;
        font-weight: 700;
        color: var(--color-primary);
        transition: opacity 0.15s;
    }

    .legend-copy {
        display: flex;
        flex-direction: column;
        gap: 1px;
        min-width: 0;
    }

    .legend-note {
        font-size: 8px;
        color: color-mix(in srgb, var(--color-primary) 42%, transparent);
        letter-spacing: 0.04em;
        text-transform: none;
        padding-left: 24px;
    }

    .legend-note-inline {
        padding-left: 0;
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

    .legend-static-item {
        cursor: default;
    }

    .legend-static-item:hover {
        background: none;
    }

    .legend-node-sample {
        flex: 0 0 auto;
        position: relative;
    }

    .sample-task {
        width: 9px;
        height: 9px;
        border-radius: 999px;
        background: #42d4f4;
        border: 1px solid rgba(255, 255, 255, 0.18);
    }

    .sample-terminal {
        width: 14px;
        height: 14px;
        border-radius: 999px;
        background: #42d4f4;
        border: 2.5px solid #dc3545;
    }

    .sample-interchange {
        width: 12px;
        height: 12px;
        border-radius: 999px;
        background: #42d4f4;
        border: 2px solid #ffffff;
    }

    .sample-context {
        width: 5px;
        height: 5px;
        border-radius: 999px;
        background: #6b7280;
        opacity: 0.5;
    }

    .sample-completed {
        width: 8px;
        height: 8px;
        border-radius: 999px;
        background: #5a6575;
        opacity: 0.55;
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
