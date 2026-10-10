//! Integration tests for flow engine §3 (R4–R10, R25, R26) and §7.2 write rules:
//! - T-parse: links_entry_errors_are_isolated (A19)
//! - T-schema: links_schema_mapping (A25, A26)
//! - T-write-reject: write_tools_reject_invalid_links (A31)

use mem::document_crud::{TaskFields, create_task, update_document};
use mem::graph::{
    DeadlineClass, FlowState, GraphNode, LinkLabel, LinkSetBy, NEEDS_QUANTUM, PART_OF_QUANTUM,
};
use mem::graph_store::GraphStore;
use mem::pkb::PkbDocument;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

fn make_doc(id: &str, fm: serde_json::Value) -> PkbDocument {
    let mut fm_obj = fm.as_object().cloned().unwrap_or_default();
    if !fm_obj.contains_key("id") {
        fm_obj.insert("id".to_string(), json!(id));
    }
    if !fm_obj.contains_key("title") {
        fm_obj.insert("title".to_string(), json!("Test Document"));
    }
    if !fm_obj.contains_key("type") {
        fm_obj.insert("type".to_string(), json!("task"));
    }
    let full_fm = serde_json::Value::Object(fm_obj);
    PkbDocument {
        path: PathBuf::from(format!("tasks/{id}.md")),
        title: full_fm
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Test Document")
            .to_string(),
        tags: Vec::new(),
        doc_type: full_fm
            .get("type")
            .and_then(|v| v.as_str())
            .map(String::from),
        status: full_fm
            .get("status")
            .and_then(|v| v.as_str())
            .map(String::from),
        consolidated: None,
        consolidated_at: None,
        modified: Some("2026-10-10T00:00:00Z".to_string()),
        body: String::new(),
        content_hash: String::new(),
        file_hash: String::new(),
        frontmatter: Some(full_fm),
    }
}

/// T-parse: links_entry_errors_are_isolated (A19)
/// `links` with one valid entry, one unknown label, one quantum of 1.5, and one unknown word:
/// 3 entries kept (the 1.5 and the unknown word read at the label's default);
/// the unknown-label entry dropped;
/// 3 `ParseWarning`s, each naming its index and field.
#[test]
fn test_links_entry_errors_are_isolated() {
    let fm = json!({
        "id": "task_iso_01",
        "title": "Isolated Errors Task",
        "type": "task",
        "links": [
            { "to": "targ_valid", "label": "serves", "quantum": 0.6 },
            { "to": "targ_bad_label", "label": "loves" },
            { "to": "targ_bad_quantum", "label": "supports", "quantum": 1.5 },
            { "to": "targ_unknown_word", "label": "needs", "quantum": "enormous" }
        ]
    });

    let doc = make_doc("task_iso_01", fm);
    let node = GraphNode::from_pkb_document(&doc);

    // 3 entries kept (the unknown-label entry is dropped)
    assert_eq!(
        node.links.len(),
        3,
        "Expected 3 links kept, got {}",
        node.links.len()
    );

    // Entry 0 (was index 0): valid
    assert_eq!(node.links[0].to.as_deref(), Some("targ_valid"));
    assert_eq!(node.links[0].label, LinkLabel::Serves);
    assert!((node.links[0].quantum - 0.6).abs() < 1e-9);

    // Entry 1 (was index 2): quantum 1.5 kept at label's default (supports default = 0.0)
    assert_eq!(node.links[1].to.as_deref(), Some("targ_bad_quantum"));
    assert_eq!(node.links[1].label, LinkLabel::Supports);
    assert!((node.links[1].quantum - 0.0).abs() < 1e-9);

    // Entry 2 (was index 3): unknown word kept at label's default (needs default = 1.0)
    assert_eq!(node.links[2].to.as_deref(), Some("targ_unknown_word"));
    assert_eq!(node.links[2].label, LinkLabel::Needs);
    assert!((node.links[2].quantum - 1.0).abs() < 1e-9);

    // 3 ParseWarnings, each naming its index and field
    assert_eq!(
        node.parse_warnings.len(),
        3,
        "Expected 3 parse warnings, got {:#?}",
        node.parse_warnings
    );

    let w0 = &node.parse_warnings[0];
    assert_eq!(
        w0.field, "links[1].label",
        "Warning 0 field should be links[1].label"
    );
    assert!(
        w0.message
            .contains("not in {serves, needs, part_of, supports, alternative, settles}")
    );

    let w1 = &node.parse_warnings[1];
    assert_eq!(
        w1.field, "links[2].quantum",
        "Warning 1 field should be links[2].quantum"
    );
    assert!(w1.message.contains("out of range"));

    let w2 = &node.parse_warnings[2];
    assert_eq!(
        w2.field, "links[3].quantum",
        "Warning 2 field should be links[3].quantum"
    );
    assert!(w2.message.contains("unrecognized quantum word"));
}

