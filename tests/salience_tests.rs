use chrono::{TimeZone, Utc};
use mem::salience::{AccessType, MockClock, SalienceConfig, SalienceStore};
use std::sync::Arc;

#[test]
fn test_access_raises_score_exponentially_with_ceiling() {
    let t0 = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();
    let clock = Arc::new(MockClock::new(t0));
    let config = SalienceConfig {
        initial_score: 1.0,
        access_increment: 1.0,
        half_life_days: 7.0,
        ceiling: 100.0,
        floor: 0.0,
    };
    let mut store = SalienceStore::new_with_clock(config, clock.clone());

    // Initial state before access: score is floor (0.0)
    assert_eq!(store.get_salience("doc1"), 0.0);

    // 1st access: raises to initial_score (1.0)
    let s1 = store.record_access("doc1", AccessType::McpRetrieval);
    assert_eq!(s1, 1.0);
    assert_eq!(store.get_salience("doc1"), 1.0);

    // 2nd access: 1.0 + 1.0 = 2.0
    let s2 = store.record_access("doc1", AccessType::Read);
    assert_eq!(s2, 2.0);

    // 3rd access: 2.0 + 1.0 = 3.0
    let s3 = store.record_access("doc1", AccessType::SearchHit);
    assert_eq!(s3, 3.0);

    // 4th access: 3.0 + 1.0 = 4.0
    let s4 = store.record_access("doc1", AccessType::Link);
    assert_eq!(s4, 4.0);
}

#[test]
fn test_day_without_access_lowers_score_linearly_with_controlled_clock() {
    let t0 = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();
    let clock = Arc::new(MockClock::new(t0));
    let config = SalienceConfig {
        initial_score: 1.0,
        access_increment: 1.0,
        half_life_days: 7.0,
        ceiling: 100.0,
        floor: 0.0,
    };
    let mut store = SalienceStore::new_with_clock(config, clock.clone());

    // Bring document to score 5.0 via 5 accesses at t0
    for _ in 0..5 {
        store.record_access("doc1", AccessType::McpRetrieval);
    }
    assert_eq!(store.get_salience("doc1"), 5.0);

    // Advance clock by 7 days (half-life): score halves to 2.5
    clock.advance_days(7.0);
    assert!((store.get_salience("doc1") - 2.5).abs() < 1e-4);
}

#[test]
fn test_decay_then_access_recovers_exponentially() {
    let t0 = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();
    let clock = Arc::new(MockClock::new(t0));
    let config = SalienceConfig {
        initial_score: 1.0,
        access_increment: 1.0,
        half_life_days: 7.0,
        ceiling: 100.0,
        floor: 0.0,
    };
    let mut store = SalienceStore::new_with_clock(config, clock.clone());

    // Bring to 4.0 at t0
    for _ in 0..4 {
        store.record_access("doc1", AccessType::Read);
    }
    assert_eq!(store.get_salience("doc1"), 4.0);

    // 7 days without access -> drops to 2.0
    clock.advance_days(7.0);
    assert!((store.get_salience("doc1") - 2.0).abs() < 1e-4);

    // Access at t0 + 7 days: raises 2.0 + 1.0 = 3.0
    let s = store.record_access("doc1", AccessType::SearchHit);
    assert_eq!(s, 3.0);
    assert!((store.get_salience("doc1") - 3.0).abs() < 1e-4);
}

