mod grid;
mod node;
mod spectrum;
use crate::{
    grid::grid_order::{GridOrder, OrderState, MAX_FEE},
    node::client::NodeClient,
    spectrum::pool::SpectrumPool,
};
use anyhow::{anyhow, Context};
use clap::{arg, ArgAction, Parser};
use config::Config;
use ergo_lib::{
    chain::transaction::{
        prover_result::ProverResult, unsigned::UnsignedTransaction, Input, Transaction,
        UnsignedInput,
    },
    ergo_chain_types::Digest32,
    ergotree_interpreter::sigma_protocol::prover::{ContextExtension, ProofBytes},
    ergotree_ir::chain::{
        address::Address,
        ergo_box::{box_value::BoxValue, ErgoBoxCandidate, NonMandatoryRegisters},
        token::{Token, TokenAmount, TokenId},
    },
    wallet::{
        box_selector::{BoxSelector, SimpleBoxSelector},
        miner_fee,
    },
};
use serde::Deserialize;
use std::iter::once;
use tokio::try_join;

#[derive(Debug, Deserialize)]
struct NodeConfig {
    #[serde(default = "api_url_default")]
    api_url: String,
    api_key: String,
}

impl NodeConfig {
    fn try_create(
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

#[derive(Debug, Deserialize)]
struct ScanConfig {
    pool_scan_id: i32,
}

impl ScanConfig {
    fn try_create(
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

fn api_url_default() -> String {
    "http://127.0.0.1:9053".into()
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct GridArgs {
    #[arg(long, short, help = "Token which will be traded in grid orders")]
    token_id: String,

    #[arg(long, short = 'n', help = "Order size for each individual order")]
    token_amount: u64,

    #[arg(long, help = "Node configuration file path [default: node_config]")]
    node_config: Option<String>,

    #[arg(long, help = "Scan configuration file path [default: scan_config]")]
    scan_config: Option<String>,

    #[arg(long, help = "Ergo node API URL [default: http://127.0.0.1:9053]")]
    api_url: Option<String>,

    #[arg(long, help = "Ergo node API key")]
    api_key: Option<String>,

    #[arg(long, help = "ID of a scan that tracks Spectrum N2T pool boxes")]
    pool_scan_id: Option<i32>,
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

    let scan_config_path: Option<String> = config_matches
        .as_ref()
        .and_then(|matches| matches.get_one("scan_config").cloned());

    let scan_config = ScanConfig::try_create(scan_config_path, args.pool_scan_id)
        .context("Failed to parse scan configuration")?;

    let node = NodeClient::new(
        node_config.api_url.as_str().try_into()?,
        node_config.api_key.as_bytes(),
    )?;

    let (wallet_status, n2t_pool_boxes) = try_join!(
        node.wallet_status(),
        node.get_scan_unspent(scan_config.pool_scan_id)
    )?;

    wallet_status.error_if_locked()?;

    let erg_token_id: TokenId = Digest32::zero().into();
    let token_id: TokenId = Digest32::try_from(args.token_id)
        .context("Failed to parse token id")?
        .into();

    let spectrum_pool = n2t_pool_boxes
        .iter()
        .filter_map(|b| SpectrumPool::try_from(b).ok())
        .filter(|pool| pool.asset_x.token_id == erg_token_id && pool.asset_y.token_id == token_id)
        .max_by_key(|pool| pool.amm_factor())
        .ok_or_else(|| anyhow!("Can't find a pool for the given token"))?;

    let current_pool_price = spectrum_pool.pure_price();

    eprintln!("Current price: {}", current_pool_price);

    let change_address = wallet_status.change_address()?;

    let owner_address = if let Address::P2Pk(miner_pk) = change_address.clone() {
        Ok(miner_pk)
    } else {
        Err(anyhow!("change address is not P2PK"))
    }?;

    let expected_output: Token = (
        spectrum_pool.asset_y.token_id.clone(),
        TokenAmount::try_from(args.token_amount)?,
    )
        .into();

    let buy_price1 = (current_pool_price * 100) / 98;
    let sell_price1 = (current_pool_price * 100) / 96;
    let buy_price2 = (current_pool_price * 100) / 96;
    let sell_price2 = (current_pool_price * 100) / 94;

    let grid1 = GridOrder::new(
        owner_address.clone(),
        buy_price1 as i64,
        sell_price1 as i64,
        expected_output.clone(),
        OrderState::Buy,
    )?;
    let grid2 = GridOrder::new(
        owner_address,
        buy_price2 as i64,
        sell_price2 as i64,
        expected_output.clone(),
        OrderState::Buy,
    )?;

    let initial_value = grid1.value.checked_add(&grid2.value)?;

    let wallet_boxes = node.wallet_boxes_unspent().await?;

    let selection = SimpleBoxSelector::new().select(wallet_boxes, initial_value, &[])?;

    let change_value: i64 = selection
        .change_boxes
        .iter()
        .map(|cb| cb.value.as_i64())
        .sum();

    let fee_value = MAX_FEE.try_into().unwrap();

    let change_boxes = selection.change_boxes.iter().map(|cb| {
        change_value
            .try_into()
            .and_then(|bv: BoxValue| bv.checked_sub(&fee_value))
            .map_err(anyhow::Error::new)
            .and_then(|bv| {
                change_address
                    .script()
                    .map(|et| (bv, et))
                    .map_err(anyhow::Error::new)
            })
            .map(|(value, ergo_tree)| ErgoBoxCandidate {
                value,
                ergo_tree,
                tokens: cb.tokens.clone(),
                additional_registers: NonMandatoryRegisters::empty(),
                creation_height: 800000,
            })
    });

    let inputs: Vec<UnsignedInput> = selection.boxes.iter().map(|b| b.clone().into()).collect();

    let fee_box = ErgoBoxCandidate {
        value: fee_value,
        ergo_tree: miner_fee::MINERS_FEE_ADDRESS.script()?,
        tokens: None,
        additional_registers: NonMandatoryRegisters::empty(),
        creation_height: 800000,
    };

    let outputs = change_boxes
        .chain(once(
            grid1.to_box_candidate(800000).map_err(anyhow::Error::new),
        ))
        .chain(once(
            grid2.to_box_candidate(800000).map_err(anyhow::Error::new),
        ))
        .chain(once(Ok(fee_box.clone())))
        .collect::<anyhow::Result<_>>()?;

    let unsigned_tx = UnsignedTransaction::new_from_vec(inputs, vec![], outputs)?;

    let signed = node.wallet_transaction_sign(&unsigned_tx).await?;

    let token_sum: TokenAmount =
        (grid1.order_amount() as u64 + grid2.order_amount() as u64).try_into()?;

    let token_output = (spectrum_pool.asset_y.token_id.clone(), token_sum).into();

    let token_input = spectrum_pool.input_amount(&token_output)?;

    let swap_output = spectrum_pool
        .clone()
        .with_swap(&token_input)?
        .to_box_candidate(800000)?;

    let change = grid1.bid_value() + grid2.bid_value()
        - *token_input.amount.as_u64() as i64
        - MAX_FEE as i64;

    let grid_out1 = grid1.into_filled()?;
    let grid_out2 = grid2.into_filled()?;

    let change_box = ErgoBoxCandidate {
        value: change.try_into()?,
        ergo_tree: change_address.script()?,
        additional_registers: NonMandatoryRegisters::empty(),
        tokens: None,
        creation_height: 800000,
    };

    let fee_box = ErgoBoxCandidate {
        value: fee_value,
        ..fee_box
    };

    let swap_inputs: Vec<Input> = [
        &spectrum_pool.pool_box,
        &signed.outputs[1],
        &signed.outputs[2],
    ]
    .iter()
    .map(|eb| {
        let prover_result = ProverResult {
            proof: ProofBytes::Empty,
            extension: ContextExtension::empty(),
        };
        Input::new(eb.box_id(), prover_result)
    })
    .collect();

    let swap_outputs = vec![
        swap_output,
        grid_out1.to_box_candidate(800000)?,
        grid_out2.to_box_candidate(800000)?,
        change_box,
        fee_box,
    ];

    let swap_tx = Transaction::new_from_vec(swap_inputs, vec![], swap_outputs)?;

    println!("{}", serde_json::to_string_pretty(&[&signed, &swap_tx])?);

    Ok(())
}
