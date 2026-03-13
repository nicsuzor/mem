#[cfg(test)]
mod tests {
    use crate::vectordb::{VectorStore, DocumentEntry};
    use crate::embeddings::{ChunkConfig, chunk_text};
    use crate::pkb::PkbDocument;
    use std::path::PathBuf;

    #[test]
    fn test_snippet_mismatch() {
        let mut store = VectorStore::new(384);

        let large_header = "A".repeat(2000);
        let unique_keyword = "UNIQUE_KEYWORD_IN_BODY";
        let body_content = format!("{} and some more text...", unique_keyword);

        let chunk_texts = vec![large_header.clone(), body_content.clone()];
        let body_chunks = vec![body_content.clone()];

        let mut emb1 = vec![0.0; 384]; emb1[1] = 1.0;

        let entry = DocumentEntry {
            path: PathBuf::from("test.md"),
            title: "Test".to_string(),
            doc_type: None,
            status: None,
            tags: vec![],
            project: None,
            id: None,
            confidence: None,
            content_hash: Some("test_hash".to_string()),
            chunk_embeddings: vec![vec![0.0; 384], emb1.clone()],
            chunk_texts,
            body_chunks,
        };

        let doc = PkbDocument {
            path: PathBuf::from("test.md"),
            title: "Test".to_string(),
            tags: vec![],
            doc_type: None,
            status: None,
            body: body_content.clone(),
            content_hash: "test_hash".to_string(),
            modified: None,
            frontmatter: None,
        };

        store.insert_precomputed(&doc, vec![large_header, body_content.clone()], vec![vec![0.0; 384], emb1]);

        let mut query = vec![0.0; 384]; query[1] = 1.0;
        let results = store.search(&query, 1, &PathBuf::from("."));

        assert!(!results.is_empty());
        println!("Snippet: '{}'", results[0].snippet);
    }
}
