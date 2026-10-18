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
