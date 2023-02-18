use std::iter::once;

use anyhow::anyhow;
use clap::{ArgGroup, Args, Subcommand};
use ergo_lib::{
    chain::transaction::{unsigned::UnsignedTransaction, TransactionError, UnsignedInput},
    ergo_chain_types::Digest32,
    ergotree_ir::chain::{
        address::Address,
        ergo_box::{
            box_value::{BoxValue, BoxValueError},
            ErgoBox, ErgoBoxCandidate, NonMandatoryRegisters,
        },
        token::{Token, TokenAmountError, TokenId},
    },
    wallet::{
        box_selector::{BoxSelector, BoxSelectorError, SimpleBoxSelector},
        miner_fee::MINERS_FEE_ADDRESS,
    },
};
use thiserror::Error;
use tokio::try_join;

use crate::{
    boxes::{
        liquidity_box::{LiquidityProvider, LiquidityProviderError},
        tracked_box::TrackedBox,
    },
    grid::grid_order::{GridOrder, GridOrderError, OrderState},
    node::client::NodeClient,
    scan_config::ScanConfig,
    spectrum::pool::{SpectrumPool, ERG_TOKEN_ID},
};

#[derive(Subcommand)]
pub enum Commands {
    #[command(group(
        ArgGroup::new("amount")
            .required(true)
            .args(&["token_amount", "total_value"])
    ))]
    Create {
        #[clap(short = 't', long, help = "TokenID of the token to be traded")]
        token_id: String,
        #[clap(
            short = 'n',
            long,
            help = "Total amount of tokens to be traded",
            group = "amount"
        )]
        token_amount: Option<u64>,
        #[clap(
            short = 'v',
            long,
            help = "Total value of the grid, in nanoERGs",
            group = "amount"
        )]
        total_value: Option<u64>,
        #[clap(
            short = 'r',
            long,
            help = "Range of the grid, in nanoERGs per token in the form lo..hi",
            value_parser = grid_order_range_from_str
        )]
        range: GridOrderRange,
        #[clap(short = 'o', long, help = "Number of orders in the grid")]
        num_orders: u64,
        #[clap(
            short,
            long,
            help = "transaction fee value, in nanoERGs",
            default_value_t = 1000000
        )]
        fee: u64,
    },
}

#[derive(Args)]
pub struct GridCommand {
    #[clap(long, help = "Scan configuration fiel path [default: scan_config]")]
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
        Commands::Create {
            token_id,
            token_amount,
            total_value,
            range,
            num_orders,
            fee,
        } => {
            let fee_value: BoxValue = fee.try_into()?;
            let token_id: TokenId = Digest32::try_from(token_id)?.into();
            let token_per_grid: Token = match (token_amount, total_value) {
                (Some(token_amount), None) => {
                    let tokens_per_grid = token_amount / num_orders;
                    Ok((token_id.clone(), tokens_per_grid.try_into()?).into())
                }
                (None, Some(total_value)) => {
                    let value_per_grid = total_value / num_orders;
                    Ok((ERG_TOKEN_ID.clone(), value_per_grid.try_into()?).into())
                }
                _ => Err(anyhow!(
                    "Either token_amount or total_value must be specified"
                )),
            }?;

            let (n2t_pool_boxes, wallet_boxes, wallet_status) = try_join!(
                node_client.get_scan_unspent(scan_config.pool_scan_id),
                node_client.wallet_boxes_unspent(),
                node_client.wallet_status()
            )?;

            wallet_status.error_if_locked()?;

            let liquidity_box = n2t_pool_boxes
                .into_iter()
                .filter_map(|b| {
                    b.try_into()
                        .ok()
                        .filter(|b: &TrackedBox<SpectrumPool>| b.value.asset_y.token_id == token_id)
                })
                .max_by_key(|lb| lb.value.amm_factor())
                .ok_or(anyhow!("No liquidity box found for token {:?}", token_id))?;

            let tx = build_new_grid_tx(
                liquidity_box,
                range,
                num_orders,
                token_per_grid,
                wallet_status.change_address()?,
                fee_value,
                wallet_boxes,
            )?;

            let signed = node_client.wallet_transaction_sign(&tx).await?;

            println!("{}", serde_json::to_string_pretty(&signed)?);

            Ok(())
        }
    }
}

