use mem::bm25::{tokenize, Bm25Index};
use mem::pkb::PkbDocument;
use mem::vectordb::{GateCandidate, VectorStore};
use std::path::{Path, PathBuf};

fn make_test_doc(
    id: &str,
    path: &str,
    title: &str,
    doc_type: &str,
    status: &str,
    body: &str,
    tags: Vec<&str>,
    modified: &str,
) -> PkbDocument {
    let mut fm = serde_json::Map::new();
    fm.insert("id".to_string(), serde_json::Value::String(id.to_string()));
    fm.insert("title".to_string(), serde_json::Value::String(title.to_string()));
    fm.insert("type".to_string(), serde_json::Value::String(doc_type.to_string()));
    fm.insert("status".to_string(), serde_json::Value::String(status.to_string()));
    fm.insert("modified".to_string(), serde_json::Value::String(modified.to_string()));

    PkbDocument {
        path: PathBuf::from(path),
        title: title.to_string(),
        tags: tags.into_iter().map(String::from).collect(),
        doc_type: Some(doc_type.to_string()),
        status: Some(status.to_string()),
        consolidated: Some(false),
        consolidated_at: None,
        modified: Some(modified.to_string()),
        body: body.to_string(),
        content_hash: format!("chash_{id}"),
        file_hash: format!("hash_{id}"),
        frontmatter: Some(serde_json::Value::Object(fm)),
    }
}

// ── AC1: Synthetic Probes for Diacritics ──
#[test]
fn test_ac1_diacritics_nfkd_folding() {
    let mut idx = Bm25Index::new();
    let root = Path::new("/pkb");

    // "András Sajó" contains accented characters á and ó
    idx.upsert(
        "cont_ec562f73",
        PathBuf::from("contacts/andras_sajo.md"),
        "András Sajó",
        Some("contact".to_string()),
        Some("active".to_string()),
        vec!["scholar".to_string()],
        Some("2026-10-20".to_string()),
        None,
        None,
        "Hungarian legal scholar and former judge".to_string(),
        "Hungarian legal scholar and former judge".to_string(),
        "Hungarian legal scholar and former judge of the European Court of Human Rights.",
    );

    idx.upsert(
        "cont_f7421ab9",
        PathBuf::from("contacts/masahiko_shoji.md"),
        "Masahiko Shoji",
        Some("contact".to_string()),
        Some("active".to_string()),
        vec!["sociology".to_string()],
        Some("2026-10-20".to_string()),
        None,
        None,
        "Japanese sociologist and researcher".to_string(),
        "Japanese sociologist and researcher".to_string(),
        "Japanese sociologist and researcher.",
    );

    // Query unaccented "Andras Sajo" must fold and retrieve András Sajó as #1
    let results_unaccented = idx.search("Andras Sajo", 5, root, None, None, None, None);
    assert!(!results_unaccented.is_empty(), "Unaccented query should match accented document");
    assert_eq!(results_unaccented[0].id, "cont_ec562f73");
    assert!(results_unaccented[0].score > 0.0);

    // Query accented "Sajó" must also retrieve András Sajó as #1
    let results_accented = idx.search("Sajó", 5, root, None, None, None, None);
    assert!(!results_accented.is_empty());
    assert_eq!(results_accented[0].id, "cont_ec562f73");
}

