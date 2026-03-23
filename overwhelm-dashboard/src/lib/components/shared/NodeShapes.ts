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

    // Determine visual tier
    const isEpicTier = !d._isProjectContainer && !d._isOverflow && isParent
        && ['epic', 'goal', 'project'].includes(d.type);

    // ── TIER 1: Project containers — explicit bounded regions ──
    if (d._isProjectContainer) {
        const bgTint = `hsl(${hue}, 20%, 10%)`;
        const borderColor = `hsl(${hue}, 40%, 35%)`;
        const labelColor = `hsl(${hue}, 50%, 65%)`;

        // Solid dark background — makes project region clearly distinct from canvas
        g.append("rect")
            .attr("x", -w / 2).attr("y", -h / 2).attr("width", w).attr("height", h)
            .attr("rx", 10)
            .attr("fill", bgTint).attr("fill-opacity", 0.7)
            .attr("stroke", isSelected ? "#fff" : borderColor)
            .attr("stroke-width", isSelected ? 3 : 2.5)
            .style("transition", "all 0.2s ease");

        // Inner inset line for extra boundary definition
        g.append("rect")
            .attr("x", -w / 2 + 3).attr("y", -h / 2 + 3)
            .attr("width", Math.max(0, w - 6)).attr("height", Math.max(0, h - 6))
            .attr("rx", 8)
            .attr("fill", "none")
            .attr("stroke", borderColor).attr("stroke-width", 0.5).attr("stroke-opacity", 0.3);

        // Project label — large, bold, pinned top-left with background pill
        if (w > 30 && h > 20) {
            const fs = Math.max(12, Math.min(22, w * 0.04));
            const labelText = (d.label || '').toUpperCase();
            const labelW = labelText.length * fs * 0.65 + 16;

            // Label background pill
            g.append("rect")
                .attr("x", -w / 2 + 6).attr("y", -h / 2 + 4)
                .attr("width", Math.min(labelW, w - 16)).attr("height", fs + 8)
                .attr("rx", 4)
                .attr("fill", bgTint).attr("fill-opacity", 0.9);

            g.append("text")
                .attr("x", -w / 2 + 14).attr("y", -h / 2 + fs + 6)
                .attr("text-anchor", "start").attr("dominant-baseline", "auto")
                .attr("font-size", fs + "px").attr("font-weight", "900")
                .attr("font-family", "var(--font-mono), monospace")
                .attr("fill", labelColor)
                .attr("letter-spacing", "0.15em")
                .attr("pointer-events", "none")
                .text(labelText);
        }
        return;
    }

    // ── TIER 2: Epics/Goals — medium tinted rectangles with all-caps titles ──
    if (isEpicTier) {
        const epicTint = `hsl(${hue}, 30%, 20%)`;
        g.append("rect")
            .attr("x", -w / 2).attr("y", -h / 2).attr("width", w).attr("height", h)
            .attr("rx", 5)
            .attr("fill", epicTint).attr("fill-opacity", 0.4)
            .attr("stroke", `hsl(${hue}, 35%, 35%)`).attr("stroke-width", isSelected ? 3 : 1)
            .style("transition", "all 0.2s ease");

        // Thin divider line below header area
        if (w > 40 && h > 30) {
            const headerH = Math.min(22, h * 0.3);
            g.append("line")
                .attr("x1", -w / 2 + 6).attr("x2", w / 2 - 6)
                .attr("y1", -h / 2 + headerH).attr("y2", -h / 2 + headerH)
                .attr("stroke", `hsl(${hue}, 30%, 40%)`).attr("stroke-width", 0.5)
                .attr("stroke-opacity", 0.6);
        }

        // Epic label — all-caps, medium size
        if (w > 30 && h > 16) {
            const fs = Math.max(7, Math.min(12, Math.min(w, h) * 0.08));
            const label = escapeHtml(d.label || '');
            g.append("foreignObject")
                .attr("x", -w / 2 + 5).attr("y", -h / 2 + 2)
                .attr("width", Math.max(0, w - 10)).attr("height", Math.min(20, h * 0.3))
                .style("pointer-events", "none")
                .append("xhtml:div")
                .style("pointer-events", "none")
                .html(`<div style="font-size:${fs}px; font-weight:700; color:hsl(${hue},40%,65%); text-transform:uppercase; letter-spacing:0.08em; line-height:1.2; overflow:hidden; text-overflow:ellipsis; white-space:nowrap;">${label}</div>`);
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

    // Status-based fill colors — muted by default, saturated only for attention states
    const STATUS_COLORS: Record<string, string> = {
        active: '#2C4A88',       // Soft blue — calm, doesn't scream
        in_progress: '#2C4A88',  // Soft blue
        review: '#3A5A9E',       // Slightly lighter soft blue
        waiting: '#1E3A6E',      // Darker muted blue
        decomposing: '#1E3A6E',  // Darker muted blue
        blocked: '#dc2626',      // STRONG red — needs attention
        ready: '#2D5A3D',        // Muted green
        todo: '#2D5A3D',         // Muted green
        inbox: '#1E4A2E',        // Dark green
        dormant: '#2D2D35',      // Very dark grey
        done: '#1E1E24',         // Near-black — greyed out
        completed: '#1E1E24',    // Near-black
        cancelled: '#18181C',    // Darkest grey
        deferred: '#2D2D35',     // Dark grey
        paused: '#4b5563',       // Grey
    };

    // Priority border colors — only P0/P1 draw the eye
    const PRIORITY_BORDERS: Record<number, string> = {
        0: '#f59e0b',  // P0 Critical — strong amber (attention!)
        1: '#d97706',  // P1 High — warm orange
        2: '#4A5568',  // P2 Med — blends with card
        3: '#3A4250',  // P3 Low — nearly invisible
        4: '#2D3340',  // P4 Backlog — disappears
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

function renderWrappedTextInCircle(g: d3.Selection<SVGGElement, any, null, undefined>, r: number, rawLabel: string, status?: string) {
    const isCompleted = ['done', 'completed', 'cancelled'].includes(status || '');
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
            .attr("font-weight", isCompleted ? "400" : "600")
            .attr("fill", isCompleted ? "rgba(255,255,255,0.25)" : "#fff")
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

    // Color: status → fill, priority → border (matches legend & treemap)
    const hue = projectHue(d.project || d.id);

    const CIRCLE_STATUS_COLORS: Record<string, string> = {
        active: '#2C4A88',       // Soft blue
        in_progress: '#2C4A88',  // Soft blue
        review: '#3A5A9E',       // Lighter blue
        waiting: '#1E3A6E',      // Darker muted blue
        decomposing: '#1E3A6E',  // Darker muted blue
        blocked: '#dc2626',      // STRONG red — needs attention
        ready: '#2D5A3D',        // Muted green
        todo: '#2D5A3D',         // Muted green
        inbox: '#1E4A2E',        // Dark green
        dormant: '#2D2D35',      // Very dark grey
        done: '#1E1E24',         // Near-black
        completed: '#1E1E24',    // Near-black
        cancelled: '#18181C',    // Darkest grey
        deferred: '#2D2D35',     // Dark grey
        paused: '#4b5563',       // Grey
    };

    const CIRCLE_PRIORITY_BORDERS: Record<number, string> = {
        0: '#f59e0b',  // P0 Critical — strong amber
        1: '#d97706',  // P1 High — warm orange
        2: '#4A5568',  // P2 Med — blends
        3: '#3A4250',  // P3 Low — nearly invisible
        4: '#2D3340',  // P4 Backlog — disappears
    };

    let cellColor: string;
    if (isParent) {
        cellColor = `hsl(${hue}, 40%, 25%)`;
    } else {
        const status = (d.status || 'inbox').toLowerCase();
        cellColor = CIRCLE_STATUS_COLORS[status] || '#4b5563';
    }

    const priorityBorder = CIRCLE_PRIORITY_BORDERS[d.priority ?? 4] || '#3A4250';

    if (isParent) {
        // ── Depth-tiered parent rendering ──
        // depth 1 = top-level projects, depth 2 = epics/goals, depth 3+ = sub-groups
        const depth = d.depth || 1;

        // Visual parameters per tier
        const isProject = depth <= 1;
        const isEpic = depth === 2;
        // depth 3+ = sub-group

        const fillOpacity = isProject ? 0.15 : isEpic ? 0.08 : 0.04;
        const strokeSat = isProject ? 55 : isEpic ? 40 : 25;
        const strokeLight = isProject ? 50 : isEpic ? 40 : 35;
        const strokeWidth = isSelected
            ? Math.max(2, r * 0.015)
            : isProject
                ? Math.max(1.5, Math.min(4, r * 0.003))
                : isEpic
                    ? Math.max(0.8, Math.min(2.5, r * 0.002))
                    : Math.max(0.4, Math.min(1.5, r * 0.001));
        const dashArray = isSelected ? "none" : isProject ? "none" : isEpic ? "6,3" : "3,2";

        g.append("circle").attr("cx", 0).attr("cy", 0).attr("r", r)
            .attr("fill", cellColor).attr("fill-opacity", fillOpacity)
            .attr("stroke", isSelected ? "#fff" : `hsl(${hue}, ${strokeSat}%, ${strokeLight}%)`)
            .attr("stroke-width", strokeWidth)
            .attr("stroke-dasharray", dashArray);

        // Parent label — positioned with background pill for readability
        const MIN_RADIUS_FOR_LABEL = 15;
        if (r > MIN_RADIUS_FOR_LABEL) {
            const minFs = isProject ? 18 : isEpic ? 12 : 8;
            const maxFs = isProject ? 80 : isEpic ? 50 : 24;
            const scaleFactor = isProject ? 0.06 : isEpic ? 0.045 : 0.03;
            const fs = Math.max(minFs, Math.min(maxFs, r * scaleFactor));
            const labelText = escapeHtml(d.label || '');
            const labelColor = `hsl(${hue}, ${isProject ? 70 : 50}%, ${isProject ? 80 : 70}%)`;
            const pillBg = `hsla(${hue}, 30%, 10%, 0.85)`;
            const labelPad = pad(r);

            // Estimate label width for background pill
            const estCharW = fs * 0.6;
            const maxLabelW = r * 1.7;
            const estLabelW = Math.min(labelText.length * estCharW + 16, maxLabelW);
            const pillH = fs + 8;

            // Background pill behind label
            g.append("rect")
                .attr("x", -estLabelW / 2).attr("y", -r + labelPad - 2)
                .attr("width", estLabelW).attr("height", pillH)
                .attr("rx", pillH / 2)
                .attr("fill", pillBg)
                .attr("class", "parent-label-bg")
                .style("pointer-events", "none");

            g.append("foreignObject")
                .attr("x", -r * 0.85).attr("y", -r + labelPad)
                .attr("width", r * 1.7).attr("height", fs * 2.5)
                .attr("class", "parent-label")
                .style("pointer-events", "none")
                .append("xhtml:div")
                .style("display", "flex")
                .style("justify-content", "center")
                .style("width", "100%")
                .style("pointer-events", "none")
                .html(`
                    <div style="font-size: ${fs}px; font-weight: ${isProject ? 900 : 700}; color: ${labelColor}; text-transform: uppercase; letter-spacing: ${isProject ? '0.15em' : '0.08em'}; text-align: center; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; text-shadow: 0 1px 4px rgba(0,0,0,0.8);">
                        ${labelText}
                    </div>
                `);
        }
    } else {
        // Leaf task circle — fill=status color, stroke=priority color
        const baseStrokeW = d.priority <= 1
            ? Math.max(1.5, Math.min(4, r * 0.04))
            : Math.max(0.5, Math.min(2, r * 0.02));
        g.append("circle").attr("cx", 0).attr("cy", 0).attr("r", r)
            .attr("fill", cellColor).attr("fill-opacity", opacity)
            .attr("stroke", isSelected ? "#fff" : priorityBorder)
            .attr("stroke-width", isSelected ? Math.max(2, r * 0.02) : baseStrokeW);

        if (d.status === "blocked" && d.dw >= 2) {
            const pulseGap = Math.max(1, r * 0.05);
            g.insert("circle", ":first-child")
                .attr("cx", 0).attr("cy", 0).attr("r", r + pulseGap)
                .attr("fill", "none").attr("stroke", "#ef4444")
                .attr("class", "danger-pulse");
        }

        // Text always rendered; visibility toggled by zoom handler in CirclePackView
        renderWrappedTextInCircle(g, r, d.label || '', d.status);
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
