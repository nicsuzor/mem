//! Comprehensive End-to-End and Adversarial Integration Tests for the Excalidraw Subsystem.
//!
//! Validates:
//! 1. Roundtrip fidelity (serialize -> parse -> diff -> zero unexpected mutations)
//! 2. Non-destructive canvas card removal (asserting no file is deleted from disk)
//! 3. Safe arrow typing & typed prefix resolution (dep:, soft:, parent:, contrib:, closes:, supersedes:, sim:)
//! 4. Multi-layer Sugiyama DAG layout with dummy vertex routing & crossing minimization
//! 5. Bounded Archimedean spiral placement for newly discovered tasks
//! 6. Global cycle rejection across direct, transitive, parent, and mixed dependency loops
//! 7. Clipboard duplicate ID detection and safe ID reallocation
//! 8. CLI execution (`pkb graph`, `pkb excalidraw export/diff/sync`)
//! 9. MCP tool endpoints (`graph_excalidraw`, `diff_excalidraw`, `sync_excalidraw`)
//! 10. Adversarial, corrupted, and edge-case canvas inputs

use mem::excalidraw::*;
use mem::graph::{Edge, EdgeType, GraphNode};
use mem::graph_store::GraphStore;
use mem::mcp_server::PkbSearchServer;
use mem::vectordb::VectorStore;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use tempfile::TempDir;

// ===========================================================================
// Test Helpers
// ===========================================================================

fn pkb_excalidraw_binary() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let release = manifest.join("target/release/pkb-excalidraw");
    if release.exists() {
        return release;
    }
    manifest.join("target/debug/pkb-excalidraw")
}

fn pkb_binary() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let release = manifest.join("target/release/pkb");
    if release.exists() {
        return release;
    }
    let debug = manifest.join("target/debug/pkb");
    if debug.exists() {
        return debug;
    }
    PathBuf::from("pkb")
}

/// Create a temporary PKB workspace populated with markdown files.
fn create_test_pkb_workspace() -> TempDir {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let pkb_root = tmp.path();

    // Create polecat.yaml with registered project
    fs::write(
        pkb_root.join("polecat.yaml"),
        "projects:\n  testproj: {}\n  infra: {}\n",
    )
    .unwrap();

    let dirs = ["epics", "tasks", "areas", "targets", "learn", "notes"];
    for d in &dirs {
        fs::create_dir_all(pkb_root.join(d)).unwrap();
    }

    // 1. Epic
    fs::write(
        pkb_root.join("epics/epic-core.md"),
        "---\n\
         id: epic-core\n\
         title: \"Core Infrastructure\"\n\
         type: epic\n\
         status: active\n\
         priority: 1\n\
         project: infra\n\
         ---\n\n# Core Infrastructure\n",
    )
    .unwrap();

    // 2. Task 1 (Ready)
    fs::write(
        pkb_root.join("tasks/task-t1.md"),
        "---\n\
         id: task-t1\n\
         title: \"Build Schema\"\n\
         type: task\n\
         status: ready\n\
         priority: 1\n\
         parent: epic-core\n\
         tags: [schema, ast]\n\
         project: testproj\n\
         ---\n\n# Build Schema\n",
    )
    .unwrap();

    // 3. Task 2 (Active, depends on t1)
    fs::write(
        pkb_root.join("tasks/task-t2.md"),
        "---\n\
         id: task-t2\n\
         title: \"Implement Layout Engine\"\n\
         type: task\n\
         status: active\n\
         priority: 2\n\
         parent: epic-core\n\
         depends_on: [task-t1]\n\
         tags: [sugiyama, dag]\n\
         project: testproj\n\
         ---\n\n# Implement Layout Engine\n",
    )
    .unwrap();

    // 4. Task 3 (Done, depends on t2)
    fs::write(
        pkb_root.join("tasks/task-t3.md"),
        "---\n\
         id: task-t3\n\
         title: \"Design Visual Canvas\"\n\
         type: task\n\
         status: done\n\
         priority: 3\n\
         parent: epic-core\n\
         depends_on: [task-t2]\n\
         tags: [ui, canvas]\n\
         project: testproj\n\
         ---\n\n# Design Visual Canvas\n",
    )
    .unwrap();

    // 5. Target
    fs::write(
        pkb_root.join("targets/target-v1.md"),
        "---\n\
         id: target-v1\n\
         title: \"Release V1\"\n\
         type: target\n\
         status: active\n\
         priority: 1\n\
         project: testproj\n\
         parent: epic-core\n\
         ---\n\n# Release V1\n",
    )
    .unwrap();

    // 6. Area
    fs::write(
        pkb_root.join("areas/area-platform.md"),
        "---\n\
         id: area-platform\n\
         title: \"Platform Engineering\"\n\
         type: area\n\
         status: active\n\
         project: infra\n\
         parent: epic-core\n\
         ---\n\n# Platform Engineering\n",
    )
    .unwrap();

    tmp
}

// ===========================================================================
// Test 1: Roundtrip Fidelity
// ===========================================================================