#[derive(Error, Debug)]
pub enum BuildNewGridTxError {
    #[error(transparent)]
    LiquidityProvider(#[from] LiquidityProviderError),
    #[error(transparent)]
    TokenAmount(#[from] TokenAmountError),
    #[error(transparent)]
    GridOrder(#[from] GridOrderError),
    #[error(transparent)]
    BoxValue(#[from] BoxValueError),
    #[error(transparent)]
    BoxSelector(#[from] BoxSelectorError),
    #[error(transparent)]
    Transaction(#[from] TransactionError),
}

#[derive(Clone, Debug)]
pub struct GridOrderRange(u64, u64);

#[derive(Error, Debug)]
pub enum GridOrderRangeError {
    #[error("Invalid range: lo must be less than hi")]
    InvalidRange,
}

impl GridOrderRange {
    pub fn new(lo: u64, hi: u64) -> Result<Self, GridOrderRangeError> {
        if lo < hi {
            Ok(GridOrderRange(lo, hi))
        } else {
            Err(GridOrderRangeError::InvalidRange)
        }
    }
}

fn grid_order_range_from_str(s: &str) -> Result<GridOrderRange, String> {
    let parts: Vec<&str> = s.split("..").collect();
    if let [lo, hi] = parts.as_slice() {
        let lo = lo.parse::<u64>().map_err(|e| e.to_string())?;
        let hi = hi.parse::<u64>().map_err(|e| e.to_string())?;
        GridOrderRange::new(lo, hi).map_err(|e| e.to_string())
    } else {
        Err(format!("Invalid range: {}", s))
    }
}

/// Create new orders with the given liquidity box and the given grid range
/// by filling the grid orders while the swaps are favorable.
pub fn new_orders_with_liquidity(
    liquidity_box: impl LiquidityProvider,
    grid_range: GridOrderRange,
    num_orders: u64,
    token_per_grid: Token,
    owner_address: Address,
) -> Result<(Vec<GridOrder>, impl LiquidityProvider), BuildNewGridTxError> {
    let GridOrderRange(lo, hi) = grid_range;

    let grid_value_fn: Box<dyn Fn(u64) -> u64> = if token_per_grid.token_id == *ERG_TOKEN_ID {
        Box::new(|bid: u64| token_per_grid.amount.as_u64() / bid)
    } else {
        Box::new(|_: u64| *token_per_grid.amount.as_u64())
    };

    let token_id = liquidity_box.asset_y().token_id.clone();

    let order_step = (hi - lo) / num_orders;

    let owner_ec_point = if let Address::P2Pk(miner_pk) = owner_address {
        Ok(*miner_pk.h)
    } else {
        Err(anyhow!("change address is not P2PK"))
    }
    .unwrap();

    let mut liquidity_state = liquidity_box;
    // Total amount of tokens that will be swapped
    let mut swap_amount: u64 = 0;

    let initial_orders: Vec<_> = (0..num_orders)
        .rev()
        .map(|n| {
            let ask = lo + order_step * (n + 1);
            let bid = lo + order_step * n;

            let amount = grid_value_fn(bid);
            let token: Token = (token_id.clone(), amount.try_into()?).into();

            let order_state = match liquidity_state.input_amount(&token) {
                Ok(t) if *t.amount.as_u64() <= amount * bid => {
                    liquidity_state = liquidity_state.clone().with_swap(&t)?;
                    swap_amount += t.amount.as_u64();
                    OrderState::Sell
                }
                _ => OrderState::Buy,
            };
            GridOrder::new(owner_ec_point.clone(), bid, ask, token, order_state, None)
                .map_err(BuildNewGridTxError::GridOrder)
        })
        .collect::<Result<_, _>>()?;

    Ok((initial_orders, liquidity_state))
}

pub fn build_new_grid_tx(
    liquidity_box: TrackedBox<impl LiquidityProvider>,
    grid_range: GridOrderRange,
    num_orders: u64,
    token_per_grid: Token,
    owner_address: Address,
    fee_value: BoxValue,
    wallet_boxes: Vec<ErgoBox>,
) -> Result<UnsignedTransaction, BuildNewGridTxError> {
    let creation_height = once(&liquidity_box.ergo_box)
        .chain(wallet_boxes.iter())
        .map(|b| b.creation_height)
        .max()
        .unwrap_or(0);

    let (initial_orders, liquidity_state) = new_orders_with_liquidity(
        liquidity_box.value,
        grid_range,
        num_orders,
        token_per_grid,
        owner_address.clone(),
    )?;

    let missing_ergs = initial_orders.iter().map(|o| o.value.as_u64()).sum::<u64>()
        + liquidity_state.asset_x().amount.as_u64()
        + fee_value.as_u64()
        - liquidity_box.ergo_box.value.as_u64();

    let liquidity_output = liquidity_state.into_box_candidate(creation_height)?;

    let order_outputs: Vec<_> = initial_orders
        .into_iter()
        .map(|o| o.into_box_candidate(creation_height))
        .collect::<Result<_, _>>()?;

    let fee_output = ErgoBoxCandidate {
        value: fee_value,
        ergo_tree: MINERS_FEE_ADDRESS.script().unwrap(),
        tokens: None,
        additional_registers: NonMandatoryRegisters::empty(),
        creation_height,
    };

    let selection = SimpleBoxSelector::new().select(wallet_boxes, missing_ergs.try_into()?, &[])?;

    let inputs: Vec<UnsignedInput> = once(liquidity_box.ergo_box.into())
        .chain(selection.boxes.into_iter().map(|b| b.into()))
        .collect();

    let change_output = selection
        .change_boxes
        .into_iter()
        .map(|assets| ErgoBoxCandidate {
            value: assets.value,
            ergo_tree: owner_address.script().unwrap(),
            tokens: assets.tokens,
            additional_registers: NonMandatoryRegisters::empty(),
            creation_height,
        });

    let output_candidates: Vec<ErgoBoxCandidate> = once(liquidity_output)
        .chain(order_outputs)
        .chain(change_output)
        .chain(once(fee_output))
        .collect();

    Ok(UnsignedTransaction::new_from_vec(
        inputs,
        vec![],
        output_candidates,
    )?)
}
