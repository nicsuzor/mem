//! Tests ensuring agent-facing schemas and documentation maintain integrity and do not drift.
//!
//! Enforces:
//! 1. No agent-facing tool schema contains "Birnbaum" (renamed to verbal contribution-weight scale).
//! 2. No tool schema contains dead section references (e.g. multi-parent.md §7 or TAXONOMY §Priority Labels).

use mem::mcp_server::PkbSearchServer;

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
fn test_intent_default_is_documented_as_p4() {
    let tools = PkbSearchServer::get_all_tools();
    let mut checked_tools = Vec::new();
    for tool in tools {
        let name = tool.name.as_ref();
        let schema_str = serde_json::to_string(&tool.input_schema).unwrap();
        if schema_str.contains("Default when unset") {
            assert!(
                !schema_str.contains("Default when unset: P3"),
                "Tool {name} input_schema documents intent default as P3 instead of P4"
            );
            assert!(
                schema_str.contains("Default when unset: P4"),
                "Tool {name} input_schema mentions 'Default when unset' but not P4: {schema_str}"
            );
            checked_tools.push(name.to_string());
        }
    }
    assert!(
        checked_tools.contains(&"create_task".to_string()),
        "create_task must document intent default as P4"
    );
    assert!(
        checked_tools.contains(&"create".to_string()),
        "create must document intent default as P4"
    );
    assert!(
        checked_tools.contains(&"decompose_task".to_string()),
        "decompose_task must document intent default as P4"
    );
}

