//! #686: tool input/output recorded on OTel spans is capped at
//! `SPAN_VALUE_MAX_BYTES`. The cut must land on a UTF-8 char boundary — a
//! multi-byte char straddling it used to panic, and the release build's
//! `panic = "abort"` took the whole server down on every restart.

use super::*;
use crate::mcp_server::helpers::SPAN_VALUE_MAX_BYTES;

/// A string over the cap with `ch` starting at byte `start`, so it straddles
/// the old fixed cut at byte 8189.
fn straddling(ch: char, start: usize) -> String {
    let mut s = "a".repeat(start);
    s.push(ch);
    s.push_str(&"b".repeat(64));
    assert!(s.len() > SPAN_VALUE_MAX_BYTES);
    assert!(!s.is_char_boundary(SPAN_VALUE_MAX_BYTES - 3));
    s
}

#[test]
fn span_value_truncation_survives_two_byte_char_at_cut() {
    // '§' at bytes 8188..8190 — the first panic in the #686 log.
    let out = PkbSearchServer::truncate_span_value(straddling('§', 8188));
    assert_eq!(out, format!("{}...", "a".repeat(8188)));
    assert!(out.len() <= SPAN_VALUE_MAX_BYTES);
}

#[test]
fn span_value_truncation_survives_three_byte_char_at_cut() {
    // '—' at bytes 8187..8190 — the second panic in the #686 log.
    let out = PkbSearchServer::truncate_span_value(straddling('—', 8187));
    assert_eq!(out, format!("{}...", "a".repeat(8187)));
    assert!(out.len() <= SPAN_VALUE_MAX_BYTES);
}

#[test]
fn span_value_truncation_leaves_short_and_ascii_values_alone() {
    let short = "§—".repeat(10);
    assert_eq!(PkbSearchServer::truncate_span_value(short.clone()), short);

    let ascii = "x".repeat(SPAN_VALUE_MAX_BYTES + 100);
    let out = PkbSearchServer::truncate_span_value(ascii);
    assert_eq!(out.len(), SPAN_VALUE_MAX_BYTES);
    assert!(out.ends_with("..."));
}
