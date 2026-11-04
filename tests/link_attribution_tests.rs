//! Acceptance tests for link attribution under Rule R27 & E17 (task `aops_4eb6c609`).
//!
//! Acceptance criteria:
//! - AC1: A link stored with no author does not read back as Nic's in any read view; shown by a test with its red trace.
//! - AC2: Each link-writing route the design names records its author as the design states; shown by a test per route with its red trace.
//! - AC3: The `admin-f630bdb4` to `targ-5b675f05` link no longer reads back as Nic's.
//! - AC4: Full suite green; the PR maps each AC to its evidence.

use mem::graph::{GraphNode, LinkSetBy};
use mem::graph_store::GraphStore;
use mem::lint::{
    lint_file, validate_git_commit_attribution, GitCommitAttributionContext, Severity,
};
use mem::mcp_server::PkbSearchServer;
use mem::pkb::PkbDocument;
use mem::vectordb::VectorStore;
use mem::embeddings::Embedder;
use parking_lot::RwLock;
use rmcp::model::ErrorCode;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn setup_test_pkb() -> (PkbSearchServer, TempDir, Arc<RwLock<GraphStore>>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    fs::create_dir_all(root.join("tasks")).unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();
    fs::create_dir_all(root.join("memories")).unwrap();
    fs::create_dir_all(root.join("notes")).unwrap();

    fs::write(
        root.join("polecat.yaml"),
        "projects:\n  proj-root: {}\n  aops: {}\n",
    )
    .unwrap();

    let parent_path = root.join("projects/proj-root.md");
    fs::write(
        &parent_path,
        "---\n\
         id: proj-root\n\
         title: \"Root Project\"\n\
         type: epic\n\
         status: active\n\
         project: proj-root\n\
         ---\n\n# Root Project\n",
    )
    .unwrap();

    let target_path = root.join("tasks/targ-5b675f05.md");
    fs::write(
        &target_path,
        "---\n\
         id: targ-5b675f05\n\
         title: \"Target 5b675f05\"\n\
         type: target\n\
         status: ready\n\
         parent: proj-root\n\
         project: proj-root\n\
         worth: 1.0\n\
         ---\n\n# Target\n",
    )
    .unwrap();

    let admin_path = root.join("tasks/admin-f630bdb4.md");
    fs::write(
        &admin_path,
        "---\n\
         id: admin-f630bdb4\n\
         title: \"Admin f630bdb4\"\n\
         type: task\n\
         status: ready\n\
         parent: proj-root\n\
         project: proj-root\n\
         links:\n\
           - to: targ-5b675f05\n\
             label: serves\n\
             quantum: 0.5\n\
         ---\n\n# Admin\n",
    )
    .unwrap();

    let docs = mem::pkb::scan_directory(root)
        .iter()
        .filter_map(|p| mem::pkb::parse_file_relative(p, root))
        .collect::<Vec<_>>();

    let graph = Arc::new(RwLock::new(GraphStore::build(&docs, root)));
    let store = Arc::new(RwLock::new(VectorStore::new(3)));
    let embedder = Arc::new(Embedder::new_dummy());
    let db_path = root.join("pkb_vectors.bin");

    let server = PkbSearchServer::new(
        store,
        embedder,
        root.to_path_buf(),
        db_path,
        graph.clone(),
    );

    (server, tmp, graph)
}

fn find_file_by_id(root: &std::path::Path, id: &str) -> PathBuf {
    mem::pkb::scan_directory(root)
        .into_iter()
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(id) && n.ends_with(".md"))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| panic!("File for id {id} not found"))
}

// =========================================================================
// AC1: A link stored with no author does not read back as Nic's
// =========================================================================

