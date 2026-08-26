//! Tests ensuring agent-facing schemas and documentation maintain integrity and do not drift.
//!
//! Enforces:
//! 1. No agent-facing tool schema contains "Birnbaum" (renamed to verbal contribution-weight scale).
//! 2. No tool schema contains dead section references (e.g. multi-parent.md §7 or TAXONOMY §Priority Labels).
//! 3. Tool descriptions for list_tasks and get_task accurately enumerate all 8 focus_score components.
//! 4. Canonical specification specs/ranking.md exists and covers all ranking measures and standing doctrine.
//! 5. specs/ranking.md carries a well-formed `pinned_commit` that resolves to a real commit,
//!    and that the prose agrees with.

use mem::mcp_server::PkbSearchServer;
use std::path::Path;
use std::process::Command;

/// Extract the `pinned_commit` value from a spec's YAML frontmatter.
///
/// Returns `None` if there is no frontmatter or no `pinned_commit` key in it.
fn extract_pinned_commit(content: &str) -> Option<String> {
    let body = content.strip_prefix("---\n")?;
    let (frontmatter, _) = body.split_once("\n---")?;
    frontmatter.lines().find_map(|line| {
        line.strip_prefix("pinned_commit:")
            .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
    })
}

/// True if `sha` is a full-length lowercase hex object name.
fn is_wellformed_sha(sha: &str) -> bool {
    sha.len() == 40
        && sha
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f'))
}

/// Run a git command in the crate root, returning trimmed stdout on success.
fn git(manifest_dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(manifest_dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

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

    // Must record a pinned commit. The value itself is deliberately not asserted
    // here — see test_ranking_spec_pinned_commit_is_valid.
    assert!(
        extract_pinned_commit(&content).is_some(),
        "specs/ranking.md frontmatter must record a pinned_commit"
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

/// The ranking spec pins the commit whose shipped behaviour it describes.
///
/// This asserts the *properties* the pin must have — present, well-formed, agreed
/// with by the prose, and resolving to a real commit — rather than comparing it to
/// a hardcoded value. A hardcoded expectation would make the spec unupdatable: each
/// phase of the ranking programme updates specs/ranking.md as part of its own
/// definition of done, and any such update must be free to re-pin.
#[test]
fn test_ranking_spec_pinned_commit_is_valid() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let ranking_spec_path = Path::new(manifest_dir).join("specs/ranking.md");
    let content = std::fs::read_to_string(&ranking_spec_path)
        .expect("should be able to read specs/ranking.md");

    let pin = extract_pinned_commit(&content)
        .expect("specs/ranking.md frontmatter must record a pinned_commit");

    assert!(
        is_wellformed_sha(&pin),
        "pinned_commit must be a 40-character lowercase hex commit sha, got {pin:?}"
    );

    // The prose in §Overview quotes the same commit; the two must not drift apart.
    let body = content
        .split_once("\n---")
        .map(|(_, rest)| rest)
        .unwrap_or(&content);
    assert!(
        body.contains(&pin),
        "specs/ranking.md prose must quote the same pinned_commit ({pin}) as its frontmatter"
    );

    // Resolution needs the object to be present locally. Skip rather than fail where
    // it cannot be: no git checkout (vendored crate, packaged source) or a shallow
    // clone that legitimately does not carry the object.
    if git(manifest_dir, &["rev-parse", "--git-dir"]).is_none() {
        eprintln!("skipping pinned_commit resolution: not a git checkout");
        return;
    }
    if git(manifest_dir, &["rev-parse", "--is-shallow-repository"]).as_deref() == Some("true") {
        eprintln!("skipping pinned_commit resolution: shallow clone");
        return;
    }

    let kind = git(manifest_dir, &["cat-file", "-t", &pin]);
    assert_eq!(
        kind.as_deref(),
        Some("commit"),
        "pinned_commit {pin} must resolve to a real commit in this repository, got {kind:?}"
    );
}
