use opentelemetry_otlp::WithExportConfig;
pub mod config;
pub mod session_registry;
pub mod client_context;

use opentelemetry::{global, KeyValue};
use opentelemetry_sdk::{
    trace::{BatchSpanProcessor, TracerProvider},
    Resource,
};


use config::OtelConfig;
use opentelemetry_sdk::propagation::TraceContextPropagator;

pub fn init_telemetry() -> Option<TracerProvider> {
    let config = OtelConfig::from_env()?;

    global::set_text_map_propagator(TraceContextPropagator::new());

    let exporter = opentelemetry_otlp::new_exporter()
        .http()
        .with_endpoint(config.endpoint)
        .build_span_exporter()
        .ok()?;

    let mut resource_attrs = vec![
        KeyValue::new("service.name", "mem-mcp"),
    ];

    if let Some(project) = config.project_name {
        resource_attrs.push(KeyValue::new("openinference.project.name", project.clone()));
        resource_attrs.push(KeyValue::new("project.name", project));
    }

    let processor = BatchSpanProcessor::builder(exporter, opentelemetry_sdk::runtime::Tokio).build();

    let provider = TracerProvider::builder()
        .with_span_processor(processor)
        .with_config(opentelemetry_sdk::trace::Config::default().with_resource(Resource::new(resource_attrs)))
        .build();

    opentelemetry::global::set_tracer_provider(provider.clone());

    Some(provider)
}
