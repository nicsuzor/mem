//! PKB Search MCP Server — entry point
//!
//! An in-memory vector database for semantic search over personal knowledge base
//! markdown files. Provides an MCP interface via stdio transport.

mod distance;
mod embeddings;
mod graph;
mod graph_store;
mod mcp_server;
mod metrics;
mod pkb;
mod document_crud;
mod task_index;
mod vectordb;

use anyhow::Result;
use clap::Parser;
use parking_lot::RwLock;
use rmcp::ServiceExt;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "pkb-search", version, about = "PKB semantic search MCP server")]
struct Cli {
    /// Path to the PKB root directory containing markdown files
    #[arg(default_value_t = default_pkb_root())]
    pkb_root: String,

    /// Path to the persistent vector database file
    #[arg(long, default_value_t = default_db_path())]
    db_path: String,

    /// Force full reindex on startup
    #[arg(long, default_value_t = false)]
    reindex: bool,
}

fn default_pkb_root() -> String {
    std::env::var("ACA_DATA").unwrap_or_else(|_| ".".to_string())
}

fn default_db_path() -> String {
    std::env::var("ACA_DATA")
        .map(|d| {
            PathBuf::from(d)
                .join("pkb_vectors.bin")
                .to_string_lossy()
                .to_string()
        })
        .unwrap_or_else(|_| "pkb_vectors.bin".to_string())
}

/// Index PKB files into the vector store. Returns (indexed, removed, total).
pub fn index_pkb(
    pkb_root: &std::path::Path,
    db_path: &std::path::Path,
    store: &Arc<RwLock<vectordb::VectorStore>>,
    embedder: &embeddings::Embedder,
    force_all: bool,
) -> (usize, usize, usize) {
    let files = pkb::scan_directory(pkb_root);
    tracing::info!("Found {} markdown files in {}", files.len(), pkb_root.display());

    // Use relative paths for store keys (portable across machines)
    let existing_paths: HashSet<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(pkb_root)
                .unwrap_or(p)
                .to_string_lossy()
                .to_string()
        })
        .collect();

    // Remove deleted files
    let removed = {
        let mut store = store.write();
        store.remove_deleted(&existing_paths)
    };

    // Index new/modified files
    let mut indexed = 0;
    for file_path in &files {
        let rel_path = file_path
            .strip_prefix(pkb_root)
            .unwrap_or(file_path);
        let path_str = rel_path.to_string_lossy().to_string();

        // Check if file needs updating
        let mtime = std::fs::metadata(file_path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let needs_update = force_all || {
            let store = store.read();
            store.needs_update(&path_str, mtime)
        };

        if !needs_update {
            continue;
        }

        match pkb::parse_file_relative(file_path, pkb_root) {
            Some(doc) => {
                let mut store = store.write();
                match store.upsert(&doc, embedder) {
                    Ok(()) => {
                        indexed += 1;
                        tracing::debug!("Indexed: {}", doc.title);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to index {}: {e}", file_path.display());
                    }
                }
            }
            None => {
                tracing::debug!("Skipped (parse failed): {}", file_path.display());
            }
        }
    }

    let total = store.read().len();
    tracing::info!("Indexing complete: {indexed} indexed, {removed} removed, {total} total");

    (indexed, removed, total)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging to stderr (stdout is for MCP protocol)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let pkb_root = PathBuf::from(&cli.pkb_root);
    let db_path = PathBuf::from(&cli.db_path);

    eprintln!("🔍 PKB Search MCP Server starting...");
    eprintln!("   PKB root: {}", pkb_root.display());
    eprintln!("   DB path:  {}", db_path.display());

    // Initialize embedder
    eprintln!("   Loading embedder...");
    let embedder = Arc::new(embeddings::Embedder::new()?);

    // Load or create vector store
    let store = Arc::new(RwLock::new(vectordb::VectorStore::load_or_create(
        &db_path,
        embedder.dimension(),
    )?));

    // Index PKB files
    eprintln!("   Indexing PKB files...");
    let (indexed, removed, total) = index_pkb(&pkb_root, &db_path, &store, &embedder, cli.reindex);
    eprintln!("   ✓ {total} documents indexed ({indexed} new/updated, {removed} removed)");

    // Save after initial indexing
    {
        let store_read = store.read();
        store_read.save(&db_path)?;
    }

    // Build graph store and persist for CLI consumption
    eprintln!("   Building knowledge graph...");
    let graph_store = graph_store::GraphStore::build_from_directory(&pkb_root);
    let graph_path = db_path.with_extension("graph.json");
    let _ = graph_store.save(&graph_path);
    let graph = Arc::new(RwLock::new(graph_store));
    eprintln!(
        "   {} nodes, {} edges",
        graph.read().node_count(),
        graph.read().edge_count()
    );

    // Create and start MCP server
    eprintln!("   Starting MCP server on stdio...");
    let server = mcp_server::PkbSearchServer::new(
        store.clone(),
        embedder.clone(),
        pkb_root.clone(),
        db_path.clone(),
        graph.clone(),
    );

    let service = server.serve(rmcp::transport::stdio()).await?;
    eprintln!("   ✓ MCP server ready");

    // Wait for client to disconnect
    service.waiting().await?;

    // Save on shutdown
    eprintln!("   Saving vector store...");
    let store_read = store.read();
    store_read.save(&db_path)?;
    eprintln!("   ✓ Shutdown complete");

    Ok(())
}
