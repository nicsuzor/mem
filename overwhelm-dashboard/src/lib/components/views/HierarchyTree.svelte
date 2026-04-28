<script lang="ts">
    import { graphData } from "../../stores/graph";
    import { selection } from "../../stores/selection";
    import { projectColor } from "../../data/projectUtils";

    export let taskId: string | null;

    $: task = taskId ? $graphData?.nodes.find(n => n.id === taskId) : undefined;
    $: isProjectContainer = taskId?.startsWith('__project_') && !taskId.endsWith('_uncategorized__');
    $: projectName = isProjectContainer ? taskId?.replace(/^__project_/, '').replace(/__$/, '') : null;

    function byPriorityThenLabel(a: any, b: any) {
        const priorityDelta = (a.priority ?? 5) - (b.priority ?? 5);
        if (priorityDelta !== 0) return priorityDelta;
        return (a.label || a.id).localeCompare(b.label || b.id);
    }

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

    $: siblings = (() => {
        if (!$graphData || !task || !task.parent) return [];
        return $graphData.nodes
            .filter(n => n.parent === task.parent && n.id !== task.id)
            .sort(byPriorityThenLabel);
    })();

    // Find direct children (down)
    $: children = (() => {
        if (isProjectContainer && projectName) {
            return $graphData?.nodes
                .filter(n => n.project === projectName && !n.parent)
                .sort(byPriorityThenLabel) || [];
        }
        return $graphData?.nodes
            .filter(n => n.parent === taskId)
            .sort(byPriorityThenLabel) || [];
    })();

    $: grandchildMap = (() => {
        const map = new Map<string, typeof children>();
        for (const child of children) {
            const preview = $graphData?.nodes
                .filter(n => n.parent === child.id)
                .sort(byPriorityThenLabel) || [];
            if (preview.length > 0) map.set(child.id, preview);
        }
        return map;
    })();

    // Upstream (Blockers - what this task depends on)
    $: upstream = (() => {
        if (!$graphData || !taskId) return [];
        return $graphData.links
            .filter(l => l.type === 'depends_on' && (typeof l.source === 'object' ? l.source.id : l.source) === taskId)
            .map(l => {
                const targetId = typeof l.target === 'object' ? l.target.id : l.target;
                return $graphData.nodes.find(n => n.id === targetId);
            })
            .filter(n => !!n);
    })();

    // Downstream (Blocked by this - what depends on this task)
    $: downstream = (() => {
        if (!$graphData || !taskId) return [];
        return $graphData.links
            .filter(l => l.type === 'depends_on' && (typeof l.target === 'object' ? l.target.id : l.target) === taskId)
            .map(l => {
                const sourceId = typeof l.source === 'object' ? l.source.id : l.source;
                return $graphData.nodes.find(n => n.id === sourceId);
            })
            .filter(n => !!n);
    })();

    function select(id: string) {
        selection.update(s => ({ ...s, activeNodeId: id }));
    }

    function statusIcon(status: string): string {
        switch (status) {
            case 'done': return '✓';
            case 'cancelled': return '✗';
            case 'blocked': return '✗';
            case 'in_progress': return '●';
            case 'review': return '◷';
            case 'merge_ready': return '⊞';
            default: return '○';
        }
    }

    function statusColor(status: string): string {
        switch (status) {
            case 'done': return 'text-green-500/60';
            case 'cancelled': return 'text-primary/30';
            case 'blocked': return 'text-red-400';
            case 'in_progress': return 'text-blue-400';
            case 'review': return 'text-yellow-400';
            case 'merge_ready': return 'text-amber-400';
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

    function nodeSummary(node: any) {
        return [node.type?.toUpperCase(), `P${node.priority ?? '?'}`].filter(Boolean).join(' · ');
    }
</script>

<div class="lineage-tree font-mono" data-component="lineage-map">
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

    <div class="current-node"
        style={task?.project ? `border-left-color: ${projectColor(task.project)}` : ''}
    >
        <span class="current-marker">★</span>
        <div class="current-copy">
            <span class="current-label">{task?.label || taskId}</span>
            {#if task}
                <span class="node-meta">{nodeSummary(task)}{task.status ? ` · ${task.status}` : ''}</span>
            {/if}
        </div>
    </div>

    {#if siblings.length > 0}
        <div class="sibling-strip">
            <span class="inline-label">Siblings</span>
            <div class="pill-list">
                {#each siblings.slice(0, 4) as sibling}
                    <button class="context-pill" onclick={() => select(sibling.id)} title={sibling.label}>
                        <span class="status-icon {statusColor(sibling.status)}">{statusIcon(sibling.status)}</span>
                        <span class="pill-label">{sibling.label}</span>
                    </button>
                {/each}
                {#if siblings.length > 4}
                    <span class="gc-more">+{siblings.length - 4} more</span>
                {/if}
            </div>
        </div>
    {/if}

    {#if upstream.length > 0 || downstream.length > 0}
        <div class="dependency-grid">
            {#if upstream.length > 0}
                <div class="dependency-section">
                    <span class="dependency-header">Depends On</span>
                    {#each upstream as dep}
                        <button class="dependency-row" onclick={() => select(dep.id)}>
                            <span class="status-icon {statusColor(dep.status)}">{statusIcon(dep.status)}</span>
                            <span class="dependency-label">{dep.label}</span>
                            <span class="pill-meta">P{dep.priority ?? '?'}</span>
                        </button>
                    {/each}
                </div>
            {/if}

            {#if downstream.length > 0}
                <div class="dependency-section">
                    <span class="dependency-header">Blocks</span>
                    {#each downstream as dep}
                        <button class="dependency-row" onclick={() => select(dep.id)}>
                            <span class="status-icon {statusColor(dep.status)}">{statusIcon(dep.status)}</span>
                            <span class="dependency-label">{dep.label}</span>
                            <span class="pill-meta">P{dep.priority ?? '?'}</span>
                        </button>
                    {/each}
                </div>
            {/if}
        </div>
    {/if}

    {#if children.length > 0}
        <div class="children-section">
            {#each children as child, i}
                {@const isLast = i === children.length - 1}
                {@const preview = grandchildMap.get(child.id) || []}
                <div class="child-row-wrap">
                    <div class="child-row">
                        <span class="tree-connector">{isLast ? '└' : '├'}─</span>
                        <span class="status-icon {statusColor(child.status)}">{statusIcon(child.status)}</span>
                        <button class="child-label" class:completed={child.status === 'done' || child.status === 'cancelled'} onclick={() => select(child.id)}>{child.label}</button>
                        <span class="child-priority">P{child.priority ?? '?'}</span>
                    </div>
                    {#if preview.length > 0}
                        <div class="grandchild-group">
                            {#each preview.slice(0, 3) as nested, j}
                                <div class="grandchild-row">
                                    <span class="tree-connector gc-connector">{isLast ? ' ' : '│'} {j === preview.length - 1 || j === 2 ? '└' : '├'}─</span>
                                    <span class="status-icon gc-icon {statusColor(nested.status)}">{statusIcon(nested.status)}</span>
                                    <button class="gc-label" onclick={() => select(nested.id)}>{nested.label}</button>
                                </div>
                            {/each}
                            {#if preview.length > 3}
                                <div class="grandchild-row">
                                    <span class="tree-connector gc-connector">{isLast ? ' ' : '│'} └─</span>
                                    <span class="gc-more">+{preview.length - 3} more</span>
                                </div>
                            {/if}
                        </div>
                    {/if}
                </div>
            {/each}
        </div>
    {:else}
        <div class="empty-copy">No children below this node.</div>
    {/if}
</div>

<style>
    .lineage-tree {
        font-size: 9px;
        line-height: 1.35;
        display: flex;
        flex-direction: column;
        gap: 7px;
    }

    .ancestor-path {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 2px;
    }

    .ancestor-crumb {
        display: inline-flex;
        align-items: center;
        gap: 4px;
        padding: 1px 5px;
        border: none;
        border-left: 2px solid transparent;
        border-radius: 2px;
        background: none;
        color: color-mix(in srgb, var(--color-primary) 70%, transparent);
        cursor: pointer;
    }

    .ancestor-crumb:hover {
        background: color-mix(in srgb, var(--color-primary) 8%, transparent);
        color: var(--color-primary);
    }

    .type-icon {
        font-size: 9px;
        opacity: 0.6;
    }

    .context-pill,
    .dependency-row,
    .preview-row,
    .child-label,
    .gc-label {
        background: none;
        cursor: pointer;
        transition: color 0.15s, border-color 0.15s, background 0.15s;
    }

    .context-pill:hover,
    .dependency-row:hover,
    .preview-row:hover,
    .child-label:hover,
    .gc-label:hover {
        border-color: color-mix(in srgb, var(--color-primary) 34%, transparent);
        color: var(--color-primary);
        background: color-mix(in srgb, var(--color-primary) 8%, transparent);
    }

    .crumb-label {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        color: color-mix(in srgb, var(--color-primary) 76%, transparent);
    }

    .crumb-sep {
        color: color-mix(in srgb, var(--color-primary) 24%, transparent);
        font-size: 10px;
    }

    .current-node {
        display: flex;
        align-items: flex-start;
        gap: 6px;
        padding: 4px 7px;
        background: color-mix(in srgb, var(--color-primary) 8%, transparent);
        border: 1px solid color-mix(in srgb, var(--color-primary) 25%, transparent);
        border-left: 3px solid transparent;
        border-radius: 3px;
    }

    .current-marker {
        color: var(--color-primary);
        font-size: 9px;
        margin-top: 3px;
    }

    .current-copy {
        display: flex;
        flex-direction: column;
        min-width: 0;
        gap: 2px;
    }

    .current-label {
        font-weight: 700;
        color: var(--color-primary);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .node-meta,
    .pill-meta,
    .descendant-meta {
        font-size: 8px;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        color: color-mix(in srgb, var(--color-primary) 34%, transparent);
    }

    .dependency-label,
    .pill-label {
        color: color-mix(in srgb, var(--color-primary) 76%, transparent);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .sibling-strip {
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    .pill-list {
        display: flex;
        flex-wrap: wrap;
        gap: 4px;
    }

    .context-pill {
        display: inline-flex;
        align-items: center;
        gap: 4px;
        max-width: 100%;
        padding: 2px 6px;
        border: 1px solid color-mix(in srgb, var(--color-primary) 12%, transparent);
        border-radius: 999px;
        background: color-mix(in srgb, var(--color-primary) 3%, transparent);
    }

    .inline-label {
        font-size: 8px;
        font-weight: 800;
        letter-spacing: 0.14em;
        text-transform: uppercase;
        color: color-mix(in srgb, var(--color-primary) 42%, transparent);
    }

    .dependency-grid {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(0, 1fr));
        gap: 6px;
    }

    .children-section {
        display: flex;
        flex-direction: column;
        gap: 4px;
        max-height: 260px;
        overflow-y: auto;
    }

    .dependency-section {
        display: flex;
        flex-direction: column;
        gap: 3px;
        padding: 5px 6px;
        border: 1px solid color-mix(in srgb, var(--color-primary) 10%, transparent);
        border-radius: 3px;
        background: color-mix(in srgb, var(--color-primary) 3%, transparent);
        border-left: 1px solid color-mix(in srgb, var(--color-primary) 10%, transparent);
    }

    .dependency-header {
        font-size: 8px;
        font-weight: bold;
        text-transform: uppercase;
        color: color-mix(in srgb, var(--color-primary) 40%, transparent);
    }

    .dependency-row {
        display: grid;
        grid-template-columns: 12px minmax(0, 1fr) auto;
        align-items: center;
        gap: 4px;
        padding: 1px 0;
        border: none;
        text-align: left;
    }

    .child-row-wrap {
        display: flex;
        flex-direction: column;
        gap: 2px;
    }

    .child-row {
        display: grid;
        grid-template-columns: 16px 12px minmax(0, 1fr) auto;
        align-items: center;
        gap: 4px;
        padding: 0;
    }

    .tree-connector {
        color: color-mix(in srgb, var(--color-primary) 26%, transparent);
        white-space: pre;
        font-size: 9px;
    }

    .status-icon {
        font-size: 8px;
        flex-shrink: 0;
        width: 10px;
        text-align: center;
    }

    .child-label {
        color: color-mix(in srgb, var(--color-primary) 70%, transparent);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        transition: color 0.15s;
    }
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
        display: flex;
        flex-direction: column;
        gap: 1px;
        margin-left: 16px;
    }

    .grandchild-row {
        display: grid;
        grid-template-columns: 16px 10px minmax(0, 1fr);
        align-items: center;
        gap: 4px;
        padding: 0;
    }

    .gc-connector {
        font-size: 8px;
    }

    .gc-icon {
        font-size: 7px;
        width: 8px;
    }

    .gc-label {
        color: color-mix(in srgb, var(--color-primary) 50%, transparent);
        font-size: 8px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .gc-more {
        font-size: 8px;
        color: color-mix(in srgb, var(--color-primary) 30%, transparent);
        font-style: italic;
    }

    .empty-copy {
        font-size: 9px;
        color: color-mix(in srgb, var(--color-primary) 24%, transparent);
        font-style: italic;
    }
</style>