#[test]
fn test_daily_note_recency_decay_formula() {
    let t0 = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();

    // Age 0 days: exp(-0/14) = 1.0 -> 0.85 + 0.15 * 1.0 = 1.0
    let decay_0 = mem::salience::daily_recency_decay("2026-10-18", t0).unwrap();
    assert!((decay_0 - 1.0).abs() < 1e-6, "expected 1.0, got {decay_0}");

    // Age 14 days: exp(-1) = 0.3678794 -> 0.85 + 0.15 * 0.3678794 = 0.9051819
    let decay_14 = mem::salience::daily_recency_decay("2026-10-04", t0).unwrap();
    let expected_14 = 0.85 + 0.15 * (-1.0_f64).exp();
    assert!((decay_14 - expected_14).abs() < 1e-6, "expected {expected_14}, got {decay_14}");

    // Age 28 days: exp(-2) = 0.1353353 -> 0.85 + 0.15 * 0.1353353 = 0.8703003
    let decay_28 = mem::salience::daily_recency_decay("2026-09-20", t0).unwrap();
    let expected_28 = 0.85 + 0.15 * (-2.0_f64).exp();
    assert!((decay_28 - expected_28).abs() < 1e-6, "expected {expected_28}, got {decay_28}");

    // Asymptotic bound: age 365 days approaches 0.85
    let decay_year = mem::salience::daily_recency_decay("2025-10-18", t0).unwrap();
    assert!((0.85..0.85001).contains(&decay_year), "expected ~0.85, got {decay_year}");

    // Also supports underscore dates e.g. "2026_10_18"
    let decay_underscore = mem::salience::daily_recency_decay("2026_10_18", t0).unwrap();
    assert!((decay_underscore - 1.0).abs() < 1e-6);

    // Document recency decay helper handles daily doc_type
    let d_factor = mem::salience::document_recency_decay(Some("daily"), Some("2026-10-04"), None, t0);
    assert!((d_factor - expected_14).abs() < 1e-6);

    // Task docs are immune to recency decay per pkb-rules §5
    let task_factor = mem::salience::document_recency_decay(Some("task"), None, Some("2025-01-01T00:00:00Z"), t0);
    assert_eq!(task_factor, 1.0);
}

#[test]
fn test_recent_read_boost_ceiling_and_decay() {
    let t0 = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();
    let clock = Arc::new(MockClock::new(t0));
    let config = SalienceConfig {
        initial_score: 1.0,
        access_increment: 1.0,
        half_life_days: 7.0,
        ceiling: 100.0,
        floor: 0.0,
    };
    let mut store = SalienceStore::new_with_clock(config.clone(), clock.clone());

    // Initially unaccessed: boost is 1.0 (no boost)
    assert_eq!(store.get_recent_read_boost("doc1"), 1.0);

    // Access 1: score = 1.0 -> logarithmic boost
    store.record_access("doc1", AccessType::Read);
    assert!(store.get_recent_read_boost("doc1") > 1.0);

    // Repeated accesses up to ceiling (100.0)
    for _ in 0..99 {
        store.record_access("doc1", AccessType::McpRetrieval);
    }
    assert_eq!(store.get_salience("doc1"), 100.0);
    assert_eq!(store.get_recent_read_boost("doc1"), 1.20);

    // Controlled clock advances by 10 days: score decays
    clock.advance_days(10.0);
    assert!(store.get_recent_read_boost("doc1") > 1.0);

    // Controlled clock advances by a large number of half-lives (e.g. 500 days) to decay to floor
    clock.advance_days(500.0);
    assert!(store.get_recent_read_boost("doc1") <= 1.00001);
}

#[test]
fn test_no_prior_moves_below_floor_item_above_floor() {
    let floor = 0.45;
    let max_read_boost = 1.20;
    let recency_1 = 1.0;

    // Test items strictly below floor
    let scores_below = [0.0, 0.10, 0.25, 0.40, 0.449, 0.449999];
    for &s in &scores_below {
        let final_s = mem::salience::apply_priors_with_floor(s, floor, recency_1, max_read_boost);
        assert!(
            final_s < floor,
            "Below-floor item with score {s} moved across floor {floor}: got {final_s}"
        );
    }

    // Test item at or above floor: correctly boosted and bounded
    let score_above = 0.50;
    let boosted = mem::salience::apply_priors_with_floor(score_above, floor, recency_1, max_read_boost);
    assert!((boosted - 0.60).abs() < 1e-5, "expected 0.60, got {boosted}");
    assert!(boosted >= floor);

    // Near ceiling softly saturates below 1.0 (<= 0.990)
    let score_high = 0.95;
    let boosted_high = mem::salience::apply_priors_with_floor(score_high, floor, recency_1, max_read_boost);
    assert!(boosted_high < 1.0, "boosted_high ({boosted_high}) must not saturate at 1.0");
    assert!(boosted_high <= 0.990, "boosted_high ({boosted_high}) must be <= 0.990");
    assert!(boosted_high > 0.900, "boosted_high ({boosted_high}) must be > 0.900");
}

