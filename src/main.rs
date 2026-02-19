//! Fast-indexer: Knowledge graph builder for markdown PKB files.
//!
//! Thin wrapper over shared graph modules. Scans markdown files,
//! extracts frontmatter metadata, and outputs knowledge graphs
//! in JSON, GraphML, DOT, or MCP-index format.

mod graph;
mod graph_store;
mod metrics;
mod pkb;
mod task_index;

use anyhow::Result;
use clap::Parser;
use std::path::Path;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Root directory to scan
    #[arg(default_value = ".")]
    root: String,

    /// Output file path (extension auto-added based on format)
    #[arg(short, long, default_value = "graph")]
    output: String,

    /// Output format: json, graphml, dot, mcp-index, all (default: all)
    #[arg(short, long, default_value = "all")]
    format: String,

    /// Filter by frontmatter type (e.g., task,project,goal)
    #[arg(short = 't', long, value_delimiter = ',')]
    filter_type: Option<Vec<String>>,

    /// Filter by status (e.g., active,in_progress)
    #[arg(short = 's', long, value_delimiter = ',')]
    status: Option<Vec<String>>,

    /// Filter by priority (e.g., 0,1)
    #[arg(short = 'p', long, value_delimiter = ',')]
    priority: Option<Vec<i32>>,

    /// Suppress informational output
    #[arg(short, long)]
    quiet: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let root = Path::new(&args.root).canonicalize()?;

    if !args.quiet {
        println!("Scanning directory: {:?}", root);
    }

    // Build graph from directory
    let gs = graph_store::GraphStore::build_from_directory(&root);

    if !args.quiet {
        println!(
            "Graph: {} nodes, {} edges",
            gs.node_count(),
            gs.edge_count()
        );
    }

    // Handle MCP index format
    if args.format.to_lowercase() == "mcp-index" {
        let output_base = args
            .output
            .trim_end_matches(".json")
            .trim_end_matches(".graphml")
            .trim_end_matches(".dot");
        let path = format!("{}.json", output_base);
        let index = task_index::build_mcp_index(&gs, &root);
        let json = serde_json::to_string_pretty(&index)?;
        std::fs::write(&path, json)?;
        if !args.quiet {
            println!(
                "MCP index: {} tasks, {} ready, {} blocked -> {}",
                index.tasks.len(),
                index.ready.len(),
                index.blocked.len(),
                path,
            );
        }
        return Ok(());
    }

    // Output graph formats
    let output_base = args
        .output
        .trim_end_matches(".json")
        .trim_end_matches(".graphml")
        .trim_end_matches(".dot");

    let formats: Vec<&str> = match args.format.to_lowercase().as_str() {
        "json" => vec!["json"],
        "graphml" => vec!["graphml"],
        "dot" => vec!["dot"],
        _ => vec!["json", "graphml", "dot"],
    };

    for fmt in &formats {
        match *fmt {
            "graphml" => {
                let path = format!("{}.graphml", output_base);
                std::fs::write(&path, gs.output_graphml())?;
                if !args.quiet {
                    println!("  Saved {}", path);
                }
            }
            "dot" => {
                let path = format!("{}.dot", output_base);
                std::fs::write(&path, gs.output_dot())?;
                if !args.quiet {
                    println!("  Saved {}", path);
                }
            }
            _ => {
                let path = format!("{}.json", output_base);
                std::fs::write(&path, gs.output_json()?)?;
                if !args.quiet {
                    println!("  Saved {}", path);
                }
            }
        }
    }

    if !args.quiet {
        println!(
            "Done: {} nodes, {} edges ({} format{})",
            gs.node_count(),
            gs.edge_count(),
            formats.len(),
            if formats.len() > 1 { "s" } else { "" }
        );
    }

    Ok(())
}
