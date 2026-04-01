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

        // Manhattan routing: right-angle segments with a midpoint bend
        const mx = (sx + tx) / 2;
        return `M${sx},${sy} L${mx},${sy} L${mx},${ty} L${tx},${ty}`;
    })
    .attr("stroke", (d: any) => d.color)
    .attr("stroke-width", (d: any) => d.width)
    .attr("stroke-dasharray", (d: any) => d.dash)
    .attr("opacity", (d: any) => d.type === "ref" ? 0.35 : 0.85);
}

interface Rect {
    x: number;
    y: number;
    X: number; // right edge (x + width)
    Y: number; // bottom edge (y + height)
    containerId?: string;
}

// Obstacle-aware routing state — set by ForceView before each tick
let _obstacles: Rect[] = [];
let _nodeGroupMap: Map<string, string> = new Map();

/** Called by ForceView to provide obstacle data for edge routing */
export function setEdgeObstacles(obstacles: Rect[], nodeGroupMap: Map<string, string>) {
    _obstacles = obstacles;
    _nodeGroupMap = nodeGroupMap;
    if (obstacles.length > 0 && !_loggedOnce) {
        console.log(`[EdgeRouter] ${obstacles.length} obstacles, ${nodeGroupMap.size} nodes mapped`);
        _loggedOnce = true;
    }
}
let _loggedOnce = false;

/** Check if a horizontal or vertical line segment intersects a rectangle */
function segmentIntersectsRect(
    x1: number, y1: number, x2: number, y2: number, r: Rect
): boolean {
    // Inflate rect slightly for clearance
    const pad = 15;
    const rx = r.x - pad, ry = r.y - pad, rX = r.X + pad, rY = r.Y + pad;

    if (Math.abs(y1 - y2) < 1) {
        // Horizontal segment
        const minX = Math.min(x1, x2), maxX = Math.max(x1, x2);
        return y1 > ry && y1 < rY && maxX > rx && minX < rX;
    }
    if (Math.abs(x1 - x2) < 1) {
        // Vertical segment
        const minY = Math.min(y1, y2), maxY = Math.max(y1, y2);
        return x1 > rx && x1 < rX && maxY > ry && minY < rY;
    }
    return false;
}

/** Route a Manhattan path around obstacle rectangles */
function routeAroundObstacles(
    sx: number, sy: number, tx: number, ty: number,
    sourceId: string, targetId: string
): string {
    // Groups that the source and target belong to — we don't avoid those
    const sourceGroup = _nodeGroupMap.get(sourceId);
    const targetGroup = _nodeGroupMap.get(targetId);

    // Filter to obstacles that are NOT the source/target's own groups
    const obstacles = _obstacles.filter(r =>
        r.containerId !== sourceGroup && r.containerId !== targetGroup
    );

    if (obstacles.length === 0) {
        // No obstacles to avoid — simple Manhattan
        const mx = (sx + tx) / 2;
        return `M${sx},${sy} L${mx},${sy} L${mx},${ty} L${tx},${ty}`;
    }

    // Try default Manhattan path (midpoint routing)
    const mx = (sx + tx) / 2;
    const defaultHits = obstacles.filter(r =>
        segmentIntersectsRect(sx, sy, mx, sy, r) ||
        segmentIntersectsRect(mx, sy, mx, ty, r) ||
        segmentIntersectsRect(mx, ty, tx, ty, r)
    );

    if (defaultHits.length === 0) {
        return `M${sx},${sy} L${mx},${sy} L${mx},${ty} L${tx},${ty}`;
    }

    // Default path hits obstacles — try routing around them.
    // Try four candidate routes: left, right, above, below the blocking obstacles.
    // Pick the shortest one that doesn't hit obstacles.
    const pad = 25;

    const leftX = Math.min(...defaultHits.map(r => r.x)) - pad;
    const rightX = Math.max(...defaultHits.map(r => r.X)) + pad;
    const topY = Math.min(...defaultHits.map(r => r.y)) - pad;
    const bottomY = Math.max(...defaultHits.map(r => r.Y)) + pad;

    const candidates: { path: string; segments: [number, number, number, number][] }[] = [
        // Route via left
        {
            path: `M${sx},${sy} L${leftX},${sy} L${leftX},${ty} L${tx},${ty}`,
            segments: [[sx, sy, leftX, sy], [leftX, sy, leftX, ty], [leftX, ty, tx, ty]],
        },
        // Route via right
        {
            path: `M${sx},${sy} L${rightX},${sy} L${rightX},${ty} L${tx},${ty}`,
            segments: [[sx, sy, rightX, sy], [rightX, sy, rightX, ty], [rightX, ty, tx, ty]],
        },
        // Route above
        {
            path: `M${sx},${sy} L${sx},${topY} L${tx},${topY} L${tx},${ty}`,
            segments: [[sx, sy, sx, topY], [sx, topY, tx, topY], [tx, topY, tx, ty]],
        },
        // Route below
        {
            path: `M${sx},${sy} L${sx},${bottomY} L${tx},${bottomY} L${tx},${ty}`,
            segments: [[sx, sy, sx, bottomY], [sx, bottomY, tx, bottomY], [tx, bottomY, tx, ty]],
        },
    ];

    let bestPath: string | null = null;
    let bestScore = Infinity;

    for (const c of candidates) {
        let hits = 0;
        let len = 0;
        for (const [x1, y1, x2, y2] of c.segments) {
            len += Math.abs(x2 - x1) + Math.abs(y2 - y1);
            hits += obstacles.filter(r => segmentIntersectsRect(x1, y1, x2, y2, r)).length;
        }
        // Heavily penalize paths that still hit obstacles
        const score = len + hits * 10000;
        if (score < bestScore) {
            bestScore = score;
            bestPath = c.path;
        }
    }

    return bestPath || `M${sx},${sy} L${mx},${sy} L${mx},${ty} L${tx},${ty}`;
}

export function routeSfdpEdges(linkSelection: Selection<any, any, any, any>) {
    linkSelection.attr("d", (d: any) => {
        if (!d.source || !d.target) return null;
        const sx = d.source.x, sy = d.source.y;
        const tx = d.target.x, ty = d.target.y;
        if (sx == null || tx == null) return null;

        const sourceId = d.source.id || d.source;
        const targetId = d.target.id || d.target;

        return routeAroundObstacles(sx, sy, tx, ty, sourceId, targetId);
    });
}
