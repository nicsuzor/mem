use opentelemetry_otlp::WithExportConfig;
pub mod config;
pub mod session_registry;
pub mod client_context;
pub mod call_metrics;

use opentelemetry::{global, KeyValue};
use opentelemetry_sdk::{
    trace::{BatchSpanProcessor, TracerProvider},
    Resource,
};


use std::env;
use config::OtelConfig;
use opentelemetry_sdk::propagation::TraceContextPropagator;

pub fn resource_attributes(config: &OtelConfig) -> Vec<KeyValue> {
    let service_name = env::var("OTEL_SERVICE_NAME")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "mem-mcp".to_string());

    let project_name = config
        .project_name
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(OtelConfig::resolve_project_name);

    vec![
        KeyValue::new("service.name", service_name),
        KeyValue::new("openinference.project.name", project_name.clone()),
        KeyValue::new("project.name", project_name),
    ]
}

pub fn init_telemetry() -> Option<TracerProvider> {
    let config = OtelConfig::from_env()?;

    global::set_text_map_propagator(TraceContextPropagator::new());

    let exporter = opentelemetry_otlp::new_exporter()
        .http()
        .with_endpoint(config.endpoint.clone())
        .with_headers(config.headers.clone())
        .build_span_exporter()
        .ok()?;

    let resource_attrs = resource_attributes(&config);

    let processor = BatchSpanProcessor::builder(exporter, opentelemetry_sdk::runtime::Tokio).build();

    let provider = TracerProvider::builder()
        .with_span_processor(processor)
        .with_config(opentelemetry_sdk::trace::Config::default().with_resource(Resource::new(resource_attrs)))
        .build();

    opentelemetry::global::set_tracer_provider(provider.clone());

    Some(provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    static OTEL_MOD_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_resource_attributes_default_academicops() {
        let _guard = OTEL_MOD_TEST_MUTEX.lock().unwrap();
        env::remove_var("PHOENIX_PROJECT");
        env::remove_var("OPENINFERENCE_PROJECT_NAME");
        env::remove_var("OTEL_SERVICE_NAME");

        let config = OtelConfig {
            endpoint: "http://localhost:4318".to_string(),
            protocol: "http/protobuf".to_string(),
            task_id: None,
            project_name: None,
            headers: HashMap::new(),
        };

        let attrs = resource_attributes(&config);
        let map: HashMap<String, String> = attrs
            .into_iter()
            .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
            .collect();

        assert_eq!(map.get("openinference.project.name").map(String::as_str), Some("academicOps"));
        assert_eq!(map.get("project.name").map(String::as_str), Some("academicOps"));
        assert_eq!(map.get("service.name").map(String::as_str), Some("mem-mcp"));
    }

    #[test]
    fn test_resource_attributes_with_configured_project() {
        let _guard = OTEL_MOD_TEST_MUTEX.lock().unwrap();
        let config = OtelConfig {
            endpoint: "http://localhost:4318".to_string(),
            protocol: "http/protobuf".to_string(),
            task_id: None,
            project_name: Some("my_custom_project".to_string()),
            headers: HashMap::new(),
        };

        let attrs = resource_attributes(&config);
        let map: HashMap<String, String> = attrs
            .into_iter()
            .map(|kv| (kv.key.as_str().to_string(), kv.value.to_string()))
            .collect();

        assert_eq!(map.get("openinference.project.name").map(String::as_str), Some("my_custom_project"));
        assert_eq!(map.get("project.name").map(String::as_str), Some("my_custom_project"));
    }
}
