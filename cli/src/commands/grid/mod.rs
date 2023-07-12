mod create;
mod subcommands;

use clap::{Args, Subcommand};
use off_the_grid::node::client::NodeClient;

use crate::scan_config::ScanConfig;

use self::{
    create::{handle_grid_create, CreateOptions},
    subcommands::{handle_grid_details, handle_grid_list, handle_grid_redeem, RedeemOptions},
};

#[derive(Subcommand)]
pub enum Commands {
    Create(CreateOptions),
    Redeem(RedeemOptions),
    List {
        #[clap(short = 't', long, help = "TokenID to filter by")]
        token_id: Option<String>,
    },
    Details {
        #[clap(short = 'i', long, help = "Grid group identity")]
        grid_identity: String,
    },
}

#[derive(Args)]
pub struct GridCommand {
    #[clap(long, help = "Scan configuration file path [default: scan_config]")]
    scan_config: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

pub async fn handle_grid_command(
    node_client: NodeClient,
    orders_command: GridCommand,
) -> anyhow::Result<()> {
    let scan_config = ScanConfig::try_create(orders_command.scan_config, None)?;

    match orders_command.command {
        Commands::Create(options) => handle_grid_create(node_client, scan_config, options).await,
        Commands::Redeem(options) => handle_grid_redeem(node_client, scan_config, options).await,
        Commands::List { token_id } => handle_grid_list(node_client, scan_config, token_id).await,
        Commands::Details { grid_identity } => {
            handle_grid_details(node_client, scan_config, grid_identity).await
        }
    }
}
