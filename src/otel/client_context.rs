use http::request::Parts;
use opentelemetry::{global, Context};
use opentelemetry::propagation::{Extractor, TextMapPropagator};
use opentelemetry::trace::TraceContextExt;

pub struct HeaderExtractor<'a>(pub &'a http::HeaderMap);

impl<'a> Extractor for HeaderExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|k| k.as_str()).collect()
    }
}

pub struct JsonMapExtractor<'a>(pub &'a serde_json::Map<String, serde_json::Value>);

impl<'a> Extractor for JsonMapExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        if let Some(v) = self.0.get(key).and_then(|v| v.as_str()) {
            return Some(v);
        }
        for (k, v) in self.0.iter() {
            if k.eq_ignore_ascii_case(key) {
                if let Some(s) = v.as_str() {
                    return Some(s);
                }
            }
        }
        if let Some(nested) = self.0.get("_meta").and_then(|v| v.as_object()) {
            if let Some(v) = nested.get(key).and_then(|v| v.as_str()) {
                return Some(v);
            }
            for (k, v) in nested.iter() {
                if k.eq_ignore_ascii_case(key) {
                    if let Some(s) = v.as_str() {
                        return Some(s);
                    }
                }
            }
        }
        None
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|k| k.as_str()).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSource {
    Header,
    Meta,
    Argument,
    None,
}

impl SessionSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionSource::Header => "header",
            SessionSource::Meta => "meta",
            SessionSource::Argument => "argument",
            SessionSource::None => "none",
        }
    }
}

pub struct ClientContext {
    pub parent_context: Option<Context>,
    pub session_id: Option<String>,
    pub session_source: SessionSource,
    pub task_id: Option<String>,
    pub client_name: Option<String>,
    pub agent_id: Option<String>,
    pub prompt_id: Option<String>,
}