/// T-schema: links_schema_mapping (A25, A26)
/// Tests:
/// - Duplicate declared at both ends: agreeing (1 edge, no warning) vs disagreeing (`from` entry wins, warning emitted on other node).
/// - `parent: P` at quantum 0; explicit `part_of` link without quantum at 0.
/// - Map entries `{to, quantum, set_by}` inside `depends_on`, `soft_depends_on`, `contributes_to`, and `parent`.
/// - Explicit `links` entry wins over mapped old key with ParseWarning.
/// - Status to FlowState mapping table (cancelled -> gone, retired/done/missing -> done, active -> open).
/// - `worth` anchor words (10 words), float in [-1.0, 1.0], invalid -> unpriced with warning.
/// - `standing_weight` fallback and precedence warning when both `worth` and `standing_weight` present.
/// - Unclassed `due` is fake; node without `due` has `deadline_class: None`.
/// - `needs` link with no quantum carries 1.0.
/// - `agent-proposed` reads at 0.0 whatever label; `nic` reads at stated quantum; omitted `set_by` reads as `nic`.
#[test]
fn test_links_schema_mapping() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // 1. Both-ends duplicate agreeing: A -> B
    let doc_a = make_doc(
        "node_a",
        json!({
            "id": "node_a",
            "title": "Node A",
            "type": "task",
            "status": "open",
            "links": [
                { "to": "node_b", "label": "serves", "quantum": 0.6, "probability": 0.85, "effect": "helps", "set_by": "nic" }
            ]
        }),
    );
    let doc_b = make_doc(
        "node_b",
        json!({
            "id": "node_b",
            "title": "Node B",
            "type": "target",
            "status": "open",
            "worth": "high", // 0.60
            "links": [
                { "from": "node_a", "label": "serves", "quantum": 0.6, "probability": 0.85, "effect": "helps", "set_by": "nic" }
            ]
        }),
    );

    // 2. Both-ends duplicate disagreeing: C -> D (C declares quantum 0.8, D declares quantum 0.2)
    let doc_c = make_doc(
        "node_c",
        json!({
            "id": "node_c",
            "title": "Node C",
            "type": "task",
            "status": "ready",
            "links": [
                { "to": "node_d", "label": "serves", "quantum": 0.8, "set_by": "nic" }
            ]
        }),
    );
    let doc_d = make_doc(
        "node_d",
        json!({
            "id": "node_d",
            "title": "Node D",
            "type": "target",
            "status": "open",
            "worth": 0.5,
            "links": [
                { "from": "node_c", "label": "serves", "quantum": 0.2, "set_by": "nic" }
            ]
        }),
    );

    // 3. parent: P at quantum 0.0, and map entry in parent:
    let doc_p = make_doc(
        "epic_p",
        json!({
            "id": "epic_p",
            "title": "Epic P",
            "type": "task",
            "status": "in_progress"
        }),
    );
    let doc_child_bare = make_doc(
        "child_bare",
        json!({
            "id": "child_bare",
            "title": "Child Bare Parent",
            "type": "task",
            "status": "inbox",
            "parent": "epic_p"
        }),
    );
    let doc_child_map = make_doc(
        "child_map",
        json!({
            "id": "child_map",
            "title": "Child Map Parent",
            "type": "task",
            "status": "open",
            "parent": { "to": "epic_p", "quantum": 0.4 }
        }),
    );

    // 4. Map entries inside depends_on, soft_depends_on, contributes_to
    let doc_deps = make_doc(
        "task_deps",
        json!({
            "id": "task_deps",
            "title": "Task with mapped old keys",
            "type": "task",
            "status": "open",
            "depends_on": [
                "dep_bare",
                { "to": "dep_map", "quantum": 0.7, "set_by": "agent-proposed" }
            ],
            "soft_depends_on": [
                "soft_bare",
                { "to": "soft_map" } // unstated quantum defaults to 0.3
            ],
            "contributes_to": [
                { "to": "target_contrib_word", "stated_weight": "probable" }, // 0.85
                { "to": "target_contrib_explicit", "quantum": 0.95 }
            ]
        }),
    );

    // 5. Explicit links entry wins over mapped old key with ParseWarning
    let doc_conflict = make_doc(
        "task_conflict",
        json!({
            "id": "task_conflict",
            "title": "Task with link vs old key conflict",
            "type": "task",
            "status": "open",
            "depends_on": [ "dep_overlap" ],
            "links": [
                { "from": "dep_overlap", "label": "needs", "quantum": 0.5 }
            ]
        }),
    );

    // 6. Status to FlowState cases
    let doc_cancelled = make_doc(
        "task_canc",
        json!({ "id": "task_canc", "status": "cancelled" }),
    );
    let doc_retired = make_doc("task_ret", json!({ "id": "task_ret", "status": "retired" }));
    let doc_done = make_doc("task_done", json!({ "id": "task_done", "status": "done" }));
    let doc_missing_status = make_doc("task_nostat", json!({ "id": "task_nostat" }));
    let doc_paused = make_doc(
        "task_paused",
        json!({ "id": "task_paused", "status": "paused" }),
    );
    let doc_blocked = make_doc(
        "task_blocked",
        json!({ "id": "task_blocked", "status": "blocked" }),
    );
    let doc_partial = make_doc(
        "task_partial",
        json!({ "id": "task_partial", "status": "partial" }),
    );
    let doc_review = make_doc(
        "task_review",
        json!({ "id": "task_review", "status": "review" }),
    );

    // 7. Worth cases (anchor words, floats, invalid, standing_weight precedence)
    let doc_w_anchor = make_doc(
        "targ_anchor",
        json!({ "id": "targ_anchor", "worth": "critical" }),
    );
    let doc_w_loss = make_doc(
        "targ_loss",
        json!({ "id": "targ_loss", "worth": "catastrophic loss" }),
    );
    let doc_w_invalid = make_doc(
        "targ_invalid",
        json!({ "id": "targ_invalid", "worth": 1.5 }),
    );
    let doc_w_sw_both = make_doc(
        "targ_both",
        json!({ "id": "targ_both", "worth": 0.4, "standing_weight": 0.8 }),
    );
    let doc_w_sw_only = make_doc(
        "targ_sw_only",
        json!({ "id": "targ_sw_only", "standing_weight": 0.7 }),
    );

    // 8. Deadline class cases: due without class -> fake; due with class; no due -> None
    let doc_due_unclassed = make_doc(
        "task_due_u",
        json!({ "id": "task_due_u", "due": "2026-10-15" }),
    );
    let doc_due_hard = make_doc(
        "task_due_h",
        json!({ "id": "task_due_h", "due": "2026-10-15", "deadline_class": "hard" }),
    );
    let doc_no_due = make_doc("task_no_due", json!({ "id": "task_no_due" }));

    // 9. R26 Proposals: agent-proposed reads at 0.0 whatever label; nic reads at stated; omitted reads as nic
    let doc_proposals = make_doc(
        "task_props",
        json!({
            "id": "task_props",
            "title": "Proposals Test",
            "type": "task",
            "links": [
                { "to": "targ_ap_needs", "label": "needs", "quantum": 0.8, "set_by": "agent-proposed" },
                { "to": "targ_ap_serves", "label": "serves", "quantum": 0.9, "set_by": "agent-proposed" },
                { "to": "targ_nic_serves", "label": "serves", "quantum": 0.7, "set_by": "nic" },
                { "to": "targ_omitted_set_by", "label": "serves", "quantum": 0.65 },
                { "from": "dep_needs_no_q", "label": "needs" } // needs with no quantum -> 1.0
            ]
        }),
    );

    let docs = vec![
        doc_a,
        doc_b,
        doc_c,
        doc_d,
        doc_p,
        doc_child_bare,
        doc_child_map,
        doc_deps,
        doc_conflict,
        doc_cancelled,
        doc_retired,
        doc_done,
        doc_missing_status,
        doc_paused,
        doc_blocked,
        doc_partial,
        doc_review,
        doc_w_anchor,
        doc_w_loss,
        doc_w_invalid,
        doc_w_sw_both,
        doc_w_sw_only,
        doc_due_unclassed,
        doc_due_hard,
        doc_no_due,
        doc_proposals,
    ];

    let store = GraphStore::build(&docs, root);

    // Verify 1: Both-ends duplicate agreeing: Node A -> Node B
    let edges_a_b: Vec<_> = store
        .resolve_flow_edges()
        .into_iter()
        .filter(|e| e.src == "node_a" && e.dst == "node_b")
        .collect();
    assert_eq!(
        edges_a_b.len(),
        1,
        "Agreeing duplicate must yield exactly 1 edge"
    );
    assert_eq!(edges_a_b[0].quantum, 0.6);
    let node_a = store.get_node("node_a").unwrap();
    let node_b = store.get_node("node_b").unwrap();
    assert!(
        !node_a
            .parse_warnings
            .iter()
            .any(|w| w.message.contains("disagreement"))
    );
    assert!(
        !node_b
            .parse_warnings
            .iter()
            .any(|w| w.message.contains("disagreement"))
    );

    // Verify 2: Both-ends duplicate disagreeing: Node C -> Node D
    // C declares 0.8, D declares 0.2. From-node C wins!
    let edges_c_d: Vec<_> = store
        .resolve_flow_edges()
        .into_iter()
        .filter(|e| e.src == "node_c" && e.dst == "node_d")
        .collect();
    assert_eq!(
        edges_c_d.len(),
        1,
        "Disagreeing duplicate must yield exactly 1 edge"
    );
    assert!(
        (edges_c_d[0].quantum - 0.8).abs() < 1e-9,
        "From-node C entry must win (quantum 0.8)"
    );
    let node_d = store.get_node("node_d").unwrap();
    assert!(
        node_d
            .parse_warnings
            .iter()
            .any(|w| w.message.contains("disagreement") && w.message.contains("node_c")),
        "Node D must have a duplicate disagreement ParseWarning naming node_c"
    );

    // Verify 3: parent: P
    let node_cb = store.get_node("child_bare").unwrap();
    let part_of_link = node_cb
        .links
        .iter()
        .find(|l| l.label == LinkLabel::PartOf)
        .unwrap();
    assert_eq!(part_of_link.to.as_deref(), Some("epic_p"));
    assert_eq!(part_of_link.quantum, PART_OF_QUANTUM);
    assert_eq!(part_of_link.set_by, LinkSetBy::Migrated);

    let node_cm = store.get_node("child_map").unwrap();
    let part_of_map = node_cm
        .links
        .iter()
        .find(|l| l.label == LinkLabel::PartOf)
        .unwrap();
    assert_eq!(part_of_map.to.as_deref(), Some("epic_p"));
    assert_eq!(part_of_map.quantum, 0.4);

    // Verify 4: Map entries inside old keys
    let node_deps = store.get_node("task_deps").unwrap();
    // depends_on: bare string -> quantum 1.0, migrated
    let dep_bare_link = node_deps
        .links
        .iter()
        .find(|l| l.from.as_deref() == Some("dep_bare"))
        .unwrap();
    assert_eq!(dep_bare_link.label, LinkLabel::Needs);
    assert_eq!(dep_bare_link.quantum, NEEDS_QUANTUM);
    assert_eq!(dep_bare_link.set_by, LinkSetBy::Migrated);
    // depends_on: map entry -> quantum 0.7, agent-proposed
    let dep_map_link = node_deps
        .links
        .iter()
        .find(|l| l.from.as_deref() == Some("dep_map"))
        .unwrap();
    assert_eq!(dep_map_link.label, LinkLabel::Needs);
    assert_eq!(dep_map_link.set_by, LinkSetBy::AgentProposed);
    assert_eq!(dep_map_link.quantum, 0.0); // agent-proposed reads at 0.0!

    // soft_depends_on: bare string -> supports, quantum 0.3
    let soft_bare_link = node_deps
        .links
        .iter()
        .find(|l| l.from.as_deref() == Some("soft_bare"))
        .unwrap();
    assert_eq!(soft_bare_link.label, LinkLabel::Supports);
    assert!((soft_bare_link.quantum - 0.3).abs() < 1e-9);

    // soft_depends_on: map entry with unstated quantum -> supports, quantum 0.3
    let soft_map_link = node_deps
        .links
        .iter()
        .find(|l| l.from.as_deref() == Some("soft_map"))
        .unwrap();
    assert_eq!(soft_map_link.label, LinkLabel::Supports);
    assert!((soft_map_link.quantum - 0.3).abs() < 1e-9);

    // contributes_to: word "probable" -> 0.85
    let ct_word_link = node_deps
        .links
        .iter()
        .find(|l| l.to.as_deref() == Some("target_contrib_word"))
        .unwrap();
    assert_eq!(ct_word_link.label, LinkLabel::Serves);
    assert!((ct_word_link.quantum - 0.85).abs() < 1e-9);

    // contributes_to: explicit quantum 0.95
    let ct_exp_link = node_deps
        .links
        .iter()
        .find(|l| l.to.as_deref() == Some("target_contrib_explicit"))
        .unwrap();
    assert_eq!(ct_exp_link.label, LinkLabel::Serves);
    assert!((ct_exp_link.quantum - 0.95).abs() < 1e-9);

    // Verify 5: Conflict precedence: explicit links wins over mapped old key
    let node_conf = store.get_node("task_conflict").unwrap();
    let overlap_links: Vec<_> = node_conf
        .links
        .iter()
        .filter(|l| l.from.as_deref() == Some("dep_overlap"))
        .collect();
    assert_eq!(
        overlap_links.len(),
        1,
        "Only 1 link for dep_overlap should exist on node"
    );
    assert_eq!(
        overlap_links[0].quantum, 0.5,
        "Explicit link quantum 0.5 must win over depends_on 1.0"
    );
    assert!(
        node_conf
            .parse_warnings
            .iter()
            .any(|w| w.field == "depends_on" && w.message.contains("links entry wins"))
    );

    // Verify 6: Status to FlowState
    assert_eq!(
        store.get_node("task_canc").unwrap().flow_state(),
        FlowState::Gone
    );
    assert_eq!(
        store.get_node("task_ret").unwrap().flow_state(),
        FlowState::Done
    );
    assert_eq!(
        store.get_node("task_done").unwrap().flow_state(),
        FlowState::Done
    );
    assert_eq!(
        store.get_node("task_nostat").unwrap().flow_state(),
        FlowState::Done
    );
    assert_eq!(
        store.get_node("task_paused").unwrap().flow_state(),
        FlowState::Open
    );
    assert_eq!(
        store.get_node("task_blocked").unwrap().flow_state(),
        FlowState::Open
    );
    assert_eq!(
        store.get_node("task_partial").unwrap().flow_state(),
        FlowState::Open
    );
    assert_eq!(
        store.get_node("task_review").unwrap().flow_state(),
        FlowState::Open
    );

    // Verify 7: Worth
    assert_eq!(store.get_node("targ_anchor").unwrap().worth, Some(1.00));
    assert_eq!(store.get_node("targ_loss").unwrap().worth, Some(-1.00));
    let n_inv = store.get_node("targ_invalid").unwrap();
    assert_eq!(n_inv.worth, None, "1.5 worth must be read as unpriced None");
    assert!(n_inv.parse_warnings.iter().any(|w| w.field == "worth"));

    let n_both = store.get_node("targ_both").unwrap();
    assert_eq!(
        n_both.worth,
        Some(0.4),
        "worth (0.4) must win over standing_weight (0.8)"
    );
    assert!(
        n_both
            .parse_warnings
            .iter()
            .any(|w| w.field == "worth" && w.message.contains("both worth and standing_weight"))
    );

    let n_sw = store.get_node("targ_sw_only").unwrap();
    assert_eq!(
        n_sw.worth,
        Some(0.7),
        "standing_weight only must be read as worth"
    );

    // Verify 8: Deadline class
    assert_eq!(
        store.get_node("task_due_u").unwrap().deadline_class,
        Some(DeadlineClass::Fake)
    );
    assert_eq!(
        store.get_node("task_due_h").unwrap().deadline_class,
        Some(DeadlineClass::Hard)
    );
    assert_eq!(store.get_node("task_no_due").unwrap().deadline_class, None);

    // Verify 9: Proposals and needs defaults
    let n_props = store.get_node("task_props").unwrap();
    let l_ap_needs = n_props
        .links
        .iter()
        .find(|l| l.to.as_deref() == Some("targ_ap_needs"))
        .unwrap();
    assert_eq!(
        l_ap_needs.quantum, 0.0,
        "agent-proposed needs must read at 0.0"
    );
    assert_eq!(l_ap_needs.strength(), 0.0);

    let l_ap_serves = n_props
        .links
        .iter()
        .find(|l| l.to.as_deref() == Some("targ_ap_serves"))
        .unwrap();
    assert_eq!(
        l_ap_serves.quantum, 0.0,
        "agent-proposed serves must read at 0.0"
    );

    let l_nic = n_props
        .links
        .iter()
        .find(|l| l.to.as_deref() == Some("targ_nic_serves"))
        .unwrap();
    assert_eq!(
        l_nic.quantum, 0.7,
        "nic serves must read at stated quantum 0.7"
    );
    assert_eq!(l_nic.set_by, LinkSetBy::Nic);

    let l_omitted = n_props
        .links
        .iter()
        .find(|l| l.to.as_deref() == Some("targ_omitted_set_by"))
        .unwrap();
    assert_eq!(
        l_omitted.set_by,
        LinkSetBy::Nic,
        "omitted set_by defaults to nic"
    );
    assert_eq!(l_omitted.quantum, 0.65);

    let l_needs_no_q = n_props
        .links
        .iter()
        .find(|l| l.from.as_deref() == Some("dep_needs_no_q"))
        .unwrap();
    assert_eq!(l_needs_no_q.label, LinkLabel::Needs);
    assert_eq!(
        l_needs_no_q.quantum, 1.0,
        "needs link with no quantum carries 1.0"
    );

    // Verify links_out and links_in on GraphStore
    let links_out_a = store.links_out("node_a");
    assert_eq!(links_out_a.len(), 1);
    assert_eq!(links_out_a[0].to.as_deref(), Some("node_b"));
    assert_eq!(links_out_a[0].from, None);
    assert!((links_out_a[0].strength - 0.6 * 0.85).abs() < 1e-9);

    let links_in_b = store.links_in("node_b");
    assert_eq!(links_in_b.len(), 1);
    assert_eq!(links_in_b[0].from.as_deref(), Some("node_a"));
    assert_eq!(links_in_b[0].to, None);
}

