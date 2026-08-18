use clap::{Parser, Subcommand};
use mem::graph_store::GraphStore;
use mem::pkb;
use std::path::PathBuf;
use anyhow::Result;

#[derive(Parser)]
#[command(name = "pkb-excalidraw")]
#[command(about = "Excalidraw integration for PKB", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: ExcalidrawCommands,
}

#[derive(Subcommand, Debug)]
pub enum ExcalidrawCommands {
    /// Export graph or ego-network to an Excalidraw JSON canvas
    Export {
        /// Destination file path for Excalidraw JSON canvas
        output_path: PathBuf,
        /// Focus node ID for ego network layout (omit to export entire graph)
        #[arg(short = 'f', long)]
        focus: Option<String>,
        /// Path to the PKB root directory
        #[arg(long, default_value = "/Users/suzor/brain")]
        pkb_root: PathBuf,
        /// Number of hops for ego network layout (default: 2)
        #[arg(short = 'H', long, default_value_t = 2)]
        hops: usize,
    },
    /// Diff two Excalidraw canvases
    Diff {
        canvas_path: PathBuf,
        #[arg(long)]
        base: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = "/Users/suzor/brain")]
        pkb_root: PathBuf,
    },
    /// Sync changes from an Excalidraw canvas back to PKB markdown files
    Sync {
        canvas_path: PathBuf,
        #[arg(long)]
        base: Option<PathBuf>,
        #[arg(long)]
        pkb_root: PathBuf,
        #[arg(long)]
        sync_edge_removals: bool,
        #[arg(long)]
        dry_run: bool,
    },
}

fn load_graph(pkb_root: &std::path::Path) -> GraphStore {
    use rayon::prelude::*;
    let files = mem::pkb::scan_directory(pkb_root);
    let docs: Vec<mem::pkb::PkbDocument> = files
        .par_iter()
        .filter_map(|p| mem::pkb::parse_file_relative(p, pkb_root))
        .collect();
    GraphStore::build(&docs, pkb_root)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        ExcalidrawCommands::Export {
            output_path,
            focus,
            pkb_root,
            hops,
        } => {
            let gs = load_graph(&pkb_root);
            let (content, n_nodes, n_edges) = gs.output_excalidraw(focus.as_deref(), hops)?;
            std::fs::write(&output_path, content)?;
            println!(
                "Exported Excalidraw canvas ({} nodes, {} edges) -> {}",
                n_nodes,
                n_edges,
                output_path.display()
            );
        }
        ExcalidrawCommands::Diff {
            canvas_path,
            base,
            json,
            pkb_root,
        } => {
            let gs = load_graph(&pkb_root);
            let canvas_str = std::fs::read_to_string(&canvas_path)?;
            let canvas = mem::excalidraw::parse_canvas(&canvas_str)?;
            let base_snapshot = if let Some(b) = base {
                let base_str = std::fs::read_to_string(&b)?;
                Some(mem::excalidraw::parse_base_snapshot(&base_str)?)
            } else {
                None
            };
            let diff = mem::excalidraw::diff_canvas(base_snapshot.as_ref(), &gs, &canvas)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&diff)?);
            } else {
                // simple print for diff
                println!("Canvas diff generated.");
            }
        }
        ExcalidrawCommands::Sync {
            canvas_path,
            base,
            pkb_root,
            sync_edge_removals,
            dry_run,
        } => {
            let mut gs = load_graph(&pkb_root);
            let canvas_str = std::fs::read_to_string(&canvas_path)?;
            let canvas = mem::excalidraw::parse_canvas(&canvas_str)?;
            let base_snapshot = if let Some(b) = base {
                let base_str = std::fs::read_to_string(&b)?;
                Some(mem::excalidraw::parse_base_snapshot(&base_str)?)
            } else {
                None
            };
            let diff = mem::excalidraw::diff_canvas(base_snapshot.as_ref(), &gs, &canvas)?;
            mem::excalidraw::sync_canvas(&pkb_root, &mut gs, &diff, sync_edge_removals)?;
            println!("Canvas synced back to PKB.");
        }
    }
    Ok(())
}