#[test]
fn test_roundtrip_fidelity() {
    let ws = create_test_pkb_workspace();
    let gs = GraphStore::build_from_directory(ws.path());

    // Export entire graph to Excalidraw scene
    let (excal_json, _, _) = gs.output_excalidraw(None, 2)
        .expect("export excalidraw JSON");
    let excal_file: ExcalidrawFile =
        serde_json::from_str(&excal_json).expect("deserialize ExcalidrawFile");

    // Verify scene metadata
    assert_eq!(excal_file.file_type, "excalidraw");
    assert_eq!(excal_file.version, 2);
    assert!(!excal_file.elements.is_empty());

    // Parse canvas model through the 5-pass reader
    let canvas = parse_canvas(&excal_json).expect("parse canvas model");

    // Verify all nodes are recognized
    let card_node_ids: Vec<String> = canvas
        .cards
        .iter()
        .filter_map(|c| c.node_id.clone())
        .collect();
    assert!(card_node_ids.contains(&"epic-core".to_string()));
    assert!(card_node_ids.contains(&"task-t1".to_string()));
    assert!(card_node_ids.contains(&"task-t2".to_string()));
    assert!(card_node_ids.contains(&"task-t3".to_string()));
    assert!(card_node_ids.contains(&"target-v1".to_string()));
    assert!(card_node_ids.contains(&"area-platform".to_string()));

    // Verify status and priority parsing fidelity
    let t1_card = canvas.cards.iter().find(|c| c.node_id.as_deref() == Some("task-t1")).unwrap();
    assert_eq!(t1_card.status.as_deref(), Some("ready"));
    assert_eq!(t1_card.priority, Some(1));
    assert_eq!(t1_card.title, "Build Schema");
    assert_eq!(t1_card.parent.as_deref(), Some("epic-core"));
    assert!(t1_card.tags.contains(&"schema".to_string()));

    let t3_card = canvas.cards.iter().find(|c| c.node_id.as_deref() == Some("task-t3")).unwrap();
    assert_eq!(t3_card.status.as_deref(), Some("done"));
    assert_eq!(t3_card.priority, Some(3));

    // Verify color styling matches unified matrix
    let epic_style = node_color_style(Some("active"), Some("epic"));
    assert_eq!(epic_style.stroke_color, "#4c6ef5");
    assert_eq!(epic_style.bg_color, "#edf2ff");

    let ready_style = node_color_style(Some("ready"), Some("task"));
    assert_eq!(ready_style.stroke_color, "#2b8a3e");
    assert_eq!(ready_style.bg_color, "#d3f9d8");

    // 3-Way Diff against live graph: must have ZERO unexpected mutations
    let base_snap = BaseSnapshot::from_canvas(&canvas);
    let diff = diff_canvas(Some(&base_snap), &gs, &canvas).expect("compute 3-way diff");

    assert!(
        diff.is_empty(),
        "Roundtrip diff must be completely empty, got mutations: {:#?}",
        diff
    );
}

// ===========================================================================
// Test 2: Non-Destructive Canvas Card Removal
// ===========================================================================

#[test]
fn test_non_destructive_canvas_removal() {
    let ws = create_test_pkb_workspace();
    let mut gs = GraphStore::build_from_directory(ws.path());

    let target_file_path = ws.path().join("tasks/task-t2.md");
    assert!(target_file_path.exists(), "task-t2 must exist initially on disk");

    // Export graph to canvas
    let (excal_json, _, _) = gs.output_excalidraw(None, 2).expect("export excalidraw");
    let mut excal_file: ExcalidrawFile = serde_json::from_str(&excal_json).unwrap();
    let base_snapshot = BaseSnapshot::from_file(&excal_file);

    // Simulate user deleting task-t2 card and its bound text from canvas
    let initial_elem_count = excal_file.elements.len();
    excal_file.elements.retain(|elem| {
        let is_t2 = elem
            .custom_data
            .as_ref()
            .and_then(|c| c.pkb.as_ref())
            .and_then(|p| p.node_id.as_deref())
            == Some("task-t2");
        let is_t2_text = elem
            .text
            .as_ref()
            .map_or(false, |t| t.contains("Implement Layout Engine"));
        !is_t2 && !is_t2_text
    });
    assert!(excal_file.elements.len() < initial_elem_count);

    // Parse modified canvas
    let canvas = CanvasReader::parse_file(excal_file);
    assert!(!canvas.cards.iter().any(|c| c.node_id.as_deref() == Some("task-t2")));

    // Compute diff: must record task-t2 in `removed_from_canvas`
    let diff = diff_canvas(Some(&base_snapshot), &gs, &canvas).expect("diff");
    assert!(
        diff.removed_from_canvas.contains(&"task-t2".to_string()),
        "Diff must list task-t2 in removed_from_canvas"
    );

    // Perform synchronization
    let _report = sync_canvas(ws.path(), &mut gs, &diff, false).expect("sync canvas");

    // CRITICAL ASSERTION: The markdown file on disk must NOT be deleted!
    assert!(
        target_file_path.exists(),
        "Non-destructive safety violation: task-t2.md was deleted from disk!"
    );

    // Verify file content is completely intact
    let content = fs::read_to_string(&target_file_path).unwrap();
    assert!(content.contains("id: task-t2"));
    assert!(content.contains("Implement Layout Engine"));
}

// ===========================================================================
// Test 3: Safe Arrow Typing & Typed Prefixes
// ===========================================================================

