use ergo_lib::ergotree_ir::chain::{
    ergo_box::{box_value::BoxValueError, ErgoBoxCandidate},
    token::{Token, TokenAmountError, TokenId},
};
use num_bigint::BigInt;
use std::{cmp::Ordering, ops::Deref};
use thiserror::Error;

use crate::grid::multigrid_order::{
    FillMultiGridOrders, GridOrderEntries, GridOrderEntry, MultiGridOrder, OrderState,
};

#[derive(Error, Debug)]
pub enum LiquidityProviderError {
    #[error("Insufficient liquidity to perform swap")]
    InsufficientLiquidity,
    #[error("Liquidity box does not contain token {0:?}")]
    MissingToken(TokenId),
    #[error(transparent)]
    BoxValueError(#[from] BoxValueError),
    #[error(transparent)]
    TokenAmountError(#[from] TokenAmountError),
    #[error("Cannot convert {0} to u64")]
    BigIntTruncated(BigInt),
    #[error("{0}")]
    Other(String),
}

/// Trait for boxes that can be used to swap tokens
pub trait LiquidityProvider: Sized + Clone {
    fn can_swap(&self, token_id: &TokenId) -> bool;

    fn with_swap(self, input: &Token) -> Result<Self, LiquidityProviderError>;

    fn into_box_candidate(
        self,
        creation_height: u32,
    ) -> Result<ErgoBoxCandidate, LiquidityProviderError>;

    fn output_amount(&self, input: &Token) -> Result<Token, LiquidityProviderError>;

    fn input_amount(&self, output: &Token) -> Result<Token, LiquidityProviderError>;

    fn asset_x(&self) -> &Token;

    fn asset_y(&self) -> &Token;
}

pub enum OrderMatchingState {
    NotMatched(GridOrderEntries),
    MatchedBid(GridOrderEntries),
    MatchedAsk(GridOrderEntries),
}

impl OrderMatchingState {
    fn entries_mut(&mut self) -> &mut GridOrderEntries {
        match self {
            OrderMatchingState::NotMatched(entries) => entries,
            OrderMatchingState::MatchedBid(entries) => entries,
            OrderMatchingState::MatchedAsk(entries) => entries,
        }
    }
}

impl<T> FillMultiGridOrders for T
where
    T: LiquidityProvider,
{
    type Error = LiquidityProviderError;

    fn fill_orders<G>(
        self,
        grid_orders: Vec<G>,
    ) -> Result<(Self, Vec<(G, MultiGridOrder)>), Self::Error>
    where
        G: Deref<Target = MultiGridOrder>,
    {
        let mut orders: Vec<_> = grid_orders
            .into_iter()
            .map(|order| {
                let entries = order.entries.clone();
                (order, OrderMatchingState::NotMatched(entries))
            })
            .collect();

        let mut liquidity_x_diff = 0i64;
        let mut liquidity_y_diff = 0i64;
        let mut current_surplus = 0i64;

        let calculate_surplus = |entry: GridOrderEntry, cur_x: i64, cur_y: i64| {
            let (new_x, new_y) = match entry.state {
                OrderState::Buy => (
                    cur_x + entry.bid_value as i64,
                    cur_y - *entry.token_amount.as_u64() as i64,
                ),
                OrderState::Sell => (
                    cur_x - entry.ask_value as i64,
                    cur_y + *entry.token_amount.as_u64() as i64,
                ),
            };

            match new_y.cmp(&0) {
                Ordering::Greater => {
                    let input =
                        (self.asset_y().token_id, (new_y as u64).try_into().unwrap()).into();
                    if let Ok(output) = self.output_amount(&input) {
                        let surplus = new_x + *output.amount.as_u64() as i64;

                        Some((entry.state, new_x, new_y, surplus))
                    } else {
                        None
                    }
                }
                Ordering::Less => {
                    let output = (
                        self.asset_y().token_id,
                        new_y.unsigned_abs().try_into().unwrap(),
                    )
                        .into();
                    if let Ok(input) = self.input_amount(&output) {
                        let surplus = new_x - *input.amount.as_u64() as i64;

                        Some((entry.state, new_x, new_y, surplus))
                    } else {
                        None
                    }
                }
                Ordering::Equal => Some((entry.state, new_x, new_y, new_x)),
            }
        };

        loop {
            let best_order = orders
                .iter_mut()
                .filter_map(|(_, state)| match state {
                    OrderMatchingState::NotMatched(entries) => {
                        let bid_surplus = entries.bid_entry_mut().copied().and_then(|entry| {
                            calculate_surplus(entry, liquidity_x_diff, liquidity_y_diff)
                        });

                        let ask_surplus = entries.ask_entry_mut().copied().and_then(|entry| {
                            calculate_surplus(entry, liquidity_x_diff, liquidity_y_diff)
                        });

                        match (bid_surplus, ask_surplus) {
                            (None, None) => None,
                            (None, Some(ask)) => Some((state, ask)),
                            (Some(bid), None) => Some((state, bid)),
                            (Some(ask), Some(bid)) => {
                                if ask.2 > bid.2 {
                                    Some((state, ask))
                                } else {
                                    Some((state, bid))
                                }
                            }
                        }
                    }
                    OrderMatchingState::MatchedBid(entries) => entries
                        .bid_entry_mut()
                        .copied()
                        .and_then(|entry| {
                            calculate_surplus(entry, liquidity_x_diff, liquidity_y_diff)
                        })
                        .map(|bid| (state, bid)),
                    OrderMatchingState::MatchedAsk(entries) => entries
                        .ask_entry_mut()
                        .copied()
                        .and_then(|entry| {
                            calculate_surplus(entry, liquidity_x_diff, liquidity_y_diff)
                        })
                        .map(|ask| (state, ask)),
                })
                .max_by_key(|(_, (_, _, _, surplus))| *surplus);

            match best_order {
                Some((state, (order_state, new_x, new_y, surplus)))
                    if surplus > current_surplus =>
                {
                    liquidity_x_diff = new_x;
                    liquidity_y_diff = new_y;
                    current_surplus = surplus;

                    match order_state {
                        OrderState::Buy => {
                            state.entries_mut().bid_entry_mut().unwrap().state = OrderState::Sell;
                            *state = OrderMatchingState::MatchedBid(state.entries_mut().clone());
                        }
                        OrderState::Sell => {
                            state.entries_mut().ask_entry_mut().unwrap().state = OrderState::Buy;
                            *state = OrderMatchingState::MatchedAsk(state.entries_mut().clone());
                        }
                    }
                }
                _ => break,
            }
        }

        let filled_orders: Vec<_> = orders
            .into_iter()
            .filter_map(|(order, state)| {
                match state {
                    OrderMatchingState::NotMatched(_) => None,
                    OrderMatchingState::MatchedBid(entries) => Some(entries),
                    OrderMatchingState::MatchedAsk(entries) => Some(entries),
                }
                .and_then(|entries| order.clone().with_entries(entries).ok())
                .map(|filled| (order, filled))
            })
            .collect();

        match liquidity_y_diff.cmp(&0) {
            Ordering::Greater => {
                let input = (
                    self.asset_y().token_id,
                    (liquidity_y_diff as u64).try_into().unwrap(),
                )
                    .into();

                let swapped = self.with_swap(&input)?;
                Ok((swapped, filled_orders))
            }
            Ordering::Less => {
                let output = (
                    self.asset_y().token_id,
                    liquidity_y_diff.unsigned_abs().try_into().unwrap(),
                )
                    .into();

                let input = self.input_amount(&output)?;
                let swapped = self.with_swap(&input)?;
                Ok((swapped, filled_orders))
            }
            Ordering::Equal => Ok((self, filled_orders)),
        }
    }
}
