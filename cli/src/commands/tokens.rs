use std::collections::HashSet;

use clap::{Args, Subcommand};
use futures::future::join_all;
use off_the_grid::{
    boxes::tracked_box::TrackedBox,
    node::client::NodeClient,
    spectrum::pool::SpectrumPool,
    units::{TokenInfo, TokenStore, Unit},
};

use crate::scan_config::ScanConfig;

#[derive(Subcommand)]
pub enum Commands {
    /// Update the unit list from the explorer API
    Update {
        #[clap(long, help = "Scan configuration file path [default: scan_config]")]
        scan_config: Option<String>,
        #[clap(
            long,
            help = "Explorer API URL",
            default_value = "https://api.ergoplatform.com/api/v1"
        )]
        explorer_url: String,
    },
}

#[derive(Args)]
pub struct TokensCommand {
    #[command(subcommand)]
    pub command: Commands,
}

pub async fn handle_tokens_command(
    node_client: NodeClient,
    units_command: TokensCommand,
) -> anyhow::Result<()> {
    match units_command.command {
        Commands::Update {
            scan_config,
            explorer_url,
        } => {
            let scan_config = ScanConfig::try_create(scan_config, None)?;

            let n2t_pools: Vec<TrackedBox<SpectrumPool>> = node_client
                .get_scan_unspent(scan_config.n2t_scan_id)
                .await?
                .into_iter()
                .filter_map(|b| b.try_into().ok())
                .collect();

            let current_tokens = TokenStore::load(None).unwrap_or(TokenStore::with_tokens(vec![]));

            let token_ids: HashSet<_> = n2t_pools
                .iter()
                .map(|b| b.value.asset_y.token_id)
                .filter(|token_id| match current_tokens.get_unit(token_id) {
                    Unit::Known(_) => false,
                    Unit::Unknown(_) => true,
                })
                .collect();

            if token_ids.is_empty() {
                return Ok(());
            }

            println!("Updating {} tokens from explorer API", token_ids.len());

            let explorer_client = reqwest::Client::new();

            let urls = token_ids
                .iter()
                .map(|token_id| {
                    format!(
                        "{}/tokens/{}",
                        explorer_url.trim_end_matches('/'),
                        String::from(*token_id)
                    )
                })
                .collect::<Vec<_>>();

            let responses = join_all(urls.into_iter().map(|url| {
                let client = &explorer_client;
                async move {
                    let resp = client.get(url).send().await;
                    match resp {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                resp.json::<TokenInfo>().await.ok()
                            } else {
                                None
                            }
                        }
                        Err(_) => None,
                    }
                }
            }))
            .await;

            let unitsystem = TokenStore::with_tokens(
                responses
                    .into_iter()
                    .flatten()
                    .chain(current_tokens.tokens().cloned())
                    .collect(),
            );

            unitsystem.save(None)?;
        }
    }
    Ok(())
}
