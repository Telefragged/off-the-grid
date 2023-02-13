use crate::{
    node::{
        client::NodeClient,
        scan::{CreateScanRequest, TrackingRule},
    },
    scan_config::ScanConfig,
    spectrum::pool,
};
use clap::{arg, Args, Subcommand};
use ergo_lib::ergotree_ir::{mir::constant::Constant, serialization::SigmaSerializable};

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

pub async fn handle_scan_command(
    node_client: NodeClient,
    scan_command: ScansCommand,
) -> anyhow::Result<()> {
    match scan_command.command {
        Commands::CreateConfig { output_path } => {
            // Safe to unwrap because the script is always valid
            let n2t_scan_script = pool::N2T_POOL_ADDRESS
                .script()
                .ok()
                .and_then(|s| s.sigma_serialize_bytes().ok())
                .unwrap();
            let n2t_scan_value = Constant::from(n2t_scan_script);
            let n2t_scan_value_bytes = n2t_scan_value.sigma_serialize_bytes().unwrap();

            let scans = node_client.list_scans().await?;

            let scan = scans.iter().find(|s| {
                if let TrackingRule::Equals { value, register } = &s.tracking_rule {
                    value == &n2t_scan_value_bytes && register == "R1"
                } else {
                    false
                }
            });

            let n2t_scan_id = if let Some(scan) = scan {
                println!("Using existing N2T Scan with id {}", scan.scan_id);
                scan.scan_id
            } else {
                let create_scan = CreateScanRequest {
                    tracking_rule: TrackingRule::Equals {
                        value: n2t_scan_value_bytes,
                        register: "R1".to_string(),
                    },
                    scan_name: "Spectrum N2T".to_string(),
                    wallet_interaction: crate::node::scan::WalletInteraction::Off,
                    remove_offchain: true,
                };
                let scan = node_client.create_scan(create_scan).await?;
                println!("Created N2T Scan with id {}", scan.scan_id);
                scan.scan_id
            };

            let scan_config = ScanConfig {
                pool_scan_id: n2t_scan_id,
            };

            let output_path = output_path.unwrap_or_else(|| "scan_config.json".to_string());
            std::fs::write(output_path.clone(), serde_json::to_string_pretty(&scan_config)?)?;

            println!("Scan config created at {}", output_path);
        }
    }

    Ok(())
}
