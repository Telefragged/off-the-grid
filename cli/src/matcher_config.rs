use config::Config;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct MatcherConfig {
    pub reward_address: Option<String>,
    pub matcher_interval: Option<f64>,
}

impl MatcherConfig {
    pub fn try_create(config_path: Option<String>) -> Result<Self, config::ConfigError> {
        let config_required = config_path.is_some();

        let scan_config_reader = Config::builder()
            .add_source(config::Environment::with_prefix("MATCHER"))
            .add_source(
                config::File::with_name(
                    &config_path.unwrap_or_else(|| "matcher_config".to_string()),
                )
                .required(config_required),
            )
            .build()?;

        scan_config_reader.try_deserialize()
    }
}
