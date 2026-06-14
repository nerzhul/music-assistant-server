//! `ma-runtime` — Tokio runtime, signal handling, and shared application context.
//!
//! Phase 0 stub: install a tracing subscriber and provide a `Runtime` builder.

use anyhow::Result;
use std::future::Future;
use std::path::PathBuf;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::time::ChronoUtc;
use tracing_subscriber::prelude::*;

/// Install a global `tracing` subscriber. Idempotent.
pub fn install_tracing(log_level: &str) -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));
    let layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_timer(ChronoUtc::rfc_3339());
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init();
    Ok(())
}

/// Tiny wrapper around the tokio runtime that knows where the data lives.
pub struct Runtime {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Runtime {
    pub fn new(data_dir: PathBuf, cache_dir: PathBuf) -> Self {
        Self {
            data_dir,
            cache_dir,
        }
    }

    /// Run `future` on a multi-threaded tokio runtime until it completes.
    pub fn block_on<F: Future>(self, future: F) -> F::Output {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        rt.block_on(future)
    }
}
