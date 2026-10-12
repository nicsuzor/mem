use std::cell::RefCell;
use std::time::Duration;

#[derive(Debug, Default, Clone, Copy)]
pub struct CallMetrics {
    pub graph_read_wait_us: u64,
    pub graph_read_count: u32,
    pub rebuilt: bool,
    pub embed_time_us: u64,
    pub hybrid_search_time_us: u64,
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

    pub fn finish_recording() -> Option<CallMetrics> {
        CURRENT_CALL_METRICS.with(|m| m.borrow_mut().take())
    }
}
