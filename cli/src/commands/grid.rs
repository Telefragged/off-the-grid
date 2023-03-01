use std::iter::once;

use anyhow::anyhow;
use clap::{ArgGroup, Args, Parser, Subcommand};
use ergo_lib::{
    chain::transaction::{unsigned::UnsignedTransaction, TransactionError, UnsignedInput},
    ergo_chain_types::{Digest32, EcPoint},
    ergotree_ir::chain::{
        address::Address,
        ergo_box::{
            box_value::{BoxValue, BoxValueError},
            ErgoBox, ErgoBoxCandidate, NonMandatoryRegisters,
        },
        token::{Token, TokenAmount, TokenAmountError, TokenId},
    },
    wallet::{
        box_selector::{BoxSelector, BoxSelectorError, SimpleBoxSelector},
        miner_fee::MINERS_FEE_ADDRESS,
    },
};
use itertools::Itertools;
use off_the_grid::{
    boxes::{
        liquidity_box::{LiquidityProvider, LiquidityProviderError},
        tracked_box::TrackedBox,
    },
    grid::grid_order::{GridOrder, GridOrderError, OrderState},
    node::client::NodeClient,
    spectrum::pool::SpectrumPool,
};
use thiserror::Error;
use tokio::try_join;

use crate::scan_config::ScanConfig;

#[derive(Parser)]
pub struct CreateOptions {
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
    #[clap(long, help = "Disable auto filling the grid orders")]
    no_auto_fill: bool,
    #[clap(short = 'y', help = "Submit transaction")]
    submit: bool,
    #[clap(short = 'i', long, help = "Grid group identity [default: random]")]
    grid_identity: Option<String>,
}

#[derive(Parser)]
#[command(group(
    ArgGroup::new("filter")
        .required(true)
        .args(&["token_id", "grid_identity", "all"])
))]
pub struct RedeemOptions {
    #[clap(short = 't', long, help = "TokenID to filter by")]
    token_id: Option<String>,
    #[clap(short = 'i', long, help = "Grid group identity")]
    grid_identity: Option<String>,
    #[clap(short = 'a', long, help = "Redeem all orders")]
    all: bool,
    #[clap(
        short,
        long,
        help = "transaction fee value, in nanoERGs",
        default_value_t = 1000000
    )]
    fee: u64,
    #[clap(short = 'y', help = "Submit transaction")]
    submit: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(group(
        ArgGroup::new("amount")
            .required(true)
            .args(&["token_amount", "total_value"])
    ))]
    Create(CreateOptions),
    Redeem(RedeemOptions),
    List {
        #[clap(short = 't', long, help = "TokenID to filter by")]
        token_id: Option<String>,
    },
}

