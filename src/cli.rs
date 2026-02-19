//! PKB CLI — interactive search and file management for the PKB vector store
//!
//! Provides subcommands: search, add, list, reindex, status

mod distance;
mod embeddings;
mod mcp_server;
mod pkb;
mod vectordb;

use anyhow::Result;
use clap::{Parser, Subcommand};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "mem", about = "PKB memory — semantic search over your knowledge base")]
struct Cli {
    /// Path to the PKB root directory
    #[arg(long, global = true, default_value_t = default_pkb_root())]
    pkb_root: String,

    /// Path to the persistent vector database file
    #[arg(long, global = true, default_value_t = default_db_path())]
    db_path: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Semantic search across your knowledge base
    Search {
        /// Search query
        query: Vec<String>,

        /// Maximum number of results
        #[arg(short = 'n', long, default_value_t = 5)]
        limit: usize,

        /// Show full snippets (not truncated)
        #[arg(short, long)]
        full: bool,
    },

    /// Add a file to the index
    Add {
        /// Path(s) to markdown files to add
        files: Vec<PathBuf>,
    },

    /// List indexed documents
    List {
        /// Filter by tag
        #[arg(short, long)]
        tag: Option<String>,

        /// Filter by document type
        #[arg(short = 'T', long = "type")]
        doc_type: Option<String>,

        /// Filter by status
        #[arg(short, long)]
        status: Option<String>,

        /// Show counts only
        #[arg(short, long)]
        count: bool,
    },

    /// Reindex all PKB files
    Reindex {
        /// Force reindex even if files unchanged
        #[arg(short, long)]
        force: bool,
    },

    /// Show index status
    Status,
}

fn default_pkb_root() -> String {
    std::env::var("ACA_DATA").unwrap_or_else(|_| {
        dirs::home_dir()
            .map(|h| h.join("brain").to_string_lossy().to_string())
            .unwrap_or_else(|| "~/brain".to_string())
    })
}

fn default_db_path() -> String {
    std::env::var("ACA_DATA")
        .map(|d| {
            PathBuf::from(d)
                .join("shodh_memory_data/pkb_vectors.bin")
                .to_string_lossy()
                .to_string()
        })
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .map(|h| {
                    h.join("brain/shodh_memory_data/pkb_vectors.bin")
                        .to_string_lossy()
                        .to_string()
                })
                .unwrap_or_else(|| "pkb_vectors.bin".to_string())
        })
}

fn load_store(db_path: &PathBuf, dim: usize) -> Result<Arc<RwLock<vectordb::VectorStore>>> {
    Ok(Arc::new(RwLock::new(
        vectordb::VectorStore::load_or_create(db_path, dim)?,
    )))
}

fn main() -> Result<()> {
    // Quiet logging for CLI mode — only warnings
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let pkb_root = PathBuf::from(&cli.pkb_root);
    let db_path = PathBuf::from(&cli.db_path);

    let embedder = Arc::new(embeddings::Embedder::new()?);
    let store = load_store(&db_path, embedder.dimension())?;

    match cli.command {
        Commands::Search { query, limit, full } => {
            let query_text = query.join(" ");
            if query_text.is_empty() {
                eprintln!("Error: search query cannot be empty");
                std::process::exit(1);
            }

            let query_embedding = embedder.encode(&query_text)?;
            let results = store.read().search(&query_embedding, limit);

            if results.is_empty() {
                println!("No results found for: {query_text}");
                return Ok(());
            }

            println!();
            for (i, result) in results.iter().enumerate() {
                let score_bar = score_to_bar(result.score);
                let tags = if result.tags.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", result.tags.join(", "))
                };

                println!(
                    "  \x1b[1;36m{}.\x1b[0m \x1b[1m{}\x1b[0m {score_bar}{tags}",
                    i + 1,
                    result.title,
                );
                println!(
                    "     \x1b[2m{}\x1b[0m",
                    result.path.display()
                );

                if !result.snippet.is_empty() {
                    let snippet = if full {
                        result.snippet.clone()
                    } else {
                        truncate_snippet(&result.snippet, 120)
                    };
                    println!("     {snippet}");
                }
                println!();
            }
        }

        Commands::Add { files } => {
            if files.is_empty() {
                eprintln!("Error: specify at least one file to add");
                std::process::exit(1);
            }

            let mut added = 0;
            let mut failed = 0;

            for file in &files {
                let path = if file.is_absolute() {
                    file.clone()
                } else {
                    std::env::current_dir()?.join(file)
                };

                if !path.exists() {
                    eprintln!("  \x1b[31m✗\x1b[0m {}: file not found", path.display());
                    failed += 1;
                    continue;
                }

                match pkb::parse_file(&path) {
                    Some(doc) => {
                        let title = doc.title.clone();
                        match store.write().upsert(&doc, &embedder) {
                            Ok(()) => {
                                println!("  \x1b[32m✓\x1b[0m {title}");
                                added += 1;
                            }
                            Err(e) => {
                                eprintln!("  \x1b[31m✗\x1b[0m {}: {e}", path.display());
                                failed += 1;
                            }
                        }
                    }
                    None => {
                        eprintln!("  \x1b[31m✗\x1b[0m {}: failed to parse", path.display());
                        failed += 1;
                    }
                }
            }

            // Save
            store.read().save(&db_path)?;
            println!("\n{added} added, {failed} failed, {} total", store.read().len());
        }

        Commands::List {
            tag,
            doc_type,
            status,
            count,
        } => {
            let results = store.read().list_documents(
                tag.as_deref(),
                doc_type.as_deref(),
                status.as_deref(),
            );

            if count {
                println!("{}", results.len());
                return Ok(());
            }

            if results.is_empty() {
                println!("No documents found.");
                return Ok(());
            }

            println!();
            for result in &results {
                let meta = format_meta(&result.doc_type, &result.status, &result.tags);
                println!(
                    "  \x1b[1m{}\x1b[0m{meta}",
                    result.title,
                );
                println!(
                    "  \x1b[2m{}\x1b[0m",
                    result.path.display()
                );
                println!();
            }
            println!("{} documents", results.len());
        }

        Commands::Reindex { force } => {
            let (indexed, removed, total) =
                index_pkb(&pkb_root, &store, &embedder, force);
            store.read().save(&db_path)?;
            println!("✓ {total} documents ({indexed} indexed, {removed} removed)");
        }

        Commands::Status => {
            let s = store.read();
            let total = s.len();
            let db_size = std::fs::metadata(&db_path)
                .map(|m| m.len())
                .unwrap_or(0);

            println!("PKB root:  {}", pkb_root.display());
            println!("DB path:   {}", db_path.display());
            println!("Documents: {total}");
            println!("DB size:   {:.1} MB", db_size as f64 / 1_048_576.0);
        }
    }

    Ok(())
}

