<script lang="ts">
    import { graphData } from "../../stores/graph";
    import { selection } from "../../stores/selection";
    import { projectColor } from "../../data/projectUtils";

    export let taskId: string | null;

    $: task = taskId ? $graphData?.nodes.find(n => n.id === taskId) : undefined;
    $: isProjectContainer = taskId?.startsWith('__project_') && !taskId.endsWith('_uncategorized__');
    $: projectName = isProjectContainer ? taskId?.replace(/^__project_/, '').replace(/__$/, '') : null;

    // Find ancestors (up) — compact chain
    $: ancestors = (() => {
        if (isProjectContainer || !$graphData) return [];
        const list: typeof $graphData.nodes = [];
        let curr = task;
        const seen = new Set<string>();
        while (curr && curr.parent && !seen.has(curr.parent)) {
            const parent = $graphData?.nodes.find(n => n.id === curr?.parent);
            if (!parent) break;
            list.unshift(parent);
            seen.add(curr.parent);
            curr = parent;
        }
        return list;
    })();

    // Find direct children (down)
    $: children = (() => {
        if (isProjectContainer && projectName) {
            return $graphData?.nodes
                .filter(n => n.project === projectName && !n.parent)
                .sort((a, b) => (a.priority ?? 5) - (b.priority ?? 5)) || [];
        }
        return $graphData?.nodes
            .filter(n => n.parent === taskId)
            .sort((a, b) => (a.priority ?? 5) - (b.priority ?? 5)) || [];
    })();

    // Grandchildren (one level deeper, grouped by child)
    $: grandchildMap = (() => {
        const map = new Map<string, typeof children>();
        for (const child of children) {
            const gc = $graphData?.nodes
                .filter(n => n.parent === child.id && !['done', 'completed', 'cancelled'].includes(n.status))
                .sort((a, b) => (a.priority ?? 5) - (b.priority ?? 5)) || [];
            if (gc.length > 0) map.set(child.id, gc);
        }
        return map;
    })();

    function select(id: string) {
        selection.update(s => ({ ...s, activeNodeId: id }));
    }

    function statusIcon(status: string): string {
        switch (status) {
            case 'done': case 'completed': return '✓';
            case 'blocked': return '✗';
            case 'active': case 'in_progress': return '●';
            case 'waiting': return '◷';
            case 'decomposing': return '⊞';
            default: return '○';
        }
    }

    function statusColor(status: string): string {
        switch (status) {
            case 'done': case 'completed': return 'text-green-500/60';
            case 'blocked': return 'text-red-400';
            case 'active': case 'in_progress': return 'text-blue-400';
            case 'waiting': return 'text-yellow-400';
            default: return 'text-primary/40';
        }
    }

    function nodeTypeIcon(type: string | undefined): string {
        switch (type) {
            case 'goal': return '◉';
            case 'project': case 'subproject': return '◈';
            case 'epic': return '▣';
            default: return '';
        }
    }
</script>