#[test]
fn test_ac1_unattributed_link_does_not_read_as_nic_in_graph_node() {
    let doc = PkbDocument {
        path: PathBuf::from("tasks/task-unattributed.md"),
        title: "Unattributed Task".to_string(),
        tags: Vec::new(),
        doc_type: Some("task".to_string()),
        status: Some("ready".to_string()),
        consolidated: None,
        consolidated_at: None,
        modified: None,
        body: String::new(),
        content_hash: String::new(),
        file_hash: String::new(),
        frontmatter: Some(json!({
            "id": "task-unattributed",
            "title": "Unattributed Task",
            "type": "task",
            "status": "ready",
            "links": [
                {
                    "to": "targ-1",
                    "label": "serves",
                    "quantum": 0.5
                }
            ]
        })),
    };

    let node = GraphNode::from_pkb_document(&doc);
    assert_eq!(node.links.len(), 1);
    let link = &node.links[0];
    
    // AC1: Link stored with no author must NOT read back as Nic's
    assert_ne!(
        link.set_by,
        LinkSetBy::Nic,
        "AC1 violation: omitted set_by read back as Nic!"
    );
    assert_eq!(
        link.set_by,
        LinkSetBy::Unattributed,
        "AC1 failure: omitted set_by must be LinkSetBy::Unattributed"
    );
    assert_eq!(
        link.set_by.as_str(),
        "unattributed",
        "AC1 failure: as_str() must be 'unattributed'"
    );
}

#[test]
fn test_ac1_unattributed_link_in_graph_store_and_read_views() {
    let (server, tmp, graph) = setup_test_pkb();

    // 1. Check GraphStore links_out
    let links_out = graph.read().links_out("admin-f630bdb4");
    let link = links_out
        .iter()
        .find(|l| l.to.as_deref() == Some("targ-5b675f05"))
        .expect("admin-f630bdb4 must have outgoing link to targ-5b675f05");
    assert_ne!(
        link.set_by,
        LinkSetBy::Nic,
        "AC1 violation: links_out must NOT attribute omitted set_by to Nic"
    );
    assert_eq!(
        link.set_by,
        LinkSetBy::Unattributed,
        "AC1 failure: links_out must be LinkSetBy::Unattributed"
    );

    // 2. Check GraphStore links_in
    let links_in = graph.read().links_in("targ-5b675f05");
    let in_link = links_in
        .iter()
        .find(|l| l.from.as_deref() == Some("admin-f630bdb4"))
        .expect("targ-5b675f05 must have incoming link from admin-f630bdb4");
    assert_ne!(
        in_link.set_by,
        LinkSetBy::Nic,
        "AC1 violation: links_in must NOT attribute omitted set_by to Nic"
    );
    assert_eq!(
        in_link.set_by,
        LinkSetBy::Unattributed,
        "AC1 failure: links_in must be LinkSetBy::Unattributed"
    );

    // 3. Check MCP get_task read view
    let res = server
        .dispatch_tool_sync("get_task", &json!({"id": "admin-f630bdb4"}))
        .expect("get_task must succeed");
    let json_text = res.content[0].raw.as_text().unwrap().text.as_str();
    let task_val: serde_json::Value = serde_json::from_str(json_text).unwrap();
    let links = task_val.get("links_out").and_then(|v| v.as_array()).unwrap();
    let mcp_link = links
        .iter()
        .find(|l| l.get("to").and_then(|v| v.as_str()) == Some("targ-5b675f05"))
        .expect("link to targ-5b675f05 in get_task");
    let set_by = mcp_link.get("set_by").and_then(|v| v.as_str()).unwrap_or("missing");
    assert_ne!(
        set_by,
        "nic",
        "AC1 violation: get_task read view attributed link to nic!"
    );
    assert_eq!(
        set_by,
        "unattributed",
        "AC1 failure: get_task read view must return 'unattributed'"
    );

    // 4. Check lint emits flow-edge-unattributed style diagnostic
    let file_path = tmp.path().join("tasks/admin-f630bdb4.md");
    let lint_res = lint_file(&file_path, false, None, None, None);
    let unattributed_diag = lint_res
        .diagnostics
        .iter()
        .find(|d| d.rule == "flow-edge-unattributed");
    assert!(
        unattributed_diag.is_some(),
        "AC1 failure: lint must emit flow-edge-unattributed for link without set_by"
    );
    assert_eq!(
        unattributed_diag.unwrap().severity,
        Severity::Style,
        "flow-edge-unattributed must be style severity"
    );
}

// =========================================================================
// AC2: Route 1 - Dashboard allows set_by: nic
// =========================================================================

