//! Aider-compatible unified-diff parser and applier for PKB document bodies.
//!
//! Ported from Aider's `aider/coders/udiff_coder.py` and `aider/coders/search_replace.py`.
//! Reference spec: https://raw.githubusercontent.com/Aider-AI/aider/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/website/docs/unified-diffs.md

use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UdiffError {
    #[error("UnifiedDiffNoMatch: hunk {hunk_index} of {total_hunks} failed to apply!\n\nTarget content does not contain these {num_lines} exact lines in a row:\n```\n{expected_original}```{note}")]
    NoMatch {
        hunk_index: usize,
        total_hunks: usize,
        num_lines: usize,
        expected_original: String,
        note: String,
    },
    #[error("UnifiedDiffNotUnique: hunk {hunk_index} of {total_hunks} failed to apply!\n\nTarget content contains multiple sets of lines matching the diff.\nTry adding additional context lines (` `) to uniquely identify where to edit.\nTarget contains multiple copies of these {num_lines} lines:\n```\n{expected_original}```{note}")]
    NotUnique {
        hunk_index: usize,
        total_hunks: usize,
        num_lines: usize,
        expected_original: String,
        note: String,
    },
    #[error("No valid diff hunks found in input")]
    NoHunksFound,
    #[error("Invalid hunk syntax at hunk {hunk_index}: {reason}")]
    InvalidHunk {
        hunk_index: usize,
        reason: String,
    },
}

pub type UnifiedDiffError = UdiffError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDiffEdit {
    pub path: Option<String>,
    pub hunk_lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffResult {
    pub new_content: String,
    pub hunks_applied: usize,
}

// -----------------------------------------------------------------------------
// Relative Indentation Engine (Port of Aider's RelativeIndenter)
// -----------------------------------------------------------------------------

pub struct RelativeIndenter {
    pub marker: char,
}

impl RelativeIndenter {
    pub fn new(texts: &[&str]) -> Self {
        let mut chars = HashSet::new();
        for text in texts {
            for c in text.chars() {
                chars.insert(c);
            }
        }

        let arrow = '←';
        let marker = if !chars.contains(&arrow) {
            arrow
        } else {
            Self::select_unique_marker(&chars).unwrap_or('←')
        };

        Self { marker }
    }

    fn select_unique_marker(chars: &HashSet<char>) -> Option<char> {
        for codepoint in (0x10000..=0x10FFFF).rev() {
            if let Some(c) = char::from_u32(codepoint) {
                if !chars.contains(&c) {
                    return Some(c);
                }
            }
        }
        None
    }

    pub fn make_relative(&self, text: &str) -> Result<String, String> {
        if text.contains(self.marker) {
            return Err(format!(
                "Text already contains the outdent marker: {}",
                self.marker
            ));
        }

        let lines = split_keepends(text);
        let mut output = Vec::with_capacity(lines.len() * 3);
        let mut prev_indent = "";

        for line in &lines {
            let line_without_end = line.trim_end_matches(['\r', '\n']);
            let len_indent = line_without_end.len() - line_without_end.trim_start().len();
            let indent = &line[..len_indent];
            let change = (len_indent as isize) - (prev_indent.len() as isize);

            let cur_indent = if change > 0 {
                indent[indent.len() - (change as usize)..].to_string()
            } else if change < 0 {
                std::iter::repeat_n(self.marker, (-change) as usize).collect::<String>()
            } else {
                String::new()
            };

            output.push(cur_indent);
            output.push("\n".to_string());
            output.push(line[len_indent..].to_string());
            prev_indent = indent;
        }

        Ok(output.concat())
    }

    pub fn make_absolute(&self, text: &str) -> Result<String, String> {
        let lines = split_keepends(text);
        if !lines.len().is_multiple_of(2) {
            return Err("Invalid relative format: odd number of line segments".to_string());
        }

        let mut output = Vec::with_capacity(lines.len() / 2);
        let mut prev_indent = String::new();

        for i in (0..lines.len()).step_by(2) {
            let dent = lines[i].trim_end_matches(['\r', '\n']);
            let non_indent = &lines[i + 1];

            let cur_indent = if dent.starts_with(self.marker) {
                let len_outdent = dent.chars().count();
                if len_outdent > prev_indent.len() {
                    "".to_string()
                } else {
                    prev_indent[..prev_indent.len() - len_outdent].to_string()
                }
            } else {
                format!("{}{}", prev_indent, dent)
            };

            let out_line = if non_indent.trim_end_matches(['\r', '\n']).is_empty() {
                non_indent.clone() // don't indent a blank line
            } else {
                format!("{}{}", cur_indent, non_indent)
            };

            output.push(out_line);
            prev_indent = cur_indent;
        }

        let res = output.concat();
        if res.contains(self.marker) {
            return Err("Error transforming text back to absolute indents".to_string());
        }

        Ok(res)
    }
}

fn split_keepends(s: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                lines.push(s[start..=i + 1].to_string());
                i += 2;
                start = i;
                continue;
            } else {
                lines.push(s[start..=i].to_string());
                i += 1;
                start = i;
                continue;
            }
        } else if bytes[i] == b'\n' {
            lines.push(s[start..=i].to_string());
            i += 1;
            start = i;
            continue;
        }
        i += 1;
    }
    if start < bytes.len() {
        lines.push(s[start..].to_string());
    }
    lines
}