<div class="lineage-tree font-mono" data-component="lineage-map">
    <!-- Compact ancestor breadcrumb -->
    {#if ancestors.length > 0}
        <div class="ancestor-path">
            {#each ancestors as ancestor, i}
                {@const icon = nodeTypeIcon(ancestor.type)}
                <button
                    class="ancestor-crumb"
                    onclick={() => select(ancestor.id)}
                    style={ancestor.project ? `border-left-color: ${projectColor(ancestor.project)}` : ''}
                    title={ancestor.label}
                >
                    {#if icon}<span class="type-icon">{icon}</span>{/if}
                    <span class="crumb-label">{ancestor.label}</span>
                </button>
                {#if i < ancestors.length - 1}
                    <span class="crumb-sep">›</span>
                {/if}
            {/each}
        </div>
    {/if}

    <!-- Current node -->
    <div class="current-node"
        style={task?.project ? `border-left-color: ${projectColor(task.project)}` : ''}
    >
        <span class="current-marker">★</span>
        <span class="current-label">{task?.label || taskId}</span>
    </div>

    <!-- Children tree -->
    {#if children.length > 0}
        <div class="children-section">
            {#each children as child, i}
                {@const isLast = i === children.length - 1}
                {@const gc = grandchildMap.get(child.id) || []}
                <div class="child-row">
                    <span class="tree-connector">{isLast ? '└' : '├'}─</span>
                    <span class="status-icon {statusColor(child.status)}">{statusIcon(child.status)}</span>
                    <button
                        class="child-label"
                        class:completed={['done', 'completed'].includes(child.status)}
                        onclick={() => select(child.id)}
                    >
                        {child.label}
                    </button>
                    <span class="child-priority">P{child.priority ?? '?'}</span>
                </div>
                <!-- Inline grandchildren (collapsed) -->
                {#if gc.length > 0}
                    <div class="grandchild-group">
                        {#each gc.slice(0, 3) as gchild, j}
                            <div class="grandchild-row">
                                <span class="tree-connector gc-connector">{isLast ? ' ' : '│'} {j === gc.length - 1 || j === 2 ? '└' : '├'}─</span>
                                <span class="status-icon {statusColor(gchild.status)} gc-icon">{statusIcon(gchild.status)}</span>
                                <button class="gc-label" onclick={() => select(gchild.id)}>{gchild.label}</button>
                            </div>
                        {/each}
                        {#if gc.length > 3}
                            <div class="grandchild-row">
                                <span class="tree-connector gc-connector">{isLast ? ' ' : '│'} └─</span>
                                <span class="gc-more">+{gc.length - 3} more</span>
                            </div>
                        {/if}
                    </div>
                {/if}
            {/each}
        </div>
    {/if}
</div>

<style>
    .lineage-tree {
        font-size: 10px;
        line-height: 1.6;
    }

    .ancestor-path {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 2px;
        margin-bottom: 6px;
    }

    .ancestor-crumb {
        display: inline-flex;
        align-items: center;
        gap: 3px;
        padding: 1px 6px;
        background: none;
        border: none;
        border-left: 2px solid transparent;
        color: color-mix(in srgb, var(--color-primary) 60%, transparent);
        cursor: pointer;
        transition: color 0.15s, background 0.15s;
        border-radius: 2px;
        max-width: 140px;
    }
    .ancestor-crumb:hover {
        color: var(--color-primary);
        background: color-mix(in srgb, var(--color-primary) 8%, transparent);
    }

    .type-icon {
        font-size: 9px;
        opacity: 0.6;
    }

    .crumb-label {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .crumb-sep {
        color: color-mix(in srgb, var(--color-primary) 25%, transparent);
        font-size: 11px;
    }

    .current-node {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 4px 8px;
        background: color-mix(in srgb, var(--color-primary) 8%, transparent);
        border: 1px solid color-mix(in srgb, var(--color-primary) 25%, transparent);
        border-left: 3px solid transparent;
        border-radius: 3px;
        margin-bottom: 6px;
    }

    .current-marker {
        color: var(--color-primary);
        font-size: 9px;
    }

    .current-label {
        font-weight: 700;
        color: var(--color-primary);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .children-section {
        display: flex;
        flex-direction: column;
        max-height: 280px;
        overflow-y: auto;
    }

    .child-row {
        display: flex;
        align-items: center;
        gap: 4px;
        padding: 1px 0;
    }

    .tree-connector {
        color: color-mix(in srgb, var(--color-primary) 25%, transparent);
        font-size: 10px;
        flex-shrink: 0;
        white-space: pre;
    }

    .status-icon {
        font-size: 9px;
        flex-shrink: 0;
        width: 12px;
        text-align: center;
    }

    .child-label {
        background: none;
        border: none;
        color: color-mix(in srgb, var(--color-primary) 70%, transparent);
        cursor: pointer;
        text-align: left;
        padding: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        flex: 1;
        transition: color 0.15s;
    }
    .child-label:hover { color: var(--color-primary); }
    .child-label.completed {
        text-decoration: line-through;
        opacity: 0.4;
    }

    .child-priority {
        font-size: 8px;
        color: color-mix(in srgb, var(--color-primary) 30%, transparent);
        flex-shrink: 0;
    }

    .grandchild-group {
        margin-left: 4px;
    }

    .grandchild-row {
        display: flex;
        align-items: center;
        gap: 3px;
        padding: 0;
    }

    .gc-connector {
        font-size: 9px;
    }

    .gc-icon {
        font-size: 8px;
        width: 10px;
    }

    .gc-label {
        background: none;
        border: none;
        color: color-mix(in srgb, var(--color-primary) 50%, transparent);
        cursor: pointer;
        text-align: left;
        padding: 0;
        font-size: 9px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        flex: 1;
    }
    .gc-label:hover { color: var(--color-primary); }

    .gc-more {
        font-size: 8px;
        color: color-mix(in srgb, var(--color-primary) 30%, transparent);
        font-style: italic;
    }
</style>
