//! Structural validation for Excalidraw canvases (route B, `task_aops_d7b96134`).
//!
//! `cmd_check` in `src/bin/pkb_excalidraw.rs` already gates `pkb-excalidraw`'s own
//! writes inside `atomic_save`. The three MCP ingestion tools (`graph_excalidraw`,
//! `diff_excalidraw`, `sync_excalidraw`) did not go through any equivalent gate:
//! `parse_canvas` was a bare `serde_json::from_str`, and every top-level
//! [`super::schema::ExcalidrawFile`] field carries a serde default, so `{}` and any
//! other structurally broken payload deserialized as a "valid" (if empty) canvas.
//!
//! This module is the ingestion-side gate: [`validate_raw_shape`] rejects payloads
//! that don't look like a real Excalidraw scene at all (missing top-level
//! `elements`/`type`), and [`validate_file`] rejects a syntactically valid
//! [`super::schema::ExcalidrawFile`] whose elements would corrupt or fail to open
//! in the actual Excalidraw app. Both name the offending element or field in every
//! failure message so a caller can act on the report without re-deriving it.
//!
//! Deliberately out of scope here, both because `generate_excalidraw_scene` —
//! this crate's own canvas generator, used by `graph_excalidraw` — does not
//! produce them, which means hard-requiring them at the ingestion gate would
//! reject this tool's own output on the round trip (export -> edit ->
//! `sync_excalidraw`):
//! - The index-ordering/alphabet invariants (laws 3.1 and 3.2 in
//!   `specs/excalidraw-tooling.md`): the generator never emits an `index` field
//!   at all. These remain enforced solely by `pkb-excalidraw`'s `atomic_save`,
//!   the only writer that mints indices.
//! - The second half of law 3.5 (an arrow's bound endpoints must list the arrow
//!   back in their own `boundElements`): the generator binds arrows via
//!   `startBinding`/`endBinding` only and never populates the endpoint shapes'
//!   `boundElements` for arrows. [`validate_file`] still checks the half of this
//!   invariant that matters most for the failure mode recorded under Evidence on
//!   `task_aops_d7b96134` — a shape whose `boundElements` *does* list an arrow
//!   that no longer binds it at either end (see check 5, "stale boundElements").

use super::schema::ExcalidrawElement;
use std::collections::HashMap;

/// Validate the raw JSON text before typed deserialization. Rejects payloads that
/// don't declare the minimal top-level shape of a real Excalidraw scene — most
/// notably `{}`, which would otherwise silently deserialize into an empty but
/// "valid" [`super::schema::ExcalidrawFile`] because every top-level field carries
/// a serde default.
pub fn validate_raw_shape(json_str: &str) -> Result<(), Vec<String>> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| vec![format!("invalid JSON: {e}")])?;

    let obj = value
        .as_object()
        .ok_or_else(|| vec!["canvas JSON must be a top-level object".to_string()])?;

    let mut fails = Vec::new();
    if !obj.contains_key("elements") {
        fails.push(
            "canvas is missing required top-level field 'elements' — not a recognizable Excalidraw scene"
                .to_string(),
        );
    }
    match obj.get("type").and_then(|v| v.as_str()) {
        None => fails.push(
            "canvas is missing required top-level field 'type' — not a recognizable Excalidraw scene"
                .to_string(),
        ),
        Some(t) if t != "excalidraw" => {
            fails.push(format!("canvas 'type' must be \"excalidraw\", got {t:?}"))
        }
        _ => {}
    }

    if fails.is_empty() {
        Ok(())
    } else {
        Err(fails)
    }
}

