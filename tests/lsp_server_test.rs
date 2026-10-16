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

#[test]
fn test_lsp_malformed_request_does_not_crash_server() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();

    let (server_conn, client_conn) = lsp_server::Connection::memory();
    let server = LspServer::new(pkb_root);
    let server_handle = std::thread::spawn(move || {
        let _ = server.run_connection(server_conn);
    });

    // 1. Initialize
    let init_id = lsp_server::RequestId::from(1);
    #[allow(deprecated)]
    let init_params = InitializeParams {
        root_uri: None,
        capabilities: ClientCapabilities::default(),
        ..Default::default()
    };
    client_conn
        .sender
        .send(lsp_server::Message::Request(lsp_server::Request::new(
            init_id,
            "initialize".to_string(),
            serde_json::to_value(init_params).unwrap(),
        )))
        .unwrap();
    let _ = client_conn.receiver.recv().unwrap();

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

    // 2. Send malformed hover request (invalid json structure for HoverParams)
    let bad_hover_id = lsp_server::RequestId::from(2);
    client_conn
        .sender
        .send(lsp_server::Message::Request(lsp_server::Request::new(
            bad_hover_id.clone(),
            "textDocument/hover".to_string(),
            serde_json::json!({ "invalid_field": 123 }),
        )))
        .unwrap();

    let resp = match client_conn.receiver.recv().unwrap() {
        lsp_server::Message::Response(r) => r,
        other => panic!("Expected error response, got {:?}", other),
    };
    assert_eq!(resp.id, bad_hover_id);
    assert!(
        resp.response_result.is_err(),
        "Malformed request should return error, not crash"
    );

    // 3. Verify server is still alive and responds to shutdown
    let shutdown_id = lsp_server::RequestId::from(3);
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

    drop(client_conn);
    server_handle.join().unwrap();
}

#[test]
fn test_lsp_cli_accepts_stdio_flag() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();

    // Check that pkb lsp --root <dir> --stdio can start without exiting code 2 (clap argument error)
    let pkb_exe = env!("CARGO_BIN_EXE_pkb");
    let mut child = std::process::Command::new(pkb_exe)
        .args(["lsp", "--root", pkb_root.to_str().unwrap(), "--stdio"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to spawn pkb lsp with --stdio");

    // Give it a short moment to ensure clap didn't immediately fail
    std::thread::sleep(std::time::Duration::from_millis(150));

    // If clap failed on --stdio, it would have exited immediately with code 2
    if let Ok(Some(status)) = child.try_wait() {
        panic!("pkb lsp exited immediately with status: {:?}", status);
    }

    // Kill the running process
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn test_lsp_cli_root_env_var_expansion() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();
    let sub = pkb_root.join("sub_pkb");
    fs::create_dir_all(&sub).unwrap();

    let pkb_exe = env!("CARGO_BIN_EXE_pkb");
    let output = std::process::Command::new(pkb_exe)
        .env("CUSTOM_PKB_ROOT_TEST", pkb_root.to_str().unwrap())
        .args(["lsp", "--root", "$CUSTOM_PKB_ROOT_TEST/sub_pkb", "--stdio"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("Failed to execute pkb lsp");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("PKB root: {}", sub.display())),
        "Expected stderr to contain expanded root {}, got: {}",
        sub.display(),
        stderr
    );
}

#[test]
fn test_lsp_cli_pkb_root_alias_and_deprecation_warning() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();
    let sub = pkb_root.join("sub_pkb");
    fs::create_dir_all(&sub).unwrap();

    let pkb_exe = env!("CARGO_BIN_EXE_pkb");
    let output = std::process::Command::new(pkb_exe)
        .env("CUSTOM_PKB_ROOT_TEST2", pkb_root.to_str().unwrap())
        .args([
            "lsp",
            "--pkb-root",
            "$CUSTOM_PKB_ROOT_TEST2/sub_pkb",
            "--stdio",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("Failed to execute pkb lsp");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("warning: --pkb-root is deprecated, please use --root instead"),
        "Expected deprecation warning in stderr, got: {}",
        stderr
    );
    assert!(
        stderr.contains(&format!("PKB root: {}", sub.display())),
        "Expected stderr to contain expanded root {}, got: {}",
        sub.display(),
        stderr
    );
}

#[test]
fn test_global_root_flag_before_subcommand() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();
    let sub = pkb_root.join("sub_pkb");
    fs::create_dir_all(&sub).unwrap();

    let pkb_exe = env!("CARGO_BIN_EXE_pkb");
    let output = std::process::Command::new(pkb_exe)
        .env("CUSTOM_PKB_ROOT_TEST3", pkb_root.to_str().unwrap())
        .args(["--root", "$CUSTOM_PKB_ROOT_TEST3/sub_pkb", "lsp", "--stdio"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("Failed to execute pkb lsp");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("PKB root: {}", sub.display())),
        "Expected stderr to contain expanded root {}, got: {}",
        sub.display(),
        stderr
    );
}

