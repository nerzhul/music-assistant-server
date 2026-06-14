//! Shared state for the Sendspin server.
//!
//! `SendspinState` is the single source of truth for connected clients,
//! groups, the active server hello, and mDNS/registrar handles. It is
//! designed to be shared via `Arc<SendspinState>` between the WebSocket
//! listener, role handlers, and the `PushStream` writer tasks.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use ma_protocol_sendspin::messages::{ConnectionReason, ServerHello, SyncState};

/// Unique client identifier (opaque string from `client/hello`).
pub type ClientId = String;
/// Unique group identifier. MA uses one global group by default; future
/// multi-room support would generate per-room group ids.
pub type GroupId = String;

/// Group playback state, exposed to the player via `group/update`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupPlaybackState {
    Playing,
    Stopped,
}

impl GroupPlaybackState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::Stopped => "stopped",
        }
    }
}

/// A connected Sendspin client.
#[derive(Debug, Clone)]
pub struct ClientRecord {
    pub client_id: ClientId,
    pub name: String,
    pub supported_roles: Vec<String>,
    pub active_roles: Vec<String>,
    pub remote_addr: Option<SocketAddr>,
    pub connection_reason: Option<ConnectionReason>,
    pub state: SyncState,
    pub volume: u8,
    pub muted: bool,
    pub static_delay_ms: u32,
    pub required_lead_time_ms: u32,
    pub min_buffer_ms: u32,
    pub supported_player_commands: Vec<String>,
    pub group_id: GroupId,
    pub connected_at: Instant,
    /// Sender half of an mpsc channel used to push outbound messages
    /// (server/hello, server/time, server/state, server/command,
    /// stream/*, group/update) to this client's writer task.
    pub outbound_tx: mpsc::Sender<OutboundMessage>,
}

/// Outbound message that a connection's writer task will encode to JSON or
/// send as a binary frame.
#[derive(Debug, Clone)]
pub enum OutboundMessage {
    Text(String),
    Binary(Vec<u8>),
}

/// Events emitted by the server to its listeners. Used for `ClientAdded` /
/// `ClientRemoved` / `ClientUpdated` callbacks, mirroring the Python
/// `aiosendspin` server event model.
#[derive(Debug, Clone)]
pub enum ConnectionEvent {
    ClientAdded { client_id: ClientId },
    ClientRemoved { client_id: ClientId },
    ClientUpdated { client_id: ClientId },
}

/// Shared Sendspin server state. Cheap to clone (`Arc` internally).
pub struct SendspinState {
    /// `server_id` advertised in `server/hello`.
    pub server_id: String,
    /// Friendly name advertised in `server/hello`.
    pub server_name: String,
    /// Reason to put in `server/hello` for outbound connections.
    pub connection_reason: ConnectionReason,
    /// Currently connected clients, keyed by `client_id`.
    pub clients: RwLock<HashMap<ClientId, Arc<ClientRecord>>>,
    /// Broadcast channel for connection events. Multiple subscribers are
    /// supported via [`SendspinState::subscribe`].
    events_tx: broadcast::Sender<ConnectionEvent>,
}

impl SendspinState {
    /// Create a new state with the given identity.
    pub fn new(server_id: impl Into<String>, server_name: impl Into<String>) -> Arc<Self> {
        let (events_tx, _) = broadcast::channel(64);
        Arc::new(Self {
            server_id: server_id.into(),
            server_name: server_name.into(),
            connection_reason: ConnectionReason::Discovery,
            clients: RwLock::new(HashMap::new()),
            events_tx,
        })
    }

    /// Register a new listener and return a receiver for connection events.
    pub fn subscribe(&self) -> broadcast::Receiver<ConnectionEvent> {
        self.events_tx.subscribe()
    }

