use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
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

pub fn record_call(
    pkb_root: &Path,
    tool_name: &str,
    response_bytes: usize,
    latency_ms: u128,
    is_error: bool,
) {
    let telemetry_path = pkb_root.join("telemetry.jsonl");
    let entry = ToolCallTelemetry {
        timestamp: Utc::now(),
        tool_name: tool_name.to_string(),
        response_bytes,
        latency_ms,
        is_error,
    };

    if let Ok(json) = serde_json::to_string(&entry) {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&telemetry_path)
        {
            let _ = writeln!(file, "{}", json);
        }
    }
}

pub fn get_stats(pkb_root: &Path) -> HashMap<String, ToolStats> {
    let telemetry_path = pkb_root.join("telemetry.jsonl");
    let mut stats: HashMap<String, ToolStats> = HashMap::new();

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