#[test]
fn test_ac2_route1_dashboard_allows_nic_attribution() {
    let (server, tmp, _graph) = setup_test_pkb();

    // 1. create_task with _client = overwhelm-dashboard
    let res = server.dispatch_tool_sync(
        "create_task",
        &json!({
            "title": "Dashboard Created Task",
            "slug": "dash-task",
            "parent": "proj-root",
            "project": "proj-root",
            "_client": "overwhelm-dashboard",
            "links": [
                {
                    "to": "targ-5b675f05",
                    "label": "serves",
                    "quantum": 0.8,
                    "set_by": "nic"
                }
            ]
        }),
    );
    assert!(
        res.is_ok(),
        "AC2 Route 1: Dashboard create_task with set_by: nic must be accepted, got: {:?}",
        res.err()
    );

    let created_out = res.unwrap();
    let created_text = created_out.content[0].raw.as_text().unwrap().text.as_str();
    let created_val: serde_json::Value = serde_json::from_str(created_text).unwrap();
    let task_id = created_val["id"].as_str().unwrap();

    // Verify written file contains set_by: nic
    let task_file = find_file_by_id(tmp.path(), task_id);
    let content = fs::read_to_string(&task_file).unwrap();
    assert!(
        content.contains("set_by: nic"),
        "Dashboard write must persist set_by: nic. File content:\n{content}"
    );

    // 2. update_task with _client = overwhelm-dashboard
    let update_res = server.dispatch_tool_sync(
        "update_task",
        &json!({
            "id": task_id,
            "_client": "overwhelm-dashboard",
            "links": [
                {
                    "to": "targ-5b675f05",
                    "label": "serves",
                    "quantum": 0.95,
                    "set_by": "nic"
                }
            ]
        }),
    );
    assert!(
        update_res.is_ok(),
        "AC2 Route 1: Dashboard update_task with set_by: nic must be accepted, got: {:?}",
        update_res.err()
    );
    let updated_content = fs::read_to_string(&task_file).unwrap();
    assert!(
        updated_content.contains("set_by: nic"),
        "Dashboard update must persist set_by: nic"
    );
}

// =========================================================================
// AC2: Route 2 - MCP Tools: agent cannot attribute nic & defaults to agent-proposed
// =========================================================================

#[test]
fn test_ac2_route2_agent_cannot_attribute_nic() {
    let (server, _tmp, _graph) = setup_test_pkb();

    // 1. Agent create_task with set_by: nic must fail
    let res_create = server.dispatch_tool_sync(
        "create_task",
        &json!({
            "title": "Agent Created Task",
            "slug": "agent-task",
            "parent": "proj-root",
            "project": "proj-root",
            "links": [
                {
                    "to": "targ-5b675f05",
                    "label": "serves",
                    "quantum": 0.5,
                    "set_by": "nic"
                }
            ]
        }),
    );
    assert!(
        res_create.is_err(),
        "AC2 Route 2 failure: agent create_task with set_by: nic must be rejected!"
    );
    let err = res_create.unwrap_err();
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("Agents cannot attribute links to nic; use set_by: 'agent-proposed' with justification"),
        "Expected error message instructing agent-proposed with justification, got: {}",
        err.message
    );

    // 2. Agent update_task with set_by: nic must fail
    let res_update = server.dispatch_tool_sync(
        "update_task",
        &json!({
            "id": "admin-f630bdb4",
            "links": [
                {
                    "to": "targ-5b675f05",
                    "label": "serves",
                    "quantum": 0.7,
                    "set_by": "nic"
                }
            ]
        }),
    );
    assert!(
        res_update.is_err(),
        "AC2 Route 2 failure: agent update_task with set_by: nic must be rejected!"
    );
    let err_update = res_update.unwrap_err();
    assert_eq!(err_update.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err_update.message.contains("Agents cannot attribute links to nic"),
        "Got: {}", err_update.message
    );

    // 3. Agent create document with set_by: nic must fail
    let res_create_doc = server.dispatch_tool_sync(
        "create",
        &json!({
            "title": "Agent Document",
            "doc_type": "task",
            "parent": "proj-root",
            "project": "proj-root",
            "links": [
                {
                    "to": "targ-5b675f05",
                    "label": "serves",
                    "quantum": 0.5,
                    "set_by": "nic"
                }
            ]
        }),
    );
    assert!(
        res_create_doc.is_err(),
        "AC2 Route 2 failure: agent create with set_by: nic must be rejected!"
    );
    let err_create_doc = res_create_doc.unwrap_err();
    assert_eq!(err_create_doc.code, ErrorCode::INVALID_PARAMS);

    // 4. Agent decompose_task with set_by: nic must fail
    let res_decompose = server.dispatch_tool_sync(
        "decompose_task",
        &json!({
            "parent_id": "proj-root",
            "subtasks": [
                {
                    "title": "Subtask 1",
                    "links": [
                        {
                            "to": "targ-5b675f05",
                            "label": "serves",
                            "quantum": 0.5,
                            "set_by": "nic"
                        }
                    ]
                }
            ]
        }),
    );
    assert!(
        res_decompose.is_err(),
        "AC2 Route 2 failure: agent decompose_task with set_by: nic must be rejected!"
    );
    let err_decompose = res_decompose.unwrap_err();
    assert_eq!(err_decompose.code, ErrorCode::INVALID_PARAMS);

    // 5. Agent batch_update with set_by: nic must fail
    let res_batch = server.dispatch_tool_sync(
        "batch_update",
        &json!({
            "ids": ["admin-f630bdb4"],
            "updates": {
                "links": [
                    {
                        "to": "targ-5b675f05",
                        "label": "serves",
                        "quantum": 0.5,
                        "set_by": "nic"
                    }
                ]
            }
        }),
    );
    assert!(
        res_batch.is_err(),
        "AC2 Route 2 failure: agent batch_update with set_by: nic must be rejected!"
    );
    let err_batch = res_batch.unwrap_err();
    assert_eq!(err_batch.code, ErrorCode::INVALID_PARAMS);
}

