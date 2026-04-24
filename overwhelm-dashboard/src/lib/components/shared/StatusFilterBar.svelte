<script lang="ts">
    import { filters } from '../../stores/filters';
    import { graphData } from '../../stores/graph';

    // Chips follow the canonical lifecycle in aops-core/TAXONOMY.md:
    //   inbox → ready → queued → in_progress → merge_ready → review/done
    // `ready` is auto-computed (decomposed + deps resolved); `queued` is the
    // human gate that promotes tasks for agent dispatch. They are distinct.
    const STATUS_GROUPS = [
        { label: 'ALL',         statuses: [] as string[],        color: '#94a3b8' },
        { label: 'INBOX',       statuses: ['inbox'],             color: '#38bdf8' },  // sky — captured, untriaged
        { label: 'READY',       statuses: ['ready'],             color: '#86efac' },  // light lime — decomposed + unblocked (auto)
        { label: 'QUEUED',      statuses: ['queued'],            color: '#4ade80' },  // lime — human-gated, dispatchable
        { label: 'IN PROGRESS', statuses: ['in_progress'],       color: '#a78bfa' },  // violet — claimed, in flight
        { label: 'MERGE',       statuses: ['merge_ready'],       color: '#fbbf24' },  // amber — awaiting merge
        { label: 'REVIEW',      statuses: ['review'],            color: '#fb923c' },  // orange — needs attention
        { label: 'BLOCKED',     statuses: ['blocked'],           color: '#f87171' },  // red — external blocker
        { label: 'PAUSED',      statuses: ['paused'],            color: '#94a3b8' },  // slate — in-flight, deferred
        { label: 'SOMEDAY',     statuses: ['someday'],           color: '#64748b' },  // dark slate — parked idea
        { label: 'DONE',        statuses: ['done'],              color: '#6ee7b7' },  // mint — success
        { label: 'CANCELLED',   statuses: ['cancelled'],         color: '#475569' },  // grey — dropped
    ] as const;

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
