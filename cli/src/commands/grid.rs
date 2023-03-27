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
use fraction::ToPrimitive;
use itertools::Itertools;
use off_the_grid::{
    boxes::{
        liquidity_box::{LiquidityProvider, LiquidityProviderError},
        tracked_box::TrackedBox,
    },
    grid::grid_order::{GridOrder, GridOrderError, OrderState},
    node::client::NodeClient,
    spectrum::pool::SpectrumPool,
    units::{Price, TokenStore, Unit, UnitAmount},
};
use thiserror::Error;
use tokio::try_join;

use crate::scan_config::ScanConfig;
use off_the_grid::units::Fraction;

#[derive(Parser)]
#[command(group(
    ArgGroup::new("amount")
        .required(true)
        .args(&["token_amount", "total_value"])
))]
pub struct CreateOptions {
    #[clap(short = 't', long, help = "TokenID of the token to be traded")]
    token_id: String,
    #[clap(
        short = 'n',
        long,
        help = "Total amount of tokens to be traded",
        group = "amount"
    )]
    token_amount: Option<String>,
    #[clap(short = 'v', long, help = "Total value of the grid", group = "amount")]
    total_value: Option<String>,
    #[clap(
        short = 'r',
        long,
        help = "Range of the grid, in the form start..stop",
        value_parser = grid_order_range_from_str
    )]
    range: (String, String),
    #[clap(short = 'o', long, help = "Number of orders in the grid")]
    num_orders: u64,
    #[clap(short, long, help = "transaction fee value", default_value = "0.001")]
    fee: String,
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

