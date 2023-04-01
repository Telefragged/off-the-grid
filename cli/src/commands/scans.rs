use clap::{Args, Subcommand};
use ergo_lib::ergotree_ir::{
    chain::address::Address, mir::constant::Constant, serialization::SigmaSerializable,
    sigma_protocol::sigma_boolean::ProveDlog,
};
use off_the_grid::{
    grid::{grid_order::GRID_ORDER_SCRIPT, multigrid_order::MULTIGRID_ORDER_SCRIPT},
    node::{
        client::NodeClient,
        scan::{CreateScanRequest, NodeScan, TrackingRule, WalletInteraction},
    },
    spectrum::pool,
};

use crate::scan_config::ScanConfig;

#[derive(Subcommand)]
pub enum Commands {
    /// Create a scan config file
    CreateConfig {
        #[arg(short, long, help = "Output path [default: scan_config.json]")]
        output_path: Option<String>,
    },
}

#[derive(Args)]
pub struct ScansCommand {
    #[command(subcommand)]
    command: Commands,
}

fn n2t_tracking_rule() -> TrackingRule {
    // We assume the pool script is always valid
    let n2t_scan_script = pool::N2T_POOL_SCRIPT.sigma_serialize_bytes().unwrap();
    let n2t_scan_value = Constant::from(n2t_scan_script);
    let n2t_scan_value_bytes = n2t_scan_value.sigma_serialize_bytes().unwrap();

    TrackingRule::Equals {
        value: n2t_scan_value_bytes,
        register: "R1".to_string(),
    }
}

fn wallet_grid_tracking_rule(owner_dlog: ProveDlog) -> TrackingRule {
    // We assume the grid order script is always valid
    let wallet_grid_script = GRID_ORDER_SCRIPT.sigma_serialize_bytes().unwrap();

    let wallet_grid_value: Constant = wallet_grid_script.into();
    let wallet_grid_value_bytes = wallet_grid_value.sigma_serialize_bytes().unwrap();
    let owner_group_element_value: Constant = (*owner_dlog.h).into();
    let owner_group_element_value_bytes =
        owner_group_element_value.sigma_serialize_bytes().unwrap();

    TrackingRule::And {
        args: vec![
            TrackingRule::Equals {
                value: wallet_grid_value_bytes,
                register: "R1".to_string(),
            },
            TrackingRule::Equals {
                value: owner_group_element_value_bytes,
                register: "R4".to_string(),
            },
        ],
    }
}

fn wallet_multigrid_tracking_rule(owner_dlog: ProveDlog) -> TrackingRule {
    // We assume the grid order script is always valid
    let wallet_grid_script = MULTIGRID_ORDER_SCRIPT.sigma_serialize_bytes().unwrap();

    let wallet_grid_value: Constant = wallet_grid_script.into();
    let wallet_grid_value_bytes = wallet_grid_value.sigma_serialize_bytes().unwrap();
    let owner_group_element_value: Constant = (*owner_dlog.h).into();
    let owner_group_element_value_bytes =
        owner_group_element_value.sigma_serialize_bytes().unwrap();

    TrackingRule::And {
        args: vec![
            TrackingRule::Equals {
                value: wallet_grid_value_bytes,
                register: "R1".to_string(),
            },
            TrackingRule::Equals {
                value: owner_group_element_value_bytes,
                register: "R4".to_string(),
            },
        ],
    }
}

async fn get_or_create_scan(
    node_client: &NodeClient,
    tracking_rule: TrackingRule,
    scan: Option<&NodeScan>,
    scan_name: &str,
) -> anyhow::Result<i32> {
    if let Some(scan) = scan {
        println!(
            "Using existing scan {} with id {}",
            scan.scan_name, scan.scan_id
        );
        Ok(scan.scan_id)
    } else {
        let create_scan = CreateScanRequest {
            tracking_rule,
            scan_name: scan_name.to_string(),
            wallet_interaction: WalletInteraction::Off,
            remove_offchain: true,
        };
        let scan = node_client.create_scan(create_scan).await?;
        println!("Created new scan {} with id {}", scan_name, scan.scan_id);
        Ok(scan.scan_id)
    }
}

pub async fn handle_scan_command(
    node_client: NodeClient,
    scan_command: ScansCommand,
) -> anyhow::Result<()> {
    match scan_command.command {
        Commands::CreateConfig { output_path } => {
            let wallet_status = node_client.wallet_status().await?;
            wallet_status.error_if_locked()?;
            let change_address = wallet_status.change_address()?;

            let owner_dlog = if let Address::P2Pk(owner_dlog) = change_address {
                Ok(owner_dlog)
            } else {
                Err(anyhow::anyhow!("Change address is not a P2PK address"))
            }?;

            let n2t_tracking_rule = n2t_tracking_rule();
            let wallet_grid_tracking_rule = wallet_grid_tracking_rule(owner_dlog.clone());
            let wallet_multigrid_tracking_rule = wallet_multigrid_tracking_rule(owner_dlog);

            let scans = node_client.list_scans().await?;

            let n2t_scan = scans.iter().find(|s| s.tracking_rule == n2t_tracking_rule);
            let wallet_grid_scan = scans
                .iter()
                .find(|s| s.tracking_rule == wallet_grid_tracking_rule);
            let wallet_multigrid_scan = scans
                .iter()
                .find(|s| s.tracking_rule == wallet_multigrid_tracking_rule);

            let n2t_scan_id =
                get_or_create_scan(&node_client, n2t_tracking_rule, n2t_scan, "N2T Pool").await?;

            let wallet_grid_scan_id = get_or_create_scan(
                &node_client,
                wallet_grid_tracking_rule,
                wallet_grid_scan,
                "Wallet Grid",
            )
            .await?;

            let wallet_multigrid_scan_id = get_or_create_scan(
                &node_client,
                wallet_multigrid_tracking_rule,
                wallet_multigrid_scan,
                "Wallet Multigrid",
            )
            .await?;

            let scan_config = ScanConfig {
                n2t_scan_id,
                wallet_grid_scan_id,
                wallet_multigrid_scan_id,
            };

            let output_path = output_path.unwrap_or_else(|| "scan_config.json".to_string());
            std::fs::write(
                output_path.clone(),
                serde_json::to_string_pretty(&scan_config)?,
            )?;

            println!("Scan config created at {}", output_path);
        }
    }

    Ok(())
}
