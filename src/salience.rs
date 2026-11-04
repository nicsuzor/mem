//! Document salience scoring with exponential access growth and linear time decay.
//!
//! Documents accumulate salience exponentially with each access event (read, search hit,
//! link, MCP retrieval) up to a configurable ceiling. Without access, salience decays
//! linearly per day toward a floor (0.0).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Clock trait for testing time-dependent decay with controlled clocks.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// Production system clock using wall-clock UTC time.
#[derive(Debug, Clone, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Controlled mock clock for reproducible, instant time manipulation in tests.
#[derive(Debug)]
pub struct MockClock {
    current_time: parking_lot::RwLock<DateTime<Utc>>,
}

impl MockClock {
    pub fn new(initial: DateTime<Utc>) -> Self {
        Self {
            current_time: parking_lot::RwLock::new(initial),
        }
    }

    pub fn set(&self, time: DateTime<Utc>) {
        *self.current_time.write() = time;
    }

    pub fn advance_days(&self, days: f64) {
        let seconds = (days * 86400.0).round() as i64;
        let mut lock = self.current_time.write();
        *lock += chrono::Duration::seconds(seconds);
    }

    pub fn advance_hours(&self, hours: f64) {
        let seconds = (hours * 3600.0).round() as i64;
        let mut lock = self.current_time.write();
        *lock += chrono::Duration::seconds(seconds);
    }
}

impl Clock for MockClock {
    fn now(&self) -> DateTime<Utc> {
        *self.current_time.read()
    }
}

/// Types of access that increase a document's salience.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessType {
    Read,
    SearchHit,
    Link,
    McpRetrieval,
}

impl AccessType {
    pub fn as_str(&self) -> &'static str {
        match self {
            AccessType::Read => "read",
            AccessType::SearchHit => "search_hit",
            AccessType::Link => "link",
            AccessType::McpRetrieval => "mcp_retrieval",
        }
    }
}

/// Configuration parameters for exponential growth and linear decay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SalienceConfig {
    /// Score granted on first access (or after decaying to floor).
    pub initial_score: f64,
    /// Multiplier applied to current score on each subsequent access.
    pub growth_factor: f64,
    /// Linear points lost per 24-hour day without access.
    pub decay_per_day: f64,
    /// Maximum score clamp ceiling.
    pub ceiling: f64,
    /// Minimum score clamp floor.
    pub floor: f64,
}

impl Default for SalienceConfig {
    fn default() -> Self {
        Self {
            initial_score: 1.0,
            growth_factor: 2.0,
            decay_per_day: 1.0,
            ceiling: 100.0,
            floor: 0.0,
        }
    }
}

/// Historical and current salience record for a single document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SalienceEntry {
    pub id: String,
    pub score: f64,
    pub last_accessed: DateTime<Utc>,
    pub access_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_access_type: Option<AccessType>,
}

impl SalienceEntry {
    pub fn new(id: String, initial_score: f64, now: DateTime<Utc>, access_type: AccessType) -> Self {
        Self {
            id,
            score: initial_score,
            last_accessed: now,
            access_count: 1,
            last_access_type: Some(access_type),
        }
    }

    /// Calculate the decayed score at a given timestamp without mutating state.
    pub fn score_at(&self, now: DateTime<Utc>, config: &SalienceConfig) -> f64 {
        if self.score <= config.floor {
            return config.floor;
        }
        let elapsed_seconds = (now - self.last_accessed).num_seconds().max(0) as f64;
        let elapsed_days = elapsed_seconds / 86400.0;
        let decayed = self.score - (elapsed_days * config.decay_per_day);
        decayed.clamp(config.floor, config.ceiling)
    }
}

/// In-memory store tracking salience scores across documents.
pub struct SalienceStore {
    entries: HashMap<String, SalienceEntry>,
    config: SalienceConfig,
    clock: Arc<dyn Clock>,
}

impl SalienceStore {
    pub fn new(config: SalienceConfig) -> Self {
        Self {
            entries: HashMap::new(),
            config,
            clock: Arc::new(SystemClock),
        }
    }