// -----------------------------------------------------------------------------
// Hunk helpers and before/after extraction
// -----------------------------------------------------------------------------

pub fn hunk_to_before_after(hunk: &[String], lines_only: bool) -> (Vec<String>, Vec<String>) {
    let mut before = Vec::new();
    let mut after = Vec::new();

    for raw_line in hunk {
        if raw_line.is_empty() {
            before.push("\n".to_string());
            after.push("\n".to_string());
            continue;
        }

        let op = raw_line.chars().next().unwrap_or(' ');
        let content_line = if raw_line.len() >= 2 {
            raw_line[1..].to_string()
        } else {
            "\n".to_string()
        };

        match op {
            ' ' => {
                before.push(content_line.clone());
                after.push(content_line);
            }
            '-' => {
                before.push(content_line);
            }
            '+' => {
                after.push(content_line);
            }
            '@' => {
                // Ignore hunk header markers inside hunk
            }
            _ => {
                // If no op prefix, treat full line as context
                before.push(raw_line.clone());
                after.push(raw_line.clone());
            }
        }
    }

    if lines_only {
        (before, after)
    } else {
        (vec![before.concat()], vec![after.concat()])
    }
}

pub fn cleanup_pure_whitespace_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                if line.ends_with("\r\n") {
                    "\r\n".to_string()
                } else if line.ends_with('\n') || line.ends_with('\r') {
                    "\n".to_string()
                } else {
                    String::new()
                }
            } else {
                line.clone()
            }
        })
        .collect()
}

pub fn normalize_hunk(hunk: &[String]) -> Vec<String> {
    let (before_lines, after_lines) = hunk_to_before_after(hunk, true);
    let before_clean = cleanup_pure_whitespace_lines(&before_lines);
    let after_clean = cleanup_pure_whitespace_lines(&after_lines);

    let mut res = Vec::new();
    let mut b_idx = 0;
    let mut a_idx = 0;
    for raw_line in hunk {
        if raw_line.is_empty() {
            continue;
        }
        let op = raw_line.chars().next().unwrap_or(' ');
        match op {
            ' ' => {
                if b_idx < before_clean.len() {
                    res.push(format!(" {}", before_clean[b_idx]));
                    b_idx += 1;
                    a_idx += 1;
                }
            }
            '-' => {
                if b_idx < before_clean.len() {
                    res.push(format!("-{}", before_clean[b_idx]));
                    b_idx += 1;
                }
            }
            '+' => {
                if a_idx < after_clean.len() {
                    res.push(format!("+{}", after_clean[a_idx]));
                    a_idx += 1;
                }
            }
            _ => {
                res.push(raw_line.clone());
            }
        }
    }
    res
}

// -----------------------------------------------------------------------------
// Search and Replace Strategies
// -----------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum SearchReplaceError {
    NoMatch,
    NotUnique,
}