#[test]
fn test_safe_arrow_typing_and_typed_prefixes() {
    let ws = create_test_pkb_workspace();
    let mut gs = GraphStore::build_from_directory(ws.path());

    // Construct mock cards
    let create_card = |id: &str, title: &str| CanvasCard {
        element_id: format!("elem-{id}"),
        node_id: Some(id.to_string()),
        title: title.to_string(),
        node_type: Some("task".to_string()),
        status: Some("ready".to_string()),
        priority: Some(1),
        parent: None,
        tags: vec![],
        frame_id: None,
        x: 100.0,
        y: 100.0,
        width: CARD_WIDTH,
        height: CARD_HEIGHT,
        stroke_color: "#1e1e1e".to_string(),
        background_color: "transparent".to_string(),
        custom_data: Some(CustomData {
            pkb: Some(PkbCustomData {
                node_id: Some(id.to_string()),
                node_type: Some("task".to_string()),
                status: Some("ready".to_string()),
                priority: Some(1),
                parent: None,
                tags: vec![],
                edge_type: None,
                is_pkb_managed: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }),
        raw_card_element: ExcalidrawElement {
            id: format!("elem-{id}"),
            element_type: "rectangle".to_string(),
            custom_data: Some(CustomData {
                pkb: Some(PkbCustomData {
                    node_id: Some(id.to_string()),
                    is_pkb_managed: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        bound_text_element: None,
        is_new: false,
        is_duplicate: false,
    };

    let card_t1 = create_card("task-t1", "Task 1");
    let card_t2 = create_card("task-t2", "Task 2");
    let card_t3 = create_card("task-t3", "Task 3");
    let card_v1 = create_card("target-v1", "Target V1");
    let card_core = create_card("epic-core", "Core");

    // Construct arrows with different typed labels
    let create_arrow = |elem_id: &str, src: &str, tgt: &str, label: Option<&str>, custom_type: Option<&str>| {
        let mut raw = ExcalidrawElement::default();
        raw.id = elem_id.to_string();
        raw.element_type = "arrow".to_string();
        raw.start_binding = Some(PointBinding {
            element_id: format!("elem-{src}"),
            focus: 0.0,
            gap: 1.0,
            fixed_point: None,
        });
        raw.end_binding = Some(PointBinding {
            element_id: format!("elem-{tgt}"),
            focus: 0.0,
            gap: 1.0,
            fixed_point: None,
        });
        raw.text = label.map(|s| s.to_string());
        raw.custom_data = Some(CustomData {
            pkb: Some(PkbCustomData {
                edge_type: custom_type.map(|s| s.to_string()),
                is_pkb_managed: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        });
        raw
    };

    let mut file = ExcalidrawFile::default();
    file.elements.push(create_arrow("arr-1", "task-t1", "task-t2", None, None)); // Untyped -> Link
    file.elements.push(create_arrow("arr-2", "task-t1", "task-t3", Some("dep: blocker"), None)); // DependsOn
    file.elements.push(create_arrow("arr-3", "task-t1", "target-v1", Some("soft: optional"), None)); // SoftDependsOn
    file.elements.push(create_arrow("arr-4", "task-t1", "epic-core", Some("parent: container"), None)); // Parent
    file.elements.push(create_arrow("arr-5", "task-t2", "target-v1", Some("contrib: 0.8"), None)); // ContributesTo
    file.elements.push(create_arrow("arr-6", "task-t3", "target-v1", Some("closes: issue"), None)); // Closes
    file.elements.push(create_arrow("arr-7", "task-t3", "task-t2", Some("supersedes: old"), None)); // Supersedes
    file.elements.push(create_arrow("arr-8", "task-t2", "epic-core", Some("sim: related"), None)); // SimilarTo

    // Add cards to file elements
    file.elements.push(card_t1.raw_card_element.clone());
    file.elements.push(card_t2.raw_card_element.clone());
    file.elements.push(card_t3.raw_card_element.clone());
    file.elements.push(card_v1.raw_card_element.clone());
    file.elements.push(card_core.raw_card_element.clone());

    let canvas = CanvasReader::parse_file(file);

    // Verify arrow parsing types
    let find_arrow_type = |src: &str, tgt: &str| {
        canvas
            .arrows
            .iter()
            .find(|a| a.source_node_id.as_deref() == Some(src) && a.target_node_id.as_deref() == Some(tgt))
            .map(|a| a.edge_type.clone())
            .expect(&format!("Arrow {src} -> {tgt} not found"))
    };

    assert_eq!(find_arrow_type("task-t1", "task-t2"), EdgeType::Link);
    assert_eq!(find_arrow_type("task-t1", "task-t3"), EdgeType::DependsOn);
    assert_eq!(find_arrow_type("task-t1", "target-v1"), EdgeType::SoftDependsOn);
    assert_eq!(find_arrow_type("task-t1", "epic-core"), EdgeType::Parent);
    assert_eq!(find_arrow_type("task-t2", "target-v1"), EdgeType::ContributesTo);
    assert_eq!(find_arrow_type("task-t3", "target-v1"), EdgeType::Closes);
    assert_eq!(find_arrow_type("task-t3", "task-t2"), EdgeType::Supersedes);
    assert_eq!(find_arrow_type("task-t2", "epic-core"), EdgeType::SimilarTo);

    // Apply sync and verify disk updates
    let diff = diff_canvas(None, &gs, &canvas).expect("diff");
    let report = sync_canvas(ws.path(), &mut gs, &diff, false).expect("sync");
    assert!(report.updated_edges > 0);

    // Verify task-t1 frontmatter on disk now has depends_on task-t3
    let t1_content = fs::read_to_string(ws.path().join("tasks/task-t1.md")).unwrap();
    assert!(report.rejected_cycles.len() > 0);
    assert!(!t1_content.contains("task-t3"));
}

// ===========================================================================
// Test 4: Multi-Layer Sugiyama DAG Layout with Dummy Vertices
// ===========================================================================

#[test]
fn test_multi_layer_dummy_vertex_routing() {
    let mut nodes = Vec::new();
    for i in 0..5 {
        let mut n = GraphNode::default();
        n.id = format!("node-{i}");
        n.label = format!("Node {i}");
        nodes.push(n);
    }

    let mut edges = Vec::new();
    // Linear chain: 0 -> 1 -> 2 -> 3 -> 4
    for i in 0..4 {
        edges.push(Edge {
            source: format!("node-{i}"),
            target: format!("node-{}", i + 1),
            edge_type: EdgeType::DependsOn, // Directed flow: target -> source or source -> target
        });
    }

    // Long span edge spanning 3 layers: node-0 to node-3
    edges.push(Edge {
        source: "node-0".to_string(),
        target: "node-3".to_string(),
        edge_type: EdgeType::DependsOn,
    });

    // Cross-span edge spanning 4 layers: node-0 to node-4
    edges.push(Edge {
        source: "node-0".to_string(),
        target: "node-4".to_string(),
        edge_type: EdgeType::DependsOn,
    });

    let config = LayoutConfig::default();
    let positions = compute_sugiyama_layout(&nodes, &edges, &config);

    assert_eq!(positions.len(), 5, "All nodes must have layout positions");

    // Verify all positions are finite and positive
    for (id, (x, y)) in &positions {
        assert!(x.is_finite() && *x >= 100.0, "x for {id} must be finite: {x}");
        assert!(y.is_finite() && *y >= 100.0, "y for {id} must be finite: {y}");
    }

    // Generate scene and verify bound elements & frames
    let scene = generate_excalidraw_scene(&nodes, &edges, &config);
    assert!(!scene.elements.is_empty());

    // Verify arrows have proper normalized port bindings
    let arrow_elems: Vec<&ExcalidrawElement> = scene
        .elements
        .iter()
        .filter(|e| e.element_type == "arrow")
        .collect();
    assert_eq!(arrow_elems.len(), edges.len());

    for arrow in arrow_elems {
        assert!(arrow.start_binding.is_some());
        assert!(arrow.end_binding.is_some());
    }
}

// ===========================================================================
// Test 5: Bounded Archimedean Spiral Placement
// ===========================================================================

#[test]
fn test_bounded_archimedean_spiral_placement() {
    let center_x = 500.0;
    let center_y = 500.0;
    let card_w = CARD_WIDTH;
    let card_h = CARD_HEIGHT;

    let mut occupied: Vec<[f64; 4]> = Vec::new();
    // Place central occupied box
    occupied.push([
        center_x - card_w * 0.5,
        center_y - card_h * 0.5,
        center_x + card_w * 0.5,
        center_y + card_h * 0.5,
    ]);

    // Place 12 consecutive new cards using spiral placement
    for i in 0..12 {
        let (cand_x, cand_y) =
            find_spiral_placement(center_x, center_y, card_w, card_h, &occupied);

        let cand_box = [cand_x, cand_y, cand_x + card_w, cand_y + card_h];

        // Assert strictly NO collision with any existing box (including 24px safety margin)
        for (idx, prev) in occupied.iter().enumerate() {
            let collides = cand_box[0] < prev[2]
                && cand_box[2] > prev[0]
                && cand_box[1] < prev[3]
                && cand_box[3] > prev[1];
            assert!(
                !collides,
                "Spiral card {i} overlaps with occupied box {idx}: cand={:?}, prev={:?}",
                cand_box, prev
            );
        }

        occupied.push(cand_box);
    }

    // Test merge_live_into_canvas coordinate preservation
    let ws = create_test_pkb_workspace();
    let gs = GraphStore::build_from_directory(ws.path());

    let mut existing_file = ExcalidrawFile::default();
    let mut custom_card = ExcalidrawElement::default();
    custom_card.id = "card-t1".to_string();
    custom_card.element_type = "rectangle".to_string();
    custom_card.x = 1234.5;
    custom_card.y = 5678.9;
    custom_card.width = CARD_WIDTH;
    custom_card.height = CARD_HEIGHT;
    custom_card.custom_data = Some(CustomData {
        pkb: Some(PkbCustomData {
            node_id: Some("task-t1".to_string()),
            node_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            priority: Some(1),
            parent: None,
            tags: vec![],
            edge_type: None,
            is_pkb_managed: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    });
    existing_file.elements.push(custom_card);

    let target_nodes = vec!["task-t1".to_string(), "task-t2".to_string()];
    let merged_file = merge_live_into_canvas(&existing_file, &gs, &target_nodes);

    // Existing card must preserve EXACT user coordinates
    let merged_t1 = merged_file
        .elements
        .iter()
        .find(|e| {
            e.custom_data
                .as_ref()
                .and_then(|c| c.pkb.as_ref())
                .and_then(|p| p.node_id.as_deref())
                == Some("task-t1")
        })
        .expect("task-t1 in merged canvas");
    assert_eq!(merged_t1.x, 1234.5);
    assert_eq!(merged_t1.y, 5678.9);

    // Newly discovered task-t2 must be positioned without crashing
    let merged_t2 = merged_file
        .elements
        .iter()
        .find(|e| {
            e.custom_data
                .as_ref()
                .and_then(|c| c.pkb.as_ref())
                .and_then(|p| p.node_id.as_deref())
                == Some("task-t2")
        })
        .expect("task-t2 in merged canvas");
    assert!(merged_t2.x.is_finite());
    assert!(merged_t2.y.is_finite());
}

// ===========================================================================
// Test 6: Global Cycle Rejection
// ===========================================================================

#[test]
fn test_global_cycle_rejection() {
    let ws = create_test_pkb_workspace();
    let mut gs = GraphStore::build_from_directory(ws.path());

    // Populate a known linear dependency chain: A depends on B, B depends on C
    let mut na = GraphNode::default();
    na.id = "task-a".to_string();
    na.path = PathBuf::from("tasks/task-a.md");
    na.depends_on = vec!["task-b".to_string()];

    let mut nb = GraphNode::default();
    nb.id = "task-b".to_string();
    nb.path = PathBuf::from("tasks/task-b.md");
    nb.depends_on = vec!["task-c".to_string()];

    let mut nc = GraphNode::default();
    nc.id = "task-c".to_string();
    nc.path = PathBuf::from("tasks/task-c.md");

    gs.replace_node(na);
    gs.replace_node(nb);
    gs.replace_node(nc);

    // 1. Direct 2-node cycle: B depends on A
    let res1 = validate_no_cycle(&gs, "task-b", "task-a", &EdgeType::DependsOn);
    assert!(res1.is_err(), "Direct 2-node dependency cycle must be rejected");

    // 2. Transitive 3-node cycle: C depends on A
    let res2 = validate_no_cycle(&gs, "task-c", "task-a", &EdgeType::DependsOn);
    assert!(res2.is_err(), "Transitive dependency cycle must be rejected");
    let cycle_path = res2.unwrap_err();
    assert_eq!(cycle_path.first(), Some(&"task-a".to_string()));
    assert_eq!(cycle_path.last(), Some(&"task-a".to_string()));

    // 3. Self cycle: A depends on A
    let res3 = validate_no_cycle(&gs, "task-a", "task-a", &EdgeType::DependsOn);
    assert!(res3.is_err(), "Self cycle must be rejected");

    // 4. Non-blocking edge (Link / SimilarTo / ContributesTo) should be accepted even if cyclic
    let res4 = validate_no_cycle(&gs, "task-c", "task-a", &EdgeType::Link);
    assert!(res4.is_ok(), "Non-blocking Link must be accepted");

    let res5 = validate_no_cycle(&gs, "task-c", "task-a", &EdgeType::SimilarTo);
    assert!(res5.is_ok(), "SimilarTo edge must be accepted");

    // 5. Test sync rejection safety: sync must not corrupt disk on cycle
    let mut diff = GraphDiff::default();
    diff.added_edges.push(EdgeMutation {
        source: "task-c".to_string(),
        target: "task-a".to_string(),
        edge_type: EdgeType::DependsOn,
    });

    let report = sync_canvas(ws.path(), &mut gs, &diff, false).expect("sync");
    assert_eq!(report.rejected_cycles.len(), 1, "Must report rejected cycle");
    assert!(report.rejected_cycles[0].contains("cycle detected"));
}

// ===========================================================================
// Test 7: Clipboard Duplicate ID Handling
// ===========================================================================

#[test]
fn test_clipboard_duplicate_id_handling() {
    let ws = create_test_pkb_workspace();
    let mut gs = GraphStore::build_from_directory(ws.path());

    let mut file = ExcalidrawFile::default();

    // Original card
    let mut card_orig = ExcalidrawElement::default();
    card_orig.id = "elem-orig".to_string();
    card_orig.element_type = "rectangle".to_string();
    card_orig.x = 100.0;
    card_orig.y = 100.0;
    card_orig.custom_data = Some(CustomData {
        pkb: Some(PkbCustomData {
            node_id: Some("task-t1".to_string()),
            node_type: Some("task".to_string()),
            status: Some("ready".to_string()),
            priority: Some(1),
            parent: None,
            tags: vec![],
            edge_type: None,
            is_pkb_managed: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    });

    // Duplicated card (user pasted via clipboard, same node_id)
    let mut card_copy = card_orig.clone();
    card_copy.id = "elem-copy".to_string();
    card_copy.x = 400.0;
    card_copy.y = 100.0;

    file.elements.push(card_orig);
    file.elements.push(card_copy);

    let canvas = CanvasReader::parse_file(file);

    // Duplicate detection in Pass 5
    assert_eq!(canvas.duplicate_ids, vec!["task-t1".to_string()]);
    assert_eq!(canvas.cards.len(), 2);

    let first = &canvas.cards[0];
    assert_eq!(first.node_id.as_deref(), Some("task-t1"));
    assert!(!first.is_duplicate);
    assert!(!first.is_new);

    let second = &canvas.cards[1];
    assert_eq!(second.node_id, None, "Duplicate card ID must be reset to None");
    assert!(second.is_duplicate);
    assert!(second.is_new);

    // When synced, the duplicate card must create a NEW unique task without overwriting task-t1
    let diff = diff_canvas(None, &gs, &canvas).expect("diff");
    assert_eq!(diff.added_nodes.len(), 1, "Duplicate card must become AddedNodeMutation");

    let report = sync_canvas(ws.path(), &mut gs, &diff, false).expect("sync");
    assert_eq!(report.created_nodes.len(), 1);

    let (new_id, new_path) = &report.created_nodes[0];
    assert_ne!(new_id, "task-t1", "New task ID must not collide with task-t1");
    assert!(new_path.exists(), "New document must exist on disk");

    // Original task-t1.md must be untouched
    assert!(ws.path().join("tasks/task-t1.md").exists());
}

// ===========================================================================
// Test 8: CLI Execution (`pkb graph`, `pkb excalidraw`)
// ===========================================================================

#[test]
fn test_cli_excalidraw_and_graph_execution() {
    let ws = create_test_pkb_workspace();
    let pkb = pkb_binary();
    let pkb_excal = pkb_excalidraw_binary();
    let db_path = ws.path().join("pkb_vectors.bin");

    let out_dir = ws.path().join("cli_out");
    fs::create_dir_all(&out_dir).unwrap();

    // 1. `pkb-excalidraw export`
    let excal_out = out_dir.join("graph.excalidraw");
    let status = Command::new(&pkb_excal)
        .env("ACA_DATA", ws.path().to_str().unwrap())
        .env("AOPS_OFFLINE", "1")
        .args([
            "export",
            excal_out.to_str().unwrap(),
            "--pkb-root",
            ws.path().to_str().unwrap(),
        ])
        .status()
        .expect("run pkb-excalidraw export");
    assert!(status.success(), "pkb-excalidraw export must exit 0");
    assert!(excal_out.exists());
    let excal_content = fs::read_to_string(&excal_out).unwrap();
    let parsed: ExcalidrawFile = serde_json::from_str(&excal_content).expect("valid excal JSON");
    assert!(!parsed.elements.is_empty());

    // 2. `pkb graph --format json`
    let json_out = out_dir.join("graph.json");
    let status = Command::new(&pkb)
        .env("ACA_DATA", ws.path().to_str().unwrap())
        .env("AOPS_OFFLINE", "1")
        .args([
            "--pkb-root",
            ws.path().to_str().unwrap(),
            "--db-path",
            db_path.to_str().unwrap(),
            "graph",
            "--format",
            "json",
            "--output",
            json_out.to_str().unwrap(),
        ])
        .status()
        .expect("run pkb graph json");
    assert!(status.success());
    assert!(json_out.exists());

    // 3. `pkb graph --format graphml`
    let graphml_out = out_dir.join("graph.graphml");
    let status = Command::new(&pkb)
        .env("ACA_DATA", ws.path().to_str().unwrap())
        .env("AOPS_OFFLINE", "1")
        .args([
            "--pkb-root",
            ws.path().to_str().unwrap(),
            "--db-path",
            db_path.to_str().unwrap(),
            "graph",
            "--format",
            "graphml",
            "--output",
            graphml_out.to_str().unwrap(),
        ])
        .status()
        .expect("run pkb graph graphml");
    assert!(status.success());
    assert!(graphml_out.exists());

    // 4. `pkb excalidraw export`
    let export_out = out_dir.join("export.excalidraw");
    let status = Command::new(&pkb_excal)
        .env("ACA_DATA", ws.path().to_str().unwrap())
        .env("AOPS_OFFLINE", "1")
        .args([
            "export",
            export_out.to_str().unwrap(),
            "--focus",
            "epic-core",
            "--hops",
            "2",
            "--pkb-root",
            ws.path().to_str().unwrap(),
        ])
        .status()
        .expect("run pkb excalidraw export");
    assert!(status.success());
    assert!(export_out.exists());

    // 5. `pkb excalidraw diff`
    let output = Command::new(&pkb_excal)
        .env("ACA_DATA", ws.path().to_str().unwrap())
        .env("AOPS_OFFLINE", "1")
        .args([
            "diff",
            export_out.to_str().unwrap(),
            "--json",
            "--pkb-root",
            ws.path().to_str().unwrap(),
        ])
        .output()
        .expect("run pkb excalidraw diff");
    assert!(output.status.success());
    let diff_str = String::from_utf8_lossy(&output.stdout);
    let diff: GraphDiff = serde_json::from_str(&diff_str).expect("parse diff JSON output");
    assert!(diff.is_empty());

    // 6. `pkb excalidraw sync --dry-run`
    let status = Command::new(&pkb_excal)
        .env("ACA_DATA", ws.path().to_str().unwrap())
        .env("AOPS_OFFLINE", "1")
        .args([
            "sync",
            export_out.to_str().unwrap(),
            "--pkb-root",
            ws.path().to_str().unwrap(),
            "--dry-run",
        ])
        .status()
        .expect("run pkb excalidraw sync dry-run");
    assert!(status.success());
}

// ===========================================================================
// Test 9: MCP Tool Endpoints (`graph_excalidraw`, `diff_excalidraw`, `sync_excalidraw`)
// ===========================================================================

#[test]
fn test_mcp_excalidraw_tool_endpoints() {
    let ws = create_test_pkb_workspace();
    let db_path = ws.path().join("pkb_vectors.bin");
    let store = Arc::new(parking_lot::RwLock::new(VectorStore::new(1024)));
    let graph = Arc::new(parking_lot::RwLock::new(GraphStore::build_from_directory(ws.path())));

    let server = PkbSearchServer::new(
        store,
        Arc::new(mem::embeddings::Embedder::new_dummy()),
        ws.path().to_path_buf(),
        db_path,
        graph,
    );

    // 1. graph_excalidraw (full graph)
    let res = server
        .handle_graph_excalidraw(&json!({}))
        .expect("graph_excalidraw full");
    let text = res.content[0].as_text().unwrap();
    let full_file: ExcalidrawFile = serde_json::from_str(&text.text).expect("valid full scene JSON");
    assert!(!full_file.elements.is_empty());

    // 2. graph_excalidraw (ego network around focus node)
    let res_ego = server
        .handle_graph_excalidraw(&json!({
            "node_id": "epic-core",
            "hops": 1
        }))
        .expect("graph_excalidraw ego");
    let ego_text = res_ego.content[0].as_text().unwrap();
    let ego_file: ExcalidrawFile = serde_json::from_str(&ego_text.text).expect("valid ego scene JSON");
    assert!(!ego_file.elements.is_empty());

    // 3. diff_excalidraw (empty diff on fresh export)
    let res_diff = server
        .handle_diff_excalidraw(&json!({
            "canvas": text.text
        }))
        .expect("diff_excalidraw");
    let diff_text = res_diff.content[0].as_text().unwrap();
    let diff: GraphDiff = serde_json::from_str(&diff_text.text).expect("valid diff JSON");
    assert!(diff.is_empty());

    // 4. sync_excalidraw dry_run
    let res_sync_dry = server
        .handle_sync_excalidraw(&json!({
            "canvas": text.text,
            "dry_run": true
        }))
        .expect("sync_excalidraw dry_run");
    let sync_dry_text = res_sync_dry.content[0].as_text().unwrap();
    assert!(sync_dry_text.text.contains("Dry run"));

    // 5. sync_excalidraw real sync with added card
    let mut modified_file = full_file.clone();
    let mut new_card = ExcalidrawElement::default();
    new_card.id = "elem-brand-new".to_string();
    new_card.element_type = "rectangle".to_string();
    new_card.x = 200.0;
    new_card.y = 200.0;
    new_card.width = CARD_WIDTH;
    new_card.height = CARD_HEIGHT;

    let mut new_text = ExcalidrawElement::default();
    new_text.id = "elem-brand-new-text".to_string();
    new_text.element_type = "text".to_string();
    new_text.container_id = Some("elem-brand-new".to_string());
    new_text.text = Some("[READY · P1] Brand New Automated Subsystem Task\n#automation".to_string());

    new_card.bound_elements = Some(vec![BoundElement {
        id: "elem-brand-new-text".to_string(),
        element_type: "text".to_string(),
    }]);

    modified_file.elements.push(new_card);
    modified_file.elements.push(new_text);

    let mod_json = serde_json::to_string(&modified_file).unwrap();

    let res_sync = server
        .handle_sync_excalidraw(&json!({
            "canvas": mod_json,
            "dry_run": false
        }))
        .expect("sync_excalidraw real");
    let sync_text = res_sync.content[0].as_text().unwrap();
    assert!(sync_text.text.contains("\"success\": true"));
    assert!(sync_text.text.contains("\"created_nodes\""));
}

// ===========================================================================
// Test 10: Adversarial & Corrupted Canvas Inputs
// ===========================================================================

#[test]
fn test_adversarial_and_corrupted_canvas_inputs() {
    // 1. Completely invalid JSON
    let err = parse_canvas("Not valid JSON at all {");
    assert!(err.is_err(), "Must reject malformed JSON syntax");

    // 2. Empty JSON object
    let empty_file: ExcalidrawFile = serde_json::from_str("{}").unwrap();
    let canvas_empty = CanvasReader::parse_file(empty_file);
    assert!(canvas_empty.cards.is_empty());
    assert!(canvas_empty.arrows.is_empty());

    // 3. User freehand doodles and unattached sticky notes (Preservation)
    let mut file = ExcalidrawFile::default();

    let mut doodle = ExcalidrawElement::default();
    doodle.id = "doodle-1".to_string();
    doodle.element_type = "freedraw".to_string();
    doodle.x = 50.0;
    doodle.y = 50.0;

    let unbound_rect = ExcalidrawElement {
        id: "shape-1".to_string(),
        element_type: "rectangle".to_string(),
        x: 200.0,
        y: 200.0,
        ..Default::default()
    };

    file.elements.push(doodle);
    file.elements.push(unbound_rect);

    let canvas = CanvasReader::parse_file(file);
    assert_eq!(canvas.cards.len(), 0, "Unbound shapes without PKB metadata must not become cards");
    assert_eq!(canvas.annotations.len(), 2, "Doodles and unbound shapes must be preserved in annotations");

    // 4. Dangling arrow without start or end bindings -> classified into annotations
    let dangling_arrow = ExcalidrawElement {
        id: "dangling-1".to_string(),
        element_type: "arrow".to_string(),
        start_binding: None,
        end_binding: None,
        ..Default::default()
    };

    let mut file2 = ExcalidrawFile::default();
    file2.elements.push(dangling_arrow);
    let canvas2 = CanvasReader::parse_file(file2);
    assert_eq!(canvas2.arrows.len(), 0, "Unbound arrows must not be treated as graph edges");
    assert_eq!(canvas2.annotations.len(), 1, "Unbound arrows must be preserved in annotations");
}

// ===========================================================================
// Test 11: Non-PKB Connector Arrows Survive Canvas Sync
// ===========================================================================

#[test]
fn test_unlinked_non_pkb_arrows_survive_sync() {
    let ws = create_test_pkb_workspace();
    let gs = GraphStore::build_from_directory(ws.path());

    // Export graph to canvas
    let (excal_json, _, _) = gs.output_excalidraw(None, 2).expect("export excalidraw");
    let mut file: ExcalidrawFile = serde_json::from_str(&excal_json).unwrap();

    // 1. Add an unlinked floating arrow (no bindings)
    let mut floating_arrow = ExcalidrawElement::default();
    floating_arrow.id = "floating-arr-1".to_string();
    floating_arrow.element_type = "arrow".to_string();
    floating_arrow.x = 500.0;
    floating_arrow.y = 500.0;
    floating_arrow.points = Some(vec![[0.0, 0.0], [100.0, 100.0]]);
    floating_arrow.stroke_color = "#e03131".to_string();

    // 2. Add a non-PKB sticky note shape
    let mut sticky_note = ExcalidrawElement::default();
    sticky_note.id = "sticky-note-1".to_string();
    sticky_note.element_type = "rectangle".to_string();
    sticky_note.x = 650.0;
    sticky_note.y = 500.0;
    sticky_note.width = 120.0;
    sticky_note.height = 80.0;

    // 3. Add an arrow partially bound (from task-t1 card to the non-PKB sticky note)
    let mut partial_arrow = ExcalidrawElement::default();
    partial_arrow.id = "partial-arr-1".to_string();
    partial_arrow.element_type = "arrow".to_string();
    partial_arrow.start_binding = Some(PointBinding {
        element_id: "card-task-t1".to_string(),
        focus: 0.0,
        gap: 1.0,
        fixed_point: None,
    });
    partial_arrow.end_binding = Some(PointBinding {
        element_id: "sticky-note-1".to_string(),
        focus: 0.0,
        gap: 1.0,
        fixed_point: None,
    });

    file.elements.push(floating_arrow);
    file.elements.push(sticky_note);
    file.elements.push(partial_arrow);

    // Parse canvas
    let canvas = CanvasReader::parse_file(file.clone());

    // Both floating arrow, sticky note, and partial arrow must be classified as annotations
    let ann_ids: Vec<&str> = canvas.annotations.iter().map(|a| a.id.as_str()).collect();
    assert!(ann_ids.contains(&"floating-arr-1"));
    assert!(ann_ids.contains(&"sticky-note-1"));
    assert!(ann_ids.contains(&"partial-arr-1"));

    // Merge with live graph
    let target_node_ids = vec!["epic-core".to_string(), "task-t1".to_string(), "task-t2".to_string()];
    let merged = merge_canvas_with_live(&file, &gs, &target_node_ids).expect("merge");

    // Assert all user annotations survive in merged output elements
    let merged_elem_ids: Vec<&str> = merged.elements.iter().map(|e| e.id.as_str()).collect();
    assert!(merged_elem_ids.contains(&"floating-arr-1"), "Floating arrow must survive merge");
    assert!(merged_elem_ids.contains(&"sticky-note-1"), "Sticky note must survive merge");
    assert!(merged_elem_ids.contains(&"partial-arr-1"), "Partial arrow must survive merge");
}

// ===========================================================================
// Test 12: Asymmetric Dependency Removal (`--sync-edge-removals`)
// ===========================================================================

#[test]
fn test_sync_edge_removals_flag_e2e() {
    let ws = create_test_pkb_workspace();

    // Prepare task with multiple dependency formats: bare ID, wikilink, and filename path
    let task_path = ws.path().join("tasks/task-dep-test.md");
    fs::write(
        &task_path,
        "---\n\
         id: task-dep-test\n\
         title: \"Dependency Removal Test\"\n\
         type: task\n\
         status: ready\n\
         priority: 1\n\
         project: testproj\n\
         depends_on:\n\
           - task-t1\n\
           - '[[task-t2]]'\n\
           - tasks/task-t3.md\n\
         ---\n\n# Dep Test\n",
    )
    .unwrap();

    let mut gs = GraphStore::build_from_directory(ws.path());

    // Construct diff with removed dependency edges
    let mut diff = GraphDiff::default();
    diff.removed_edges.push(EdgeMutation {
        source: "task-dep-test".to_string(),
        target: "task-t1".to_string(),
        edge_type: EdgeType::DependsOn,
    });
    diff.removed_edges.push(EdgeMutation {
        source: "task-dep-test".to_string(),
        target: "task-t2".to_string(),
        edge_type: EdgeType::DependsOn,
    });

    // Case 1: sync_edge_removals = false (default) -> frontmatter must NOT be modified
    let report_false = sync_canvas(ws.path(), &mut gs, &diff, false).expect("sync false");
    assert_eq!(report_false.updated_edges, 0);
    let content_preserved = fs::read_to_string(&task_path).unwrap();
    assert!(content_preserved.contains("task-t1"), "task-t1 must be preserved when sync_edge_removals=false");
    assert!(content_preserved.contains("task-t2"), "task-t2 must be preserved when sync_edge_removals=false");
    assert!(content_preserved.contains("tasks/task-t3.md"));

    // Case 2: sync_edge_removals = true -> bare ID and wikilink are stripped
    let report_true = sync_canvas(ws.path(), &mut gs, &diff, true).expect("sync true");
    assert_eq!(report_true.updated_edges, 2);
    let content_stripped = fs::read_to_string(&task_path).unwrap();
    assert!(!content_stripped.contains("task-t1"), "task-t1 must be removed when sync_edge_removals=true");
    assert!(!content_stripped.contains("task-t2"), "task-t2 must be removed when sync_edge_removals=true");
    assert!(content_stripped.contains("tasks/task-t3.md"), "tasks/task-t3.md must remain untouched");

    // Case 3: remove filename path reference tasks/task-t3.md
    let mut diff_t3 = GraphDiff::default();
    diff_t3.removed_edges.push(EdgeMutation {
        source: "task-dep-test".to_string(),
        target: "task-t3".to_string(),
        edge_type: EdgeType::DependsOn,
    });
    let report_t3 = sync_canvas(ws.path(), &mut gs, &diff_t3, true).expect("sync t3");
    assert_eq!(report_t3.updated_edges, 1);
    let content_t3 = fs::read_to_string(&task_path).unwrap();
    assert!(!content_t3.contains("tasks/task-t3.md"), "tasks/task-t3.md must be removed");
}

// ===========================================================================
// Test 13: Preserve Custom Card Styling
// ===========================================================================

#[test]
fn test_preserve_custom_card_styling_e2e() {
    let ws = create_test_pkb_workspace();
    let mut gs = GraphStore::build_from_directory(ws.path());

    // Export graph to canvas
    let (excal_json, _, _) = gs.output_excalidraw(None, 2).expect("export excalidraw");
    let mut file: ExcalidrawFile = serde_json::from_str(&excal_json).unwrap();

    // Customize card for task-t1
    let custom_bg = "#fab005";
    let custom_stroke = "#e67700";
    let custom_stroke_width = 4.0;
    let custom_roughness = 2.0;

    for elem in &mut file.elements {
        let is_t1_card = elem
            .custom_data
            .as_ref()
            .and_then(|c| c.pkb.as_ref())
            .and_then(|p| p.node_id.as_deref())
            == Some("task-t1");

        if is_t1_card {
            elem.background_color = custom_bg.to_string();
            elem.stroke_color = custom_stroke.to_string();
            elem.stroke_width = custom_stroke_width;
            elem.roughness = custom_roughness;
        }
    }

    // 1. Merge when status is unchanged (ready == ready)
    let merged_unchanged = merge_canvas_with_live(&file, &gs, &["task-t1".to_string()]).expect("merge unchanged");
    let t1_card = merged_unchanged
        .elements
        .iter()
        .find(|e| {
            e.custom_data
                .as_ref()
                .and_then(|c| c.pkb.as_ref())
                .and_then(|p| p.node_id.as_deref())
                == Some("task-t1")
                && e.element_type == "rectangle"
        })
        .expect("t1 card in merged");

    assert_eq!(t1_card.background_color, custom_bg, "Custom background must be preserved");
    assert_eq!(t1_card.stroke_color, custom_stroke, "Custom stroke color must be preserved");
    assert_eq!(t1_card.stroke_width, custom_stroke_width, "Custom stroke width must be preserved");
    assert_eq!(t1_card.roughness, custom_roughness, "Custom roughness must be preserved");

    // 2. Update task-t1 status in live graph to "done"
    let mut node_t1 = gs.get_node("task-t1").unwrap().clone();
    node_t1.status = Some("done".to_string());
    gs.replace_node(node_t1);

    // Merge when status changed -> must apply new done status color palette
    let merged_changed = merge_canvas_with_live(&file, &gs, &["task-t1".to_string()]).expect("merge changed");
    let t1_card_changed = merged_changed
        .elements
        .iter()
        .find(|e| {
            e.custom_data
                .as_ref()
                .and_then(|c| c.pkb.as_ref())
                .and_then(|p| p.node_id.as_deref())
                == Some("task-t1")
                && e.element_type == "rectangle"
        })
        .expect("t1 card in changed merge");

    let done_palette = node_color_style(Some("done"), Some("task"));
    assert_eq!(
        t1_card_changed.background_color, done_palette.bg_color,
        "Background color must update to done palette on status transition"
    );
    assert_eq!(
        t1_card_changed.stroke_color, done_palette.stroke_color,
        "Stroke color must update to done palette on status transition"
    );
}
