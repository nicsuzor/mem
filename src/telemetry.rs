use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use chrono::{DateTime, Utc};

#[derive(Debug, Serialize, Deserialize)]
pub struct ToolCallTelemetry {
    pub timestamp: DateTime<Utc>,
    pub tool_name: String,
    pub response_bytes: usize,
    pub latency_ms: u128,
    pub is_error: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ToolStats {
    pub count: usize,
    pub total_bytes: usize,
    pub total_latency_ms: u128,
    pub error_count: usize,
}

/// Resolve the telemetry file path.
///
/// Telemetry is per-host runtime state, not knowledge — it must not live under
/// `pkb_root`, which may be a synced directory (see issue #260).
///
/// Resolution order:
/// 1. `$MEM_TELEMETRY_PATH` (explicit override)
/// 2. `$XDG_STATE_HOME/mem/telemetry.jsonl` (Linux default `~/.local/state/mem/...`)
/// 3. `~/.local/state/mem/telemetry.jsonl` (macOS / Windows fallback — `dirs::state_dir()`
///    is `None` outside Linux, so we synthesise the same XDG-style path under `$HOME`).
pub fn telemetry_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MEM_TELEMETRY_PATH") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    let base = dirs::state_dir().or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))?;
    Some(base.join("mem").join("telemetry.jsonl"))
}

pub fn record_call(
    tool_name: &str,
    response_bytes: usize,
    latency_ms: u128,
    is_error: bool,
) {
    let Some(telemetry_path) = telemetry_path() else { return };

    let entry = ToolCallTelemetry {
        timestamp: Utc::now(),
        tool_name: tool_name.to_string(),
        response_bytes,
        latency_ms,
        is_error,
    };

    if let Ok(json) = serde_json::to_string(&entry) {
        if let Some(parent) = telemetry_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&telemetry_path)
        {
            let _ = writeln!(file, "{}", json);
        }
    }
}

pub fn get_stats() -> HashMap<String, ToolStats> {
    let mut stats: HashMap<String, ToolStats> = HashMap::new();
    let Some(telemetry_path) = telemetry_path() else { return stats };

    if let Ok(file) = File::open(telemetry_path) {
        let reader = BufReader::new(file);
        for line in reader.lines() {
            if let Ok(line) = line {
                if let Ok(entry) = serde_json::from_str::<ToolCallTelemetry>(&line) {
                    let s = stats.entry(entry.tool_name).or_default();
                    s.count += 1;
                    s.total_bytes += entry.response_bytes;
                    s.total_latency_ms += entry.latency_ms;
                    if entry.is_error {
                        s.error_count += 1;
                    }
                }
            }
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Tests in this module mutate the process-wide MEM_TELEMETRY_PATH env var.
    // Serialise them via a mutex since cargo test runs tests in parallel by default
    // and serial_test is not a crate dependency.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn telemetry_path_respects_env_override() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let unique = std::env::temp_dir().join("mem-telemetry-test-override.jsonl");
        std::env::set_var("MEM_TELEMETRY_PATH", &unique);
        let resolved = telemetry_path().expect("env override resolves");
        assert_eq!(resolved, unique);
        std::env::remove_var("MEM_TELEMETRY_PATH");
    }

    #[test]
    fn telemetry_path_default_is_under_state_dir() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("MEM_TELEMETRY_PATH");
        let resolved = telemetry_path().expect("default path resolves on this host");
        assert!(resolved.ends_with("mem/telemetry.jsonl"));
    }

    #[test]
    fn record_and_read_roundtrip_via_env_override() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("mem-telemetry-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("telemetry.jsonl");
        std::env::set_var("MEM_TELEMETRY_PATH", &path);

        record_call("search", 123, 7, false);
        record_call("search", 456, 9, true);
        record_call("get_document", 10, 1, false);

        let stats = get_stats();
        let s = stats.get("search").expect("search recorded");
        assert_eq!(s.count, 2);
        assert_eq!(s.total_bytes, 579);
        assert_eq!(s.error_count, 1);
        let g = stats.get("get_document").expect("get_document recorded");
        assert_eq!(g.count, 1);

        std::env::remove_var("MEM_TELEMETRY_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
