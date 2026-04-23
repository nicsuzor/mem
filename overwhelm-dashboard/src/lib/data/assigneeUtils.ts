/**
 * Classify a task's assignee as human vs automated so the queue can
 * show a visual cue before triage decisions are made.
 *
 * Convention in this workspace:
 *   - Explicit person handles (nic, user names) → human
 *   - Bot/agent handles (claude, polecat-*, crew-*, bot, etc.) → automated
 *   - Empty/null assignee → auto (agents pull from the ready queue)
 */

export type AssigneeKind = 'human' | 'auto';

const AUTO_PATTERNS: RegExp[] = [
    /^claude(-.*)?$/i,
    /^polecat(-.*)?$/i,
    /^crew(-.*)?$/i,
    /^swarm(-.*)?$/i,
    /^burst(-.*)?$/i,
    /^worker(-.*)?$/i,
    /bot$/i,
    /^agent(-.*)?$/i,
    /^autonomous$/i,
    /^scheduler$/i,
    /^cron(-.*)?$/i,
];

const KNOWN_HUMAN_HANDLES = new Set(['nic', 'user', 'me']);

export function classifyAssignee(assignee: string | null | undefined): AssigneeKind {
    if (!assignee) return 'auto';
    const handle = assignee.trim().toLowerCase();
    if (!handle) return 'auto';
    if (KNOWN_HUMAN_HANDLES.has(handle)) return 'human';
    if (AUTO_PATTERNS.some((p) => p.test(handle))) return 'auto';
    return 'human';
}

export function assigneeIcon(kind: AssigneeKind): string {
    return kind === 'human' ? 'person' : 'smart_toy';
}

export function assigneeLabel(assignee: string | null | undefined, kind: AssigneeKind): string {
    if (assignee && assignee.trim()) return assignee;
    return kind === 'auto' ? 'unassigned · auto' : 'unassigned';
}
