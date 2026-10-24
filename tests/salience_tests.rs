use chrono::{TimeZone, Utc};
use mem::salience::{AccessType, MockClock, SalienceConfig, SalienceStore};
use std::sync::Arc;

#[test]
fn test_access_raises_score_exponentially_with_ceiling() {
    let t0 = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();
    let clock = Arc::new(MockClock::new(t0));
    let config = SalienceConfig {
        initial_score: 1.0,
        growth_factor: 2.0,
        decay_per_day: 1.0,
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

    // 2nd access: 1.0 * 2.0 = 2.0
    let s2 = store.record_access("doc1", AccessType::Read);
    assert_eq!(s2, 2.0);

    // 3rd access: 2.0 * 2.0 = 4.0
    let s3 = store.record_access("doc1", AccessType::SearchHit);
    assert_eq!(s3, 4.0);

    // 4th access: 4.0 * 2.0 = 8.0
    let s4 = store.record_access("doc1", AccessType::Link);
    assert_eq!(s4, 8.0);

    // 5th access: 8.0 * 2.0 = 16.0
    let s5 = store.record_access("doc1", AccessType::McpRetrieval);
    assert_eq!(s5, 16.0);

    // 6th access: 16.0 * 2.0 = 32.0
    let s6 = store.record_access("doc1", AccessType::McpRetrieval);
    assert_eq!(s6, 32.0);

    // 7th access: 32.0 * 2.0 = 64.0
    let s7 = store.record_access("doc1", AccessType::McpRetrieval);
    assert_eq!(s7, 64.0);

    // 8th access: 64.0 * 2.0 = 128.0 -> clamped at ceiling 100.0
    let s8 = store.record_access("doc1", AccessType::McpRetrieval);
    assert_eq!(s8, 100.0);
    assert_eq!(store.get_salience("doc1"), 100.0);

    // Further access remains at ceiling
    let s9 = store.record_access("doc1", AccessType::McpRetrieval);
    assert_eq!(s9, 100.0);
}

#[test]
fn test_day_without_access_lowers_score_linearly_with_controlled_clock() {
    let t0 = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();
    let clock = Arc::new(MockClock::new(t0));
    let config = SalienceConfig {
        initial_score: 1.0,
        growth_factor: 2.0,
        decay_per_day: 1.0,
        ceiling: 100.0,
        floor: 0.0,
    };
    let mut store = SalienceStore::new_with_clock(config, clock.clone());

    // Bring document to score 16.0 via 5 accesses at t0
    for _ in 0..5 {
        store.record_access("doc1", AccessType::McpRetrieval);
    }
    assert_eq!(store.get_salience("doc1"), 16.0);

    // Advance clock by 1 day (controlled clock)
    clock.advance_days(1.0);
    // Score should decay linearly by decay_per_day (1.0) -> 15.0
    assert_eq!(store.get_salience("doc1"), 15.0);

    // Advance clock by another day (total 2 days)
    clock.advance_days(1.0);
    // Score should be 14.0
    assert_eq!(store.get_salience("doc1"), 14.0);

    // Advance clock by 3.5 days (total 5.5 days without access)
    clock.advance_days(3.5);
    // Score should be 16.0 - 5.5 = 10.5
    assert_eq!(store.get_salience("doc1"), 10.5);

    // Advance clock by 20 days (past floor)
    clock.advance_days(20.0);
    // Score should be clamped at floor 0.0
    assert_eq!(store.get_salience("doc1"), 0.0);
}

#[test]
fn test_decay_then_access_recovers_exponentially() {
    let t0 = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();
    let clock = Arc::new(MockClock::new(t0));
    let config = SalienceConfig {
        initial_score: 1.0,
        growth_factor: 2.0,
        decay_per_day: 1.0,
        ceiling: 100.0,
        floor: 0.0,
    };
    let mut store = SalienceStore::new_with_clock(config, clock.clone());

    // Bring to 8.0 at t0
    for _ in 0..4 {
        store.record_access("doc1", AccessType::Read);
    }
    assert_eq!(store.get_salience("doc1"), 8.0);

    // 2 days without access -> drops to 6.0
    clock.advance_days(2.0);
    assert_eq!(store.get_salience("doc1"), 6.0);

    // Access at t0 + 2 days: raises 6.0 * 2.0 = 12.0
    let s = store.record_access("doc1", AccessType::SearchHit);
    assert_eq!(s, 12.0);
    assert_eq!(store.get_salience("doc1"), 12.0);
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
        growth_factor: 2.0,
        decay_per_day: 1.0,
        ceiling: 100.0,
        floor: 0.0,
    };
    let mut store = SalienceStore::new_with_clock(config.clone(), clock.clone());

    // Initially unaccessed: boost is 1.0 (no boost)
    assert_eq!(store.get_recent_read_boost("doc1"), 1.0);

    // Access 1: score = 1.0 -> boost = 1.0 + 0.20 * (1.0 / 100.0) = 1.002
    store.record_access("doc1", AccessType::Read);
    assert!((store.get_recent_read_boost("doc1") - 1.002).abs() < 1e-5);

    // Repeated accesses up to ceiling: score = 100.0 -> boost = 1.0 + 0.20 * 1.0 = 1.20
    for _ in 0..10 {
        store.record_access("doc1", AccessType::McpRetrieval);
    }
    assert_eq!(store.get_salience("doc1"), 100.0);
    assert_eq!(store.get_recent_read_boost("doc1"), 1.20);

    // Controlled clock advances by 10 days: score decays to 90.0
    clock.advance_days(10.0);
    assert_eq!(store.get_salience("doc1"), 90.0);
    assert!((store.get_recent_read_boost("doc1") - 1.18).abs() < 1e-5);

    // Controlled clock advances by 90 more days: decays to floor 0.0 -> boost back to 1.0
    clock.advance_days(90.0);
    assert_eq!(store.get_salience("doc1"), 0.0);
    assert_eq!(store.get_recent_read_boost("doc1"), 1.0);
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

    // Near ceiling clamped to 1.0
    let score_high = 0.95;
    let boosted_high = mem::salience::apply_priors_with_floor(score_high, floor, recency_1, max_read_boost);
    assert_eq!(boosted_high, 1.0);
}

