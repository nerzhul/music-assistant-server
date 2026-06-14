//! `ma-discovery` — mDNS browse + register for Sendspin.
//!
//! Two directions are supported:
//!
//! * **Server** — registers `_sendspin-server._tcp.local.` so that LAN
//!   clients can discover the MA server.
//! * **Client browser** — browses `_sendspin._tcp.local.` so that the
//!   server can initiate outgoing WebSocket connections to discovered
//!   clients (the recommended "server-initiated" connection flow).
//!
//! TXT record keys per the spec: `path` (recommended `/sendspin`),
//! `name` (optional, friendly name).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

/// mDNS service type for clients that MA can connect TO.
pub const SERVICE_TYPE_CLIENT: &str = "_sendspin._tcp.local.";
/// mDNS service type for the MA server that LAN clients can discover.
pub const SERVICE_TYPE_SERVER: &str = "_sendspin-server._tcp.local.";
/// Default WebSocket path advertised in TXT records.
pub const DEFAULT_PATH: &str = "/sendspin";

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("mdns daemon error: {0}")]
    Daemon(String),
    #[error("invalid service instance name: {0}")]
    InvalidName(String),
}

/// Information about a discovered Sendspin client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredClient {
    pub instance_name: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    pub friendly_name: Option<String>,
    /// Full URL of the client's WebSocket endpoint (`ws://host:port/path`).
    pub url: String,
}

impl DiscoveredClient {
    pub fn from_mdns(instance: &str, host: &str, port: u16, txt: &HashMap<String, String>) -> Self {
        let path = txt
            .get("path")
            .cloned()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| DEFAULT_PATH.to_string());
        let friendly_name = txt.get("name").cloned();
        let url = format!("ws://{host}:{port}{path}");
        Self {
            instance_name: instance.to_string(),
            host: host.to_string(),
            port,
            path,
            friendly_name,
            url,
        }
    }
}

#[async_trait]
pub trait DiscoveryListener: Send + Sync {
    /// Called for every newly discovered client.
    async fn on_client_added(&self, client: DiscoveredClient);
    /// Called when a previously discovered client disappears.
    async fn on_client_removed(&self, instance_name: &str);
}

/// Browse `_sendspin._tcp.local.` continuously and forward events to a
/// `DiscoveryListener`.
pub struct ClientBrowser {
    daemon: ServiceDaemon,
    receiver: Option<mdns_sd::Receiver<ServiceEvent>>,
}

impl ClientBrowser {
    /// Start browsing. The actual event loop runs in a spawned task; events
    /// are routed to `listener`.
    pub fn start(listener: Arc<dyn DiscoveryListener>) -> Result<Self, DiscoveryError> {
        let daemon = ServiceDaemon::new().map_err(|e| DiscoveryError::Daemon(e.to_string()))?;
        let receiver = daemon
            .browse(SERVICE_TYPE_CLIENT)
            .map_err(|e| DiscoveryError::Daemon(e.to_string()))?;
        let recv = receiver.clone();
        tokio::spawn(async move {
            // Active clients cache: instance → (host, port, txt).
            type ActiveMap = HashMap<String, (String, u16, HashMap<String, String>)>;
            let active: Arc<Mutex<ActiveMap>> = Arc::new(Mutex::new(ActiveMap::new()));
            loop {
                match recv.recv_async().await {
                    Ok(event) => match event {
                        ServiceEvent::ServiceResolved(info) => {
                            let instance = info.get_fullname().to_string();
                            let host = info
                                .get_addresses()
                                .iter()
                                .next()
                                .map(|a| a.to_string())
                                .unwrap_or_default();
                            let port = info.get_port();
                            let txt: HashMap<String, String> = info
                                .get_properties()
                                .iter()
                                .map(|p| (p.key().to_string(), p.val_str().to_string()))
                                .collect();
                            active
                                .lock()
                                .insert(instance.clone(), (host.clone(), port, txt.clone()));
                            let dc =
                                DiscoveredClient::from_mdns(info.get_fullname(), &host, port, &txt);
                            debug!(instance = %instance, url = %dc.url, "sendspin client discovered");
                            listener.on_client_added(dc).await;
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            let key = fullname.clone();
                            active.lock().remove(&key);
                            debug!(instance = %key, "sendspin client removed");
                            listener.on_client_removed(&key).await;
                        }
                        ServiceEvent::SearchStopped(_) => {
                            warn!("sendspin mDNS search stopped");
                            break;
                        }
                        _ => {}
                    },
                    Err(e) => {
                        warn!(error = %e, "sendspin mDNS receiver error");
                        break;
                    }
                }
            }
        });
        Ok(Self {
            daemon,
            receiver: Some(receiver),
        })
    }

    /// Stop browsing. Drops the receiver and shuts the daemon down.
    pub fn shutdown(self) -> Result<(), DiscoveryError> {
        drop(self.receiver);
        let _ = self.daemon.shutdown();
        Ok(())
    }
}

/// Register the MA Sendspin server under `_sendspin-server._tcp.local.`.
pub struct ServerRegistrar {
    daemon: ServiceDaemon,
    fullname: String,
}

impl ServerRegistrar {
    pub fn register(
        instance_name: &str,
        port: u16,
        path: Option<&str>,
        friendly_name: Option<&str>,
    ) -> Result<Self, DiscoveryError> {
        let daemon = ServiceDaemon::new().map_err(|e| DiscoveryError::Daemon(e.to_string()))?;
        let mut properties: Vec<(&str, &str)> = Vec::new();
        let default_path = path.unwrap_or(DEFAULT_PATH).to_string();
        properties.push(("path", default_path.as_str()));
        if let Some(n) = friendly_name {
            properties.push(("name", n));
        }
        let service_hostname = format!("{}{}", instance_name, SERVICE_TYPE_SERVER);
        let service_info = ServiceInfo::new(
            SERVICE_TYPE_SERVER,
            instance_name,
            &service_hostname,
            "",
            port,
            &properties[..],
        )
        .map_err(|e| DiscoveryError::Daemon(e.to_string()))?
        .enable_addr_auto();
        let fullname = service_info.get_fullname().to_string();
        daemon
            .register(service_info)
            .map_err(|e| DiscoveryError::Daemon(e.to_string()))?;
        info!(name = %fullname, port, "sendspin server registered on mDNS");
        Ok(Self { daemon, fullname })
    }

    pub fn fullname(&self) -> &str {
        &self.fullname
    }

    pub fn shutdown(self) -> Result<(), DiscoveryError> {
        let _ = self.daemon.shutdown();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_client_from_txt() {
        let mut txt = HashMap::new();
        txt.insert("path".to_string(), "/sendspin".to_string());
        txt.insert("name".to_string(), "Kitchen".to_string());
        let c =
            DiscoveredClient::from_mdns("KITCHEN._sendspin._tcp.local.", "192.168.1.5", 8928, &txt);
        assert_eq!(c.path, "/sendspin");
        assert_eq!(c.friendly_name.as_deref(), Some("Kitchen"));
        assert_eq!(c.url, "ws://192.168.1.5:8928/sendspin");
    }

    #[test]
    fn default_path_when_missing() {
        let txt = HashMap::new();
        let c = DiscoveredClient::from_mdns("x", "h", 1234, &txt);
        assert_eq!(c.path, DEFAULT_PATH);
    }
}