fn search_and_replace(
    search_text: &str,
    replace_text: &str,
    original_text: &str,
) -> Result<String, SearchReplaceError> {
    let count = original_text.matches(search_text).count();
    if count == 1 {
        return Ok(original_text.replacen(search_text, replace_text, 1));
    }
    if count > 1 {
        return Err(SearchReplaceError::NotUnique);
    }

    // Fallback 1: if search_text ends with '\n' but target content ends without '\n'
    if search_text.ends_with('\n') && !original_text.ends_with('\n') {
        let s_trimmed = search_text.trim_end_matches(['\r', '\n']);
        let r_trimmed = replace_text.trim_end_matches(['\r', '\n']);
        let c = original_text.matches(s_trimmed).count();
        if c == 1 && original_text.ends_with(s_trimmed) {
            let prefix_len = original_text.len() - s_trimmed.len();
            return Ok(format!("{}{}", &original_text[..prefix_len], r_trimmed));
        }
        if c > 1 {
            return Err(SearchReplaceError::NotUnique);
        }
    }

    // Fallback 2: if original_text ends with '\n' but search_text does not
    if original_text.ends_with('\n') && !search_text.ends_with('\n') {
        let s_nl = format!("{}\n", search_text);
        let r_nl = format!("{}\n", replace_text);
        let c = original_text.matches(&s_nl).count();
        if c == 1 {
            return Ok(original_text.replacen(&s_nl, &r_nl, 1));
        }
        if c > 1 {
            return Err(SearchReplaceError::NotUnique);
        }
    }

    Err(SearchReplaceError::NoMatch)
}

fn try_strategy(
    search_text: &str,
    replace_text: &str,
    original_text: &str,
    strip_blank: bool,
    rel_indent: bool,
) -> Result<String, SearchReplaceError> {
    let (mut s, mut r, mut o) = (
        search_text.to_string(),
        replace_text.to_string(),
        original_text.to_string(),
    );

    if strip_blank {
        s = format!("{}\n", s.trim_matches('\n'));
        r = format!("{}\n", r.trim_matches('\n'));
    }

    let mut ri = None;
    if rel_indent {
        let indenter = RelativeIndenter::new(&[&s, &r, &o]);
        let s_rel = indenter.make_relative(&s).map_err(|_| SearchReplaceError::NoMatch)?;
        let r_rel = indenter.make_relative(&r).map_err(|_| SearchReplaceError::NoMatch)?;
        let o_rel = indenter.make_relative(&o).map_err(|_| SearchReplaceError::NoMatch)?;
        s = s_rel;
        r = r_rel;
        o = o_rel;
        ri = Some(indenter);
    }

    let res = search_and_replace(&s, &r, &o)?;

    if let Some(indenter) = ri {
        let abs = indenter.make_absolute(&res).map_err(|_| SearchReplaceError::NoMatch)?;
        Ok(abs)
    } else {
        Ok(res)
    }
}

pub fn flexible_search_and_replace(
    search_text: &str,
    replace_text: &str,
    original_text: &str,
) -> Result<String, SearchReplaceError> {
    let preprocs = [
        (false, false), // exact match
        (true, false),  // strip blank lines
        (false, true),  // relative indentation
        (true, true),   // strip blank lines + relative indentation
    ];

    let mut saw_not_unique = false;

    for (strip_blank, rel_indent) in preprocs {
        match try_strategy(search_text, replace_text, original_text, strip_blank, rel_indent) {
            Ok(res) => return Ok(res),
            Err(SearchReplaceError::NotUnique) => {
                saw_not_unique = true;
            }
            Err(SearchReplaceError::NoMatch) => {}
        }
    }

    if saw_not_unique {
        Err(SearchReplaceError::NotUnique)
    } else {
        Err(SearchReplaceError::NoMatch)
    }
}

