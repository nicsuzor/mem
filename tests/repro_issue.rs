#[cfg(test)]
mod tests {
    use mem::document_crud::{update_document, create_document, DocumentFields};
    use serde_json::json;
    use std::collections::HashMap;
    use std::fs;

    #[test]
    fn test_repro_contributes_to() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();

        let fields = DocumentFields {
            title: "Repro Task".to_string(),
            doc_type: "task".to_string(),
            ..Default::default()
        };

        let path = create_document(&root, fields).unwrap();

        let mut updates = HashMap::new();
        let contributes_to = json!([
            {"to": "task-9c33dd1b", "weight": "Certain", "why": "interim subdeadline"}
        ]);
        updates.insert("contributes_to".to_string(), contributes_to.clone());
        updates.insert("severity".to_string(), json!("high"));

        update_document(&path, updates).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        println!("Content after update:\n{}", content);

        assert!(content.contains("severity: high") || content.contains("severity: \"high\""), "severity should be present");
        assert!(content.contains("contributes_to:"), "contributes_to should be present");
        assert!(content.contains("to: task-9c33dd1b") || content.contains("to: \"task-9c33dd1b\""), "nested field 'to' should be present");
        assert!(content.contains("weight: Certain") || content.contains("weight: \"Certain\""), "nested field 'weight' should be present");
    }
}
