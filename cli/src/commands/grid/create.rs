use std::iter::once;

use anyhow::{anyhow, Context};
use clap::{ArgGroup, Parser};
use colored::Colorize;
use ergo_lib::{
    chain::transaction::{unsigned::UnsignedTransaction, TransactionError, UnsignedInput},
    ergo_chain_types::EcPoint,
    ergotree_ir::chain::{
        address::Address,
        ergo_box::{
            box_value::{BoxValue, BoxValueError},
            ErgoBox, ErgoBoxCandidate, NonMandatoryRegisters,
        },
        token::{TokenAmount, TokenAmountError, TokenId},
    },
    wallet::{
        box_selector::{BoxSelector, BoxSelectorError, ErgoBoxAssetsData, SimpleBoxSelector},
        miner_fee::MINERS_FEE_ADDRESS,
    },
};
use num_traits::ToPrimitive;
use off_the_grid::{
    boxes::{
        describe_box::{BoxAssetDisplay, ErgoBoxDescriptors},
        liquidity_box::{LiquidityProvider, LiquidityProviderError},
        tracked_box::TrackedBox,
        wallet_box::WalletBox,
    },
    grid::multigrid_order::{
        FillMultiGridOrders, GridOrderEntries, GridOrderEntry, MultiGridOrder, MultiGridOrderError,
        OrderState,
    },
    node::client::NodeClient,
    spectrum::pool::SpectrumPool,
    units::{Fraction, Price, TokenStore, Unit, UnitAmount, ERG_UNIT},
};
use tabled::{
    row,
    settings::{
        object::{Columns, Rows},
        Alignment, Disable, Format, Modify, Style,
    },
    Table, Tabled,
};
use thiserror::Error;
use tokio::try_join;

use crate::scan_config::ScanConfig;

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

fn grid_order_range_from_str(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.split("..").collect();
    if let [start, stop] = parts.as_slice() {
        Ok((start.to_string(), stop.to_string()))
    } else {
        Err(format!("Invalid range: {}", s))
    }
}

#[derive(Clone, Debug)]
struct GridPriceRange<'a> {
    start: Price<'a>,
    stop: Price<'a>,
    num_orders: u64,
}

#[derive(Error, Debug)]
enum GridOrderRangeError {
    #[error("Invalid range: start must be below stop")]
    InvalidRange,
}

impl<'a> GridPriceRange<'a> {
    pub fn new(
        start: Price<'a>,
        stop: Price<'a>,
        num_orders: u64,
    ) -> Result<Self, GridOrderRangeError> {
        if start.price() >= stop.price() {
            return Err(GridOrderRangeError::InvalidRange);
        }

        Ok(GridPriceRange {
            start,
            stop,
            num_orders,
        })
    }
}

impl IntoIterator for GridPriceRange<'_> {
    type Item = (Fraction, Fraction);
    type IntoIter = GridPriceIterator;

    fn into_iter(self) -> Self::IntoIter {
        let start = self.start.price();
        let stop = self.stop.price();
        let step = (stop - start) / self.num_orders;
        GridPriceIterator {
            base: start,
            current: 0,
            num_orders: self.num_orders,
            step,
        }
    }
}

struct GridPriceIterator {
    base: Fraction,
    current: u64,
    num_orders: u64,
    step: Fraction,
}

impl Iterator for GridPriceIterator {
    type Item = (Fraction, Fraction);

    fn next(&mut self) -> Option<Self::Item> {
        if self.current >= self.num_orders {
            return None;
        }

        let lo = self.base + self.step * self.current;
        let hi = self.base + self.step * (self.current + 1);

        self.current += 1;
        // return the reciprocal of the fraction to get the price
        // in the base token
        Some((hi.recip(), lo.recip()))
    }
}

