use crate::otel::session_registry::SessionRegistry;
use http::request::Parts;
use opentelemetry::{global, Context};
use opentelemetry::propagation::Extractor;
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

pub struct ClientContext {
    pub parent_context: Option<Context>,
    pub session_id: String,
    pub mcp_session_id: Option<String>,
    pub task_id: Option<String>,
    pub client_name: Option<String>,
}

impl ClientContext {
    pub fn extract(
        parts: Option<&Parts>,
        meta: Option<&rmcp::model::Meta>,
        registry: &SessionRegistry,
    ) -> Self {
        let mut parent_context = None;
        let mut session_id = None;
        let mut mcp_session_id = None;
        let mut client_name = None;

        if let Some(parts) = parts {
            let extractor = HeaderExtractor(&parts.headers);
            let ctx = global::get_text_map_propagator(|prop| prop.extract(&extractor));
            if ctx.span().span_context().is_valid() {
                parent_context = Some(ctx);
            }

            mcp_session_id = parts
                .headers
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            session_id = parts
                .headers
                .get("x-session-id")
                .or_else(|| parts.headers.get("session-id"))
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
        }

        if session_id.is_none() {
            if let Some(meta) = meta {
                if let Some(sid) = meta.0.get("session_id").and_then(|v| v.as_str()) {
                    session_id = Some(sid.to_string());
                }
            }
        }

        if let Some(ref mcp_id) = mcp_session_id {
            if let Some(cached) = registry.get(mcp_id) {
                if session_id.is_none() {
                    session_id = cached.agent_session_id.clone();
                }
                client_name = cached.client_name;
            }
        }

        let session_id = session_id
            .or_else(|| mcp_session_id.clone())
            .or_else(|| std::env::var("AOPS_SESSION_ID").ok())
            .unwrap_or_else(|| "unknown".to_string());

        let task_id = std::env::var("GENAI_ENGINE_TASK_ID")
            .or_else(|_| std::env::var("AOPS_TASK_ID"))
            .ok()
            .filter(|s| !s.is_empty());

        Self {
            parent_context,
            session_id,
            mcp_session_id,
            task_id,
            client_name,
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
        
        // 1. HTTP header takes precedence
        let mut req = Request::builder().header("x-session-id", "header-session").body(()).unwrap();
        let (parts, _) = req.into_parts();
        let meta = json!({ "session_id": "meta-session" });
        
        let cx = ClientContext::extract(Some(&parts), Some(&rmcp::model::Meta(meta.as_object().unwrap().clone())), &registry);
        assert_eq!(cx.session_id, "header-session");

        // 2. Meta fallback
        let req = Request::builder().body(()).unwrap();
        let (parts, _) = req.into_parts();
        let cx = ClientContext::extract(Some(&parts), Some(&rmcp::model::Meta(meta.as_object().unwrap().clone())), &registry);
        assert_eq!(cx.session_id, "meta-session");
        
        // 3. MCP transport fallback
        let mut req = Request::builder().header("mcp-session-id", "transport-session").body(()).unwrap();
        let (parts, _) = req.into_parts();
        let cx = ClientContext::extract(Some(&parts), None, &registry);
        assert_eq!(cx.session_id, "transport-session");
    }

    #[test]
    fn test_traceparent_extraction() {
        opentelemetry::global::set_text_map_propagator(opentelemetry_sdk::propagation::TraceContextPropagator::new());
        let registry = SessionRegistry::new();
        let mut req = Request::builder()
            .header("traceparent", "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01")
            .body(())
            .unwrap();
        let (parts, _) = req.into_parts();
        let cx = ClientContext::extract(Some(&parts), None, &registry);
        
        assert!(cx.parent_context.is_some(), "Context should be successfully extracted");
        let extracted = cx.parent_context.unwrap();
        assert!(extracted.span().span_context().is_valid());
        assert_eq!(
            extracted.span().span_context().trace_id().to_string(),
            "0af7651916cd43dd8448eb211c80319c"
        );
    }
}
