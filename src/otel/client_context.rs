use http::request::Parts;
use opentelemetry::{
    baggage::BaggageExt,
    global,
    propagation::{Extractor, TextMapCompositePropagator, TextMapPropagator},
    trace::TraceContextExt,
    Context,
};
use opentelemetry_sdk::propagation::{BaggagePropagator, TraceContextPropagator};
use std::collections::BTreeMap;

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
    pub baggage_session_id: Option<String>,
    pub session_source: SessionSource,
    pub task_id: Option<String>,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub forwarded_user_agent: Option<String>,
    pub agent_id: Option<String>,
    pub prompt_id: Option<String>,
    pub tracestate: Option<String>,
    pub antigravity_root_conversation_id: Option<String>,
    pub antigravity_conversation_id: Option<String>,
    pub baggage_entries: Vec<(String, String)>,
}

fn non_empty_trimmed(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn extract_trace_and_baggage(extractor: &dyn Extractor) -> (Option<Context>, BTreeMap<String, String>) {
    let mut ctx = global::get_text_map_propagator(|prop| prop.extract(extractor));
    if !ctx.span().span_context().is_valid() || ctx.baggage().is_empty() {
        let composite = TextMapCompositePropagator::new(vec![
            Box::new(TraceContextPropagator::new()),
            Box::new(BaggagePropagator::new()),
        ]);
        let fallback_ctx = composite.extract(extractor);
        if !ctx.span().span_context().is_valid() && fallback_ctx.span().span_context().is_valid() {
            ctx = fallback_ctx.clone();
        }
        if ctx.baggage().is_empty() && !fallback_ctx.baggage().is_empty() {
            ctx = ctx.with_baggage(fallback_ctx.baggage().iter().map(|(k, (v, _))| {
                opentelemetry::KeyValue::new(k.clone(), v.clone())
            }));
        }
    }

    let mut baggage_map = BTreeMap::new();
    for (k, (v, _)) in ctx.baggage().iter() {
        if let Some(val) = non_empty_trimmed(&v.as_str()) {
            baggage_map.insert(k.as_str().to_string(), val);
        }
    }

    let valid_ctx = if ctx.span().span_context().is_valid() {
        Some(ctx)
    } else {
        None
    };

    (valid_ctx, baggage_map)
}

impl ClientContext {
    pub fn extract(
        parts: Option<&Parts>,
        meta: Option<&rmcp::model::RequestMetaObject>,
        args: &serde_json::Value,
    ) -> Self {
        let mut parent_context = None;
        let mut merged_baggage: BTreeMap<String, String> = BTreeMap::new();

        let mut header_session = None;
        let mut header_baggage_session = None;
        let mut header_agent = None;
        let mut header_prompt = None;
        let mut header_client = None;
        let mut header_tracestate = None;
        let mut forwarded_user_agent = None;

        if let Some(parts) = parts {
            let extractor = HeaderExtractor(&parts.headers);
            let (hdr_ctx, hdr_baggage) = extract_trace_and_baggage(&extractor);
            if hdr_ctx.is_some() {
                parent_context = hdr_ctx;
            }
            header_baggage_session = hdr_baggage.get("session.id").cloned();
            for (k, v) in hdr_baggage {
                merged_baggage.insert(k, v);
            }

            header_tracestate = parts
                .headers
                .get("tracestate")
                .and_then(|v| v.to_str().ok())
                .and_then(non_empty_trimmed);

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

            forwarded_user_agent = parts
                .headers
                .get("x-forwarded-user-agent")
                .and_then(|v| v.to_str().ok())
                .and_then(non_empty_trimmed);
        }

        let mut meta_session = None;
        let mut meta_baggage_session = None;
        let mut meta_agent = None;
        let mut meta_prompt = None;
        let mut meta_client = None;
        let mut meta_client_version = None;
        let mut meta_tracestate = None;
        let mut antigravity_root_conversation_id = None;
        let mut antigravity_conversation_id = None;

        if let Some(meta) = meta {
            let extractor = JsonMapExtractor(&meta.0 .0);
            let (meta_ctx, meta_baggage) = extract_trace_and_baggage(&extractor);
            // _meta takes precedence over HTTP headers
            if meta_ctx.is_some() {
                parent_context = meta_ctx;
            }
            meta_baggage_session = meta_baggage.get("session.id").cloned();
            for (k, v) in meta_baggage {
                merged_baggage.insert(k, v);
            }

            meta_tracestate = extractor
                .get("tracestate")
                .and_then(non_empty_trimmed);

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

            if let Some(info) = meta.client_info() {
                meta_client = non_empty_trimmed(&info.name);
                meta_client_version = non_empty_trimmed(&info.version);
            }
            if meta_client.is_none() {
                meta_client = meta
                    .0
                    .get("io.modelcontextprotocol/clientInfo")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                    .and_then(non_empty_trimmed);
            }
            if meta_client_version.is_none() {
                meta_client_version = meta
                    .0
                    .get("io.modelcontextprotocol/clientInfo")
                    .and_then(|v| v.get("version"))
                    .and_then(|v| v.as_str())
                    .and_then(non_empty_trimmed);
            }

            // Allowlist antigravity.google/root_conversation_id and conversation_id only (never artifacts_dir)
            antigravity_root_conversation_id = meta
                .0
                .get("antigravity.google/root_conversation_id")
                .and_then(|v| v.as_str())
                .and_then(non_empty_trimmed);
            antigravity_conversation_id = meta
                .0
                .get("antigravity.google/conversation_id")
                .and_then(|v| v.as_str())
                .and_then(non_empty_trimmed);
        }

        let arg_session = args
            .get("session_id")
            .and_then(|v| v.as_str())
            .and_then(non_empty_trimmed);

        let baggage_session_id = meta_baggage_session.clone().or(header_baggage_session.clone());
        let baggage_agent = merged_baggage.get("agent.id").cloned();
        let baggage_prompt = merged_baggage.get("prompt.id").cloned();

        let agent_id = meta_agent.or(baggage_agent).or(header_agent);
        let prompt_id = meta_prompt.or(baggage_prompt).or(header_prompt);
        let client_name = meta_client.or(header_client);
        let tracestate = meta_tracestate.or(header_tracestate);

        // Precedence: 1. _meta (baggage["session.id"] or session_id), 2. HTTP header (baggage["session.id"] or x-session-id), 3. args["session_id"], 4. allowlisted antigravity conversation_id fallback
        let (session_id, session_source) = if let Some(s) = meta_baggage_session.or(meta_session) {
            (Some(s), SessionSource::Meta)
        } else if let Some(s) = header_baggage_session.or(header_session) {
            (Some(s), SessionSource::Header)
        } else if let Some(s) = arg_session {
            (Some(s), SessionSource::Argument)
        } else if let Some(s) = antigravity_conversation_id
            .clone()
            .or_else(|| antigravity_root_conversation_id.clone())
        {
            (Some(s), SessionSource::Meta)
        } else {
            (None, SessionSource::None)
        };

        let task_id = merged_baggage
            .get("task.id")
            .cloned()
            .or_else(|| {
                std::env::var("GENAI_ENGINE_TASK_ID")
                    .or_else(|_| std::env::var("AOPS_TASK_ID"))
                    .ok()
                    .filter(|s| !s.is_empty())
            });

        let baggage_entries = merged_baggage.into_iter().collect();

        Self {
            parent_context,
            session_id,
            baggage_session_id,
            session_source,
            task_id,
            client_name,
            client_version: meta_client_version,
            forwarded_user_agent,
            agent_id,
            prompt_id,
            tracestate,
            antigravity_root_conversation_id,
            antigravity_conversation_id,
            baggage_entries,
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
        // 1. _meta takes precedence over HTTP header and argument
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
        assert_eq!(cx.session_id.as_deref(), Some("meta-session"));
        assert_eq!(cx.session_source, SessionSource::Meta);
        assert_eq!(cx.client_name.as_deref(), Some("overwhelm-dashboard"));
        assert_eq!(args.get("session_id").and_then(|v| v.as_str()), Some("arg-session"));

        // 2. HTTP header takes precedence over argument
        let cx = ClientContext::extract(Some(&parts), None, &args);
        assert_eq!(cx.session_id.as_deref(), Some("header-session"));
        assert_eq!(cx.session_source, SessionSource::Header);

        // 3. Top-level session_id argument
        let cx = ClientContext::extract(None, None, &args);
        assert_eq!(cx.session_id.as_deref(), Some("arg-session"));
        assert_eq!(cx.session_source, SessionSource::Argument);

        // 4. Blank / whitespace-only session_id resolves to None
        let blank_args = json!({ "session_id": "   " });
        let cx = ClientContext::extract(None, None, &blank_args);
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

    #[test]
    fn test_meta_takes_precedence_over_headers_and_extracts_baggage_and_antigravity_allowlist() {
        let req = Request::builder()
            .header(
                "traceparent",
                "00-11111111111111111111111111111111-1111111111111111-01",
            )
            .header("tracestate", "vendor=header")
            .header("baggage", "session.id=header-baggage-sess,env=prod")
            .header("x-forwarded-user-agent", "Mozilla/5.0 GeminiWeb/1.0")
            .body(())
            .unwrap();
        let (parts, _) = req.into_parts();

        let meta = json!({
            "traceparent": "00-22222222222222222222222222222222-2222222222222222-01",
            "tracestate": "vendor=meta",
            "baggage": "session.id=meta-baggage-sess,agent.id=james",
            "io.modelcontextprotocol/clientInfo": { "name": "claude-code", "version": "2.1.0" },
            "antigravity.google/root_conversation_id": "root-conv-1",
            "antigravity.google/conversation_id": "conv-2",
            "antigravity.google/artifacts_dir": "/secret/path/should/never/appear"
        });
        let cx = ClientContext::extract(
            Some(&parts),
            Some(&rmcp::model::RequestMetaObject(rmcp::model::MetaObject(
                meta.as_object().unwrap().clone(),
            ))),
            &json!({}),
        );

        let extracted = cx.parent_context.as_ref().expect("parent_context from _meta");
        assert_eq!(
            extracted.span().span_context().trace_id().to_string(),
            "22222222222222222222222222222222",
            "_meta traceparent must take precedence over HTTP header"
        );
        assert_eq!(cx.session_id.as_deref(), Some("meta-baggage-sess"));
        assert_eq!(cx.baggage_session_id.as_deref(), Some("meta-baggage-sess"));
        assert_eq!(cx.agent_id.as_deref(), Some("james"));
        assert_eq!(cx.client_name.as_deref(), Some("claude-code"));
        assert_eq!(cx.client_version.as_deref(), Some("2.1.0"));
        assert_eq!(cx.forwarded_user_agent.as_deref(), Some("Mozilla/5.0 GeminiWeb/1.0"));
        assert_eq!(cx.tracestate.as_deref(), Some("vendor=meta"));
        assert_eq!(cx.antigravity_root_conversation_id.as_deref(), Some("root-conv-1"));
        assert_eq!(cx.antigravity_conversation_id.as_deref(), Some("conv-2"));

        // Fallback to antigravity.google/conversation_id when session.id is absent
        let meta_agy_only = json!({
            "antigravity.google/root_conversation_id": "root-conv-only",
            "antigravity.google/conversation_id": "conv-only",
            "antigravity.google/artifacts_dir": "/secret/artifacts"
        });
        let cx_agy = ClientContext::extract(
            None,
            Some(&rmcp::model::RequestMetaObject(rmcp::model::MetaObject(
                meta_agy_only.as_object().unwrap().clone(),
            ))),
            &json!({}),
        );
        assert_eq!(cx_agy.session_id.as_deref(), Some("conv-only"));
        assert_eq!(cx_agy.baggage_session_id, None);
    }
}
