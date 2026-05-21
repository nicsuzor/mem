//! Benchmark harness for the expensive MCP write handlers.
//!
//! Builds a synthetic on-disk PKB of configurable size, then runs N timed
//! iterations of each handler and prints per-call percentiles (mean, p50,
//! p95, p99, max). Tempdir is cleaned up at exit.
//!
//! Targets the handlers most-affected by the hot-path collapse:
//! `append`, `update_task` (metadata-only), `complete_task`, `release_task`
//! (single + recursive), and `create_task`.
//!
//! Usage:
//!   cargo run --release --example bench_handlers
//!   PKB_BENCH_N_TASKS=5000 PKB_BENCH_ITERS=100 cargo run --release --example bench_handlers
//!   PKB_BENCH_DUMMY_EMBEDDER=1 cargo run --release --example bench_handlers   # default
//!   RUST_LOG=perf::graph_rebuild=debug,perf::vector=debug \
//!     cargo run --release --example bench_handlers
//!
//! Env vars:
//!   PKB_BENCH_N_TASKS         synthetic PKB size (default 1000; comma-separated for sweep, e.g. "100,1000,5000")
//!   PKB_BENCH_ITERS           iterations per handler (default 100)
//!   PKB_BENCH_DUMMY_EMBEDDER  use dummy embedder (default 1 — keeps ONNX off the timing path)
//!
//! The bench is single-threaded — it measures per-call cost, not the
//! contention story. The global-mutex elimination win shows up only under
//! parallel workload; for that, run polecat against the new binary instead.

use mem::embeddings::Embedder;
use mem::graph_store::GraphStore;
use mem::mcp_server::PkbSearchServer;
use mem::vectordb::VectorStore;
use parking_lot::RwLock;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

// ─────────────────────────────────────────────────────────────────────────────
// Synthetic PKB
// ─────────────────────────────────────────────────────────────────────────────

struct Bench {
    _tmp: TempDir,
    server: Arc<PkbSearchServer>,
    n_tasks: usize,
}

impl Bench {
    fn new(n_tasks: usize, use_dummy_embedder: bool) -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let pkb_root = tmp.path().to_path_buf();
        let tasks_dir = pkb_root.join("tasks");
        let projects_dir = pkb_root.join("projects");
        fs::create_dir_all(&tasks_dir).unwrap();
        fs::create_dir_all(&projects_dir).unwrap();

        // One project all tasks belong to.
        fs::write(
            projects_dir.join("proj-bench.md"),
            "---\nid: proj-bench\ntype: project\ntitle: Bench Project\nstatus: active\n---\n\nBench.\n",
        )
        .unwrap();

        // Tasks: chain-of-deps within groups of 5, first-of-group parents to project.
        for i in 0..n_tasks {
            let id = format!("task-bench-{i:05}");
            let group = i / 5;
            let mut body = String::new();
            body.push_str("---\n");
            body.push_str(&format!("id: {id}\n"));
            body.push_str("type: task\n");
            body.push_str(&format!("title: \"Bench task {i}\"\n"));
            body.push_str("status: active\n");
            body.push_str("priority: 2\n");
            body.push_str("project: proj-bench\n");
            if i % 5 == 0 {
                body.push_str("parent: proj-bench\n");
            } else {
                body.push_str(&format!("parent: task-bench-{:05}\n", group * 5));
                let prev = i - 1;
                body.push_str(&format!("depends_on:\n  - task-bench-{prev:05}\n"));
            }
            body.push_str("---\n\nSynthetic body content for benchmarking.\n");
            fs::write(tasks_dir.join(format!("{id}.md")), body).unwrap();
        }

        let files = mem::pkb::scan_directory_all(&pkb_root);
        let docs: Vec<_> = files
            .iter()
            .filter_map(|p| mem::pkb::parse_file_relative(p, &pkb_root))
            .collect();

        let t_build = Instant::now();
        let graph = GraphStore::build(&docs, &pkb_root);
        let build_ms = t_build.elapsed().as_secs_f64() * 1000.0;
        println!(
            "  built graph: {} nodes, {} edges in {:.1}ms",
            graph.node_count(),
            graph.edge_count(),
            build_ms
        );

        let dim = if use_dummy_embedder { 3 } else { 1024 };
        let store = VectorStore::new(dim);
        let embedder = if use_dummy_embedder {
            Embedder::new_dummy()
        } else {
            Embedder::new().expect("init real embedder")
        };

        let server = PkbSearchServer::new(
            Arc::new(RwLock::new(store)),
            Arc::new(embedder),
            pkb_root.clone(),
            pkb_root.join("db.bin"),
            Arc::new(RwLock::new(graph)),
        );

