//! Benchmark harness for `update_task` write-path latency.
//!
//! Refs: task-a4dcc039 — investigates which phase dominates after PR #230's
//! async-coalesced save_store. Loads the live PKB, runs N=20 frontmatter-only
//! updates against a single task, and prints per-phase percentiles.
//!
//! Usage:
//!   AOPS_PKB_ROOT=/Users/suzor/brain RUST_LOG=perf::update_task=debug,perf::graph_rebuild=debug,perf::vector=debug \
//!     cargo run --release --example bench_update_task
//!
//! With dummy embedder (skips model load — fastest path, isolates non-embed cost):
//!   PKB_BENCH_DUMMY_EMBEDDER=1 AOPS_PKB_ROOT=/Users/suzor/brain cargo run --release --example bench_update_task

use mem::embeddings::Embedder;
use mem::graph_store::GraphStore;
use mem::vectordb::VectorStore;
use parking_lot::RwLock;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn print_stats(name: &str, mut samples: Vec<f64>) {
    if samples.is_empty() {
        println!("  {name}: (no samples)");
        return;
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    println!(
        "  {:30}  n={:3}  mean={:7.1}ms  p50={:7.1}ms  p95={:7.1}ms  max={:7.1}ms",
        name,
        samples.len(),
        mean,
        percentile(&samples, 50.0),
        percentile(&samples, 95.0),
        percentile(&samples, 100.0),
    );
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(true)
        .init();

    let pkb_root = std::env::var("AOPS_PKB_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir().unwrap().join("brain")
        });
    if !pkb_root.exists() {
        anyhow::bail!("PKB root not found: {}", pkb_root.display());
    }
    println!("PKB root: {}", pkb_root.display());

    // Match cli.rs default: pkb_root/pkb_vectors.bin
    let db_path = std::env::var("PKB_BENCH_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| pkb_root.join("pkb_vectors.bin"));

    // Load embedder (dummy by default for isolating non-embed costs)
    let use_dummy = std::env::var("PKB_BENCH_DUMMY_EMBEDDER").is_ok();
    let embedder = if use_dummy {
        println!("Embedder: DUMMY (frontmatter-only updates skip embedding via body-hash anyway)");
        Arc::new(Embedder::new_dummy())
    } else {
        println!("Embedder: real (BGE-M3)");
        Arc::new(Embedder::new()?)
    };

    // Load vector store (or empty)
    let store = if db_path.exists() {
        let dim = 1024;
        match VectorStore::load_or_create(&db_path, dim) {
            Ok(s) => {
                println!("Vector store: {} docs", s.len());
                s
            }
            Err(e) => {
                println!("Vector store load failed: {e}; using empty");
                VectorStore::new(dim)
            }
        }
    } else {
        println!("No vector store at {}; using empty", db_path.display());
        VectorStore::new(1024)
    };
    let store = Arc::new(RwLock::new(store));

    // Build initial graph
    let t0 = Instant::now();
    let files = mem::pkb::scan_directory_all(&pkb_root);
    let docs: Vec<mem::pkb::PkbDocument> = files
        .iter()
        .filter_map(|p| mem::pkb::parse_file_relative(p, &pkb_root))
        .collect();
    let graph = GraphStore::build(&docs, &pkb_root);
    println!(
        "Graph: {} nodes, {} edges (built in {:.1}s)",
        graph.node_count(),
        graph.edge_count(),
        t0.elapsed().as_secs_f64()
    );
    let graph = Arc::new(RwLock::new(graph));

    // Pick a task to update
    let target_id = std::env::var("PKB_BENCH_TARGET_TASK")
        .unwrap_or_else(|_| "task-a4dcc039".to_string());

    let server = Arc::new(mem::mcp_server::PkbSearchServer::new(
        store.clone(),
        embedder.clone(),
        pkb_root.clone(),
        db_path.clone(),
        graph.clone(),
    ));

    // Warm-up: ensure the target task exists by attempting a get_task style resolve.
    {
        let g = graph.read();
        match g.resolve(&target_id) {
            Some(node) => println!("Target: {} ({})", target_id, node.path.display()),
            None => anyhow::bail!("target task not found in graph: {target_id}"),
        }
    }

    let n = std::env::var("PKB_BENCH_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20usize);
    let warmup = 2usize;
    println!("\nRunning {warmup} warmup + {n} measured update_task calls...");

    let mut totals = Vec::with_capacity(n);

    for i in 0..(n + warmup) {
        // Toggle between two priority values to avoid no-op short-circuit
        let prio = if i % 2 == 0 { 2 } else { 3 };
        let args = json!({
            "id": target_id,
            "priority": prio,
        });

        let t = Instant::now();
        // Call via the Tool dispatch surface so we hit the real handler
        let res = server.bench_update_task(&args);
        let elapsed = t.elapsed().as_secs_f64() * 1000.0;
        if let Err(e) = res {
            eprintln!("call {i} failed: {:?}", e);
            continue;
        }
        if i >= warmup {
            totals.push(elapsed);
        }
    }

    println!("\n=== Results (n={}, frontmatter-only update) ===", totals.len());
    print_stats("update_task TOTAL (wall)", totals);

    // Wait for any queued Tier-2 background rebuild(s) to drain so we can
    // report coalescing effectiveness honestly.
    let exec_at_loop_end = server.tier2_execution_count();
    let pending_at_loop_end = server.graph_rebuild_pending();
    let mut last = u64::MAX;
    let mut stable_ticks = 0;
    let drain_start = Instant::now();
    for _ in 0..600 {
        // ≤ 30s total
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let now = server.tier2_execution_count();
        if !server.graph_rebuild_pending() && now == last {
            stable_ticks += 1;
            if stable_ticks >= 3 {
                break;
            }
        } else {
            stable_ticks = 0;
        }
        last = now;
    }
    let drain_ms = drain_start.elapsed().as_secs_f64() * 1000.0;

    println!(
        "\n=== Tier-2 background rebuild ===\n  \
         pending at end of loop:        {pending_at_loop_end}\n  \
         executions at end of loop:     {exec_at_loop_end}\n  \
         total executions after drain:  {}\n  \
         drain wall time:               {:.1}ms\n  \
         coalescing ratio:              {} writes / {} rebuilds",
        server.tier2_execution_count(),
        drain_ms,
        n + warmup,
        server.tier2_execution_count(),
    );

    println!(
        "\nPer-phase numbers were emitted via tracing at debug level on target perf::*. \
         Re-run with RUST_LOG=perf::update_task=debug,perf::graph_rebuild=debug,perf::vector=debug \
         to see them inline."
    );

    // Concurrent variant: K parallel writers each fire M writes via
    // spawn_blocking. Bursty enough that schedule_graph_rebuild calls
    // overlap with in-flight Tier-2s, which is when coalescing actually
    // engages. Sequential bench can't observe coalescing because writes
    // pace themselves to ≥ Tier-1 latency.
    if std::env::var("PKB_BENCH_CONCURRENT").is_ok() {
        let k: usize = std::env::var("PKB_BENCH_K")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8);
        let m: usize = std::env::var("PKB_BENCH_M")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);
        let exec_baseline = server.tier2_execution_count();
        println!(
            "\n=== Concurrent variant ({k} writers × {m} writes = {} total) ===",
            k * m
        );

        let run_start = Instant::now();
        let mut handles = Vec::with_capacity(k);
        for w in 0..k {
            let server = server.clone();
            let target_id = target_id.clone();
            handles.push(tokio::spawn(async move {
                let mut latencies = Vec::with_capacity(m);
                for i in 0..m {
                    let prio = if (w * m + i) % 2 == 0 { 2 } else { 3 };
                    let args = json!({
                        "id": target_id,
                        "priority": prio,
                    });
                    let server = server.clone();
                    let t = Instant::now();
                    let res = tokio::task::spawn_blocking(move || {
                        server.bench_update_task(&args)
                    })
                    .await
                    .expect("spawn_blocking joined");
                    let elapsed = t.elapsed().as_secs_f64() * 1000.0;
                    if let Err(e) = res {
                        eprintln!("worker {w} call {i} failed: {:?}", e);
                        continue;
                    }
                    latencies.push(elapsed);
                }
                latencies
            }));
        }
        let mut all_latencies = Vec::with_capacity(k * m);
        for h in handles {
            all_latencies.extend(h.await.expect("worker joined"));
        }
        let run_wall_ms = run_start.elapsed().as_secs_f64() * 1000.0;
        print_stats("update_task TOTAL (concurrent)", all_latencies);
        println!("  total wall time:               {:.1}ms", run_wall_ms);

        // Drain background rebuilds and report coalescing for the concurrent run.
        let exec_at_end = server.tier2_execution_count();
        let pending_at_end = server.graph_rebuild_pending();
        let drain_start = Instant::now();
        let mut last = u64::MAX;
        let mut stable_ticks = 0;
        for _ in 0..600 {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            let now = server.tier2_execution_count();
            if !server.graph_rebuild_pending() && now == last {
                stable_ticks += 1;
                if stable_ticks >= 3 {
                    break;
                }
            } else {
                stable_ticks = 0;
            }
            last = now;
        }
        let drain_ms = drain_start.elapsed().as_secs_f64() * 1000.0;
        let total_writes = k * m;
        let rebuilds_during_run = exec_at_end - exec_baseline;
        let rebuilds_total = server.tier2_execution_count() - exec_baseline;
        println!(
            "\n=== Tier-2 (concurrent) ===\n  \
             pending at end of run:         {pending_at_end}\n  \
             rebuilds during run:           {rebuilds_during_run}\n  \
             total rebuilds after drain:    {rebuilds_total}\n  \
             drain wall time:               {:.1}ms\n  \
             coalescing ratio:              {total_writes} writes / {rebuilds_total} rebuilds",
            drain_ms,
        );
    } else {
        println!(
            "\n(Set PKB_BENCH_CONCURRENT=1 to also run the K-writer concurrent variant; \
             tune with PKB_BENCH_K and PKB_BENCH_M.)"
        );
    }

    Ok(())
}
