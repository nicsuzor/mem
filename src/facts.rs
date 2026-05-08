//! Facts table for PKB.
//!
//! Persistent key-value store for node-specific facts with source attribution
//! and observation timestamps. Powered by SQLite.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::path::Path;
use std::str::FromStr;
use strum_macros::{Display, EnumString};

/// Source of a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum FactSource {
    Github,
    Overwhelm,
    PkbLint,
    Materializer,
    Manual,
    Test,
}

/// A single fact record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub node_id: String,
    pub source: FactSource,
    pub key: String,
    pub value: JsonValue,
    pub observed_at: DateTime<Utc>,
}

pub struct PkbFacts {
    conn: Mutex<Connection>,
}

impl PkbFacts {
    /// Open a facts database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open facts DB at {}", path.display()))?;
        
        let slf = Self { conn: Mutex::new(conn) };
        slf.migrate()?;
        Ok(slf)
    }

    /// Open an in-memory facts database for testing.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let slf = Self { conn: Mutex::new(conn) };
        slf.migrate()?;
        Ok(slf)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS pkb_facts (
                node_id TEXT NOT NULL,
                source TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                observed_at TEXT NOT NULL
            )",
            [],
        )?;

        // Indices for the requested read patterns
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_facts_node_key ON pkb_facts (node_id, key, observed_at DESC)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_facts_source_observed ON pkb_facts (source, observed_at DESC)",
            [],
        )?;

        Ok(())
    }

    /// Put a fact into the database.
    pub fn put(
        &self,
        node_id: &str,
        source: FactSource,
        key: &str,
        value: JsonValue,
        observed_at: DateTime<Utc>,
    ) -> Result<()> {
        let value_str = serde_json::to_string(&value)?;
        let source_str = source.to_string();
        let observed_at_str = observed_at.to_rfc3339();

        self.conn.lock().execute(
            "INSERT INTO pkb_facts (node_id, source, key, value, observed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![node_id, source_str, key, value_str, observed_at_str],
        )?;

        Ok(())
    }

    /// Test seam for putting a fact.
    pub fn put_for_test(&self, node_id: &str, key: &str, value: JsonValue) -> Result<()> {
        self.put(node_id, FactSource::Test, key, value, Utc::now())
    }

    /// Get facts for a specific node and key, ordered by observed_at DESC.
    pub fn get_by_node_key(&self, node_id: &str, key: &str, limit: usize) -> Result<Vec<Fact>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT node_id, source, key, value, observed_at 
             FROM pkb_facts 
             WHERE node_id = ?1 AND key = ?2 
             ORDER BY observed_at DESC 
             LIMIT ?3",
        )?;

        let rows = stmt.query_map(params![node_id, key, limit], |row| {
            let source_str: String = row.get(1)?;
            let source = FactSource::from_str(&source_str).map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, "Invalid FactSource")),
                )
            })?;

            let value_str: String = row.get(3)?;
            let value = serde_json::from_str(&value_str).map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, "Invalid JSON value")),
                )
            })?;

            let observed_at_str: String = row.get(4)?;
            let observed_at = DateTime::parse_from_rfc3339(&observed_at_str)
                .map_err(|_| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(std::io::ErrorKind::Other, "Invalid timestamp")),
                    )
                })?
                .with_timezone(&Utc);

            Ok(Fact {
                node_id: row.get(0)?,
                source,
                key: row.get(2)?,
                value,
                observed_at,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get facts for a specific source, ordered by observed_at DESC.
    pub fn get_by_source(&self, source: FactSource, limit: usize) -> Result<Vec<Fact>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT node_id, source, key, value, observed_at 
             FROM pkb_facts 
             WHERE source = ?1 
             ORDER BY observed_at DESC 
             LIMIT ?2",
        )?;

        let source_str = source.to_string();
        let rows = stmt.query_map(params![source_str, limit], |row| {
            let source_str: String = row.get(1)?;
            let source = FactSource::from_str(&source_str).map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, "Invalid FactSource")),
                )
            })?;

            let value_str: String = row.get(3)?;
            let value = serde_json::from_str(&value_str).map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, "Invalid JSON value")),
                )
            })?;

            let observed_at_str: String = row.get(4)?;
            let observed_at = DateTime::parse_from_rfc3339(&observed_at_str)
                .map_err(|_| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(std::io::ErrorKind::Other, "Invalid timestamp")),
                    )
                })?
                .with_timezone(&Utc);

            Ok(Fact {
                node_id: row.get(0)?,
                source,
                key: row.get(2)?,
                value,
                observed_at,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_put_get_facts() -> Result<()> {
        let facts = PkbFacts::open_in_memory()?;
        let now = Utc::now();
        
        facts.put("node-1", FactSource::Github, "pr_state", json!({"status": "open"}), now)?;
        
        let results = facts.get_by_node_key("node-1", "pr_state", 10)?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].node_id, "node-1");
        assert_eq!(results[0].key, "pr_state");
        assert_eq!(results[0].value, json!({"status": "open"}));
        
        Ok(())
    }

    #[test]
    fn test_ordering() -> Result<()> {
        let facts = PkbFacts::open_in_memory()?;
        let now = Utc::now();
        let earlier = now - chrono::Duration::minutes(10);
        
        facts.put("node-1", FactSource::Github, "key", json!("newer"), now)?;
        facts.put("node-1", FactSource::Github, "key", json!("older"), earlier)?;
        
        let results = facts.get_by_node_key("node-1", "key", 10)?;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].value, json!("newer"));
        assert_eq!(results[1].value, json!("older"));
        
        Ok(())
    }

    #[test]
    fn test_get_by_source() -> Result<()> {
        let facts = PkbFacts::open_in_memory()?;
        facts.put("node-1", FactSource::Github, "k1", json!(1), Utc::now())?;
        facts.put("node-2", FactSource::Overwhelm, "k2", json!(2), Utc::now())?;
        
        let github_facts = facts.get_by_source(FactSource::Github, 10)?;
        assert_eq!(github_facts.len(), 1);
        assert_eq!(github_facts[0].node_id, "node-1");
        
        Ok(())
    }

    #[test]
    fn test_use_cases() -> Result<()> {
        let facts = PkbFacts::open_in_memory()?;
        
        // (a) GitHub PR state
        facts.put("task-1", FactSource::Github, "pr_state", json!({"number": 123, "state": "merged"}), Utc::now())?;
        
        // (b) materialisation drift
        facts.put("task-1", FactSource::Materializer, "materialisation_drift", json!({"diff": "significant"}), Utc::now())?;
        
        // (c) link_failure_reason
        facts.put("node-abc", FactSource::PkbLint, "link_failure_reason", json!("missing_target"), Utc::now())?;
        
        // (d) rendered_thread_id
        facts.put("task-1", FactSource::Overwhelm, "rendered_thread_id", json!("thread_xyz"), Utc::now())?;
        
        let task_facts = facts.get_by_node_key("task-1", "pr_state", 1)?;
        assert_eq!(task_facts[0].value["state"], "merged");
        
        Ok(())
    }
}
