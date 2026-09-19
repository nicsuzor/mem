//! Machine-specific path lint for note bodies.
//!
//! The PKB is one git repo replicated under different mount points on
//! different machines (`~/brain`, `/home/nic/brain`, `/Users/suzor/brain`,
//! `/data/brain` inside the services container). A note that names one of
//! those roots as the path to another PKB file resolves on exactly one
//! machine and is a dangling reference everywhere else. The stable ways to
//! refer to a PKB file are a wikilink (`[[id]]`) or a PKB-root-relative path
//! (`knowledge/foo.md`).
//!
//! This module finds machine-specific PKB paths in text a caller is about to
//! write. It only fires when a known root is followed by an actual path
//! segment — so prose that merely names the mount points (`` `~/brain/` ``,
//! `/Users/suzor/...`) passes, while `~/brain/knowledge/foo.md` is caught.
//! Paths to non-PKB files (source repos, dotfiles) are out of scope and are
//! not matched.

/// PKB roots as they appear on the machines the PKB is replicated to.
/// Each entry must be followed by `/` + a path segment to count as a hit.
pub const KNOWN_PKB_ROOTS: &[&str] = &[
    "~/brain",
    "$HOME/brain",
    "/home/nic/brain",
    "/Users/suzor/brain",
    "/data/brain",
];

/// One machine-specific PKB path found in a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachinePath {
    /// The full matched string, e.g. `~/brain/knowledge/foo.md`.
    pub matched: String,
    /// The root prefix that matched, e.g. `~/brain`.
    pub root: String,
    /// The remainder after the root, as a PKB-root-relative path, e.g.
    /// `knowledge/foo.md`. This is the suggested replacement.
    pub relative: String,
}

/// Characters that terminate a path token in prose or markdown.
fn ends_path(c: char) -> bool {
    c.is_whitespace() || matches!(c, '`' | '"' | '\'' | ')' | '>' | ']' | ',' | ';')
}

/// Find every machine-specific PKB path in `text`.
///
/// `extra_roots` lets the server add the absolute root it is actually
/// running against (e.g. `/data/brain`), so the check stays correct on a
/// machine whose mount point is not in [`KNOWN_PKB_ROOTS`].
pub fn find_machine_paths(text: &str, extra_roots: &[&str]) -> Vec<MachinePath> {
    let mut roots: Vec<&str> = KNOWN_PKB_ROOTS.to_vec();
    for r in extra_roots {
        let r = r.trim_end_matches('/');
        if !r.is_empty() && r != "/" && !roots.contains(&r) {
            roots.push(r);
        }
    }
    // Longest root first so `/data/brain` wins over a hypothetical `/data`.
    roots.sort_by_key(|r| std::cmp::Reverse(r.len()));

    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut pos = 0;
    while pos < text.len() {
        let rest = &text[pos..];
        let mut advanced = false;
        for root in &roots {
            if !rest.starts_with(root) {
                continue;
            }
            let after = &rest[root.len()..];
            if !after.starts_with('/') {
                continue;
            }
            let tail = &after[1..];
            let end = tail.find(ends_path).unwrap_or(tail.len());
            // Trailing sentence punctuation is not part of the path.
            let relative = tail[..end].trim_end_matches(['.', ':', '/']);
            // A real path segment has at least one alphanumeric character;
            // `~/brain/` and `/Users/suzor/brain/...` are prose, not paths.
            if relative.chars().any(|c| c.is_ascii_alphanumeric()) {
                let matched = format!("{root}/{relative}");
                if seen.insert(matched.clone()) {
                    found.push(MachinePath {
                        matched,
                        root: (*root).to_string(),
                        relative: relative.to_string(),
                    });
                }
                pos += root.len() + 1 + end;
                advanced = true;
            }
            break;
        }
        if !advanced {
            pos += rest.chars().next().map(char::len_utf8).unwrap_or(1);
        }
    }
    found
}

/// Machine-specific PKB paths in `new_text` that are not already present in
/// `existing_text`. Used by whole-body rewrites so that editing a note which
/// still carries an old path (the sweep of existing notes is separate work)
/// is not blocked, while adding a new one is.
pub fn find_new_machine_paths(
    new_text: &str,
    existing_text: &str,
    extra_roots: &[&str],
) -> Vec<MachinePath> {
    find_machine_paths(new_text, extra_roots)
        .into_iter()
        .filter(|p| !existing_text.contains(&p.matched))
        .collect()
}

/// The lines a unified diff adds (`+` lines, excluding the `+++` header),
/// joined with newlines. Context and removed lines are ignored, so an edit to
/// a note that still contains an old machine path is not blocked unless the
/// edit introduces a new one.
pub fn added_lines_of_diff(diff: &str) -> String {
    diff.lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .map(|l| &l[1..])
        .collect::<Vec<_>>()
        .join("\n")
}

