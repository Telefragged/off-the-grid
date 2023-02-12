use config::Config;
use serde::Deserialize;

fn api_url_default() -> String {
    "http://127.0.0.1:9053".into()
}

#[derive(Debug, Deserialize)]
pub struct NodeConfig {
    #[serde(default = "api_url_default")]
    pub api_url: String,
    pub api_key: String,
}

impl NodeConfig {
    pub fn try_create(
        config_path: Option<String>,
        api_url: Option<String>,
        api_key: Option<String>,
    ) -> Result<Self, config::ConfigError> {
        let config_required = config_path.is_some();

        let scan_config_reader = Config::builder()
            .add_source(config::Environment::with_prefix("NODE"))
            .add_source(
                config::File::with_name(&config_path.unwrap_or_else(|| "node_config".to_string()))
                    .required(config_required),
            )
            .set_override_option("api_url", api_url)?
            .set_override_option("api_key", api_key)?
            .build()?;

        scan_config_reader.try_deserialize()
    }
}
