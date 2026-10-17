//! Document salience scoring with exponential access growth and linear time decay.
//!
//! Documents accumulate salience exponentially with each access event (read, search hit,
//! link, MCP retrieval) up to a configurable ceiling. Without access, salience decays
//! linearly per day toward a floor (0.0).

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
        *lock = *lock + chrono::Duration::seconds(seconds);
    }

    pub fn advance_hours(&self, hours: f64) {
        let seconds = (hours * 3600.0).round() as i64;
        let mut lock = self.current_time.write();
        *lock = *lock + chrono::Duration::seconds(seconds);
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

/// Persistent store tracking salience scores across documents.
pub struct SalienceStore {
    entries: HashMap<String, SalienceEntry>,
    config: SalienceConfig,
    clock: Arc<dyn Clock>,
    persist_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct SerializedStore {
    config: SalienceConfig,
    entries: HashMap<String, SalienceEntry>,
}

impl SalienceStore {
    pub fn new(config: SalienceConfig) -> Self {
        Self {
            entries: HashMap::new(),
            config,
            clock: Arc::new(SystemClock),
            persist_path: None,
        }
    }

    pub fn new_with_clock(config: SalienceConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            entries: HashMap::new(),
            config,
            clock,
            persist_path: None,
        }
    }

    pub fn set_clock(&mut self, clock: Arc<dyn Clock>) {
        self.clock = clock;
    }

    pub fn set_persist_path(&mut self, path: PathBuf) {
        self.persist_path = Some(path);
    }

    pub fn config(&self) -> &SalienceConfig {
        &self.config
    }

    /// Load store from disk JSON file, or initialize a new store if absent.
    pub fn load_or_create(path: &Path, config: SalienceConfig, clock: Arc<dyn Clock>) -> Result<Self> {
        if path.exists() {
            let data = std::fs::read_to_string(path)?;
            if let Ok(deserialized) = serde_json::from_str::<SerializedStore>(&data) {
                return Ok(Self {
                    entries: deserialized.entries,
                    config,
                    clock,
                    persist_path: Some(path.to_path_buf()),
                });
            }
        }
        Ok(Self {
            entries: HashMap::new(),
            config,
            clock,
            persist_path: Some(path.to_path_buf()),
        })
    }

    /// Atomically persist current state to disk if a persistence path is set.
    pub fn save(&self) -> Result<()> {
        if let Some(ref path) = self.persist_path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let serialized = SerializedStore {
                config: self.config.clone(),
                entries: self.entries.clone(),
            };
            let json = serde_json::to_string_pretty(&serialized)?;
            let tmp_path = path.with_extension("tmp");
            std::fs::write(&tmp_path, json.as_bytes())?;
            std::fs::rename(&tmp_path, path)?;
        }
        Ok(())
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
