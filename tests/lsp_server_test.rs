//! Integration tests for PKB Language Server Protocol (LSP).

use lsp_types::*;
use mem::lsp::LspServer;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_hover_pkb_reference_shows_preview() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();

    // 1. Create a PKB task file
    let tasks_dir = pkb_root.join("tasks");
    fs::create_dir_all(&tasks_dir).unwrap();
    let task_path = tasks_dir.join("task-test.md");
    let task_content = r#"---
id: task-test
title: Build LSP Preview
type: task
status: ready
intent: 1
tags:
  - lsp
  - vscode
---
This is the goal and description of the test task.
"#;
    fs::write(&task_path, task_content).unwrap();

    // 2. Initialize LSP server
    let mut server = LspServer::new(pkb_root.clone());

    // 3. Open a document containing a reference to [[task-test]]
    let doc_path = pkb_root.join("notes").join("overview.md");
    let doc_uri: Uri = format!("file://{}", doc_path.to_string_lossy())
        .parse()
        .unwrap();
    let doc_text = "Check out [[task-test]] for details.\n";
    server.handle_did_open(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: doc_uri.clone(),
            language_id: "markdown".to_string(),
            version: 1,
            text: doc_text.to_string(),
        },
    });

    // 4. Hover over `[[task-test]]` (e.g. character 14 is inside `task-test`)
    let hover_params = HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: doc_uri.clone(),
            },
            position: Position {
                line: 0,
                character: 14,
            },
        },
        work_done_progress_params: Default::default(),
    };

    let hover = server.handle_hover(&hover_params);
    assert!(
        hover.is_some(),
        "Hovering over [[task-test]] should return a hover preview"
    );

    let hover = hover.unwrap();
    let hover_text = match hover.contents {
        HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }) => value,
        other => panic!("Expected Markdown markup, got {:?}", other),
    };

    assert!(
        hover_text.contains("Build LSP Preview"),
        "Hover preview should contain title 'Build LSP Preview', got: {hover_text}"
    );
    assert!(
        hover_text.contains("task-test"),
        "Hover preview should contain ID 'task-test', got: {hover_text}"
    );
    assert!(
        hover_text.contains("ready"),
        "Hover preview should contain status 'ready', got: {hover_text}"
    );
    assert!(
        hover_text.contains("This is the goal and description"),
        "Hover preview should contain body snippet, got: {hover_text}"
    );

    // Hover outside reference (at character 0 'C') should return None
    let hover_outside = server.handle_hover(&HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: doc_uri },
            position: Position {
                line: 0,
                character: 0,
            },
        },
        work_done_progress_params: Default::default(),
    });
    assert!(
        hover_outside.is_none(),
        "Hovering outside a reference should return None"
    );
}

#[test]
fn test_definition_opens_referenced_file() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();

    let tasks_dir = pkb_root.join("tasks");
    fs::create_dir_all(&tasks_dir).unwrap();
    let task_path = tasks_dir.join("task-open-me.md");
    let task_content = r#"---
id: task-open-me
title: Target Document To Open
type: task
status: ready
---
Full content of the target document that opens in a new editor tab.
"#;
    fs::write(&task_path, task_content).unwrap();

    let mut server = LspServer::new(pkb_root.clone());

    let doc_path = pkb_root.join("notes").join("ref.md");
    let doc_uri: Uri = format!("file://{}", doc_path.to_string_lossy())
        .parse()
        .unwrap();
    let doc_text = "Link to [[task-open-me|Target Title]] here.\n";
    server.handle_did_open(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: doc_uri.clone(),
            language_id: "markdown".to_string(),
            version: 1,
            text: doc_text.to_string(),
        },
    });

    // 1. Definition request at character 12 (inside `task-open-me`)
    let def_params = GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: doc_uri.clone(),
            },
            position: Position {
                line: 0,
                character: 12,
            },
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };

    let def_resp = server.handle_definition(&def_params);
    assert!(
        def_resp.is_some(),
        "Definition request on [[task-open-me]] must resolve"
    );

    let location = match def_resp.unwrap() {
        GotoDefinitionResponse::Scalar(loc) => loc,
        other => panic!("Expected scalar Location, got {:?}", other),
    };

    let target_uri_str = location.uri.as_str();
    assert!(
        target_uri_str.ends_with("task-open-me.md"),
        "Target URI must point to task-open-me.md, got: {target_uri_str}"
    );

    // Verify reading the whole file at that path yields the full content
    let raw_file_path = target_uri_str.strip_prefix("file://").unwrap();
    let file_content =
        fs::read_to_string(raw_file_path).expect("File at definition URI must exist");
    assert!(
        file_content
            .contains("Full content of the target document that opens in a new editor tab."),
        "Whole referenced file must be readable from target URI"
    );

    // 2. Document links request
    let links = server.handle_document_link(&DocumentLinkParams {
        text_document: TextDocumentIdentifier { uri: doc_uri },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    });
    assert_eq!(links.len(), 1, "Should find 1 document link in the file");
    assert!(
        links[0]
            .target
            .as_ref()
            .unwrap()
            .as_str()
            .ends_with("task-open-me.md"),
        "Document link target must point to task-open-me.md"
    );
}

