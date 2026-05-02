import type { VisibilityState } from "../stores/filters";

export class EdgeType {
    constructor(
        public id: string,
        public displayName: string,
        public color: string,
        public dashStyle: string,
        public distKey: string,
        public weightKey: string,
        public filterKey: string
    ) {}
}

export const EDGE_TYPES: Record<string, EdgeType> = {
    parent_intra: new EdgeType(
        "parent_intra",
        "Intra-Group",
        "#3b82f6", // blue
        "solid",
        "colaLinkDistIntraParent",
        "colaLinkWeightIntraParent",
        "edgeParent"
    ),
    parent_inter: new EdgeType(
        "parent_inter",
        "Parent (Inter)",
        "#facc15", // yellow
        "solid",
        "colaLinkDistInterParent",
        "colaLinkWeightInterParent",
        "edgeParent"
    ),
    depends_on: new EdgeType(
        "depends_on",
        "Depends On",
        "#ef4444", // red
        "solid",
        "colaLinkDistDependsOn",
        "colaLinkWeightDependsOn",
        "edgeDependencies"
    ),
    soft_depends_on: new EdgeType(
        "soft_depends_on",
        "Soft Depends",
        "#9ca3af", // gray
        "dashed",
        "colaLinkDistSoftDependsOn",
        "colaLinkWeightSoftDependsOn",
        "edgeSoftDependencies"
    ),
    contributes_to: new EdgeType(
        "contributes_to",
        "Contributes To",
        "#10b981", // emerald
        "solid",
        "colaLinkDistContributesTo",
        "colaLinkWeightContributesTo",
        "edgeContributes"
    ),
    similar_to: new EdgeType(
        "similar_to",
        "Similar To",
        "#c4b5fd", // violet
        "dashed",
        "colaLinkDistSimilarTo",
        "colaLinkWeightSimilarTo",
        "edgeSimilar"
    ),
    ref: new EdgeType(
        "ref",
        "References",
        "#a3a3a3", // neutral
        "dashed",
        "colaLinkDistRef",
        "colaLinkWeightRef",
        "edgeReferences"
    )
};

export function getEdgeTypeDef(edgeTypeStr: string, isIntraGroup: boolean = false): EdgeType {
    if (edgeTypeStr === "parent") {
        return isIntraGroup ? EDGE_TYPES.parent_intra : EDGE_TYPES.parent_inter;
    }
    return EDGE_TYPES[edgeTypeStr] || EDGE_TYPES.ref;
}

export class NodeType {
    constructor(
        public id: string,
        public displayName: string,
        public baseSize: number,
        public isStructural: boolean
    ) {}
}

export const NODE_TYPES: Record<string, NodeType> = {
    epic: new NodeType("epic", "Epic", 18, true),
    group: new NodeType("group", "Group", 18, true),
    target: new NodeType("target", "Target", 34, true),
    task: new NodeType("task", "Task", 12, false),
    note: new NodeType("note", "Note", 8, false)
};

export function getNodeTypeDef(typeStr: string): NodeType {
    return NODE_TYPES[typeStr.toLowerCase()] || NODE_TYPES.task;
}
