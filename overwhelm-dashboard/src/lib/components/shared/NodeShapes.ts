import * as d3 from 'd3';
import type { GraphNode } from '../../data/prepareGraphData';
import { projectHue } from '../../data/projectUtils';
import { INCOMPLETE_STATUSES, PRIORITY_BORDERS as SHARED_PRIORITY_BORDERS, STATUS_FILLS, STRUCTURAL_TYPES } from '../../data/constants';

function escapeHtml(str: string): string {
    return str
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
}

function estimateTextWidth(text: string, fontSize: number): number {
    // Increased from 0.51 to 0.62 to better estimate width for modern, bold sans-serif fonts
    return text.length * fontSize * 0.62;
}

function wrapWordsToWidth(label: string, fontSize: number, maxWidth: number): string[] {
    const words = label.split(/\s+/).filter(Boolean);
    if (words.length === 0) return [];

    const lines: string[] = [];
    let current = '';

    for (const word of words) {
        const test = current ? `${current} ${word}` : word;
        if (current && estimateTextWidth(test, fontSize) > maxWidth) {
            lines.push(current);
            current = word;
        } else {
            current = test;
        }
    }

    if (current) lines.push(current);
    return lines;
}

function clampWrappedLines(lines: string[], fontSize: number, maxWidth: number, maxLines: number): string[] {
    if (lines.length <= maxLines) return lines;

    const truncated = lines.slice(0, maxLines);
    while (
        truncated[maxLines - 1] &&
        estimateTextWidth(`${truncated[maxLines - 1]}...`, fontSize) > maxWidth &&
        truncated[maxLines - 1].includes(' ')
    ) {
        truncated[maxLines - 1] = truncated[maxLines - 1].split(' ').slice(0, -1).join(' ');
    }
    truncated[maxLines - 1] = `${truncated[maxLines - 1]}...`;
    return truncated;
}

function fitTreemapText(
    label: string,
    maxWidth: number,
    maxHeight: number,
    options: { minFontSize?: number; maxFontSize?: number; maxLines?: number } = {},
) {
    const minFontSize = options.minFontSize ?? 5;
    const maxFontSize = options.maxFontSize ?? 14;
    const maxLines = options.maxLines ?? 3;
    const effectiveWidth = maxWidth * 1.06;

    if (!label.trim() || maxWidth <= 0 || maxHeight <= 0) {
        return { fontSize: minFontSize, lineHeight: minFontSize * 1.18, lines: [] as string[] };
    }

    const longestWord = label.split(/\s+/).reduce((a, b) => a.length > b.length ? a : b, '');
    for (let fontSize = maxFontSize; fontSize >= minFontSize; fontSize -= 0.5) {
        if (estimateTextWidth(longestWord, fontSize) > maxWidth * 1.06) continue;
        const lines = wrapWordsToWidth(label, fontSize, effectiveWidth);
        const lineHeight = fontSize * 1.18;
        if (lines.length <= maxLines && lines.length * lineHeight <= maxHeight) {
            return { fontSize, lineHeight, lines };
        }
    }

    const fallbackSize = minFontSize;
    const fallbackHeight = fallbackSize * 1.18;
    const fallbackLines = clampWrappedLines(
        wrapWordsToWidth(label, fallbackSize, effectiveWidth),
        fallbackSize,
        effectiveWidth,
        maxLines,
    );
    return { fontSize: fallbackSize, lineHeight: fallbackHeight, lines: fallbackLines };
}

// Status is encoded via fill color + text color, not opacity.
// Opacity is reserved for interactive states (hover flashlight, selection).

// Epic styling constants
const EPIC_SCALE = 1.3;
const EPIC_CORNER_RADIUS = 16;
const EPIC_OUTER_OFFSET = 3;
const EPIC_OUTER_CORNER_RADIUS = 18;
const EPIC_OUTER_STROKE_WIDTH = 1;
const EPIC_OUTER_OPACITY = 0.3;
const EPIC_OUTER_DASHARRAY = "4,2";
const EPIC_INNER_STROKE_WIDTH = 2.5;
const EPIC_INNER_FILL_OPACITY = 0.7;
const EPIC_INNER_STROKE_OPACITY = 0.6;
const EPIC_BADGE_Y_OFFSET = -6;
const EPIC_BADGE_FONT_SIZE = 7;
const EPIC_BADGE_LETTER_SPACING = "1.5px";

