# Typed-Fact MCP Tool Surface

This document defines the contract for typed-fact MCP tools in `mem`.

## Core Mandate

Every typed-fact MCP tool returns `TypedFact<T>` (single value) or `Vec<TypedFact<T>>` (collection). The `source` field is non-optional. This ensures that every fact produced by `mem` has a clear, traceable producer, preventing "producer-blindness" during system decomposition.

## Cited Prior Contracts

- `aops-core/skills/daily/SKILL.md` L131, L229, L232 — *"Do NOT run gh pr list from this skill. The repo-sync-cron artefact is the single source."*
- `aops-core/skills/sleep/SKILL.md` L339 — *"single producer / two consumers (/daily and dashboard)"*
- `aops-core/PR-STATE-INDEX.md` — schema spec for the artefact

## Tool Inventory

| Tool | Producer | FactSource emitted |
|---|---|---|
| `effective_status(node_id)` | `$AOPS_SESSIONS/state/pr-state.json` | `PrStateJson` or `GraphStoreNode` |
| `target_ancestors(node_id)` | graph_store edges | `Derived { from: [GraphStoreNode] }` |
| `list_modified_since(t, kinds)` | per-file mtimes in graph_store | `GraphStoreNode` |
| `pkb_snapshot_at(t)` | `git ls-tree` + graph hydration | `GitLsTree { commit }` |
| `link_failure_reasons()` | parse outcome at build | `Derived { from: [PrStateJson] }` |
| `diagnostics()` | aggregate over above | `Derived { from: [...] }` |
| `last_session_end_at()` | session frontmatter | `SessionFrontmatter { path }` |

## Implementation Details

The `FactSource` enum and `TypedFact<T>` struct are defined in `src/facts.rs`.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FactSource {
    PrStateJson { generated_at: DateTime<Utc> },
    GraphStoreNode,
    GitLsTree { commit: String },
    SessionFrontmatter { path: PathBuf },
    Derived { from: Vec<FactSource> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypedFact<T> {
    pub value: T,
    pub source: FactSource,
    pub observed_at: DateTime<Utc>,
}
```