    /// Build a `server/hello` payload for the given client. Activates roles
    /// per [`ma_protocol_sendspin::roles::activate_for_family`].
    pub fn hello_for(&self, supported_roles: &[String]) -> ServerHello {
        let mut active_roles: Vec<String> = Vec::new();
        for role in supported_roles {
            if ma_protocol_sendspin::roles::activate_for_family(
                std::slice::from_ref(role),
                &mut active_roles,
            )
            .is_some()
            {
                // continue collecting; activate_for_family appends to active_roles
                // for non-duplicate families.
            } else {
                // unknown / unsupported role — skip silently per spec
            }
        }
        ServerHello {
            server_id: self.server_id.clone(),
            name: self.server_name.clone(),
            version: 1,
            active_roles,
            connection_reason: Some(self.connection_reason),
        }
    }

    /// Insert or update a client record.
    pub fn upsert_client(&self, record: ClientRecord) -> Arc<ClientRecord> {
        let id = record.client_id.clone();
        let arc = Arc::new(record);
        self.clients.write().insert(id.clone(), Arc::clone(&arc));
        let _ = self
            .events_tx
            .send(ConnectionEvent::ClientAdded { client_id: id });
        arc
    }

    /// Update an existing client record (used when `client/state` arrives
    /// after the handshake).
    pub fn update_client<F>(&self, client_id: &str, mutate: F) -> Option<Arc<ClientRecord>>
    where
        F: FnOnce(&mut ClientRecord),
    {
        let mut guard = self.clients.write();
        if let Some(existing) = guard.get_mut(client_id) {
            let mut record: ClientRecord = (**existing).clone();
            mutate(&mut record);
            let arc = Arc::new(record);
            guard.insert(client_id.to_string(), Arc::clone(&arc));
            let _ = self.events_tx.send(ConnectionEvent::ClientUpdated {
                client_id: client_id.to_string(),
            });
            Some(arc)
        } else {
            None
        }
    }

    /// Remove a client record.
    pub fn remove_client(&self, client_id: &str) -> Option<Arc<ClientRecord>> {
        let removed = self.clients.write().remove(client_id);
        let _ = self.events_tx.send(ConnectionEvent::ClientRemoved {
            client_id: client_id.to_string(),
        });
        removed
    }

    /// Get a snapshot of the currently connected client records.
    pub fn clients_snapshot(&self) -> Vec<Arc<ClientRecord>> {
        self.clients.read().values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ma_protocol_sendspin::roles::{app, spec};

    #[tokio::test]
    async fn hello_activates_known_roles() {
        let state = SendspinState::new("srv1", "Test");
        let hello = state.hello_for(&[
            spec::PLAYER_V1.into(),
            spec::CONTROLLER_V1.into(),
            app::BRIDGE_PLAYER.into(),
        ]);
        assert_eq!(hello.server_id, "srv1");
        assert!(hello.active_roles.contains(&spec::PLAYER_V1.to_string()));
        assert!(hello
            .active_roles
            .contains(&spec::CONTROLLER_V1.to_string()));
    }

    #[tokio::test]
    async fn upsert_and_remove_client() {
        let state = SendspinState::new("srv1", "Test");
        let mut events = state.subscribe();
        let (tx, _rx) = mpsc::channel(1);
        let record = ClientRecord {
            client_id: "c1".into(),
            name: "Speaker".into(),
            supported_roles: vec![spec::PLAYER_V1.into()],
            active_roles: vec![spec::PLAYER_V1.into()],
            remote_addr: None,
            connection_reason: None,
            state: SyncState::Synchronized,
            volume: 50,
            muted: false,
            static_delay_ms: 0,
            required_lead_time_ms: 200,
            min_buffer_ms: 200,
            supported_player_commands: vec![],
            group_id: "g1".into(),
            connected_at: Instant::now(),
            outbound_tx: tx,
        };
        state.upsert_client(record);
        assert_eq!(state.clients_snapshot().len(), 1);
        // give the broadcast a tick to deliver
        tokio::task::yield_now().await;
        let ev = events.try_recv();
        assert!(matches!(ev, Ok(ConnectionEvent::ClientAdded { .. })));
        let removed = state.remove_client("c1");
        assert!(removed.is_some());
        assert_eq!(state.clients_snapshot().len(), 0);
    }
}
