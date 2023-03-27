use crate::{matcher_config::MatcherConfig, scan_config::ScanConfig};
use clap::Args;
use ergo_lib::{
    chain::transaction::{Input, Transaction, TxId},
    ergotree_interpreter::sigma_protocol::prover::ProofBytes,
    ergotree_ir::{
        chain::{
            address::{AddressEncoder, NetworkPrefix},
            ergo_box::{BoxId, ErgoBox, ErgoBoxCandidate, NonMandatoryRegisters},
        },
        ergo_tree::ErgoTree,
    },
    wallet::miner_fee::MINERS_FEE_ADDRESS,
};
use itertools::Itertools;
use off_the_grid::{
    boxes::{liquidity_box::LiquidityProvider, tracked_box::TrackedBox},
    grid::grid_order::{FillGridOrders, GridOrder, MAX_FEE},
    node::client::NodeClient,
    spectrum::pool::SpectrumPool,
};
use std::{
    collections::{HashMap, HashSet},
    iter::once,
    time::Duration,
};
use tokio::try_join;

pub struct BoxIdGate {
    current_ids: HashSet<BoxId>,
}

impl BoxIdGate {
    pub fn new() -> Self {
        Self {
            current_ids: HashSet::new(),
        }
    }

    /// Returns true if there are new box ids and updates the current ids
    /// to the new ids.
    pub fn check_box_ids(&mut self, box_ids: &[BoxId]) -> Option<(Vec<BoxId>, Vec<BoxId>)> {
        let new_id_set: HashSet<_> = box_ids.iter().cloned().collect();
        // Only check for newly created boxes as spent boxes don't make a difference to
        // the order matching.
        let new_ids: Vec<_> = new_id_set.difference(&self.current_ids).cloned().collect();
        if new_ids.is_empty() {
            None
        } else {
            let spent_ids: Vec<_> = self.current_ids.difference(&new_id_set).cloned().collect();

            self.current_ids = new_id_set;
            Some((spent_ids, new_ids))
        }
    }
}


pub struct MempoolOverlay {
    spent_boxes: HashSet<BoxId>,
    created_boxes: HashMap<BoxId, ErgoBox>,
}

impl MempoolOverlay {
    pub fn add_transaction(&mut self, tx: Transaction) {
        for input in tx.inputs {
            self.spent_boxes.insert(input.box_id);
            self.created_boxes.remove(&input.box_id);
        }

        for ouput in tx.outputs {
            self.created_boxes.insert(ouput.box_id(), ouput);
        }
    }
}

// Workaround for scan APIs also returning spent boxes when including mempool.
// Assumes that the the transactions are ordered in a way that chained transactions
// appear after the transaction that created their inputs. This is the case for
// the reference node.
// https://github.com/ergoplatform/ergo/blob/1b0d72e09ebde8460a1a2d484e85a3d7f3271590/src/main/scala/org/ergoplatform/nodeView/mempool/ErgoMemPool.scala#L80
impl FromIterator<Transaction> for MempoolOverlay {
    fn from_iter<I: IntoIterator<Item = Transaction>>(iter: I) -> Self {
        let mut overlay = MempoolOverlay {
            spent_boxes: HashSet::new(),
            created_boxes: HashMap::new(),
        };

        for tx in iter {
            overlay.add_transaction(tx);
        }

        overlay
    }
}

pub struct MempoolOverlayIter<'a, I, J> {
    box_iter: I,
    overlay_created: J,
    overlay: &'a MempoolOverlay,
}

impl<'a, T, I, J> Iterator for MempoolOverlayIter<'a, I, J>
where
    I: Iterator<Item = TrackedBox<T>>,
    J: Iterator<Item = &'a ErgoBox>,
    TrackedBox<T>: TryFrom<&'a ErgoBox>,
{
    type Item = TrackedBox<T>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(b) = self.box_iter.next() {
                if !self.overlay.spent_boxes.contains(&b.ergo_box.box_id()) {
                    return Some(b);
                }
            } else if let Some(b) = self.overlay_created.next() {
                if let Ok(b) = b.try_into() {
                    return Some(b);
                }
            } else {
                return None;
            }
        }
    }
}

trait OverlayExt<T> {
    fn overlay(
        self,
        txs: &MempoolOverlay,
    ) -> MempoolOverlayIter<'_, Self, std::collections::hash_map::Values<'_, BoxId, ErgoBox>>
    where
        Self: Sized;
}

impl<T, E, I> OverlayExt<T> for I
where
    for<'a> T: TryFrom<&'a ErgoBox, Error = E>,
    I: Iterator<Item = TrackedBox<T>>,
{
    fn overlay(
        self,
        overlay: &MempoolOverlay,
    ) -> MempoolOverlayIter<'_, I, std::collections::hash_map::Values<'_, BoxId, ErgoBox>> {
        MempoolOverlayIter {
            box_iter: self,
            overlay_created: overlay.created_boxes.values(),
            overlay,
        }
    }
}

#[derive(Args)]
pub struct MatcherCommand {
    #[clap(long, help = "Scan configuration file path [default: scan_config]")]
    scan_config: Option<String>,
    #[clap(
        long,
        help = "Matcher configuration file path [default: matcher_config]"
    )]
    matcher_config: Option<String>,
}