#[derive(Error, Debug)]
pub enum BuildNewGridTxError {
    #[error(transparent)]
    LiquidityProvider(#[from] LiquidityProviderError),
    #[error(transparent)]
    TokenAmount(#[from] TokenAmountError),
    #[error(transparent)]
    MultiGridOrder(#[from] MultiGridOrderError),
    #[error(transparent)]
    BoxValue(#[from] BoxValueError),
    #[error(transparent)]
    BoxSelector(#[from] BoxSelectorError),
    #[error(transparent)]
    Transaction(#[from] TransactionError),
    #[error("Invalid fraction: {0}")]
    InvalidFraction(Fraction),
}

pub async fn handle_grid_create(
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

    let erg_unit = *ERG_UNIT;

    let unit: Unit = token_store
        .get_unit_by_id(&token_id)
        .ok_or_else(|| anyhow!("{} is not a known token or a valid token ID", token_id))?;

    let token_id = unit.token_id();

    let fee_amount = erg_unit
        .str_amount(&fee)
        .ok_or_else(|| anyhow!("Invalid fee value"))?;

    let fee_value: BoxValue = fee_amount.amount().try_into()?;

    let token_per_grid = match (token_amount, total_value) {
        (Some(token_amount), None) => {
            let token_amount = unit
                .str_amount(&token_amount)
                .ok_or_else(|| anyhow!("Invalid token amount {}", token_amount))?;

            let tokens_per_grid = token_amount.amount() / num_orders;
            Ok(OrderValueTarget::Token(tokens_per_grid.try_into()?))
        }
        (None, Some(total_value)) => {
            let total_value = erg_unit
                .str_amount(&total_value)
                .ok_or_else(|| anyhow!("Invalid total value {}", total_value))?;

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

    let start: Fraction = range
        .0
        .parse()
        .map_err(|_| anyhow!("Failed to parse start price {}", range.0))?;

    let end: Fraction = range
        .1
        .parse()
        .map_err(|_| anyhow!("Failed to parse end price {}", range.1))?;

    let start_price = Price::new(unit, erg_unit, start);
    let end_price = Price::new(unit, erg_unit, end);

    let range = GridPriceRange::new(start_price, end_price, num_orders)?;

    let grid_tx_data = build_new_grid_data(
        liquidity_box,
        range,
        token_id,
        token_per_grid,
        wallet_status.change_address()?,
        fee_value,
        wallet_boxes,
        grid_identity,
    )
    .context("Creating grid transaction")?;

    if submit {
        let tx = grid_tx_data.try_into()?;

        let signed = node_client.wallet_transaction_sign(&tx).await?;

        let tx_id = node_client.transaction_submit(&signed).await?;
        println!("Transaction submitted: {:?}", tx_id);
    } else {
        print_grid_tx_data_summary(&grid_tx_data, &token_store);
    }

    Ok(())
}

fn fraction_to_u64(fraction: Fraction) -> Result<u64, BuildNewGridTxError> {
    fraction
        .to_u64()
        .ok_or(BuildNewGridTxError::InvalidFraction(fraction))
}

fn new_multi_order<F>(
    range: GridPriceRange,
    token_id: TokenId,
    grid_identity: String,
    owner_ec_point: EcPoint,
    grid_value_fn: F,
) -> Result<MultiGridOrder, BuildNewGridTxError>
where
    F: Fn(Fraction) -> Result<u64, BuildNewGridTxError>,
{
    let grid_identity = grid_identity.into_bytes();

    let initial_orders: GridOrderEntries = range
        .into_iter()
        .map(|(bid, ask)| {
            let amount = grid_value_fn(bid)?;

            Result::<_, BuildNewGridTxError>::Ok(GridOrderEntry::new(
                OrderState::Buy,
                amount.try_into()?,
                fraction_to_u64((bid * amount).floor())?,
                fraction_to_u64((ask * amount).floor())?,
            ))
        })
        .collect::<Result<_, _>>()?;

    Ok(MultiGridOrder::new(
        owner_ec_point,
        token_id,
        initial_orders,
        Some(grid_identity),
    )?)
}

enum OrderValueTarget {
    Value(BoxValue),
    Token(TokenAmount),
}

enum LiquidityData<T: LiquidityProvider> {
    WithLiquidity { input: TrackedBox<T>, output: T },
    WithoutLiquidity,
}

impl<T> LiquidityData<T>
where
    T: LiquidityProvider,
{
    pub fn creation_height(&self) -> u32 {
        match self {
            LiquidityData::WithLiquidity { input, output: _ } => input.ergo_box.creation_height,
            LiquidityData::WithoutLiquidity => 0,
        }
    }

    pub fn input(&self) -> Option<&TrackedBox<T>> {
        match self {
            LiquidityData::WithLiquidity { input, output: _ } => Some(input),
            LiquidityData::WithoutLiquidity => None,
        }
    }

    pub fn output(&self) -> Option<&T> {
        match self {
            LiquidityData::WithLiquidity { input: _, output } => Some(output),
            LiquidityData::WithoutLiquidity => None,
        }
    }
}

struct MinerFeeValue(pub BoxValue);

impl ErgoBoxDescriptors for MinerFeeValue {
    fn box_name(&self) -> String {
        "Miner fee".to_string()
    }

    fn assets<'a>(&self, _: &'a TokenStore) -> BoxAssetDisplay<'a> {
        let amount = UnitAmount::new(*ERG_UNIT, *self.0.as_u64());
        BoxAssetDisplay::Single(amount)
    }
}

struct NewGridTxData<T: LiquidityProvider> {
    owner_address: Address,
    liquidity_data: LiquidityData<T>,
    selected_boxes: Vec<WalletBox<ErgoBox>>,
    change_boxes: Vec<WalletBox<ErgoBoxAssetsData>>,
    grid_output: MultiGridOrder,
    fee_value: MinerFeeValue,
}

impl<T> NewGridTxData<T>
where
    T: LiquidityProvider + ErgoBoxDescriptors,
{
    pub fn iter_input_descriptions(&self) -> impl Iterator<Item = &dyn ErgoBoxDescriptors> {
        let liquidity_iter = self
            .liquidity_data
            .input()
            .into_iter()
            .map(|input| input as &dyn ErgoBoxDescriptors);

        liquidity_iter
            .chain(
                self.selected_boxes
                    .iter()
                    .map(|input| input as &dyn ErgoBoxDescriptors),
            )
            .fuse()
    }

    pub fn iter_output_descriptions(&self) -> impl Iterator<Item = &dyn ErgoBoxDescriptors> {
        let liquidity_iter = self
            .liquidity_data
            .output()
            .into_iter()
            .map(|output| output as &dyn ErgoBoxDescriptors);

        liquidity_iter
            .chain(once(&self.grid_output as &dyn ErgoBoxDescriptors))
            .chain(
                self.change_boxes
                    .iter()
                    .map(|output| output as &dyn ErgoBoxDescriptors),
            )
            .chain(once(&self.fee_value as &dyn ErgoBoxDescriptors))
            .fuse()
    }
}

impl<T> TryFrom<NewGridTxData<T>> for UnsignedTransaction
where
    T: LiquidityProvider,
{
    type Error = BuildNewGridTxError;

    fn try_from(value: NewGridTxData<T>) -> Result<Self, Self::Error> {
        let creation_height = value
            .selected_boxes
            .iter()
            .map(|input| input.0.creation_height)
            .max()
            .unwrap_or_else(|| value.liquidity_data.creation_height());

        let (liquidity_input, liquidity_output) = match value.liquidity_data {
            LiquidityData::WithLiquidity {
                input: liquidity_box_input,
                output,
            } => {
                let input = liquidity_box_input.ergo_box.into();

                let output_candidate = output.into_box_candidate(creation_height)?;

                (Some(input), Some(output_candidate))
            }
            LiquidityData::WithoutLiquidity => (None, None),
        };

        let inputs: Vec<UnsignedInput> = value
            .selected_boxes
            .into_iter()
            .map(|input| input.into_inner().into())
            .chain(liquidity_input)
            .collect();

        let grid_output = value.grid_output.into_box_candidate(creation_height)?;

        let change_outputs = value
            .change_boxes
            .into_iter()
            .map(|assets| ErgoBoxCandidate {
                value: assets.0.value,
                ergo_tree: value
                    .owner_address
                    .script()
                    .expect("Address was already parsed"),
                tokens: assets.0.tokens,
                additional_registers: NonMandatoryRegisters::empty(),
                creation_height,
            });

        let fee_output = ErgoBoxCandidate {
            value: value.fee_value.0,
            ergo_tree: MINERS_FEE_ADDRESS.script().unwrap(),
            tokens: None,
            additional_registers: NonMandatoryRegisters::empty(),
            creation_height,
        };

        let output_candidates: Vec<ErgoBoxCandidate> = liquidity_output
            .into_iter()
            .chain(once(grid_output))
            .chain(change_outputs)
            .chain(once(fee_output))
            .collect();

        Ok(UnsignedTransaction::new_from_vec(
            inputs,
            vec![],
            output_candidates,
        )?)
    }
}

#[derive(Tabled)]
struct BoxSummary {
    #[tabled(rename = "Box type")]
    box_type: String,
    #[tabled(rename = "Value")]
    value: String,
    #[tabled(rename = "Tokens")]
    token: String,
}

impl BoxSummary {
    pub fn new(desc: &dyn ErgoBoxDescriptors, token_store: &TokenStore) -> Self {
        let (first_asset, second_asset) = desc.assets(token_store).strings(None);
        Self {
            box_type: desc.box_name(),
            value: first_asset,
            token: second_asset,
        }
    }
}

fn style_box_table<F>(table: &mut Table, formatting: F)
where
    F: FnMut(&str) -> String + Clone,
{
    table
        .with(Style::empty())
        .with(Modify::new(Columns::new(1..)).with(Alignment::right()))
        .with(Modify::new(Rows::single(0)).with(Alignment::left()))
        .with(Disable::row(Columns::single(0)))
        .with(Modify::new(Columns::new(0..)).with(Format::content(formatting)));
}

fn print_grid_tx_data_summary<T>(data: &NewGridTxData<T>, token_store: &TokenStore)
where
    T: LiquidityProvider + ErgoBoxDescriptors,
{
    let input_descriptions = data
        .iter_input_descriptions()
        .map(|input| BoxSummary::new(input, token_store));

    let output_descriptions = data
        .iter_output_descriptions()
        .map(|output| BoxSummary::new(output, token_store));

    let mut input = Table::new(input_descriptions);
    style_box_table(&mut input, |i| i.bright_red().to_string());

    let mut output = Table::new(output_descriptions);
    style_box_table(&mut output, |i| i.bright_green().to_string());

    let mut combined = row![input, output];
    combined.with(Style::empty());

    println!("{}", combined);
}

/// Build a transaction that creates a new grid of orders
#[allow(clippy::too_many_arguments)]
fn build_new_grid_data<T: LiquidityProvider>(
    liquidity_box: Option<TrackedBox<T>>,
    grid_range: GridPriceRange,
    token_id: TokenId,
    order_value_target: OrderValueTarget,
    owner_address: Address,
    fee_value: BoxValue,
    wallet_boxes: Vec<WalletBox<ErgoBox>>,
    grid_identity: String,
) -> Result<NewGridTxData<T>, BuildNewGridTxError> {
    let grid_value_fn: Box<dyn Fn(Fraction) -> Result<u64, BuildNewGridTxError>> =
        match order_value_target {
            OrderValueTarget::Value(value_per_grid) => Box::new(move |bid: Fraction| {
                fraction_to_u64((Fraction::from(*value_per_grid.as_u64()) / bid).floor())
            }),
            OrderValueTarget::Token(token_per_grid) => {
                Box::new(move |_: Fraction| Ok(*token_per_grid.as_u64()))
            }
        };

    let owner_ec_point = if let Address::P2Pk(owner_dlog) = &owner_address {
        Ok(*owner_dlog.h.clone())
    } else {
        Err(anyhow!("change address is not P2PK"))
    }
    .unwrap();

    let initial_order = new_multi_order(
        grid_range,
        token_id,
        grid_identity,
        owner_ec_point,
        grid_value_fn,
    )?;

    let (liquidity_state, initial_orders) = match liquidity_box.as_ref() {
        Some(liquidity_box) => {
            let (liquidity_state, initial_orders) =
                fill_orders(liquidity_box.value.clone(), initial_order)?;

            (liquidity_state, initial_orders)
        }
        None => (None, initial_order),
    };

    let missing_ergs = once(initial_orders.value.as_i64())
        .chain(once(fee_value.as_i64()))
        .chain(
            liquidity_state
                .iter()
                .map(|s| *s.asset_x().amount.as_u64() as i64),
        )
        .chain(liquidity_box.iter().map(|lb| -lb.ergo_box.value.as_i64()))
        .sum::<i64>()
        .try_into()?;

    let selection = SimpleBoxSelector::new().select(wallet_boxes, missing_ergs, &[])?;

    let liquidity_data = liquidity_box
        .zip(liquidity_state)
        .map(|(input, output)| LiquidityData::WithLiquidity { input, output })
        .unwrap_or(LiquidityData::WithoutLiquidity);

    let change_boxes = selection
        .change_boxes
        .into_iter()
        .map(WalletBox::new)
        .collect();

    let fee_value = MinerFeeValue(fee_value);

    Ok(NewGridTxData {
        liquidity_data,
        grid_output: initial_orders,
        selected_boxes: selection.boxes.into(),
        change_boxes,
        fee_value,
        owner_address,
    })
}

fn fill_orders<T: LiquidityProvider>(
    liquidity_box: T,
    order: MultiGridOrder,
) -> Result<(Option<T>, MultiGridOrder), LiquidityProviderError> {
    let (new_pool, filled) = liquidity_box.fill_orders(vec![&order])?;
    match filled.into_iter().next() {
        Some((_, filled_order)) => Ok((Some(new_pool), filled_order)),
        None => Ok((None, order)),
    }
}
