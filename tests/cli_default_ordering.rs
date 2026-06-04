//! Behavioural tests for the CLI `tasks` surface: the DEFAULT ordering (no
//! `--sort` argument) must be `focus_score`-descending, agreeing with the MCP
//! `list_tasks` default, while an EXPLICIT `--sort` argument is honoured verbatim.
//!
//! These run fully offline — `pkb tasks` builds the graph from the PKB directory
//! and needs no embeddings/ONNX — so they execute on every machine. (mem-e394a6d0)

use std::process::Command;

/// Seed a temp PKB whose focus_score ordering DIVERGES from priority ordering:
///   - t-hi   : priority 0          → focus_score 10000
///   - t-mid  : priority 1          → focus_score  5000
///   - t-sev  : priority 2, sev 4   → focus_score 100000 (severity dominates)
///
/// So focus order is [t-sev, t-hi, t-mid] but priority order is [t-hi, t-mid, t-sev].
fn seed(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("tasks")).unwrap();
    std::fs::create_dir_all(dir.join("projects")).unwrap();
    std::fs::write(
        dir.join("projects/p.md"),
        "---\nid: p\ntitle: P\ntype: project\nstatus: active\n---\n# P\n",
    )
    .unwrap();
    let tasks = [
        ("t-hi", "priority: 0\n"),
        ("t-mid", "priority: 1\n"),
        ("t-sev", "priority: 2\nseverity: 4\n"),
    ];
    for (id, extra) in tasks {
        std::fs::write(
            dir.join(format!("tasks/{id}.md")),
            format!("---\nid: {id}\ntitle: Task {id}\ntype: task\nstatus: active\n{extra}parent: p\n---\nbody for {id}\n"),
        )
        .unwrap();
    }
}

/// Run `pkb tasks all --flat [extra args]` and return the ordered, de-duplicated
/// list of seeded task IDs as they appear in the output.
fn ordered_ids(dir: &std::path::Path, extra: &[&str]) -> Vec<String> {
    let db = dir.join("db.bin");
    let mut args: Vec<String> = vec![
        "--pkb-root".into(),
        dir.to_string_lossy().into(),
        "--db-path".into(),
        db.to_string_lossy().into(),
        "tasks".into(),
        "all".into(),
        "--flat".into(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));

    let out = Command::new(env!("CARGO_BIN_EXE_pkb"))
        .args(&args)
        .env("AOPS_OFFLINE", "1")
        .output()
        .expect("run pkb tasks");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let known = ["t-hi", "t-mid", "t-sev"];
    let mut seen = Vec::new();
    for line in stdout.lines() {
        for id in known {
            // Match the `[t-xx]` id token specifically so the title mention
            // ("Task t-xx") doesn't double-count.
            if line.contains(&format!("[{id}]")) && !seen.contains(&id.to_string()) {
                seen.push(id.to_string());
            }
        }
    }
    seen
}

/// AC1 (CLI parity): with NO `--sort` argument the default order is focus_score
/// descending — the SEV4 task sorts ahead of the P0 task even though its raw
/// priority is lower. This is the same comparator the MCP `list_tasks` default
/// uses, so the two surfaces agree.
#[test]
fn cli_tasks_default_order_is_focus_desc() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let ids = ordered_ids(dir.path(), &[]);
    assert_eq!(
        ids,
        vec!["t-sev", "t-hi", "t-mid"],
        "default `pkb tasks` order must be focus_score-DESC (sev4 first), got {ids:?}"
    );
}

/// AC4 (backward compatibility): an EXPLICIT `--sort priority` is honoured
/// verbatim — ascending by raw priority — and is NOT overridden by the new
/// focus_score default. The SEV4/P2 task drops to last under priority sort.
#[test]
fn cli_tasks_explicit_sort_priority_is_honoured() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let ids = ordered_ids(dir.path(), &["--sort", "priority"]);
    assert_eq!(
        ids,
        vec!["t-hi", "t-mid", "t-sev"],
        "explicit `--sort priority` must order by raw priority ascending, got {ids:?}"
    );
}

/// AC6 (determinism): repeated identical default calls return identical ordering.
#[test]
fn cli_tasks_default_order_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let first = ordered_ids(dir.path(), &[]);
    let second = ordered_ids(dir.path(), &[]);
    assert_eq!(first, second, "repeated default `pkb tasks` calls must agree");
    assert_eq!(first.len(), 3, "all three seeded tasks should appear");
}
