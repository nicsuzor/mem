<script lang="ts">
    import { filters } from '../../stores/filters';
    import { graphData } from '../../stores/graph';
    import { STATUS_FILLS, STATUS_LABELS, STATUS_ORDER } from '../../data/constants';

    // Chips follow the canonical lifecycle in aops-core/TAXONOMY.md and consume the
    // canonical palette from constants.ts — chip color == card fill color for the
    // same status ("green in the filter" = "green on the card").
    const STATUS_GROUPS = [
        { label: 'ALL', statuses: [] as string[], color: '#94a3b8' },
        ...STATUS_ORDER.map(s => ({
            label: STATUS_LABELS[s],
            statuses: [s],
            color: STATUS_FILLS[s],
        })),
    ];

    function toggleGroup(group: typeof STATUS_GROUPS[number]) {
        if (group.statuses.length === 0) {
            filters.update(f => ({ ...f, selectedStatuses: [] }));
            return;
        }
        filters.update(f => {
            const current = f.selectedStatuses;
            const anyActive = group.statuses.some(s => current.includes(s));
            if (anyActive) {
                return { ...f, selectedStatuses: current.filter(s => !(group.statuses as readonly string[]).includes(s)) };
            } else {
                return { ...f, selectedStatuses: [...current, ...group.statuses] };
            }
        });
    }

    $: counts = $graphData ? (() => {
        const nodes = $graphData.nodes;
        const result: Record<string, number> = {};
        for (const g of STATUS_GROUPS) {
            if (g.statuses.length === 0) {
                result['ALL'] = nodes.length;
            } else {
                result[g.label] = nodes.filter(n => (g.statuses as readonly string[]).includes(n.status)).length;
            }
        }
        return result;
    })() : {};
</script>

<div class="status-filter-bar" role="toolbar" aria-label="Status filter">
    {#each STATUS_GROUPS as group}
        {@const active = group.statuses.length === 0
            ? $filters.selectedStatuses.length === 0
            : group.statuses.some(s => $filters.selectedStatuses.includes(s))}
        {@const count = group.statuses.length === 0 ? (counts['ALL'] ?? 0) : (counts[group.label] ?? 0)}
        <button
            class="status-chip"
            class:active
            style="--chip-color: {group.color}"
            onclick={() => toggleGroup(group)}
            title="Filter by {group.label}"
        >
            <span class="chip-dot"></span>
            <span class="chip-label">{group.label}</span>
            <span class="chip-count">{count}</span>
        </button>
    {/each}
    <div class="separator"></div>
    <button
        class="status-chip"
        class:active={$filters.minCriticality > 0}
        style="--chip-color: #f59e0b"
        onclick={() => filters.update(f => ({ ...f, minCriticality: f.minCriticality > 0 ? 0 : 0.4 }))}
        title="Show only high-criticality tasks (criticality ≥ 40%)"
    >
        <span class="chip-dot"></span>
        <span class="chip-label">HIGH_CRIT</span>
    </button>
</div>

<style>
    .status-filter-bar {
        display: flex;
        align-items: center;
        gap: 4px;
        flex-wrap: nowrap;
        overflow-x: auto;
        padding: 6px 12px;
        scrollbar-width: none;
    }
    .status-filter-bar::-webkit-scrollbar { display: none; }

    .separator {
        width: 1px;
        height: 18px;
        background: color-mix(in srgb, currentColor 15%, transparent);
        flex-shrink: 0;
        margin: 0 4px;
    }

    .status-chip {
        display: inline-flex;
        align-items: center;
        gap: 5px;
        padding: 3px 8px 3px 6px;
        border-radius: 3px;
        border: 1px solid color-mix(in srgb, var(--chip-color) 20%, transparent);
        background: transparent;
        cursor: pointer;
        font-family: var(--font-mono);
        font-size: 10px;
        font-weight: 700;
        letter-spacing: 0.08em;
        color: color-mix(in srgb, var(--chip-color) 40%, var(--color-primary, #ccc));
        opacity: 0.45;
        transition: background 0.12s, border-color 0.12s, color 0.12s, opacity 0.12s, box-shadow 0.12s;
        white-space: nowrap;
        flex-shrink: 0;
    }
    .status-chip:hover {
        opacity: 0.75;
        background: color-mix(in srgb, var(--chip-color) 12%, transparent);
        border-color: color-mix(in srgb, var(--chip-color) 45%, transparent);
        color: var(--chip-color);
    }
    .status-chip.active {
        opacity: 1;
        background: color-mix(in srgb, var(--chip-color) 80%, #111);
        border-color: var(--chip-color);
        color: #fff;
        box-shadow: 0 0 10px color-mix(in srgb, var(--chip-color) 55%, transparent),
                    0 0 2px color-mix(in srgb, var(--chip-color) 80%, transparent);
        text-shadow: 0 0 6px rgba(255,255,255,0.4);
    }

    .chip-dot {
        width: 6px;
        height: 6px;
        border-radius: 50%;
        background: var(--chip-color);
        opacity: 0.5;
        flex-shrink: 0;
    }
    .status-chip.active .chip-dot {
        opacity: 1;
        background: #fff;
        box-shadow: 0 0 5px #fff;
    }

    .chip-label {
        line-height: 1;
    }

    .chip-count {
        font-size: 9px;
        opacity: 0.5;
        font-weight: 500;
    }
    .status-chip.active .chip-count {
        opacity: 1;
        font-weight: 700;
    }
</style>
