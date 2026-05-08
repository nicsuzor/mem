use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Source of a typed fact.
///
/// Every typed-fact MCP tool must return its data wrapped in [`TypedFact`],
/// which includes a `FactSource`. This provides a structural guard against
/// "producer-blindness" — adding a tool that hasn't named its producer
/// becomes a compile error (or at least a type error if the tool is
/// expected to return TypedFact).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FactSource {
    /// Data sourced from `pr-state.json`.
    PrStateJson { generated_at: DateTime<Utc> },
    /// Data sourced from a node in the GraphStore.
    GraphStoreNode,
    /// Data sourced from `git ls-tree` at a specific commit.
    GitLsTree { commit: String },
    /// Data sourced from session frontmatter at the given path.
    SessionFrontmatter { path: PathBuf },
    /// Data derived from one or more other fact sources.
    Derived { from: Vec<FactSource> },
}

/// A value annotated with its source and observation time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypedFact<T> {
    pub value: T,
    pub source: FactSource,
    pub observed_at: DateTime<Utc>,
}

impl<T> TypedFact<T> {
    /// Create a new typed fact with the current time as `observed_at`.
    pub fn new(value: T, source: FactSource) -> Self {
        Self {
            value,
            source,
            observed_at: Utc::now(),
        }
    }
}

/// Trait for MCP tools that provide typed facts.
///
/// Every method in this trait must return a [`TypedFact`] or a [`Vec<TypedFact>`].
///
/// ### Tool Inventory
///
/// | Tool | Producer | FactSource emitted |
/// |---|---|---|
/// | `effective_status(node_id)` | `$AOPS_SESSIONS/state/pr-state.json` | `PrStateJson` or `GraphStoreNode` |
/// | `target_ancestors(node_id)` | graph_store edges | `Derived { from: [GraphStoreNode] }` |
/// | `list_modified_since(t, kinds)` | per-file mtimes in graph_store | `GraphStoreNode` |
/// | `pkb_snapshot_at(t)` | `git ls-tree` + graph hydration | `GitLsTree { commit }` |
/// | `link_failure_reasons()` | parse outcome at build | `Derived { from: [PrStateJson] }` |
/// | `diagnostics()` | aggregate over above | `Derived { from: [...] }` |
/// | `last_session_end_at()` | session frontmatter | `SessionFrontmatter { path }` |
pub trait FactsProvider {
    // Methods will be added in subsequent subtasks (U1, U2, U8, U9, U10, U11, U14).
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_fact_source_serialization() {
        let source = FactSource::PrStateJson {
            generated_at: Utc::now(),
        };
        let serialized = serde_json::to_string(&source).unwrap();
        let deserialized: FactSource = serde_json::from_str(&serialized).unwrap();
        assert_eq!(source, deserialized);
    }

    #[test]
    fn test_typed_fact_serialization() {
        let fact = TypedFact::new(
            "test_value".to_string(),
            FactSource::GraphStoreNode,
        );
        let serialized = serde_json::to_string(&fact).unwrap();
        let deserialized: TypedFact<String> = serde_json::from_str(&serialized).unwrap();
        assert_eq!(fact, deserialized);
    }
}