/// T-write-reject: write_tools_reject_invalid_links (A31)
/// `create_task` and `update_task` with an unknown label, quantum 1.5, `worth: 2`, `deadline_class: maybe`:
/// each returns an error; no file changes.
#[test]
fn test_write_tools_reject_invalid_links() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // 1. create_task with unknown label in links
    let res1 = create_task(
        root,
        TaskFields {
            title: "Bad Label Task".to_string(),
            links: vec![json!({ "to": "targ_1", "label": "bogus_label" })],
            ..Default::default()
        },
    );
    assert!(res1.is_err(), "create_task with unknown label must fail");
    let err1 = res1.unwrap_err().to_string();
    assert!(
        err1.contains("bogus_label"),
        "Error message must name the invalid label: {err1}"
    );

    // 2. create_task with quantum 1.5
    let res2 = create_task(
        root,
        TaskFields {
            title: "Bad Quantum Task".to_string(),
            links: vec![json!({ "to": "targ_1", "label": "serves", "quantum": 1.5 })],
            ..Default::default()
        },
    );
    assert!(res2.is_err(), "create_task with quantum 1.5 must fail");
    let err2 = res2.unwrap_err().to_string();
    assert!(
        err2.contains("out of range"),
        "Error message must describe out of range: {err2}"
    );

    // 3. create_task with worth: 2
    let res3 = create_task(
        root,
        TaskFields {
            title: "Bad Worth Task".to_string(),
            worth: Some(json!(2)),
            ..Default::default()
        },
    );
    assert!(res3.is_err(), "create_task with worth: 2 must fail");
    let err3 = res3.unwrap_err().to_string();
    assert!(
        err3.contains("out of range"),
        "Error message must describe worth out of range: {err3}"
    );

    // 4. create_task with deadline_class: maybe
    let res4 = create_task(
        root,
        TaskFields {
            title: "Bad Deadline Class Task".to_string(),
            deadline_class: Some("maybe".to_string()),
            ..Default::default()
        },
    );
    assert!(
        res4.is_err(),
        "create_task with deadline_class 'maybe' must fail"
    );
    let err4 = res4.unwrap_err().to_string();
    assert!(
        err4.contains("maybe"),
        "Error message must describe invalid deadline_class: {err4}"
    );

    // Verify no files were created by the failed create_task attempts
    let entries: Vec<_> = fs::read_dir(root).unwrap().collect();
    // At most empty or only subdirectories created before validation, but no tasks created
    for entry in entries {
        let p = entry.unwrap().path();
        if p.is_dir() {
            let sub_files: Vec<_> = fs::read_dir(&p).unwrap().collect();
            assert!(
                sub_files.is_empty(),
                "Directory {:?} should contain no files after failed creation",
                p
            );
        }
    }

    // Now create a valid task so we can test update_document / update_task rejections
    let valid_path = create_task(
        root,
        TaskFields {
            title: "Valid Task For Update".to_string(),
            id: Some("task_valid_01".to_string()),
            ..Default::default()
        },
    )
    .unwrap();

    let initial_content = fs::read_to_string(&valid_path).unwrap();

    // 5. update_document with unknown label in links
    let mut updates_bad_label = HashMap::new();
    updates_bad_label.insert(
        "links".to_string(),
        json!([
            { "to": "targ_1", "label": "bogus_label" }
        ]),
    );
    let res5 = update_document(&valid_path, updates_bad_label);
    assert!(
        res5.is_err(),
        "update_document with invalid label must fail"
    );
    assert_eq!(
        fs::read_to_string(&valid_path).unwrap(),
        initial_content,
        "File content must not change on error"
    );

    // 6. update_document with quantum 1.5
    let mut updates_bad_q = HashMap::new();
    updates_bad_q.insert(
        "links".to_string(),
        json!([
            { "to": "targ_1", "label": "serves", "quantum": 1.5 }
        ]),
    );
    let res6 = update_document(&valid_path, updates_bad_q);
    assert!(res6.is_err(), "update_document with quantum 1.5 must fail");
    assert_eq!(
        fs::read_to_string(&valid_path).unwrap(),
        initial_content,
        "File content must not change on error"
    );

    // 7. update_document with worth: 2
    let mut updates_bad_w = HashMap::new();
    updates_bad_w.insert("worth".to_string(), json!(2));
    let res7 = update_document(&valid_path, updates_bad_w);
    assert!(res7.is_err(), "update_document with worth 2 must fail");
    assert_eq!(
        fs::read_to_string(&valid_path).unwrap(),
        initial_content,
        "File content must not change on error"
    );

    // 8. update_document with deadline_class: maybe
    let mut updates_bad_dc = HashMap::new();
    updates_bad_dc.insert("deadline_class".to_string(), json!("maybe"));
    let res8 = update_document(&valid_path, updates_bad_dc);
    assert!(
        res8.is_err(),
        "update_document with deadline_class 'maybe' must fail"
    );
    assert_eq!(
        fs::read_to_string(&valid_path).unwrap(),
        initial_content,
        "File content must not change on error"
    );
}
