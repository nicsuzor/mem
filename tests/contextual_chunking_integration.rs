use mem::chunker::{
    chunk_document, chunk_markdown_clean, DocumentRecord, CHUNK_TARGET_CHARS,
};
use mem::pkb::PkbDocument;
use mem::vectordb::VectorStore;
use std::path::{Path, PathBuf};

fn make_doc(
    id: &str,
    path: &str,
    title: &str,
    doc_type: &str,
    body: &str,
    tags: Vec<&str>,
) -> PkbDocument {
    let mut fm = serde_json::Map::new();
    fm.insert("id".to_string(), serde_json::Value::String(id.to_string()));
    fm.insert("title".to_string(), serde_json::Value::String(title.to_string()));
    fm.insert("type".to_string(), serde_json::Value::String(doc_type.to_string()));
    fm.insert("status".to_string(), serde_json::Value::String("active".to_string()));

    PkbDocument {
        path: PathBuf::from(path),
        title: title.to_string(),
        tags: tags.into_iter().map(String::from).collect(),
        doc_type: Some(doc_type.to_string()),
        status: Some("active".to_string()),
        consolidated: Some(false),
        consolidated_at: None,
        modified: Some("2026-10-24".to_string()),
        body: body.to_string(),
        content_hash: format!("chash_{id}"),
        file_hash: format!("hash_{id}"),
        frontmatter: Some(serde_json::Value::Object(fm)),
    }
}

#[test]
fn test_corpus_rechunking_contextual_headers_and_bm25_search() {
    let mut store = VectorStore::new(3);
    let root = Path::new("/pkb");

    // Create a multi-chunk document with headings
    let mut body = String::new();
    body.push_str("# Section 1: Ingestion\n\n");
    for i in 0..15 {
        body.push_str(&format!("Line {:02} about fast stream parsing with zero copies.\n", i));
    }
    body.push_str("\n## Subsection 1.1: Kafka Workers\n\n");
    for i in 0..15 {
        body.push_str(&format!("Line {:02} detailing consumer group balancing and heartbeats.\n", i));
    }
    body.push_str("\n# Section 2: Storage\n\n");
    for i in 0..15 {
        body.push_str(&format!("Line {:02} describing persistent LSM trees and WAL flush mechanics.\n", i));
    }

    let doc = make_doc(
        "doc_pipeline",
        "docs/pipeline.md",
        "Data Pipeline Architecture",
        "knowledge",
        &body,
        vec!["pipeline", "storage"],
    );

    // Chunk using chunk_document
    let doc_chunks = chunk_document(&doc.title, doc.doc_type.as_deref(), &doc.body);
    assert!(doc_chunks.len() >= 2, "Document must split into multiple chunks");

    // Verify AC2: Every chunk has contextual header prepended
    for chunk in &doc_chunks {
        assert!(
            chunk.text.starts_with("Data Pipeline Architecture | knowledge"),
            "Chunk text missing expected header prefix: {}",
            chunk.text
        );
        // Clean body does NOT contain the synthetic header
        assert!(
            !chunk.body.contains("Data Pipeline Architecture | knowledge"),
            "Clean chunk body should not contain synthetic header: {}",
            chunk.body
        );
        // Start and end offsets slice accurately into body
        assert_eq!(&body[chunk.start_offset..chunk.end_offset].trim(), &chunk.body);
    }

    // Verify chunk texts and synthetic vectors in store
    let chunk_texts: Vec<String> = doc_chunks.iter().map(|c| c.text.clone()).collect();
    let num_chunks = chunk_texts.len();
    // Distinct vector per chunk
    let mut chunk_embeddings = Vec::new();
    for i in 0..num_chunks {
        let mut v = vec![0.0f32; 3];
        v[i % 3] = 1.0;
        chunk_embeddings.push(v);
    }

    store.insert_precomputed(&doc, chunk_texts.clone(), chunk_embeddings.clone());

    // Verify BM25 lexical indexing carries contextual headers (doc_type, title, heading)
    // Search for "knowledge" (from contextual header {doc_type})
    let bm25_hits = store.search_bm25("knowledge", 10, root, None, None, None, None);
    assert!(!bm25_hits.is_empty(), "BM25 should match doc_type from contextual headers");
    assert_eq!(bm25_hits[0].id, "doc_pipeline");

    // Search for specific heading term "Ingestion" or "Kafka"
    let kafka_hits = store.search_bm25("Kafka", 10, root, None, None, None, None);
    assert!(!kafka_hits.is_empty(), "BM25 should match heading token from contextual headers");
    assert_eq!(kafka_hits[0].id, "doc_pipeline");

    // Search for body term "LSM"
    let lsm_hits = store.search_bm25("LSM", 10, root, None, None, None, None);
    assert!(!lsm_hits.is_empty(), "BM25 should match body token");
    assert_eq!(lsm_hits[0].id, "doc_pipeline");

    // Verify AC3: Multi-chunk max-pooling dense score and best-chunk snippet selection
    let query_chunk0 = vec![1.0, 0.0, 0.0];
    let vector_results = store.search(&query_chunk0, 10, root, None, None, None, None);
    assert!(!vector_results.is_empty());
    let top_result = &vector_results[0];
    assert_eq!(top_result.id, "doc_pipeline");
    // Cosine similarity for query aligned with chunk 0 is 1.0
    assert!((top_result.score - 1.0).abs() < 1e-4);

    // Verify snippet does not contain the synthetic contextual header
    assert!(
        !top_result.snippet.contains("Data Pipeline Architecture | knowledge"),
        "Snippet must not duplicate synthetic header: {}",
        top_result.snippet
    );
    assert!(
        !top_result.chunk_text.contains("Data Pipeline Architecture | knowledge"),
        "Clean chunk_text must not contain synthetic header: {}",
        top_result.chunk_text
    );
}

