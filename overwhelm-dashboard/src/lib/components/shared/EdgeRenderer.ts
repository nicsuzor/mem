import type { Selection } from 'd3';

export function routeContainmentEdges(linkSelection: Selection<any, any, any, any>) {
    // In circle pack containment views, edges are hidden
    linkSelection.attr("d", null).attr("opacity", 0);
}

export function routeTreemapEdges(linkSelection: Selection<any, any, any, any>) {
    linkSelection.attr("d", (d: any) => {
        if (!d.source || !d.target) return null;
        const sx = d.source.x, sy = d.source.y;
        const tx = d.target.x, ty = d.target.y;
        if (sx == null || tx == null) return null;
        if (sx === tx && sy === ty) return null; // Ignore self-links or unpositioned elements

        // Draw a sweeping arc for dependencies
        const dx = tx - sx;
        const dy = ty - sy;
        const dr = Math.sqrt(dx * dx + dy * dy) * 1.5; // Curvature factor

        // A simple sweeping arc
        return `M${sx},${sy} A${dr},${dr} 0 0,1 ${tx},${ty}`;
    })
    .attr("fill", "none")
    .attr("stroke", (d: any) => d.color || (d.type === "ref" ? "#a3a3a3" : "#f59e0b"))
    .attr("stroke-width", (d: any) => d.width || (d.type === "ref" ? 1 : 3))
    .attr("stroke-dasharray", (d: any) => d.dash || (d.type === "ref" ? "4,4" : "none"))
    .attr("marker-end", (d: any) => d.type === "ref" ? "url(#ar)" : "url(#ad)")
    .attr("opacity", (d: any) => d.type === "ref" ? 0.3 : 0.75);
}

export function routeArcEdges(linkSelection: Selection<any, any, any, any>) {
    linkSelection.attr("d", (d: any) => {
        if (!d.source || !d.target) return null;
        const sx = d.source.x, sy = d.source.y;
        const tx = d.target.x, ty = d.target.y;
        if (sx == null || tx == null) return null;

        const dx = tx - sx;
        // quadratic bezier arc
        if (Math.abs(sy - ty) < 2) {
            // same row/depth: arch upward
            const rx = (sx + tx) / 2;
            const ry = sy - Math.abs(dx) * 0.3;
            return `M${sx},${sy} Q${rx},${ry} ${tx},${ty}`;
        }
        // different row: S-curve
        const my = (sy + ty) / 2;
        return `M${sx},${sy} C${sx},${my} ${tx},${my} ${tx},${ty}`;
    })
    .attr("stroke", (d: any) => d.color || (d.type === "ref" ? "#a3a3a3" : "#f59e0b"))
    .attr("stroke-width", (d: any) => d.width || (d.type === "ref" ? 1 : 3))
    .attr("stroke-dasharray", (d: any) => d.dash || "none")
    .attr("opacity", (d: any) => d.type === "ref" ? 0.3 : 0.75);
}

export function routeForceEdges(linkSelection: Selection<any, any, any, any>) {
    linkSelection.attr("d", (d: any) => {
        if (!d.source || !d.target) return null;
        const sx = d.source.x, sy = d.source.y;
        const tx = d.target.x, ty = d.target.y;
        if (sx == null || tx == null) return null;

        if (d.type === 'parent') {
            const my = (sy + ty) / 2;
            return `M${sx},${sy} C${sx},${my} ${tx},${my} ${tx},${ty}`;
        }
        // Curved arc for deps/refs
        const dx = tx - sx, dy = ty - sy;
        const dist = Math.sqrt(dx * dx + dy * dy);
        if (dist < 1) return `M${sx},${sy} L${tx},${ty}`;
        const bulge = Math.min(60, dist * 0.2);
        const mx = (sx + tx) / 2 - (dy / dist) * bulge;
        const my = (sy + ty) / 2 + (dx / dist) * bulge;
        return `M${sx},${sy} Q${mx},${my} ${tx},${ty}`;
    })
    .attr("stroke", (d: any) => d.color)
    .attr("stroke-width", (d: any) => d.width)
    .attr("stroke-dasharray", (d: any) => d.dash)
    .attr("opacity", (d: any) => d.type === "ref" ? 0.35 : 0.85);
}

export function routeSfdpEdges(linkSelection: Selection<any, any, any, any>) {
    // Curved paths avoid routing through unrelated epic group bounding boxes.
    // Parent edges use a gentle S-curve; dependency/ref edges use a direct quadratic arc.
    linkSelection.attr("d", (d: any) => {
        if (!d.source || !d.target) return null;
        const sx = d.source.x, sy = d.source.y;
        const tx = d.target.x, ty = d.target.y;
        if (sx == null || tx == null) return null;

        if (d.type === 'parent') {
            // S-curve for parent edges — stays close to source/target columns
            const my = (sy + ty) / 2;
            return `M${sx},${sy} C${sx},${my} ${tx},${my} ${tx},${ty}`;
        }
        // Quadratic arc for deps/refs — curves away from the midpoint line
        const dx = tx - sx, dy = ty - sy;
        const dist = Math.sqrt(dx * dx + dy * dy);
        const bulge = Math.min(60, dist * 0.2);
        // Perpendicular offset for the control point
        const mx = (sx + tx) / 2 - (dy / dist) * bulge;
        const my = (sy + ty) / 2 + (dx / dist) * bulge;
        return `M${sx},${sy} Q${mx},${my} ${tx},${ty}`;
    });
}