    pub fn new_with_clock(config: SalienceConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            entries: HashMap::new(),
            config,
            clock,
        }
    }

    pub fn set_clock(&mut self, clock: Arc<dyn Clock>) {
        self.clock = clock;
    }

    pub fn config(&self) -> &SalienceConfig {
        &self.config
    }

    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    /// Read the recent-read boost factor in [1.0, 1.20] for a document.
    pub fn get_recent_read_boost(&self, id: &str) -> f64 {
        let salience = self.get_salience(id);
        recent_read_boost(salience, &self.config)
    }

    /// Read the current decayed salience score for a document.
    pub fn get_salience(&self, id: &str) -> f64 {
        let now = self.clock.now();
        self.entries
            .get(id)
            .map(|e| e.score_at(now, &self.config))
            .unwrap_or(self.config.floor)
    }

    /// Read the entry for a document, with its score decayed to current clock time.
    pub fn get_entry(&self, id: &str) -> Option<SalienceEntry> {
        let now = self.clock.now();
        self.entries.get(id).map(|e| {
            let mut res = e.clone();
            res.score = e.score_at(now, &self.config);
            res
        })
    }

    /// Record an access event for a document, raising its score exponentially.
    pub fn record_access(&mut self, id: &str, access_type: AccessType) -> f64 {
        let now = self.clock.now();
        let config = &self.config;
        let entry = self.entries.entry(id.to_string()).or_insert_with(|| {
            SalienceEntry {
                id: id.to_string(),
                score: config.floor,
                last_accessed: now,
                access_count: 0,
                last_access_type: None,
            }
        });

        let current_decayed = if entry.access_count == 0 {
            config.floor
        } else {
            entry.score_at(now, config)
        };

        let new_score = if current_decayed <= config.floor {
            config.initial_score
        } else {
            (current_decayed * config.growth_factor).min(config.ceiling)
        };

        entry.score = new_score;
        entry.last_accessed = now;
        entry.access_count += 1;
        entry.last_access_type = Some(access_type);

        new_score
    }

    /// Record access for multiple documents at once (e.g. search hits).
    pub fn record_access_batch(&mut self, ids: &[String], access_type: AccessType) -> HashMap<String, f64> {
        let mut results = HashMap::new();
        for id in ids {
            let s = self.record_access(id, access_type);
            results.insert(id.clone(), s);
        }
        results
    }

    /// Seed or set an initial score for a document (e.g. from frontmatter).
    pub fn set_initial_score(&mut self, id: &str, score: f64, timestamp: DateTime<Utc>) {
        let clamped = score.clamp(self.config.floor, self.config.ceiling);
        self.entries.insert(
            id.to_string(),
            SalienceEntry {
                id: id.to_string(),
                score: clamped,
                last_accessed: timestamp,
                access_count: 1,
                last_access_type: None,
            },
        );
    }

    /// Return top N documents ordered by decayed salience descending.
    pub fn top_salient(&self, n: usize) -> Vec<(String, f64)> {
        let now = self.clock.now();
        let mut list: Vec<(String, f64)> = self
            .entries
            .iter()
            .map(|(id, e)| (id.clone(), e.score_at(now, &self.config)))
            .collect();
        list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        list.truncate(n);
        list
    }

    /// Return all active (non-floor) document IDs and scores.
    pub fn iter_scores(&self) -> Vec<(String, f64)> {
        let now = self.clock.now();
        self.entries
            .iter()
            .map(|(id, e)| (id.clone(), e.score_at(now, &self.config)))
            .filter(|(_, s)| *s > self.config.floor)
            .collect()
    }
}

/// Compute daily note recency decay factor: `0.85 + 0.15 * exp(-age_days / 14.0)`
/// where age_days is elapsed days between `date` and `now`.
pub fn daily_recency_decay(date_str: &str, now: DateTime<Utc>) -> Option<f64> {
    let clean = date_str.trim().replace('_', "-");
    let candidate = if clean.len() >= 10 {
        &clean[..clean.floor_char_boundary(10)]
    } else {
        &clean
    };
    let date = chrono::NaiveDate::parse_from_str(candidate, "%Y-%m-%d").ok()?;
    let now_date = now.date_naive();
    let age_days = (now_date - date).num_days().max(0) as f64;
    Some(0.85 + 0.15 * (-age_days / 14.0).exp())
}

/// Multiplicative recent-read access boost derived from salience score.
/// Bounded between 1.0 (unaccessed or decayed to floor) and 1.20 (at ceiling).
pub fn recent_read_boost(salience_score: f64, config: &SalienceConfig) -> f64 {
    if salience_score <= config.floor {
        return 1.0;
    }
    const MAX_BOOST: f64 = 0.20;
    let range = (config.ceiling - config.floor).max(1e-9);
    let normalized_salience = ((salience_score - config.floor) / range).clamp(0.0, 1.0);
    1.0 + MAX_BOOST * normalized_salience
}

