// Central focus-score → display size mapping.
// All views use this so a task's visual weight stays consistent across
// layouts. t = (focus / maxFocus)^exponent; size = min + (max - min) * t.
// Exponent < 1 compresses differences (sqrt = gentle), > 1 exaggerates them.

export const FOCUS_SIZE_EXPONENT = 1.5;

export function maxFocusOf(nodes: Array<{ focusScore?: number }>): number {
    let m = 0;
    for (const n of nodes) {
        const f = n.focusScore || 0;
        if (f > m) m = f;
    }
    return m;
}

export function focusSize(
    focusScore: number | undefined | null,
    maxFocus: number,
    minSize: number,
    maxSize: number,
    exponent: number = FOCUS_SIZE_EXPONENT,
): number {
    if (maxFocus <= 0) return minSize;
    const ratio = Math.max(0, focusScore || 0) / maxFocus;
    const t = Math.pow(ratio, exponent);
    return minSize + (maxSize - minSize) * t;
}