fn non_empty_trimmed(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

impl ClientContext {
    pub fn extract(
        parts: Option<&Parts>,
        meta: Option<&rmcp::model::RequestMetaObject>,
        args: &serde_json::Value,
    ) -> Self {
        let mut parent_context = None;
        let mut header_session = None;
        let mut header_agent = None;
        let mut header_prompt = None;
        let mut header_client = None;

        if let Some(parts) = parts {
            let extractor = HeaderExtractor(&parts.headers);
            let mut ctx = global::get_text_map_propagator(|prop| prop.extract(&extractor));
            if !ctx.span().span_context().is_valid() {
                ctx = opentelemetry_sdk::propagation::TraceContextPropagator::new().extract(&extractor);
            }
            if ctx.span().span_context().is_valid() {
                parent_context = Some(ctx);
            }

            header_session = parts
                .headers
                .get("x-session-id")
                .or_else(|| parts.headers.get("session-id"))
                .and_then(|v| v.to_str().ok())
                .and_then(non_empty_trimmed);

            header_agent = parts
                .headers
                .get("x-agent-id")
                .and_then(|v| v.to_str().ok())
                .and_then(non_empty_trimmed);

            header_prompt = parts
                .headers
                .get("x-prompt-id")
                .and_then(|v| v.to_str().ok())
                .and_then(non_empty_trimmed);

            header_client = parts
                .headers
                .get("x-client-name")
                .and_then(|v| v.to_str().ok())
                .and_then(non_empty_trimmed);
        }

        let mut meta_session = None;
        let mut meta_agent = None;
        let mut meta_prompt = None;
        let mut meta_client = None;

        if let Some(meta) = meta {
            if parent_context.is_none() {
                let extractor = JsonMapExtractor(&meta.0 .0);
                let mut ctx = global::get_text_map_propagator(|prop| prop.extract(&extractor));
                if !ctx.span().span_context().is_valid() {
                    ctx = opentelemetry_sdk::propagation::TraceContextPropagator::new().extract(&extractor);
                }
                if ctx.span().span_context().is_valid() {
                    parent_context = Some(ctx);
                }
            }

            meta_session = meta
                .0
                .get("session_id")
                .or_else(|| meta.0.get("sessionId"))
                .and_then(|v| v.as_str())
                .and_then(non_empty_trimmed);

            meta_agent = meta
                .0
                .get("agent_id")
                .or_else(|| meta.0.get("agentId"))
                .and_then(|v| v.as_str())
                .and_then(non_empty_trimmed);

            meta_prompt = meta
                .0
                .get("prompt_id")
                .or_else(|| meta.0.get("promptId"))
                .and_then(|v| v.as_str())
                .and_then(non_empty_trimmed);

            meta_client = meta
                .client_info()
                .and_then(|info| non_empty_trimmed(&info.name))
                .or_else(|| {
                    meta.0
                        .get("io.modelcontextprotocol/clientInfo")
                        .and_then(|v| v.get("name"))
                        .and_then(|v| v.as_str())
                        .and_then(non_empty_trimmed)
                });
        }

        let arg_session = args
            .get("session_id")
            .and_then(|v| v.as_str())
            .and_then(non_empty_trimmed);

        let agent_id = meta_agent.or(header_agent);
        let prompt_id = meta_prompt.or(header_prompt);
        let client_name = meta_client.or(header_client);

        // Precedence: 1. Header (x-session-id / session-id), 2. _meta, 3. Top-level args["session_id"]
        let (session_id, session_source) = if let Some(s) = header_session {
            (Some(s), SessionSource::Header)
        } else if let Some(s) = meta_session {
            (Some(s), SessionSource::Meta)
        } else if let Some(s) = arg_session {
            (Some(s), SessionSource::Argument)
        } else {
            (None, SessionSource::None)
        };

        let task_id = std::env::var("GENAI_ENGINE_TASK_ID")
            .or_else(|_| std::env::var("AOPS_TASK_ID"))
            .ok()
            .filter(|s| !s.is_empty());

        Self {
            parent_context,
            session_id,
            session_source,
            task_id,
            client_name,
            agent_id,
            prompt_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Request;
    use serde_json::json;

    #[test]
    fn test_session_id_precedence_and_preservation() {
        // 1. HTTP header takes precedence over meta and argument
        let req = Request::builder()
            .header("x-session-id", "header-session")
            .body(())
            .unwrap();
        let (parts, _) = req.into_parts();
        let meta = json!({
            "session_id": "meta-session",
            "io.modelcontextprotocol/clientInfo": { "name": "overwhelm-dashboard", "version": "1.0.0" }
        });
        let args = json!({ "session_id": "arg-session" });

        let cx = ClientContext::extract(
            Some(&parts),
            Some(&rmcp::model::RequestMetaObject(rmcp::model::MetaObject(
                meta.as_object().unwrap().clone(),
            ))),
            &args,
        );
        assert_eq!(cx.session_id.as_deref(), Some("header-session"));
        assert_eq!(cx.session_source, SessionSource::Header);
        assert_eq!(cx.client_name.as_deref(), Some("overwhelm-dashboard"));
        assert_eq!(args.get("session_id").and_then(|v| v.as_str()), Some("arg-session"));

        // 2. Meta takes precedence over argument; session-id header fallback works too
        let req = Request::builder().body(()).unwrap();
        let (parts, _) = req.into_parts();
        let cx = ClientContext::extract(
            Some(&parts),
            Some(&rmcp::model::RequestMetaObject(rmcp::model::MetaObject(
                meta.as_object().unwrap().clone(),
            ))),
            &args,
        );
        assert_eq!(cx.session_id.as_deref(), Some("meta-session"));
        assert_eq!(cx.session_source, SessionSource::Meta);

        // 3. Top-level session_id argument
        let cx = ClientContext::extract(Some(&parts), None, &args);
        assert_eq!(cx.session_id.as_deref(), Some("arg-session"));
        assert_eq!(cx.session_source, SessionSource::Argument);

        // 4. Blank / whitespace-only session_id resolves to None
        let blank_args = json!({ "session_id": "   " });
        let cx = ClientContext::extract(Some(&parts), None, &blank_args);
        assert_eq!(cx.session_id, None);
        assert_eq!(cx.session_source, SessionSource::None);
    }

    #[test]
    fn test_client_name_extraction_and_header_fallback() {
        let meta = json!({
            "io.modelcontextprotocol/clientInfo": { "name": "overwhelm-dashboard", "version": "1.0.0" }
        });
        let req = Request::builder()
            .header("x-client-name", "other-client")
            .body(())
            .unwrap();
        let (parts, _) = req.into_parts();
        let args = json!({ "session_id": "s1" });

        // _meta clientInfo takes precedence over header fallback
        let cx = ClientContext::extract(
            Some(&parts),
            Some(&rmcp::model::RequestMetaObject(rmcp::model::MetaObject(
                meta.as_object().unwrap().clone(),
            ))),
            &args,
        );
        assert_eq!(cx.client_name.as_deref(), Some("overwhelm-dashboard"));

        // Header fallback when _meta clientInfo is absent
        let cx = ClientContext::extract(Some(&parts), None, &args);
        assert_eq!(cx.client_name.as_deref(), Some("other-client"));
    }

    #[test]
    fn test_traceparent_extraction() {
        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );
        let req = Request::builder()
            .header(
                "traceparent",
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
            )
            .body(())
            .unwrap();
        let (parts, _) = req.into_parts();
        let args = json!({});
        let cx = ClientContext::extract(Some(&parts), None, &args);

        assert!(
            cx.parent_context.is_some(),
            "Context should be successfully extracted"
        );
        let extracted = cx.parent_context.unwrap();
        assert!(extracted.span().span_context().is_valid());
        assert_eq!(
            extracted.span().span_context().trace_id().to_string(),
            "0af7651916cd43dd8448eb211c80319c"
        );
    }
}
