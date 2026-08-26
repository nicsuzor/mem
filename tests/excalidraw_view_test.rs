use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::NamedTempFile;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// Configure a `Command` with a kernel-level backstop (`PR_SET_PDEATHSIG` on Linux)
/// so that the child process is automatically terminated by the kernel if the
/// parent harness process dies abnormally (e.g. SIGKILL, OOM, panic abort, CI timeout).
fn kill_on_parent_death(cmd: &mut Command) -> &mut Command {
    #[cfg(target_os = "linux")]
    unsafe {
        cmd.pre_exec(|| {
            let parent_pid = libc::getppid();
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent_pid {
                libc::_exit(1);
            }
            Ok(())
        });
    }
    cmd
}

fn run_bin(args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_excalidraw-view"));
    kill_on_parent_death(&mut cmd);
    let output = cmd
        .args(args)
        .output()
        .expect("Failed to execute excalidraw-view binary");
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (code, stdout, stderr)
}

fn run_bin_stdin(args: &[&str], input: &str) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_excalidraw-view"));
    kill_on_parent_death(&mut cmd);
    let mut child = cmd
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn excalidraw-view binary");

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).unwrap();
    }

    let output = child.wait_with_output().expect("Failed to wait on child");
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (code, stdout, stderr)
}

#[test]
fn test_help_and_no_args() {
    let (code, _stdout, stderr) = run_bin(&[]);
    assert_eq!(code, 1);
    assert!(stderr.contains("Usage: excalidraw-view"));

    let (code, stdout, _stderr) = run_bin(&["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Usage: excalidraw-view"));

    let (code, stdout, _stderr) = run_bin(&["-h"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Usage: excalidraw-view"));
}

#[test]
fn test_summary_mode_default_and_explicit() {
    let sample = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "r1",
                "type": "rectangle",
                "x": 100,
                "y": 200,
                "width": 50,
                "height": 60,
                "index": "a0",
                "isDeleted": false
            },
            {
                "id": "t1",
                "type": "text",
                "x": 110,
                "y": 210,
                "width": 30,
                "height": 20,
                "index": "a1",
                "text": "Hello",
                "originalText": "Hello",
                "isDeleted": false
            }
        ]
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), sample).unwrap();
    let path = file.path().to_str().unwrap();

    // Default mode is summary
    let (code, stdout, stderr) = run_bin(&[path]);
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert!(stdout.contains("elements: 2  types: {'rectangle': 1, 'text': 1}"));
    assert!(stdout.contains("extents: x [100, 150]  y [200, 260]"));
    assert!(stdout.contains("max index: a1"));
    assert!(stdout.contains("array order == index order: True"));

    // Explicit summary mode
    let (code2, stdout2, _) = run_bin(&[path, "summary"]);
    assert_eq!(code2, 0);
    assert_eq!(stdout, stdout2);
}

#[test]
fn test_summary_empty_elements() {
    let sample = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": []
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), sample).unwrap();
    let path = file.path().to_str().unwrap();

    let (code, stdout, _) = run_bin(&[path, "summary"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("elements: 0  types: {}"));
    assert!(stdout.contains("extents: x [0, 0]  y [0, 0]"));
    assert!(stdout.contains("max index: "));
}

#[test]
fn test_map_mode_with_arrow_labels_task_3bff27d4() {
    let sample = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "r1",
                "type": "rectangle",
                "x": 10,
                "y": 20,
                "width": 100,
                "height": 50,
                "backgroundColor": "#ffc9c9",
                "index": "a0",
                "boundElements": [{ "id": "t1", "type": "text" }]
            },
            {
                "id": "t1",
                "type": "text",
                "x": 15,
                "y": 25,
                "width": 80,
                "height": 30,
                "index": "a1",
                "text": "Box Label\nSubtext",
                "originalText": "Box Label\nSubtext",
                "containerId": "r1"
            },
            {
                "id": "a1",
                "type": "arrow",
                "x": 110,
                "y": 45,
                "width": 50,
                "height": 0,
                "index": "a2",
                "startBinding": { "elementId": "r1" },
                "endBinding": { "elementId": "r2" },
                "boundElements": [{ "id": "t2", "type": "text" }]
            },
            {
                "id": "t2",
                "type": "text",
                "x": 130,
                "y": 40,
                "width": 40,
                "height": 20,
                "index": "a3",
                "text": "Flow\nTransition",
                "originalText": "Flow\nTransition",
                "containerId": "a1"
            },
            {
                "id": "a2",
                "type": "arrow",
                "x": 110,
                "y": 50,
                "width": 50,
                "height": 0,
                "index": "a4",
                "startBinding": { "elementId": "r1" },
                "endBinding": { "elementId": "r2" }
            },
            {
                "id": "r2",
                "type": "rectangle",
                "x": 200,
                "y": 20,
                "width": 100,
                "height": 50,
                "backgroundColor": "transparent",
                "index": "a5"
            },
            {
                "id": "t3",
                "type": "text",
                "x": 350,
                "y": 50,
                "width": 60,
                "height": 20,
                "index": "a6",
                "text": "Standalone\nText",
                "originalText": "Standalone\nText"
            }
        ]
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), sample).unwrap();
    let path = file.path().to_str().unwrap();

    let (code, stdout, _) = run_bin(&[path, "map"]);
    assert_eq!(code, 0);

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 5);

    // 1. r1 rectangle with label
    assert_eq!(lines[0], "r1\trectangle\t10,20\t100x50\t#ffc9c9\tBox Label / Subtext");
    // 2. a1 arrow with label (BUG FIX: task_3bff27d4)
    assert_eq!(lines[1], "a1\tarrow\tr1 -> r2\tFlow / Transition");
    // 3. a2 arrow without label
    assert_eq!(lines[2], "a2\tarrow\tr1 -> r2");
    // 4. r2 rectangle without label
    assert_eq!(lines[3], "r2\trectangle\t200,20\t100x50\ttransparent\t");
    // 5. t3 free text
    assert_eq!(lines[4], "t3\ttext\t350,50\tStandalone / Text");
}

#[test]
fn test_style_mode() {
    let sample = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "r1",
                "type": "rectangle",
                "roughness": 1,
                "fillStyle": "solid",
                "strokeStyle": "solid",
                "strokeWidth": 2,
                "fontFamily": 1,
                "fontSize": 20,
                "opacity": 100,
                "roundness": { "type": 3 }
            },
            {
                "id": "r2",
                "type": "rectangle",
                "roughness": 1,
                "fillStyle": "hachure",
                "strokeStyle": "dashed",
                "strokeWidth": 1,
                "fontFamily": 2,
                "fontSize": 16,
                "opacity": 80,
                "roundness": null
            },
            {
                "id": "r3",
                "type": "rectangle",
                "roughness": 2,
                "fillStyle": "solid",
                "strokeStyle": "solid",
                "strokeWidth": 2,
                "fontFamily": 1,
                "fontSize": 20,
                "opacity": 100,
                "roundness": { "type": 3 }
            }
        ]
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), sample).unwrap();
    let path = file.path().to_str().unwrap();

    let (code, stdout, _) = run_bin(&[path, "style"]);
    assert_eq!(code, 0);

    assert!(stdout.contains("roughness: modal=1  all={1: 2, 2: 1}"));
    assert!(stdout.contains("fillStyle: modal=solid  all={'solid': 2, 'hachure': 1}"));
    assert!(stdout.contains("strokeStyle: modal=solid  all={'solid': 2, 'dashed': 1}"));
    assert!(stdout.contains("strokeWidth: modal=2  all={2: 2, 1: 1}"));
    assert!(stdout.contains("fontFamily: modal=1  all={1: 2, 2: 1}"));
    assert!(stdout.contains("fontSize: modal=20  all={20: 2, 16: 1}"));
    assert!(stdout.contains("opacity: modal=100  all={100: 2, 80: 1}"));
    assert!(stdout.contains(r#"roundness (rectangles): modal={"type":3}"#));
}

#[test]
fn test_check_mode_success() {
    let sample = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "r1",
                "type": "rectangle",
                "index": "a0",
                "boundElements": [{ "id": "t1", "type": "text" }]
            },
            {
                "id": "t1",
                "type": "text",
                "index": "a1",
                "text": "Hello world",
                "originalText": "Hello\nworld",
                "containerId": "r1"
            },
            {
                "id": "r2",
                "type": "rectangle",
                "index": "a2"
            },
            {
                "id": "a1",
                "type": "arrow",
                "index": "a3",
                "startBinding": { "elementId": "r1" },
                "endBinding": { "elementId": "r2" }
            },
            {
                "id": "a_decorative",
                "type": "arrow",
                "index": "a4"
            }
        ]
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), sample).unwrap();
    let path = file.path().to_str().unwrap();

    let (code, stdout, _) = run_bin(&[path, "check"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("OK: 5 elements, ids unique, index-sorted, all bindings resolve"));
}