#[test]
fn test_bare_id_reference_hover_and_definition() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();

    let tasks_dir = pkb_root.join("tasks");
    fs::create_dir_all(&tasks_dir).unwrap();
    let task_path = tasks_dir.join("task-bare.md");
    let task_content = r#"---
id: task-bare
title: Bare ID Target
type: task
status: ready
---
Bare ID content.
"#;
    fs::write(&task_path, task_content).unwrap();

    let mut server = LspServer::new(pkb_root.clone());

    let doc_path = pkb_root.join("notes").join("bare.md");
    let doc_uri: Uri = format!("file://{}", doc_path.to_string_lossy())
        .parse()
        .unwrap();
    // Bare ID mention in prose (e.g. `Fixed in task-bare yesterday.`)
    let doc_text = "Fixed in task-bare yesterday.\n";
    server.handle_did_open(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: doc_uri.clone(),
            language_id: "markdown".to_string(),
            version: 1,
            text: doc_text.to_string(),
        },
    });

    // Hover over `task-bare` at character 11
    let hover_params = HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: doc_uri.clone(),
            },
            position: Position {
                line: 0,
                character: 11,
            },
        },
        work_done_progress_params: Default::default(),
    };

    let hover = server.handle_hover(&hover_params);
    assert!(
        hover.is_some(),
        "Hovering over bare ID task-bare should return a preview"
    );

    let def_params = GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: doc_uri },
            position: Position {
                line: 0,
                character: 11,
            },
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let def_resp = server.handle_definition(&def_params);
    assert!(
        def_resp.is_some(),
        "Definition request on bare ID task-bare should resolve"
    );
}

#[test]
fn test_lsp_jsonrpc_connection_lifecycle() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();

    let tasks_dir = pkb_root.join("tasks");
    fs::create_dir_all(&tasks_dir).unwrap();
    let task_path = tasks_dir.join("task-rpc.md");
    let task_content = r#"---
