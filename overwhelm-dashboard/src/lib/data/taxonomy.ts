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
    ) { }
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
        "#59b108", // green
        "dashed",
        "colaLinkDistSoftDependsOn",
        "colaLinkWeightSoftDependsOn",
        "edgeSoftDependencies"
    ),
    contributes_to: new EdgeType(
        "contributes_to",
        "Contributes To",
        "#ff7300", // orange
        "solid",
        "colaLinkDistContributesTo",
        "colaLinkWeightContributesTo",
        "edgeContributes"
    ),
    similar_to: new EdgeType(
        "similar_to",
        "Similar To",
        "#00ffc8", // cyan
        "dashed",
        "colaLinkDistSimilarTo",
        "colaLinkWeightSimilarTo",
        "edgeSimilar"
    ),
    ref: new EdgeType(
        "ref",
        "References",
        "#c11cf3", // pink
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
    const def = EDGE_TYPES[edgeTypeStr];
    if (!def) {
        throw new Error(
            `getEdgeTypeDef: unknown edge type "${edgeTypeStr}". ` +
            `Known types: ${Object.keys(EDGE_TYPES).join(", ")}, parent. ` +
            `Canonicalise upstream (see styleEdge) or extend EDGE_TYPES — ` +
            `do not add a silent fallback.`
        );
    }
    return def;
}

export class NodeType {
    constructor(
        public id: string,
        public displayName: string,
        public baseSize: number,
        public isStructural: boolean
    ) { }
}

// Keep keys in sync with TYPE_SHAPE in constants.ts — getNodeTypeDef throws
// on unknowns.
export const NODE_TYPES: Record<string, NodeType> = {
    // Hierarchical / strategic
    goal: new NodeType("goal", "Goal", 18, true),
    target: new NodeType("target", "Target", 34, true),
    project: new NodeType("project", "Project", 18, true),
    epic: new NodeType("epic", "Epic", 18, true),
    group: new NodeType("group", "Group", 18, true),
    // Workable
    task: new NodeType("task", "Task", 12, false),
    subtask: new NodeType("subtask", "Subtask", 10, false),
    action: new NodeType("action", "Action", 12, false),
    bug: new NodeType("bug", "Bug", 12, false),
    feature: new NodeType("feature", "Feature", 12, false),
    // Reflection / learning
    learn: new NodeType("learn", "Learn", 10, false),
    review: new NodeType("review", "Review", 10, false),
    "audit-report": new NodeType("audit-report", "Audit", 10, false),
    "session-log": new NodeType("session-log", "Session", 10, false),
    // Knowledge artefacts
    note: new NodeType("note", "Note", 8, false),
    knowledge: new NodeType("knowledge", "Knowledge", 10, false),
    document: new NodeType("document", "Document", 10, false),
    reference: new NodeType("reference", "Reference", 8, false),
    spec: new NodeType("spec", "Spec", 10, false),
    memory: new NodeType("memory", "Memory", 8, false),
    index: new NodeType("index", "Index", 10, true),
    case: new NodeType("case", "Case", 10, false),
    daily: new NodeType("daily", "Daily", 10, false),
    // People
    person: new NodeType("person", "Person", 10, false),
    contact: new NodeType("contact", "Contact", 10, false),
};

export function getNodeTypeDef(typeStr: string): NodeType {
    const key = typeStr.toLowerCase();
    const def = NODE_TYPES[key];
    if (!def) {
        throw new Error(
            `getNodeTypeDef: unknown node type "${typeStr}". ` +
            `Known types: ${Object.keys(NODE_TYPES).join(", ")}. ` +
            `Extend NODE_TYPES or normalise upstream — do not add a silent fallback.`
        );
    }
    return def;
}
