use mem::graph::{GraphNode, LinkLabel};
use mem::pkb::PkbDocument;
use serde_json::json;
use std::path::PathBuf;

fn make_doc(id: &str, fm: serde_json::Value) -> PkbDocument {
    let mut fm_obj = fm.as_object().cloned().unwrap_or_default();
    if !fm_obj.contains_key("id") {
        fm_obj.insert("id".to_string(), json!(id));
    }
    if !fm_obj.contains_key("title") {
        fm_obj.insert("title".to_string(), json!("Test Document"));
    }
    let full_fm = serde_json::Value::Object(fm_obj);
    PkbDocument {
        path: PathBuf::from(format!("tasks/{id}.md")),
        title: "Test Document".to_string(),
        tags: Vec::new(),
        doc_type: full_fm.get("type").and_then(|v| v.as_str()).map(String::from),
        status: full_fm.get("status").and_then(|v| v.as_str()).map(String::from),
        consolidated: None,
        consolidated_at: None,
        modified: Some("2026-11-06T00:00:00Z".to_string()),
        body: String::new(),
        content_hash: String::new(),
        file_hash: String::new(),
        frontmatter: Some(full_fm),
    }
}

#[test]
fn test_depends_on_maps_to_needs_with_is_to_true() {
    let fm = json!({
        "id": "task_b",
        "type": "task",
        "depends_on": ["task_a"]
    });
    let doc = make_doc("task_b", fm);
    let node = GraphNode::from_pkb_document(&doc);

    assert_eq!(node.links.len(), 1);
    let link = &node.links[0];
    assert_eq!(link.label, LinkLabel::Needs);
    assert_eq!(link.to.as_deref(), Some("task_a"));
    assert_eq!(link.from, None);
    assert_eq!(node.needs, vec!["task_a".to_string()]);
}

#[test]
fn test_needs_frontmatter_parsed_directly_as_needs_edge() {
    let fm = json!({
        "id": "task_b",
        "type": "task",
        "needs": ["task_a"]
    });
    let doc = make_doc("task_b", fm);
    let node = GraphNode::from_pkb_document(&doc);

    assert_eq!(node.links.len(), 1);
    let link = &node.links[0];
    assert_eq!(link.label, LinkLabel::Needs);
    assert_eq!(link.to.as_deref(), Some("task_a"));
    assert_eq!(link.from, None);
    assert_eq!(node.needs, vec!["task_a".to_string()]);
}

#[test]
fn test_type_epic_treated_as_task() {
    let fm = json!({
        "id": "epic_01",
        "type": "epic"
    });
    let doc = make_doc("epic_01", fm);
    let node = GraphNode::from_pkb_document(&doc);

    assert_eq!(node.node_type.as_deref(), Some("task"));
    assert_eq!(node.raw_node_type.as_deref(), Some("epic"));
}

#[test]
fn test_retired_labels_rejected() {
    let fm = json!({
        "id": "task_legacy",
        "type": "task",
        "links": [
            { "to": "task_s", "label": "supports" },
            { "to": "task_alt", "label": "alternative" },
            { "to": "task_set", "label": "settles" }
        ],
        "soft_depends_on": ["task_s"]
    });
    let doc = make_doc("task_legacy", fm);
    let node = GraphNode::from_pkb_document(&doc);

    // None of the retired links are stored
    assert!(node.links.is_empty());
    // Parse warnings emitted for each invalid label and retired soft_depends_on
    assert!(node.parse_warnings.iter().any(|w| w.field == "links[0].label" && w.message.contains("not in {serves, needs, part_of}")));
    assert!(node.parse_warnings.iter().any(|w| w.field == "links[1].label" && w.message.contains("not in {serves, needs, part_of}")));
    assert!(node.parse_warnings.iter().any(|w| w.field == "links[2].label" && w.message.contains("not in {serves, needs, part_of}")));
    assert!(node.parse_warnings.iter().any(|w| w.field == "soft_depends_on" && w.message.contains("retired")));
}