pub fn directly_apply_hunk(
    content: &str,
    hunk: &[String],
) -> Result<String, SearchReplaceError> {
    let (before_vec, after_vec) = hunk_to_before_after(hunk, false);
    let before = before_vec.concat();
    let after = after_vec.concat();

    if before.is_empty() {
        // If hunk is addition-only with no before context, append to content
        return Ok(format!("{}{}", content, after));
    }

    let non_ws_count = before.chars().filter(|c| !c.is_whitespace()).count();
    let match_count = content.matches(&before).count();
    if non_ws_count < 10 && match_count > 1 {
        return Err(SearchReplaceError::NotUnique);
    }

    flexible_search_and_replace(&before, &after, content)
}

pub fn apply_partial_hunk(
    content: &str,
    preceding_context: &[String],
    changes: &[String],
    following_context: &[String],
) -> Option<String> {
    let len_prec = preceding_context.len();
    let len_foll = following_context.len();
    let use_all = len_prec + len_foll;

    for drop in 0..=use_all {
        let use_count = use_all - drop;

        for use_prec in (0..=len_prec).rev() {
            if use_prec > use_count {
                continue;
            }

            let use_foll = use_count - use_prec;
            if use_foll > len_foll {
                continue;
            }

            let this_prec = if use_prec > 0 {
                &preceding_context[len_prec - use_prec..]
            } else {
                &[]
            };

            let this_foll = &following_context[..use_foll];

            let mut sub_hunk = Vec::new();
            sub_hunk.extend_from_slice(this_prec);
            sub_hunk.extend_from_slice(changes);
            sub_hunk.extend_from_slice(this_foll);

            if let Ok(res) = directly_apply_hunk(content, &sub_hunk) {
                return Some(res);
            }
        }
    }

    None
}

pub fn apply_hunk(content: &str, hunk: &[String]) -> Result<String, SearchReplaceError> {
    // 1. Direct apply
    match directly_apply_hunk(content, hunk) {
        Ok(res) => return Ok(res),
        Err(SearchReplaceError::NotUnique) => return Err(SearchReplaceError::NotUnique),
        Err(SearchReplaceError::NoMatch) => {}
    }

    // 2. Partial hunk application (grouping context and change sections)
    let mut sections: Vec<Vec<String>> = Vec::new();
    let mut cur_section: Vec<String> = Vec::new();
    let mut cur_is_change = false;

    for line in hunk {
        let op = line.chars().next().unwrap_or(' ');
        let is_change = op == '+' || op == '-';
        if is_change != cur_is_change && !cur_section.is_empty() {
            sections.push(cur_section);
            cur_section = Vec::new();
            cur_is_change = is_change;
        }
        cur_section.push(line.clone());
    }
    if !cur_section.is_empty() {
        sections.push(cur_section);
    }

    // Must alternate [ctx0, chg0, ctx1, chg1, ctx2, ...]
    // Ensure leading and trailing context sections exist
    if !sections.is_empty() {
        let first_op = sections[0][0].chars().next().unwrap_or(' ');
        if first_op == '+' || first_op == '-' {
            sections.insert(0, Vec::new());
        }
        let last_op = sections.last().unwrap()[0].chars().next().unwrap_or(' ');
        if last_op == '+' || last_op == '-' {
            sections.push(Vec::new());
        }
    }

    if sections.len() >= 3 {
        let mut cur_content = content.to_string();
        let mut all_done = true;

        for i in (1..sections.len()).step_by(2) {
            let preceding = &sections[i - 1];
            let changes = &sections[i];
            let following = if i + 1 < sections.len() {
                &sections[i + 1]
            } else {
                &[][..]
            };

            if let Some(res) = apply_partial_hunk(&cur_content, preceding, changes, following) {
                cur_content = res;
            } else {
                all_done = false;
                break;
            }
        }

        if all_done {
            return Ok(cur_content);
        }
    }

    Err(SearchReplaceError::NoMatch)
}

// -----------------------------------------------------------------------------
// Diff Extraction / Parser (Port of Aider's find_diffs & process_fenced_block)
// -----------------------------------------------------------------------------