#[test]
fn test_lsp_custom_status_and_resolve_reference_requests() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();

    let tasks_dir = pkb_root.join("tasks");
    fs::create_dir_all(&tasks_dir).unwrap();
    let epic_path = tasks_dir.join("epic-root.md");
    fs::write(
        &epic_path,
        "---\nid: epic-root\ntitle: Root Epic\ntype: epic\nstatus: ready\n---\nRoot epic.\n",
    )
    .unwrap();
    let task_path = tasks_dir.join("task-custom.md");
    let task_content = r#"---
id: task-custom
title: Custom Resolve Target
type: task
status: in_progress
intent: 2
tags:
  - preview
parent: epic-root
---
Body of the custom resolve target for markdown preview.
"#;
    fs::write(&task_path, task_content).unwrap();

    let (server_conn, client_conn) = lsp_server::Connection::memory();
    let server = LspServer::new(pkb_root.clone());
    let server_handle = std::thread::spawn(move || {
        server.run_connection(server_conn).unwrap();
    });

    // 1. Initialize
    let init_id = lsp_server::RequestId::from(1);
    #[allow(deprecated)]
    let init_params = InitializeParams {
        root_uri: None,
        capabilities: ClientCapabilities::default(),
        ..Default::default()
    };
    client_conn
        .sender
        .send(lsp_server::Message::Request(lsp_server::Request::new(
            init_id,
            "initialize".to_string(),
            serde_json::to_value(init_params).unwrap(),
        )))
        .unwrap();
    let _ = client_conn.receiver.recv().unwrap();

    client_conn
        .sender
        .send(lsp_server::Message::Notification(
            lsp_server::Notification::new(
                "initialized".to_string(),
                serde_json::to_value(InitializedParams {}).unwrap(),
            ),
        ))
        .unwrap();

    // 2. Request mem/status
    let status_id = lsp_server::RequestId::from(2);
    client_conn
        .sender
        .send(lsp_server::Message::Request(lsp_server::Request::new(
            status_id.clone(),
            "mem/status".to_string(),
            serde_json::json!({}),
        )))
        .unwrap();

    let status_resp = match client_conn.receiver.recv().unwrap() {
        lsp_server::Message::Response(r) => r,
        other => panic!("Expected mem/status response, got {:?}", other),
    };
    assert_eq!(status_resp.id, status_id);
    let status_val = status_resp.response_result.unwrap();
    assert_eq!(status_val["rootExists"], true);
    assert_eq!(status_val["nodeCount"], 2);
    assert_eq!(status_val["openDocuments"], 0);
    assert_eq!(status_val["version"], env!("CARGO_PKG_VERSION"));

    // 3. Request mem/resolveReference for existing node
    let resolve_id = lsp_server::RequestId::from(3);
    client_conn
        .sender
        .send(lsp_server::Message::Request(lsp_server::Request::new(
            resolve_id.clone(),
            "mem/resolveReference".to_string(),
            serde_json::json!({ "target": "task-custom" }),
        )))
        .unwrap();

    let resolve_resp = match client_conn.receiver.recv().unwrap() {
        lsp_server::Message::Response(r) => r,
        other => panic!("Expected mem/resolveReference response, got {:?}", other),
    };
    assert_eq!(resolve_resp.id, resolve_id);
    let resolved_val = resolve_resp.response_result.unwrap();
    assert_eq!(resolved_val["id"], "task-custom");
    assert_eq!(resolved_val["label"], "Custom Resolve Target");
    assert_eq!(resolved_val["type"], "task");
    assert_eq!(resolved_val["status"], "in_progress");
    assert_eq!(resolved_val["intent"], 2);
    assert_eq!(resolved_val["parent"], "epic-root");
    assert!(resolved_val["uri"]
        .as_str()
        .unwrap()
        .ends_with("task-custom.md"));
    assert!(resolved_val["previewMarkdown"]
        .as_str()
        .unwrap()
        .contains("Custom Resolve Target"));

    // 4. Request mem/resolveReference for non-existent node -> null
    let missing_id = lsp_server::RequestId::from(4);
    client_conn
        .sender
        .send(lsp_server::Message::Request(lsp_server::Request::new(
            missing_id.clone(),
            "mem/resolveReference".to_string(),
            serde_json::json!({ "target": "nonexistent-node-id" }),
        )))
        .unwrap();

    let missing_resp = match client_conn.receiver.recv().unwrap() {
        lsp_server::Message::Response(r) => r,
        other => panic!("Expected mem/resolveReference response, got {:?}", other),
    };
    assert_eq!(missing_resp.id, missing_id);
    assert!(missing_resp.response_result.unwrap().is_null());

    // 5. Shutdown
    let shutdown_id = lsp_server::RequestId::from(5);
    client_conn
        .sender
        .send(lsp_server::Message::Request(lsp_server::Request::new(
            shutdown_id,
            "shutdown".to_string(),
            serde_json::Value::Null,
        )))
        .unwrap();
    let _ = client_conn.receiver.recv().unwrap();
    client_conn
        .sender
        .send(lsp_server::Message::Notification(
            lsp_server::Notification::new("exit".to_string(), serde_json::Value::Null),
        ))
        .unwrap();
    drop(client_conn);
    server_handle.join().unwrap();
}