/// Compute document recency decay factor under clock `now`.
/// Daily notes decay according to `0.85 + 0.15 * exp(-age_days / 14)` from their `date` field.
/// Other documents with a modified timestamp decay gradually, while tasks do not decay
/// (age is not staleness per pkb-rules §5).
pub fn document_recency_decay(
    doc_type: Option<&str>,
    date: Option<&str>,
    modified: Option<&str>,
    now: DateTime<Utc>,
) -> f64 {
    if let Some(t) = doc_type {
        if matches!(t.to_ascii_lowercase().as_str(), "task" | "epic" | "target" | "project") {
            return 1.0;
        }
    }
    let is_daily = doc_type.map(|t| t.eq_ignore_ascii_case("daily")).unwrap_or(false);
    if let Some(d) = date {
        if is_daily || doc_type.is_none() || d.len() >= 10 {
            if let Some(decay) = daily_recency_decay(d, now) {
                return decay;
            }
        }
    }
    if let Some(m) = modified {
        if let Ok(mod_dt) = DateTime::parse_from_rfc3339(m) {
            let age_days = (now - mod_dt.with_timezone(&Utc)).num_seconds().max(0) as f64 / 86400.0;
            return 0.85 + 0.15 * (-age_days / 30.0).exp();
        } else if let Ok(mod_date) = chrono::NaiveDate::parse_from_str(&m[..m.floor_char_boundary(10)], "%Y-%m-%d") {
            let age_days = (now.date_naive() - mod_date).num_days().max(0) as f64;
            return 0.85 + 0.15 * (-age_days / 30.0).exp();
        }
    }
    1.0
}

/// Continuous soft-knee saturation curve activating at threshold T = 0.900
/// and asymptotically approaching ceiling C = 0.990.
///
/// Preserves C^0 continuity at T, C^1 smooth derivative (unit slope at T),
/// strict monotonicity, and strictly bounds non-exact scores <= 0.990.
pub fn soft_knee_ceiling(s: f64) -> f64 {
    const T: f64 = 0.900;
    const C: f64 = 0.990;
    if s <= T {
        s
    } else {
        let k = C - T; // 0.090
        let delta = s - T;
        T + (k * delta) / (k + delta)
    }
}

/// Apply multiplicative priors (recency decay and recent-read boost) to a normalized
/// relevance score in [0, 1].
///
/// Invariance: No recency or recent-read prior moves a below-floor item above the relevance floor.
/// Priors only reorder items that are already at or above the floor.
/// Continuous soft-knee saturation ensures boosted scores never plateau at 1.000.
pub fn apply_priors_with_floor(
    score: f64,
    floor: f64,
    recency_factor: f64,
    recent_read_boost: f64,
) -> f64 {
    let floor = floor.clamp(0.0, 1.0);
    if score < floor {
        // Below floor: never move across the relevance floor
        let attenuated = score * recency_factor;
        attenuated.min(score).min(if floor > 1e-6 { floor - 1e-6 } else { floor }).max(0.0)
    } else {
        // At or above floor: apply bounded multiplicative priors with soft-knee ceiling
        let combined = recency_factor * recent_read_boost;
        let boosted = score * combined;
        soft_knee_ceiling(boosted).max(floor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soft_knee_ceiling_properties() {
        // C0 continuity at T = 0.900
        assert_eq!(soft_knee_ceiling(0.900), 0.900);
        assert_eq!(soft_knee_ceiling(0.850), 0.850);

        // Strictly monotonic above T
        let s1 = soft_knee_ceiling(0.920);
        let s2 = soft_knee_ceiling(0.960);
        let s3 = soft_knee_ceiling(1.050);
        let s4 = soft_knee_ceiling(1.500);
        let s5 = soft_knee_ceiling(10.0);

        assert!(s1 < s2, "s1 ({s1}) < s2 ({s2})");
        assert!(s2 < s3, "s2 ({s2}) < s3 ({s3})");
        assert!(s3 < s4, "s3 ({s3}) < s4 ({s4})");
        assert!(s4 < s5, "s4 ({s4}) < s5 ({s5})");

        // Asymptotic ceiling at 0.990
        assert!(s5 <= 0.990, "s5 ({s5}) must not exceed ceiling 0.990");
        assert!(s5 > 0.985);

        // C1 continuous derivative at T (unit slope)
        let eps = 1e-6;
        let slope_right = (soft_knee_ceiling(0.900 + eps) - 0.900) / eps;
        assert!((slope_right - 1.0).abs() < 1e-4, "slope at T+ must be ~1.0, got {slope_right}");
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

        let out1 = apply_priors_with_floor(s1, floor, recency, read_boost);
        let out2 = apply_priors_with_floor(s2, floor, recency, read_boost);
        let out3 = apply_priors_with_floor(s3, floor, recency, read_boost);

        assert!(out1 < out2, "out1 ({out1}) must be strictly less than out2 ({out2})");
        assert!(out2 < out3, "out2 ({out2}) must be strictly less than out3 ({out3})");
        assert!(out3 <= 0.990, "out3 ({out3}) must not exceed soft ceiling 0.990");
        assert!(out1 >= floor, "out1 must meet or exceed floor");
        assert!(out2 >= floor, "out2 must meet or exceed floor");
        assert!(out3 >= floor, "out3 must meet or exceed floor");
    }
}