id: task-rpc
title: Task For JSON-RPC
type: task
status: ready
---
Testing full JSON-RPC message exchange.
"#;
    fs::write(&task_path, task_content).unwrap();

    let (server_conn, client_conn) = lsp_server::Connection::memory();
    let server = LspServer::new(pkb_root.clone());

    // Run server in background thread
    let server_handle = std::thread::spawn(move || {
        server.run_connection(server_conn).unwrap();
    });

    // 1. Initialize
    let root_uri = format!("file://{}", pkb_root.to_string_lossy())
        .parse()
        .unwrap();
    let init_params = InitializeParams {
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: root_uri,
            name: "pkb".to_string(),
        }]),
        capabilities: ClientCapabilities::default(),
        ..Default::default()
    };
    let init_id = lsp_server::RequestId::from(1);
    client_conn
        .sender
        .send(lsp_server::Message::Request(lsp_server::Request::new(
            init_id.clone(),
            "initialize".to_string(),
            serde_json::to_value(init_params).unwrap(),
        )))
        .unwrap();

    let resp = match client_conn.receiver.recv().unwrap() {
        lsp_server::Message::Response(r) => r,
        other => panic!("Expected response, got {:?}", other),
    };
    assert_eq!(resp.id, init_id);
    let init_result: InitializeResult =
        serde_json::from_value(resp.response_result.unwrap()).unwrap();
    assert!(init_result.capabilities.hover_provider.is_some());
    assert!(init_result.capabilities.definition_provider.is_some());

    // Send initialized notification as required by LSP protocol
    client_conn
        .sender
        .send(lsp_server::Message::Notification(
            lsp_server::Notification::new(
                "initialized".to_string(),
                serde_json::to_value(InitializedParams {}).unwrap(),
            ),
        ))
        .unwrap();

    // 2. Open document
    let doc_path = pkb_root.join("notes").join("doc.md");
    let doc_uri: Uri = format!("file://{}", doc_path.to_string_lossy())
        .parse()
        .unwrap();
    let doc_text = "See [[task-rpc]] here.\n";
    client_conn
        .sender
        .send(lsp_server::Message::Notification(
            lsp_server::Notification::new(
                "textDocument/didOpen".to_string(),
                serde_json::to_value(DidOpenTextDocumentParams {
                    text_document: TextDocumentItem {
                        uri: doc_uri.clone(),
                        language_id: "markdown".to_string(),
                        version: 1,
                        text: doc_text.to_string(),
                    },
                })
                .unwrap(),
            ),
        ))
        .unwrap();

    // 3. Hover request
    let hover_id = lsp_server::RequestId::from(2);
    let hover_params = HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: doc_uri.clone(),
            },
            position: Position {
                line: 0,
                character: 8,
            },
        },
        work_done_progress_params: Default::default(),
    };
    client_conn
        .sender
        .send(lsp_server::Message::Request(lsp_server::Request::new(
            hover_id.clone(),
            "textDocument/hover".to_string(),
            serde_json::to_value(hover_params).unwrap(),
        )))
        .unwrap();

    let hover_resp = match client_conn.receiver.recv().unwrap() {
        lsp_server::Message::Response(r) => r,
        other => panic!("Expected hover response, got {:?}", other),
    };
    assert_eq!(hover_resp.id, hover_id);
    let hover: Hover = serde_json::from_value(hover_resp.response_result.unwrap()).unwrap();
    let hover_str = match hover.contents {
        HoverContents::Markup(m) => m.value,
        _ => panic!("Expected markup"),
    };
    assert!(hover_str.contains("Task For JSON-RPC"));

    // 4. Definition request
    let def_id = lsp_server::RequestId::from(3);
    let def_params = GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: doc_uri },
            position: Position {
                line: 0,
                character: 8,
            },
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    client_conn
        .sender
        .send(lsp_server::Message::Request(lsp_server::Request::new(
            def_id.clone(),
            "textDocument/definition".to_string(),
            serde_json::to_value(def_params).unwrap(),
        )))
        .unwrap();

    let def_resp = match client_conn.receiver.recv().unwrap() {
        lsp_server::Message::Response(r) => r,
        other => panic!("Expected definition response, got {:?}", other),
    };
    assert_eq!(def_resp.id, def_id);
    let def: GotoDefinitionResponse =
        serde_json::from_value(def_resp.response_result.unwrap()).unwrap();
    match def {
        GotoDefinitionResponse::Scalar(loc) => {
            assert!(loc.uri.as_str().ends_with("task-rpc.md"));
        }
        _ => panic!("Expected scalar Location"),
    }

    // 5. Shutdown
    let shutdown_id = lsp_server::RequestId::from(4);
    client_conn
        .sender
        .send(lsp_server::Message::Request(lsp_server::Request::new(
            shutdown_id.clone(),
            "shutdown".to_string(),
            serde_json::Value::Null,
        )))
        .unwrap();

    let shutdown_resp = match client_conn.receiver.recv().unwrap() {
        lsp_server::Message::Response(r) => r,
        other => panic!("Expected shutdown response, got {:?}", other),
    };
    assert_eq!(shutdown_resp.id, shutdown_id);

    // Send exit notification
    client_conn
        .sender
        .send(lsp_server::Message::Notification(
            lsp_server::Notification::new("exit".to_string(), serde_json::Value::Null),
        ))
        .unwrap();

    // Drop client connection so receiver closes and server thread joins
    drop(client_conn);
    server_handle.join().unwrap();
}