#[test]
fn test_document_record_compatibility_and_max_pooling() {
    let v1 = vec![0.8, 0.6, 0.0];
    let v2 = vec![0.0, 0.6, 0.8];
    let chunks = vec![
        mem::chunker::ChunkRecord {
            text: "My Doc | note | Section A\n\nContent A".to_string(),
            body: "Content A".to_string(),
            heading_path: Some("Section A".to_string()),
            vector: Some(v1.clone()),
            start_offset: 0,
            end_offset: 9,
        },
        mem::chunker::ChunkRecord {
            text: "My Doc | note | Section B\n\nContent B".to_string(),
            body: "Content B".to_string(),
            heading_path: Some("Section B".to_string()),
            vector: Some(v2.clone()),
            start_offset: 10,
            end_offset: 19,
        },
    ];

    let doc_record = DocumentRecord::new(
        "doc_test".to_string(),
        "My Doc".to_string(),
        Some("note".to_string()),
        chunks,
    );

    // AC3: DocumentRecord.vector is the mean of chunk vectors
    assert_eq!(doc_record.vector.len(), 3);
    assert!((doc_record.vector[0] - 0.4).abs() < 1e-6);
    assert!((doc_record.vector[1] - 0.6).abs() < 1e-6);
    assert!((doc_record.vector[2] - 0.4).abs() < 1e-6);

    // AC3: Dense scoring computes max_i cos(e(q), e(c_i))
    let q = vec![0.0, 0.0, 1.0];
    let (score, best_idx) = doc_record.max_pool_score(&q);
    assert_eq!(best_idx, 1);
    // cos(q, v2) = (0.8) / (1.0 * 1.0) = 0.8
    assert!((score - 0.8).abs() < 1e-5);

    // AC3: Snippet generation selects highest scoring chunk
    let snippet = doc_record.best_snippet(&q, 100);
    assert_eq!(snippet, "Content B");
}

#[test]
fn test_line_boundary_and_overlap_constraints() {
    // 35 lines of 60 characters each (~2100 chars)
    let lines: Vec<String> = (0..35)
        .map(|i| format!("Line {:02} {}", i, "y".repeat(50)))
        .collect();
    let body = lines.join("\n");

    let chunks = chunk_document("Boundary Doc", Some("spec"), &body);
    assert!(chunks.len() >= 2);

    for chunk in &chunks {
        // Line-boundary check: each line in the chunk body must be a complete line
        for line in chunk.body.lines() {
            assert!(line.starts_with("Line "), "Line was truncated mid-line: {}", line);
        }
        // Size bounds check
        assert!(chunk.body.len() <= CHUNK_TARGET_CHARS + 250);
    }

    // Overlap bounds check
    let clean_chunks = chunk_markdown_clean(&body);
    assert_eq!(clean_chunks.len(), chunks.len());
    let c0_lines: Vec<&str> = clean_chunks[0].lines().collect();
    let c1_lines: Vec<&str> = clean_chunks[1].lines().collect();
    assert!(c1_lines.contains(c0_lines.last().unwrap()));
}
