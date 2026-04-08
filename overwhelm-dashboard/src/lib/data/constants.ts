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

export const STATUS_FILLS: Record<string, string> = {
    done: "#dcfce7",
    completed: "#dcfce7",
    cancelled: "#f1f5f9",
    active: "#dbeafe",
    in_progress: "#c7d2fe",
    blocked: "#3D2B2B",
    waiting: "#fef9c3",
    inbox: "#f1f5f9",
    todo: "#f1f5f9",
    review: "#f3e8ff",
    decomposing: "#e0f2fe",
    dormant: "#f1f5f9",
    archived: "#e2e8f0",
};

export const STATUS_TEXT: Record<string, string> = {
    done: "#166534",
    completed: "#166534",
    cancelled: "#94a3b8",
    active: "#1e3a5f",
    in_progress: "#312e81",
    blocked: "#9B7070",
    waiting: "#854d0e",
    inbox: "#475569",
    todo: "#475569",
    review: "#6b21a8",
    decomposing: "#0369a1",
    dormant: "#94a3b8",
    archived: "#94a3b8",
};

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
    { value: 2, label: 'ACTIVE',   short: 'ACTV', color: '#6c757d' },
    { value: 3, label: 'PLANNED',  short: 'PLAN', color: '#adb5bd' },
    { value: 4, label: 'BACKLOG',  short: 'BKLG', color: '#dee2e6' },
] as const;

export const PRIORITY_BORDERS: Record<number, string> = Object.fromEntries(
    PRIORITIES.map(p => [p.value, p.color])
);

export const INCOMPLETE_STATUSES = new Set([
    "inbox",
    "active",
    "in_progress",
    "blocked",
    "waiting",
    "todo",
    "pending",
]);

export const MUTED_FILL = "#e8eaed";
export const MUTED_TEXT = "#9ca3af";
