//! Tests that the `export_graph` MCP tool is registered with the schema the
//! `pkb__export_graph` MCP tool doc promises (params, read-only annotation).

use mem::mcp_server::PkbSearchServer;

#[test]
fn test_export_graph_tool_is_registered_read_only_with_expected_params() {
    let tools = PkbSearchServer::get_all_tools();
    let tool = tools
        .iter()
        .find(|t| t.name.as_ref() == "export_graph")
        .expect("export_graph tool must be registered");

    assert!(
        tool.annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false),
        "export_graph must be marked read_only_hint: true"
    );

    let desc = tool.description.as_deref().unwrap_or("").to_lowercase();
    assert!(desc.contains("dot"), "description must mention DOT: {desc}");
    assert!(desc.contains("digraph"), "description must mention digraph syntax: {desc}");

    let schema_str = serde_json::to_string(&tool.input_schema).unwrap();
    for param in ["focus", "max_depth", "project", "include_done"] {
        assert!(
            schema_str.contains(param),
            "export_graph schema must declare param '{param}', got: {schema_str}"
        );
    }
}
