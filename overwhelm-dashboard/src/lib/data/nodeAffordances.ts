// Categorical node affordances — borders/rings that signal a discrete state
// (catastrophic severity, focus pick, blocked, you-are-here) rather than the
// continuous focus-emphasis axis in focusEmphasis.ts.
//
// Single source of truth for the colours, widths and dash patterns. Views
// still draw their own SVG since shape (rect vs circle) varies, but every
// site reads style values from here — no inline magic.
//
// No animations. Pulsing borders proved distracting and hostile to focus
// on dense graphs; static rings convey the same information without motion.

/** SEV3/SEV4 catastrophic obligations (multi-parent §1.2). Red, dashed. */
export const CATASTROPHIC = {
    color: '#ef4444',
    /** width as a function of node radius (circle) */
    widthForRadius: (r: number) => Math.max(2.5, r * 0.06),
    /** width for rect-style nodes */
    widthForRect: 4,
    dashCircle: '8,4',
    dashRect: '12,6',
} as const;

/** Top-N focus picks (graph.focus from server). Amber outer + lemon inner. */
export const FOCUS_PICK = {
    outerColor: '#f59e0b',
    outerWidth: 8,
    outerOpacity: 0.6,
    innerColor: '#fbbf24',
    innerWidth: 3,
} as const;

/** Blocked status — pink dashed ring for fast scanability. */
export const BLOCKED = {
    color: '#ff8797',
    /** width as a function of node radius */
    widthForRadius: (r: number) => Math.max(1.1, r * 0.022),
    dash: '5,3',
    opacity: 0.82,
} as const;

/** "You are here" — current/in-progress task on a metro line. */
export const YOU_ARE_HERE = {
    color: '#fde68a',
    width: 2,
    opacity: 0.8,
    /** ring offset above the stop radius */
    radiusPad: 6,
} as const;