export function buildTaskCardNode(g: d3.Selection<SVGGElement, GraphNode, null, undefined>, d: GraphNode, isSelected = false) {
    const hw = d.w / 2;
    const hh = d.h / 2;

    // Add native tooltip to all nodes
    g.append("title").text(`${d.label} (${d.status})\nPriority: P${d.priority}\nProject: ${d.project || 'None'}`);

    if (isSelected) {
        g.classed("selected-node", true);
    } else {
        g.classed("selected-node", false);
    }

    // P0/P1 priority glow ring for incomplete nodes
    const isIncomplete = INCOMPLETE_STATUSES.has(d.status);
    if (d.priority <= 1 && isIncomplete) {
        const glowPad = d.priority === 0 ? 4 : 3;
        const glowFilter = d.priority === 0 ? 'url(#glow-p0)' : 'url(#glow-p1)';
        const glowColor = d.priority === 0 ? '#dc3545' : '#f59e0b';
        const rx = d.shape === 'pill' ? hh + glowPad : (d.shape === 'rounded' ? 14 : 6);
        g.insert("rect", ":first-child")
            .attr("x", -hw - glowPad).attr("y", -hh - glowPad)
            .attr("width", d.w + glowPad * 2).attr("height", d.h + glowPad * 2)
            .attr("rx", rx).attr("fill", "none")
            .attr("stroke", glowColor).attr("stroke-width", 1.5)
            .attr("stroke-opacity", 0.38)
            .attr("filter", glowFilter);
    }

    if (d.spotlight && d.isLeaf) {
        const pad = 9;
        const rx = d.shape === 'pill' ? hh + pad : (d.shape === 'rounded' ? 14 : 8);
        g.insert("rect", ":first-child")
            .attr("x", -hw - pad).attr("y", -hh - pad)
            .attr("width", (hw + pad) * 2).attr("height", (hh + pad) * 2)
            .attr("rx", rx).attr("fill", "none")
            .attr("stroke", "#f59e0b").attr("class", "spotlight-ring");
        g.append("text")
            .attr("x", 0).attr("y", -hh - pad - 5)
            .attr("text-anchor", "middle").attr("font-size", "8px")
            .attr("font-weight", "700").attr("fill", "#f59e0b")
            .attr("letter-spacing", "0.6px").attr("pointer-events", "none")
            .text("★ START HERE");
    }

    if (d.shape === "pill") {
        g.append("rect").attr("x", -hw).attr("y", -hh).attr("width", d.w).attr("height", d.h)
            .attr("class", "node-surface")
            .attr("rx", hh).attr("ry", hh)
            .attr("fill", d.fill).attr("stroke", d.borderColor).attr("stroke-width", d.borderWidth);
    } else if (d.shape === "hexagon") {
        // Epics: distinctive double-border hexagon with larger size and type badge
        // Color by unique node ID so every epic has a distinct hue
        const hue = projectHue(d.id);
        const epicFill = `hsl(${hue}, 35%, 85%)`;
        const epicStroke = `hsl(${hue}, 45%, 45%)`;

        const sw = d.w, sh = d.h;
        const shw = sw / 2, shh = sh / 2;
        const c = Math.min(shh * 0.5, EPIC_CORNER_RADIUS);
        const pts = `${-shw + c},${-shh} ${shw - c},${-shh} ${shw},${0} ${shw - c},${shh} ${-shw + c},${shh} ${-shw},${0}`;

        // Outer glow hexagon
        const off = EPIC_OUTER_OFFSET;
        const c2 = Math.min((shh + off) * 0.5, EPIC_OUTER_CORNER_RADIUS);
        const pts2 = `${-(shw + off) + c2},${-(shh + off)} ${(shw + off) - c2},${-(shh + off)} ${shw + off},${0} ${(shw + off) - c2},${shh + off} ${-(shw + off) + c2},${shh + off} ${-(shw + off)},${0}`;

        g.append("polygon").attr("points", pts2)
            .attr("fill", "none").attr("stroke", epicStroke).attr("stroke-width", EPIC_OUTER_STROKE_WIDTH)
            .attr("stroke-opacity", EPIC_OUTER_OPACITY).attr("stroke-dasharray", EPIC_OUTER_DASHARRAY);

        g.append("polygon").attr("class", "node-surface").attr("points", pts)
            .attr("fill", epicFill).attr("stroke", epicStroke).attr("stroke-width", Math.max(d.borderWidth, EPIC_INNER_STROKE_WIDTH));

        // Epic type badge at top
        g.append("text")
            .attr("x", 0).attr("y", -shh + EPIC_BADGE_Y_OFFSET)
            .attr("text-anchor", "middle").attr("font-size", `${EPIC_BADGE_FONT_SIZE}px`)
            .attr("font-weight", "800").attr("fill", epicStroke)
            .attr("letter-spacing", EPIC_BADGE_LETTER_SPACING).attr("pointer-events", "none")
            .text("EPIC");
    } else {
        g.append("rect").attr("x", -hw).attr("y", -hh).attr("width", d.w).attr("height", d.h)
            .attr("class", "node-surface")
            .attr("rx", d.shape === "rounded" ? 10 : 4)
            .attr("fill", d.fill).attr("stroke", d.borderColor).attr("stroke-width", d.borderWidth);
    }

    if (d.status === "blocked") {
        g.insert("rect", ":first-child")
            .attr("x", -hw - 3).attr("y", -hh - 3).attr("width", d.w + 6).attr("height", d.h + 6)
            .attr("rx", hh + 3).attr("ry", hh + 3).attr("fill", "none")
            .attr("stroke", "#ff7588")
            .attr("stroke-width", 2)
            .attr("stroke-opacity", 0.8)
            .attr("stroke-dasharray", "6,3");

        g.append("rect")
            .attr("x", -hw).attr("y", -hh)
            .attr("width", Math.min(5, d.w)).attr("height", d.h)
            .attr("rx", 2)
            .attr("fill", "#ff7588")
            .attr("opacity", 0.9);

        if (d.w > 86 && d.h > 28) {
            g.append("rect")
                .attr("x", hw - 58).attr("y", -hh + 4)
                .attr("width", 54).attr("height", 14)
                .attr("rx", 7)
                .attr("fill", "rgba(106,49,66,0.95)")
                .attr("stroke", "#ff9cab")
                .attr("stroke-width", 0.8);
            g.append("text")
                .attr("x", hw - 31).attr("y", -hh + 11)
                .attr("text-anchor", "middle")
                .attr("dominant-baseline", "central")
                .attr("font-size", "7px")
                .attr("font-weight", "800")
                .attr("letter-spacing", "0.08em")
                .attr("fill", "#ffe4e8")
                .text("BLOCKED");
        }
    }

    const lh = d.fontSize + 4;
    const ty = -(d.lines.length * lh) / 2 + d.fontSize * 0.38 + (d.badge ? 6 : 0);

    d.lines.forEach((line, i) => {
        g.append("text").attr("class", "node-text")
            .attr("x", 0).attr("y", ty + i * lh)
            .attr("text-anchor", "middle").attr("dominant-baseline", "central")
            .attr("font-size", d.fontSize + "px")
            .attr("fill", d.textColor).text(line);
    });

    if (d.dw > 0) {
        const tw = d.dw.toFixed(1).length * 6 + 16;
        g.append("rect")
            .attr("x", -tw / 2).attr("y", hh + 4)
            .attr("width", tw).attr("height", 15)
            .attr("rx", 7).attr("fill", d.borderColor).attr("opacity", 0.15);
        g.append("text")
            .attr("class", "node-badge").attr("x", 0).attr("y", hh + 14.5)
            .attr("text-anchor", "middle").attr("font-size", "8px")
            .attr("fill", d.borderColor).text("⚖ " + d.dw.toFixed(1));
    }
}

/**
 * Compute header height for a treemap container label.
 * Shared between layout (paddingTop) and rendering (header bar + label).
 * Returns { headerH, fs, lines, badgeReserve } so both sides stay in sync.
 */
