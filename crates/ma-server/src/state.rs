//! Shared application state for `ma-server`.

use ma_config::MassConfig;

pub struct AppState {
    pub config: MassConfig,
}

impl AppState {
    pub fn new(config: MassConfig) -> Self {
        Self { config }
    }
}
