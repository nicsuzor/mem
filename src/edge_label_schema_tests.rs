//! Edge-label schema conformance: every consumer of the edge-label set
//! (frontmatter parser, write-path validation, write-path error messages and
//! `pkb lint`, including its padding fix) derives from the single definition
//! in `crate::graph` (`LinkLabel::STORED` / `LinkLabel::COMPUTED`). Each test
//! iterates that definition, so a label added to it is covered here with no
//! further edit, and a consumer that keeps its own copy of the set fails here.

use crate::graph::{validate_links_value, GraphNode, LinkLabel};
use crate::lint::lint_file;
use std::io::Write;

fn doc_with_label(label: &str) -> String {
    format!(
        "---\nid: t1\ntitle: T1\ntype: task\nstatus: active\nlinks:\n  - to: t2\n    label: {label}\n---\n\nBody\n"
    )
}

fn write_tmp(content: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::with_suffix(".md").unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f
}

fn parse_node(label: &str) -> GraphNode {
    let f = write_tmp(&doc_with_label(label));
    let doc = crate::pkb::parse_file(f.path()).expect("parse");
    GraphNode::from_pkb_document(&doc)
}

fn lint_label_diag(label: &str) -> Option<crate::lint::Diagnostic> {
    let f = write_tmp(&doc_with_label(label));
    lint_file(f.path(), false, None, None, None)
        .diagnostics
        .into_iter()
        .find(|d| d.rule == "flow-edge-label-invalid")
}

/// Every string a stored `links[].label` must be refused for: computed labels
/// plus strings that are no label at all.
fn refused() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = LinkLabel::COMPUTED.iter().map(|l| l.as_str()).collect();
    v.extend(["blocks", "relates", "frobnicates", "supports", "settles", "alternative"]);
    v
}

#[test]
fn definition_partitions_all_labels() {
    assert!(!LinkLabel::STORED.is_empty());
    assert_eq!(
        LinkLabel::ALL.len(),
        LinkLabel::STORED.len() + LinkLabel::COMPUTED.len()
    );
    for l in LinkLabel::STORED {
        assert!(l.is_stored());
        assert_eq!(
            LinkLabel::from_str(l.as_str()),
            Some(*l),
            "{l} must round-trip"
        );
    }
    for l in LinkLabel::COMPUTED {
        assert!(!l.is_stored());
        assert_eq!(
            LinkLabel::from_str(l.as_str()),
            None,
            "computed {l} must not parse"
        );
        let r = l.replacement().expect("computed label names a replacement");
        assert!(r.is_stored(), "{l} replacement {r} must be a stored label");
    }
}

#[test]
fn parser_accepts_exactly_the_stored_labels() {
    for l in LinkLabel::STORED {
        let node = parse_node(l.as_str());
        assert!(
            node.parse_warnings.is_empty(),
            "parser rejected stored label {l}: {:?}",
            node.parse_warnings
        );
        assert_eq!(node.links.len(), 1, "parser dropped stored label {l}");
        assert_eq!(node.links[0].label, *l);
    }
    for bad in refused() {
        let node = parse_node(bad);
        assert!(node.links.is_empty(), "parser accepted label {bad}");
        let msg = &node.parse_warnings[0].message;
        assert!(
            msg.contains(&LinkLabel::rejection(bad)),
            "parser warning {msg:?}"
        );
    }
}

#[test]
fn write_validation_accepts_exactly_the_stored_labels() {
    for l in LinkLabel::STORED {
        let v = serde_json::json!([{ "to": "t2", "label": l.as_str() }]);
        assert!(
            validate_links_value(&v).is_ok(),
            "write path rejected stored label {l}: {:?}",
            validate_links_value(&v)
        );
    }
    for bad in refused() {
        let v = serde_json::json!([{ "to": "t2", "label": bad }]);
        assert!(
            validate_links_value(&v).is_err(),
            "write path accepted {bad}"
        );
    }
}

#[test]
fn write_error_message_derives_from_the_definition() {
    for bad in refused() {
        let v = serde_json::json!([{ "to": "t2", "label": bad }]);
        let err = validate_links_value(&v).unwrap_err();
        assert_eq!(
            err,
            format!("links[0].label \"{bad}\" {}", LinkLabel::rejection(bad))
        );
        for l in LinkLabel::STORED {
            assert!(err.contains(l.as_str()), "write error {err:?} omits {l}");
        }
    }
}

#[test]
fn lint_accepts_exactly_the_stored_labels() {
    for l in LinkLabel::STORED {
        assert!(
            lint_label_diag(l.as_str()).is_none(),
            "pkb lint rejected stored label {l}: {:?}",
            lint_label_diag(l.as_str())
        );
    }
    for bad in refused() {
        let d = lint_label_diag(bad).unwrap_or_else(|| panic!("pkb lint accepted label {bad}"));
        assert!(
            d.message.contains(&LinkLabel::rejection(bad)),
            "lint message {:?}",
            d.message
        );
    }
}

#[test]
fn lint_padding_fix_covers_every_stored_label() {
    for l in LinkLabel::STORED {
        let content = doc_with_label(&format!("\"{} \"", l.as_str()));
        let f = write_tmp(&content);
        let res = lint_file(f.path(), true, None, None, None);
        let fixed = res.fixed_content.unwrap_or(content);
        assert!(
            fixed.contains(&format!("label: {}\n", l.as_str())),
            "lint --fix did not unpad stored label {l}: {fixed}"
        );
    }
}

#[test]
fn link_is_rejected_everywhere_and_names_serves() {
    // `link` names body wikilinks in links_in output; it is not a stored
    // flow label (flow-rule §5.2, Q21, Q23). Every consumer refuses it and
    // names `serves` as the replacement.
    assert!(LinkLabel::COMPUTED.contains(&LinkLabel::Link));
    assert_eq!(LinkLabel::Link.replacement(), Some(LinkLabel::Serves));
    assert_eq!(LinkLabel::from_str("link"), None);

    let v = serde_json::json!([{ "to": "t2", "label": "link" }]);
    let err = validate_links_value(&v).unwrap_err();
    assert!(err.contains("use `serves`"), "write error: {err}");

    let node = parse_node("link");
    assert!(node.links.is_empty());
    assert!(node.parse_warnings[0].message.contains("use `serves`"));

    let d = lint_label_diag("link").expect("lint flags link");
    assert!(
        d.message.contains("use `serves`"),
        "lint message: {}",
        d.message
    );
}

#[test]
fn computed_link_label_still_serialises_for_links_in() {
    assert_eq!(serde_json::to_value(LinkLabel::Link).unwrap(), "link");
    assert_eq!(serde_json::to_value(LinkLabel::PartOf).unwrap(), "part_of");
}