// ── AC1: Synthetic Probes for Apostrophes ──
#[test]
fn test_ac1_apostrophes_probe() {
    let mut idx = Bm25Index::new();
    let root = Path::new("/pkb");

    idx.upsert(
        "doc_user_guide",
        PathBuf::from("notes/guide.md"),
        "User Guide",
        Some("note".to_string()),
        Some("active".to_string()),
        vec!["guide".to_string()],
        Some("2026-10-20".to_string()),
        None,
        None,
        "User guide to O'Connor algorithms".to_string(),
        "User guide to O'Connor algorithms".to_string(),
        "User guide to algorithms and procedures.",
    );

    idx.upsert(
        "doc_noise",
        PathBuf::from("notes/noise.md"),
        "Letter S and T analysis",
        Some("note".to_string()),
        Some("active".to_string()),
        vec![],
        Some("2026-10-20".to_string()),
        None,
        None,
        "Analysis of s and t".to_string(),
        "Analysis of s and t".to_string(),
        "Analysis of variable s and variable t in equations.",
    );

    // In v1, tokenize("don't user's") split on apostrophe into ["don", "t", "user", "s"].
    // The loose "t" and "s" positively matched doc_noise.
    // In v2, apostrophes do not emit loose 's' and 't' sub-tokens, and "don't" is a stopword.
    let tokens = tokenize("don't user's O'Connor");
    assert!(!tokens.contains(&"s".to_string()), "must not emit loose 's' token from apostrophe");
    assert!(!tokens.contains(&"t".to_string()), "must not emit loose 't' token from apostrophe");

    // Query "user's guide" stems "user's" to "user", matching doc_user_guide
    let results_user = idx.search("user's guide", 5, root, None, None, None, None);
    assert!(!results_user.is_empty());
    assert_eq!(results_user[0].id, "doc_user_guide");

    // Query "don't user's" should NOT match doc_noise on loose 's'/'t'
    let results_dont = idx.search("don't user's", 5, root, None, None, None, None);
    for r in &results_dont {
        assert_ne!(r.id, "doc_noise", "doc_noise must not match loose 's' or 't' from apostrophes");
    }
}

// ── AC1: Stopword and Filler Sentence Probe ──
#[test]
fn test_ac1_stopword_and_filler_probe() {
    let mut idx = Bm25Index::new();
    let root = Path::new("/pkb");

    // From F15: "hydration not the blood test"
    idx.upsert(
        "blood_test_tips",
        PathBuf::from("notes/blood_test.md"),
        "Blood test tips",
        Some("note".to_string()),
        Some("active".to_string()),
        vec![],
        Some("2026-10-20".to_string()),
        None,
        None,
        "Blood test tips and clinical analysis".to_string(),
        "Blood test tips and clinical analysis".to_string(),
        "Blood test tips and clinical laboratory diagnostics.",
    );

    idx.upsert(
        "hydration_protocol",
        PathBuf::from("notes/hydration.md"),
        "Daily Hydration Protocol",
        Some("note".to_string()),
        Some("active".to_string()),
        vec![],
        Some("2026-10-20".to_string()),
        None,
        None,
        "Hydration intake guidelines".to_string(),
        "Hydration intake guidelines".to_string(),
        "Hydration protocols, fluid balance and water intake guidelines for peak performance.",
    );

    idx.upsert(
        "filler_doc",
        PathBuf::from("notes/filler.md"),
        "Filler Document",
        Some("note".to_string()),
        Some("active".to_string()),
        vec![],
        Some("2026-10-20".to_string()),
        None,
        None,
        "This is not the only way that it was there then".to_string(),
        "This is not the only way that it was there then".to_string(),
        "This is not the only way that it was there then or with all of these.",
    );

    // Query containing stopwords only should return empty (stopwords stripped)
    let results_filler = idx.search("this is not the that it was there then or with all of these", 5, root, None, None, None, None);
    assert!(
        results_filler.is_empty() || results_filler.iter().all(|r| r.score == 0.0),
        "Pure stopword query should not match or score"
    );

    // "hydration not the blood test": "not" and "the" should be stripped
    let tokens = tokenize("hydration not the blood test");
    assert!(!tokens.contains(&"not".to_string()), "tokens must not contain stopword 'not'");
    assert!(!tokens.contains(&"the".to_string()), "tokens must not contain stopword 'the'");
}

