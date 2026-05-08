// Single source of truth for visual emphasis derived from focus_score.
//
// Spec: PKB doc `multi-parent` — focus_score is the canonical ranking signal;
// component fields (urgency, downstream_weight, criticality) are diagnostic.
// We mirror that here: one normalised prominence number drives three
// orthogonal visual channels (size, opacity, saturation). All views call
// these helpers — no view should invent its own focus-driven dimming curve.
//
// Magic numbers live here, named, in one place.

import { MUTED_FILL, MUTED_TEXT } from './constants';

// SEV4-committed obligations score 100k+ on focus_score, dwarfing the
// 0–30k band that ordinary tasks occupy. Clamping to NOMINAL_MAX prevents
// a single catastrophic node from compressing every other task into the
// "barely visible" bucket. SEV4 nodes still get a distinct border + glow
// treatment via `isCatastrophic` — they don't need extra size/opacity to
// stand out.
export const FOCUS_NOMINAL_MAX = 30000;

// Size: prominence drives a 1.0× → 1.6× multiplier on the type-derived
// base scale. Gentle by design — type and content already drive size more
// than focus does, and we don't want SEV4 outliers consuming the canvas.
export const FOCUS_SCALE_GAIN = 0.6;

// Opacity floor for low-focus alive nodes. Below this and the rest of the
// graph stops being legible — the spec's "surface, don't gate" principle.
export const FOCUS_OPACITY_FLOOR = 0.7;

// Maximum desaturation of fill colour for zero-focus nodes. Higher = more
// dramatic emphasis, but past ~0.4 colours become indistinguishable.
export const FOCUS_DESAT_MAX = 0.25;

// Multiplier for nodes hidden behind a "half" filter visibility. Default
// "half" dims most of the graph (P2-P4 are half by default), so this needs
// to stay readable — ~0.7 contrasts P0/P1 without burying the rest.
export const VISIBILITY_HALF_OPACITY = 0.7;

// Multiplier for completed (done/cancelled) nodes. They stay visible but
// recede so active work dominates.
export const COMPLETION_OPACITY = 0.5;

// Multiplier for nodes outside the current selection's neighbourhood.
// Strong dim — when something is selected, everything else should clearly
// recede. Wins over the filter axis because it's irrelevant once the user
// has signalled "I'm looking at this branch right now".
export const SELECTION_MASK_OPACITY = 0.18;

// Curve shape for focusSize. <1 compresses differences (favours readability
// of long tail), >1 exaggerates the head. 1.5 chosen to match prior behaviour.
export const FOCUS_SIZE_EXPONENT = 1.5;

// Recency staleness — small additional desat for nodes nobody has touched
// in a while. Cap is small (5%) so it whispers rather than shouts; staleness
// is a hint, not an emphasis signal.
export const STALENESS_DESAT_MAX = 0.05;
const STALENESS_DAYS_FULL = 60; // days at which staleness desat reaches max

/**
 * Normalised focus signal in [0, 1]. Single source of "how prominent should
 * this node be?" — every emphasis channel is a pure function of this.
 *
 * We use log1p compression because focus_score ranges over 4+ orders of
 * magnitude (1 → 30 000+) and we want the head and tail to both be
 * distinguishable. Clamped to FOCUS_NOMINAL_MAX so SEV4 outliers don't
 * crush the rest of the distribution.
 */
export function focusProminence(focusScore: number | null | undefined, maxFocus: number): number {
    const f = Math.max(0, focusScore || 0);
    if (maxFocus <= 0) return 0;
    const cap = Math.min(Math.max(maxFocus, 1), FOCUS_NOMINAL_MAX);
    return Math.min(Math.log1p(f) / Math.log1p(cap), 1);
}

/** Maximum focus_score in a node set — needed for prominence normalisation. */
export function maxFocusOf(nodes: Array<{ focusScore?: number }>): number {
    let m = 0;
    for (const n of nodes) {
        const f = n.focusScore || 0;
        if (f > m) m = f;
    }
    return m;
}

/**
 * Size mapping from focus_score directly to a [min, max] pixel range.
 * Used by views that size by focus alone (Force, CirclePack, Treemap leaves).
 * Power curve gives finer differentiation at the high end.
 */
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

/**
 * Multiplier on a base node size from prominence. Used by views whose size
 * is primarily driven by content/type (the SVG card views) and want focus
 * as a secondary modifier rather than the dominant signal.
 */
export function focusScaleMultiplier(prominence: number): number {
    return 1 + prominence * FOCUS_SCALE_GAIN;
}

/**
 * The single canonical opacity calculation. Composes axes multiplicatively
 * with one exception: when `selectionMasked` is set, it dominates and the
 * filter axis is silenced — the user has actively narrowed attention.
 *
 *   1. Prominence — low focus → opaque-but-receded (floor at FLOOR).
 *   2. Completion — done/cancelled nodes recede further.
 *   3. Filter visibility — `half` filter state pushes opacity down;
 *      `hidden` returns 0.
 *   4. Selection mask — nodes outside the selected neighbourhood recede.
 *
 * Focus picks are signalled via rings, not opacity — keeping it off the
 * opacity axis avoids double-dimming non-picks that are also filter-half.
 */
