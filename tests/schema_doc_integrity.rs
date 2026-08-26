//! Tests ensuring agent-facing schemas and documentation maintain integrity and do not drift.
//!
//! Enforces:
//! 1. No agent-facing tool schema contains "Birnbaum" (renamed to verbal contribution-weight scale).
//! 2. No tool schema contains dead section references (e.g. multi-parent.md §7 or TAXONOMY §Priority Labels).
//! 3. Tool descriptions for list_tasks and get_task accurately enumerate all 8 focus_score components.
//! 4. Canonical specification specs/ranking.md exists and covers all ranking measures and standing doctrine.

use mem::mcp_server::PkbSearchServer;
use std::path::Path;

#[test]
fn test_no_birnbaum_in_agent_facing_schemas() {
    let tools = PkbSearchServer::get_all_tools();
    for tool in tools {
        let name = tool.name.as_ref();
        if let Some(desc) = tool.description.as_deref() {
            assert!(
                !desc.to_lowercase().contains("birnbaum"),
                "Tool {name} description contains 'Birnbaum': {desc}"
            );
        }
        let schema_str = serde_json::to_string(&tool.input_schema).unwrap();
        assert!(
            !schema_str.to_lowercase().contains("birnbaum"),
            "Tool {name} input_schema contains 'Birnbaum': {schema_str}"
        );
    }
}

#[test]
fn test_no_dead_spec_references_in_tool_schemas() {
    let tools = PkbSearchServer::get_all_tools();
    for tool in tools {
        let name = tool.name.as_ref();
        let desc = tool.description.as_deref().unwrap_or("");
        let schema_str = serde_json::to_string(&tool.input_schema).unwrap();
        let combined = format!("{desc} {schema_str}");

        assert!(
            !combined.contains("multi-parent.md §7"),
            "Tool {name} contains dead citation 'multi-parent.md §7'"
        );
        assert!(
            !combined.contains("TAXONOMY §Priority Labels"),
            "Tool {name} contains dead citation 'TAXONOMY §Priority Labels'"
        );
    }
}

#[test]
fn test_tool_descriptions_enumerate_all_eight_focus_score_components() {
    let tools = PkbSearchServer::get_all_tools();
    let expected_components = [
        "priority",
        "severity",
        "deadline",
        "age",
        "downstream weight",
        "stakeholder",
        "urgency",
        "voi",
    ];

    for tool_name in ["list_tasks", "get_task"] {
        let tool = tools
            .iter()
            .find(|t| t.name.as_ref() == tool_name)
            .unwrap_or_else(|| panic!("{tool_name} tool must exist"));
        let desc = tool
            .description
            .as_deref()
            .unwrap_or_else(|| panic!("{tool_name} must have description"))
            .to_lowercase();

        for component in &expected_components {
            assert!(
                desc.contains(component),
                "Tool {tool_name} description must mention focus_score component '{component}', got: {desc}"
            );
        }
    }
}

#[test]
fn test_ranking_spec_exists_and_contains_canonical_sections() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let ranking_spec_path = Path::new(manifest_dir).join("specs/ranking.md");
    assert!(
        ranking_spec_path.exists(),
        "specs/ranking.md must exist on disk at {}",
        ranking_spec_path.display()
    );

    let content = std::fs::read_to_string(&ranking_spec_path)
        .expect("should be able to read specs/ranking.md");

    // Must record the pinned commit
    assert!(
        content.contains("41d82fee29479346a81275a8b12d8166c249b627"),
        "specs/ranking.md must record the pinned commit 41d82fee29479346a81275a8b12d8166c249b627"
    );

    // Must cover all ranking measures
    let required_terms = [
        "priority_base",
        "severity_bonus",
        "deadline_score",
        "age_staleness_bonus",
        "downstream_weight",
        "stakeholder_waiting",
        "urgency_term",
        "voi_term",
        "criticality",
        "uncertainty",
        "effective_priority",
        "scope",
        "pagerank",
        "betweenness",
        "verbal contribution-weight scale",
        "classify_tasks",
        "focus_cmp",
    ];

    for term in &required_terms {
        assert!(
            content.contains(term),
            "specs/ranking.md must cover '{term}'"
        );
    }

    // Must record the standing doctrine on graph metrics
    assert!(
        content.contains("Standing Ruling (Phase 2)") || content.contains("Standing Doctrine"),
        "specs/ranking.md must record the standing doctrine on graph metrics"
    );
    assert!(
        content.contains("gardening lens"),
        "specs/ranking.md must explain graph metrics as a gardening lens"
    );

    // Must state the mechanism vs model validation distinction
    assert!(
        content.contains("Mechanism Tests Exist") && content.contains("Model Validation Does NOT Exist"),
        "specs/ranking.md must distinguish between mechanism tests and empirical model validation"
    );
}