        Bench {
            _tmp: tmp,
            server: Arc::new(server),
            n_tasks,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Timing
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Stats {
    samples: Vec<Duration>,
    errors: usize,
}

impl Stats {
    fn record(&mut self, d: Duration, ok: bool) {
        self.samples.push(d);
        if !ok {
            self.errors += 1;
        }
    }

    fn print(&self, label: &str) {
        if self.samples.is_empty() {
            println!("  {label:28} (no samples)");
            return;
        }
        let mut s = self.samples.clone();
        s.sort();
        let n = s.len();
        let total: Duration = s.iter().sum();
        let mean = total / n as u32;
        let p = |q: f64| s[((n as f64 - 1.0) * q).round() as usize];
        let errs = if self.errors == 0 {
            String::new()
        } else {
            format!("  errs={}", self.errors)
        };
        println!(
            "  {label:28} n={:>4}  mean={:>9}  p50={:>9}  p95={:>9}  p99={:>9}  max={:>9}{errs}",
            n,
            fmt_dur(mean),
            fmt_dur(p(0.50)),
            fmt_dur(p(0.95)),
            fmt_dur(p(0.99)),
            fmt_dur(*s.last().unwrap()),
        );
    }
}

fn fmt_dur(d: Duration) -> String {
    let us = d.as_micros();
    if us < 1_000 {
        format!("{us}µs")
    } else if us < 1_000_000 {
        format!("{:.2}ms", us as f64 / 1000.0)
    } else {
        format!("{:.2}s", us as f64 / 1_000_000.0)
    }
}

fn time_handler(label: &str, iters: usize, mut f: impl FnMut(usize) -> bool) {
    // Warm-up
    if iters > 0 {
        f(0);
    }
    let mut stats = Stats::default();
    for i in 0..iters {
        let t = Instant::now();
        let ok = f(i);
        stats.record(t.elapsed(), ok);
    }
    stats.print(label);
}

// ─────────────────────────────────────────────────────────────────────────────
// Suite
// ─────────────────────────────────────────────────────────────────────────────

fn run_suite(n_tasks: usize, iters: usize, use_dummy_embedder: bool) {
    println!("\n=== n_tasks={n_tasks}  iters={iters}  dummy_embedder={use_dummy_embedder} ===");
    let bench = Bench::new(n_tasks, use_dummy_embedder);
    let server = bench.server.clone();
    let n = bench.n_tasks;

    // append — body changes each call; touches a different task per iter so
    // we exercise distinct files (not the same file warmed).
    time_handler("append", iters, |i| {
        let id = format!("task-bench-{:05}", i % n);
        server
            .bench_append_to_document(&json!({
                "id": id,
                "content": format!("bench append {i}"),
            }))
            .is_ok()
    });

    // update_task (metadata-only) — priority bump, body unchanged.
    time_handler("update_task (metadata)", iters, |i| {
        let id = format!("task-bench-{:05}", i % n);
        server
            .bench_update_task(&json!({
                "id": id,
                "priority": (i % 3) as i32 + 1,
            }))
            .is_ok()
    });

    // complete_task — different task each iter (capped at n).
    let complete_iters = iters.min(n);
    time_handler("complete_task", complete_iters, |i| {
        let id = format!("task-bench-{:05}", i);
        server
            .bench_complete_task(&json!({
                "id": id,
                "completion_evidence": "bench evidence",
            }))
            .is_ok()
    });

    // release_task (single) — different task, status=merge_ready.
    // Use upper-half range to avoid collision with complete_task.
    let release_iters = iters.min(n / 2);
    time_handler("release_task (single)", release_iters, |i| {
        let id = format!("task-bench-{:05}", n / 2 + i);
        server
            .bench_release_task(&json!({
                "id": id,
                "status": "merge_ready",
                "summary": "bench release",
            }))
            .is_ok()
    });

    // create_task — new task each iter. Requires `parent`; use the project
    // root so each new task lands as a child of proj-bench.
    time_handler("create_task", iters, |i| {
        server
            .bench_create_task(&json!({
                "title": format!("Bench created task {i}"),
                "project": "proj-bench",
                "parent": "proj-bench",
            }))
            .is_ok()
    });

    drop(bench);
}

fn run_recursive_release_amplification(n_tasks: usize, n_children: usize) {
    println!("\n=== release_task --recursive (root + {n_children} descendants, base n_tasks={n_tasks}) ===");
    let bench = Bench::new(n_tasks, true);

    // Re-parent n_children "first-of-group" tasks onto a single root so the
    // recursive close has a real cascade to do. (Re-parenting first-of-group
    // tasks keeps the dep chains intact within each group.)
    let root = "task-bench-00000";
    let mut reparented = 0;
    for i in 0..n_children {
        // Skip the root itself and stay within group-leaders (index % 5 == 0).
        let idx = (i + 1) * 5;
        if idx >= n_tasks {
            break;
        }
        let child = format!("task-bench-{:05}", idx);
        if bench
            .server
            .bench_update_task(&json!({"id": child, "parent": root}))
            .is_ok()
        {
            reparented += 1;
        }
    }

    let t = Instant::now();
    let ok = bench
        .server
        .bench_release_task(&json!({
            "id": root,
            "status": "done",
            "summary": "bench recursive release",
            "recursive": true,
        }))
        .is_ok();
    let elapsed = t.elapsed();
    println!(
        "  reparented={reparented}  elapsed={}  ok={ok}",
        fmt_dur(elapsed)
    );
    drop(bench);
}

// ─────────────────────────────────────────────────────────────────────────────
// main
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(true)
        .init();

    let sizes: Vec<usize> = std::env::var("PKB_BENCH_N_TASKS")
        .ok()
        .map(|s| {
            s.split(',')
                .filter_map(|p| p.trim().parse().ok())
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![1_000]);
    let iters: usize = std::env::var("PKB_BENCH_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);
    let use_dummy_embedder = std::env::var("PKB_BENCH_DUMMY_EMBEDDER")
        .map(|v| v != "0")
        .unwrap_or(true);

    for &n in &sizes {
        run_suite(n, iters, use_dummy_embedder);
    }

    // Recursive-release amplification: tests the #3 batch fix. Use the
    // largest size in the sweep so the cascade is meaningful.
    let big = *sizes.iter().max().unwrap_or(&1_000);
    let n_children = std::cmp::min(100, big / 5 - 1);
    if n_children > 5 {
        run_recursive_release_amplification(big, n_children);
    }

    // Give the background workers a moment to drain so the tempdir cleanup
    // happens after they're done. The bench harness doesn't wait on them
    // for the timing numbers — that's the whole point of #2 — but it should
    // wait here for orderly shutdown.
    let _ = std::env::var("PKB_BENCH_DRAIN_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok());
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    println!("\n(done)");
    let _ = PathBuf::new();
    Ok(())
}
