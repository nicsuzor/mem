use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ClientSessionMetadata {
    pub agent_session_id: Option<String>,
    pub client_name: Option<String>,
    pub last_seen: Instant,
}

#[derive(Debug, Default)]
pub struct SessionRegistry {
    sessions: RwLock<HashMap<String, ClientSessionMetadata>>,
}

impl SessionRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Check if a client name matches any listed in PKB_GATEWAY_CLIENTS.
    pub fn is_gateway_client(&self, client_name: Option<&str>) -> bool {
        let name = match client_name {
            Some(n) if !n.trim().is_empty() => n.trim(),
            _ => return false,
        };
        if let Ok(gateways) = std::env::var("PKB_GATEWAY_CLIENTS") {
            gateways
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .any(|g| g.eq_ignore_ascii_case(name))
        } else {
            false
        }
    }

    pub fn register_handshake(
        &self,
        mcp_session_id: &str,
        client_name: Option<String>,
    ) {
        let mut sessions = self.sessions.write();
        let is_gw = self.is_gateway_client(client_name.as_deref());
        if let Some(session) = sessions.get_mut(mcp_session_id) {
            session.client_name = client_name;
            session.last_seen = Instant::now();
            if is_gw {
                session.agent_session_id = None;
            }
        } else {
            sessions.insert(
                mcp_session_id.to_string(),
                ClientSessionMetadata {
                    agent_session_id: None,
                    client_name,
                    last_seen: Instant::now(),
                },
            );
        }
    }

    pub fn count(&self) -> usize {
        self.sessions.read().len()
    }

    pub fn get(&self, mcp_session_id: &str) -> Option<ClientSessionMetadata> {
        self.sessions.read().get(mcp_session_id).cloned()
    }

    /// Retrieve the bound agent session ID for a transport connection.
    /// Returns None if the connection is unknown, has no binding, or is a gateway client.
    pub fn get_bound_session(&self, mcp_session_id: &str) -> Option<String> {
        let sessions = self.sessions.read();
        let meta = sessions.get(mcp_session_id)?;
        if self.is_gateway_client(meta.client_name.as_deref()) {
            return None;
        }
        meta.agent_session_id.clone()
    }

    /// Bind an explicit caller session ID to a direct transport connection.
    /// Never binds if the connection is identified as a gateway client.
    /// Returns Some(previous_id) if the connection was rebound from a different session.
    pub fn maybe_bind(
        &self,
        mcp_session_id: &str,
        explicit_session_id: &str,
    ) -> Option<String> {
        let mut sessions = self.sessions.write();
        let meta = sessions
            .entry(mcp_session_id.to_string())
            .or_insert_with(|| ClientSessionMetadata {
                agent_session_id: None,
                client_name: None,
                last_seen: Instant::now(),
            });
        meta.last_seen = Instant::now();

        if self.is_gateway_client(meta.client_name.as_deref()) {
            return None;
        }

        let rebound_from = match &meta.agent_session_id {
            Some(old_id) if old_id != explicit_session_id => Some(old_id.clone()),
            _ => None,
        };

        meta.agent_session_id = Some(explicit_session_id.to_string());
        rebound_from
    }

    pub fn update_activity(
        &self,
        mcp_session_id: &str,
    ) {
        let mut sessions = self.sessions.write();
        if let Some(session) = sessions.get_mut(mcp_session_id) {
            session.last_seen = Instant::now();
        } else {
            sessions.insert(
                mcp_session_id.to_string(),
                ClientSessionMetadata {
                    agent_session_id: None,
                    client_name: None,
                    last_seen: Instant::now(),
                },
            );
        }
    }

    pub fn prune_stale(&self, max_age: Duration) {
        let now = Instant::now();
        let mut sessions = self.sessions.write();
        sessions.retain(|_, meta| now.duration_since(meta.last_seen) < max_age);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direct_connection_binding_and_rebinding() {
        let registry = SessionRegistry::new();
        let conn = "mcp-conn-1";

        assert_eq!(registry.get_bound_session(conn), None);

        // First explicit bind
        let rebound = registry.maybe_bind(conn, "session-1");
        assert_eq!(rebound, None);
        assert_eq!(registry.get_bound_session(conn), Some("session-1".to_string()));

        // Repeated same bind does not rebind
        let rebound = registry.maybe_bind(conn, "session-1");
        assert_eq!(rebound, None);
        assert_eq!(registry.get_bound_session(conn), Some("session-1".to_string()));

        // Rebind to different session
        let rebound = registry.maybe_bind(conn, "session-2");
        assert_eq!(rebound, Some("session-1".to_string()));
        assert_eq!(registry.get_bound_session(conn), Some("session-2".to_string()));
    }

    #[test]
    fn test_gateway_connection_never_binds() {
        std::env::set_var("PKB_GATEWAY_CLIENTS", "services,gateway-client");
        let registry = SessionRegistry::new();
        let conn = "mcp-gw-conn";

        registry.register_handshake(conn, Some("services".to_string()));

        // Explicit bind attempted on gateway connection
        let rebound = registry.maybe_bind(conn, "session-gw-1");
        assert_eq!(rebound, None);
        assert_eq!(registry.get_bound_session(conn), None);

        std::env::remove_var("PKB_GATEWAY_CLIENTS");
    }
}