#[test]
fn test_check_mode_half_bound_arrow_task_737c102e() {
    let sample = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "r1",
                "type": "rectangle",
                "index": "a0"
            },
            {
                "id": "a1",
                "type": "arrow",
                "index": "a1",
                "startBinding": { "elementId": "r1" }
            }
        ]
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), sample).unwrap();
    let path = file.path().to_str().unwrap();

    let (code, stdout, _) = run_bin(&[path, "check"]);
    assert_eq!(code, 1);
    assert!(stdout.contains("FAIL"));
    assert!(stdout.contains("arrow a1 is half-bound: startBinding=r1, endBinding=none"));
}

#[test]
fn test_check_mode_all_failure_cases() {
    let sample = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "dup",
                "type": "rectangle",
                "index": "a0"
            },
            {
                "id": "dup",
                "type": "rectangle",
                "index": "a0"
            },
            {
                "id": "no_idx",
                "type": "rectangle"
            },
            {
                "id": "dangling_arr",
                "type": "arrow",
                "index": "a1",
                "startBinding": { "elementId": "ghost1" },
                "endBinding": { "elementId": "ghost2" }
            },
            {
                "id": "t_orphan_container",
                "type": "text",
                "index": "a2",
                "containerId": "ghost_box",
                "text": "Alpha",
                "originalText": "Beta"
            },
            {
                "id": "r_no_backref",
                "type": "rectangle",
                "index": "a3"
            },
            {
                "id": "t_no_backref",
                "type": "text",
                "index": "a4",
                "containerId": "r_no_backref",
                "text": "Same",
                "originalText": "Same"
            },
            {
                "id": "r_dangling_bound",
                "type": "rectangle",
                "index": "a5",
                "boundElements": [{ "id": "ghost_bound", "type": "text" }]
            }
        ]
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), sample).unwrap();
    let path = file.path().to_str().unwrap();

    let (code, stdout, _) = run_bin(&[path, "check"]);
    assert_eq!(code, 1);
    assert!(stdout.contains("FAIL"));
    assert!(stdout.contains("duplicate ids: ['dup']"));
    assert!(stdout.contains("duplicate indices: ['a0']"));
    assert!(stdout.contains("elements missing index: ['no_idx']"));
    assert!(stdout.contains("dangling_arr.startBinding -> missing element ghost1"));
    assert!(stdout.contains("dangling_arr.endBinding -> missing element ghost2"));
    assert!(stdout.contains("text t_orphan_container bound to missing container ghost_box"));
    assert!(stdout.contains("container r_no_backref lacks boundElements backref to text t_no_backref"));
    assert!(stdout.contains("r_dangling_bound.boundElements -> missing element ghost_bound"));
    assert!(stdout.contains("t_orphan_container: text and originalText disagree in content"));
}

#[test]
fn test_diff_mode() {
    let sample1 = r##"{
        "type": "excalidraw",
        "elements": [
            { "id": "r1", "type": "rectangle", "x": 10, "y": 20, "width": 100, "height": 50, "backgroundColor": "transparent", "text": "Box 1" },
            { "id": "r2", "type": "rectangle", "x": 200, "y": 20, "width": 100, "height": 50, "backgroundColor": "#ff0000" },
            { "id": "a1", "type": "arrow", "x": 110, "y": 45, "width": 50, "height": 0, "startBinding": { "elementId": "r1" }, "endBinding": { "elementId": "r2" } }
        ]
    }"##;
    let sample2 = r##"{
        "type": "excalidraw",
        "elements": [
            { "id": "r1", "type": "rectangle", "x": 15, "y": 20, "width": 120, "height": 50, "backgroundColor": "#00ff00", "text": "Box One" },
            { "id": "a1", "type": "arrow", "x": 110, "y": 45, "width": 50, "height": 0, "startBinding": { "elementId": "r1" }, "endBinding": { "elementId": "r3" } },
            { "id": "r3", "type": "ellipse", "x": 250, "y": 20, "width": 80, "height": 80, "backgroundColor": "#0000ff" }
        ]
    }"##;

    let file1 = NamedTempFile::new().unwrap();
    fs::write(file1.path(), sample1).unwrap();
    let file2 = NamedTempFile::new().unwrap();
    fs::write(file2.path(), sample2).unwrap();

    let (code, stdout, _) = run_bin(&[file1.path().to_str().unwrap(), "diff", file2.path().to_str().unwrap()]);
    assert_eq!(code, 0);

    assert!(stdout.contains("Summary Diff: 3 elements -> 3 elements (delta: +0)"));
    assert!(stdout.contains("Type histogram changes:"));
    assert!(stdout.contains("  ellipse: 0 -> 1 (+1)"));
    assert!(stdout.contains("  rectangle: 2 -> 1 (-1)"));
    assert!(stdout.contains("Color histogram changes:"));
    assert!(stdout.contains("Disappeared elements (1):"));
    assert!(stdout.contains("  - [rectangle r2] pos=(200,20)"));
    assert!(stdout.contains("Added elements (1):"));
    assert!(stdout.contains("  - [ellipse r3] pos=(250,20)"));
    assert!(stdout.contains("Modified elements (1):"));
    assert!(stdout.contains("text: \"Box 1\" -> \"Box One\""));
    assert!(stdout.contains("position shifted by (+5,+0)"));
    assert!(stdout.contains("size: 100x50 -> 120x50"));
    assert!(stdout.contains("bg: transparent -> #00ff00"));
    assert!(stdout.contains("Topology / Arrow binding changes (1):"));
    assert!(stdout.contains("  - [arrow a1] start: r1 -> r1, end: r2 -> r3"));
}

