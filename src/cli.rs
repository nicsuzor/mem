//! PKB CLI — interactive search and file management for the PKB vector store
//!
//! Provides subcommands: search, add, list, reindex, status

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
use clap::{Parser, Subcommand};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "aops", about = "AcademicOps — semantic search and task management for your knowledge base")]
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

    /// List tasks (ready, blocked, or all)
    Tasks {
        /// Filter: ready, blocked, all (default: ready)
        #[arg(default_value = "ready")]
        filter: String,

        /// Filter by project
        #[arg(short, long)]
        project: Option<String>,

        /// Sort by: priority, weight, due (default: priority)
        #[arg(short, long, default_value = "priority")]
        sort: String,
    },

    /// Show task details and relationships
    Task {
        /// Task ID
        id: String,
    },

    /// Show dependency tree for a task
    Deps {
        /// Task ID
        id: String,

        /// Show as tree
        #[arg(long)]
        tree: bool,
    },

    /// Show network metrics for a document or all tasks
    Metrics {
        /// Document/task ID (omit for summary)
        id: Option<String>,
    },

    /// Create a new task
    New {
        /// Task title
        title: Vec<String>,

        /// Parent task ID
        #[arg(long)]
        parent: Option<String>,

        /// Priority (0=critical, 1=high, 2=medium, 3=low, 4=someday)
        #[arg(short, long)]
        priority: Option<i32>,

        /// Project name
        #[arg(long)]
        project: Option<String>,

        /// Tags (comma-separated)
        #[arg(short, long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
    },

    /// Mark a task as done
    Done {
        /// Task ID
        id: String,
    },

    /// Update task frontmatter fields
    Update {
        /// Task ID
        id: String,

        /// Status (active, in_progress, blocked, waiting, review, merge_ready, done, cancelled)
        #[arg(short, long)]
        status: Option<String>,

        /// Priority (0=critical, 1=high, 2=medium, 3=low, 4=someday)
        #[arg(short, long)]
        priority: Option<i32>,

        /// Project name
        #[arg(long)]
        project: Option<String>,

        /// Assignee
        #[arg(short, long)]
        assignee: Option<String>,

        /// Tags (comma-separated, replaces existing)
        #[arg(short, long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
    },

    /// Export knowledge graph
    Graph {
        /// Output format: json, graphml, dot, mcp-index, all
        #[arg(short, long, default_value = "json")]
        format: String,

        /// Output file (default: stdout; for 'all' format, used as base name)
        #[arg(short, long)]
        output: Option<String>,
    },
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
                index_pkb(&pkb_root, &db_path, &store, &embedder, force);
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

        Commands::Tasks {
            filter,
            project,
            sort,
        } => {
            let gs = graph_store::GraphStore::build_from_directory(&pkb_root);

            let tasks: Vec<&graph::GraphNode> = match filter.as_str() {
                "blocked" => gs.blocked_tasks(),
                "all" => gs.all_tasks(),
                _ => gs.ready_tasks(), // "ready" is default
            };

            // Filter by project
            let tasks: Vec<&&graph::GraphNode> = if let Some(ref proj) = project {
                tasks.iter().filter(|t| t.project.as_deref() == Some(proj)).collect()
            } else {
                tasks.iter().collect()
            };

            if tasks.is_empty() {
                println!("No {} tasks found.", filter);
                return Ok(());
            }

            println!();
            println!(
                "  \x1b[1m{:<12} {:>4}  {:>6}  {}\x1b[0m",
                "ID", "PRI", "WEIGHT", "TITLE"
            );
            println!("  {}", "-".repeat(60));

            for task in &tasks {
                let pri = task.priority.unwrap_or(2);
                let pri_color = match pri {
                    0 => "\x1b[31m",  // red
                    1 => "\x1b[33m",  // yellow
                    2 => "\x1b[0m",   // default
                    3 => "\x1b[2m",   // dim
                    _ => "\x1b[2m",
                };
                let weight = if task.downstream_weight > 0.0 {
                    format!("{:.1}", task.downstream_weight)
                } else {
                    "-".to_string()
                };
                let exposure = if task.stakeholder_exposure { "!" } else { "" };

                println!(
                    "  {:<12} {pri_color}{:>4}\x1b[0m  {:>5}{:<1}  {}",
                    task.task_id.as_deref().unwrap_or(&task.id),
                    pri,
                    weight,
                    exposure,
                    task.label,
                );
            }
            println!("\n  {} {} tasks", tasks.len(), filter);
        }

        Commands::Task { id } => {
            let gs = graph_store::GraphStore::build_from_directory(&pkb_root);

            match gs.get_node(&id) {
                Some(node) => {
                    println!();
                    println!("  \x1b[1m{}\x1b[0m", node.label);
                    println!("  \x1b[2m{}\x1b[0m", node.path.display());
                    println!();

                    if let Some(ref t) = node.node_type {
                        println!("  Type:     {t}");
                    }
                    if let Some(ref s) = node.status {
                        println!("  Status:   {s}");
                    }
                    if let Some(p) = node.priority {
                        println!("  Priority: {p}");
                    }
                    if let Some(ref proj) = node.project {
                        println!("  Project:  {proj}");
                    }
                    if let Some(ref due) = node.due {
                        println!("  Due:      {due}");
                    }
                    if let Some(ref a) = node.assignee {
                        println!("  Assignee: {a}");
                    }
                    if !node.tags.is_empty() {
                        println!("  Tags:     {}", node.tags.join(", "));
                    }

                    if !node.depends_on.is_empty() {
                        println!("\n  \x1b[1mDepends on:\x1b[0m");
                        for dep in &node.depends_on {
                            let label = gs.get_node(dep).map(|n| n.label.as_str()).unwrap_or("?");
                            println!("    <- {dep} ({label})");
                        }
                    }
                    if !node.blocks.is_empty() {
                        println!("\n  \x1b[1mBlocks:\x1b[0m");
                        for b in &node.blocks {
                            let label = gs.get_node(b).map(|n| n.label.as_str()).unwrap_or("?");
                            println!("    -> {b} ({label})");
                        }
                    }
                    if !node.children.is_empty() {
                        println!("\n  \x1b[1mChildren:\x1b[0m");
                        for c in &node.children {
                            let label = gs.get_node(c).map(|n| n.label.as_str()).unwrap_or("?");
                            println!("    {c} ({label})");
                        }
                    }
                    if let Some(ref p) = node.parent {
                        let label = gs.get_node(p).map(|n| n.label.as_str()).unwrap_or("?");
                        println!("\n  \x1b[1mParent:\x1b[0m {p} ({label})");
                    }

                    if node.downstream_weight > 0.0 {
                        println!(
                            "\n  Weight: {:.2}{}",
                            node.downstream_weight,
                            if node.stakeholder_exposure {
                                " (stakeholder exposure)"
                            } else {
                                ""
                            }
                        );
                    }
                    println!();
                }
                None => {
                    eprintln!("Task not found: {id}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Deps { id, tree } => {
            let gs = graph_store::GraphStore::build_from_directory(&pkb_root);

            if gs.get_node(&id).is_none() {
                eprintln!("Task not found: {id}");
                std::process::exit(1);
            }

            let deps = gs.dependency_tree(&id);
            if deps.is_empty() {
                println!("No dependencies for {id}");
                return Ok(());
            }

            println!();
            for (dep_id, depth) in &deps {
                let indent = if tree {
                    "  ".repeat(*depth)
                } else {
                    "  ".to_string()
                };
                let label = gs
                    .get_node(dep_id)
                    .map(|n| n.label.as_str())
                    .unwrap_or("?");
                let status = gs
                    .get_node(dep_id)
                    .and_then(|n| n.status.as_deref())
                    .unwrap_or("?");
                println!("{indent}{dep_id} [{status}] {label}");
            }
            println!();
        }

        Commands::Metrics { id } => {
            let gs = graph_store::GraphStore::build_from_directory(&pkb_root);
            let node_ids: Vec<String> = gs.nodes().map(|n| n.id.clone()).collect();
            let edges = gs.edges();

            match id {
                Some(ref nid) => {
                    let node = gs.get_node(nid);
                    if node.is_none() {
                        eprintln!("Node not found: {nid}");
                        std::process::exit(1);
                    }
                    let node = node.unwrap();
                    let m = metrics::compute_network_metrics(
                        nid,
                        &node_ids,
                        edges,
                        node.downstream_weight,
                        node.stakeholder_exposure,
                    );
                    if let Some(m) = m {
                        println!();
                        println!("  \x1b[1m{}\x1b[0m", node.label);
                        println!("  In-degree:           {}", m.in_degree);
                        println!("  Out-degree:          {}", m.out_degree);
                        println!("  Downstream weight:   {:.2}", m.downstream_weight);
                        println!("  Stakeholder:         {}", m.stakeholder_exposure);
                        println!("  Betweenness:         {:.4}", m.betweenness);
                        println!("  PageRank:            {:.4}", m.pagerank);
                        println!();
                    }
                }
                None => {
                    // Summary: top 10 by pagerank
                    let pr = metrics::compute_pagerank(&node_ids, edges);
                    let mut ranked: Vec<_> = pr.iter().collect();
                    ranked.sort_by(|a, b| {
                        b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal)
                    });

                    println!();
                    println!(
                        "  \x1b[1m{:<30} {:>10}\x1b[0m",
                        "NODE", "PAGERANK"
                    );
                    println!("  {}", "-".repeat(42));

                    for (id, score) in ranked.iter().take(20) {
                        let label = gs
                            .get_node(id)
                            .map(|n| n.label.as_str())
                            .unwrap_or("?");
                        let display = if label.len() > 28 {
                            format!("{}...", &label[..25])
                        } else {
                            label.to_string()
                        };
                        println!("  {:<30} {:>10.4}", display, score);
                    }
                    println!(
                        "\n  {} nodes, {} edges",
                        gs.node_count(),
                        gs.edge_count()
                    );
                    println!();
                }
            }
        }

        Commands::New {
            title,
            parent,
            priority,
            project,
            tags,
        } => {
            let title_str = title.join(" ");
            if title_str.is_empty() {
                eprintln!("Error: title cannot be empty");
                std::process::exit(1);
            }

            let fields = document_crud::TaskFields {
                title: title_str,
                parent,
                priority,
                project,
                tags: tags.unwrap_or_default(),
                ..Default::default()
            };

            match document_crud::create_task(&pkb_root, fields) {
                Ok(path) => {
                    println!("Created: {}", path.display());

                    // Auto-index the new file
                    if let Some(doc) = pkb::parse_file(&path) {
                        match store.write().upsert(&doc, &embedder) {
                            Ok(()) => {
                                store.read().save(&db_path)?;
                                println!("Indexed: {}", doc.title);
                            }
                            Err(e) => eprintln!("Warning: failed to index: {e}"),
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Done { id } => {
            let gs = graph_store::GraphStore::build_from_directory(&pkb_root);

            match gs.get_node(&id) {
                Some(node) => {
                    let path = node.path.clone();
                    let mut updates = std::collections::HashMap::new();
                    updates.insert(
                        "status".to_string(),
                        serde_json::Value::String("done".to_string()),
                    );

                    document_crud::update_document(&path, updates)?;

                    // Re-index the updated file
                    if let Some(doc) = pkb::parse_file(&path) {
                        let _ = store.write().upsert(&doc, &embedder);
                        store.read().save(&db_path)?;
                    }

                    println!("Done: {} ({})", node.label, id);
                }
                None => {
                    eprintln!("Task not found: {id}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Update {
            id,
            status,
            priority,
            project,
            assignee,
            tags,
        } => {
            let gs = graph_store::GraphStore::build_from_directory(&pkb_root);

            match gs.get_node(&id) {
                Some(node) => {
                    let path = node.path.clone();
                    let mut updates = std::collections::HashMap::new();

                    if let Some(s) = status {
                        updates.insert("status".to_string(), serde_json::Value::String(s));
                    }
                    if let Some(p) = priority {
                        updates.insert(
                            "priority".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(p)),
                        );
                    }
                    if let Some(proj) = project {
                        updates.insert("project".to_string(), serde_json::Value::String(proj));
                    }
                    if let Some(a) = assignee {
                        updates.insert("assignee".to_string(), serde_json::Value::String(a));
                    }
                    if let Some(t) = tags {
                        let tag_values: Vec<serde_json::Value> =
                            t.into_iter().map(serde_json::Value::String).collect();
                        updates.insert("tags".to_string(), serde_json::Value::Array(tag_values));
                    }

                    if updates.is_empty() {
                        eprintln!("No updates specified. Use --status, --priority, --project, --assignee, or --tags.");
                        std::process::exit(1);
                    }

                    document_crud::update_document(&path, updates)?;

                    // Re-index the updated file
                    if let Some(doc) = pkb::parse_file(&path) {
                        let _ = store.write().upsert(&doc, &embedder);
                        store.read().save(&db_path)?;
                    }

                    println!("Updated: {} ({})", node.label, id);
                }
                None => {
                    eprintln!("Task not found: {id}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Graph { format, output } => {
            let gs = graph_store::GraphStore::build_from_directory(&pkb_root);

            match format.to_lowercase().as_str() {
                "all" => {
                    let base = output.as_deref().unwrap_or("graph");
                    let base = base
                        .trim_end_matches(".json")
                        .trim_end_matches(".graphml")
                        .trim_end_matches(".dot");

                    let json_path = format!("{base}.json");
                    std::fs::write(&json_path, gs.output_json()?)?;
                    println!("  Saved {json_path}");

                    let graphml_path = format!("{base}.graphml");
                    std::fs::write(&graphml_path, gs.output_graphml())?;
                    println!("  Saved {graphml_path}");

                    let dot_path = format!("{base}.dot");
                    std::fs::write(&dot_path, gs.output_dot())?;
                    println!("  Saved {dot_path}");

                    println!(
                        "Graph: {} nodes, {} edges (3 formats)",
                        gs.node_count(),
                        gs.edge_count(),
                    );
                }
                "mcp-index" => {
                    let index = task_index::build_mcp_index(&gs, &pkb_root);
                    let json = serde_json::to_string_pretty(&index)?;

                    match output {
                        Some(path) => {
                            std::fs::write(&path, &json)?;
                            println!(
                                "MCP index: {} tasks, {} ready, {} blocked -> {}",
                                index.tasks.len(),
                                index.ready.len(),
                                index.blocked.len(),
                                path,
                            );
                        }
                        None => print!("{json}"),
                    }
                }
                _ => {
                    let content = match format.as_str() {
                        "graphml" => gs.output_graphml(),
                        "dot" => gs.output_dot(),
                        _ => gs.output_json()?,
                    };

                    match output {
                        Some(path) => {
                            std::fs::write(&path, &content)?;
                            println!(
                                "Graph: {} nodes, {} edges -> {}",
                                gs.node_count(),
                                gs.edge_count(),
                                path
                            );
                        }
                        None => print!("{content}"),
                    }
                }
            }
        }
    }

    Ok(())
}

fn index_pkb(
    pkb_root: &std::path::Path,
    db_path: &std::path::Path,
    store: &Arc<RwLock<vectordb::VectorStore>>,
    embedder: &embeddings::Embedder,
    force_all: bool,
) -> (usize, usize, usize) {
    use indicatif::{ProgressBar, ProgressStyle};
    use rayon::prelude::*;

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
        .cloned()
        .collect();

    let skipped = files.len() - to_process.len();
    if skipped > 0 {
        eprintln!("  {skipped} files unchanged, {} to index", to_process.len());
    }

    if to_process.is_empty() {
        let total = store.read().len();
        return (0, removed, total);
    }

    let pb = ProgressBar::new(to_process.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "  {bar:30.cyan/dim} {pos}/{len} [{elapsed_precise}] {per_sec} {msg}"
        )
        .unwrap()
        .progress_chars("━╸─"),
    );

    // Parse all files in parallel with rayon
    pb.set_message("parsing...");
    let parsed: Vec<_> = to_process
        .par_iter()
        .filter_map(|path| {
            pkb::parse_file(path).map(|doc| {
                let text = doc.embedding_text();
                let chunks = embeddings::chunk_text(&text, &embeddings::ChunkConfig::default());
                (doc, chunks)
            })
        })
        .collect();

    // Batch embed and store — batches of 100 docs with progressive saves
    let mut indexed = 0;

    for batch in parsed.chunks(100) {
        // Collect all chunks from this batch
        let mut all_chunks: Vec<&str> = Vec::new();
        let mut chunk_counts: Vec<usize> = Vec::new();

        for (_doc, chunks) in batch {
            chunk_counts.push(chunks.len());
            for chunk in chunks {
                all_chunks.push(chunk.as_str());
            }
        }

        pb.set_message("embedding...");
        match embedder.encode_batch(&all_chunks) {
            Ok(all_embeddings) => {
                let mut emb_offset = 0;
                let mut s = store.write();
                for (i, (doc, chunks)) in batch.iter().enumerate() {
                    let count = chunk_counts[i];
                    let doc_embeddings = all_embeddings[emb_offset..emb_offset + count].to_vec();
                    emb_offset += count;
                    s.insert_precomputed(doc, chunks.clone(), doc_embeddings);
                    indexed += 1;
                }
            }
            Err(e) => {
                pb.suspend(|| eprintln!("  ✗ batch embed failed: {e}"));
            }
        }
        pb.inc(batch.len() as u64);

        // Progressive save so interrupted runs don't lose work
        if let Err(e) = store.read().save(db_path) {
            pb.suspend(|| eprintln!("  ✗ progressive save failed: {e}"));
        }
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