#[test]
fn test_ac2_route2_agent_omitted_set_by_defaults_to_proposed() {
    let (server, tmp, _graph) = setup_test_pkb();

    // Agent creates task without set_by in links
    let res = server.dispatch_tool_sync(
        "create_task",
        &json!({
            "title": "Agent Auto Proposed Task",
            "slug": "agent-prop",
            "parent": "proj-root",
            "project": "proj-root",
            "links": [
                {
                    "to": "targ-5b675f05",
                    "label": "serves",
                    "quantum": 0.5,
                    "justification": "Required for AC2 verification"
                }
            ]
        }),
    );
    assert!(
        res.is_ok(),
        "Agent create_task with omitted set_by must succeed, got: {:?}",
        res.err()
    );

    let created_out = res.unwrap();
    let created_text = created_out.content[0].raw.as_text().unwrap().text.as_str();
    let created_val: serde_json::Value = serde_json::from_str(created_text).unwrap();
    let task_id = created_val["id"].as_str().unwrap();

    let task_file = find_file_by_id(tmp.path(), task_id);
    let content = fs::read_to_string(&task_file).unwrap();
    assert!(
        content.contains("set_by: agent-proposed"),
        "Agent omitted set_by must default to agent-proposed. File content:\n{content}"
    );
}

// =========================================================================
// AC2: Route 3 & Route 4 - Git commit attribution validation
// =========================================================================

