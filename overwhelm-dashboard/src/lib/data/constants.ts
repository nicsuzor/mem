/**
 * Graph visualization constants.
 */

/** Known node types — used for type dropdowns and validation. */
export const NODE_TYPES = [
    'goal', 'project', 'epic', 'task', 'action',
    'bug', 'feature', 'learn', 'daily',
    'knowledge', 'person', 'context', 'template', 'note',
] as const;

export const TYPE_BASE_SCALE: Record<string, number> = {
    goal: 1.5,
    project: 1.25,
    epic: 1.1,
    task: 1.0,
    action: 0.85,
    bug: 1.0,
    feature: 1.05,
    learn: 0.85,
    daily: 0.7,
    knowledge: 0.8,
    person: 0.9,
    context: 0.75,
    template: 0.7,
    note: 0.8,
};

export const TYPE_SHAPE: Record<string, string> = {
    goal: "pill",
    project: "rounded",
    epic: "hexagon",
    task: "rect",
    action: "rect",
    bug: "rect",
    feature: "rounded",
    learn: "rect",
    daily: "rect",
    knowledge: "rect",
    person: "pill",
};

// Canonical status palette — single source of truth.
// Used as fills on task cards AND as dot/chip colors in StatusFilterBar.
// "Green in the filter" = "green on the task card" by construction.
// Lifecycle: inbox → ready → queued → in_progress → merge_ready → done,
// with branches: review, blocked, paused, someday, cancelled.
// Labels chosen for distinguishable hue per lifecycle stage.
export const STATUS_FILLS: Record<string, string> = {
    inbox: "#38bdf8",  // sky — captured, untriaged
    ready: "#0cb4f7ff",  // cyan — decomposed + unblocked (auto)
    queued: "#0ddd4cff",  // lime — human-gated, dispatchable
    in_progress: "#a78bfa",  // violet — claimed, in flight
    merge_ready: "#fbbf24",  // amber — awaiting merge
    review: "#fb923c",  // orange — needs attention
    blocked: "#f87171",  // red — external blocker
    paused: "#94a3b8",  // slate — in-flight, deferred
    someday: "#64748b",  // dark slate — parked idea
    done: "#6ee7b7",  // mint — success
    cancelled: "#475569",  // grey — dropped
};

// Readable text color paired with each STATUS_FILLS value.
// Bright fills (sky, lime, amber, mint) get dark text; dim fills get light text.
export const STATUS_TEXT: Record<string, string> = {
    inbox: "#0a1929",
    ready: "#0a2015",
    queued: "#0a2015",
    in_progress: "#14102a",
    merge_ready: "#2a1e05",
    review: "#2a1608",
    blocked: "#2a0a0a",
    paused: "#141a24",
    someday: "#eef2f8",
    done: "#0a2015",
    cancelled: "#eef2f8",
};

// Canonical display order + labels for status (used by filter bar and legend).
// Keep in sync with STATUS_FILLS keys.
export const STATUS_ORDER = [
    'inbox', 'ready', 'queued', 'in_progress', 'merge_ready',
    'review', 'blocked', 'paused', 'someday', 'done', 'cancelled',
] as const;

export const STATUS_LABELS: Record<string, string> = {
    inbox: 'INBOX',
    ready: 'READY',
    queued: 'QUEUED',
    in_progress: 'IN PROGRESS',
    merge_ready: 'MERGE',
    review: 'REVIEW',
    blocked: 'BLOCKED',
    paused: 'PAUSED',
    someday: 'SOMEDAY',
    done: 'DONE',
    cancelled: 'CANCELLED',
};

// Coarse buckets surfaced by the mem graph's `status_group` (active/blocked/completed).
// Note: `active` here is the coarse group label (open work), not the retired `active` status.
export const STATUS_GROUP_SWATCHES = {
    active: `linear-gradient(135deg, ${STATUS_FILLS.ready} 0%, ${STATUS_FILLS.in_progress} 100%)`,
    blocked: STATUS_FILLS.blocked,
    completed: STATUS_FILLS.done,
} as const;