/// Human-readable rejection message naming every offending string and the
/// stable form to use instead.
pub fn rejection_message(paths: &[MachinePath]) -> String {
    let mut msg = String::from(
        "Machine-specific PKB path rejected. The PKB is mounted at a different \
         root on each machine, so an absolute or home-relative path to a PKB \
         file resolves on one machine only. Refer to PKB files by wikilink \
         (`[[id]]`) or PKB-root-relative path instead.\n",
    );
    for p in paths {
        msg.push_str(&format!(
            "- `{}` -> write `{}` or the document's `[[id]]`\n",
            p.matched, p.relative
        ));
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_each_known_root_when_it_points_at_a_file() {
        for root in KNOWN_PKB_ROOTS {
            let body = format!("See {root}/knowledge/framework/foo.md for details.");
            let found = find_machine_paths(&body, &[]);
            assert_eq!(found.len(), 1, "root {root} should be flagged");
            assert_eq!(found[0].root, *root);
            assert_eq!(found[0].relative, "knowledge/framework/foo.md");
            assert_eq!(
                found[0].matched,
                format!("{root}/knowledge/framework/foo.md")
            );
        }
    }

    #[test]
    fn finds_paths_inside_backticks_and_parentheses() {
        let body = "Path: `~/brain/tasks/x.md` and (/home/nic/brain/notes/y.md).";
        let found = find_machine_paths(body, &[]);
        let matched: Vec<&str> = found.iter().map(|p| p.matched.as_str()).collect();
        assert_eq!(
            matched,
            vec!["~/brain/tasks/x.md", "/home/nic/brain/notes/y.md"]
        );
    }

    #[test]
    fn root_relative_path_and_wikilink_pass() {
        let body = "See `knowledge/framework/foo.md` or [[mcp-tool-design-id-over-paths]] \
                    or knowledge/framework/foo.md.";
        assert!(find_machine_paths(body, &[]).is_empty());
    }

    #[test]
    fn prose_naming_the_mount_points_passes() {
        // This is the shape of the task that commissioned the lint: the roots
        // are named, but none is used as a path to a file.
        let body = "The PKB is replicated under different mount points (`~/brain`, \
                    `~/brain/`, `/home/nic/brain`, `/data`, `/Users/suzor/...`).";
        assert!(find_machine_paths(body, &[]).is_empty());
    }

    #[test]
    fn non_pkb_paths_under_the_same_home_pass() {
        let body =
            "Compose file: `/Users/suzor/dotfiles/containers/services/docker-compose.yml:174` \
                    and repo at /home/nic/src/academicOps/aops-core.";
        assert!(find_machine_paths(body, &[]).is_empty());
    }

    #[test]
    fn server_root_is_honoured_as_extra_root() {
        let body = "Saved to /srv/pkb/knowledge/foo.md";
        assert!(find_machine_paths(body, &[]).is_empty());
        let found = find_machine_paths(body, &["/srv/pkb/"]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].relative, "knowledge/foo.md");
    }

    #[test]
    fn trailing_sentence_punctuation_is_not_part_of_the_path() {
        let found = find_machine_paths(
            "See ~/brain/knowledge/foo.md. Then ~/brain/tasks/bar.md:",
            &[],
        );
        let matched: Vec<&str> = found.iter().map(|p| p.matched.as_str()).collect();
        assert_eq!(
            matched,
            vec!["~/brain/knowledge/foo.md", "~/brain/tasks/bar.md"]
        );
    }

    #[test]
    fn duplicate_hits_are_reported_once() {
        let body = "~/brain/a.md then ~/brain/a.md again";
        assert_eq!(find_machine_paths(body, &[]).len(), 1);
    }

    #[test]
    fn rewrite_keeping_an_existing_path_passes_but_adding_one_fails() {
        let existing = "Old note mentions ~/brain/old.md";
        let same = "Rewritten, still mentions ~/brain/old.md";
        assert!(find_new_machine_paths(same, existing, &[]).is_empty());
        let added = "Rewritten, mentions ~/brain/old.md and ~/brain/new.md";
        let found = find_new_machine_paths(added, existing, &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].matched, "~/brain/new.md");
    }

    #[test]
    fn diff_added_lines_only() {
        let diff = "--- a\n+++ b\n@@ -1,2 +1,2 @@\n context ~/brain/ctx.md\n-removed ~/brain/gone.md\n+added ~/brain/new.md\n";
        let added = added_lines_of_diff(diff);
        assert_eq!(added, "added ~/brain/new.md");
        let found = find_machine_paths(&added, &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].matched, "~/brain/new.md");
    }

    #[test]
    fn message_names_offending_string_and_replacement() {
        let found = find_machine_paths("x /Users/suzor/brain/knowledge/a.md y", &[]);
        let msg = rejection_message(&found);
        assert!(msg.contains("/Users/suzor/brain/knowledge/a.md"));
        assert!(msg.contains("knowledge/a.md"));
        assert!(msg.contains("[[id]]"));
    }
}
