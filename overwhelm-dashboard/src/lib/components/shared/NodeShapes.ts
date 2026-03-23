import * as d3 from 'd3';
import type { GraphNode } from '../../data/prepareGraphData';

function escapeHtml(str: string): string {
    return str
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
}

function statusOpacity(d: GraphNode) {
    if (['done', 'completed', 'cancelled'].includes(d.status)) return 0.15;
    if (['active', 'inbox', 'todo', 'in_progress', 'review'].includes(d.status)) return 0.9;
    if (['waiting', 'decomposing', 'dormant'].includes(d.status)) return 0.5;
    return 0.35;
}

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

    const opacity = statusOpacity(d);

    if (d.shape === "pill") {
        g.append("rect").attr("x", -hw).attr("y", -hh).attr("width", d.w).attr("height", d.h)
            .attr("rx", hh).attr("ry", hh)
            .attr("fill", d.fill).attr("stroke", d.borderColor).attr("stroke-width", d.borderWidth)
            .attr("fill-opacity", opacity).attr("stroke-opacity", Math.max(opacity, 0.4));
    } else if (d.shape === "hexagon") {
        const c = Math.min(hh * 0.6, 12);
        const pts = `${-hw + c},${-hh} ${hw - c},${-hh} ${hw},${0} ${hw - c},${hh} ${-hw + c},${hh} ${-hw},${0}`;
        g.append("polygon").attr("points", pts)
            .attr("fill", d.fill).attr("stroke", d.borderColor).attr("stroke-width", d.borderWidth)
            .attr("fill-opacity", opacity).attr("stroke-opacity", Math.max(opacity, 0.4));
    } else {
        g.append("rect").attr("x", -hw).attr("y", -hh).attr("width", d.w).attr("height", d.h)
            .attr("rx", d.shape === "rounded" ? 10 : 4)
            .attr("fill", d.fill).attr("stroke", d.borderColor).attr("stroke-width", d.borderWidth)
            .attr("fill-opacity", opacity).attr("stroke-opacity", Math.max(opacity, 0.4));
    }

    if (d.status === "blocked" && d.dw >= 2) {
        g.insert("rect", ":first-child")
            .attr("x", -hw - 4).attr("y", -hh - 4).attr("width", d.w + 8).attr("height", d.h + 8)
            .attr("rx", hh + 4).attr("ry", hh + 4).attr("fill", "none").attr("stroke", "#ef4444")
            .attr("class", "danger-pulse");
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

function projectHue(projectId: string): number {
    let hash = 0;
    const id = projectId || 'default';
    for (let i = 0; i < id.length; i++) {
        hash = (hash << 5) - hash + id.charCodeAt(i);
        hash |= 0;
    }
    return Math.abs(hash) % 360;
}

export function buildTreemapNode(g: d3.Selection<SVGGElement, any, null, undefined>, d: any, isSelected = false) {
    // expects d._lw and d._lh to be populated by the layout algo if using true sizes, else uses d.w/d.h
    const w = d._lw || d.w;
    const h = d._lh || d.h;
    const isParent = !d.isLeaf;
    const opacity = statusOpacity(d);

    if (isSelected) {
        g.classed("selected-node", true);
    } else {
        g.classed("selected-node", false);
    }

    // Base color logic: Project-based Hue
    const hue = projectHue(d.project || d.id);

    // Project container nodes: faint tinted region with prominent label
    if (d._isProjectContainer) {
        const tint = `hsl(${hue}, 30%, 18%)`;
        g.append("rect")
            .attr("x", -w / 2).attr("y", -h / 2).attr("width", w).attr("height", h)
            .attr("rx", 6)
            .attr("fill", tint).attr("fill-opacity", 0.35)
            .attr("stroke", `hsl(${hue}, 40%, 35%)`).attr("stroke-width", isSelected ? 3 : 1.5)
            .style("transition", "all 0.2s ease");

        // Project label — always visible at top-left
        if (w > 30 && h > 20) {
            const fs = Math.max(10, Math.min(16, w * 0.03));
            g.append("text")
                .attr("x", -w / 2 + 8).attr("y", -h / 2 + fs + 4)
                .attr("text-anchor", "start").attr("dominant-baseline", "auto")
                .attr("font-size", fs + "px").attr("font-weight", "800")
                .attr("font-family", "var(--font-mono), monospace")
                .attr("fill", `hsl(${hue}, 50%, 65%)`).attr("fill-opacity", 0.9)
                .attr("letter-spacing", "0.12em")
                .attr("text-transform", "uppercase")
                .attr("pointer-events", "none")
                .text((d.label || '').toUpperCase());
        }
        return;
    }

    if (d._isOverflow) {
        g.append("rect")
            .attr("x", -w / 2).attr("y", -h / 2).attr("width", w).attr("height", h)
            .attr("rx", 4)
            .attr("fill", "rgba(0,0,0,0.4)")
            .attr("stroke", "rgba(255,255,255,0.15)").attr("stroke-width", 1)
            .attr("stroke-dasharray", "4,4")
            .style("transition", "all 0.2s ease");
            
        if (w > 40 && h > 15) {
            g.append("text")
                .attr("x", 0).attr("y", 0)
                .attr("text-anchor", "middle").attr("dominant-baseline", "central")
                .attr("font-size", "10px").attr("font-weight", "bold").attr("font-family", "monospace")
                .attr("fill", "rgba(255,255,255,0.5)")
                .text(d.label || '[...]');
        }
        return; // Skip the rest of the drawing logic for overflow nodes
    }

    // Status-based fill colors (primary visual signal)
    const STATUS_COLORS: Record<string, string> = {
        active: '#2563eb',       // Blue
        in_progress: '#2563eb',  // Blue
        review: '#3b82f6',       // Lighter blue
        waiting: '#1d4ed8',      // Darker blue
        decomposing: '#1e40af',  // Deep blue
        blocked: '#dc2626',      // Red
        ready: '#16a34a',        // Green
        todo: '#22c55e',         // Light green
        inbox: '#15803d',        // Dark green
        dormant: '#4b5563',      // Grey
        done: '#374151',         // Dark grey
        completed: '#374151',    // Dark grey
        cancelled: '#1f2937',    // Darker grey
        deferred: '#4b5563',     // Grey
        paused: '#4b5563',       // Grey
    };

    // Priority border colors (secondary signal via thin stroke)
    const PRIORITY_BORDERS: Record<number, string> = {
        0: '#fbbf24',  // P0 Critical — bright amber
        1: '#f97316',  // P1 High — orange
        2: '#a3a3a3',  // P2 Med — neutral
        3: '#6b7280',  // P3 Low — subtle
        4: '#4b5563',  // P4 Backlog — dim
    };

    let cellColor: string;
    if (isParent) {
        // Parents get a stable project hue (unchanged)
        cellColor = `hsl(${hue}, 40%, 25%)`;
    } else {
        const status = (d.status || 'inbox').toLowerCase();
        cellColor = STATUS_COLORS[status] || '#4b5563';
    }

    const priorityBorder = PRIORITY_BORDERS[d.priority ?? 4] || '#4b5563';

    // WCAG AA contrast: compute relative luminance and pick text color
    // that guarantees >= 4.5:1 contrast ratio
    function relativeLuminance(hex: string): number {
        if (!hex || !hex.startsWith('#') || hex.length < 7) return 0;
        const srgb = [hex.slice(1,3), hex.slice(3,5), hex.slice(5,7)]
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

    // Add native tooltip
    g.append("title").text(`${d.label} (${d.status})\nPriority: P${d.priority}\nProject: ${d.project || 'None'}`);

    // Base solid background — status fill + priority border
    g.append("rect")
        .attr("x", -w / 2).attr("y", -h / 2).attr("width", w).attr("height", h)
        .attr("rx", 4)
        .attr("fill", cellColor).attr("fill-opacity", isParent ? 0.2 : 0.85)
        .attr("stroke", isSelected ? "#fff" : (isParent ? cellColor : priorityBorder))
        .attr("stroke-width", isSelected ? 4 : (d.priority <= 1 ? 2.5 : 1))
        .style("transition", "all 0.2s ease");

    if (isParent && h > 20) {
        // Parent Header Bar — height adapts to label wrapping
        const label = d.label || '';
        const fontSize = d.depth <= 1 ? 11 : 9;
        const charWidth = fontSize * 0.56;
        const availableWidth = Math.max(20, w - 12);
        const charsPerLine = Math.max(4, Math.floor(availableWidth / charWidth));
        const lines = Math.min(3, Math.ceil(label.length / charsPerLine));
        const lineHeight = fontSize * 1.3;
        const basePad = d.depth <= 1 ? 10 : 6;
        const headerH = Math.min(Math.max(d.depth <= 1 ? 24 : 16, lines * lineHeight + basePad), h * 0.8);
        g.append("rect")
            .attr("x", -w / 2).attr("y", -h / 2)
            .attr("width", w).attr("height", headerH)
            .attr("rx", 4)
            .attr("fill", cellColor).attr("fill-opacity", 0.8);
    }

    // Grid overlay removed — status colors provide sufficient visual distinction
    // without the noise of overlaid patterns

    // Striped overlay for blocked tasks
    if (d.status === "blocked") {
        g.append("rect")
            .attr("x", -w / 2).attr("y", -h / 2).attr("width", w).attr("height", h)
            .attr("rx", 4)
            .attr("fill", "url(#striped-blocked)").attr("pointer-events", "none")
            .attr("opacity", 0.4);
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
    const MIN_TEXT_WIDTH = 8;
    const MIN_TEXT_HEIGHT = 6;
    const MIN_ASPECT_RATIO_FOR_TEXT = 0.3;
    const MIN_ABS_WIDTH_FOR_TEXT = 20;
    if (w > MIN_TEXT_WIDTH && h > MIN_TEXT_HEIGHT && (w >= h * MIN_ASPECT_RATIO_FOR_TEXT || w > MIN_ABS_WIDTH_FOR_TEXT)) {
        const label = escapeHtml(d.label || '');
        const pad = 6;

        if (isParent) {
            // Parent nodes: Draw label in the header bar
            // Font size scales with node but never smaller than child leaf text
            const parentFs = d.depth <= 1
                ? Math.max(8, Math.min(11, Math.min(w, h) * 0.09))
                : Math.max(6, Math.min(9, Math.min(w, h) * 0.09));
            // Header text height matches the dynamic header bar
            const charWidth = parentFs * 0.56;
            const textAvailW = Math.max(20, w - pad * 2);
            const charsPerLine = Math.max(4, Math.floor(textAvailW / charWidth));
            const textLines = Math.min(3, Math.ceil(label.length / Math.max(1, charsPerLine)));
            const headerTextH = Math.min(textLines * parentFs * 1.3 + 6, h * 0.7);
            if (w > 20 && h > 12) {
                g.append("foreignObject")
                    .attr("x", -w / 2 + pad).attr("y", -h / 2 + 2)
                    .attr("width", Math.max(0, w - pad * 2)).attr("height", headerTextH)
                    .style("pointer-events", "none")
                    .append("xhtml:div")
                    .style("display", "flex")
                    .style("align-items", "flex-start")
                    .style("width", "100%")
                    .style("height", "100%")
                    .style("pointer-events", "none")
                    .html(`
                        <div style="font-size: ${parentFs}px; font-weight: 700; color: #fff; overflow: hidden; text-overflow: ellipsis; display: -webkit-box; -webkit-line-clamp: ${textLines}; -webkit-box-orient: vertical; text-transform: uppercase; letter-spacing: 0.05em; line-height: 1.3;">
                            ${label}
                        </div>
                    `);
            }
        } else {
            // Leaf nodes: Draw title
            const fs = Math.max(4, Math.min(11, Math.min(w, h) * 0.22));
            const linesAvailable = Math.max(1, Math.floor((h - pad * 2) / (fs * 1.2)));

            const isBlocked = d.status === "blocked";

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
                    ${isBlocked && h > 40 ? `<div style="display: flex; justify-content: flex-end; margin-bottom: 2px;"><span class="material-symbols-outlined" style="font-size: ${fs + 2}px; color: #fff; background: var(--color-destructive); border-radius: 50%;">warning</span></div>` : ''}
                    <div style="font-size: ${fs}px; font-weight: 500; color: ${textColor}; line-height: 1.1; overflow: hidden; display: -webkit-box; -webkit-line-clamp: ${linesAvailable}; -webkit-box-orient: vertical; letter-spacing: -0.01em;">
                        ${label}
                    </div>
                `);
        }
    }
}

function renderWrappedTextInCircle(g: d3.Selection<SVGGElement, any, null, undefined>, r: number, rawLabel: string) {
    const innerW = r * 1.3;
    const innerH = r * 1.3;

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
            .attr("font-weight", "600")
            .attr("fill", "#fff")
            .attr("pointer-events", "none")
            .text(line);
    });
}

export function buildCirclePackNode(g: d3.Selection<SVGGElement, any, null, undefined>, d: any, isSelected = false) {
    const r = Math.max(d._lr || d.w / 2 || 5, 2);
    const isParent = !d.isLeaf;
    const opacity = statusOpacity(d);

    // Add native tooltip
    g.append("title").text(`${d.label} (${d.status})\nType: ${d.type}`);

    if (isSelected) {
        g.classed("selected-node", true);
    } else {
        g.classed("selected-node", false);
    }

    // Color logic similar to Treemap
    const hue = projectHue(d.project || d.id);
    const spectralHues = [
        "#ef4444", // 0 Critical
        "#f97316", // 1 High
        "#f59e0b", // 2 Med
        "#06b6d4", // 3 Low
        "#8b5cf6", // 4 Backlog
    ];

    let cellColor: string;
    if (isParent) {
        cellColor = `hsl(${hue}, 40%, 25%)`;
    } else {
        if (d.priority !== undefined && d.priority >= 0 && d.priority <= 2 && d.status !== 'done') {
            cellColor = spectralHues[d.priority];
        } else {
            cellColor = `hsl(${hue}, 35%, ${d.status === 'active' ? '35%' : '15%'})`;
        }
    }

    if (isParent) {
        // Parent containment circle — border scaled to radius
        const parentStroke = Math.max(0.5, Math.min(2, r * 0.01));
        g.append("circle").attr("cx", 0).attr("cy", 0).attr("r", r)
            .attr("fill", cellColor).attr("fill-opacity", 0.1)
            .attr("stroke", isSelected ? "#fff" : `hsl(${hue}, 50%, 45%)`)
            .attr("stroke-width", isSelected ? Math.max(1, r * 0.01) : parentStroke)
            .attr("stroke-dasharray", isSelected ? "none" : "3,2");

        // Parent label at top
        const MIN_RADIUS_FOR_LABEL = 15;
        const MIN_FONT_SIZE = 6;
        const MAX_FONT_SIZE = 14;
        const FONT_SIZE_SCALE_FACTOR = 0.12;
        if (r > MIN_RADIUS_FOR_LABEL) {
            const fs = Math.max(MIN_FONT_SIZE, Math.min(MAX_FONT_SIZE, r * FONT_SIZE_SCALE_FACTOR));
            g.append("foreignObject")
                .attr("x", -r * 0.7).attr("y", -r + pad(r))
                .attr("width", r * 1.4).attr("height", fs * 2.5)
                .style("pointer-events", "none")
                .append("xhtml:div")
                .style("display", "flex")
                .style("justify-content", "center")
                .style("width", "100%")
                .style("pointer-events", "none")
                .html(`
                    <div style="font-size: ${fs}px; font-weight: 800; color: hsl(${hue}, 70%, 75%); text-transform: uppercase; letter-spacing: 0.1em; text-align: center; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; text-shadow: 0 0 5px rgba(0,0,0,0.5);">
                        ${escapeHtml(d.label || '')}
                    </div>
                `);
        }
    } else {
        // Leaf task circle — border scaled to radius
        const strokeW = Math.max(0.5, Math.min(3, r * 0.03));
        g.append("circle").attr("cx", 0).attr("cy", 0).attr("r", r)
            .attr("fill", cellColor).attr("fill-opacity", opacity)
            .attr("stroke", isSelected ? "#fff" : cellColor)
            .attr("stroke-width", isSelected ? Math.max(1, r * 0.02) : strokeW);

        if (d.status === "blocked" && d.dw >= 2) {
            const pulseGap = Math.max(1, r * 0.05);
            g.insert("circle", ":first-child")
                .attr("cx", 0).attr("cy", 0).attr("r", r + pulseGap)
                .attr("fill", "none").attr("stroke", "#ef4444")
                .attr("class", "danger-pulse");
        }

        if (r > 6) {
            renderWrappedTextInCircle(g, r, d.label || '');
        }
    }
    }

    function pad(r: number) {
    return Math.min(20, r * 0.15);
    }


export function buildArcNode(g: d3.Selection<SVGGElement, any, null, undefined>, d: any, isSelected = false) {
    const r = Math.max(4, (d.dw || 1) * 0.5 + 3);
    const opacity = statusOpacity(d);

    // Add native tooltip
    g.append("title").text(`${d.label} (${d.status})`);

    if (isSelected) {
        g.classed("selected-node", true);
    } else {
        g.classed("selected-node", false);
    }

    g.append("circle").attr("cx", 0).attr("cy", 0).attr("r", r)
        .attr("fill", d.fill).attr("fill-opacity", opacity)
        .attr("stroke", isSelected ? "#fff" : d.borderColor).attr("stroke-width", isSelected ? 4 : 1);

    g.append("text").attr("class", "node-text")
        .attr("x", 0).attr("y", r + 12)
        .attr("text-anchor", "middle").attr("font-size", "8px")
        .attr("fill", "#a3a3a3").attr("opacity", 0.8)
        .text((d.label || '').substring(0, 15));
}