export const TYPE_BADGE: Record<string, string> = {
    goal: "GOAL",
    project: "PROJECT",
    epic: "EPIC",
    task: "",
    action: "ACTION",
    bug: "BUG",
    feature: "FEATURE",
    learn: "LEARN",
};

export const ASSIGNEE_COLORS: Record<string, string> = {
    bot: "#17a2b8",
    claude: "#17a2b8",
    worker: "#fd7e14",
    nic: "#6f42c1",
};

export const ASSIGNEE_DEFAULT = "#6c757d";

export const PRIORITIES = [
    { value: 0, label: 'CRITICAL', short: 'CRIT', color: '#dc3545' },
    { value: 1, label: 'INTENDED', short: 'INTD', color: '#f59e0b' },
    { value: 2, label: 'ACTIVE', short: 'ACTV', color: '#4f8fda' },
    { value: 3, label: 'PLANNED', short: 'PLAN', color: '#7b86c9' },
    { value: 4, label: 'BACKLOG', short: 'BKLG', color: '#8c96a3' },
] as const;

export const PRIORITY_BORDERS: Record<number, string> = Object.fromEntries(
    PRIORITIES.map(p => [p.value, p.color])
);

// Open-work set — matches mem's ACTIVE_STATUSES ∪ BLOCKED_STATUSES
// (see src/graph.rs and aops-core/TAXONOMY.md). Aliases (active, draft,
// waiting, decomposing, todo, …) are normalised upstream by the mem server,
// so they should never reach the dashboard.
export const INCOMPLETE_STATUSES = new Set<string>([
    "inbox",
    "ready",
    "queued",
    "in_progress",
    "merge_ready",
    "review",
    "blocked",
    "paused",
    "someday",
]);

// Terminal set — `done` and `cancelled` only.
export const COMPLETED_STATUSES = new Set<string>(["done", "cancelled"]);

// Mirrors src/graph.rs::resolve_status_alias. Normalise legacy/alternate
// status spellings to the canonical 11. Anything unrecognised passes through
// so the linter / dashboard can surface it. Defence-in-depth against data
// paths that bypass mem's normalisation (e.g. server-side projectData,
// stale .graph.json on disk, raw frontmatter accessed via _raw).
const STATUS_ALIASES: Record<string, string> = {
    active: "queued",
    todo: "inbox", open: "inbox", draft: "inbox",
    "early-scaffold": "inbox", planning: "inbox", seed: "inbox",
    "in-progress": "in_progress", "in-preparation": "in_progress",
    partial: "in_progress", decomposing: "in_progress",
    in_review: "review", "in-review": "review", "ready-for-review": "review",
    ISSUES_FOUND: "review", "conditionally-accepted": "review",
    "revise-and-resubmit": "review", waiting: "review", invited: "review",
    "awaiting-approval": "review", submitted: "review",
    "merge-ready": "merge_ready",
    complete: "done", completed: "done", closed: "done",
    archived: "done", resolved: "done", "published-spir": "done",
    historical: "done", accepted: "done",
    dead: "cancelled",
    deferred: "paused", dormant: "paused",
};

export function resolveStatusAlias(status: string | null | undefined): string {
    if (!status) return "inbox";
    return STATUS_ALIASES[status] ?? status;
}

// Self-consistency guard: legend palette and order must agree.
// Throws at module load if the keys ever diverge.
if (STATUS_ORDER.length !== Object.keys(STATUS_FILLS).length
    || STATUS_ORDER.some(s => !(s in STATUS_FILLS))) {
    throw new Error("STATUS_ORDER and STATUS_FILLS keys must match");
}

export const MUTED_FILL = "#e8eaed";
export const MUTED_TEXT = "#9ca3af";