export function treemapHeaderMetrics(w: number, h: number, label: string, depth: number) {
    const pad = 6;
    const badgeReserve = w > 56 ? 32 : 0;
    const textAvailW = Math.max(18, w - pad * 2 - badgeReserve);
    const minReadableWidth = depth <= 1 ? 64 : 50;
    const minReadableHeight = depth <= 1 ? 24 : 18;
    if (w < minReadableWidth || h < minReadableHeight) {
        return {
            headerH: 0,
            fs: 0,
            lines: 0,
            labelLines: [] as string[],
            lineHeight: 0,
            badgeReserve,
            pad,
        };
    }

    const maxLines = depth <= 1 ? 3 : 2;
    const maxFontSize = Math.max(8, Math.min(w * 0.21, h * 0.28));
    const fitted = fitTreemapText(label, textAvailW, Math.max(20, Math.min(depth <= 1 ? 64 : 44, h * (depth <= 1 ? 0.42 : 0.28))), {
        minFontSize: 6,
        maxFontSize,
        maxLines,
    });
    if (fitted.fontSize < (depth <= 1 ? 8 : 6.5)) {
        return {
            headerH: 0,
            fs: fitted.fontSize,
            lines: 0,
            labelLines: [] as string[],
            lineHeight: fitted.lineHeight,
            badgeReserve,
            pad,
        };
    }
    const basePad = depth <= 1 ? 11 : 9;
    const headerH = Math.max(15, Math.min(depth <= 1 ? 64 : 48, fitted.lines.length * fitted.lineHeight + basePad));
    return {
        headerH,
        fs: fitted.fontSize,
        lines: fitted.lines.length,
        labelLines: fitted.lines,
        lineHeight: fitted.lineHeight,
        badgeReserve,
        pad,
    };
}

/** Render a child-count badge in top-right of a container: visible/total leaf descendants */
function renderCountBadge(
    g: d3.Selection<SVGGElement, any, null, undefined>,
    w: number, h: number, hue: number,
    leafCount: number, totalLeafCount: number, fontSize: number
) {
    if ((leafCount <= 0 && totalLeafCount <= 0) || w < 50) return;
    const badgeFs = Math.max(6, Math.min(9, fontSize * 0.8));
    const hasHidden = totalLeafCount > leafCount;
    const badgeText = hasHidden ? `${leafCount}/${totalLeafCount}` : `${leafCount}`;
    const badgeW = badgeText.length * badgeFs * 0.55 + 10;
    const badgeH = badgeFs + 4;
    const badgeX = w / 2 - badgeW - 4;
    const badgeY = -h / 2 + 3;

    g.append("rect")
        .attr("x", badgeX).attr("y", badgeY)
        .attr("width", badgeW).attr("height", badgeH)
        .attr("rx", badgeH / 2)
        .attr("fill", hasHidden ? `hsla(${hue}, 35%, 18%, 0.9)` : `hsla(${hue}, 30%, 20%, 0.8)`)
        .attr("stroke", hasHidden ? `hsla(${hue}, 50%, 50%, 0.6)` : `hsla(${hue}, 40%, 45%, 0.5)`)
        .attr("stroke-width", 0.5)
        .style("pointer-events", "none");
    g.append("text")
        .attr("x", badgeX + badgeW / 2).attr("y", badgeY + badgeH / 2)
        .attr("text-anchor", "middle").attr("dominant-baseline", "central")
        .attr("font-size", badgeFs + "px").attr("font-weight", "600")
        .attr("font-family", "var(--font-mono), monospace")
        .attr("fill", hasHidden ? `hsl(${hue}, 50%, 75%)` : `hsl(${hue}, 40%, 70%)`)
        .style("pointer-events", "none")
        .text(badgeText);
}

