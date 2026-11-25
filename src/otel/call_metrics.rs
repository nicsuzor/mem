use std::cell::RefCell;
use std::time::Duration;

#[derive(Debug, Default, Clone)]
pub struct SearchTelemetry {
    pub result_ids: Vec<String>,
    pub result_count: usize,
    pub abstained: bool,
    pub reranker_loaded: bool,
    pub degraded: bool,
    pub stage_embed_ms: Option<f64>,
    pub stage_retrieve_ms: Option<f64>,
    pub stage_total_ms: Option<f64>,
    pub caller: Option<String>,
    pub surface: Option<String>,
    pub prompt_kind: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct CallMetrics {
    pub graph_read_wait_us: u64,
    pub graph_read_count: u32,
    pub rebuilt: bool,
    pub embed_time_us: u64,
    pub hybrid_search_time_us: u64,
    pub response_bytes: usize,
    pub estimated_prompt_tokens: u32,
    pub truncated: bool,
    pub fields_projected: bool,
    pub search_telemetry: Option<SearchTelemetry>,
}

thread_local! {
    static CURRENT_CALL_METRICS: RefCell<Option<CallMetrics>> = const { RefCell::new(None) };
}

impl CallMetrics {
    pub fn start_recording() {
        CURRENT_CALL_METRICS.with(|m| {
            *m.borrow_mut() = Some(CallMetrics::default());
        });
    }

    pub fn record_graph_read_wait(duration: Duration) {
        CURRENT_CALL_METRICS.with(|m| {
            if let Some(ref mut metrics) = *m.borrow_mut() {
                metrics.graph_read_wait_us = metrics.graph_read_wait_us.saturating_add(duration.as_micros() as u64);
                metrics.graph_read_count = metrics.graph_read_count.saturating_add(1);
            }
        });
    }

    pub fn record_rebuild(rebuilt: bool) {
        CURRENT_CALL_METRICS.with(|m| {
            if let Some(ref mut metrics) = *m.borrow_mut() {
                metrics.rebuilt = metrics.rebuilt || rebuilt;
            }
        });
    }

    pub fn record_embed_duration(duration: Duration) {
        CURRENT_CALL_METRICS.with(|m| {
            if let Some(ref mut metrics) = *m.borrow_mut() {
                metrics.embed_time_us = metrics.embed_time_us.saturating_add(duration.as_micros() as u64);
            }
        });
    }

    pub fn record_hybrid_search_duration(duration: Duration) {
        CURRENT_CALL_METRICS.with(|m| {
            if let Some(ref mut metrics) = *m.borrow_mut() {
                metrics.hybrid_search_time_us = metrics.hybrid_search_time_us.saturating_add(duration.as_micros() as u64);
            }
        });
    }

    pub fn record_response_size(bytes: usize) {
        CURRENT_CALL_METRICS.with(|m| {
            if let Some(ref mut metrics) = *m.borrow_mut() {
                metrics.response_bytes = bytes;
                metrics.estimated_prompt_tokens = (bytes / 4) as u32;
            }
        });
    }

    pub fn record_truncation(truncated: bool) {
        CURRENT_CALL_METRICS.with(|m| {
            if let Some(ref mut metrics) = *m.borrow_mut() {
                metrics.truncated = metrics.truncated || truncated;
            }
        });
    }

    pub fn record_fields_projected(projected: bool) {
        CURRENT_CALL_METRICS.with(|m| {
            if let Some(ref mut metrics) = *m.borrow_mut() {
                metrics.fields_projected = metrics.fields_projected || projected;
            }
        });
    }

    pub fn record_search_telemetry(telemetry: SearchTelemetry) {
        CURRENT_CALL_METRICS.with(|m| {
            if let Some(ref mut metrics) = *m.borrow_mut() {
                metrics.search_telemetry = Some(telemetry);
            }
        });
    }

    pub fn current_embed_time_us() -> u64 {
        CURRENT_CALL_METRICS.with(|m| m.borrow().as_ref().map(|x| x.embed_time_us).unwrap_or(0))
    }

    pub fn finish_recording() -> Option<CallMetrics> {
        CURRENT_CALL_METRICS.with(|m| m.borrow_mut().take())
    }
}