// ── AC1: No q_count Multiplier ──
#[test]
fn test_ac1_no_q_count_multiplier() {
    let mut idx = Bm25Index::new();
    let root = Path::new("/pkb");

    idx.upsert(
        "doc1",
        PathBuf::from("notes/doc1.md"),
        "BM25 Retrieval",
        Some("note".to_string()),
        Some("active".to_string()),
        vec![],
        Some("2026-10-20".to_string()),
        None,
        None,
        "Algorithm implementation".to_string(),
        "Algorithm implementation".to_string(),
        "Algorithm implementation for ranking.",
    );

    let res_single = idx.search("algorithm", 5, root, None, None, None, None);
    let res_repeated = idx.search("algorithm algorithm algorithm", 5, root, None, None, None, None);

    assert_eq!(res_single.len(), 1);
    assert_eq!(res_repeated.len(), 1);
    // In v2 without q_count multiplier, repeating query terms does not artificially multiply score
    assert!(
        (res_single[0].score - res_repeated[0].score).abs() < 1e-4,
        "Scores should match without q_count multiplier: single={}, repeated={}",
        res_single[0].score,
        res_repeated[0].score
    );
}

// ── AC2: Gate Predicate Pre-Pool Exclusion in bm25::search and vectordb::search ──
#[test]
fn test_ac2_gate_predicate_pre_pool_exclusion() {
    let mut idx = Bm25Index::new();
    let root = Path::new("/pkb");

    idx.upsert(
        "task_active",
        PathBuf::from("tasks/task_active.md"),
        "Search Optimization",
        Some("task".to_string()),
        Some("active".to_string()),
        vec!["search".to_string()],
        Some("2026-10-20".to_string()),
        None,
        None,
        "Active search task".to_string(),
        "Active search task".to_string(),
        "Optimizing indexing performance.",
    );

    idx.upsert(
        "task_superseded",
        PathBuf::from("tasks/task_superseded.md"),
        "Search Optimization",
        Some("task".to_string()),
        Some("superseded".to_string()),
        vec!["search".to_string()],
        Some("2026-10-20".to_string()),
        None,
        None,
        "Superseded search task".to_string(),
        "Superseded search task".to_string(),
        "Optimizing indexing performance with extra terms indexing indexing.",
    );

    // Gate predicate excludes superseded documents
    let gate = |cand: &GateCandidate| cand.status != Some("superseded");

    // With limit 1, if predicate ran AFTER truncation, task_superseded (higher score) might have
    // taken the slot and then been dropped, leaving 0 results. Pre-pool exclusion ensures
    // task_active is returned!
    let results = idx.search("indexing", 1, root, None, None, None, Some(&gate));
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "task_active");

    // Test vectordb.search and search_hybrid pre-pool gate predicate
    let mut store = VectorStore::new(3);
    let doc_active = make_test_doc(
        "task_active",
        "tasks/task_active.md",
        "Search Optimization",
        "task",
        "active",
        "Optimizing indexing performance.",
        vec!["search"],
        "2026-10-20",
    );
    let doc_superseded = make_test_doc(
        "task_superseded",
        "tasks/task_superseded.md",
        "Search Optimization",
        "task",
        "superseded",
        "Optimizing indexing performance with extra terms indexing indexing.",
        vec!["search"],
        "2026-10-20",
    );

    store.insert_precomputed(&doc_active, vec![doc_active.body.clone()], vec![vec![1.0, 0.0, 0.0]]);
    store.insert_precomputed(&doc_superseded, vec![doc_superseded.body.clone()], vec![vec![1.0, 0.0, 0.0]]);

    let vec_results = store.search(
        &[1.0, 0.0, 0.0],
        1,
        root,
        None,
        None,
        None,
        Some(&gate),
    );
    assert_eq!(vec_results.len(), 1);
    assert_eq!(vec_results[0].id, "task_active");

    let hybrid_results = store.search_hybrid(
        "indexing",
        &[1.0, 0.0, 0.0],
        1,
        root,
        None,
        None,
        None,
        None,
        Some(&gate),
    );
    assert_eq!(hybrid_results.len(), 1);
    assert_eq!(hybrid_results[0].id, "task_active");
}