pub async fn handle_matcher_command(
    node_client: NodeClient,
    matcher_command: MatcherCommand,
) -> anyhow::Result<()> {
    let scan_config = ScanConfig::try_create(matcher_command.scan_config, None)?;
    let matcher_config = MatcherConfig::try_create(matcher_command.matcher_config)?;
    let matcher_interval = Duration::from_secs_f64(matcher_config.matcher_interval.unwrap_or(10.0));
    let address_encoder = AddressEncoder::new(NetworkPrefix::Mainnet);

    let reward_address = match matcher_config.reward_address {
        Some(address) => address_encoder.parse_address_from_str(&address)?,
        None => {
            let wallet_status = node_client.wallet_status().await?;
            wallet_status.error_if_locked()?;
            wallet_status.change_address()?
        }
    };

    let reward_script = reward_address.script()?;

    println!(
        "Using reward address: {}",
        address_encoder.address_to_str(&reward_address)
    );

    let mut box_id_gate = BoxIdGate::new();

    loop {
        let (grid_orders, n2t_pools, mempool_txs) = try_join!(
            node_client.get_scan_unspent(scan_config.wallet_grid_scan_id),
            node_client.get_scan_unspent(scan_config.n2t_scan_id),
            node_client.transaction_unconfirmed_all(),
        )?;

        let overlay: MempoolOverlay = mempool_txs.into_iter().collect();

        let grid_orders: Vec<TrackedBox<GridOrder>> = grid_orders
            .into_iter()
            .filter_map(|b| b.try_into().ok())
            .overlay(&overlay)
            .collect();

        let n2t_pools: Vec<TrackedBox<SpectrumPool>> = n2t_pools
            .into_iter()
            .filter_map(|b| b.try_into().ok())
            .overlay(&overlay)
            .collect();

        if box_id_gate
            .check_box_ids(
                &grid_orders
                    .iter()
                    .map(|b| b.ergo_box.box_id())
                    .chain(n2t_pools.iter().map(|b| b.ergo_box.box_id()))
                    .collect::<Vec<_>>(),
            )
            .is_some()
        {
            let grouped_orders = grid_orders
                .into_iter()
                .into_group_map_by(|b| b.value.token.token_id);

            for (token_id, orders) in grouped_orders {
                let pool = n2t_pools
                    .iter()
                    .filter(|p| p.value.asset_y.token_id == token_id)
                    .max_by_key(|p| p.value.asset_x.amount.as_u64())
                    .cloned();

                if let Some(pool) = pool {
                    let result = match_orders(pool, orders, &reward_script, &node_client).await;

                    match result {
                        Ok(Some(tx_id)) => println!("Filled orders with tx {}", tx_id),
                        Err(e) => println!("Error filling orders: {}", e),
                        Ok(None) => (),
                    }
                }
            }
        } else {
            tokio::time::sleep(matcher_interval).await;
        }
    }
}

async fn match_orders(
    pool: TrackedBox<SpectrumPool>,
    orders: Vec<TrackedBox<GridOrder>>,
    change_script: &ErgoTree,
    node_client: &NodeClient,
) -> Result<Option<TxId>, anyhow::Error> {
    let (new_pool, filled) = pool.value.clone().fill_orders(orders)?;

    let input_value = filled
        .iter()
        .map(|(b, _)| b.value.value.as_i64())
        .sum::<i64>()
        + *pool.value.asset_x.amount.as_u64() as i64;

    let output_value = filled.iter().map(|(_, o)| o.value.as_i64()).sum::<i64>()
        + *new_pool.asset_x.amount.as_u64() as i64;

    let surplus = input_value - output_value;

    if !filled.is_empty() {
        if surplus > MAX_FEE as i64 {
            let creation_height = once(pool.ergo_box.creation_height)
                .chain(filled.iter().map(|(tb, _)| tb.ergo_box.creation_height))
                .max()
                .unwrap_or(0);

            let pool_input = Input::from_unsigned_input(pool.ergo_box.into(), ProofBytes::Empty);

            let pool_candidate = new_pool.into_box_candidate(creation_height)?;

            let (order_inputs, order_outputs): (Vec<Input>, Vec<ErgoBoxCandidate>) = filled
                .into_iter()
                .map(|(tb, order)| {
                    let input = Input::from_unsigned_input(tb.ergo_box.into(), ProofBytes::Empty);
                    (input, order.into_box_candidate(creation_height).unwrap())
                })
                .unzip();

            let change_candidate = ErgoBoxCandidate {
                value: (surplus - MAX_FEE as i64).try_into()?,
                ergo_tree: change_script.clone(),
                tokens: None,
                additional_registers: NonMandatoryRegisters::empty(),
                creation_height,
            };

            let fee_candidate = ErgoBoxCandidate {
                value: MAX_FEE.try_into().unwrap(),
                ergo_tree: MINERS_FEE_ADDRESS.script()?,
                tokens: None,
                additional_registers: NonMandatoryRegisters::empty(),
                creation_height,
            };

            let tx = Transaction::new_from_vec(
                once(pool_input).chain(order_inputs).collect(),
                vec![],
                once(pool_candidate)
                    .chain(order_outputs)
                    .chain(once(change_candidate))
                    .chain(once(fee_candidate))
                    .collect(),
            )?;

            let tx_id = node_client.transaction_submit(&tx).await?;

            Ok(Some(tx_id))
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
    }
}