export function buildTreemapNode(g: d3.Selection<SVGGElement, any, null, undefined>, d: any, isSelected = false) {
    // expects d._lw and d._lh to be populated by the layout algo if using true sizes, else uses d.w/d.h
    const w = d._lw || d.w;
    const h = d._lh || d.h;
    const isParent = !(d._isLeaf ?? d.isLeaf);
    const isMicroLeaf = !isParent && (w < 18 || h < 12 || w / Math.max(h, 1) < 0.22);


    if (isSelected) {
        g.classed("selected-node", true);
    } else {
        g.classed("selected-node", false);
    }

    // Base color logic: Node-specific Hue
    const hue = projectHue(d.id);

    // Determine visual tier
    const isEpicTier = !d._isProjectContainer && !d._isOverflow && isParent
        && STRUCTURAL_TYPES.has(d.type);

    // ── TIER 1: Project containers — explicit bounded regions ──
    if (d._isProjectContainer) {
        const bgTint = `hsl(${hue}, 24%, 12%)`;
        const borderColor = `hsl(${hue}, 62%, 58%)`;
        const labelColor = `hsl(${hue}, 88%, 92%)`;

        // Solid dark background — makes project region clearly distinct from canvas
        g.append("rect")
            .attr("class", "node-surface")
            .attr("x", -w / 2).attr("y", -h / 2).attr("width", w).attr("height", h)
            .attr("rx", 10)
            .attr("fill", bgTint).attr("fill-opacity", 0.82)
            .attr("stroke", isSelected ? "#fff" : borderColor)
            .attr("stroke-width", isSelected ? 3 : 2.2)
            .style("transition", "all 0.2s ease");

        // Inner inset line for extra boundary definition
        g.append("rect")
            .attr("x", -w / 2 + 3).attr("y", -h / 2 + 3)
            .attr("width", Math.max(0, w - 6)).attr("height", Math.max(0, h - 6))
            .attr("rx", 8)
            .attr("fill", "none")
            .attr("stroke", borderColor).attr("stroke-width", 0.7).attr("stroke-opacity", 0.45);

        // Project label — uses shared header metrics for consistent sizing
        if (w > 30 && h > 20) {
            const label = d.label || '';
            const m = treemapHeaderMetrics(w, h, label, d.depth || 0);
            if (m.headerH <= 0 || m.labelLines.length === 0) {
                return;
            }
            const labelW = Math.max(0, w - m.pad * 2 - m.badgeReserve);
            const labelHtml = m.labelLines.map((line: string) => escapeHtml(line)).join('<br/>');

            // Label background bar
            g.append("rect")
                .attr("x", -w / 2).attr("y", -h / 2)
                .attr("width", w).attr("height", m.headerH)
                .attr("rx", 4)
                .attr("fill", bgTint).attr("fill-opacity", 0.96);

            // HR divider at bottom of header
            g.append("line")
                .attr("x1", -w / 2 + 6).attr("x2", w / 2 - 6)
                .attr("y1", -h / 2 + m.headerH).attr("y2", -h / 2 + m.headerH)
                .attr("stroke", borderColor).attr("stroke-width", 0.5).attr("stroke-opacity", 0.5);

            g.append("foreignObject")
                .attr("x", -w / 2 + m.pad).attr("y", -h / 2 + 1)
                .attr("width", labelW).attr("height", m.headerH - 2)
                .style("pointer-events", "none")
                .append("xhtml:div")
                .style("pointer-events", "none")
                .html(`<div style="font-size:${m.fs}px; font-weight:820; color:${labelColor}; text-transform:none; letter-spacing:0.01em; line-height:${m.lineHeight}px; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; font-family:var(--font-display), sans-serif;">${labelHtml}</div>`);

            renderCountBadge(g, w, h, hue, d._leafCount || 0, d.totalLeafCount || 0, m.fs);
        }
        return;
    }

    // ── TIER 2: Epics/Goals — medium tinted rectangles with all-caps titles ──
    if (isEpicTier) {
        const epicTint = `hsl(${hue}, 34%, 22%)`;
        g.append("rect")
            .attr("class", "node-surface")
            .attr("x", -w / 2).attr("y", -h / 2).attr("width", w).attr("height", h)
            .attr("rx", 5)
            .attr("fill", epicTint).attr("fill-opacity", 0.72)
            .attr("stroke", `hsl(${hue}, 54%, 58%)`).attr("stroke-width", isSelected ? 3 : 1.5)
            .style("transition", "all 0.2s ease");

        // Epic label — uses shared header metrics
        if (w > 30 && h > 16) {
            const label = d.label || '';
            const m = treemapHeaderMetrics(w, h, label, d.depth || 0);
            if (m.headerH <= 0 || m.labelLines.length === 0) {
                return;
            }
            const labelW = Math.max(0, w - m.pad * 2 - m.badgeReserve);
            const labelHtml = m.labelLines.map((line: string) => escapeHtml(line)).join('<br/>');

            // HR divider at bottom of header
            if (w > 40 && h > 30) {
                g.append("line")
                    .attr("x1", -w / 2 + 6).attr("x2", w / 2 - 6)
                    .attr("y1", -h / 2 + m.headerH).attr("y2", -h / 2 + m.headerH)
                    .attr("stroke", `hsl(${hue}, 30%, 40%)`).attr("stroke-width", 0.5)
                    .attr("stroke-opacity", 0.6);
            }

            g.append("foreignObject")
                .attr("x", -w / 2 + m.pad).attr("y", -h / 2 + 1)
                .attr("width", labelW).attr("height", m.headerH - 2)
                .style("pointer-events", "none")
                .append("xhtml:div")
                .style("pointer-events", "none")
                .html(`<div style="font-size:${m.fs}px; font-weight:780; color:hsl(${hue},82%,90%); text-transform:none; letter-spacing:0.005em; line-height:${m.lineHeight}px; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; font-family:var(--font-display), sans-serif;">${labelHtml}</div>`);

            renderCountBadge(g, w, h, hue, d._leafCount || 0, d.totalLeafCount || 0, m.fs);
        }
        return;
    }

    if (d._isOverflow) {
        const rollupLabel = d.label || 'Other Tasks';
        const rollupCount = d._rollupCount ?? d.totalLeafCount ?? d._leafCount ?? 0;
        const bgTint = `hsl(${hue}, 28%, 14%)`;
        const borderColor = `hsl(${hue}, 34%, 50%)`;
        const labelColor = `hsl(${hue}, 68%, 88%)`;
        const isRibbon = h < 30;

        g.append("rect")
            .attr("class", "node-surface")
            .attr("x", -w / 2).attr("y", -h / 2).attr("width", w).attr("height", h)
            .attr("rx", 6)
            .attr("fill", bgTint).attr("fill-opacity", 0.96)
            .attr("stroke", isSelected ? "#fff" : borderColor)
            .attr("stroke-width", isSelected ? 3 : 1.4)
            .attr("stroke-dasharray", "5,3")
            .style("transition", "all 0.2s ease");

        g.append("rect")
            .attr("x", -w / 2 + 3).attr("y", -h / 2 + 3)
            .attr("width", Math.max(0, w - 6)).attr("height", Math.max(0, h - 6))
            .attr("rx", 4)
            .attr("fill", "none")
            .attr("stroke", borderColor)
            .attr("stroke-width", 0.8)
            .attr("stroke-opacity", 0.28);

        if (isRibbon) {
            const pillW = Math.max(34, Math.min(52, w * 0.18));
            const labelFit = fitTreemapText(
                rollupLabel,
                Math.max(0, w - pillW - 18),
                Math.max(0, h - 6),
                { minFontSize: 5, maxFontSize: Math.max(7, Math.min(11, h * 0.52)), maxLines: 1 },
            );
            const labelHtml = labelFit.lines.map((line: string) => escapeHtml(line)).join('<br/>');

            g.append("foreignObject")
                .attr("x", -w / 2 + 6)
                .attr("y", -h / 2 + 2)
                .attr("width", Math.max(0, w - pillW - 14))
                .attr("height", Math.max(0, h - 4))
                .style("pointer-events", "none")
                .append("xhtml:div")
                .style("display", "flex")
                .style("align-items", "center")
                .style("width", "100%")
                .style("height", "100%")
                .style("overflow", "hidden")
                .style("pointer-events", "none")
                .html(`<div style="font-size:${labelFit.fontSize}px; font-weight:740; color:${labelColor}; text-transform:none; letter-spacing:0.01em; line-height:${labelFit.lineHeight}px; overflow:hidden; white-space:nowrap; font-family:var(--font-display), sans-serif;">${labelHtml}</div>`);

            g.append("rect")
                .attr("x", w / 2 - pillW - 6)
                .attr("y", -h / 2 + 2)
                .attr("width", pillW)
                .attr("height", Math.max(0, h - 4))
                .attr("rx", Math.max(8, (h - 4) / 2))
                .attr("fill", `hsla(${hue}, 24%, 9%, 0.92)`)
                .attr("stroke", `hsla(${hue}, 56%, 66%, 0.72)`)
                .attr("stroke-width", 1);

            g.append("text")
                .attr("x", w / 2 - pillW / 2 - 6)
                .attr("y", 0)
                .attr("text-anchor", "middle")
                .attr("dominant-baseline", "central")
                .attr("font-size", `${Math.max(9, Math.min(13, h * 0.62))}px`)
                .attr("font-weight", "800")
                .attr("font-family", "var(--font-mono), monospace")
                .attr("fill", "rgba(255,255,255,0.94)")
                .text(String(rollupCount));
        } else {
            const headerFit = fitTreemapText(
                rollupLabel,
                Math.max(0, w - 12),
                Math.max(18, Math.min(54, h * 0.34)),
                { minFontSize: 5, maxFontSize: Math.max(8, Math.min(16, Math.min(w * 0.18, h * 0.18))), maxLines: 3 },
            );
            const headerH = headerFit.lines.length > 0 ? Math.max(16, Math.min(56, headerFit.lines.length * headerFit.lineHeight + 10)) : 0;
            if (headerH > 0) {
                const labelHtml = headerFit.lines.map((line: string) => escapeHtml(line)).join('<br/>');
                g.append("rect")
                    .attr("x", -w / 2).attr("y", -h / 2)
                    .attr("width", w).attr("height", headerH)
                    .attr("rx", 4)
                    .attr("fill", bgTint)
                    .attr("fill-opacity", 1);

                g.append("line")
                    .attr("x1", -w / 2 + 6).attr("x2", w / 2 - 6)
                    .attr("y1", -h / 2 + headerH).attr("y2", -h / 2 + headerH)
                    .attr("stroke", borderColor)
                    .attr("stroke-width", 0.6)
                    .attr("stroke-opacity", 0.55);

                g.append("foreignObject")
                    .attr("x", -w / 2 + 6).attr("y", -h / 2 + 1)
                    .attr("width", Math.max(0, w - 12)).attr("height", headerH - 2)
                    .style("pointer-events", "none")
                    .append("xhtml:div")
                    .style("pointer-events", "none")
                    .html(`<div style="font-size:${headerFit.fontSize}px; font-weight:760; color:${labelColor}; text-transform:none; letter-spacing:0.01em; line-height:${headerFit.lineHeight}px; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; font-family:var(--font-display), sans-serif;">${labelHtml}</div>`);
            }

            const countBadgeW = Math.max(42, Math.min(w * 0.45, 72));
            const countBadgeH = Math.max(34, Math.min(h * 0.32, 54));
            const countFontSize = Math.max(16, Math.min(28, Math.min(w, h) * 0.18));
            const subFontSize = Math.max(8, Math.min(11, countFontSize * 0.42));
            const centerY = (headerH > 0 ? -h / 2 + headerH : -h / 2) + Math.max(0, h - headerH) / 2;

            g.append("rect")
                .attr("x", -countBadgeW / 2)
                .attr("y", centerY - countBadgeH / 2)
                .attr("width", countBadgeW)
                .attr("height", countBadgeH)
                .attr("rx", countBadgeH / 2)
                .attr("fill", `hsla(${hue}, 24%, 9%, 0.92)`)
                .attr("stroke", `hsla(${hue}, 56%, 66%, 0.72)`)
                .attr("stroke-width", 1.1);

            g.append("text")
                .attr("x", 0)
                .attr("y", centerY - 3)
                .attr("text-anchor", "middle")
                .attr("dominant-baseline", "central")
                .attr("font-size", `${countFontSize}px`)
                .attr("font-weight", "800")
                .attr("font-family", "var(--font-mono), monospace")
                .attr("fill", "rgba(255,255,255,0.94)")
                .text(String(rollupCount));

            g.append("text")
                .attr("x", 0)
                .attr("y", centerY + countBadgeH * 0.22)
                .attr("text-anchor", "middle")
                .attr("dominant-baseline", "central")
                .attr("font-size", `${subFontSize}px`)
                .attr("font-weight", "700")
                .attr("font-family", "var(--font-mono), monospace")
                .attr("letter-spacing", "0.08em")
                .attr("fill", "rgba(255,255,255,0.68)")
                .text(rollupCount === 1 ? 'TASK' : 'TASKS');
        }

        return;
    }

    // Priority border colors — only P0/P1 draw the eye
    // P0/P1 use shared PRIORITIES colors; P2+ are muted to blend with cards
    let cellColor: string;
    if (isParent) {
        // Parents get a unique hue based on their ID
        const hue = projectHue(d.id);
        cellColor = `hsl(${hue}, 48%, 24%)`;
    } else {
        const status = (d.status || 'inbox').toLowerCase();
        cellColor = STATUS_FILLS[status] || '#4b5563';
    }

    const priorityBorder = SHARED_PRIORITY_BORDERS[d.priority ?? 4] || '#64748b';

    // WCAG AA contrast: compute relative luminance and pick text color
    // that guarantees >= 4.5:1 contrast ratio
    function relativeLuminance(hex: string): number {
        if (!hex || !hex.startsWith('#') || hex.length < 7) return 0;
        const srgb = [hex.slice(1, 3), hex.slice(3, 5), hex.slice(5, 7)]
            .map(c => { const v = parseInt(c, 16) / 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); });
        return 0.2126 * srgb[0] + 0.7152 * srgb[1] + 0.0722 * srgb[2];
    }

    function getContrastColor(color: string): string {
        if (!color) return '#ffffff';
        // HSL: extract lightness
        if (color.startsWith('hsl')) {
            const m = color.match(/hsl\(\d+,\s*\d+%,\s*(\d+)%/);
            return (m && parseInt(m[1]) > 55) ? '#000000' : '#ffffff';
        }
        if (!color.startsWith('#')) return '#ffffff';
        const lum = relativeLuminance(color);
        // WCAG AA: contrast ratio >= 4.5:1
        // White text on bg: (1.05) / (lum + 0.05)
        // Black text on bg: (lum + 0.05) / (0.05)
        const whiteContrast = 1.05 / (lum + 0.05);
        const blackContrast = (lum + 0.05) / 0.05;
        return whiteContrast >= blackContrast ? '#ffffff' : '#1a1a1a';
    }

    const textColor = getContrastColor(cellColor);

    if (isMicroLeaf) {
        const compactW = Math.max(2, Math.min(w, 8));
        const compactH = Math.max(2, Math.min(h, 8));
        g.append("rect")
            .attr("class", "node-surface")
            .attr("x", -compactW / 2).attr("y", -compactH / 2)
            .attr("width", compactW).attr("height", compactH)
            .attr("rx", Math.min(2, compactH / 2))
            .attr("fill", cellColor)
            .attr("fill-opacity", 0.9)
            .attr("stroke", isSelected ? "#fff" : priorityBorder)
            .attr("stroke-width", isSelected ? 2 : 0.75);

        if (d.status === "blocked" && compactW >= 4) {
            g.append("rect")
                .attr("x", -compactW / 2)
                .attr("y", -compactH / 2)
                .attr("width", Math.min(2, compactW))
                .attr("height", compactH)
                .attr("rx", 1)
                .attr("fill", "#9B5555")
                .attr("opacity", 0.75);
        }

        return;
    }

    // Base solid background — status fill + priority border
    g.append("rect")
        .attr("class", "node-surface")
        .attr("x", -w / 2).attr("y", -h / 2).attr("width", w).attr("height", h)
        .attr("rx", 4)
        .attr("fill", cellColor).attr("fill-opacity", isParent ? 0.56 : 0.98)
        .attr("stroke", isSelected ? "#fff" : (isParent ? cellColor : priorityBorder))
        .attr("stroke-width", isSelected ? 3 : (d.priority <= 1 ? 2.1 : 1.35))
        .style("transition", "all 0.2s ease");

    if (isParent && h > 20) {
        // Parent Header Bar — uses shared metrics for consistent height
        const label = d.label || '';
        const m = treemapHeaderMetrics(w, h, label, d.depth || 0);
        if (m.headerH > 0) {
            g.append("rect")
                .attr("x", -w / 2).attr("y", -h / 2)
                .attr("width", w).attr("height", m.headerH)
                .attr("rx", 4)
                .attr("fill", cellColor).attr("fill-opacity", 0.96);
        }
    }

    // Grid overlay removed — status colors provide sufficient visual distinction
    // without the noise of overlaid patterns

    // Subtle blocked indicator — thin left border, not screaming red
    if (d.status === "blocked") {
        g.append("rect")
            .attr("x", -w / 2).attr("y", -h / 2).attr("width", 3).attr("height", h)
            .attr("rx", 1)
            .attr("fill", "#ff8192").attr("pointer-events", "none")
            .attr("opacity", 0.95);

        g.append("rect")
            .attr("x", -w / 2 + 1).attr("y", -h / 2 + 1)
            .attr("width", Math.max(0, w - 2)).attr("height", Math.max(0, h - 2))
            .attr("rx", 3)
            .attr("fill", "none")
            .attr("stroke", "#ff9cab")
            .attr("stroke-width", 1.2)
            .attr("stroke-dasharray", "5,3")
            .attr("stroke-opacity", 0.85)
            .attr("pointer-events", "none");

    }

    // Double border for Critical (P0) priorities
    if (d.priority === 0 && !isParent) {
        g.append("rect")
            .attr("x", -w / 2 + 3).attr("y", -h / 2 + 3).attr("width", Math.max(0, w - 6)).attr("height", Math.max(0, h - 6))
            .attr("fill", "none")
            .attr("stroke", "#fff").attr("stroke-width", 1)
            .attr("stroke-dasharray", "2,2").attr("pointer-events", "none");
    }

    // Only attempt to render text if we have enough space. Small nodes collapse to solid colored boxes.
    // Address tall-node symptom: Do not attempt to render text in narrow vertical slices
    const MIN_TEXT_WIDTH = 25;
    const MIN_TEXT_HEIGHT = 12;
    const MIN_ASPECT_RATIO_FOR_TEXT = 0.3;
    const MIN_ABS_WIDTH_FOR_TEXT = 30;
    if (w > MIN_TEXT_WIDTH && h > MIN_TEXT_HEIGHT && (w >= h * MIN_ASPECT_RATIO_FOR_TEXT || w > MIN_ABS_WIDTH_FOR_TEXT)) {
        const label = escapeHtml(d.label || '');
        const pad = 6;

        if (isParent) {
            // Parent nodes: Draw label in the header bar — shared metrics
            const m = treemapHeaderMetrics(w, h, d.label || '', d.depth || 0);
            if (m.headerH <= 0 || m.labelLines.length === 0) {
                return;
            }
            const labelW = Math.max(0, w - m.pad * 2 - m.badgeReserve);
            const labelHtml = m.labelLines.map((line: string) => escapeHtml(line)).join('<br/>');
            if (w > 20 && h > 12) {
                g.append("foreignObject")
                    .attr("x", -w / 2 + m.pad).attr("y", -h / 2 + 1)
                    .attr("width", labelW).attr("height", m.headerH - 2)
                    .style("pointer-events", "none")
                    .append("xhtml:div")
                    .style("display", "flex")
                    .style("align-items", "flex-start")
                    .style("width", "100%")
                    .style("height", "100%")
                    .style("pointer-events", "none")
                    .html(`
                        <div style="font-size: ${m.fs}px; font-weight: 760; color: rgba(255,255,255,0.96); overflow: hidden; white-space: nowrap; text-overflow: ellipsis; text-transform: none; letter-spacing: 0.01em; line-height: ${m.lineHeight}px;">
                            ${labelHtml}
                        </div>
                    `);

                renderCountBadge(g, w, h, hue, d._leafCount || 0, d.totalLeafCount || 0, m.fs);
            }
        } else {
            // Leaf nodes: Draw title
            const isBlocked = d.status === "blocked";
            const blockedReserve = isBlocked && h > 40 ? 16 : 0;
            const textFit = fitTreemapText(
                d.label || '',
                Math.max(0, w - pad * 2),
                Math.max(0, h - pad * 2 - blockedReserve),
                {
                    minFontSize: 5,
                    maxFontSize: Math.max(8, Math.min(w * 0.25, (h - pad * 2) / 1.18)),
                    maxLines: Math.max(1, Math.min(6, Math.floor((h - pad * 2) / 12))),
                },
            );
            const labelHtml = textFit.lines.map((line: string) => escapeHtml(line)).join('<br/>');

            g.append("foreignObject")
                .attr("x", -w / 2 + pad).attr("y", -h / 2 + pad)
                .attr("width", Math.max(0, w - pad * 2)).attr("height", Math.max(0, h - pad * 2))
                .style("pointer-events", "none")
                .append("xhtml:div")
                .style("display", "flex")
                .style("flex-direction", "column")
                .style("justify-content", "flex-start")
                .style("width", "100%")
                .style("height", "100%")
                .style("pointer-events", "none")
                .html(`
                    ${isBlocked && h > 40 ? `<div style="display: flex; justify-content: flex-end; margin-bottom: 4px;"><span class="material-symbols-outlined" style="display:inline-flex; align-items:center; justify-content:center; min-width:${Math.max(20, textFit.fontSize + 10)}px; height:${Math.max(20, textFit.fontSize + 10)}px; font-size:${textFit.fontSize + 4}px; color:#fff4f6; background:rgba(106,49,66,0.98); border:1px solid #ffb4c0; border-radius:999px; box-shadow:0 0 0 2px rgba(255,128,146,0.18);">pause_circle</span></div>` : ''}
                    <div style="font-size: ${textFit.fontSize}px; font-weight: 600; color: ${textColor}; line-height: ${textFit.lineHeight}px; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; letter-spacing: -0.01em;">
                        ${labelHtml}
                    </div>
                `);
        }
    }
}