#[test]
fn test_document_link_resolves_markdown_links_and_wikilinks() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();

    let tasks_dir = pkb_root.join("tasks");
    fs::create_dir_all(&tasks_dir).unwrap();
    fs::write(
        tasks_dir.join("task-wiki.md"),
        "---\nid: task-wiki\ntitle: Wiki Task\ntype: task\nstatus: ready\n---\nWiki body.\n",
    )
    .unwrap();
    fs::write(
        tasks_dir.join("task-md.md"),
        "---\nid: task-md\ntitle: Markdown Link Task\ntype: task\nstatus: ready\n---\nMD body.\n",
    )
    .unwrap();

    let mut server = LspServer::new(pkb_root.clone());
    let doc_path = pkb_root.join("notes").join("links.md");
    let doc_uri: Uri = format!("file://{}", doc_path.to_string_lossy())
        .parse()
        .unwrap();
    let doc_text = "See [[task-wiki]] and [MD Link](task-md) and [External](https://example.com) and [Unresolved](missing-id).\n";
    server.handle_did_open(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: doc_uri.clone(),
            language_id: "markdown".to_string(),
            version: 1,
            text: doc_text.to_string(),
        },
    });

    let links = server.handle_document_link(&DocumentLinkParams {
        text_document: TextDocumentIdentifier { uri: doc_uri },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    });

    assert_eq!(
        links.len(),
        2,
        "Expected 2 resolved document links (1 wikilink + 1 markdown link), got: {:?}",
        links
    );
    assert!(links[0]
        .target
        .as_ref()
        .unwrap()
        .as_str()
        .ends_with("task-wiki.md"));
    assert!(links[1]
        .target
        .as_ref()
        .unwrap()
        .as_str()
        .ends_with("task-md.md"));
}

#[test]
fn test_lsp_cli_warns_on_missing_root_directory() {
    let dir = tempdir().unwrap();
    let missing_root = dir.path().join("does_not_exist_pkb_root");

    let pkb_exe = env!("CARGO_BIN_EXE_pkb");
    let output = std::process::Command::new(pkb_exe)
        .args(["lsp", "--root", missing_root.to_str().unwrap(), "--stdio"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("Failed to execute pkb lsp");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "warning: PKB root directory does not exist: {}",
            missing_root.display()
        )),
        "Expected missing root warning in stderr, got: {}",
        stderr
    );
}

#[test]
fn test_hover_top_bar_renders_codicons_and_compact_labels() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();

    let tasks_dir = pkb_root.join("tasks");
    fs::create_dir_all(&tasks_dir).unwrap();
    let task_path = tasks_dir.join("task-compact.md");
    let task_content = r#"---
id: task-compact
title: Compact Card Task
type: task
status: ready
intent: 1
tags:
  - lsp
  - vscode