pub fn find_diffs(content: &str) -> Vec<RawDiffEdit> {
    let mut text = content.to_string();
    if !text.ends_with('\n') {
        text.push('\n');
    }

    let lines = split_keepends(&text);
    let mut line_num = 0;
    let mut edits = Vec::new();

    let mut has_diff_fence = false;
    for line in &lines {
        if line.starts_with("```diff") {
            has_diff_fence = true;
            break;
        }
    }

    if has_diff_fence {
        while line_num < lines.len() {
            if lines[line_num].starts_with("```diff") {
                let (next_line, these_edits) = process_fenced_block(&lines, line_num + 1);
                edits.extend(these_edits);
                line_num = next_line;
            } else {
                line_num += 1;
            }
        }
    } else {
        // Raw diff without fences
        let (_, these_edits) = process_fenced_block(&lines, 0);
        edits.extend(these_edits);
    }

    edits
}

fn process_fenced_block(lines: &[String], start_line_num: usize) -> (usize, Vec<RawDiffEdit>) {
    let mut end_line_num = lines.len();
    for (idx, line) in lines.iter().enumerate().skip(start_line_num) {
        if line.starts_with("```") {
            end_line_num = idx;
            break;
        }
    }

    let mut block: Vec<String> = lines[start_line_num..end_line_num].to_vec();
    block.push("@@ @@\n".to_string());

    let mut fname = None;
    if block.len() >= 2 && block[0].starts_with("--- ") && block[1].starts_with("+++ ") {
        let a_fname = block[0][4..].trim();
        let b_fname = block[1][4..].trim();

        let parsed_fname = if (a_fname.starts_with("a/") || a_fname == "/dev/null")
            && b_fname.starts_with("b/")
        {
            &b_fname[2..]
        } else {
            b_fname
        };
        fname = Some(parsed_fname.to_string());
        block = block[2..].to_vec();
    }

    let mut edits = Vec::new();
    let mut keeper = false;
    let mut hunk: Vec<String> = Vec::new();

    for line in block {
        hunk.push(line.clone());
        if line.len() < 2 {
            continue;
        }

        if line.starts_with("+++ ") && hunk.len() >= 2 && hunk[hunk.len() - 2].starts_with("--- ") {
            let drop_count = if hunk.len() >= 3 && hunk[hunk.len() - 3] == "\n" {
                3
            } else {
                2
            };
            let keep_len = hunk.len().saturating_sub(drop_count);
            let final_hunk = hunk[..keep_len].to_vec();
            if !final_hunk.is_empty() {
                edits.push(RawDiffEdit {
                    path: fname.clone(),
                    hunk_lines: final_hunk,
                });
            }
            hunk.clear();
            keeper = false;
            fname = Some(line[4..].trim().to_string());
            continue;
        }

        let op = line.chars().next().unwrap_or(' ');
        if op == '-' || op == '+' {
            keeper = true;
            continue;
        }
        if op != '@' {
            continue;
        }
        if !keeper {
            hunk.clear();
            continue;
        }

        let keep_len = hunk.len().saturating_sub(1);
        let final_hunk = hunk[..keep_len].to_vec();
        if !final_hunk.is_empty() {
            edits.push(RawDiffEdit {
                path: fname.clone(),
                hunk_lines: final_hunk,
            });
        }
        hunk.clear();
        keeper = false;
    }

    (end_line_num + 1, edits)
}

// -----------------------------------------------------------------------------
// Top-Level Unified Diff Applier
// -----------------------------------------------------------------------------