#[test]
fn test_lib_and_item_mode() {
    let lib_sample = r##"{
        "type": "excalidrawlib",
        "version": 2,
        "source": "https://excalidraw.com",
        "libraryItems": [
            {
                "id": "lib1",
                "status": "published",
                "name": "Server Box",
                "elements": [
                    {
                        "id": "e1",
                        "type": "rectangle",
                        "x": 100,
                        "y": 200,
                        "width": 120,
                        "height": 80,
                        "index": "a0",
                        "groupIds": ["g1"],
                        "boundElements": [{ "id": "e2", "type": "text" }]
                    },
                    {
                        "id": "e2",
                        "type": "text",
                        "x": 110,
                        "y": 210,
                        "width": 100,
                        "height": 30,
                        "index": "a1",
                        "text": "Server",
                        "groupIds": ["g1"],
                        "containerId": "e1"
                    }
                ]
            },
            {
                "id": "lib2",
                "status": "published",
                "name": "Database Node",
                "elements": [
                    {
                        "id": "e3",
                        "type": "ellipse",
                        "x": 300,
                        "y": 400,
                        "width": 60,
                        "height": 60,
                        "index": "a0"
                    }
                ]
            }
        ]
    }"##;

    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), lib_sample).unwrap();
    let path = file.path().to_str().unwrap();

    // lib command
    let (code, stdout, _) = run_bin(&[path, "lib"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("2 items  (format v2, source https://excalidraw.com)"));
    assert!(stdout.contains("#0\tServer Box\t2 els\t120x80\trectangle+text"));
    assert!(stdout.contains("#1\tDatabase Node\t1 els\t60x60\tellipse"));

    // item command by name
    let (code2, stdout2, stderr2) = run_bin(&[path, "item", "Server", "--after", "a5", "--at", "500,600"]);
    assert_eq!(code2, 0, "stderr: {}", stderr2);

    let parsed: serde_json::Value = serde_json::from_str(&stdout2).expect("Output should be valid JSON");
    let arr = parsed.as_array().expect("Output should be array");
    assert_eq!(arr.len(), 2);

    // Verify properties of item 0 (rectangle)
    let r = &arr[0];
    assert_eq!(r["type"], "rectangle");
    assert_eq!(r["x"], 500.0);
    assert_eq!(r["y"], 600.0);
    assert_eq!(r["index"], "a500");
    assert_eq!(r["version"], 1);
    assert_eq!(r["isDeleted"], false);
    let r_id = r["id"].as_str().unwrap();
    assert_eq!(r_id.len(), 21);

    // Verify properties of item 1 (text)
    let t = &arr[1];
    assert_eq!(t["type"], "text");
    assert_eq!(t["x"], 510.0);
    assert_eq!(t["y"], 610.0);
    assert_eq!(t["index"], "a501");
    assert_eq!(t["text"], "Server");
    assert_eq!(t["containerId"], r_id); // Remapped containerId!

    // Verify boundElements backref remapped
    let bound_id = r["boundElements"][0]["id"].as_str().unwrap();
    assert_eq!(bound_id, t["id"].as_str().unwrap());

    // Verify group remapping
    let r_group = r["groupIds"][0].as_str().unwrap();
    let t_group = t["groupIds"][0].as_str().unwrap();
    assert_eq!(r_group, t_group);
    assert_ne!(r_group, "g1");
    assert_eq!(r_group.len(), 21);

    // item command by #1
    let (code3, stdout3, _) = run_bin(&[path, "item", "#1", "--after", "b0"]);
    assert_eq!(code3, 0);
    let parsed3: serde_json::Value = serde_json::from_str(&stdout3).unwrap();
    assert_eq!(parsed3.as_array().unwrap().len(), 1);
    assert_eq!(parsed3[0]["index"], "b000");
}

// ============================================================================
// New Extended Tests: struct-diff, nodes/edges/inspect/get, CRUD, theme, overlap, arrows-check
// ============================================================================

