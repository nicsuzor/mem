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

    pub fn register_handshake(
        &self,
        mcp_session_id: &str,
        client_name: Option<String>,
    ) {
        let mut sessions = self.sessions.write();
        sessions.insert(
            mcp_session_id.to_string(),
            ClientSessionMetadata {
                agent_session_id: None,
                client_name,
                last_seen: Instant::now(),
            },
        );
    }

    pub fn get(&self, mcp_session_id: &str) -> Option<ClientSessionMetadata> {
        self.sessions.read().get(mcp_session_id).cloned()
    }

    pub fn update_activity(
        &self,
        mcp_session_id: &str,
        agent_session_id: Option<String>,
    ) {
        let mut sessions = self.sessions.write();
        if let Some(session) = sessions.get_mut(mcp_session_id) {
            session.last_seen = Instant::now();
            if let Some(id) = agent_session_id {
                session.agent_session_id = Some(id);
            }
        } else {
            sessions.insert(
                mcp_session_id.to_string(),
                ClientSessionMetadata {
                    agent_session_id,
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
