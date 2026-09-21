use std::env;

#[derive(Debug, Clone)]
pub struct OtelConfig {
    pub endpoint: String,
    pub protocol: String,
    pub task_id: Option<String>,
    pub project_name: Option<String>,
}

impl OtelConfig {
    pub fn from_env() -> Option<Self> {
        let endpoint = if let Ok(ep) = env::var("GENAI_ENGINE_TRACE_ENDPOINT") {
            if ep.is_empty() { return None; }
            ep
        } else if let Ok(ep) = env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") {
            if ep.is_empty() { return None; }
            ep
        } else if let Ok(ep) = env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
            if ep.is_empty() { return None; }
            if ep.ends_with('/') {
                format!("{}v1/traces", ep)
            } else {
                format!("{}/v1/traces", ep)
            }
        } else {
            return None;
        };

        let protocol = env::var("GENAI_ENGINE_TRACE_PROTOCOL")
            .or_else(|_| env::var("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL"))
            .or_else(|_| env::var("OTEL_EXPORTER_OTLP_PROTOCOL"))
            .unwrap_or_else(|_| "http/protobuf".to_string());

        let task_id = env::var("GENAI_ENGINE_TASK_ID")
            .or_else(|_| env::var("AOPS_TASK_ID"))
            .ok()
            .filter(|s| !s.is_empty());

        let project_name = env::var("OTEL_SERVICE_NAME")
            .or_else(|_| env::var("PHOENIX_PROJECT_NAME"))
            .or_else(|_| env::var("GENAI_ENGINE_PROJECT_NAME"))
            .ok()
            .filter(|s| !s.is_empty());

        Some(Self {
            endpoint,
            protocol,
            task_id,
            project_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_endpoint_resolution() {
        // Test GENAI_ENGINE_TRACE_ENDPOINT (verbatim)
        env::set_var("GENAI_ENGINE_TRACE_ENDPOINT", "http://host:4318/my/custom/path");
        let config = OtelConfig::from_env().unwrap();
        assert_eq!(config.endpoint, "http://host:4318/my/custom/path");
        env::remove_var("GENAI_ENGINE_TRACE_ENDPOINT");

        // Test OTEL_EXPORTER_OTLP_TRACES_ENDPOINT (verbatim)
        env::set_var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "http://host:4318/v1/traces");
        let config = OtelConfig::from_env().unwrap();
        assert_eq!(config.endpoint, "http://host:4318/v1/traces");
        env::remove_var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT");

        // Test OTEL_EXPORTER_OTLP_ENDPOINT (appends /v1/traces)
        env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://host:4318");
        let config = OtelConfig::from_env().unwrap();
        assert_eq!(config.endpoint, "http://host:4318/v1/traces");

        // With trailing slash
        env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://host:4318/");
        let config = OtelConfig::from_env().unwrap();
        assert_eq!(config.endpoint, "http://host:4318/v1/traces");
        env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
    }
}
