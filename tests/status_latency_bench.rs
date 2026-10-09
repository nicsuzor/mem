use mem::embeddings::{Embedder, EMBEDDING_DIM};
use mem::vectordb::VectorStore;
use mem::{check_index_staleness_with_stats, document_needs_reindex, index_pkb};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;

#[test]
fn bench_status_latency_on_full_corpus() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pkb_root = dir.path();
    let db_path = pkb_root.join("test.db");

    println!("Generating full corpus of 4,000 markdown documents...");
    let t_gen = Instant::now();
    for i in 0..4000 {
        let sub = if i < 1000 {
            "tasks"
        } else if i < 2000 {
            "notes"
        } else if i < 3000 {
            "daily"
        } else {
            "knowledge"
        };
        let sub_dir = pkb_root.join(sub);
        std::fs::create_dir_all(&sub_dir).unwrap();
        let file_path = sub_dir.join(format!("doc_{i:04}.md"));
        let content = format!(
            "---\nid: doc_{i:04}\ntitle: Document {i:04}\ntype: {}\nstatus: {}\nmodified: 2026-10-09T12:00:00Z\ntags:\n  - pkb\n  - perf\n---\n\n## Content for document {i:04}\n\nThis is paragraph 1 of the body text for document {i:04}. It contains typical prose length for knowledge base items.\n\nParagraph 2 adds more text to simulate realistic disk sizes and hashing work.",
            if i < 1000 { "task" } else { "note" },
            if i % 2 == 0 { "ready" } else { "inbox" }
        );
        std::fs::write(&file_path, content).unwrap();
    }
    println!("Generated 4,000 files in {:.2?}", t_gen.elapsed());

    let store = Arc::new(RwLock::new(VectorStore::new(EMBEDDING_DIM)));
    let embedder = Embedder::new_dummy();

    println!("Indexing full corpus into store...");
    let t_idx = Instant::now();
    let (indexed, _, total) = index_pkb(pkb_root, &db_path, &store, &embedder, false);
    assert_eq!(indexed, 4000);
    assert_eq!(total, 4000);
    println!("Indexed 4,000 documents in {:.2?}", t_idx.elapsed());

    // 1. Measure "BEFORE" behavior:
    // What the old check_index_staleness did:
    // Read, parse, and hash all 4,000 documents under the store read lock.
    println!("\nMeasuring BEFORE: un-stamped full read + parse + hash under store lock...");
    let files = mem::pkb::scan_directory(pkb_root);
    assert_eq!(files.len(), 4000);

    let t_before_start = Instant::now();
    let before_count = {
        let s = store.read();
        files
            .iter()
            .filter_map(|file_path| mem::pkb::parse_file_relative(file_path, pkb_root))
            .filter(|doc| document_needs_reindex(&s, doc))
            .count()
    };
    let before_duration = t_before_start.elapsed();
    assert_eq!(before_count, 0);
    println!(
        "BEFORE latency: {:.2?} ({} ms)",
        before_duration,
        before_duration.as_millis()
    );

    // 2. Measure "AFTER" behavior on unchanged corpus:
    // Skipped by stamp, lock never held across the scan.
    println!("\nMeasuring AFTER: stamp-skipping lock-free scan on 4,000 unchanged files...");
    let t_after_start = Instant::now();
    let after_report = check_index_staleness_with_stats(pkb_root, &store);
    let after_duration = t_after_start.elapsed();
    assert_eq!(after_report.stale_count, 0);
    assert_eq!(after_report.files_read, 0);
    assert_eq!(after_report.files_scanned, 4000);
    println!(
        "AFTER latency (unchanged): {:.2?} ({:.3} ms) — files_read: {}",
        after_duration,
        after_duration.as_secs_f64() * 1000.0,
        after_report.files_read
    );

    // Run again to verify consistency
    let t_after_repeat = Instant::now();
    let after_report2 = check_index_staleness_with_stats(pkb_root, &store);
    let after_duration2 = t_after_repeat.elapsed();
    assert_eq!(after_report2.stale_count, 0);
    assert_eq!(after_report2.files_read, 0);
    println!(
        "AFTER latency (repeat): {:.2?} ({:.3} ms) — files_read: {}",
        after_duration2,
        after_duration2.as_secs_f64() * 1000.0,
        after_report2.files_read
    );

    // 3. Measure AFTER behavior when 1 file is modified:
    println!("\nMeasuring AFTER when 1 file out of 4,000 is modified...");
    std::thread::sleep(std::time::Duration::from_millis(20));
    let mod_file = pkb_root.join("tasks/doc_0042.md");
    std::fs::write(
        &mod_file,
        "---\nid: doc_0042\ntitle: Document 0042 Modified\ntype: task\nstatus: in_progress\nmodified: 2026-10-09T23:00:00Z\ntags:\n  - pkb\n  - perf\n---\n\n## Content modified\nNew body content.",
    )
    .unwrap();

    let t_mod_start = Instant::now();
    let mod_report = check_index_staleness_with_stats(pkb_root, &store);
    let mod_duration = t_mod_start.elapsed();
    assert_eq!(mod_report.stale_count, 1);
    assert_eq!(mod_report.files_read, 1);
    assert_eq!(mod_report.files_scanned, 4000);
    println!(
        "AFTER latency (1 file modified): {:.2?} ({:.3} ms) — files_read: {}, stale_count: {}",
        mod_duration,
        mod_duration.as_secs_f64() * 1000.0,
        mod_report.files_read,
        mod_report.stale_count
    );

    let speedup = before_duration.as_secs_f64() / after_duration.as_secs_f64();
    println!("\n=== Summary ===");
    println!("Corpus size: 4,000 files");
    println!(
        "Before (read+parse+hash every file under lock): {:.2} ms",
        before_duration.as_secs_f64() * 1000.0
    );
    println!(
        "After (unchanged files skipped by stamp, lock-free): {:.2} ms",
        after_duration.as_secs_f64() * 1000.0
    );
    println!("Speedup factor: {:.1}x faster", speedup);
    assert!(
        speedup > 10.0,
        "Expected at least 10x speedup, got {:.1}x",
        speedup
    );
}