#[test]
fn test_struct_diff_mode() {
    let sample1 = r##"{
        "type": "excalidraw",
        "elements": [
            {
                "id": "node_auth",
                "type": "rectangle",
                "x": 100,
                "y": 100,
                "width": 140,
                "height": 60,
                "index": "a0",
                "customData": { "role": "emphasis" },
                "boundElements": [{ "id": "t_auth", "type": "text" }]
            },
            {
                "id": "t_auth",
                "type": "text",
                "index": "a1",
                "containerId": "node_auth",
                "text": "Auth Service",
                "originalText": "Auth Service"
            },
            {
                "id": "node_db",
                "type": "rectangle",
                "x": 400,
                "y": 100,
                "width": 140,
                "height": 60,
                "index": "a2",
                "customData": { "role": "surface" },
                "boundElements": [{ "id": "t_db", "type": "text" }, { "id": "edge_auth_db", "type": "arrow" }]
            },
            {
                "id": "t_db",
                "type": "text",
                "index": "a3",
                "containerId": "node_db",
                "text": "SQL Database",
                "originalText": "SQL Database"
            },
            {
                "id": "node_old",
                "type": "rectangle",
                "x": 100,
                "y": 300,
                "width": 100,
                "height": 50,
                "index": "a4",
                "boundElements": [{ "id": "t_old", "type": "text" }]
            },
            {
                "id": "t_old",
                "type": "text",
                "index": "a5",
                "containerId": "node_old",
                "text": "Deprecated Service",
                "originalText": "Deprecated Service"
            },
            {
                "id": "edge_auth_db",
                "type": "arrow",
                "index": "a6",
                "startBinding": { "elementId": "node_auth" },
                "endBinding": { "elementId": "node_db" },
                "boundElements": [{ "id": "t_edge", "type": "text" }]
            },
            {
                "id": "t_edge",
                "type": "text",
                "index": "a7",
                "containerId": "edge_auth_db",
                "text": "queries",
                "originalText": "queries"
            }
        ]
    }"##;

    let sample2 = r##"{
        "type": "excalidraw",
        "elements": [
            {
                "id": "node_auth",
                "type": "rectangle",
                "x": 100,
                "y": 100,
                "width": 140,
                "height": 60,
                "index": "a0",
                "customData": { "role": "info" },
                "boundElements": [{ "id": "t_auth", "type": "text" }]
            },
            {
                "id": "t_auth",
                "type": "text",
                "index": "a1",
                "containerId": "node_auth",
                "text": "Authentication Gateway",
                "originalText": "Authentication Gateway"
            },
            {
                "id": "node_db",
                "type": "rectangle",
                "x": 400,
                "y": 100,
                "width": 140,
                "height": 60,
                "index": "a2",
                "customData": { "role": "surface" },
                "boundElements": [{ "id": "t_db", "type": "text" }]
            },
            {
                "id": "t_db",
                "type": "text",
                "index": "a3",
                "containerId": "node_db",
                "text": "SQL Database",
                "originalText": "SQL Database"
            },
            {
                "id": "node_cache",
                "type": "rectangle",
                "x": 400,
                "y": 300,
                "width": 120,
                "height": 50,
                "index": "a4",
                "customData": { "role": "success" },
                "boundElements": [{ "id": "t_cache", "type": "text" }]
            },
            {
                "id": "t_cache",
                "type": "text",
                "index": "a5",
                "containerId": "node_cache",
                "text": "Redis Cache",
                "originalText": "Redis Cache"
            },
            {
                "id": "edge_auth_db",
                "type": "arrow",
                "index": "a6",
                "startBinding": { "elementId": "node_auth" },
                "endBinding": { "elementId": "node_cache" },
                "boundElements": [{ "id": "t_edge", "type": "text" }]
            },
            {
                "id": "t_edge",
                "type": "text",
                "index": "a7",
                "containerId": "edge_auth_db",
                "text": "reads from",
                "originalText": "reads from"
            }
        ]
    }"##;

    let file1 = NamedTempFile::new().unwrap();
    fs::write(file1.path(), sample1).unwrap();
    let file2 = NamedTempFile::new().unwrap();
    fs::write(file2.path(), sample2).unwrap();

    let (code, stdout, stderr) = run_bin(&[
        file1.path().to_str().unwrap(),
        "struct-diff",
        file2.path().to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {}", stderr);

    // Added node
    assert!(stdout.contains(r#"+ node [node_cache] type="rectangle" label="Redis Cache" role="success""#));
    // Removed node
    assert!(stdout.contains(r#"- node [node_old] type="rectangle" label="Deprecated Service""#));
    // Relabeled node
    assert!(stdout.contains(r#"~ node [node_auth] relabeled: "Auth Service" -> "Authentication Gateway""#));
    // Role changed node
    assert!(stdout.contains(r#"~ node [node_auth] role: "emphasis" -> "info""#));
    // Retargeted edge
    assert!(stdout.contains("~ edge [edge_auth_db] retargeted: node_auth->node_db -> node_auth->node_cache"));
    // Relabeled edge
    assert!(stdout.contains(r#"~ edge [edge_auth_db] relabeled: "queries" -> "reads from""#));
}

#[test]
fn test_nodes_edges_inspect_and_get() {
    let sample = r##"{
        "type": "excalidraw",
        "elements": [
            {
                "id": "srv",
                "type": "rectangle",
                "x": 100,
                "y": 200,
                "width": 120,
                "height": 60,
                "index": "a0",
                "customData": { "role": "emphasis" },
                "boundElements": [{ "id": "srv_txt", "type": "text" }, { "id": "arr1", "type": "arrow" }]
            },
            {
                "id": "srv_txt",
                "type": "text",
                "x": 110,
                "y": 210,
                "width": 80,
                "height": 20,
                "index": "a1",
                "containerId": "srv",
                "text": "Server Core",
                "originalText": "Server Core"
            },
            {
                "id": "db",
                "type": "ellipse",
                "x": 400,
                "y": 200,
                "width": 80,
                "height": 80,
                "index": "a2",
                "customData": { "role": "surface" },
                "boundElements": [{ "id": "db_txt", "type": "text" }]
            },
            {
                "id": "db_txt",
                "type": "text",
                "index": "a3",
                "containerId": "db",
                "text": "Data Store",
                "originalText": "Data Store"
            },
            {
                "id": "arr1",
                "type": "arrow",
                "x": 220,
                "y": 230,
                "width": 180,
                "height": 0,
                "index": "a4",
                "startBinding": { "elementId": "srv" },
                "endBinding": { "elementId": "db" },
                "boundElements": [{ "id": "arr1_txt", "type": "text" }]
            },
            {
                "id": "arr1_txt",
                "type": "text",
                "index": "a5",
                "containerId": "arr1",
                "text": "sync",
                "originalText": "sync"
            }
        ]
    }"##;

    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), sample).unwrap();
    let path = file.path().to_str().unwrap();

    // nodes command
    let (code, stdout, _) = run_bin(&[path, "nodes"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("srv\trectangle\t100,200\t120x60\temphasis\tServer Core"));
    assert!(stdout.contains("db\tellipse\t400,200\t80x80\tsurface\tData Store"));

    // edges command
    let (code_e, stdout_e, _) = run_bin(&[path, "edges"]);
    assert_eq!(code_e, 0);
    assert!(stdout_e.contains("arr1\tsrv -> db\tsync"));

    // inspect command on node
    let (code_i, stdout_i, _) = run_bin(&[path, "inspect", "srv"]);
    assert_eq!(code_i, 0);
    assert!(stdout_i.contains("id: srv"));
    assert!(stdout_i.contains("type: rectangle"));
    assert!(stdout_i.contains("bounds: 100,200 120x60"));
    assert!(stdout_i.contains("label: Server Core"));
    assert!(stdout_i.contains("role: emphasis"));
    assert!(stdout_i.contains("outbound: [arr1]"));

    // inspect command on arrow
    let (code_ia, stdout_ia, _) = run_bin(&[path, "inspect", "arr1"]);
    assert_eq!(code_ia, 0);
    assert!(stdout_ia.contains("id: arr1"));
    assert!(stdout_ia.contains("type: arrow"));
    assert!(stdout_ia.contains("from: srv"));
    assert!(stdout_ia.contains("to: db"));
    assert!(stdout_ia.contains("label: sync"));

    // get command
    let (code_g, stdout_g, _) = run_bin(&[path, "get", "srv"]);
    assert_eq!(code_g, 0);
    let parsed: serde_json::Value = serde_json::from_str(&stdout_g).unwrap();
    assert_eq!(parsed["id"], "srv");
    assert_eq!(parsed["type"], "rectangle");
}

#[test]
fn test_crud_add_node_and_add_text() {
    let initial = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": []
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), initial).unwrap();
    let path = file.path().to_str().unwrap();

    // add-node
    let (code, stdout, stderr) = run_bin(&[
        path,
        "add-node",
        "--type",
        "rectangle",
        "--text",
        "User Auth Service",
        "--at",
        "150,250",
        "--role",
        "emphasis",
        "--id",
        "auth_node_1",
    ]);
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert!(stdout.contains("OK: created node auth_node_1"));

    // Verify document state
    let content = fs::read_to_string(path).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&content).unwrap();
    let els = doc["elements"].as_array().unwrap();
    assert_eq!(els.len(), 2); // shape + text

    let shape = &els[0];
    assert_eq!(shape["id"], "auth_node_1");
    assert_eq!(shape["type"], "rectangle");
    assert_eq!(shape["x"], 150.0);
    assert_eq!(shape["y"], 250.0);
    assert_eq!(shape["customData"]["role"], "emphasis");
    assert_eq!(shape["backgroundColor"], "#c9b458"); // Theme gold

    let text = &els[1];
    assert_eq!(text["containerId"], "auth_node_1");
    assert_eq!(text["text"], "User Auth Service");
    assert_eq!(text["originalText"], "User Auth Service");

    // add-text
    let (code2, stdout2, stderr2) = run_bin(&[
        path,
        "add-text",
        "--text",
        "Free Annotation",
        "--at",
        "500,100",
        "--font-size",
        "18",
    ]);
    assert_eq!(code2, 0, "stderr: {}", stderr2);
    assert!(stdout2.contains("OK: created text"));

    // Check check validation
    let (code_chk, stdout_chk, _) = run_bin(&[path, "check"]);
    assert_eq!(code_chk, 0);
    assert!(stdout_chk.contains("OK: 3 elements, ids unique, index-sorted, all bindings resolve"));
}

#[test]
fn test_crud_connect() {
    let initial = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "n1",
                "type": "rectangle",
                "x": 100,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a0"
            },
            {
                "id": "n2",
                "type": "rectangle",
                "x": 400,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a1"
            }
        ]
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), initial).unwrap();
    let path = file.path().to_str().unwrap();

    let (code, stdout, stderr) = run_bin(&[
        path,
        "connect",
        "--from",
        "n1",
        "--to",
        "n2",
        "--label",
        "HTTPS POST",
    ]);
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert!(stdout.contains("OK: connected n1 -> n2 with arrow"));

    // Verify arrow structure
    let content = fs::read_to_string(path).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&content).unwrap();
    let els = doc["elements"].as_array().unwrap();
    assert_eq!(els.len(), 4); // n1, n2, arrow, label text

    let arrow = els.iter().find(|e| e["type"] == "arrow").unwrap();
    assert_eq!(arrow["startBinding"]["elementId"], "n1");
    assert_eq!(arrow["endBinding"]["elementId"], "n2");

    let bound_text_id = arrow["boundElements"][0]["id"].as_str().unwrap();
    let label = els.iter().find(|e| e["id"] == bound_text_id).unwrap();
    assert_eq!(label["text"], "HTTPS POST");
    assert_eq!(label["originalText"], "HTTPS POST");
    assert_eq!(label["containerId"], arrow["id"]);

    // Check validity
    let (code_chk, stdout_chk, _) = run_bin(&[path, "check"]);
    assert_eq!(code_chk, 0, "check output: {}", stdout_chk);
}

#[test]
fn test_crud_set_text_and_fit() {
    let initial = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "box1",
                "type": "rectangle",
                "x": 100,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a0",
                "boundElements": [{ "id": "txt1", "type": "text" }]
            },
            {
                "id": "txt1",
                "type": "text",
                "x": 110,
                "y": 115,
                "width": 80,
                "height": 20,
                "index": "a1",
                "containerId": "box1",
                "text": "Short",
                "originalText": "Short"
            }
        ]
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), initial).unwrap();
    let path = file.path().to_str().unwrap();

    let long_text = "Much Longer Text That Causes The Box To Center Grow Automatically";
    let (code, stdout, stderr) = run_bin(&[path, "set-text", "box1", long_text]);
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert!(stdout.contains("OK: updated text for box1"));

    let content = fs::read_to_string(path).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&content).unwrap();
    let box1 = &doc["elements"][0];
    let txt1 = &doc["elements"][1];

    assert_eq!(txt1["text"], long_text);
    assert_eq!(txt1["originalText"], long_text);
    assert!(box1["width"].as_f64().unwrap() > 100.0);
}

