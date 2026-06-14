//! `ma-server` binary entry point.

use std::path::PathBuf;

use anyhow::Result;
use ma_config::MassConfig;
use ma_runtime::Runtime;
use ma_server::run;
use tracing::warn;

fn main() -> Result<()> {
    let cfg_path = std::env::var("MA_CONFIG_FILE").map(PathBuf::from).ok();

    let mut config = match cfg_path {
        Some(p) if p.exists() => MassConfig::load_from_file(p)?,
        _ => MassConfig::default(),
    };
    config.apply_env_overrides();

    ma_runtime::install_tracing(&config.server.log_level)?;

    let data_dir = config.data_dir();
    let cache_dir = config.cache_dir();
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        warn!(error = %e, "could not create data dir");
    }
    if let Err(e) = std::fs::create_dir_all(&cache_dir) {
        warn!(error = %e, "could not create cache dir");
    }

    let rt = Runtime::new(data_dir, cache_dir);
    rt.block_on(run(config))
}
