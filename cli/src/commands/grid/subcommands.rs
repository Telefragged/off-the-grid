use std::collections::{hash_map::Entry, HashMap};

use anyhow::anyhow;
use clap::{ArgGroup, Parser};
use ergo_lib::{
    ergo_chain_types::Digest32,
    ergotree_ir::chain::{
        address::Address,
        ergo_box::box_value::BoxValue,
        token::{Token, TokenAmount, TokenId},
    },
    wallet::box_selector::ErgoBoxAssetsData,
};
use off_the_grid::{
    boxes::{tracked_box::TrackedBox, wallet_box::WalletBox},
    grid::multigrid_order::{MultiGridOrder, OrderState},
    node::client::NodeClient,
    units::{Price, TokenStore, UnitAmount, ERG_UNIT},
};
use tabled::Table;

use crate::scan_config::ScanConfig;
use off_the_grid::units::Fraction;

use super::{MinerFeeValue, SummarizedInput, SummarizedOutput, SummarizedTransaction};

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
        default_value = "0.001"
    )]
    fee: String,
    #[clap(short = 'y', help = "Submit transaction")]
    submit: bool,
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

    let token_store = TokenStore::load(None)?;

    let grid_identity = grid_identity.map(|i| i.into_bytes());

    let fee_amount = ERG_UNIT
        .str_amount(&fee)
        .ok_or_else(|| anyhow!("Invalid fee value"))?;

    let token_id = token_id
        .map(|i| Digest32::try_from(i).map(|i| i.into()))
        .transpose()?;

    let grid_orders = node_client
        .get_scan_unspent(scan_config.wallet_multigrid_scan_id)
        .await?
        .into_iter()
        .filter_map(|b| b.try_into().ok())
        .filter(|b: &TrackedBox<MultiGridOrder>| {
            grid_identity
                .as_ref()
                .map(|i| b.value.metadata.as_ref().map(|m| *m == *i).unwrap_or(false))
                .unwrap_or(true)
        })
        .filter(|b: &TrackedBox<MultiGridOrder>| {
            token_id
                .as_ref()
                .map(|i| b.value.token_id == *i)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();

    if grid_orders.is_empty() {
        return Err(anyhow!("No grid orders found"));
    }

    let wallet_status = node_client.wallet_status().await?;
    wallet_status.error_if_locked()?;

    let fee_value: BoxValue = fee_amount.amount().try_into()?;

    let redeem_data = build_redeem_multi_tx(
        grid_orders,
        node_client.wallet_status().await?.change_address()?,
        fee_value,
    )?;

    let described_tx = redeem_data.into_described_tx(&token_store)?;

    if submit {
        let tx = described_tx.try_into()?;
        let signed = node_client.wallet_transaction_sign(&tx).await?;

        let tx_id = node_client.transaction_submit(&signed).await?;
        println!("Transaction submitted: {:?}", tx_id);
    } else {
        let table: Table = described_tx.into();
        println!("{}", table);
    }

    Ok(())
}

pub async fn handle_grid_list(
    node_client: NodeClient,
    scan_config: ScanConfig,
    token_id: Option<String>,
) -> Result<(), anyhow::Error> {
    let token_id = token_id
        .map(|i| Digest32::try_from(i).map(|i| i.into()))
        .transpose()?;

    let grid_orders = node_client
        .get_scan_unspent(scan_config.wallet_multigrid_scan_id)
        .await?
        .into_iter()
        .filter_map(|b| b.try_into().ok())
        .filter(|b: &TrackedBox<MultiGridOrder>| {
            token_id
                .as_ref()
                .map(|i| b.value.token_id == *i)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();

    if grid_orders.is_empty() {
        println!("No grid orders found");
        return Ok(());
    }

    let tokens = TokenStore::load(None)?;

    let name_width = grid_orders
        .iter()
        .map(|o| o.value.metadata.as_ref().map(|m| m.len()).unwrap_or(0))
        .max()
        .unwrap_or(0);

    for order in grid_orders {
        let entries = &order.value.entries;

        let num_buy_orders = entries
            .iter()
            .filter(|o| o.state == OrderState::Buy)
            .count();

        let num_sell_orders = entries
            .iter()
            .filter(|o| o.state == OrderState::Sell)
            .count();

        let bid = entries.bid_entry().map(|o| o.bid()).unwrap_or_default();

        let ask = entries.ask_entry().map(|o| o.ask()).unwrap_or_default();

        let profit = order.value.profit();

        let total_value = *order.value.value.as_u64();

        let total_tokens = order
            .ergo_box
            .tokens
            .as_ref()
            .map(|t| *t.first().amount.as_u64())
            .unwrap_or(0);

        let token_id = order.value.token_id;

        let token_info = tokens.get_unit(&token_id);
        let erg_info = *ERG_UNIT;

        let total_value = UnitAmount::new(erg_info, total_value);
        let total_tokens = UnitAmount::new(token_info, total_tokens);

        let profit = UnitAmount::new(erg_info, profit);

        let to_price = |amount: Fraction| Price::new(token_info, erg_info, amount);

        let bid = to_price(bid);
        let ask = to_price(ask);
        let profit_in_token = ask.convert_price(&profit).unwrap();

        let grid_identity = if let Some(grid_identity) = order.value.metadata.as_ref() {
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

pub async fn handle_grid_details(
    node_client: NodeClient,
    scan_config: ScanConfig,
    grid_identity: String,
) -> Result<(), anyhow::Error> {
    let grid_identity = grid_identity.into_bytes();

    let grid_order = node_client
        .get_scan_unspent(scan_config.wallet_multigrid_scan_id)
        .await?
        .into_iter()
        .filter_map(|b| b.try_into().ok())
        .find(|b: &TrackedBox<MultiGridOrder>| {
            b.value
                .metadata
                .as_ref()
                .map(|i| *i == *grid_identity)
                .unwrap_or(false)
        });

    match grid_order {
        Some(grid_order) => {
            let tokens = TokenStore::load(None)?;

            let token_id = grid_order.value.token_id;

            let token_info = tokens.get_unit(&token_id);
            let erg_info = *ERG_UNIT;

            for entry in grid_order.value.entries.iter() {
                let bid = entry.bid();
                let ask = entry.ask();

                let to_price = |amount: Fraction| Price::new(token_info, erg_info, amount);

                let price = match entry.state {
                    OrderState::Buy => bid,
                    OrderState::Sell => ask,
                };

                let price = to_price(price);

                let amount = UnitAmount::new(token_info, *entry.token_amount.as_u64());

                let state_str = match entry.state {
                    OrderState::Buy => "Buy",
                    OrderState::Sell => "Sell",
                };

                println!(
                    "{:>4} {:>8} @ {:>15}",
                    state_str,
                    amount.to_string(),
                    price.indirect().to_string(),
                );
            }
            Ok(())
        }
        None => {
            println!("No grid order found");
            Ok(())
        }
    }
}

struct RedeemMultiData {
    orders: Vec<TrackedBox<MultiGridOrder>>,
    change_boxes: Vec<WalletBox<ErgoBoxAssetsData>>,
    fee_value: MinerFeeValue,
}

impl RedeemMultiData {
    pub fn into_described_tx(
        self,
        token_store: &TokenStore,
    ) -> anyhow::Result<SummarizedTransaction> {
        let creation_height = self
            .orders
            .iter()
            .map(|o| o.ergo_box.creation_height)
            .max()
            .unwrap_or(0);

        let inputs = self
            .orders
            .into_iter()
            .map(|i| SummarizedInput::new(i, token_store))
            .collect();

        let change_outputs = self
            .change_boxes
            .into_iter()
            .map(|o| SummarizedOutput::new(o, token_store, creation_height));

        let fee_output = SummarizedOutput::new(self.fee_value, token_store, creation_height)
            .expect("Fee output");

        let outputs: Result<Vec<_>, _> = change_outputs
            .chain(std::iter::once(Ok(fee_output)))
            .collect();

        Ok(SummarizedTransaction {
            inputs,
            outputs: outputs?,
        })
    }
}

fn build_redeem_multi_tx(
    orders: Vec<TrackedBox<MultiGridOrder>>,
    change_address: Address,
    fee_value: BoxValue,
) -> anyhow::Result<RedeemMultiData> {
    let change_value = orders
        .iter()
        .map(|o| o.ergo_box.value.as_u64())
        .sum::<u64>()
        .checked_sub(*fee_value.as_u64())
        .ok_or(anyhow!("Not enough funds for fee"))?;

    let mut change_tokens: HashMap<TokenId, TokenAmount> = HashMap::new();

    for order in orders.iter() {
        for token in order.ergo_box.tokens.as_ref().iter().flat_map(|b| b.iter()) {
            match change_tokens.entry(token.token_id) {
                Entry::Occupied(mut e) => {
                    let amount = e.get_mut();
                    *amount = amount.checked_add(&token.amount)?;
                }
                Entry::Vacant(e) => {
                    e.insert(token.amount);
                }
            }
        }
    }

    let tokens = if change_tokens.is_empty() {
        None
    } else {
        Some(
            change_tokens
                .into_iter()
                .map(Token::from)
                .collect::<Vec<_>>()
                .try_into()?,
        )
    };

    let change_asset_data = WalletBox::new(
        ErgoBoxAssetsData {
            value: change_value.try_into()?,
            tokens,
        },
        change_address,
    );

    Ok(RedeemMultiData {
        orders,
        change_boxes: vec![change_asset_data],
        fee_value: MinerFeeValue(fee_value),
    })
}