export function emphasisOpacity(opts: {
    prominence: number;
    isCompleted?: boolean;
    visibilityState?: 'bright' | 'half' | 'hidden';
    selectionMasked?: boolean;
}): number {
    if (opts.visibilityState === 'hidden') return 0;

    // Prominence opacity: linear from FLOOR up to 1.0
    let op = FOCUS_OPACITY_FLOOR + (1 - FOCUS_OPACITY_FLOOR) * opts.prominence;

    if (opts.isCompleted) op *= COMPLETION_OPACITY;

    if (opts.selectionMasked) {
        op *= SELECTION_MASK_OPACITY;
    } else if (opts.visibilityState === 'half') {
        op *= VISIBILITY_HALF_OPACITY;
    }

    return op;
}

/**
 * Build the set of nodes that count as "in focus" given an active selection.
 * Includes the active node, its edge-connected neighbours, and its ancestor
 * + descendant chain. For force/arc layouts also includes parent-siblings,
 * since spatial co-location there reads as related.
 *
 * Returns null when nothing is selected — callers can short-circuit.
 */
export function computeSelectionMask<
    N extends { id: string; parent?: string | null },
    L extends { source: string | { id: string }; target: string | { id: string } },
>(
    nodes: N[],
    links: L[],
    activeId: string | null | undefined,
    opts: { includeSiblings?: boolean } = {},
): Set<string> | null {
    if (!activeId) return null;
    const mask = new Set<string>([activeId]);

    for (const l of links) {
        const sid = typeof l.source === 'object' ? l.source.id : l.source;
        const tid = typeof l.target === 'object' ? l.target.id : l.target;
        if (sid === activeId) mask.add(tid);
        if (tid === activeId) mask.add(sid);
    }

    const parentMap = new Map<string, string>();
    for (const n of nodes) if (n.parent) parentMap.set(n.id, n.parent);

    let cur = parentMap.get(activeId);
    while (cur) { mask.add(cur); cur = parentMap.get(cur); }

    for (const n of nodes) {
        let c = parentMap.get(n.id);
        while (c) {
            if (c === activeId) { mask.add(n.id); break; }
            c = parentMap.get(c);
        }
    }

    if (opts.includeSiblings) {
        const activeParent = parentMap.get(activeId);
        if (activeParent) {
            for (const n of nodes) if (n.parent === activeParent) mask.add(n.id);
        }
    }

    return mask;
}

/** Hex colour utilities (dup'd minimal versions; full ones in prepareGraphData). */
function hexToRgb(hex: string): [number, number, number] {
    const h = hex.replace(/^#/, '');
    if (h.length === 3) {
        return [
            parseInt(h[0] + h[0], 16),
            parseInt(h[1] + h[1], 16),
            parseInt(h[2] + h[2], 16),
        ];
    }
    return [
        parseInt(h.substring(0, 2), 16),
        parseInt(h.substring(2, 4), 16),
        parseInt(h.substring(4, 6), 16),
    ];
}

function rgbToHex(r: number, g: number, b: number): string {
    return '#' + [r, g, b].map(x => {
        const h = Math.round(x).toString(16);
        return h.length === 1 ? '0' + h : h;
    }).join('');
}

export function interpolateColor(colorA: string, colorB: string, t: number): string {
    const tt = Math.max(0, Math.min(1, t));
    const [ra, ga, ba] = hexToRgb(colorA);
    const [rb, gb, bb] = hexToRgb(colorB);
    return rgbToHex(
        ra + (rb - ra) * tt,
        ga + (gb - ga) * tt,
        ba + (bb - ba) * tt,
    );
}

function stalenessDesat(staleDays: number | undefined): number {
    if (!staleDays || staleDays <= 7) return 0;
    const t = Math.min((staleDays - 7) / (STALENESS_DAYS_FULL - 7), 1);
    return STALENESS_DESAT_MAX * t;
}

/**
 * Blend a status fill toward the muted neutral. Combines prominence-driven
 * desat (the dominant axis) with a small staleness bump. Single pass — no
 * stacking of multiple desat curves.
 */
export function emphasisFill(
    baseFill: string,
    prominence: number,
    opts: { staleDays?: number } = {},
): string {
    const desat = (1 - prominence) * FOCUS_DESAT_MAX + stalenessDesat(opts.staleDays);
    return interpolateColor(baseFill, MUTED_FILL, Math.min(desat, 1));
}

export function emphasisTextColor(
    baseText: string,
    prominence: number,
    opts: { staleDays?: number } = {},
): string {
    const desat = (1 - prominence) * FOCUS_DESAT_MAX + stalenessDesat(opts.staleDays);
    return interpolateColor(baseText, MUTED_TEXT, Math.min(desat, 1));
}