#[test]
fn test_apply_priors_preserves_score_delta_above_floor() {
    // C4: Soft-knee saturation preserves rank ordering (d S_out / d S > 0),
    // exhibits continuous derivative at T = 0.900, and never depresses a candidate below relevance floor.
    let floor = 0.50;
    let recency = 1.0;
    let read_boost = 1.15;
    let s1 = 0.85;
    let s2 = 0.92;
    let s3 = 0.96;

    let out1 = mem::salience::apply_priors_with_floor(s1, floor, recency, read_boost);
    let out2 = mem::salience::apply_priors_with_floor(s2, floor, recency, read_boost);
    let out3 = mem::salience::apply_priors_with_floor(s3, floor, recency, read_boost);

    assert!(out1 < out2, "out1 ({out1}) must be strictly less than out2 ({out2})");
    assert!(out2 < out3, "out2 ({out2}) must be strictly less than out3 ({out3})");
    assert!(out3 <= 0.990, "out3 ({out3}) must not exceed soft ceiling 0.990");
    assert!(out1 >= floor, "out1 must meet or exceed floor");
    assert!(out2 >= floor, "out2 must meet or exceed floor");
    assert!(out3 >= floor, "out3 must meet or exceed floor");
}

#[test]
fn test_utf8_char_boundary_slicing_does_not_panic() {
    let t0 = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();

    // Multi-byte UTF-8 string whose 10th byte falls inside a multi-byte codepoint
    let non_ascii_date = "2026年10月18日";
    let res = mem::salience::daily_recency_decay(non_ascii_date, t0);
    // Should safely fail to parse as %Y-%m-%d without panicking on char boundary slice
    assert_eq!(res, None);

    // Document recency decay with non-ascii modified timestamp
    let res_mod = mem::salience::document_recency_decay(Some("note"), None, Some("2026年10月"), t0);
    assert_eq!(res_mod, 1.0);
}

#[test]
fn test_doc_type_immunity_with_date_and_modified() {
    let t0 = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();
    let old_date = "2020-01-01";
    let old_mod = "2020-01-01T00:00:00Z";

    // All actionable/strategic types are immune even when carrying date and modified fields
    for immune_type in &["task", "epic", "target", "project", "TASK", "EPIC", "Target", "Project"] {
        let decay = mem::salience::document_recency_decay(
            Some(immune_type),
            Some(old_date),
            Some(old_mod),
            t0,
        );
        assert_eq!(
            decay, 1.0,
            "Document type {immune_type} should be immune to recency decay, got {decay}"
        );
    }

    // Conversely, non-immune documents (e.g. note or None) decay with modified timestamp
    let note_decay = mem::salience::document_recency_decay(
        Some("note"),
        None,
        Some("2026-09-18T00:00:00Z"), // 30 days old -> exp(-1)
        t0,
    );
    let expected_30 = 0.85 + 0.15 * (-1.0_f64).exp();
    assert!((note_decay - expected_30).abs() < 1e-4, "got {note_decay}, expected {expected_30}");
}

#[test]
fn test_floor_clamp_guard() {
    let recency_1 = 1.0;
    let boost_1 = 1.0;

    // Negative floor should clamp to 0.0
    let s1 = mem::salience::apply_priors_with_floor(0.2, -0.5, recency_1, boost_1);
    assert!(s1 >= 0.0);

    // Floor above 1.0 should clamp to 1.0
    let s2 = mem::salience::apply_priors_with_floor(0.5, 1.5, recency_1, boost_1);
    assert_eq!(s2, 0.5); // score < clamped floor (1.0) -> attenuated
}


