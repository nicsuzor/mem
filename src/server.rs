//! PKB Search MCP Server — entry point
//!
//! An in-memory vector database for semantic search over personal knowledge base
//! markdown files. Provides an MCP interface via stdio transport.
//!
//! Indexing is handled externally by `aops reindex`. This server only reads
//! the pre-built vector store and errors out if the index is stale.

use mem::{embeddings, graph_store, mcp_server, vectordb};

use anyhow::Result;
use clap::Parser;
use parking_lot::RwLock;
use rmcp::ServiceExt;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "pkb", version, about = "PKB semantic search MCP server")]
struct Cli {
    /// Path to the PKB root directory containing markdown files
    #[arg(default_value_t = default_pkb_root())]
    pkb_root: String,

    /// Path to the persistent vector database file
    #[arg(long, default_value_t = default_db_path())]
    db_path: String,

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

    // Check index freshness — fail fast if stale
    eprintln!("   Checking index freshness...");
    let stale_count = mem::check_index_staleness(&pkb_root, &store);
    if stale_count > 0 {
        eprintln!("   ✗ Index is stale: {stale_count} document(s) need re-indexing.");
        eprintln!("   Run `aops reindex` before starting the server.");
        std::process::exit(1);
    }
    let total = store.read().len();
    eprintln!("   ✓ Index is fresh ({total} documents)");

    // Build graph store
    eprintln!("   Building knowledge graph...");
    let graph_store = graph_store::GraphStore::build_from_directory(&pkb_root);
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
