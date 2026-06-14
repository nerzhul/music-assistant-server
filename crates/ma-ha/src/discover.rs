//! Discovery loop for Home Assistant `media_player.*` entities.
//!
//! `Discover` polls `/api/states` on a configurable interval (default
//! 30s) and emits a `Vec<HaEntity>` of media_player entities matching
//! the configured include globs. The MA server subscribes to the
//! `changed()` channel and updates its player registry accordingly.

use std::sync::Arc;
use std::time::Duration;

use async_channel::{bounded, Receiver, Sender};
use thiserror::Error;
use tracing::{debug, warn};

use crate::client::{HaClient, HaClientError, HaEntity};
use crate::config::HaConfig;

/// Convenience alias for the receiver end of the discovery channel.
pub type DiscoverRx = Receiver<Vec<DiscoveredEntity>>;

#[derive(Debug, Error)]
pub enum DiscoverError {
    #[error("client error: {0}")]
    Client(#[from] HaClientError),
    #[error("regex error: {0}")]
    Regex(#[from] regex::Error),
    #[error("discover task panicked")]
    Join(#[from] tokio::task::JoinError),
}

pub type Result<T> = std::result::Result<T, DiscoverError>;

/// A discovered media_player entity, tagged with the snapshot
/// timestamp.
#[derive(Debug, Clone)]
pub struct DiscoveredEntity {
    pub entity: HaEntity,
    pub last_seen: std::time::Instant,
}

pub struct Discover {
    client: Arc<HaClient>,
    config: HaConfig,
    /// Pre-compiled include globs (as `regex::Regex`). Empty = no
    /// filter (every `media_player.*` passes).
    include: Vec<regex::Regex>,
    /// Output channel. The discovery task sends a new snapshot every
    /// `poll_interval`. Receivers are notified via `changed()`.
    tx: Sender<Vec<DiscoveredEntity>>,
    rx: Receiver<Vec<DiscoveredEntity>>,
}

impl Discover {
    /// Build a `Discover` from a `HaConfig` and a `HaClient`. The
    /// include globs are compiled up front.
    pub fn new(config: &HaConfig, client: Arc<HaClient>) -> Result<Self> {
        let include: Vec<regex::Regex> = config
            .include_entity_id_globs
            .iter()
            .map(|g| regex::Regex::new(g))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let (tx, rx) = bounded(8);
        Ok(Self {
            client,
            config: config.clone(),
            include,
            tx,
            rx,
        })
    }

    /// Receiver handle for the latest snapshot. The first item sent
    /// is the result of the first poll; the channel never closes.
    pub fn subscribe(&self) -> Receiver<Vec<DiscoveredEntity>> {
        self.rx.clone()
    }

    /// Run the polling loop in a background tokio task. The task
    /// lives for the lifetime of the process; cancel by dropping the
    /// `Discover` (the channel closes when both `tx` and `rx` are
    /// dropped).
    pub fn spawn(self: Arc<Self>, poll_interval: Duration) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(poll_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                match this.poll_once().await {
                    Ok(snapshot) => {
                        if this.tx.send(snapshot).await.is_err() {
                            // All receivers dropped: stop polling.
                            debug!("discover: all receivers dropped, exiting");
                            break;
                        }
                    }
                    Err(e) => warn!(error = %e, "discover: poll failed"),
                }
            }
        })
    }

    /// Run a single poll. Public so the integration tests can call
    /// it without spawning the loop.
    pub async fn poll_once(&self) -> Result<Vec<DiscoveredEntity>> {
        let states = self.client.list_states().await?;
        let now = std::time::Instant::now();
        let mut out = Vec::new();
        for e in states {
            if !e.is_media_player() {
                continue;
            }
            if !self.include.is_empty() && !self.include.iter().any(|re| re.is_match(&e.entity_id))
            {
                continue;
            }
            if self.config.import_players {
                out.push(DiscoveredEntity {
                    entity: e,
                    last_seen: now,
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::HaEntity;

    #[test]
    fn glob_filter_matches() {
        let cfg = HaConfig {
            include_entity_id_globs: vec!["media_player\\.living_.*".into()],
            ..Default::default()
        };
        let client = Arc::new(HaClient::new("http://ha.local:8123", "tok", true).unwrap());
        let d = Discover::new(&cfg, client).unwrap();
        assert!(d
            .include
            .iter()
            .any(|r| r.is_match("media_player.living_room")));
        assert!(!d.include.iter().any(|r| r.is_match("media_player.kitchen")));
    }

    #[test]
    fn empty_globs_matches_all() {
        let cfg = HaConfig::default();
        let client = Arc::new(HaClient::new("http://ha.local:8123", "tok", true).unwrap());
        let d = Discover::new(&cfg, client).unwrap();
        assert!(d.include.is_empty());
    }

    #[test]
    fn entity_id_match_works() {
        let e = HaEntity {
            entity_id: "media_player.bedroom".into(),
            state: "playing".into(),
            ..Default::default()
        };
        assert!(e.is_media_player());
    }
}
