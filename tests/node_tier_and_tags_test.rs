use mem::graph::GraphNode;
use mem::pkb::{parse_file_relative, PkbDocument};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_graph_node_tier_derivation() {
    // 1. status: archived -> tier: archive
    let archived_doc = PkbDocument {
        path: "doc1.md".into(),
        title: "Archived Doc".into(),
        doc_type: Some("note".into()),
        status: Some("archived".into()),
        tags: vec![],
        modified: None,
        content_hash: "hash".into(),
        file_hash: "hash".into(),
        frontmatter: Some(serde_json::json!({
            "id": "doc1",
            "title": "Archived Doc",
            "type": "note",
            "status": "archived"
        })),
        body: "Some body".into(),
        consolidated: None,
        consolidated_at: None,
    };
    let node1 = GraphNode::from_pkb_document(&archived_doc);
    assert_eq!(node1.tier, "archive");

    // 2. type: session-log -> tier: log
    let session_log_doc = PkbDocument {
        path: "doc2.md".into(),
        title: "Session Log".into(),
        doc_type: Some("session-log".into()),
        status: Some("done".into()),
        tags: vec![],
        modified: None,
        content_hash: "hash".into(),
        file_hash: "hash".into(),
        frontmatter: Some(serde_json::json!({
            "id": "doc2",
            "title": "Session Log",
            "type": "session-log",
            "status": "done"
        })),
        body: "Some transcript".into(),
        consolidated: None,
        consolidated_at: None,
    };
    let node2 = GraphNode::from_pkb_document(&session_log_doc);
    assert_eq!(node2.tier, "log");

    // 3. type: transcript -> tier: log
    let transcript_doc = PkbDocument {
        path: "doc3.md".into(),
        title: "Transcript".into(),
        doc_type: Some("transcript".into()),
        status: Some("done".into()),
        tags: vec![],
        modified: None,
        content_hash: "hash".into(),
        file_hash: "hash".into(),
        frontmatter: Some(serde_json::json!({
            "id": "doc3",
            "title": "Transcript",
            "type": "transcript",
            "status": "done"
        })),
        body: "Transcript body".into(),
        consolidated: None,
        consolidated_at: None,
    };
    let node3 = GraphNode::from_pkb_document(&transcript_doc);
    assert_eq!(node3.tier, "log");

    // 4. type: verbatim-log -> tier: log
    let verbatim_doc = PkbDocument {
        path: "doc4.md".into(),
        title: "Verbatim Log".into(),
        doc_type: Some("verbatim-log".into()),
        status: Some("done".into()),
        tags: vec![],
        modified: None,
        content_hash: "hash".into(),
        file_hash: "hash".into(),
        frontmatter: Some(serde_json::json!({
            "id": "doc4",
            "title": "Verbatim Log",
            "type": "verbatim-log",
            "status": "done"
        })),
        body: "Verbatim body".into(),
        consolidated: None,
        consolidated_at: None,
    };
    let node4 = GraphNode::from_pkb_document(&verbatim_doc);
    assert_eq!(node4.tier, "log");

    // 5. type: template -> tier: knowledge (never log for template)
    let template_doc = PkbDocument {
        path: "doc5.md".into(),
        title: "Template".into(),
        doc_type: Some("template".into()),
        status: Some("ready".into()),
        tags: vec![],
        modified: None,
        content_hash: "hash".into(),
        file_hash: "hash".into(),
        frontmatter: Some(serde_json::json!({
            "id": "doc5",
            "title": "Template",
            "type": "template",
            "status": "ready"
        })),
        body: "Template body".into(),
        consolidated: None,
        consolidated_at: None,
    };
    let node5 = GraphNode::from_pkb_document(&template_doc);
    assert_eq!(node5.tier, "knowledge");

    // 6. Normal task/note -> tier: knowledge
    let task_doc = PkbDocument {
        path: "doc6.md".into(),
        title: "Task".into(),
        doc_type: Some("task".into()),
        status: Some("ready".into()),
        tags: vec![],
        modified: None,
        content_hash: "hash".into(),
        file_hash: "hash".into(),
        frontmatter: Some(serde_json::json!({
            "id": "doc6",
            "title": "Task",
            "type": "task",
            "status": "ready"
        })),
        body: "Task body".into(),
        consolidated: None,
        consolidated_at: None,
    };
    let node6 = GraphNode::from_pkb_document(&task_doc);
    assert_eq!(node6.tier, "knowledge");

    // 7. status: archived overrides log type -> tier: archive
    let archived_log = PkbDocument {
        path: "doc7.md".into(),
        title: "Old Log".into(),
        doc_type: Some("session-log".into()),
        status: Some("archived".into()),
        tags: vec![],
        modified: None,
        content_hash: "hash".into(),
        file_hash: "hash".into(),
        frontmatter: Some(serde_json::json!({
            "id": "doc7",
            "title": "Old Log",
            "type": "session-log",
            "status": "archived"
        })),
        body: "Old body".into(),
        consolidated: None,
        consolidated_at: None,
    };
    let node7 = GraphNode::from_pkb_document(&archived_log);
    assert_eq!(node7.tier, "archive");

    // 8. Frontmatter tier: field is ignored (Decision 6: no parallel tier frontmatter field)
    let fake_tier_doc = PkbDocument {
        path: "doc8.md".into(),
        title: "Fake Tier".into(),
        doc_type: Some("note".into()),
        status: Some("ready".into()),
        tags: vec![],
        modified: None,
        content_hash: "hash".into(),
        file_hash: "hash".into(),
        frontmatter: Some(serde_json::json!({
            "id": "doc8",
            "title": "Fake Tier",
            "type": "note",
            "status": "ready",
            "tier": "archive"
        })),
        body: "Body".into(),
        consolidated: None,
        consolidated_at: None,
    };
    let node8 = GraphNode::from_pkb_document(&fake_tier_doc);
    assert_eq!(node8.tier, "knowledge");
}

#[test]
fn test_pkb_tags_frontmatter_only_and_integer_tags() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // 1. Body hashtags must NOT be scraped into tags
    let file1 = root.join("test1.md");
    fs::write(
        &file1,
        "---\nid: test1\ntitle: Test 1\ntype: note\ntags:\n  - canonical\n---\n\nBody with #inline_hashtag and #42 issue ref.\n",
    )
    .unwrap();

    let doc1 = parse_file_relative(&file1, root).expect("parse file1");
    assert_eq!(doc1.tags, vec!["canonical".to_string()]);

    // 2. Integer tags in frontmatter must be preserved as strings
    let file2 = root.join("test2.md");
    fs::write(
        &file2,
        "---\nid: test2\ntitle: Test 2\ntype: note\ntags:\n  - 42\n  - 101\n  - rust\n---\n\nBody.\n",
    )
    .unwrap();

    let doc2 = parse_file_relative(&file2, root).expect("parse file2");
    let mut tags = doc2.tags.clone();
    tags.sort();
    assert_eq!(tags, vec!["101".to_string(), "42".to_string(), "rust".to_string()]);

    // 3. Scalar integer tag in frontmatter (tags: 123)
    let file3 = root.join("test3.md");
    fs::write(
        &file3,
        "---\nid: test3\ntitle: Test 3\ntype: note\ntags: 123\n---\n\nBody.\n",
    )
    .unwrap();

    let doc3 = parse_file_relative(&file3, root).expect("parse file3");
    assert_eq!(doc3.tags, vec!["123".to_string()]);
}