#[test]
fn test_ac2_route3_and_route4_git_commit_attribution() {
    // Route 3: Nic's direct commit permits set_by: nic
    let nic_ctx = GitCommitAttributionContext {
        author_name: "Nic Suzor",
        author_email: "nic@example.com",
        commit_message: "feat: add link to target",
        env_vars: &[],
    };
    let res_nic = validate_git_commit_attribution(&nic_ctx, &["nic"]);
    assert!(
        res_nic.is_ok(),
        "Route 3: Nic Suzor direct git commit must permit set_by: nic"
    );

    // Route 4: Bot committer cannot write set_by: nic
    let bot_ctx = GitCommitAttributionContext {
        author_name: "botnicbot",
        author_email: "botnicbot@users.noreply.github.com",
        commit_message: "fix: update links",
        env_vars: &[],
    };
    let res_bot = validate_git_commit_attribution(&bot_ctx, &["nic"]);
    assert!(
        res_bot.is_err(),
        "Route 4: Bot committer must be rejected when setting set_by: nic"
    );

    // Route 4: Commit with agent trailers cannot write set_by: nic
    let trailer_ctx = GitCommitAttributionContext {
        author_name: "Nic Suzor",
        author_email: "nic@example.com",
        commit_message: "feat: update task links\n\nTask: aops_4eb6c609\nAgent: polecat",
        env_vars: &[],
    };
    let res_trailer = validate_git_commit_attribution(&trailer_ctx, &["nic"]);
    assert!(
        res_trailer.is_err(),
        "Route 4: Commit message with agent trailers must be rejected when setting set_by: nic"
    );

    // Route 4: Environment with agent vars cannot write set_by: nic
    let env_ctx = GitCommitAttributionContext {
        author_name: "Nic Suzor",
        author_email: "nic@example.com",
        commit_message: "feat: update links",
        env_vars: &[("ANTIGRAVITY_AGENT", "1")],
    };
    let res_env = validate_git_commit_attribution(&env_ctx, &["nic"]);
    assert!(
        res_env.is_err(),
        "Route 4: Environment with ANTIGRAVITY_AGENT must be rejected when setting set_by: nic"
    );

    // Agent commit setting set_by: agent-proposed is allowed
    let valid_agent_ctx = GitCommitAttributionContext {
        author_name: "botnicbot",
        author_email: "botnicbot@users.noreply.github.com",
        commit_message: "feat: propose link\n\nTask: aops_4eb6c609",
        env_vars: &[("ANTIGRAVITY_AGENT", "1")],
    };
    let res_agent_valid = validate_git_commit_attribution(&valid_agent_ctx, &["agent-proposed"]);
    assert!(
        res_agent_valid.is_ok(),
        "Route 4: Agent commit setting agent-proposed must be permitted"
    );
}

// =========================================================================
// AC3: admin-f630bdb4 to targ-5b675f05 link no longer reads back as Nic's
// =========================================================================

#[test]
fn test_ac3_admin_f630bdb4_to_targ_5b675f05_link_not_nic() {
    let (server, tmp, graph) = setup_test_pkb();

    // Verify directly on graph node links
    let doc_path = tmp.path().join("tasks/admin-f630bdb4.md");
    let doc = mem::pkb::parse_file_relative(&doc_path, tmp.path()).expect("parse admin-f630bdb4");
    let node = GraphNode::from_pkb_document(&doc);

    let link = node
        .links
        .iter()
        .find(|l| l.to.as_deref() == Some("targ-5b675f05"))
        .expect("Link from admin-f630bdb4 to targ-5b675f05 must exist");

    // AC3 assertion: MUST NOT read back as Nic's
    assert_ne!(
        link.set_by,
        LinkSetBy::Nic,
        "AC3 VIOLATION: admin-f630bdb4 -> targ-5b675f05 link reads back as Nic's!"
    );
    assert_eq!(
        link.set_by,
        LinkSetBy::Unattributed,
        "AC3: admin-f630bdb4 -> targ-5b675f05 must read as LinkSetBy::Unattributed"
    );

    // Verify on GraphStore
    let out_links = graph.read().links_out("admin-f630bdb4");
    let out_link = out_links
        .iter()
        .find(|l| l.to.as_deref() == Some("targ-5b675f05"))
        .expect("outgoing link must exist in GraphStore");
    assert_ne!(out_link.set_by, LinkSetBy::Nic);
    assert_eq!(out_link.set_by, LinkSetBy::Unattributed);

    // Verify on MCP read tool
    let res = server
        .dispatch_tool_sync("get_task", &json!({"id": "admin-f630bdb4"}))
        .expect("get_task on admin-f630bdb4 must succeed");
    let json_text = res.content[0].raw.as_text().unwrap().text.as_str();
    let task_val: serde_json::Value = serde_json::from_str(json_text).unwrap();
    let links = task_val.get("links_out").and_then(|v| v.as_array()).unwrap();
    let mcp_link = links
        .iter()
        .find(|l| l.get("to").and_then(|v| v.as_str()) == Some("targ-5b675f05"))
        .expect("link to targ-5b675f05 in get_task");
    let set_by = mcp_link.get("set_by").and_then(|v| v.as_str()).unwrap_or("missing");
    assert_ne!(
        set_by,
        "nic",
        "AC3 VIOLATION: get_task returns set_by: 'nic' for admin-f630bdb4 link"
    );
    assert_eq!(set_by, "unattributed");
}