---
Body content here.
"#;
    fs::write(&task_path, task_content).unwrap();

    let mut server = LspServer::new(pkb_root.clone());
    let doc_path = pkb_root.join("notes").join("ref.md");
    let doc_uri: Uri = format!("file://{}", doc_path.to_string_lossy())
        .parse()
        .unwrap();
    let doc_text = "See [[task-compact]].\n";
    server.handle_did_open(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: doc_uri.clone(),
            language_id: "markdown".to_string(),
            version: 1,
            text: doc_text.to_string(),
        },
    });

    let hover_params = HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: doc_uri },
            position: Position {
                line: 0,
                character: 8,
            },
        },
        work_done_progress_params: Default::default(),
    };

    let hover = server
        .handle_hover(&hover_params)
        .expect("Hover must resolve");
    let hover_text = match hover.contents {
        HoverContents::Markup(MarkupContent { value, .. }) => value,
        other => panic!("Expected MarkupContent, got {:?}", other),
    };

    // Assert top bar contains codicons and compact labels
    assert!(
        hover_text.contains("$(checklist) task"),
        "Top bar must contain type codicon and compact label '$(checklist) task', got:\n{hover_text}"
    );
    assert!(
        hover_text.contains("$(play) ready"),
        "Top bar must contain status codicon and compact label '$(play) ready', got:\n{hover_text}"
    );
    assert!(
        hover_text.contains("$(flame) intent 1"),
        "Top bar must contain intent/weight codicon and compact label '$(flame) intent 1', got:\n{hover_text}"
    );
    assert!(
        hover_text.contains("$(tag)") && hover_text.contains("#lsp") && hover_text.contains("#vscode"),
        "Top bar must contain tag codicon and compact labels '$(tag)' with tags, got:\n{hover_text}"
    );

    // Assert raw legacy labels are replaced
    assert!(
        !hover_text.contains("Type:"),
        "Top bar must not contain raw 'Type:' text, got:\n{hover_text}"
    );
    assert!(
        !hover_text.contains("Status:"),
        "Top bar must not contain raw 'Status:' text, got:\n{hover_text}"
    );
    assert!(
        !hover_text.contains("Intent:"),
        "Top bar must not contain raw 'Intent:' text, got:\n{hover_text}"
    );
    assert!(
        !hover_text.contains("Tags:"),
        "Top bar must not contain raw 'Tags:' text, got:\n{hover_text}"
    );
}

#[test]
fn test_hover_referenced_nodes_render_as_human_titles_linked() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();

    let tasks_dir = pkb_root.join("tasks");
    fs::create_dir_all(&tasks_dir).unwrap();

    // 1. Create parent task
    let parent_path = tasks_dir.join("parent-epic.md");
    fs::write(
        &parent_path,
        r#"---
id: parent-epic
title: Parent Epic Title
type: task
status: ready
---
Parent description.
"#,
    )
    .unwrap();

    // 2. Create dependency task
    let dep_path = tasks_dir.join("dep-task.md");
    fs::write(
        &dep_path,
        r#"---
id: dep-task
title: Dependency Human Title
type: task
status: ready
---
Dependency description.
"#,
    )
    .unwrap();

    // 3. Create hovered task with parent and wikilink in body
    let task_path = tasks_dir.join("task-referencing.md");
    fs::write(
        &task_path,
        r#"---
id: task-referencing
title: Referencing Task
type: task
parent: parent-epic
status: ready
---
Check out [[dep-task]] for implementation details.
"#,
    )
    .unwrap();

    let mut server = LspServer::new(pkb_root.clone());
    let doc_path = pkb_root.join("notes").join("ref.md");
    let doc_uri: Uri = format!("file://{}", doc_path.to_string_lossy())
        .parse()
        .unwrap();
    let doc_text = "See [[task-referencing]].\n";
    server.handle_did_open(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: doc_uri.clone(),
            language_id: "markdown".to_string(),
            version: 1,
            text: doc_text.to_string(),
        },
    });

    let hover_params = HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: doc_uri },
            position: Position {
                line: 0,
                character: 8,
            },
        },
        work_done_progress_params: Default::default(),
    };

    let hover = server
        .handle_hover(&hover_params)
        .expect("Hover must resolve");
    let hover_text = match hover.contents {
        HoverContents::Markup(MarkupContent { value, .. }) => value,
        other => panic!("Expected MarkupContent, got {:?}", other),
    };

    // Assert parent rendered with human title linked to open node
    let parent_expected_link = format!("[Parent Epic Title](file://{})", parent_path.display());
    assert!(
        hover_text.contains(&parent_expected_link),
        "Hover card must contain parent human title linked to file: {parent_expected_link}, got:\n{hover_text}"
    );

    // Assert body wikilink rendered with target human title linked to open node
    let dep_expected_link = format!("[Dependency Human Title](file://{})", dep_path.display());
    assert!(
        hover_text.contains(&dep_expected_link),
        "Hover card must contain dep human title linked to file: {dep_expected_link}, got:\n{hover_text}"
    );

    // Assert no raw [[id]] appears in the card
    assert!(
        !hover_text.contains("[[parent-epic]]"),
        "Hover card must not contain raw '[[parent-epic]]', got:\n{hover_text}"
    );
    assert!(
        !hover_text.contains("[[dep-task]]"),
        "Hover card must not contain raw '[[dep-task]]', got:\n{hover_text}"
    );
}

