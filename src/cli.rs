//! PKB CLI — interactive search and task management for the PKB vector store
//!
//! Provides subcommands: search, add, reindex, status, tasks, task, deps, ...

mod tui;

use mem::{document_crud, embeddings, graph, graph_display, graph_store, metrics, pkb, task_index, vectordb};

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "aops", version, about = "AcademicOps — semantic search and task management for your knowledge base")]
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

#[derive(Clone, ValueEnum)]
enum TaskFilter {
    /// Actionable leaf tasks with no unmet dependencies
    Ready,
    /// Tasks waiting on unfinished dependencies
    Blocked,
    /// All open tasks (excludes done/cancelled)
    All,
}

impl std::fmt::Display for TaskFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskFilter::Ready => write!(f, "ready"),
            TaskFilter::Blocked => write!(f, "blocked"),
            TaskFilter::All => write!(f, "all"),
        }
    }
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

    /// Reindex all PKB files
    Reindex {
        /// Force reindex even if files unchanged
        #[arg(short, long)]
        force: bool,
    },

    /// Show index status
    Status,

    /// List tasks (ready, blocked, or all) — tree view by default
    Tasks {
        /// Which tasks to show
        #[arg(default_value = "ready", value_enum)]
        filter: TaskFilter,

        /// Filter by project
        #[arg(short, long)]
        project: Option<String>,

        /// Show flat table instead of tree
        #[arg(long)]
        flat: bool,

        /// Sort order: priority (default), weight, due
        #[arg(short, long)]
        sort: Option<String>,
    },

    /// Show top focus tasks — what to work on right now
    Focus {
        /// Maximum number of focus picks
        #[arg(short = 'n', long, default_value_t = 5)]
        limit: usize,
    },

    /// Show details, metadata, and local graph context for any document
    #[command(alias = "task")]
    Show {
        /// Document ID (flexible resolution)
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

        /// Dependencies (comma-separated task IDs)
        #[arg(long = "depends-on", value_delimiter = ',')]
        depends_on: Option<Vec<String>>,

        /// Assignee
        #[arg(short, long)]
        assignee: Option<String>,

        /// Complexity (mechanical, requires-judgment, multi-step, needs-decomposition, blocked-human)
        #[arg(long)]
        complexity: Option<String>,

        /// Body text / description
        #[arg(short, long)]
        body: Option<String>,
    },

    /// Create a new document (note, knowledge, memory, or any type)
    Remember {
        /// Document title
        title: Vec<String>,

        /// Document type (default: note). Examples: note, knowledge, memory, insight, observation
        #[arg(short = 'T', long = "type", default_value = "note")]
        doc_type: String,

        /// Tags (comma-separated)
        #[arg(short, long, value_delimiter = ',')]
        tags: Option<Vec<String>>,

        /// Status
        #[arg(short, long)]
        status: Option<String>,

        /// Priority (0=critical, 1=high, 2=medium, 3=low, 4=someday)
        #[arg(short, long)]
        priority: Option<i32>,

        /// Parent document ID
        #[arg(long)]
        parent: Option<String>,

        /// Project name
        #[arg(long)]
        project: Option<String>,

        /// Source context (e.g. session ID)
        #[arg(long)]
        source: Option<String>,

        /// Body text
        #[arg(short, long)]
        body: Option<String>,

        /// Override subdirectory placement
        #[arg(long)]
        dir: Option<String>,
    },

    /// Append timestamped content to an existing document
    Append {
        /// Document ID (flexible resolution)
        id: String,

        /// Content to append
        content: Vec<String>,

        /// Target section heading (e.g. "Log", "References")
        #[arg(long)]
        section: Option<String>,
    },

    /// Delete a task or memory by ID
    Delete {
        /// Task or memory ID
        id: String,
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

    /// Show full knowledge neighbourhood for a node
    Context {
        /// Node ID, task ID, filename stem, or title
        id: String,

        /// Neighbourhood radius in hops
        #[arg(long, default_value_t = 2)]
        hops: usize,
    },

    /// Find shortest paths between two nodes
    Trace {
        /// Source node (ID, filename, or title)
        from: String,

        /// Target node (ID, filename, or title)
        to: String,

        /// Maximum paths to show
        #[arg(short = 'n', long, default_value_t = 3)]
        max_paths: usize,
    },

    /// Find disconnected (orphan) nodes with no edges
    Orphans {
        /// Filter by node type (e.g. task, project, note)
        #[arg(short = 'T', long = "type")]
        node_type: Option<String>,

        /// Filter by project
        #[arg(short = 'P', long)]
        project: Option<String>,
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

    /// Search memories by semantic similarity
    Recall {
        /// Search query
        query: Vec<String>,

        /// Filter by tags (comma-separated)
        #[arg(short, long, value_delimiter = ',')]
        tags: Option<Vec<String>>,

        /// Maximum number of results
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
    },

    /// List or search tags across the knowledge base
    Tags {
        /// Tags to search for (comma-separated). Omit to show tag summary.
        #[arg(value_delimiter = ',')]
        search_tags: Option<Vec<String>>,

        /// Filter by document type
        #[arg(short = 'T', long = "type")]
        doc_type: Option<String>,

        /// Show only counts
        #[arg(long)]
        count: bool,
    },

    /// Delete a memory by ID
    Forget {
        /// Memory ID
        id: String,
    },

    /// List memories
    Memories {
        /// Filter by tags (comma-separated)
        #[arg(short, long, value_delimiter = ',')]
        tags: Option<Vec<String>>,

        /// Maximum number of results
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },

    /// Show what completing a task would unblock
    Blocks {
        /// Task ID
        id: String,

        /// Show as tree
        #[arg(long)]
        tree: bool,
    },

    /// Launch the interactive planning TUI
    Tui,
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

/// Load cached graph from disk (written by MCP server), falling back to directory scan.
fn load_graph(pkb_root: &std::path::Path, db_path: &std::path::Path) -> graph_store::GraphStore {
    let graph_path = db_path.with_extension("graph.json");
    match graph_store::GraphStore::load(&graph_path) {
        Ok(gs) => gs,
        Err(e) => {
            tracing::debug!("Graph cache load failed ({}): {e}", graph_path.display());
            graph_store::GraphStore::build_from_directory(pkb_root)
        }
    }
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

    // Only load embedder + vector store for commands that need them
    let needs_embedder = matches!(
        cli.command,
        Commands::Search { .. }
            | Commands::Add { .. }
            | Commands::Reindex { .. }
            | Commands::Status
            | Commands::Recall { .. }
    );

    // Some commands need the store but not the embedder
    let needs_store_only = matches!(
        cli.command,
        Commands::Tags { .. } | Commands::Memories { .. } | Commands::Forget { .. }
    );

    let (embedder, store) = if needs_embedder {
        let e = Arc::new(embeddings::Embedder::new()?);
        let s = load_store(&db_path, e.dimension())?;
        (Some(e), Some(s))
    } else if needs_store_only {
        let s = load_store(&db_path, embeddings::EMBEDDING_DIM)?;
        (None, Some(s))
    } else {
        (None, None)
    };

    match cli.command {
        Commands::Search { query, limit, full } => {
            let embedder = embedder.as_ref().unwrap();
            let store = store.as_ref().unwrap();
            let query_text = query.join(" ");
            if query_text.is_empty() {
                eprintln!("Error: search query cannot be empty");
                std::process::exit(1);
            }

            let query_embedding = embedder.encode_query(&query_text)?;
            let results = store.read().search(&query_embedding, limit, &pkb_root);

            if results.is_empty() {
                println!("No results found for: {query_text}");
                return Ok(());
            }

            println!();
            let mut first_id = String::new();
            for (i, result) in results.iter().enumerate() {
                let score_bar = score_to_bar(result.score);
                let tags = if result.tags.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", result.tags.join(", "))
                };

                // Show ID prominently if available
                let id_str = result.id.as_deref().unwrap_or("");
                if i == 0 && !id_str.is_empty() {
                    first_id = id_str.to_string();
                }

                println!(
                    "  \x1b[1;36m{}.\x1b[0m \x1b[1m{}\x1b[0m {score_bar}{tags}",
                    i + 1,
                    result.title,
                );
                if !id_str.is_empty() {
                    println!("     \x1b[36m{id_str}\x1b[0m  \x1b[2m{}\x1b[0m", result.path.display());
                } else {
                    println!("     \x1b[2m{}\x1b[0m", result.path.display());
                }

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

            // Navigation hint
            if !first_id.is_empty() {
                println!("  \x1b[2mTip: aops task {first_id}  — show full details\x1b[0m");
            }
        }

        Commands::Add { files } => {
            let embedder = embedder.as_ref().unwrap();
            let store = store.as_ref().unwrap();
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

                match pkb::parse_file_relative(&path, &pkb_root) {
                    Some(doc) => {
                        let title = doc.title.clone();
                        match store.write().upsert(&doc, embedder) {
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

        Commands::Reindex { force } => {
            let embedder = embedder.as_ref().unwrap();
            let store = store.as_ref().unwrap();
            let (indexed, removed, total) =
                index_pkb(&pkb_root, &db_path, store, embedder, force);
            store.read().save(&db_path)?;

            // Also rebuild and save graph cache
            let gs = graph_store::GraphStore::build_from_directory(&pkb_root);
            let graph_path = db_path.with_extension("graph.json");
            if let Err(e) = gs.save(&graph_path) {
                eprintln!("Warning: failed to save graph cache: {e}");
            }

            println!("✓ {total} documents ({indexed} indexed, {removed} removed)");
        }

        Commands::Status => {
            let store = store.as_ref().unwrap();
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
            flat,
            sort,
        } => {
            let gs = load_graph(&pkb_root, &db_path);

            let tasks: Vec<&graph::GraphNode> = match filter {
                TaskFilter::Blocked => gs.blocked_tasks(),
                TaskFilter::All => gs.all_tasks(),
                TaskFilter::Ready => gs.ready_tasks(),
            };

            // Filter by project
            let mut tasks: Vec<&graph::GraphNode> = if let Some(ref proj) = project {
                tasks.into_iter().filter(|t| t.project.as_deref() == Some(proj)).collect()
            } else {
                tasks
            };

            // Apply --sort if specified
            if let Some(ref sort_key) = sort {
                match sort_key.as_str() {
                    "weight" => {
                        tasks.sort_by(|a, b| {
                            b.downstream_weight
                                .partial_cmp(&a.downstream_weight)
                                .unwrap_or(std::cmp::Ordering::Equal)
                                .then(a.priority.unwrap_or(2).cmp(&b.priority.unwrap_or(2)))
                        });
                    }
                    "due" => {
                        tasks.sort_by(|a, b| {
                            let a_due = a.due.as_deref().unwrap_or("9999-99-99");
                            let b_due = b.due.as_deref().unwrap_or("9999-99-99");
                            a_due.cmp(b_due)
                        });
                    }
                    "priority" => {
                        tasks.sort_by(|a, b| {
                            a.priority
                                .unwrap_or(2)
                                .cmp(&b.priority.unwrap_or(2))
                                .then(
                                    b.downstream_weight
                                        .partial_cmp(&a.downstream_weight)
                                        .unwrap_or(std::cmp::Ordering::Equal),
                                )
                        });
                    }
                    // Unknown sort key: leave ordering unchanged
                    _ => {}
                }
            }

            if tasks.is_empty() {
                println!("No {} tasks found.", filter);
                return Ok(());
            }

            if flat {
                // ── Flat table ──
                let width = term_width();
                println!();
                print_dashboard(&tasks, &filter);
                println!();
                println!(
                    "  {}{}  {:<50}  {:>6}  {:<14}{}",
                    colors::BOLD, "PRI", "TITLE", "WEIGHT", "ID", colors::RESET
                );
                println!("  {}", "\u{2500}".repeat(width.saturating_sub(4)));

                for task in &tasks {
                    let pri = task.priority.unwrap_or(2);
                    let color = pri_color(pri);
                    let weight = if task.downstream_weight > 0.0 {
                        format!("{:.1}", task.downstream_weight)
                    } else {
                        "-".to_string()
                    };
                    let exposure = if task.stakeholder_exposure { "!" } else { "" };
                    let tid = task.task_id.as_deref().unwrap_or(&task.id);
                    let age = days_since_created(task.created.as_deref())
                        .map(|d| format!("  {}", format_staleness(d)))
                        .unwrap_or_default();
                    println!(
                        "  {color}P{pri}{exposure}{} {:<50}  {:>5}  {}[{tid}]{}{age}",
                        colors::RESET,
                        task.label,
                        weight,
                        colors::DIM_GRAY,
                        colors::RESET,
                    );
                }
                println!("\n  {}{} {} tasks{}", colors::DIM, tasks.len(), filter, colors::RESET);
            } else {
                // ── Tree view (default) ──
                use std::collections::{HashMap, HashSet};
                let width = term_width();

                // Build set of visible task IDs for filtering
                let mut visible: HashSet<&str> = tasks
                    .iter()
                    .map(|t| t.id.as_str())
                    .collect();

                // Collect ancestor context nodes (projects, epics, goals)
                let context_types = ["project", "epic", "goal", "subproject"];
                let mut context_ids: HashSet<String> = HashSet::new();

                for task in &tasks {
                    let mut current_id = task.parent.as_deref();
                    while let Some(pid) = current_id {
                        if visible.contains(pid) {
                            break;
                        }
                        if context_ids.contains(pid) {
                            break;
                        }
                        if let Some(parent_node) = gs.get_node(pid) {
                            if parent_node
                                .node_type
                                .as_deref()
                                .map(|t| context_types.contains(&t))
                                .unwrap_or(false)
                            {
                                context_ids.insert(pid.to_string());
                            }
                            current_id = parent_node.parent.as_deref();
                        } else {
                            break;
                        }
                    }
                }

                for cid in &context_ids {
                    visible.insert(cid.as_str());
                }

                // Group by project
                let mut by_proj: HashMap<&str, Vec<&graph::GraphNode>> = HashMap::new();
                for task in &tasks {
                    let proj = task
                        .project
                        .as_deref()
                        .unwrap_or("_no_project");
                    by_proj.entry(proj).or_default().push(task);
                }

                let mut proj_names: Vec<&str> = by_proj.keys().copied().collect();
                proj_names.sort_by(|a, b| {
                    if *a == "_no_project" {
                        std::cmp::Ordering::Greater
                    } else if *b == "_no_project" {
                        std::cmp::Ordering::Less
                    } else {
                        a.cmp(b)
                    }
                });

                // Sort siblings — context nodes first, then tasks by priority/weight
                fn sort_siblings(nodes: &mut [&graph::GraphNode], context_ids: &HashSet<String>) {
                    nodes.sort_by(|a, b| {
                        let a_ctx = context_ids.contains(&a.id);
                        let b_ctx = context_ids.contains(&b.id);
                        match (a_ctx, b_ctx) {
                            (true, false) => std::cmp::Ordering::Less,
                            (false, true) => std::cmp::Ordering::Greater,
                            (true, true) => a.label.cmp(&b.label),
                            (false, false) => {
                                a.priority
                                    .unwrap_or(2)
                                    .cmp(&b.priority.unwrap_or(2))
                                    .then(
                                        b.downstream_weight
                                            .partial_cmp(&a.downstream_weight)
                                            .unwrap_or(std::cmp::Ordering::Equal),
                                    )
                                    .then(a.label.cmp(&b.label))
                            }
                        }
                    });
                }

                // Recursive tree renderer
                fn render_tree(
                    gs: &graph_store::GraphStore,
                    node: &graph::GraphNode,
                    visible: &HashSet<&str>,
                    context_ids: &HashSet<String>,
                    prefix: &str,
                    is_last: bool,
                    output: &mut Vec<String>,
                    width: usize,
                ) {
                    let connector = if is_last { "\u{2514}\u{2500}\u{2500} " } else { "\u{251C}\u{2500}\u{2500} " };
                    let prefix_vis = strip_ansi(prefix).len() + 4;
                    let available = width.saturating_sub(prefix_vis);

                    let is_context = context_ids.contains(&node.id);
                    let line = if is_context {
                        let task_count = count_visible_tasks(gs, &node.id, visible, context_ids);
                        format_context_line(node, task_count)
                    } else {
                        format_task_line(node, available)
                    };
                    output.push(format!("{prefix}{connector}{line}"));

                    let mut children: Vec<&graph::GraphNode> = node
                        .children
                        .iter()
                        .filter(|cid| visible.contains(cid.as_str()))
                        .filter_map(|cid| gs.get_node(cid))
                        .collect();
                    sort_siblings(&mut children, context_ids);

                    let child_prefix = if is_last {
                        format!("{prefix}    ")
                    } else {
                        format!("{prefix}\u{2502}   ")
                    };

                    let mut prev_was_context = false;
                    for (i, child) in children.iter().enumerate() {
                        let child_is_last = i == children.len() - 1;
                        let child_is_context = context_ids.contains(&child.id);

                        // Breathing room between epic groups
                        if child_is_context && prev_was_context && i > 0 {
                            output.push(format!("{child_prefix}"));
                        }

                        render_tree(gs, child, visible, context_ids, &child_prefix, child_is_last, output, width);
                        prev_was_context = child_is_context;
                    }
                }

                // ── Dashboard ──
                println!();
                print_dashboard(&tasks, &filter);

                // ── Focus picks (only for default ready view) ──
                if matches!(filter, TaskFilter::Ready) && project.is_none() {
                    let picks = select_focus_picks(&tasks, 5);
                    if !picks.is_empty() {
                        println!();
                        println!("  {}\u{2501}\u{2501} Today\u{2019}s Focus \u{2501}\u{2501}{}", colors::BOLD_WHITE, colors::RESET);
                        println!();
                        for pick in &picks {
                            println!("    {}", format_task_line(pick, width.saturating_sub(4)));
                        }
                        println!();
                        println!("  {}{}{}", colors::DIM, "\u{2500}".repeat(width.saturating_sub(4)), colors::RESET);
                    }
                }

                // ── Project trees ──
                let mut total = 0;
                println!();
                for (pi, proj_name) in proj_names.iter().enumerate() {
                    let proj_tasks = by_proj.get(proj_name).unwrap();
                    let count = proj_tasks.len();
                    total += count;

                    let proj_task_ids: HashSet<&str> = proj_tasks.iter().map(|t| t.id.as_str()).collect();

                    let proj_context: HashSet<&str> = context_ids
                        .iter()
                        .filter(|cid| {
                            gs.get_node(cid)
                                .map(|n| n.project.as_deref() == proj_tasks[0].project.as_deref())
                                .unwrap_or(false)
                        })
                        .map(|s| s.as_str())
                        .collect();

                    let proj_visible: HashSet<&str> = proj_task_ids
                        .iter()
                        .chain(proj_context.iter())
                        .copied()
                        .collect();

                    let mut roots: Vec<&graph::GraphNode> = proj_visible
                        .iter()
                        .filter_map(|id| gs.get_node(id))
                        .filter(|n| {
                            match &n.parent {
                                None => true,
                                Some(pid) => !proj_visible.contains(pid.as_str()),
                            }
                        })
                        .collect();
                    sort_siblings(&mut roots, &context_ids);

                    // Project header
                    let display_name = if *proj_name == "_no_project" {
                        "ungrouped"
                    } else {
                        proj_name
                    };
                    println!(
                        "  {}{}{} {}({} {}){}",
                        colors::BOLD_CYAN, display_name, colors::RESET,
                        colors::DIM, count, filter, colors::RESET
                    );

                    let mut lines: Vec<String> = Vec::new();
                    for (i, root) in roots.iter().enumerate() {
                        let is_last = i == roots.len() - 1;
                        render_tree(&gs, root, &proj_visible, &context_ids, "", is_last, &mut lines, width);
                    }
                    for line in &lines {
                        println!("{line}");
                    }

                    if pi < proj_names.len() - 1 {
                        println!();
                    }
                }
                println!("\n  {}{} {} tasks across {} projects{}", colors::DIM, total, filter, proj_names.len(), colors::RESET);
            }
        }

        Commands::Focus { limit } => {
            let gs = load_graph(&pkb_root, &db_path);
            let tasks = gs.ready_tasks();

            if tasks.is_empty() {
                println!("No ready tasks.");
                return Ok(());
            }

            let picks = select_focus_picks(&tasks, limit);
            let width = term_width();

            println!();
            for pick in &picks {
                println!("  {}", format_task_line(pick, width.saturating_sub(2)));
            }
            println!();
        }

        Commands::Show { id } => {
            let gs = load_graph(&pkb_root, &db_path);

            match gs.resolve(&id) {
                Some(node) => {
                    println!();
                    println!("  \x1b[1m{}\x1b[0m", node.label);
                    println!("  \x1b[2m{}\x1b[0m", abs_node_path(&node.path, &pkb_root).display());
                    println!();

                    // --- Metadata ---
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
                    if let Some(ref created) = node.created {
                        println!("  Created:  {created}");
                    }

                    // --- Local Graph Context (ASCII) ---
                    println!("\n  \x1b[1mGraph Context:\x1b[0m");
                    let graph_lines = graph_display::render_ascii_graph(&gs, &node.id);
                    for line in graph_lines {
                        println!("    {line}");
                    }

                    // --- Parent Chain ---
                    let mut parents = Vec::new();
                    let mut curr = node.parent.as_deref();
                    while let Some(pid) = curr {
                        if let Some(p) = gs.get_node(pid) {
                            parents.push(format!("{} ({})", p.label, pid));
                            curr = p.parent.as_deref();
                        } else {
                            break;
                        }
                    }
                    if !parents.is_empty() {
                        parents.reverse();
                        println!("\n  \x1b[1mParent Chain:\x1b[0m");
                        for (i, p) in parents.iter().enumerate() {
                            println!("    {} \x1b[2m{}\x1b[0m", "  ".repeat(i), p);
                        }
                    }

                    // --- Weight / Metrics ---
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

                    // --- Body ---
                    let file_path = abs_node_path(&node.path, &pkb_root);
                    if let Ok(content) = std::fs::read_to_string(&file_path) {
                        let body = if content.starts_with("---") {
                            content
                                .splitn(3, "---")
                                .nth(2)
                                .unwrap_or("")
                                .trim()
                        } else {
                            content.trim()
                        };
                        if !body.is_empty() {
                            println!("\n  \x1b[1mBody:\x1b[0m");
                            let lines: Vec<_> = body.lines().collect();
                            for line in lines.iter().take(20) {
                                println!("  {line}");
                            }
                            if lines.len() > 20 {
                                println!("  \x1b[2m... (truncated)\x1b[0m");
                            }
                        }
                    }

                    // Navigation hints
                    let node_id = node.task_id.as_deref().unwrap_or(&node.id);
                    println!();
                    println!("  \x1b[2mTip: aops context {node_id}  — show full knowledge neighbourhood\x1b[0m");
                }
                None => {
                    eprintln!("Document not found: {id}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Deps { id, tree } => {
            let gs = load_graph(&pkb_root, &db_path);

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
            let gs = load_graph(&pkb_root, &db_path);
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
            depends_on,
            assignee,
            complexity,
            body,
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
                depends_on: depends_on.unwrap_or_default(),
                assignee,
                complexity,
                body,
                ..Default::default()
            };

            match document_crud::create_task(&pkb_root, fields) {
                Ok(path) => {
                    // Extract ID from filename (e.g. "task-a1b2c3d4-some-title.md" -> "task-a1b2c3d4")
                    let id = extract_id_from_path(&path);
                    let title_display = path.file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    println!("Created \x1b[1m{id}\x1b[0m: {title_display}");
                    println!("  \x1b[2m{}\x1b[0m", path.display());

                    // Rebuild graph cache so the task is immediately usable
                    rebuild_graph_cache(&pkb_root, &db_path);
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Remember {
            title,
            doc_type,
            tags,
            status,
            priority,
            parent,
            project,
            source,
            body,
            dir,
        } => {
            let title_str = title.join(" ");
            if title_str.is_empty() {
                eprintln!("Error: title cannot be empty");
                std::process::exit(1);
            }

            let fields = document_crud::DocumentFields {
                title: title_str,
                doc_type,
                tags: tags.unwrap_or_default(),
                status,
                priority,
                parent,
                project,
                source,
                body,
                dir,
                ..Default::default()
            };

            match document_crud::create_document(&pkb_root, fields) {
                Ok(path) => {
                    let id = extract_id_from_path(&path);
                    let title_display = path.file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    println!("Created \x1b[1m{id}\x1b[0m: {title_display}");
                    println!("  \x1b[2m{}\x1b[0m", path.display());

                    rebuild_graph_cache(&pkb_root, &db_path);
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Append {
            id,
            content,
            section,
        } => {
            let content_str = content.join(" ");
            if content_str.is_empty() {
                eprintln!("Error: content cannot be empty");
                std::process::exit(1);
            }

            let gs = load_graph(&pkb_root, &db_path);

            match gs.resolve(&id) {
                Some(node) => {
                    let path = abs_node_path(&node.path, &pkb_root);
                    match document_crud::append_to_document(
                        &path,
                        &content_str,
                        section.as_deref(),
                    ) {
                        Ok(()) => {
                            println!("Appended to: {} ({})", node.label, id);
                        }
                        Err(e) => {
                            eprintln!("Error: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                None => {
                    eprintln!("Document not found: {id}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Delete { id } => {
            // Try graph resolution first, fall back to filesystem glob
            let gs = load_graph(&pkb_root, &db_path);

            let (path, label) = match gs.resolve(&id) {
                Some(node) => (abs_node_path(&node.path, &pkb_root), node.label.clone()),
                None => {
                    // Filesystem fallback: search for files starting with the ID
                    let mut found = None;
                    for subdir in &["tasks", "memories", "."] {
                        let dir = pkb_root.join(subdir);
                        if dir.is_dir() {
                            if let Ok(entries) = std::fs::read_dir(&dir) {
                                for entry in entries.flatten() {
                                    let name = entry.file_name().to_string_lossy().to_string();
                                    if name.starts_with(&id) && name.ends_with(".md") {
                                        found = Some(entry.path());
                                        break;
                                    }
                                }
                            }
                        }
                        if found.is_some() {
                            break;
                        }
                    }
                    match found {
                        Some(p) => {
                            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                            (p, name)
                        }
                        None => {
                            eprintln!("Not found: {id}");
                            std::process::exit(1);
                        }
                    }
                }
            };

            match document_crud::delete_document(&path) {
                Ok(_) => {
                    println!("Deleted: {} ({})", label, id);
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Done { id } => {
            let gs = load_graph(&pkb_root, &db_path);

            match gs.get_node(&id) {
                Some(node) => {
                    let path = abs_node_path(&node.path, &pkb_root);
                    let mut updates = std::collections::HashMap::new();
                    updates.insert(
                        "status".to_string(),
                        serde_json::Value::String("done".to_string()),
                    );

                    document_crud::update_document(&path, updates)?;
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
            let gs = load_graph(&pkb_root, &db_path);

            match gs.get_node(&id) {
                Some(node) => {
                    let path = abs_node_path(&node.path, &pkb_root);
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
                    println!("Updated: {} ({})", node.label, id);
                }
                None => {
                    eprintln!("Task not found: {id}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Context { id, hops } => {
            let gs = load_graph(&pkb_root, &db_path);

            match gs.resolve(&id) {
                Some(node) => {
                    let node_id = node.id.clone();
                    println!();
                    println!("  \x1b[1m{}\x1b[0m", node.label);
                    println!("  \x1b[2m{}\x1b[0m", abs_node_path(&node.path, &pkb_root).display());
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
                    if !node.tags.is_empty() {
                        println!("  Tags:     {}", node.tags.join(", "));
                    }

                    // Relationships
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
                            let status = gs
                                .get_node(c)
                                .and_then(|n| n.status.as_deref())
                                .unwrap_or("?");
                            println!("    {c} [{status}] {label}");
                        }
                    }
                    if let Some(ref p) = node.parent {
                        let label = gs.get_node(p).map(|n| n.label.as_str()).unwrap_or("?");
                        println!("\n  \x1b[1mParent:\x1b[0m {p} ({label})");
                    }

                    // Backlinks by type
                    let backlinks = gs.backlinks_by_type(&node_id);
                    if !backlinks.is_empty() {
                        println!("\n  \x1b[1mBacklinks:\x1b[0m");
                        let mut types: Vec<_> = backlinks.keys().collect();
                        types.sort();
                        for ntype in types {
                            let entries = &backlinks[ntype];
                            println!("    \x1b[36m{ntype}\x1b[0m ({} links)", entries.len());
                            for (src, edge_type) in entries {
                                println!(
                                    "      {} \x1b[2m[{}]\x1b[0m {}",
                                    src.id,
                                    edge_type.as_str(),
                                    src.label
                                );
                            }
                        }
                    }

                    // Ego subgraph
                    let nearby = gs.ego_subgraph(&node_id, hops);
                    if !nearby.is_empty() {
                        println!("\n  \x1b[1mNearby ({hops}-hop):\x1b[0m");
                        let mut sorted = nearby;
                        sorted.sort_by_key(|(_, d)| *d);
                        for (nid, dist) in &sorted {
                            let label = gs
                                .get_node(nid)
                                .map(|n| n.label.as_str())
                                .unwrap_or("?");
                            println!("    [{dist}] {nid} ({label})");
                        }
                    }

                    println!();
                }
                None => {
                    eprintln!("Node not found: {id}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Trace {
            from,
            to,
            max_paths,
        } => {
            let gs = load_graph(&pkb_root, &db_path);

            let from_node = match gs.resolve(&from) {
                Some(n) => n,
                None => {
                    eprintln!("Source node not found: {from}");
                    std::process::exit(1);
                }
            };
            let from_id = from_node.id.clone();

            let to_node = match gs.resolve(&to) {
                Some(n) => n,
                None => {
                    eprintln!("Target node not found: {to}");
                    std::process::exit(1);
                }
            };
            let to_id = to_node.id.clone();

            let paths = gs.all_shortest_paths(&from_id, &to_id, max_paths);

            if paths.is_empty() {
                println!("No path found between {from_id} and {to_id}");
                return Ok(());
            }

            println!();
            println!(
                "  \x1b[1m{} path(s)\x1b[0m ({} hops)",
                paths.len(),
                paths[0].len() - 1
            );
            println!();

            for (i, path) in paths.iter().enumerate() {
                println!("  Path {}:", i + 1);
                for (j, nid) in path.iter().enumerate() {
                    let label = gs
                        .get_node(nid)
                        .map(|n| n.label.as_str())
                        .unwrap_or("?");
                    if j == 0 {
                        println!("    {nid} ({label})");
                    } else {
                        println!("    \x1b[2m→\x1b[0m {nid} ({label})");
                    }
                }
                println!();
            }
        }

        Commands::Orphans { node_type, project } => {
            let gs = load_graph(&pkb_root, &db_path);
            let mut orphans = gs.orphans();

            if let Some(ref t) = node_type {
                orphans.retain(|n| {
                    n.node_type
                        .as_deref()
                        .map(|nt| nt.eq_ignore_ascii_case(t))
                        .unwrap_or(false)
                });
            }

            if let Some(ref proj) = project {
                orphans.retain(|n| n.project.as_deref() == Some(proj.as_str()));
            }

            if orphans.is_empty() {
                println!("No orphan nodes found.");
                return Ok(());
            }

            orphans.sort_by(|a, b| a.label.cmp(&b.label));

            let type_desc = node_type
                .as_ref()
                .map(|t| format!(" [{t}]"))
                .unwrap_or_default();

            println!();
            println!(
                "  \x1b[1m{} orphan nodes{type_desc}\x1b[0m (no incoming or outgoing edges)\n",
                orphans.len()
            );

            for node in &orphans {
                let type_str = node
                    .node_type
                    .as_deref()
                    .map(|t| format!(" \x1b[35m[{t}]\x1b[0m"))
                    .unwrap_or_default();
                println!(
                    "  \x1b[1m{}\x1b[0m{type_str}",
                    node.label,
                );
                println!("  \x1b[2m{}\x1b[0m\n", abs_node_path(&node.path, &pkb_root).display());
            }
        }

        Commands::Graph { format, output } => {
            let gs = load_graph(&pkb_root, &db_path);

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

        Commands::Recall { query, tags, limit } => {
            let embedder = embedder.as_ref().unwrap();
            let store = store.as_ref().unwrap();
            let query_text = query.join(" ");
            if query_text.is_empty() {
                eprintln!("Error: search query cannot be empty");
                std::process::exit(1);
            }

            let query_embedding = embedder.encode(&query_text)?;
            let results = store.read().search(&query_embedding, limit * 3, &pkb_root);

            let memory_types = ["memory", "note", "insight", "observation"];
            let mut count = 0;

            println!();
            for r in &results {
                if count >= limit {
                    break;
                }
                let is_memory = r
                    .doc_type
                    .as_deref()
                    .map(|t| memory_types.iter().any(|mt| t.eq_ignore_ascii_case(mt)))
                    .unwrap_or(false);
                if !is_memory {
                    continue;
                }

                if let Some(ref required_tags) = tags {
                    let has_all = required_tags
                        .iter()
                        .all(|rt| r.tags.iter().any(|t| t.eq_ignore_ascii_case(rt)));
                    if !has_all {
                        continue;
                    }
                }

                count += 1;
                let score_bar = score_to_bar(r.score);
                let tags_str = if r.tags.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", r.tags.join(", "))
                };

                println!(
                    "  \x1b[1;36m{}.\x1b[0m \x1b[1m{}\x1b[0m {score_bar}{tags_str}",
                    count, r.title,
                );
                println!("     \x1b[2m{}\x1b[0m", r.path.display());

                // Show full body for memories
                if let Ok(content) = std::fs::read_to_string(&r.path) {
                    let body = if content.starts_with("---") {
                        content
                            .splitn(3, "---")
                            .nth(2)
                            .unwrap_or("")
                            .trim()
                    } else {
                        content.trim()
                    };
                    if !body.is_empty() {
                        for line in body.lines().take(10) {
                            println!("     {line}");
                        }
                    }
                }
                println!();
            }

            if count == 0 {
                println!("No memories found for: {query_text}");
            }
        }

        Commands::Tags {
            search_tags,
            doc_type,
            count,
        } => {
            let store = store.as_ref().unwrap();

            match search_tags {
                None => {
                    // Show tag frequency summary
                    let all_tags = store.read().list_all_tags();
                    if all_tags.is_empty() {
                        println!("No tags found in index.");
                        return Ok(());
                    }

                    let mut sorted: Vec<_> = all_tags.into_iter().collect();
                    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

                    println!();
                    if count {
                        println!("  {} unique tags", sorted.len());
                    } else {
                        println!(
                            "  \x1b[1m{:<30} {:>6}\x1b[0m",
                            "TAG", "COUNT"
                        );
                        println!("  {}", "-".repeat(38));
                        for (tag, cnt) in sorted.iter().take(30) {
                            println!("  {:<30} {:>6}", tag, cnt);
                        }
                        if sorted.len() > 30 {
                            println!(
                                "\n  \x1b[2m...and {} more tags\x1b[0m",
                                sorted.len() - 30
                            );
                        }
                    }
                    println!();
                }
                Some(ref tags_list) => {
                    // Search for documents with these tags
                    let s = store.read();
                    let all = s.list_documents(
                        None,
                        doc_type.as_deref(),
                        None,
                        None,
                        &pkb_root,
                    );
                    let matching: Vec<_> = all
                        .into_iter()
                        .filter(|r| {
                            tags_list
                                .iter()
                                .all(|tag| r.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)))
                        })
                        .collect();

                    if matching.is_empty() {
                        println!("No documents with tags: {}", tags_list.join(", "));
                        return Ok(());
                    }

                    if count {
                        println!("{}", matching.len());
                    } else {
                        println!();
                        for r in &matching {
                            let type_str = r
                                .doc_type
                                .as_deref()
                                .map(|t| format!(" \x1b[35m[{t}]\x1b[0m"))
                                .unwrap_or_default();
                            println!(
                                "  \x1b[1m{}\x1b[0m{type_str}  [{}]",
                                r.title,
                                r.tags.join(", ")
                            );
                            println!("  \x1b[2m{}\x1b[0m\n", r.path.display());
                        }
                        println!(
                            "  {} documents with tags [{}]",
                            matching.len(),
                            tags_list.join(", ")
                        );
                    }
                }
            }
        }

        Commands::Forget { id } => {
            let gs = load_graph(&pkb_root, &db_path);

            match gs.resolve(&id) {
                Some(node) => {
                    let memory_types = ["memory", "note", "insight", "observation"];
                    let is_memory = node
                        .node_type
                        .as_deref()
                        .map(|t| memory_types.iter().any(|mt| t.eq_ignore_ascii_case(mt)))
                        .unwrap_or(false);

                    if !is_memory {
                        eprintln!(
                            "Not a memory document: {id} (type: {})",
                            node.node_type.as_deref().unwrap_or("unknown")
                        );
                        eprintln!("Use 'aops delete' for non-memory documents.");
                        std::process::exit(1);
                    }

                    let path = abs_node_path(&node.path, &pkb_root);
                    let rel_path = node.path.to_string_lossy().to_string();
                    let label = node.label.clone();

                    match document_crud::delete_document(&path) {
                        Ok(_) => {
                            println!("Forgot: {} ({})", label, id);

                            // Remove from vector store to keep index consistent
                            if let Some(ref store) = store {
                                let mut w = store.write();
                                w.remove(&rel_path);
                                let _ = w.save(&db_path);
                            }

                            // Rebuild graph cache
                            rebuild_graph_cache(&pkb_root, &db_path);
                        }
                        Err(e) => {
                            eprintln!("Error: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                None => {
                    eprintln!("Memory not found: {id}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Memories { tags, limit } => {
            let store = store.as_ref().unwrap();
            let memory_types = ["memory", "note", "insight", "observation"];

            let s = store.read();
            let all = s.list_documents(None, None, None, None, &pkb_root);
            let mut memories: Vec<_> = all
                .into_iter()
                .filter(|r| {
                    r.doc_type
                        .as_deref()
                        .map(|t| memory_types.iter().any(|mt| t.eq_ignore_ascii_case(mt)))
                        .unwrap_or(false)
                })
                .collect();

            if let Some(ref required_tags) = tags {
                memories.retain(|r| {
                    required_tags
                        .iter()
                        .all(|rt| r.tags.iter().any(|t| t.eq_ignore_ascii_case(rt)))
                });
            }

            memories.truncate(limit);

            if memories.is_empty() {
                println!("No memories found.");
                return Ok(());
            }

            println!();
            for r in &memories {
                let type_str = r
                    .doc_type
                    .as_deref()
                    .map(|t| format!(" \x1b[35m[{t}]\x1b[0m"))
                    .unwrap_or_default();
                let tags_str = if r.tags.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", r.tags.join(", "))
                };
                println!(
                    "  \x1b[1m{}\x1b[0m{type_str}{tags_str}",
                    r.title,
                );
                println!("  \x1b[2m{}\x1b[0m\n", r.path.display());
            }
            println!("  {} memories", memories.len());
        }

        Commands::Blocks { id, tree } => {
            let gs = load_graph(&pkb_root, &db_path);

            if gs.get_node(&id).is_none() {
                eprintln!("Task not found: {id}");
                std::process::exit(1);
            }

            let blocks = gs.blocks_tree(&id);
            if blocks.is_empty() {
                println!("Completing {id} would not unblock any tasks.");
                return Ok(());
            }

            println!();
            for (blocked_id, depth) in &blocks {
                let indent = if tree {
                    "  ".repeat(*depth)
                } else {
                    "  ".to_string()
                };
                let label = gs
                    .get_node(blocked_id)
                    .map(|n| n.label.as_str())
                    .unwrap_or("?");
                let status = gs
                    .get_node(blocked_id)
                    .and_then(|n| n.status.as_deref())
                    .unwrap_or("?");
                println!("{indent}{blocked_id} [{status}] {label}");
            }
            println!();
        }

        Commands::Tui => {
            tui::run(&pkb_root, &db_path)?;
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

    // Use relative paths for store keys (portable across machines)
    let existing_paths: std::collections::HashSet<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(pkb_root)
                .unwrap_or(p)
                .to_string_lossy()
                .to_string()
        })
        .collect();

    let removed = {
        let mut store = store.write();
        store.remove_deleted(&existing_paths)
    };

    // Figure out which files need updating
    let to_process: Vec<_> = files
        .iter()
        .filter(|file_path| {
            let path_str = file_path
                .strip_prefix(pkb_root)
                .unwrap_or(file_path)
                .to_string_lossy()
                .to_string();
            // Compute content hash for change detection
            let content_hash = std::fs::read(file_path)
                .ok()
                .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
                .unwrap_or_default();
            force_all || {
                let store = store.read();
                store.needs_update(&path_str, &content_hash)
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

    // Parse all files in parallel with rayon (relative paths for portable storage)
    pb.set_message("parsing...");
    let parsed: Vec<_> = to_process
        .par_iter()
        .filter_map(|path| {
            pkb::parse_file_relative(path, pkb_root).map(|doc| {
                let text = doc.embedding_text();
                let chunks = embeddings::chunk_text(&text, &embeddings::ChunkConfig::default());
                (doc, chunks)
            })
        })
        .collect();

    // Batch embed and store — batches of 200 docs with progressive saves.
    // 200 docs × ~3 chunks = ~600 chunks / 32 per sub-batch = ~19 sub-batches,
    // enough to saturate all ONNX sessions across available cores.
    let mut indexed = 0;

    for batch in parsed.chunks(200) {
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

/// Reconstruct an absolute path from a (possibly relative) node path.
fn abs_node_path(path: &std::path::Path, pkb_root: &std::path::Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        pkb_root.join(path)
    }
}

/// Extract the document ID from a created file path.
/// E.g. "task-a1b2c3d4-some-title.md" -> "task-a1b2c3d4"
fn extract_id_from_path(path: &std::path::Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    // ID is prefix-8hexchars, match the pattern
    let re = regex::Regex::new(r"^([a-z]+-[0-9a-f]{8})").unwrap();
    re.find(&stem)
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| stem.to_string())
}

/// Rebuild and save the graph cache so newly created tasks are immediately visible.
fn rebuild_graph_cache(pkb_root: &std::path::Path, db_path: &std::path::Path) {
    let gs = graph_store::GraphStore::build_from_directory(pkb_root);
    let graph_path = db_path.with_extension("graph.json");
    if let Err(e) = gs.save(&graph_path) {
        eprintln!("Warning: failed to save graph cache: {e}");
    }
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

// ── Task tree display helpers ──────────────────────────────────────

mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const DIM: &str = "\x1b[2m";
    pub const P0: &str = "\x1b[1;31m";      // bold red
    pub const P1: &str = "\x1b[1;33m";      // bold yellow
    pub const P2: &str = "\x1b[36m";        // cyan
    pub const P3: &str = "\x1b[2m";         // dim
    pub const RED: &str = "\x1b[31m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const CYAN: &str = "\x1b[36m";
    pub const BOLD_CYAN: &str = "\x1b[1;36m";
    pub const DIM_GRAY: &str = "\x1b[2;37m";
    pub const BOLD_WHITE: &str = "\x1b[1;37m";
}

fn term_width() -> usize {
    terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(100)
}

fn strip_ansi(s: &str) -> String {
    let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    re.replace_all(s, "").to_string()
}

fn pri_color(pri: i32) -> &'static str {
    match pri {
        0 => colors::P0,
        1 => colors::P1,
        2 => colors::P2,
        _ => colors::P3,
    }
}

fn days_since_created(created: Option<&str>) -> Option<i64> {
    let created = created?;
    if created.len() < 10 {
        return None;
    }
    let created_dt = chrono::NaiveDate::parse_from_str(&created[..10], "%Y-%m-%d").ok()?;
    let today = chrono::Utc::now().date_naive();
    Some((today - created_dt).num_days())
}

fn format_staleness(days: i64) -> String {
    let color = if days > 30 {
        colors::RED
    } else if days >= 14 {
        colors::YELLOW
    } else {
        colors::DIM
    };
    format!("{color}{days}d{}", colors::RESET)
}

fn format_due(due: &str) -> String {
    let today = chrono::Utc::now().date_naive();
    let len = std::cmp::min(10, due.len());
    if let Ok(due_date) = chrono::NaiveDate::parse_from_str(&due[..len], "%Y-%m-%d") {
        let days_until = (due_date - today).num_days();
        let color = if days_until < 0 {
            colors::RED
        } else if days_until <= 7 {
            colors::YELLOW
        } else {
            colors::DIM
        };
        format!("{color}due:{due_date}{}", colors::RESET)
    } else {
        format!("{}due:{due}{}", colors::DIM, colors::RESET)
    }
}

fn format_complexity(complexity: &str) -> String {
    format!("{}[{complexity}]{}", colors::DIM, colors::RESET)
}

fn select_focus_picks<'a>(tasks: &[&'a graph::GraphNode], max: usize) -> Vec<&'a graph::GraphNode> {
    let today = chrono::Utc::now().date_naive();

    let mut scored: Vec<(&graph::GraphNode, i64)> = tasks
        .iter()
        .map(|&t| {
            let pri = t.priority.unwrap_or(2);
            let mut score: i64 = match pri {
                0 => 10000,
                1 => 5000,
                _ => 0,
            };

            if let Some(ref due) = t.due {
                let len = std::cmp::min(10, due.len());
                if let Ok(due_date) = chrono::NaiveDate::parse_from_str(&due[..len], "%Y-%m-%d") {
                    let days_until = (due_date - today).num_days();
                    if days_until < 0 {
                        score += 8000;
                    } else if days_until <= 7 {
                        score += 3000 + (7 - days_until) * 100;
                    } else if days_until <= 30 {
                        score += 1000;
                    }
                }
            }

            if pri >= 2 {
                if let Some(days) = days_since_created(t.created.as_deref()) {
                    score += std::cmp::min(days, 200);
                }
            }

            score += (t.downstream_weight * 10.0) as i64;

            (t, score)
        })
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.into_iter().take(max).map(|(t, _)| t).collect()
}

fn format_task_line(task: &graph::GraphNode, width: usize) -> String {
    let pri = task.priority.unwrap_or(2);
    let color = pri_color(pri);
    let exposure = if task.stakeholder_exposure { "!" } else { " " };

    // Left: priority + label
    let left = format!("{color}P{pri}{exposure}{} {}", colors::RESET, task.label);

    // Right: metadata pieces
    let mut right_parts: Vec<String> = Vec::new();

    if task.downstream_weight > 0.0 {
        right_parts.push(format!("{}wt:{:.1}{}", colors::DIM, task.downstream_weight, colors::RESET));
    }
    if let Some(ref cx) = task.complexity {
        right_parts.push(format_complexity(cx));
    }
    if let Some(ref due) = task.due {
        right_parts.push(format_due(due));
    }
    if let Some(days) = days_since_created(task.created.as_deref()) {
        right_parts.push(format_staleness(days));
    }
    let tid = task.task_id.as_deref().unwrap_or(&task.id);
    right_parts.push(format!("{}[{tid}]{}", colors::DIM_GRAY, colors::RESET));

    let right = right_parts.join("  ");

    let left_vis = strip_ansi(&left).len();
    let right_vis = strip_ansi(&right).len();
    let padding = width.saturating_sub(left_vis).saturating_sub(right_vis).max(2);

    format!("{left}{:>pad$}{right}", "", pad = padding)
}

fn format_context_line(
    node: &graph::GraphNode,
    child_task_count: usize,
) -> String {
    let ntype = node.node_type.as_deref().unwrap_or("group");
    let tid = node.task_id.as_deref().unwrap_or(&node.id);

    let block_color = match ntype {
        "epic" => colors::CYAN,
        "goal" => colors::YELLOW,
        "project" => colors::BOLD_CYAN,
        _ => colors::DIM,
    };

    let count_str = if child_task_count > 0 {
        format!(" {}({child_task_count}){}", colors::DIM, colors::RESET)
    } else {
        String::new()
    };

    format!(
        "{block_color}\u{258E}{} {}{}{}{count_str}  {}[{tid}]{}",
        colors::RESET,
        colors::BOLD,
        node.label,
        colors::RESET,
        colors::DIM_GRAY,
        colors::RESET,
    )
}

fn count_visible_tasks(
    gs: &graph_store::GraphStore,
    node_id: &str,
    visible: &std::collections::HashSet<&str>,
    context_ids: &std::collections::HashSet<String>,
) -> usize {
    let mut count = 0;
    if let Some(node) = gs.get_node(node_id) {
        for cid in &node.children {
            if !visible.contains(cid.as_str()) {
                continue;
            }
            if context_ids.contains(cid) {
                count += count_visible_tasks(gs, cid, visible, context_ids);
            } else {
                count += 1;
            }
        }
    }
    count
}

fn print_dashboard(tasks: &[&graph::GraphNode], filter: &TaskFilter) {
    let total = tasks.len();
    let urgent = tasks.iter().filter(|t| t.priority.unwrap_or(2) <= 1).count();
    let with_due = tasks.iter().filter(|t| t.due.is_some()).count();
    let overdue_count = {
        let today = chrono::Utc::now().date_naive();
        tasks.iter().filter(|t| {
            t.due.as_deref().and_then(|d| {
                let len = std::cmp::min(10, d.len());
                chrono::NaiveDate::parse_from_str(&d[..len], "%Y-%m-%d").ok()
            }).map(|d| d < today).unwrap_or(false)
        }).count()
    };

    let oldest_days = tasks
        .iter()
        .filter_map(|t| days_since_created(t.created.as_deref()))
        .max()
        .unwrap_or(0);

    let mut parts: Vec<String> = vec![
        format!("{}{} {filter}{}", colors::BOLD, total, colors::RESET),
    ];
    if urgent > 0 {
        parts.push(format!("{}{}  urgent{}", colors::RED, urgent, colors::RESET));
    }
    if overdue_count > 0 {
        parts.push(format!("{}{} overdue{}", colors::RED, overdue_count, colors::RESET));
    }
    if with_due > 0 {
        parts.push(format!("{with_due} with deadlines"));
    }
    if oldest_days > 0 {
        parts.push(format!("oldest: {oldest_days}d"));
    }

    println!("  {}", parts.join(&format!(" {}│{} ", colors::DIM, colors::RESET)));
}