#[derive(Args)]
pub struct GridCommand {
    #[clap(long, help = "Scan configuration file path [default: scan_config]")]
    scan_config: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

async fn handle_grid_create(
    node_client: NodeClient,
    scan_config: ScanConfig,
    options: CreateOptions,
) -> anyhow::Result<()> {
    let CreateOptions {
        token_id,
        token_amount,
        total_value,
        range,
        num_orders,
        fee,
        no_auto_fill,
        submit,
        grid_identity,
    } = options;
    let fee_value: BoxValue = fee.try_into()?;
    let token_id: TokenId = Digest32::try_from(token_id)?.into();
    let token_per_grid = match (token_amount, total_value) {
        (Some(token_amount), None) => {
            let tokens_per_grid = token_amount / num_orders;
            Ok(OrderValueTarget::Token(tokens_per_grid.try_into()?))
        }
        (None, Some(total_value)) => {
            let value_per_grid = total_value / num_orders;
            Ok(OrderValueTarget::Value(value_per_grid.try_into()?))
        }
        _ => Err(anyhow!(
            "Either token_amount or total_value must be specified"
        )),
    }?;

    let (wallet_boxes, wallet_status) = try_join!(
        node_client.wallet_boxes_unspent(),
        node_client.wallet_status()
    )?;

    wallet_status.error_if_locked()?;

    let liquidity_box = if !no_auto_fill {
        let n2t_pool_boxes = node_client
            .get_scan_unspent(scan_config.n2t_scan_id)
            .await?;
        Some(
            n2t_pool_boxes
                .into_iter()
                .filter_map(|b| {
                    b.try_into()
                        .ok()
                        .filter(|b: &TrackedBox<SpectrumPool>| b.value.asset_y.token_id == token_id)
                })
                .max_by_key(|lb| lb.value.amm_factor())
                .ok_or(anyhow!("No liquidity box found for token {:?}", token_id))?,
        )
    } else {
        None
    };

    let grid_identity = if let Some(grid_identity) = grid_identity {
        grid_identity
    } else {
        let mut generator = names::Generator::with_naming(names::Name::Numbered);
        generator
            .next()
            .ok_or(anyhow!("Failed to generate grid identity"))?
    };

    let tx = build_new_grid_tx(
        liquidity_box,
        range,
        num_orders,
        token_id,
        token_per_grid,
        wallet_status.change_address()?,
        fee_value,
        wallet_boxes,
        grid_identity,
    )?;

    let signed = node_client.wallet_transaction_sign(&tx).await?;

    if submit {
        let tx_id = node_client.transaction_submit(&signed).await?;
        println!("Transaction submitted: {:?}", tx_id);
    } else {
        println!("{}", serde_json::to_string_pretty(&signed)?);
    }

    Ok(())
}

pub async fn handle_grid_redeem(
    node_client: NodeClient,
    scan_config: ScanConfig,
    options: RedeemOptions,
) -> anyhow::Result<()> {
    let RedeemOptions {
        token_id,
        grid_identity,
        all: _,
        fee,
        submit,
    } = options;

    let grid_identity = grid_identity.map(|i| i.into_bytes());

    let token_id = token_id
        .map(|i| Digest32::try_from(i).map(|i| i.into()))
        .transpose()?;

    let grid_orders = node_client
        .get_scan_unspent(scan_config.wallet_grid_scan_id)
        .await?
        .into_iter()
        .filter_map(|b| b.try_into().ok())
        .filter(|b: &TrackedBox<GridOrder>| {
            grid_identity
                .as_ref()
                .map(|i| b.value.metadata.as_ref().map(|m| *m == *i).unwrap_or(false))
                .unwrap_or(true)
        })
        .filter(|b: &TrackedBox<GridOrder>| {
            token_id
                .as_ref()
                .map(|i| b.value.token.token_id == *i)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();

    if grid_orders.is_empty() {
        return Err(anyhow!("No grid orders found"));
    }

    let wallet_status = node_client.wallet_status().await?;
    wallet_status.error_if_locked()?;

    let tx = build_redeem_tx(
        grid_orders,
        node_client.wallet_status().await?.change_address()?,
        fee.try_into()?,
    )
    .unwrap();

    let signed = node_client.wallet_transaction_sign(&tx).await?;

    if submit {
        let tx_id = node_client.transaction_submit(&signed).await?;
        println!("Transaction submitted: {:?}", tx_id);
    } else {
        println!("{}", serde_json::to_string_pretty(&signed)?);
    }

    Ok(())
}

async fn handle_grid_list(
    node_client: NodeClient,
    scan_config: ScanConfig,
    token_id: Option<String>,
) -> Result<(), anyhow::Error> {
    let token_id = token_id
        .map(|i| Digest32::try_from(i).map(|i| i.into()))
        .transpose()?;

    let grid_orders = node_client
        .get_scan_unspent(scan_config.wallet_grid_scan_id)
        .await?
        .into_iter()
        .filter_map(|b| b.try_into().ok())
        .filter(|b: &TrackedBox<GridOrder>| {
            token_id
                .as_ref()
                .map(|i| b.value.token.token_id == *i)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();

    if grid_orders.is_empty() {
        println!("No grid orders found");
        return Ok(());
    }

    let grouped_orders = grid_orders
        .into_iter()
        .into_group_map_by(|o| o.value.metadata.clone());

    for (grid_identity, orders) in grouped_orders {
        let num_buy_orders = orders
            .iter()
            .filter(|o| o.value.state == OrderState::Buy)
            .count();
        let num_sell_orders = orders
            .iter()
            .filter(|o| o.value.state == OrderState::Sell)
            .count();

        let bid = orders
            .iter()
            .filter(|o| o.value.state == OrderState::Buy)
            .map(|o| o.value.bid)
            .max()
            .unwrap_or_default();

        let ask = orders
            .iter()
            .filter(|o| o.value.state == OrderState::Sell)
            .map(|o| o.value.ask)
            .min()
            .unwrap_or_default();

        let grid_identity = if let Some(grid_identity) = grid_identity.as_ref() {
            String::from_utf8(grid_identity.clone())
                .unwrap_or_else(|_| format!("{:?}", grid_identity))
        } else {
            "No identity".to_string()
        };

        println!("{}:", grid_identity);
        println!("  Buy orders: {}", num_buy_orders);
        println!("  Sell orders: {}", num_sell_orders);
        println!("  Bid: {}", bid);
        println!("  Ask: {}", ask);
        println!();
    }

    Ok(())
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
    }
}

fn build_redeem_tx(
    orders: Vec<TrackedBox<GridOrder>>,
    change_address: Address,
    fee_value: BoxValue,
) -> anyhow::Result<UnsignedTransaction> {
    let creation_height = orders
        .iter()
        .map(|o| o.ergo_box.creation_height)
        .max()
        .unwrap_or(0);

    let change_value = orders
        .iter()
        .map(|o| o.ergo_box.value.as_u64())
        .sum::<u64>()
        .checked_sub(*fee_value.as_u64())
        .ok_or(anyhow!("Not enough funds for fee"))?;

    let num_outputs = if orders.len() > 1 {
        orders.len() as u64 - 1
    } else {
        1
    };

    let mut remainder = change_value % num_outputs;

    let change_outputs = (0..num_outputs).map(|_| -> anyhow::Result<_> {
        let value = if remainder > 0 {
            remainder -= 1;
            change_value / num_outputs + 1
        } else {
            change_value / num_outputs
        };
        Ok(ErgoBoxCandidate {
            value: value.try_into()?,
            ergo_tree: change_address.script()?,
            tokens: None,
            additional_registers: NonMandatoryRegisters::empty(),
            creation_height,
        })
    });

    let fee_output_candidate = ErgoBoxCandidate {
        value: fee_value,
        ergo_tree: MINERS_FEE_ADDRESS.script().unwrap(),
        tokens: None,
        additional_registers: NonMandatoryRegisters::empty(),
        creation_height,
    };

    let inputs: Vec<_> = orders.into_iter().map(|o| o.ergo_box.into()).collect();

    Ok(UnsignedTransaction::new_from_vec(
        inputs,
        vec![],
        change_outputs
            .chain(once(Ok(fee_output_candidate)))
            .collect::<anyhow::Result<_>>()?,
    )?)
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

fn new_orders_with_liquidity<F>(
    liquidity_provider: impl LiquidityProvider,
    num_orders: u64,
    lo: u64,
    order_step: u64,
    grid_identity: String,
    owner_ec_point: EcPoint,
    grid_value_fn: F,
) -> Result<(Vec<GridOrder>, impl LiquidityProvider), BuildNewGridTxError>
where
    F: Fn(u64) -> u64,
{
    let token_id = liquidity_provider.asset_y().token_id.clone();

    let mut liquidity_state = liquidity_provider;

    let grid_identity = grid_identity.into_bytes();

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
                    OrderState::Sell
                }
                _ => OrderState::Buy,
            };
            GridOrder::new(
                owner_ec_point.clone(),
                bid,
                ask,
                token,
                order_state,
                Some(grid_identity.clone()),
            )
            .map_err(BuildNewGridTxError::GridOrder)
        })
        .collect::<Result<_, _>>()?;

    Ok((initial_orders, liquidity_state))
}

fn new_orders<F>(
    num_orders: u64,
    lo: u64,
    order_step: u64,
    token_id: TokenId,
    grid_identity: String,
    owner_ec_point: EcPoint,
    grid_value_fn: F,
) -> Result<Vec<GridOrder>, BuildNewGridTxError>
where
    F: Fn(u64) -> u64,
{
    let grid_identity = grid_identity.into_bytes();

    let initial_orders: Vec<_> = (0..num_orders)
        .rev()
        .map(|n| {
            let ask = lo + order_step * (n + 1);
            let bid = lo + order_step * n;

            let amount = grid_value_fn(bid);
            let token: Token = (token_id.clone(), amount.try_into()?).into();

            GridOrder::new(
                owner_ec_point.clone(),
                bid,
                ask,
                token,
                OrderState::Buy,
                Some(grid_identity.clone()),
            )
            .map_err(BuildNewGridTxError::GridOrder)
        })
        .collect::<Result<_, _>>()?;

    Ok(initial_orders)
}

enum OrderValueTarget {
    Value(BoxValue),
    Token(TokenAmount),
}

/// Build a transaction that creates a new grid of orders
#[allow(clippy::too_many_arguments)]
fn build_new_grid_tx(
    liquidity_box: Option<TrackedBox<impl LiquidityProvider>>,
    grid_range: GridOrderRange,
    num_orders: u64,
    token_id: TokenId,
    order_value_target: OrderValueTarget,
    owner_address: Address,
    fee_value: BoxValue,
    wallet_boxes: Vec<ErgoBox>,
    grid_identity: String,
) -> Result<UnsignedTransaction, BuildNewGridTxError> {
    let creation_height = liquidity_box
        .as_ref()
        .map(|lb| &lb.ergo_box)
        .into_iter()
        .chain(wallet_boxes.iter())
        .map(|b| b.creation_height)
        .max()
        .unwrap_or(0);

    let GridOrderRange(lo, hi) = grid_range;

    let grid_value_fn: Box<dyn Fn(u64) -> u64> = match order_value_target {
        OrderValueTarget::Value(value_per_grid) => {
            Box::new(move |bid: u64| value_per_grid.as_u64() / bid)
        }
        OrderValueTarget::Token(token_per_grid) => Box::new(move |_: u64| *token_per_grid.as_u64()),
    };

    let order_step = (hi - lo) / num_orders;

    let owner_ec_point = if let Address::P2Pk(owner_dlog) = &owner_address {
        Ok(*owner_dlog.h.clone())
    } else {
        Err(anyhow!("change address is not P2PK"))
    }
    .unwrap();

    let (initial_orders, liquidity_state) = if let Some(liquidity_box) = &liquidity_box {
        let (initial_orders, liquidity_state) = new_orders_with_liquidity(
            liquidity_box.value.clone(),
            num_orders,
            lo,
            order_step,
            grid_identity,
            owner_ec_point,
            grid_value_fn,
        )?;

        (initial_orders, Some(liquidity_state))
    } else {
        let initial_orders = new_orders(
            num_orders,
            lo,
            order_step,
            token_id,
            grid_identity,
            owner_ec_point,
            grid_value_fn,
        )?;

        (initial_orders, None)
    };

    let missing_ergs = initial_orders
        .iter()
        .map(|o| o.value.as_i64())
        .chain(once(fee_value.as_i64()))
        .chain(
            liquidity_state
                .iter()
                .map(|s| *s.asset_x().amount.as_u64() as i64),
        )
        .chain(liquidity_box.iter().map(|lb| -lb.ergo_box.value.as_i64()))
        .sum::<i64>();

    let liquidity_output = liquidity_state
        .map(|state| state.into_box_candidate(creation_height))
        .transpose()?;

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

    let inputs: Vec<UnsignedInput> = liquidity_box
        .map(|lb| lb.ergo_box.into())
        .into_iter()
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

    let output_candidates: Vec<ErgoBoxCandidate> = liquidity_output
        .into_iter()
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
