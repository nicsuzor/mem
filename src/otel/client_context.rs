use crate::otel::session_registry::SessionRegistry;
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
    Binding,
    None,
}

impl SessionSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionSource::Header => "header",
            SessionSource::Meta => "meta",
            SessionSource::Argument => "argument",
            SessionSource::Binding => "binding",
            SessionSource::None => "none",
        }
    }
}

pub struct ClientContext {
    pub parent_context: Option<Context>,
    pub session_id: Option<String>,
    pub session_source: SessionSource,
    pub rebound_from: Option<String>,
    pub mcp_session_id: Option<String>,
    pub task_id: Option<String>,
    pub client_name: Option<String>,
    pub agent_id: Option<String>,
    pub prompt_id: Option<String>,
}

impl ClientContext {
    pub fn extract(
        parts: Option<&Parts>,
        meta: Option<&rmcp::model::Meta>,
        args: &mut serde_json::Value,
        registry: &SessionRegistry,
    ) -> Self {
        let mut parent_context = None;
        let mut mcp_session_id = None;
        let mut header_session = None;
        let mut header_agent = None;
        let mut header_prompt = None;

        if let Some(parts) = parts {
            let extractor = HeaderExtractor(&parts.headers);
            let mut ctx = global::get_text_map_propagator(|prop| prop.extract(&extractor));
            if !ctx.span().span_context().is_valid() {
                ctx = opentelemetry_sdk::propagation::TraceContextPropagator::new().extract(&extractor);
            }
            if ctx.span().span_context().is_valid() {
                parent_context = Some(ctx);
            }

            mcp_session_id = parts
                .headers
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            header_session = parts
                .headers
                .get("x-session-id")
                .or_else(|| parts.headers.get("session-id"))
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            header_agent = parts
                .headers
                .get("x-agent-id")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            header_prompt = parts
                .headers
                .get("x-prompt-id")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }

        let mut meta_session = None;
        let mut meta_agent = None;
        let mut meta_prompt = None;

        if let Some(meta) = meta {
            if parent_context.is_none() {
                let extractor = JsonMapExtractor(&meta.0);
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
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            meta_agent = meta
                .0
                .get("agent_id")
                .or_else(|| meta.0.get("agentId"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            meta_prompt = meta
                .0
                .get("prompt_id")
                .or_else(|| meta.0.get("promptId"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }

        // Check and extract _session argument, removing it from args so tool validation never sees it
        let mut arg_session = None;
        let mut arg_agent = None;
        let mut arg_prompt = None;

        if let Some(obj) = args.as_object_mut() {
            if let Some(session_val) = obj.remove("_session") {
                if let Some(map) = session_val.as_object() {
                    arg_session = map
                        .get("session_id")
                        .or_else(|| map.get("sessionId"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    arg_agent = map
                        .get("agent_id")
                        .or_else(|| map.get("agentId"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    arg_prompt = map
                        .get("prompt_id")
                        .or_else(|| map.get("promptId"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                } else if let Some(s) = session_val.as_str() {
                    arg_session = Some(s.trim().to_string()).filter(|s| !s.is_empty());
                }
            }
        }

        let agent_id = arg_agent.or(meta_agent).or(header_agent);
        let prompt_id = arg_prompt.or(meta_prompt).or(header_prompt);

        // Precedence: 1. Header, 2. Meta, 3. Argument, 4. Connection binding
        let (session_id, session_source, is_explicit) = if let Some(s) = header_session {
            (Some(s), SessionSource::Header, true)
        } else if let Some(s) = meta_session {
            (Some(s), SessionSource::Meta, true)
        } else if let Some(s) = arg_session {
            (Some(s), SessionSource::Argument, true)
        } else if let Some(ref mcp_id) = mcp_session_id {
            if let Some(bound) = registry.get_bound_session(mcp_id) {
                (Some(bound), SessionSource::Binding, false)
            } else {
                (None, SessionSource::None, false)
            }
        } else {
            (None, SessionSource::None, false)
        };

        // Only sources 1-3 update a binding
        let mut rebound_from = None;
        if is_explicit {
            if let (Some(ref mcp_id), Some(ref sid)) = (&mcp_session_id, &session_id) {
                rebound_from = registry.maybe_bind(mcp_id, sid);
            }
        }

        let client_name = mcp_session_id
            .as_deref()
            .and_then(|id| registry.get(id))
            .and_then(|meta| meta.client_name);

        let task_id = std::env::var("GENAI_ENGINE_TASK_ID")
            .or_else(|_| std::env::var("AOPS_TASK_ID"))
            .ok()
            .filter(|s| !s.is_empty());

        Self {
            parent_context,
            session_id,
            session_source,
            rebound_from,
            mcp_session_id,
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
    fn test_session_id_precedence() {
        let registry = SessionRegistry::new();

        // 1. HTTP header takes precedence over meta and argument
        let req = Request::builder()
            .header("x-session-id", "header-session")
            .body(())
            .unwrap();
        let (parts, _) = req.into_parts();
        let meta = json!({ "session_id": "meta-session" });
        let mut args = json!({ "_session": { "session_id": "arg-session" } });

        let cx = ClientContext::extract(
            Some(&parts),
            Some(&rmcp::model::Meta(meta.as_object().unwrap().clone())),
            &mut args,
            &registry,
        );
        assert_eq!(cx.session_id.as_deref(), Some("header-session"));
        assert_eq!(cx.session_source, SessionSource::Header);
        assert!(args.get("_session").is_none(), "_session must be stripped from args");

        // 2. Meta takes precedence over argument
        let req = Request::builder().body(()).unwrap();
        let (parts, _) = req.into_parts();
        let mut args = json!({ "_session": { "session_id": "arg-session" } });
        let cx = ClientContext::extract(
            Some(&parts),
            Some(&rmcp::model::Meta(meta.as_object().unwrap().clone())),
            &mut args,
            &registry,
        );
        assert_eq!(cx.session_id.as_deref(), Some("meta-session"));
        assert_eq!(cx.session_source, SessionSource::Meta);

        // 3. Argument takes precedence over binding
        registry.maybe_bind("conn-1", "bound-session");
        let req = Request::builder()
            .header("mcp-session-id", "conn-1")
            .body(())
            .unwrap();
        let (parts, _) = req.into_parts();
        let mut args = json!({ "_session": { "session_id": "arg-session" } });
        let cx = ClientContext::extract(Some(&parts), None, &mut args, &registry);
        assert_eq!(cx.session_id.as_deref(), Some("arg-session"));
        assert_eq!(cx.session_source, SessionSource::Argument);

        // 4. Connection binding when no explicit session
        let req = Request::builder()
            .header("mcp-session-id", "conn-1")
            .body(())
            .unwrap();
        let (parts, _) = req.into_parts();
        let mut args = json!({});
        let cx = ClientContext::extract(Some(&parts), None, &mut args, &registry);
        assert_eq!(cx.session_id.as_deref(), Some("arg-session")); // rebound in step 3
        assert_eq!(cx.session_source, SessionSource::Binding);

        // 5. No explicit session and no binding -> None, source none (NO invented session)
        let req = Request::builder()
            .header("mcp-session-id", "unknown-conn")
            .body(())
            .unwrap();
        let (parts, _) = req.into_parts();
        let mut args = json!({});
        let cx = ClientContext::extract(Some(&parts), None, &mut args, &registry);
        assert_eq!(cx.session_id, None);
        assert_eq!(cx.session_source, SessionSource::None);
    }

    #[test]
    fn test_traceparent_extraction() {
        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );
        let registry = SessionRegistry::new();
        let req = Request::builder()
            .header(
                "traceparent",
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
            )
            .body(())
            .unwrap();
        let (parts, _) = req.into_parts();
        let mut args = json!({});
        let cx = ClientContext::extract(Some(&parts), None, &mut args, &registry);

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