#[test]
fn test_crud_move_elem() {
    let initial = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "box1",
                "type": "rectangle",
                "x": 100,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a0",
                "boundElements": [{ "id": "txt1", "type": "text" }]
            },
            {
                "id": "txt1",
                "type": "text",
                "x": 110,
                "y": 115,
                "width": 80,
                "height": 20,
                "index": "a1",
                "containerId": "box1",
                "text": "Box",
                "originalText": "Box"
            },
            {
                "id": "arr",
                "type": "arrow",
                "x": 150,
                "y": 125,
                "width": 150,
                "height": 0,
                "index": "a2",
                "points": [[0.0, 0.0], [150.0, 0.0]],
                "startBinding": { "elementId": "box1" },
                "endBinding": { "elementId": "box2" }
            },
            {
                "id": "box2",
                "type": "rectangle",
                "x": 300,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a3"
            }
        ]
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), initial).unwrap();
    let path = file.path().to_str().unwrap();

    let (code, stdout, stderr) = run_bin(&[path, "move-elem", "box1", "--by", "50,30"]);
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert!(stdout.contains("OK: moved element box1"));

    let content = fs::read_to_string(path).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&content).unwrap();
    let box1 = &doc["elements"][0];
    let txt1 = &doc["elements"][1];
    let arr = &doc["elements"][2];

    assert_eq!(box1["x"], 150.0);
    assert_eq!(box1["y"], 130.0);
    assert_eq!(txt1["x"], 160.0);
    assert_eq!(txt1["y"], 145.0);

    // Arrow start moved by (+50,+30), so arrow x,y changed and relative endpoint adjusted
    assert_eq!(arr["x"], 200.0);
    assert_eq!(arr["y"], 155.0);
    assert_eq!(arr["points"][1][0], 100.0);
    assert_eq!(arr["points"][1][1], -30.0);
}

#[test]
fn test_crud_delete_elem_and_cascade() {
    let initial = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "box1",
                "type": "rectangle",
                "x": 100,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a0",
                "boundElements": [{ "id": "txt1", "type": "text" }, { "id": "arr1", "type": "arrow" }]
            },
            {
                "id": "txt1",
                "type": "text",
                "index": "a1",
                "containerId": "box1",
                "text": "Box 1",
                "originalText": "Box 1"
            },
            {
                "id": "arr1",
                "type": "arrow",
                "index": "a2",
                "startBinding": { "elementId": "box1" },
                "endBinding": { "elementId": "box2" }
            },
            {
                "id": "box2",
                "type": "rectangle",
                "x": 300,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a3"
            }
        ]
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), initial).unwrap();
    let path = file.path().to_str().unwrap();

    let (code, stdout, stderr) = run_bin(&[path, "delete-elem", "box1", "--cascade-arrows"]);
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert!(stdout.contains("OK: deleted element box1"));

    let content = fs::read_to_string(path).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&content).unwrap();
    let els = doc["elements"].as_array().unwrap();

    // box1, txt1, and arr1 all deleted. Only box2 remains.
    assert_eq!(els.len(), 1);
    assert_eq!(els[0]["id"], "box2");

    let (code_chk, stdout_chk, _) = run_bin(&[path, "check"]);
    assert_eq!(code_chk, 0, "check: {}", stdout_chk);
}

#[test]
fn test_crud_batch_file_and_stdin() {
    let initial = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": []
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), initial).unwrap();
    let path = file.path().to_str().unwrap();

    let batch_json = r##"[
        {
            "action": "add-node",
            "type": "rectangle",
            "text": "Gateway",
            "at": [100, 100],
            "role": "emphasis",
            "id": "gw"
        },
        {
            "action": "add-node",
            "type": "rectangle",
            "text": "Worker",
            "at": [400, 100],
            "role": "success",
            "id": "wrk"
        },
        {
            "action": "connect",
            "from": "gw",
            "to": "wrk",
            "label": "dispatches"
        }
    ]"##;

    // Via stdin
    let (code, stdout, stderr) = run_bin_stdin(&[path, "batch", "-"], batch_json);
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert!(stdout.contains("OK: applied 3 batch mutations atomically"));

    let (code_chk, stdout_chk, _) = run_bin(&[path, "check"]);
    assert_eq!(code_chk, 0, "check: {}", stdout_chk);

    // Verify nodes and edge exist
    let (code_m, stdout_m, _) = run_bin(&[path, "map"]);
    assert_eq!(code_m, 0);
    assert!(stdout_m.contains("Gateway"));
    assert!(stdout_m.contains("Worker"));
    assert!(stdout_m.contains("gw -> wrk\tdispatches"));
}

#[test]
fn test_theme_export_and_apply() {
    let initial = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "n1",
                "type": "rectangle",
                "x": 100,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a0",
                "customData": { "role": "warning" }
            }
        ]
    }"##;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), initial).unwrap();
    let path = file.path().to_str().unwrap();

    // Export theme
    let (code_exp, stdout_exp, _) = run_bin(&[path, "theme", "export"]);
    assert_eq!(code_exp, 0);
    let theme_val: serde_json::Value = serde_json::from_str(&stdout_exp).unwrap();
    assert_eq!(theme_val["name"], "retro-terminal");
    assert_eq!(theme_val["roles"]["warning"], "#ffa500");

    // Apply theme
    let (code_app, stdout_app, stderr_app) = run_bin(&[path, "theme", "apply", "default"]);
    assert_eq!(code_app, 0, "stderr: {}", stderr_app);
    assert!(stdout_app.contains("OK: applied theme 'retro-terminal'"));

    let content = fs::read_to_string(path).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(doc["elements"][0]["backgroundColor"], "#ffa500");
    assert_eq!(doc["elements"][0]["fillStyle"], "hachure");
}