#[test]
fn test_hover_body_headings_render_as_bold_inline_micro_headers() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();

    let tasks_dir = pkb_root.join("tasks");
    fs::create_dir_all(&tasks_dir).unwrap();

    let task_path = tasks_dir.join("task-headings.md");
    fs::write(
        &task_path,
        r#"---
id: task-headings
title: Headings Task
type: task
status: ready
---
## Goal

Deliver a compact card layout for the PKB hover.

## Acceptance criteria

- [ ] Item 1
- [ ] Item 2
"#,
    )
    .unwrap();

    let mut server = LspServer::new(pkb_root.clone());
    let doc_path = pkb_root.join("notes").join("ref.md");
    let doc_uri: Uri = format!("file://{}", doc_path.to_string_lossy())
        .parse()
        .unwrap();
    let doc_text = "See [[task-headings]].\n";
    server.handle_did_open(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: doc_uri.clone(),
            language_id: "markdown".to_string(),
            version: 1,
            text: doc_text.to_string(),
        },
    });

    let hover_params = HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: doc_uri },
            position: Position {
                line: 0,
                character: 8,
            },
        },
        work_done_progress_params: Default::default(),
    };

    let hover = server
        .handle_hover(&hover_params)
        .expect("Hover must resolve");
    let hover_text = match hover.contents {
        HoverContents::Markup(MarkupContent { value, .. }) => value,
        other => panic!("Expected MarkupContent, got {:?}", other),
    };

    // Assert headings render as bold inline micro-headers
    assert!(
        hover_text.contains("**Goal:** Deliver a compact card layout for the PKB hover."),
        "Heading ## Goal followed by text must render as bold inline micro-header '**Goal:** <text>', got:\n{hover_text}"
    );
    assert!(
        hover_text.contains("**Acceptance criteria:**"),
        "Heading ## Acceptance criteria must render as bold inline micro-header '**Acceptance criteria:**', got:\n{hover_text}"
    );
    assert!(
        hover_text.contains("- [ ] Item 1"),
        "Checklist items must be preserved, got:\n{hover_text}"
    );

    // Assert raw markdown headings ## are replaced
    assert!(
        !hover_text.contains("## Goal"),
        "Hover card must not contain raw '## Goal', got:\n{hover_text}"
    );
    assert!(
        !hover_text.contains("## Acceptance criteria"),
        "Hover card must not contain raw '## Acceptance criteria', got:\n{hover_text}"
    );
}

#[test]
fn test_hover_action_footer_contains_extension_command_links() {
    let dir = tempdir().unwrap();
    let pkb_root = dir.path().to_path_buf();

    let tasks_dir = pkb_root.join("tasks");
    fs::create_dir_all(&tasks_dir).unwrap();

    let task_path = tasks_dir.join("task-footer.md");
    fs::write(
        &task_path,
        r#"---
id: task-footer
title: Action Footer Task
type: task
status: ready
---
Footer test content.
"#,
    )
    .unwrap();

    let mut server = LspServer::new(pkb_root.clone());
    let doc_path = pkb_root.join("notes").join("ref.md");
    let doc_uri: Uri = format!("file://{}", doc_path.to_string_lossy())
        .parse()
        .unwrap();
    let doc_text = "See [[task-footer]].\n";
    server.handle_did_open(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: doc_uri.clone(),
            language_id: "markdown".to_string(),
            version: 1,
            text: doc_text.to_string(),
        },
    });

    let hover_params = HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: doc_uri },
            position: Position {
                line: 0,
                character: 8,
            },
        },
        work_done_progress_params: Default::default(),
    };

    let hover = server
        .handle_hover(&hover_params)
        .expect("Hover must resolve");
    let hover_text = match hover.contents {
        HoverContents::Markup(MarkupContent { value, .. }) => value,
        other => panic!("Expected MarkupContent, got {:?}", other),
    };

    // Assert action footer has command links for open and subtree view
    assert!(
        hover_text.contains("command:mem.openReference?"),
        "Hover footer must contain command:mem.openReference link, got:\n{hover_text}"
    );
    assert!(
        hover_text.contains("command:mem.showSubtree?"),
        "Hover footer must contain command:mem.showSubtree link, got:\n{hover_text}"
    );
    assert!(
        hover_text.contains("task-footer"),
        "Command links must target the hovered node id 'task-footer', got:\n{hover_text}"
    );
}
