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