pub fn apply_diff(content: &str, diff_text: &str) -> Result<DiffResult, UdiffError> {
    let edits = find_diffs(diff_text);
    if edits.is_empty() {
        return Err(UdiffError::NoHunksFound);
    }

    let mut current_content = content.to_string();
    let total_hunks = edits.len();
    let mut hunks_applied = 0;

    for (idx, edit) in edits.iter().enumerate() {
        let hunk_index = idx + 1;
        let norm_hunk = normalize_hunk(&edit.hunk_lines);
        let hunk_to_use = if norm_hunk.is_empty() {
            &edit.hunk_lines
        } else {
            &norm_hunk
        };

        let (before_lines, _) = hunk_to_before_after(hunk_to_use, true);
        let expected_original = before_lines.concat();
        let num_lines = before_lines.len();

        let note = if hunks_applied > 0 {
            format!(
                "\n\nNote: {} of {} hunks applied successfully before this failure.",
                hunks_applied, total_hunks
            )
        } else {
            String::new()
        };

        match apply_hunk(&current_content, hunk_to_use) {
            Ok(new_content) => {
                current_content = new_content;
                hunks_applied += 1;
            }
            Err(SearchReplaceError::NotUnique) => {
                return Err(UdiffError::NotUnique {
                    hunk_index,
                    total_hunks,
                    num_lines,
                    expected_original,
                    note,
                });
            }
            Err(SearchReplaceError::NoMatch) => {
                return Err(UdiffError::NoMatch {
                    hunk_index,
                    total_hunks,
                    num_lines,
                    expected_original,
                    note,
                });
            }
        }
    }

    Ok(DiffResult {
        new_content: current_content,
        hunks_applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Ported test cases from Aider's tests/basic/test_udiff.py
    // 0 skipped; all 4 diff-parsing/diff-application cases ported below.
    // =========================================================================

    #[test]
    fn test_find_diffs_single_hunk() {
        let content = r#"
Some text...

```diff
--- file.txt
+++ file.txt
@@ ... @@
-Original
+Modified
```
"#;
        let edits = find_diffs(content);
        assert_eq!(edits.len(), 1);
        let edit = &edits[0];
        assert_eq!(edit.path.as_deref(), Some("file.txt"));
        assert_eq!(
            edit.hunk_lines,
            vec!["-Original\n".to_string(), "+Modified\n".to_string()]
        );
    }

    #[test]
    fn test_find_diffs_dev_null() {
        let content = r#"
Some text...

```diff
--- /dev/null
+++ file.txt
@@ ... @@
-Original
+Modified
```
"#;
        let edits = find_diffs(content);
        assert_eq!(edits.len(), 1);
        let edit = &edits[0];
        assert_eq!(edit.path.as_deref(), Some("file.txt"));
        assert_eq!(
            edit.hunk_lines,
            vec!["-Original\n".to_string(), "+Modified\n".to_string()]
        );
    }

    #[test]
    fn test_find_diffs_dirname_with_spaces() {
        let content = r#"
Some text...

```diff
--- dir name with spaces/file.txt
+++ dir name with spaces/file.txt
@@ ... @@
-Original
+Modified
```
"#;
        let edits = find_diffs(content);
        assert_eq!(edits.len(), 1);
        let edit = &edits[0];
        assert_eq!(edit.path.as_deref(), Some("dir name with spaces/file.txt"));
        assert_eq!(
            edit.hunk_lines,
            vec!["-Original\n".to_string(), "+Modified\n".to_string()]
        );
    }

    #[test]
    fn test_find_multi_diffs() {
        let content = r#"
To implement the `--check-update` option, I will make the following changes:

1. Add the `--check-update` argument to the argument parser in `aider/main.py`.
2. Modify the `check_version` function in `aider/versioncheck.py` to return a boolean indicating whether an update is available.
3. Use the returned value from `check_version` in `aider/main.py` to set the exit status code when `--check-update` is used.

Here are the diffs for those changes:

```diff
--- aider/versioncheck.py
+++ aider/versioncheck.py
@@ ... @@
     except Exception as err:
         print_cmd(f"Error checking pypi for new version: {err}")
+        return False

--- aider/main.py
+++ aider/main.py
@@ ... @@
     other_group.add_argument(
         "--version",
         action="version",
         version=f"%(prog)s {__version__}",
         help="Show the version number and exit",
     )
+    other_group.add_argument(
+        "--check-update",
+        action="store_true",
+        help="Check for updates and return status in the exit code",
+        default=False,
+    )
     other_group.add_argument(
         "--apply",
         metavar="FILE",
```

These changes will add the `--check-update` option...
"#;
        let edits = find_diffs(content);
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].path.as_deref(), Some("aider/versioncheck.py"));
        assert_eq!(edits[0].hunk_lines.len(), 3);
        assert_eq!(edits[1].path.as_deref(), Some("aider/main.py"));
        assert_eq!(edits[1].hunk_lines.len(), 15);
    }

    // =========================================================================
    // Diff application and fallback tests
    // =========================================================================

    #[test]
    fn test_apply_diff_simple_search_and_replace() {
        let original = "line one\nline two\nline three\n";
        let diff = "```diff\n@@ ... @@\n-line two\n+line TWO\n```";
        let res = apply_diff(original, diff).unwrap();
        assert_eq!(res.new_content, "line one\nline TWO\nline three\n");
        assert_eq!(res.hunks_applied, 1);
    }

    #[test]
    fn test_apply_diff_bare_hunk_without_fence() {
        let original = "foo\nbar\nbaz\n";
        let diff = "@@ -1,3 +1,3 @@\n foo\n-bar\n+qux\n baz\n";
        let res = apply_diff(original, diff).unwrap();
        assert_eq!(res.new_content, "foo\nqux\nbaz\n");
        assert_eq!(res.hunks_applied, 1);
    }

    #[test]
    fn test_apply_diff_relative_indentation() {
        let original = "        def helper():\n            x = 1\n            y = 2\n            return x + y\n";
        // Diff authored with 4-space indent instead of 8-space indent
        let diff = "@@ ... @@\n    def helper():\n        x = 1\n-       y = 2\n+       y = 42\n        return x + y\n";
        let res = apply_diff(original, diff).unwrap();
        assert_eq!(
            res.new_content,
            "        def helper():\n            x = 1\n            y = 42\n            return x + y\n"
        );
    }

    #[test]
    fn test_apply_diff_partial_hunk_context_shrinking() {
        let original = "header\nline 1\nline 2\ntarget line\nline 3\nline 4\nfooter\n";
        // Diff context has a slight mismatch at the outer line ("line 1 altered"), but inner context matches
        let diff = "@@ ... @@\n line 1 altered\n line 2\n-target line\n+replaced target\n line 3\n line 4\n";
        let res = apply_diff(original, diff).unwrap();
        assert_eq!(
            res.new_content,
            "header\nline 1\nline 2\nreplaced target\nline 3\nline 4\nfooter\n"
        );
    }

    #[test]
    fn test_apply_diff_multiple_hunks_sequential() {
        let original = "alpha\nbeta\ngamma\ndelta\nepsilon\n";
        let diff = "```diff\n@@ ... @@\n-beta\n+BETA\n@@ ... @@\n-delta\n+DELTA\n```";
        let res = apply_diff(original, diff).unwrap();
        assert_eq!(res.new_content, "alpha\nBETA\ngamma\nDELTA\nepsilon\n");
        assert_eq!(res.hunks_applied, 2);
    }

    #[test]
    fn test_apply_diff_no_match_returns_error() {
        let original = "alpha\nbeta\ngamma\n";
        let diff = "```diff\n@@ ... @@\n-nonexistent line\n+something\n```";
        let err = apply_diff(original, diff).unwrap_err();
        match err {
            UdiffError::NoMatch { hunk_index, num_lines, .. } => {
                assert_eq!(hunk_index, 1);
                assert_eq!(num_lines, 1);
            }
            other => panic!("Expected NoMatch, got: {:?}", other),
        }
    }

    #[test]
    fn test_apply_diff_not_unique_returns_error() {
        let original = "repeat\nrepeat\nrepeat\n";
        let diff = "```diff\n@@ ... @@\n-repeat\n+single\n```";
        let err = apply_diff(original, diff).unwrap_err();
        match err {
            UdiffError::NotUnique { hunk_index, .. } => {
                assert_eq!(hunk_index, 1);
            }
            other => panic!("Expected NotUnique, got: {:?}", other),
        }
    }

    #[test]
    fn test_gray_matter_integration() {
        use gray_matter::engine::YAML;
        use gray_matter::Matter;

        let initial = "---\nid: test-doc\nmodified: '2026-01-01T00:00:00Z'\ntitle: Test\n---\n\nInitial body text.\nSecond line.\n";
        let matter = Matter::<YAML>::new();
        let parsed = matter.parse(initial);

        let diff = "```diff\n@@ ... @@\n-Second line.\n+Updated second line.\n```";
        let res = apply_diff(&parsed.content, diff).unwrap();
        assert!(res.new_content.contains("Updated second line."));
    }
}
