
use config::Config;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct ScanConfig {
    pub n2t_scan_id: i32,
    pub wallet_grid_scan_id: i32,
}

impl ScanConfig {
    pub fn try_create(
        config_path: Option<String>,
        pool_scan_id: Option<i32>,
    ) -> Result<Self, config::ConfigError> {
        let config_required = config_path.is_some();

        let scan_config_reader = Config::builder()
            .add_source(config::Environment::with_prefix("SCAN"))
            .add_source(
                config::File::with_name(&config_path.unwrap_or_else(|| "scan_config".to_string()))
                    .required(config_required),
            )
            .set_override_option("pool_scan_id", pool_scan_id)?
            .build()?;

        scan_config_reader.try_deserialize()
    }
}