function renderWrappedTextInCircle(g: d3.Selection<SVGGElement, any, null, undefined>, r: number, rawLabel: string, status?: string) {
    const isCompleted = ['done', 'cancelled'].includes(status || '');
    const innerW = r * 1.4;
    const innerH = r * 1.5;

    const words = rawLabel.split(/\s+/).filter((w: string) => w);
    if (words.length === 0) return;

    function wrapAtFs(fs: number): string[] {
        const charW = fs * 0.52;
        const maxChars = Math.max(1, Math.floor(innerW / charW));
        const lines: string[] = [];
        let cur = '';
        for (const w of words) {
            const test = cur ? cur + ' ' + w : w;
            if (test.length <= maxChars) {
                cur = test;
            } else {
                if (cur) lines.push(cur);
                cur = w.length > maxChars ? w.substring(0, maxChars) : w;
            }
        }
        if (cur) lines.push(cur);
        return lines;
    }

    let bestFs = 4;
    let bestLines = wrapAtFs(4);
    for (let tryFs = Math.min(r * 0.8, 60); tryFs >= 4; tryFs -= 0.5) {
        const lines = wrapAtFs(tryFs);
        const totalH = lines.length * tryFs * 1.15;
        if (totalH <= innerH) {
            bestFs = tryFs;
            bestLines = lines;
            break;
        }
    }

    const maxLines = Math.max(1, Math.floor(innerH / (bestFs * 1.15)));
    if (bestLines.length > maxLines) {
        bestLines = bestLines.slice(0, maxLines);
        bestLines[maxLines - 1] = bestLines[maxLines - 1].slice(0, -1) + '…';
    }

    const lineH = bestFs * 1.15;
    const totalH = bestLines.length * lineH;
    const startY = -totalH / 2 + bestFs * 0.35;

    bestLines.forEach((line, i) => {
        g.append("text")
            .attr("x", 0)
            .attr("y", startY + i * lineH)
            .attr("text-anchor", "middle")
            .attr("dominant-baseline", "central")
            .attr("font-size", bestFs + "px")
            .attr("font-weight", isCompleted ? "400" : "600")
            .attr("fill", isCompleted ? "rgba(255,255,255,0.25)" : "#fff")
            .attr("pointer-events", "none")
            .text(line);
    });
}

