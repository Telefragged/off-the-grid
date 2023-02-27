mod commands;
mod node_config;
mod scan_config;
use node_config::NodeConfig;
use off_the_grid::node::client::NodeClient;

use anyhow::Context;
use clap::{arg, command, ArgAction, Parser, Subcommand};
use commands::{
    grid::{handle_grid_command, GridCommand},
    scans::{handle_scan_command, ScansCommand},
};

#[derive(Subcommand)]
pub enum Commands {
    #[command(author, version, about, long_about = None)]
    Scans(ScansCommand),
    #[command(author, version, about, long_about = None)]
    Grid(GridCommand),
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct GridArgs {
    #[arg(long, help = "Node configuration file path [default: node_config]")]
    node_config: Option<String>,

    #[arg(long, help = "Ergo node API URL [default: http://127.0.0.1:9053]")]
    api_url: Option<String>,

    #[arg(long, help = "Ergo node API key")]
    api_key: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_matches = clap::Command::new("Config")
        .arg(
            arg!(--node_config <VALUE>)
                .required(false)
                .action(ArgAction::Set),
        )
        .ignore_errors(true)
        .try_get_matches()
        .ok();

    let args = GridArgs::parse();

    let node_config_path: Option<String> = config_matches
        .as_ref()
        .and_then(|matches| matches.get_one("node_config").cloned());

    let node_config = NodeConfig::try_create(node_config_path, args.api_url, args.api_key)
        .context("Failed to parse node configuration")?;

    let node = NodeClient::new(
        node_config.api_url.as_str().try_into()?,
        node_config.api_key.as_bytes(),
    )?;

    match args.command {
        Commands::Scans(scan_command) => handle_scan_command(node, scan_command).await,
        Commands::Grid(grid_command) => handle_grid_command(node, grid_command).await,
    }
}
