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
    active: "#2C4A88",
    in_progress: "#2C4A88",
    review: "#3A5A9E",
    waiting: "#1E3A6E",
    decomposing: "#1E3A6E",
    blocked: "#6B3A3A",
    ready: "#2D5A3D",
    todo: "#2D5A3D",
    inbox: "#1E4A2E",
    dormant: "#2D2D35",
    done: "#1E1E24",
    completed: "#1E1E24",
    cancelled: "#18181C",
    deferred: "#2D2D35",
    paused: "#4b5563",
    archived: "#323846",
};

export const STATUS_TEXT: Record<string, string> = {
    active: "#edf3ff",
    in_progress: "#edf3ff",
    review: "#edf3ff",
    waiting: "#e6efff",
    decomposing: "#e6efff",
    blocked: "#ffe4e8",
    ready: "#e6f5eb",
    todo: "#e6f5eb",
    inbox: "#dbf1e3",
    dormant: "#d6dbe3",
    done: "#d7dde7",
    completed: "#d7dde7",
    cancelled: "#c5ccd6",
    deferred: "#d2d8e1",
    paused: "#edf2f7",
    archived: "#d2d8e1",
};

export const STATUS_GROUP_SWATCHES = {
    active: `linear-gradient(135deg, ${STATUS_FILLS.ready} 0%, ${STATUS_FILLS.active} 100%)`,
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
    { value: 2, label: 'ACTIVE',   short: 'ACTV', color: '#4f8fda' },
    { value: 3, label: 'PLANNED',  short: 'PLAN', color: '#7b86c9' },
    { value: 4, label: 'BACKLOG',  short: 'BKLG', color: '#8c96a3' },
] as const;

export const PRIORITY_BORDERS: Record<number, string> = Object.fromEntries(
    PRIORITIES.map(p => [p.value, p.color])
);

export const INCOMPLETE_STATUSES = new Set([
    "inbox",
    "draft",
    "ready",
    "queued",
    "active",
    "in_progress",
    "blocked",
    "waiting",
    "review",
    "merge_ready",
    "decomposing",
    "todo",
    "pending",
]);

export const MUTED_FILL = "#e8eaed";
export const MUTED_TEXT = "#9ca3af";
