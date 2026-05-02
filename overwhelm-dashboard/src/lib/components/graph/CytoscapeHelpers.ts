import {
    INCOMPLETE_STATUSES,
    STRUCTURAL_TYPES,
    PRIORITY_BORDERS,
} from "../../data/constants";
import { projectColor } from "../../data/projectUtils";
import type { GraphNode, GraphEdge } from "../../data/prepareGraphData";
import type { VisibilityState } from "../../stores/filters";

export const STATION_FILL = "#94a3b8";
export const START_FILL = "#22c55e";
export const BAD_CHOICE_FILL = "#6b7280";
export const BAD_CHOICE_BORDER = "#dc2626";

import { getEdgeTypeDef } from "../../data/taxonomy";

export function truncate(s: string, n: number): string {
    if (!s) return "";
    return s.length <= n ? s : s.slice(0, n - 1) + "…";
}

export function isIncomplete(node: GraphNode): boolean {
    return INCOMPLETE_STATUSES.has(node.status);
}

export function getProjectLineColor(project: string | null | undefined): string {
    return project ? projectColor(project) : "hsl(220, 12%, 46%)";
}

export function getEdgeRole(edgeType: string): "parent" | "dependency" | "reference" {
    if (edgeType === "parent") return "parent";
    if (edgeType === "depends_on" || edgeType === "soft_depends_on") return "dependency";
    return "reference";
}

export function getEdgeVisibilityState(
    sourceVisibility: VisibilityState,
    targetVisibility: VisibilityState,
): VisibilityState {
    if (sourceVisibility === "hidden" || targetVisibility === "hidden") return "hidden";
    if (sourceVisibility === "half" || targetVisibility === "half") return "half";
    return "bright";
}

export function getEdgeOpacity(visibilityState: VisibilityState, isOnRoute: boolean): number {
    const base = isOnRoute ? 0.5 : 0.18;
    return visibilityState === "half" ? base * 0.45 : base;
}

export function getEdgeWidth(isOnRoute: boolean): number {
    return isOnRoute ? 5 : 1.5;
}

export function getEdgeLineStyle(edgeType: string, isIntraGroup: boolean = false): { linkColor: string; linkDash: string } {
    const def = getEdgeTypeDef(edgeType, isIntraGroup);
    return { linkColor: def.color, linkDash: def.dashStyle };
}

export function computeBaseNodeData(node: GraphNode, isDestination: boolean = false, isOnRoute: boolean = true, isStart: boolean = false, visibilityState: VisibilityState = 'bright') {
    const completed = !isIncomplete(node);
    const typeLower = (node.type || "").toLowerCase();
    const isBackbone = STRUCTURAL_TYPES.has(typeLower);

    let nodeSize: number;
    let fillColor: string;
    let borderColor: string;
    let displayLabel: string;
    let borderWidth = 1;

    const isPriorityStation = !isDestination && node.priority <= 1 && isIncomplete(node) && typeLower !== "target";
    const isBadChoice = isPriorityStation && !isOnRoute;

    if (isDestination) {
        nodeSize = 34;
        fillColor = node.priority === 0 ? (PRIORITY_BORDERS[0] || "#dc3545") : (PRIORITY_BORDERS[1] || "#f59e0b");
        borderColor = getProjectLineColor(node.id);
        displayLabel = node.label;
        borderWidth = 3;
    } else if (isBadChoice) {
        nodeSize = 14;
        fillColor = BAD_CHOICE_FILL;
        borderColor = BAD_CHOICE_BORDER;
        displayLabel = truncate(node.label, 40);
    } else if (isOnRoute && isBackbone) {
        nodeSize = 18;
        fillColor = "#475569";
        borderColor = "#cbd5e1";
        displayLabel = truncate(node.label, 36);
    } else if (isStart) {
        nodeSize = isPriorityStation ? 16 : 12;
        fillColor = START_FILL;
        borderColor = "#ffffff";
        displayLabel = truncate(node.label, 40);
    } else if (isPriorityStation) {
        nodeSize = 16;
        fillColor = STATION_FILL;
        borderColor = "rgba(255,255,255,0.45)";
        displayLabel = truncate(node.label, 40);
    } else if (isOnRoute) {
        nodeSize = 12;
        fillColor = STATION_FILL;
        borderColor = "rgba(255,255,255,0.35)";
        displayLabel = truncate(node.label, 40);
    } else {
        nodeSize = 8; // generic node size if not on a specific route view
        fillColor = STATION_FILL;
        borderColor = "rgba(255,255,255,0.08)";
        displayLabel = truncate(node.label, 40);
    }

    const baseOpacity = visibilityState === "half" ? 0.45 : 0.95;
    const nodeOpacity = completed ? baseOpacity * 0.35 : baseOpacity;

    return {
        id: node.id,
        label: node.label,
        displayLabel,
        nodeType: node.type,
        priority: node.priority,
        visibilityState,
        isDestination: isDestination ? 1 : 0,
        isOnRoute: isOnRoute ? 1 : 0,
        isBackbone: isOnRoute && isBackbone ? 1 : 0,
        nodeSize,
        fillColor,
        borderColor,
        borderWidth,
        isCompleted: completed,
        nodeOpacity,
    };
}