/// Validate a typed, deserialized [`super::schema::ExcalidrawFile`]'s elements
/// against the invariants a canvas must hold to open in Excalidraw without
/// crashing, dropping elements, or silently corrupting bindings on the next
/// interaction (`specs/excalidraw-tooling.md` §3.3–3.5). Every failure names the
/// offending element by id.
pub fn validate_file(file: &super::schema::ExcalidrawFile) -> Result<(), Vec<String>> {
    let elements: Vec<&ExcalidrawElement> =
        file.elements.iter().filter(|e| !e.is_deleted).collect();

    let mut fails: Vec<String> = Vec::new();

    // 1. Duplicate element ids — Excalidraw keys everything off id; duplicates
    //    make bindings ambiguous and the file will not open cleanly.
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for e in &elements {
        *seen.entry(e.id.as_str()).or_insert(0) += 1;
    }
    let mut dup_ids: Vec<&str> = seen
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(id, _)| *id)
        .collect();
    if !dup_ids.is_empty() {
        dup_ids.sort_unstable();
        fails.push(format!("duplicate element ids: {}", dup_ids.join(", ")));
    }

    let by_id: HashMap<&str, &ExcalidrawElement> =
        elements.iter().map(|e| (e.id.as_str(), *e)).collect();

    for e in &elements {
        // 2. Half-bound arrows (law 3.5): exactly one of startBinding/endBinding
        //    populated corrupts the arrow's drag handles in the Excalidraw frontend.
        if e.element_type == "arrow" {
            let start_id = e
                .start_binding
                .as_ref()
                .map(|b| b.element_id.as_str())
                .filter(|s| !s.is_empty());
            let end_id = e
                .end_binding
                .as_ref()
                .map(|b| b.element_id.as_str())
                .filter(|s| !s.is_empty());
            if start_id.is_some() != end_id.is_some() {
                fails.push(format!(
                    "arrow {} is half-bound: startBinding={}, endBinding={} — Excalidraw corrupts drag handles on half-bound arrows",
                    e.id,
                    start_id.unwrap_or("none"),
                    end_id.unwrap_or("none"),
                ));
            }

            // 3. Dangling arrow bindings — the bound element must actually exist.
            //
            //    Note: law 3.5 in specs/excalidraw-tooling.md additionally
            //    requires the bound endpoint to list the arrow back in its own
            //    `boundElements`. That reciprocal is NOT enforced here:
            //    `generate_excalidraw_scene` (this crate's own canvas generator,
            //    used by `graph_excalidraw`) never sets it on the shapes an arrow
            //    connects, so hard-requiring it at the ingestion gate would
            //    reject this tool's own output on the round trip. Item 5 below
            //    still catches the inverse and higher-value case — a shape whose
            //    `boundElements` *does* list an arrow that no longer binds it
            //    (the "stale boundElements" class from Evidence on
            //    task_aops_d7b96134).
            for (side, binding) in [
                ("startBinding", &e.start_binding),
                ("endBinding", &e.end_binding),
            ] {
                let Some(b) = binding else { continue };
                if b.element_id.is_empty() {
                    continue;
                }
                if !by_id.contains_key(b.element_id.as_str()) {
                    fails.push(format!(
                        "arrow {}.{side} -> missing element {}",
                        e.id, b.element_id
                    ));
                }
            }
        }

        // 4. containerId -> missing container, or container lacking the backref
        //    (law 3.4, bidirectional container-text binding).
        if let Some(cid) = &e.container_id {
            match by_id.get(cid.as_str()) {
                None => fails.push(format!("text {} bound to missing container {}", e.id, cid)),
                Some(container) => {
                    let has_backref = container
                        .bound_elements
                        .as_ref()
                        .is_some_and(|bes| bes.iter().any(|be| be.id == e.id));
                    if !has_backref {
                        fails.push(format!(
                            "container {} lacks boundElements backref to text {}",
                            cid, e.id
                        ));
                    }
                }
            }
        }

        // 5. Forward boundElements entries: target missing, or target no longer
        //    points back (stale boundElements — the class recorded under Evidence
        //    on task_aops_d7b96134: a container/shape lists an element that no
        //    longer binds it at either end).
        if let Some(bound) = &e.bound_elements {
            for b in bound {
                match by_id.get(b.id.as_str()) {
                    None => fails.push(format!(
                        "{}.boundElements -> missing element {}",
                        e.id, b.id
                    )),
                    Some(target) => {
                        let reciprocal = match b.element_type.as_str() {
                            "text" => target.container_id.as_deref() == Some(e.id.as_str()),
                            "arrow" => {
                                target
                                    .start_binding
                                    .as_ref()
                                    .map(|sb| sb.element_id.as_str())
                                    == Some(e.id.as_str())
                                    || target.end_binding.as_ref().map(|eb| eb.element_id.as_str())
                                        == Some(e.id.as_str())
                            }
                            _ => true,
                        };
                        if !reciprocal {
                            fails.push(format!(
                                "{} lists stale boundElements entry {} (type {:?}) that no longer binds back",
                                e.id, b.id, b.element_type
                            ));
                        }
                    }
                }
            }
        }

        // 6. Dual text synchronization (law 3.3): text and originalText must agree
        //    on content, not just wrapping, or one layer silently overwrites the
        //    other on the next edit.
        if let (Some(t), Some(o)) = (&e.text, &e.original_text) {
            let t_words: Vec<&str> = t.split_whitespace().collect();
            let o_words: Vec<&str> = o.split_whitespace().collect();
            if t_words != o_words {
                fails.push(format!(
                    "{}: text and originalText disagree in content, not just wrapping",
                    e.id
                ));
            }
        }
    }

    if fails.is_empty() {
        Ok(())
    } else {
        Err(fails)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::excalidraw::schema::{
        BoundElement, ExcalidrawElement, ExcalidrawFile, PointBinding,
    };

    fn elem(id: &str, element_type: &str) -> ExcalidrawElement {
        ExcalidrawElement {
            id: id.to_string(),
            element_type: element_type.to_string(),
            ..ExcalidrawElement::default()
        }
    }

    #[test]
    fn empty_object_fails_raw_shape() {
        let err = validate_raw_shape("{}").unwrap_err();
        assert!(err.iter().any(|f| f.contains("elements")));
        assert!(err.iter().any(|f| f.contains("type")));
    }

    #[test]
    fn well_formed_empty_canvas_passes_raw_shape() {
        assert!(validate_raw_shape(r#"{"type":"excalidraw","version":2,"elements":[]}"#).is_ok());
    }

    #[test]
    fn wrong_type_field_fails_raw_shape() {
        let err = validate_raw_shape(r#"{"type":"excalidrawlib","elements":[]}"#).unwrap_err();
        assert!(err.iter().any(|f| f.contains("excalidraw")));
    }

    #[test]
    fn half_bound_arrow_is_rejected_naming_the_arrow() {
        let mut file = ExcalidrawFile::default();
        let mut arrow = elem("arrow-1", "arrow");
        arrow.start_binding = Some(PointBinding {
            element_id: "shape-1".to_string(),
            focus: 0.0,
            gap: 1.0,
            fixed_point: None,
        });
        file.elements.push(elem("shape-1", "rectangle"));
        file.elements.push(arrow);

        let err = validate_file(&file).unwrap_err();
        assert!(err
            .iter()
            .any(|f| f.contains("arrow-1") && f.contains("half-bound")));
    }

    #[test]
    fn stale_bound_elements_backref_is_rejected() {
        // Container claims arrow-9 as a bound element, but arrow-9's own bindings
        // don't point back at either end — exactly the class recorded under
        // Evidence on task_aops_d7b96134.
        let mut file = ExcalidrawFile::default();
        let mut container = elem("c1", "rectangle");
        container.bound_elements = Some(vec![BoundElement {
            id: "arrow-9".to_string(),
            element_type: "arrow".to_string(),
        }]);
        let mut other_a = elem("other-a", "rectangle");
        let mut other_b = elem("other-b", "rectangle");
        let mut arrow = elem("arrow-9", "arrow");
        arrow.start_binding = Some(PointBinding {
            element_id: "other-a".to_string(),
            focus: 0.0,
            gap: 1.0,
            fixed_point: None,
        });
        arrow.end_binding = Some(PointBinding {
            element_id: "other-b".to_string(),
            focus: 0.0,
            gap: 1.0,
            fixed_point: None,
        });
        other_a.bound_elements = Some(vec![BoundElement {
            id: "arrow-9".to_string(),
            element_type: "arrow".to_string(),
        }]);
        other_b.bound_elements = Some(vec![BoundElement {
            id: "arrow-9".to_string(),
            element_type: "arrow".to_string(),
        }]);

        file.elements.push(container);
        file.elements.push(other_a);
        file.elements.push(other_b);
        file.elements.push(arrow);

        let err = validate_file(&file).unwrap_err();
        assert!(
            err.iter()
                .any(|f| f.contains("c1") && f.contains("stale") && f.contains("arrow-9")),
            "expected stale boundElements failure naming c1/arrow-9, got: {err:?}"
        );
    }

    #[test]
    fn text_original_text_content_drift_is_rejected() {
        let mut file = ExcalidrawFile::default();
        let mut text = elem("t1", "text");
        text.text = Some("Hello there".to_string());
        text.original_text = Some("Goodbye there".to_string());
        file.elements.push(text);

        let err = validate_file(&file).unwrap_err();
        assert!(err
            .iter()
            .any(|f| f.contains("t1") && f.contains("originalText")));
    }

    #[test]
    fn wrapping_only_difference_in_text_is_allowed() {
        let mut file = ExcalidrawFile::default();
        let mut text = elem("t1", "text");
        text.text = Some("Hello\nthere".to_string());
        text.original_text = Some("Hello there".to_string());
        file.elements.push(text);

        assert!(validate_file(&file).is_ok());
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let mut file = ExcalidrawFile::default();
        file.elements.push(elem("dup", "rectangle"));
        file.elements.push(elem("dup", "rectangle"));

        let err = validate_file(&file).unwrap_err();
        assert!(err.iter().any(|f| f.contains("dup")));
    }

    #[test]
    fn well_formed_two_bound_arrow_passes() {
        let mut file = ExcalidrawFile::default();
        let mut a = elem("a", "rectangle");
        let mut b = elem("b", "rectangle");
        let mut arrow = elem("arrow-ok", "arrow");
        arrow.start_binding = Some(PointBinding {
            element_id: "a".to_string(),
            focus: 0.0,
            gap: 1.0,
            fixed_point: None,
        });
        arrow.end_binding = Some(PointBinding {
            element_id: "b".to_string(),
            focus: 0.0,
            gap: 1.0,
            fixed_point: None,
        });
        a.bound_elements = Some(vec![BoundElement {
            id: "arrow-ok".to_string(),
            element_type: "arrow".to_string(),
        }]);
        b.bound_elements = Some(vec![BoundElement {
            id: "arrow-ok".to_string(),
            element_type: "arrow".to_string(),
        }]);
        file.elements.push(a);
        file.elements.push(b);
        file.elements.push(arrow);

        assert!(validate_file(&file).is_ok());
    }

    #[test]
    fn deleted_elements_are_excluded_from_validation() {
        // A half-bound arrow that is isDeleted should not trip validation —
        // mirrors `live()` filtering in cmd_check.
        let mut file = ExcalidrawFile::default();
        let mut arrow = elem("arrow-deleted", "arrow");
        arrow.is_deleted = true;
        arrow.start_binding = Some(PointBinding {
            element_id: "nonexistent".to_string(),
            focus: 0.0,
            gap: 1.0,
            fixed_point: None,
        });
        file.elements.push(arrow);

        assert!(validate_file(&file).is_ok());
    }
}
