use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Warn,
    Enforce,
}

impl SessionMode {
    pub fn from_env() -> Self {
        match env::var("PKB_SESSION_MODE").as_deref() {
            Ok(val) if val.eq_ignore_ascii_case("enforce") => SessionMode::Enforce,
            _ => SessionMode::Warn,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OtelConfig {
    pub endpoint: String,
    pub protocol: String,
    pub task_id: Option<String>,
    pub project_name: Option<String>,
    /// Request headers for the OTLP exporter, from `GENAI_ENGINE_API_KEY` in
    /// `k1=v1,k2=v2` form (e.g. Cloudflare Access service-token headers).
    pub headers: std::collections::HashMap<String, String>,
}

/// Parse `k1=v1,k2=v2` into headers. Each pair splits on its first `=`, so a
/// value may itself contain `=`. Pairs without `=` or with an empty key are skipped.
pub fn parse_headers(raw: &str) -> std::collections::HashMap<String, String> {
    raw.split(',')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            let k = k.trim();
            if k.is_empty() {
                return None;
            }
            Some((k.to_string(), v.trim().to_string()))
        })
        .collect()
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

        let headers = env::var("GENAI_ENGINE_API_KEY")
            .map(|raw| parse_headers(&raw))
            .unwrap_or_default();

        Some(Self {
            endpoint,
            protocol,
            task_id,
            project_name,
            headers,
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

    #[test]
    fn test_parse_headers_access_token_form() {
        let h = parse_headers("CF-Access-Client-Id=abc.access, CF-Access-Client-Secret=x=y");
        assert_eq!(h.get("CF-Access-Client-Id").map(String::as_str), Some("abc.access"));
        assert_eq!(h.get("CF-Access-Client-Secret").map(String::as_str), Some("x=y"));
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn test_parse_headers_skips_malformed_pairs() {
        assert!(parse_headers("").is_empty());
        assert!(parse_headers("noequals,=novalue").is_empty());
    }
}