#[test]
fn test_overlap_detection() {
    // 1. Overlapping sibling boxes
    let colliding_sample = r##"{
        "type": "excalidraw",
        "elements": [
            {
                "id": "boxA",
                "type": "rectangle",
                "x": 100,
                "y": 100,
                "width": 100,
                "height": 60,
                "text": "Box A"
            },
            {
                "id": "boxB",
                "type": "rectangle",
                "x": 150,
                "y": 120,
                "width": 100,
                "height": 60,
                "text": "Box B"
            }
        ]
    }"##;
    let file1 = NamedTempFile::new().unwrap();
    fs::write(file1.path(), colliding_sample).unwrap();
    let (code1, stdout1, _) = run_bin(&[file1.path().to_str().unwrap(), "overlap"]);
    assert_eq!(code1, 1);
    assert!(stdout1.contains("OVERLAP DETECTED"));
    assert!(stdout1.contains("boxA (Box A) overlaps with boxB (Box B)"));

    // 2. Nested parent box (not a collision)
    let nested_sample = r##"{
        "type": "excalidraw",
        "elements": [
            {
                "id": "parent",
                "type": "rectangle",
                "x": 50,
                "y": 50,
                "width": 400,
                "height": 300,
                "text": "Zone"
            },
            {
                "id": "child",
                "type": "rectangle",
                "x": 100,
                "y": 100,
                "width": 80,
                "height": 40,
                "text": "Child"
            }
        ]
    }"##;
    let file2 = NamedTempFile::new().unwrap();
    fs::write(file2.path(), nested_sample).unwrap();
    let (code2, stdout2, _) = run_bin(&[file2.path().to_str().unwrap(), "overlap"]);
    assert_eq!(code2, 0);
    assert!(stdout2.contains("OK: No overlapping elements detected."));
}

#[test]
fn test_arrows_check_detection() {
    // 1. Arrow cutting through unrelated box
    let cutting_sample = r##"{
        "type": "excalidraw",
        "elements": [
            {
                "id": "n1",
                "type": "rectangle",
                "x": 50,
                "y": 100,
                "width": 60,
                "height": 40,
                "text": "Node 1"
            },
            {
                "id": "obstacle",
                "type": "rectangle",
                "x": 200,
                "y": 90,
                "width": 80,
                "height": 60,
                "text": "Obstacle"
            },
            {
                "id": "n2",
                "type": "rectangle",
                "x": 400,
                "y": 100,
                "width": 60,
                "height": 40,
                "text": "Node 2"
            },
            {
                "id": "arrow1",
                "type": "arrow",
                "x": 80,
                "y": 120,
                "points": [[0.0, 0.0], [350.0, 0.0]],
                "startBinding": { "elementId": "n1" },
                "endBinding": { "elementId": "n2" }
            }
        ]
    }"##;
    let file1 = NamedTempFile::new().unwrap();
    fs::write(file1.path(), cutting_sample).unwrap();
    let (code1, stdout1, _) = run_bin(&[file1.path().to_str().unwrap(), "arrows-check"]);
    assert_eq!(code1, 1);
    assert!(stdout1.contains("ARROW INTERSECTION DETECTED"));
    assert!(stdout1.contains("arrow arrow1 cuts through box obstacle (Obstacle)"));

    // 2. Clean routing
    let clean_sample = r##"{
        "type": "excalidraw",
        "elements": [
            {
                "id": "n1",
                "type": "rectangle",
                "x": 50,
                "y": 100,
                "width": 60,
                "height": 40,
                "text": "Node 1"
            },
            {
                "id": "n2",
                "type": "rectangle",
                "x": 250,
                "y": 100,
                "width": 60,
                "height": 40,
                "text": "Node 2"
            },
            {
                "id": "arrow1",
                "type": "arrow",
                "x": 110,
                "y": 120,
                "points": [[0.0, 0.0], [140.0, 0.0]],
                "startBinding": { "elementId": "n1" },
                "endBinding": { "elementId": "n2" }
            }
        ]
    }"##;
    let file2 = NamedTempFile::new().unwrap();
    fs::write(file2.path(), clean_sample).unwrap();
    let (code2, stdout2, _) = run_bin(&[file2.path().to_str().unwrap(), "arrows-check"]);
    assert_eq!(code2, 0);
    assert!(stdout2.contains("OK: No arrow collisions detected."));
}

#[test]
fn test_regression_vulnerability_1_cascade_delete_labeled_arrow() {
    // Vulnerability 1: delete-elem --cascade-arrows on a node connected to a labeled arrow
    // cleanly removes node, arrows, and arrow labels without check failures.
    let sample = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "node1",
                "type": "rectangle",
                "x": 100,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a0",
                "boundElements": [
                    { "id": "text1", "type": "text" },
                    { "id": "arrow1", "type": "arrow" }
                ]
            },
            {
                "id": "text1",
                "type": "text",
                "x": 110,
                "y": 115,
                "width": 80,
                "height": 20,
                "index": "a1",
                "containerId": "node1",
                "text": "Node 1",
                "originalText": "Node 1"
            },
            {
                "id": "arrow1",
                "type": "arrow",
                "x": 150,
                "y": 125,
                "width": 150,
                "height": 0,
                "index": "a2",
                "points": [[0.0, 0.0], [150.0, 0.0]],
                "startBinding": { "elementId": "node1" },
                "endBinding": { "elementId": "node2" },
                "boundElements": [
                    { "id": "arrow_label", "type": "text" }
                ]
            },
            {
                "id": "arrow_label",
                "type": "text",
                "x": 200,
                "y": 115,
                "width": 50,
                "height": 20,
                "index": "a3",
                "containerId": "arrow1",
                "text": "Connects",
                "originalText": "Connects"
            },
            {
                "id": "node2",
                "type": "rectangle",
                "x": 300,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a4",
                "boundElements": [
                    { "id": "text2", "type": "text" },
                    { "id": "arrow1", "type": "arrow" }
                ]
            },
            {
                "id": "text2",
                "type": "text",
                "x": 310,
                "y": 115,
                "width": 80,
                "height": 20,
                "index": "a5",
                "containerId": "node2",
                "text": "Node 2",
                "originalText": "Node 2"
            }
        ]
    }"##;

    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), sample).unwrap();
    let path = file.path().to_str().unwrap();

    // Verify initial file passes check
    let (code_init, _, _) = run_bin(&[path, "check"]);
    assert_eq!(code_init, 0);

    // Delete node1 with --cascade-arrows
    let (code, stdout, stderr) = run_bin(&[path, "delete-elem", "node1", "--cascade-arrows"]);
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert!(stdout.contains("OK: deleted element node1"));

    let content = fs::read_to_string(path).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&content).unwrap();
    let els = doc["elements"].as_array().unwrap();

    // node1, text1, arrow1, and arrow_label should all be deleted.
    // Only node2 and text2 should remain.
    assert_eq!(els.len(), 2);
    let surviving_ids: Vec<&str> = els.iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(surviving_ids, vec!["node2", "text2"]);

    // node2's boundElements must NOT contain arrow1 anymore
    let node2_bound = els[0].get("boundElements").and_then(|v| v.as_array()).unwrap();
    assert_eq!(node2_bound.len(), 1);
    assert_eq!(node2_bound[0]["id"], "text2");

    // Check mode must pass cleanly without dangling container references or bindings
    let (code_chk, stdout_chk, _) = run_bin(&[path, "check"]);
    assert_eq!(code_chk, 0, "check failed: {}", stdout_chk);
    assert!(stdout_chk.contains("OK: 2 elements"));
}