export function buildCirclePackNode(g: d3.Selection<SVGGElement, any, null, undefined>, d: any, isSelected = false) {
    const r = Math.max(d._lr || d.w / 2 || 5, 2);
    const isParent = !(d._isLeaf ?? d.isLeaf);


    // Add native tooltip
    g.append("title").text(`${d.label} (${d.status})\nType: ${d.type}`);

    if (isSelected) {
        g.classed("selected-node", true);
    } else {
        g.classed("selected-node", false);
    }

    // Color: status → fill, priority → border (matches legend & treemap)
    const hue = projectHue(d.project || d.id);

    let cellColor: string;
    if (isParent) {
        cellColor = `hsl(${hue}, 46%, 24%)`;
    } else {
        const status = (d.status || 'inbox').toLowerCase();
        cellColor = STATUS_FILLS[status] || '#4b5563';
    }

    const priorityBorder = SHARED_PRIORITY_BORDERS[d.priority ?? 4] || '#64748b';

    if (isParent) {
        // ── Depth-tiered parent rendering ──
        // depth 1 = top-level projects, depth 2 = epics/goals, depth 3+ = sub-groups
        const depth = d.depth || 1;

        // Visual parameters per tier
        const isProject = depth <= 1;
        const isEpic = depth === 2;
        // depth 3+ = sub-group

        const fillOpacity = isProject ? 0.42 : isEpic ? 0.3 : 0.22;
        const strokeSat = isProject ? 72 : isEpic ? 58 : 42;
        const strokeLight = isProject ? 68 : isEpic ? 58 : 48;
        const strokeWidth = isSelected
            ? Math.max(3, r * 0.02)
            : isProject
                ? Math.max(2.2, Math.min(4.8, r * 0.006))
                : isEpic
                    ? Math.max(1.3, Math.min(3, r * 0.004))
                    : Math.max(0.8, Math.min(1.6, r * 0.0025));
        const dashArray = isSelected ? "none" : isProject ? "none" : isEpic ? "6,3" : "3,3";
        const strokeColor = isSelected ? "#fff" : `hsl(${hue}, ${strokeSat}%, ${strokeLight}%)`;

        // Main circle
        g.append("circle").attr("cx", 0).attr("cy", 0).attr("r", r)
            .attr("class", "node-surface")
            .attr("fill", cellColor).attr("fill-opacity", fillOpacity)
            .attr("stroke", strokeColor)
            .attr("stroke-width", strokeWidth)
            .attr("stroke-dasharray", dashArray);

        // Parent label — centered in container with background pill
        const MIN_RADIUS_FOR_LABEL = 14;
        if (r > MIN_RADIUS_FOR_LABEL) {
            const minFs = isProject ? 11 : isEpic ? 9 : 7;
            const maxFs = isProject ? 22 : isEpic ? 16 : 11;
            const scaleFactor = isProject ? 0.047 : isEpic ? 0.038 : 0.03;
            const fs = Math.max(minFs, Math.min(maxFs, r * scaleFactor));
            const labelText = escapeHtml(d.label || '');
            const labelColor = isProject
                ? `hsl(${hue}, 90%, 90%)`
                : isEpic
                    ? `hsl(${hue}, 72%, 84%)`
                    : `hsl(${hue}, 54%, 76%)`;

            // Label dimensions — allow wrapping for legibility
            const lineH = fs * 1.25;
            const maxLines = 2;
            const labelH = lineH * maxLines;

            // Position label at top of circle, inset from edge
            const labelY = -r + Math.max(18, r * 0.28);

            // Use a generous width — the label sits at the top where the chord is narrower,
            // but we'd rather show the full name and let CSS clip than truncate aggressively
            const labelW = Math.max(52, r * (isProject ? 1.2 : isEpic ? 1.08 : 0.94));

            // Type prefix for projects/epics
            const displayLabel = labelText;

            g.append("foreignObject")
                .attr("x", -labelW / 2).attr("y", labelY)
                .attr("width", labelW).attr("height", labelH + 10)
                .attr("class", "parent-label")
                .style("pointer-events", "none")
                .append("xhtml:div")
                .style("display", "flex")
                .style("justify-content", "center")
                .style("align-items", "flex-start")
                .style("width", "100%")
                .style("height", "100%")
                .style("pointer-events", "none")
                .html(`
                    <div style="font-size: ${fs}px; font-weight: ${isProject ? 820 : isEpic ? 760 : 700}; color: ${labelColor}; text-transform: none; letter-spacing: ${isProject ? '0.01em' : '0'}; text-align: center; overflow: hidden; display: -webkit-box; -webkit-line-clamp: ${maxLines}; -webkit-box-orient: vertical; line-height: ${lineH}px; background: ${isProject ? 'rgba(8,10,12,0.82)' : isEpic ? 'rgba(8,10,12,0.74)' : 'rgba(8,10,12,0.62)'}; border: 1px solid ${isProject ? 'rgba(255,255,255,0.2)' : isEpic ? 'rgba(255,255,255,0.14)' : 'rgba(255,255,255,0.1)'}; border-radius: 999px; padding: ${isProject ? '4px 10px' : '3px 8px'}; box-shadow: ${isProject ? '0 2px 12px rgba(0,0,0,0.34)' : '0 2px 8px rgba(0,0,0,0.22)'};">
                        ${displayLabel}
                    </div>
                `);
        }
    } else {
        // Leaf task circle — fill=status color, stroke=priority color
        const isCompleted = ['done', 'cancelled'].includes(d.status);
        const baseStrokeW = isCompleted
            ? Math.max(0.3, Math.min(1, r * 0.01))
            : d.priority <= 1
                ? Math.max(1.1, Math.min(2.4, r * 0.026))
                : Math.max(0.5, Math.min(2, r * 0.02));
        const strokeColor = isSelected ? "#fff" : isCompleted ? "#2D3340" : priorityBorder;
        g.append("circle").attr("cx", 0).attr("cy", 0).attr("r", r)
            .attr("class", "node-surface")
            .attr("fill", cellColor)
            .attr("stroke", strokeColor)
            .attr("stroke-width", isSelected ? Math.max(2, r * 0.02) : baseStrokeW)
            .attr("stroke-opacity", isCompleted ? 0.3 : 1);

        // Blocked: stronger outer ring and symbol for fast scanability
        if (d.status === "blocked") {
            g.append("circle").attr("cx", 0).attr("cy", 0).attr("r", r)
                .attr("fill", "none").attr("stroke", "#ff8797")
                .attr("stroke-width", Math.max(1.1, r * 0.022))
                .attr("stroke-dasharray", "5,3")
                .attr("stroke-opacity", 0.82)
                .style("pointer-events", "none");

            if (r > 14) {
                g.append("text")
                    .attr("x", 0)
                    .attr("y", -Math.max(4, r * 0.08))
                    .attr("text-anchor", "middle")
                    .attr("dominant-baseline", "central")
                    .attr("font-size", Math.max(9, Math.min(16, r * 0.32)) + "px")
                    .attr("font-weight", "800")
                    .attr("fill", "#ffe4e8")
                    .attr("pointer-events", "none")
                    .text("!");
            }
        }

        // Text always rendered; visibility toggled by zoom handler in CirclePackView
        renderWrappedTextInCircle(g, r, d.label || '', d.status);
    }
}

function pad(r: number) {
    return Math.min(20, r * 0.15);
}


export function buildArcNode(g: d3.Selection<SVGGElement, any, null, undefined>, d: any, isSelected = false) {
    const r = Math.max(6, (d.dw || 1) * 0.8 + 4);

    // Add native tooltip
    g.append("title").text(`${d.label} (${d.status})`);

    if (isSelected) {
        g.classed("selected-node", true);
    } else {
        g.classed("selected-node", false);
    }

    g.append("circle").attr("cx", 0).attr("cy", 0).attr("r", r)
        .attr("class", "node-surface")
        .attr("fill", d.fill)
        .attr("stroke", isSelected ? "#fff" : d.borderColor).attr("stroke-width", isSelected ? 4 : 1);

    g.append("text").attr("class", "node-text")
        .attr("x", r + 6).attr("y", r + 12)
        .attr("text-anchor", "start").attr("font-size", "10px")
        .attr("fill", "#d4d4d8").attr("opacity", 0.9)
        .attr("transform", `rotate(45, ${r + 6}, ${r + 12})`)
        .text((d.label || '').substring(0, 25) + ((d.label || '').length > 25 ? '...' : ''));
}
