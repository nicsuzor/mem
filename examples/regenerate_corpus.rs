//! Regenerate `tests/fixtures/ranking_corpus_20260824.json` against current
//! `graph_store` behaviour.
//!
//! Run from the crate root:
//!
//! ```text
//! cargo run --example regenerate_corpus
//! ```
//!
//! It reads the existing fixture for its node set and metadata, rebuilds the graph
//! through `GraphStore::rebuild_from_nodes` so every score field is recomputed by
//! current code, refreshes `ready_count` / `blocked_count`, and writes the fixture
//! back in place. `metadata.source_commit` is the provenance of the *node set* and
//! is deliberately left alone.
//!
//! ## The output is NOT bit-reproducible
//!
//! Two runs over byte-identical input produce different `downstream_weight`,
//! `criticality` and `focus_score` values (measured 2026-08-26: ~6,280 differing
//! lines between consecutive runs, `focus_score` moving by up to ~5%).
//!
//! The cause is in the build pipeline, not in this example.
//! `rebuild_from_nodes` does `nodes.into_values().collect()` on a `HashMap`
//! (`src/graph_store.rs:170`), and Rust randomises `HashMap` iteration order per
//! process. That fixes the node slice order, which fixes the cone-walk traversal
//! order in `compute_downstream_metrics` (`src/graph_store.rs:2775`), whose
//! `total_weight` is an order-sensitive f64 accumulation. The rounding at
//! `(total_weight * 100.0).round() / 100.0` turns those last-bit differences into
//! visible ones, and `compute_criticality` then divides by a max that has itself
//! shifted, so the perturbation spreads to every node.
//!
//! So a regenerated fixture will differ from the committed one even with no code
//! change. Diff it for *rank-order* movement, not byte equality, and expect noise
//! at the ~1% level in these three fields. `tests/ranking_regression.rs` builds the
//! graph the same way and is subject to the same noise; its assertions are coarse
//! enough to absorb it.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use mem::graph::GraphNode;
use mem::graph_store::GraphStore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusMetadata {
    pub corpus_id: String,
    pub date: String,
    pub source_commit: String,
    pub node_count: usize,
    pub task_count: usize,
    pub sampled_task_count: usize,
    pub ready_count: usize,
    pub blocked_count: usize,
    pub privacy_assessment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusFixture {
    pub metadata: CorpusMetadata,
    pub nodes: Vec<GraphNode>,
}

fn main() {
    let fixture_path = "tests/fixtures/ranking_corpus_20260824.json";
    let fixture_str = fs::read_to_string(fixture_path).expect("Failed to read fixture");
    let mut fixture: CorpusFixture =
        serde_json::from_str(&fixture_str).expect("Failed to parse fixture");

    println!("Loaded fixture with {} nodes", fixture.nodes.len());

    let dummy_root = Path::new("/tmp/pkb_corpus_root");
    let mut node_map = HashMap::with_capacity(fixture.nodes.len());
    for n in &mut fixture.nodes {
        if n.permalinks.is_empty() {
            n.permalinks.push(n.id.trim().to_lowercase());
            if let Some(ref pl) = n.permalink {
                n.permalinks.push(pl.trim().to_lowercase());
            }
        }
        if n.weak_keys.is_empty() {
            if let Some(stem) = n.path.file_stem().and_then(|s| s.to_str()) {
                n.weak_keys.push(stem.to_lowercase());
            }
        }
        node_map.insert(n.id.clone(), n.clone());
    }

    let graph = GraphStore::rebuild_from_nodes(node_map, dummy_root);

    let ready_count = graph.ready_tasks().len();
    let blocked_count = graph.blocked_tasks().len();
    println!(
        "Rebuilt graph: ready_count={}, blocked_count={}",
        ready_count, blocked_count
    );

    let mut updated_nodes = Vec::with_capacity(fixture.nodes.len());
    for original in &fixture.nodes {
        if let Some(recomputed) = graph.resolve(&original.id) {
            updated_nodes.push(recomputed.clone());
        } else {
            updated_nodes.push(original.clone());
        }
    }

    fixture.metadata.ready_count = ready_count;
    fixture.metadata.blocked_count = blocked_count;
    fixture.nodes = updated_nodes;

    let output_json = serde_json::to_string_pretty(&fixture).expect("Failed to serialize");
    fs::write(fixture_path, output_json).expect("Failed to write fixture");
    println!("Successfully regenerated {}", fixture_path);
}
