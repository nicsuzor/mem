#[cfg(test)]
mod tests {
    use crate::document_crud::{update_document, create_document, DocumentFields};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn test_update_document_format() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();

        let fields = DocumentFields {
            title: "Test Doc".to_string(),
            doc_type: "note".to_string(),
            ..Default::default()
        };

        let path = create_document(&root, fields).unwrap();

        let mut updates = HashMap::new();
        updates.insert("title".to_string(), json!("Updated Title"));

        update_document(&path, updates).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        println!("Content after update:\n{}", content);

        assert!(content.starts_with("---\n"));
        // Check for double dashes
        let dash_count = content.matches("---").count();
        assert_eq!(dash_count, 2, "Should have exactly two '---' separators");

        assert!(content.contains("title: \"Updated Title\""));
    }
}