#[test]
fn test_regression_vulnerability_2_set_text_arrow_label_preserves_arrow_geometry() {
    // Vulnerability 2: set-text on arrow label text updates the text without distorting arrow geometry.
    let sample = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "node1",
                "type": "rectangle",
                "x": 50,
                "y": 100,
                "width": 80,
                "height": 40,
                "index": "a0",
                "boundElements": [{ "id": "arrow1", "type": "arrow" }]
            },
            {
                "id": "arrow1",
                "type": "arrow",
                "x": 90,
                "y": 120,
                "width": 200,
                "height": 50,
                "index": "a1",
                "points": [[0.0, 0.0], [200.0, 50.0]],
                "startBinding": { "elementId": "node1" },
                "endBinding": { "elementId": "node2" },
                "boundElements": [{ "id": "lbl1", "type": "text" }]
            },
            {
                "id": "lbl1",
                "type": "text",
                "x": 170,
                "y": 135,
                "width": 40,
                "height": 20,
                "index": "a2",
                "containerId": "arrow1",
                "text": "Short",
                "originalText": "Short"
            },
            {
                "id": "node2",
                "type": "rectangle",
                "x": 290,
                "y": 150,
                "width": 80,
                "height": 40,
                "index": "a3",
                "boundElements": [{ "id": "arrow1", "type": "arrow" }]
            }
        ]
    }"##;

    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), sample).unwrap();
    let path = file.path().to_str().unwrap();

    let long_label = "A Much Longer Arrow Label Text That Should Not Distort Arrow";
    let (code, stdout, stderr) = run_bin(&[path, "set-text", "lbl1", long_label]);
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert!(stdout.contains("OK: updated text for lbl1"));

    let content = fs::read_to_string(path).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&content).unwrap();
    let arrow1 = &doc["elements"][1];
    let lbl1 = &doc["elements"][2];

    // Arrow geometry must be completely preserved
    assert_eq!(arrow1["x"], 90.0);
    assert_eq!(arrow1["y"], 120.0);
    assert_eq!(arrow1["width"], 200.0);
    assert_eq!(arrow1["height"], 50.0);
    assert_eq!(arrow1["points"][0][0], 0.0);
    assert_eq!(arrow1["points"][0][1], 0.0);
    assert_eq!(arrow1["points"][1][0], 200.0);
    assert_eq!(arrow1["points"][1][1], 50.0);

    // Label text updated
    assert_eq!(lbl1["text"], long_label);
    assert_eq!(lbl1["originalText"], long_label);
    assert!(lbl1["width"].as_f64().unwrap() > 40.0);

    // Midpoint of arrow: (90 + 100, 120 + 25) = (190, 145)
    let tw = lbl1["width"].as_f64().unwrap();
    let th = lbl1["height"].as_f64().unwrap();
    let expected_lx = 190.0 - tw / 2.0;
    let expected_ly = 145.0 - th / 2.0;
    assert!((lbl1["x"].as_f64().unwrap() - expected_lx).abs() < 1e-3);
    assert!((lbl1["y"].as_f64().unwrap() - expected_ly).abs() < 1e-3);

    // Also test updating via arrow container directly
    let (code2, stdout2, _) = run_bin(&[path, "set-text", "arrow1", "Direct Arrow Text"]);
    assert_eq!(code2, 0);
    assert!(stdout2.contains("OK: updated text for arrow1"));

    let content2 = fs::read_to_string(path).unwrap();
    let doc2: serde_json::Value = serde_json::from_str(&content2).unwrap();
    let arrow1_after = &doc2["elements"][1];
    let lbl1_after = &doc2["elements"][2];

    assert_eq!(arrow1_after["x"], 90.0);
    assert_eq!(arrow1_after["y"], 120.0);
    assert_eq!(arrow1_after["width"], 200.0);
    assert_eq!(arrow1_after["height"], 50.0);
    assert_eq!(lbl1_after["text"], "Direct Arrow Text");

    let (code_chk, stdout_chk, _) = run_bin(&[path, "check"]);
    assert_eq!(code_chk, 0, "check failed: {}", stdout_chk);
}

#[test]
fn test_regression_vulnerability_3_move_elem_translates_arrow_labels() {
    // Vulnerability 3: move-elem moves shapes, connected arrows, and arrow labels.
    let sample = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "node1",
                "type": "rectangle",
                "x": 100,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a0",
                "boundElements": [
                    { "id": "text1", "type": "text" },
                    { "id": "arrow1", "type": "arrow" }
                ]
            },
            {
                "id": "text1",
                "type": "text",
                "x": 110,
                "y": 115,
                "width": 80,
                "height": 20,
                "index": "a1",
                "containerId": "node1",
                "text": "Node 1",
                "originalText": "Node 1"
            },
            {
                "id": "arrow1",
                "type": "arrow",
                "x": 150,
                "y": 125,
                "width": 150,
                "height": 0,
                "index": "a2",
                "points": [[0.0, 0.0], [150.0, 0.0]],
                "startBinding": { "elementId": "node1" },
                "endBinding": { "elementId": "node2" },
                "boundElements": [
                    { "id": "arrow_label", "type": "text" }
                ]
            },
            {
                "id": "arrow_label",
                "type": "text",
                "x": 205,
                "y": 115,
                "width": 40,
                "height": 20,
                "index": "a3",
                "containerId": "arrow1",
                "text": "Step",
                "originalText": "Step"
            },
            {
                "id": "node2",
                "type": "rectangle",
                "x": 300,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a4",
                "boundElements": [
                    { "id": "text2", "type": "text" },
                    { "id": "arrow1", "type": "arrow" }
                ]
            },
            {
                "id": "text2",
                "type": "text",
                "x": 310,
                "y": 115,
                "width": 80,
                "height": 20,
                "index": "a5",
                "containerId": "node2",
                "text": "Node 2",
                "originalText": "Node 2"
            }
        ]
    }"##;

    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), sample).unwrap();
    let path = file.path().to_str().unwrap();

    // 1. Move node1 by (40, 20)
    let (code1, stdout1, stderr1) = run_bin(&[path, "move-elem", "node1", "--by", "40,20"]);
    assert_eq!(code1, 0, "stderr: {}", stderr1);
    assert!(stdout1.contains("OK: moved element node1"));

    let content1 = fs::read_to_string(path).unwrap();
    let doc1: serde_json::Value = serde_json::from_str(&content1).unwrap();
    let n1 = &doc1["elements"][0];
    let t1 = &doc1["elements"][1];
    let arr = &doc1["elements"][2];
    let lbl = &doc1["elements"][3];

    assert_eq!(n1["x"], 140.0);
    assert_eq!(n1["y"], 120.0);
    assert_eq!(t1["x"], 150.0);
    assert_eq!(t1["y"], 135.0);

    // Arrow start moved to (190, 145)
    assert_eq!(arr["x"], 190.0);
    assert_eq!(arr["y"], 145.0);

    // Arrow midpoint shifted by (+20, +10) -> label shifted from (205, 115) to (225, 125)
    assert_eq!(lbl["x"], 225.0);
    assert_eq!(lbl["y"], 125.0);

    // 2. Move node2 by (0, 40)
    let (code2, stdout2, _) = run_bin(&[path, "move-elem", "node2", "--by", "0,40"]);
    assert_eq!(code2, 0);
    assert!(stdout2.contains("OK: moved element node2"));

    let content2 = fs::read_to_string(path).unwrap();
    let doc2: serde_json::Value = serde_json::from_str(&content2).unwrap();
    let lbl2 = &doc2["elements"][3];

    // Label shifted by (0, +20) -> (225, 145)
    assert_eq!(lbl2["x"], 225.0);
    assert_eq!(lbl2["y"], 145.0);

    let (code_chk, stdout_chk, _) = run_bin(&[path, "check"]);
    assert_eq!(code_chk, 0, "check failed: {}", stdout_chk);
}