fn grid_order_range_from_str(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.split("..").collect();
    if let [start, stop] = parts.as_slice() {
        Ok((start.to_string(), stop.to_string()))
    } else {
        Err(format!("Invalid range: {}", s))
    }
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

    let token_store = TokenStore::load(None)?;

    let erg_unit = token_store.erg_unit();

    let unit: Unit = token_store
        .get_unit_by_id(token_id.clone())
        .ok_or_else(|| {
            anyhow!(format!(
                "{} is not a known token or a valid token ID",
                token_id
            ))
        })?;

    let token_id = unit.token_id();

    let fee_amount = erg_unit
        .str_amount(&fee)
        .ok_or_else(|| anyhow!("Invalid fee value"))?;

    let fee_value: BoxValue = fee_amount.amount().try_into()?;

    let token_per_grid = match (token_amount, total_value) {
        (Some(token_amount), None) => {
            let token_amount = unit
                .str_amount(&token_amount)
                .ok_or_else(|| anyhow!(format!("Invalid token amount {}", token_amount)))?;

            let tokens_per_grid = token_amount.amount() / num_orders;
            Ok(OrderValueTarget::Token(tokens_per_grid.try_into()?))
        }
        (None, Some(total_value)) => {
            let total_value = erg_unit
                .str_amount(&total_value)
                .ok_or_else(|| anyhow!(format!("Invalid total value {}", total_value)))?;

            let value_per_grid = total_value.amount() / num_orders;
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

    let start_price: Fraction = range
        .0
        .parse()
        .map_err(|_| anyhow!("Failed to parse start price {}", range.0))?;

    let end_price: Fraction = range
        .1
        .parse()
        .map_err(|_| anyhow!("Failed to parse end price {}", range.1))?;

    let indirect_start = Price::new(unit.clone(), erg_unit.clone(), end_price).indirect();
    let indirect_end = Price::new(unit, erg_unit, start_price).indirect();

    let range = GridOrderRange::new(indirect_start.price(), indirect_end.price())?;

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

    let tokens = TokenStore::load(None)?;

    let grouped_orders = grid_orders
        .into_iter()
        .into_group_map_by(|o| o.value.metadata.clone());

    let name_width = grouped_orders
        .keys()
        .map(|k| k.as_ref().map(|k| k.len()).unwrap_or(0))
        .max()
        .unwrap_or(0);

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
            .map(|o| o.value.bid())
            .max()
            .unwrap_or_default();

        let ask = orders
            .iter()
            .filter(|o| o.value.state == OrderState::Sell)
            .map(|o| o.value.ask())
            .min()
            .unwrap_or_default();

        let profit = orders.iter().map(|o| o.value.profit()).sum::<u64>();

        let total_value = orders.iter().map(|o| o.value.value.as_u64()).sum::<u64>();

        let total_tokens = orders
            .iter()
            .filter(|o| o.value.state == OrderState::Sell)
            .map(|o| o.value.token.amount.as_u64())
            .sum::<u64>();

        let token_id = orders
            .iter()
            .map(|o| o.value.token.token_id)
            .next()
            .unwrap();

        let token_info = tokens.get_unit(&token_id);
        let erg_info = tokens.erg_unit();

        let total_value = UnitAmount::new(erg_info.clone(), total_value);
        let total_tokens = UnitAmount::new(token_info.clone(), total_tokens);

        let profit = UnitAmount::new(erg_info.clone(), profit);

        let to_price = |amount: Fraction| Price::new(token_info.clone(), erg_info.clone(), amount);

        let bid = to_price(bid);
        let ask = to_price(ask);
        let profit_in_token = ask.convert_price(&profit).unwrap();

        let grid_identity = if let Some(grid_identity) = grid_identity.as_ref() {
            String::from_utf8(grid_identity.clone())
                .unwrap_or_else(|_| format!("{:?}", grid_identity))
        } else {
            "No identity".to_string()
        };

        println!(
            "{: <9$} | {} Sell {} Buy, Bid {} Ask {}, Profit {} ({}), Total {} {}",
            grid_identity,
            num_sell_orders,
            num_buy_orders,
            bid.indirect(),
            ask.indirect(),
            profit,
            profit_in_token,
            total_value,
            total_tokens,
            name_width
        );
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
    #[error("Invalid fraction: {0}")]
    InvalidFraction(Fraction),
}

#[derive(Clone, Debug)]
pub struct GridOrderRange(Fraction, Fraction);

#[derive(Error, Debug)]
pub enum GridOrderRangeError {
    #[error("Invalid range: start and stop must be different")]
    InvalidRange,
}

impl GridOrderRange {
    pub fn new(start: Fraction, stop: Fraction) -> Result<Self, GridOrderRangeError> {
        if start != stop {
            Ok(GridOrderRange(
                start.clone().min(stop.clone()),
                start.max(stop),
            ))
        } else {
            Err(GridOrderRangeError::InvalidRange)
        }
    }
}

fn fraction_to_u64(fraction: Fraction) -> Result<u64, BuildNewGridTxError> {
    fraction
        .to_u64()
        .ok_or(BuildNewGridTxError::InvalidFraction(fraction))
}

fn new_orders_with_liquidity<F>(
    liquidity_provider: impl LiquidityProvider,
    num_orders: u64,
    lo: Fraction,
    order_step: Fraction,
    grid_identity: String,
    owner_ec_point: EcPoint,
    grid_value_fn: F,
) -> Result<(Vec<GridOrder>, impl LiquidityProvider), BuildNewGridTxError>
where
    F: Fn(Fraction) -> Result<u64, BuildNewGridTxError>,
{
    let token_id = liquidity_provider.asset_y().token_id;

    let mut liquidity_state = liquidity_provider;

    let grid_identity = grid_identity.into_bytes();

    let initial_orders: Vec<_> = (0..num_orders)
        .rev()
        .map(|n| {
            let ask = &lo + &order_step * (n + 1);
            let bid = &lo + &order_step * n;

            let amount = grid_value_fn(bid.clone())?;
            let token: Token = (token_id, amount.try_into()?).into();

            let bid_amount = fraction_to_u64((bid * amount).floor())?;

            let order_state = match liquidity_state.input_amount(&token) {
                Ok(t) if *t.amount.as_u64() <= bid_amount => {
                    liquidity_state = liquidity_state.clone().with_swap(&t)?;
                    OrderState::Sell
                }
                _ => OrderState::Buy,
            };
            GridOrder::new(
                owner_ec_point.clone(),
                bid_amount,
                fraction_to_u64((ask * amount).ceil())?,
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
    lo: Fraction,
    order_step: Fraction,
    token_id: TokenId,
    grid_identity: String,
    owner_ec_point: EcPoint,
    grid_value_fn: F,
) -> Result<Vec<GridOrder>, BuildNewGridTxError>
where
    F: Fn(Fraction) -> Result<u64, BuildNewGridTxError>,
{
    let grid_identity = grid_identity.into_bytes();

    let initial_orders: Vec<_> = (0..num_orders)
        .rev()
        .map(|n| {
            let ask = &lo + &order_step * (n + 1);
            let bid = &lo + &order_step * n;

            let amount = grid_value_fn(bid.clone())?;
            let token: Token = (token_id, amount.try_into()?).into();

            GridOrder::new(
                owner_ec_point.clone(),
                fraction_to_u64((bid * amount).floor())?,
                fraction_to_u64((ask * amount).ceil())?,
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

    let grid_value_fn: Box<dyn Fn(Fraction) -> Result<u64, BuildNewGridTxError>> =
        match order_value_target {
            OrderValueTarget::Value(value_per_grid) => Box::new(move |bid: Fraction| {
                fraction_to_u64((Fraction::from(*value_per_grid.as_u64()) / bid).floor())
            }),
            OrderValueTarget::Token(token_per_grid) => {
                Box::new(move |_: Fraction| Ok(*token_per_grid.as_u64()))
            }
        };

    let order_step = (hi - lo.clone()) / num_orders;

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