fn index_pkb(
    pkb_root: &std::path::Path,
    store: &Arc<RwLock<vectordb::VectorStore>>,
    embedder: &embeddings::Embedder,
    force_all: bool,
) -> (usize, usize, usize) {
    use indicatif::{ProgressBar, ProgressStyle};

    let files = pkb::scan_directory(pkb_root);

    let existing_paths: std::collections::HashSet<String> = files
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let removed = {
        let mut store = store.write();
        store.remove_deleted(&existing_paths)
    };

    // Figure out which files need updating
    let to_process: Vec<_> = files
        .iter()
        .filter(|file_path| {
            let path_str = file_path.to_string_lossy().to_string();
            let mtime = std::fs::metadata(file_path)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            force_all || {
                let store = store.read();
                store.needs_update(&path_str, mtime)
            }
        })
        .collect();

    let skipped = files.len() - to_process.len();
    if skipped > 0 {
        eprintln!("  {skipped} files unchanged, {} to index", to_process.len());
    }

    let pb = ProgressBar::new(to_process.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "  {bar:30.cyan/dim} {pos}/{len} [{elapsed_precise}] {per_sec} {msg}"
        )
        .unwrap()
        .progress_chars("━╸─"),
    );

    let mut indexed = 0;
    let mut failed = 0;

    for file_path in &to_process {
        let filename = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?");
        pb.set_message(filename.to_string());

        match pkb::parse_file(file_path) {
            Some(doc) => {
                let mut store = store.write();
                match store.upsert(&doc, embedder) {
                    Ok(()) => {
                        indexed += 1;
                    }
                    Err(e) => {
                        pb.suspend(|| {
                            eprintln!("  ✗ {}: {e}", file_path.display());
                        });
                        failed += 1;
                    }
                }
            }
            None => {
                failed += 1;
            }
        }
        pb.inc(1);
    }

    pb.finish_and_clear();

    let total = store.read().len();
    (indexed, removed, total)
}

fn score_to_bar(score: f32) -> String {
    let normalized = ((score + 1.0) / 2.0).clamp(0.0, 1.0);
    let filled = (normalized * 10.0) as usize;
    let bar: String = "█".repeat(filled) + &"░".repeat(10 - filled);
    format!("\x1b[33m{bar}\x1b[0m \x1b[2m{score:.3}\x1b[0m")
}

fn truncate_snippet(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    let mut end = max_len;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn format_meta(
    doc_type: &Option<String>,
    status: &Option<String>,
    tags: &[String],
) -> String {
    let mut parts = Vec::new();
    if let Some(t) = doc_type {
        parts.push(format!("\x1b[35m{t}\x1b[0m"));
    }
    if let Some(s) = status {
        parts.push(format!("\x1b[33m{s}\x1b[0m"));
    }
    if !tags.is_empty() {
        parts.push(format!("\x1b[36m[{}]\x1b[0m", tags.join(", ")));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("  {}", parts.join(" "))
    }
}