#[test]
fn test_regression_vulnerability_4_non_cascading_delete_strips_survivor_bound_elements() {
    // Vulnerability 4: Non-cascading delete-elem strips survivor boundElements.
    let sample = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "node1",
                "type": "rectangle",
                "x": 100,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a0",
                "boundElements": [
                    { "id": "text1", "type": "text" },
                    { "id": "arrow1", "type": "arrow" }
                ]
            },
            {
                "id": "text1",
                "type": "text",
                "x": 110,
                "y": 115,
                "width": 80,
                "height": 20,
                "index": "a1",
                "containerId": "node1",
                "text": "Node 1",
                "originalText": "Node 1"
            },
            {
                "id": "arrow1",
                "type": "arrow",
                "x": 150,
                "y": 125,
                "width": 150,
                "height": 0,
                "index": "a2",
                "points": [[0.0, 0.0], [150.0, 0.0]],
                "startBinding": { "elementId": "node1" },
                "endBinding": { "elementId": "node2" },
                "boundElements": [
                    { "id": "arrow_label", "type": "text" }
                ]
            },
            {
                "id": "arrow_label",
                "type": "text",
                "x": 200,
                "y": 115,
                "width": 50,
                "height": 20,
                "index": "a3",
                "containerId": "arrow1",
                "text": "Label",
                "originalText": "Label"
            },
            {
                "id": "node2",
                "type": "rectangle",
                "x": 300,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a4",
                "boundElements": [
                    { "id": "text2", "type": "text" },
                    { "id": "arrow1", "type": "arrow" }
                ]
            },
            {
                "id": "text2",
                "type": "text",
                "x": 310,
                "y": 115,
                "width": 80,
                "height": 20,
                "index": "a5",
                "containerId": "node2",
                "text": "Node 2",
                "originalText": "Node 2"
            }
        ]
    }"##;

    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), sample).unwrap();
    let path = file.path().to_str().unwrap();

    // 1. Delete node1 WITHOUT --cascade-arrows
    let (code, stdout, stderr) = run_bin(&[path, "delete-elem", "node1"]);
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert!(stdout.contains("OK: deleted element node1"));

    let content = fs::read_to_string(path).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&content).unwrap();
    let els = doc["elements"].as_array().unwrap();

    // node1 and text1 deleted. arrow1, arrow_label, node2, text2 survive.
    assert_eq!(els.len(), 4);

    let arrow1 = els.iter().find(|e| e["id"] == "arrow1").unwrap();
    assert!(arrow1["startBinding"].is_null());
    assert!(arrow1["endBinding"].is_null());
    // arrow1 still retains its bound label
    assert_eq!(arrow1["boundElements"][0]["id"], "arrow_label");

    let node2 = els.iter().find(|e| e["id"] == "node2").unwrap();
    // node2's boundElements must NOT contain arrow1 anymore
    let node2_bound = node2["boundElements"].as_array().unwrap();
    assert_eq!(node2_bound.len(), 1);
    assert_eq!(node2_bound[0]["id"], "text2");

    let (code_chk, stdout_chk, _) = run_bin(&[path, "check"]);
    assert_eq!(code_chk, 0, "check failed: {}", stdout_chk);

    // 2. Now delete arrow1 directly
    let (code_del_arr, stdout_del_arr, _) = run_bin(&[path, "delete-elem", "arrow1"]);
    assert_eq!(code_del_arr, 0);
    assert!(stdout_del_arr.contains("OK: deleted element arrow1"));

    let content2 = fs::read_to_string(path).unwrap();
    let doc2: serde_json::Value = serde_json::from_str(&content2).unwrap();
    let els2 = doc2["elements"].as_array().unwrap();
    // arrow1 and arrow_label deleted, only node2 and text2 survive
    assert_eq!(els2.len(), 2);
    let surviving_ids2: Vec<&str> = els2.iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(surviving_ids2, vec!["node2", "text2"]);

    let (code_chk2, stdout_chk2, _) = run_bin(&[path, "check"]);
    assert_eq!(code_chk2, 0, "check failed: {}", stdout_chk2);
}

#[test]
fn test_regression_vulnerability_5_theme_apply_cascades_to_bound_text_and_container() {
    // Vulnerability 5: theme apply --id <id> styles both container and its bound text.
    let sample = r##"{
        "type": "excalidraw",
        "version": 2,
        "elements": [
            {
                "id": "box1",
                "type": "rectangle",
                "x": 100,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a0",
                "strokeColor": "#000000",
                "backgroundColor": "transparent",
                "fillStyle": "solid",
                "roughness": 0,
                "customData": { "role": "warning" },
                "boundElements": [{ "id": "txt1", "type": "text" }]
            },
            {
                "id": "txt1",
                "type": "text",
                "x": 110,
                "y": 115,
                "width": 80,
                "height": 20,
                "index": "a1",
                "containerId": "box1",
                "fontFamily": 3,
                "strokeColor": "#000000",
                "text": "Box",
                "originalText": "Box"
            },
            {
                "id": "arrow1",
                "type": "arrow",
                "x": 200,
                "y": 125,
                "width": 100,
                "height": 0,
                "index": "a2",
                "points": [[0.0, 0.0], [100.0, 0.0]],
                "strokeColor": "#000000",
                "roughness": 0,
                "boundElements": [{ "id": "arr_lbl", "type": "text" }]
            },
            {
                "id": "arr_lbl",
                "type": "text",
                "x": 240,
                "y": 115,
                "width": 40,
                "height": 20,
                "index": "a3",
                "containerId": "arrow1",
                "fontFamily": 3,
                "strokeColor": "#000000",
                "text": "Flow",
                "originalText": "Flow"
            },
            {
                "id": "box2",
                "type": "rectangle",
                "x": 400,
                "y": 100,
                "width": 100,
                "height": 50,
                "index": "a4",
                "strokeColor": "#000000",
                "backgroundColor": "transparent",
                "fillStyle": "solid",
                "roughness": 0
            }
        ]
    }"##;

    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), sample).unwrap();
    let path = file.path().to_str().unwrap();

    // 1. Apply theme to box1 by ID -> styles both box1 and txt1
    let (code1, stdout1, stderr1) = run_bin(&[path, "theme", "apply", "retro-terminal", "--id", "box1"]);
    assert_eq!(code1, 0, "stderr: {}", stderr1);
    assert!(stdout1.contains("OK: applied theme 'retro-terminal' to 2 elements"));

    let content1 = fs::read_to_string(path).unwrap();
    let doc1: serde_json::Value = serde_json::from_str(&content1).unwrap();
    let box1 = &doc1["elements"][0];
    let txt1 = &doc1["elements"][1];
    let box2 = &doc1["elements"][4];

    // box1 styled
    assert_eq!(box1["fillStyle"], "hachure");
    assert_eq!(box1["roughness"], 2);
    assert_eq!(box1["strokeColor"], "#404040");
    assert_eq!(box1["backgroundColor"], "#ffa500");

    // txt1 cascaded theme
    assert_eq!(txt1["fontFamily"], 1);
    assert_eq!(txt1["strokeColor"], "#1a1a1a");

    // box2 untouched
    assert_eq!(box2["fillStyle"], "solid");
    assert_eq!(box2["roughness"], 0);
    assert_eq!(box2["strokeColor"], "#000000");

    // 2. Apply theme to arrow1 by ID -> styles both arrow1 and arr_lbl
    let (code2, stdout2, _) = run_bin(&[path, "theme", "apply", "retro-terminal", "--id", "arrow1"]);
    assert_eq!(code2, 0);
    assert!(stdout2.contains("OK: applied theme 'retro-terminal' to 2 elements"));

    let content2 = fs::read_to_string(path).unwrap();
    let doc2: serde_json::Value = serde_json::from_str(&content2).unwrap();
    let arrow1 = &doc2["elements"][2];
    let arr_lbl = &doc2["elements"][3];

    assert_eq!(arrow1["strokeColor"], "#404040");
    assert_eq!(arrow1["roughness"], 2);
    assert_eq!(arr_lbl["fontFamily"], 1);
    assert_eq!(arr_lbl["strokeColor"], "#1a1a1a");

    let (code_chk, stdout_chk, _) = run_bin(&[path, "check"]);
    assert_eq!(code_chk, 0, "check failed: {}", stdout_chk);
}
