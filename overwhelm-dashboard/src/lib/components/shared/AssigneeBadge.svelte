<script lang="ts">
    import { classifyAssignee, assigneeIcon, assigneeLabel } from '../../data/assigneeUtils';

    type Props = { assignee?: string | null; compact?: boolean };
    let { assignee = null, compact = false }: Props = $props();

    let kind = $derived(classifyAssignee(assignee));
    let icon = $derived(assigneeIcon(kind));
    let label = $derived(assigneeLabel(assignee, kind));

    // Distinct colouring so human vs automated is readable at a glance
    let className = $derived(
        kind === 'human'
            ? 'text-sky-300 border-sky-400/30 bg-sky-500/10'
            : 'text-amber-300 border-amber-400/30 bg-amber-500/10'
    );
</script>

<span
    class="inline-flex items-center gap-1 border rounded-sm {compact ? 'px-1 py-0.5 text-[9px]' : 'px-1.5 py-0.5 text-[10px]'} font-mono uppercase tracking-wider {className}"
    title={`${kind === 'human' ? 'Human-assigned' : 'Automated / unclaimed'}: ${label}`}
    data-assignee-kind={kind}
>
    <span class="material-symbols-outlined text-[12px]">{icon}</span>
    {#if !compact}
        <span>{kind === 'human' ? 'HUMAN' : 'AUTO'}</span>
    {/if}
</span>