// ── AC2: Deterministic Tie-Breaking by ID then Chunk ID ──
#[test]
fn test_ac2_deterministic_tie_breaking_by_id_then_chunk_id() {
    let mut idx = Bm25Index::new();
    let root = Path::new("/pkb");

    // Identical content -> identical BM25 score
    idx.upsert(
        "doc_zebra",
        PathBuf::from("notes/zebra.md"),
        "Matching Title",
        Some("note".to_string()),
        Some("active".to_string()),
        vec![],
        Some("2026-10-20".to_string()),
        None,
        None,
        "Identical snippet".to_string(),
        "Identical snippet".to_string(),
        "Identical content body for testing tie-break ordering.",
    );

    idx.upsert(
        "doc_alpha",
        PathBuf::from("notes/alpha.md"),
        "Matching Title",
        Some("note".to_string()),
        Some("active".to_string()),
        vec![],
        Some("2026-10-20".to_string()),
        None,
        None,
        "Identical snippet".to_string(),
        "Identical snippet".to_string(),
        "Identical content body for testing tie-break ordering.",
    );

    let results = idx.search("Identical content body", 10, root, None, None, None, None);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].score, results[1].score);
    // Tie-break by ID ascending: "doc_alpha" must precede "doc_zebra"
    assert_eq!(results[0].id, "doc_alpha");
    assert_eq!(results[1].id, "doc_zebra");

    // Test in vectordb.search as well
    let mut store = VectorStore::new(3);
    let doc_z = make_test_doc("doc_zebra", "notes/zebra.md", "Title", "note", "active", "body", vec![], "2026-10-20");
    let doc_a = make_test_doc("doc_alpha", "notes/alpha.md", "Title", "note", "active", "body", vec![], "2026-10-20");
    store.insert_precomputed(&doc_z, vec![doc_z.body.clone()], vec![vec![1.0, 0.0, 0.0]]);
    store.insert_precomputed(&doc_a, vec![doc_a.body.clone()], vec![vec![1.0, 0.0, 0.0]]);

    let vec_res = store.search(&[1.0, 0.0, 0.0], 10, root, None, None, None, None);
    assert_eq!(vec_res.len(), 2);
    assert_eq!(vec_res[0].id, "doc_alpha");
    assert_eq!(vec_res[1].id, "doc_zebra");
}

// ── AC3: rank_v1 Flag Reproduces Legacy Tokenizer Output ──
#[test]
fn test_ac3_rank_v1_legacy_parity() {
    let root = Path::new("/pkb");

    // When rank_v1 is enabled at load:
    let mut idx_v1 = Bm25Index::new_with_rank_v1(true);
    idx_v1.upsert(
        "mem_7cc72ed8",
        PathBuf::from("tasks/mem_7cc72ed8.md"),
        "PKB search: add a lexical (BM25) layer",
        Some("task".to_string()),
        Some("in_progress".to_string()),
        vec!["bm25".to_string(), "rrf".to_string()],
        Some("2026-08-31".to_string()),
        None,
        None,
        "Add a lexical (BM25) retrieval layer to PKB search".to_string(),
        "Add a lexical (BM25) retrieval layer to PKB search".to_string(),
        "Add a lexical (BM25) retrieval layer to PKB search and fuse with vector scan via RRF. PR #520.",
    );

    idx_v1.upsert(
        "other_doc",
        PathBuf::from("notes/other.md"),
        "General architectural thoughts",
        Some("note".to_string()),
        Some("active".to_string()),
        vec!["architecture".to_string()],
        Some("2026-08-30".to_string()),
        None,
        None,
        "General note about system design".to_string(),
        "General note about system design".to_string(),
        "Nothing related to the specific query here.",
    );

    // Legacy queries from existing tests
    let res_task = idx_v1.search("mem_7cc72ed8", 5, root, None, None, None, None);
    assert_eq!(res_task.len(), 1);
    assert_eq!(res_task[0].id, "mem_7cc72ed8");

    let res_pr = idx_v1.search("PR #520", 5, root, None, None, None, None);
    assert_eq!(res_pr.len(), 1);
    assert_eq!(res_pr[0].id, "mem_7cc72ed8");
}
