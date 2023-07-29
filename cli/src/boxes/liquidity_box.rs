use ergo_lib::ergotree_ir::chain::{
    ergo_box::{box_value::BoxValueError, ErgoBoxCandidate},
    token::{Token, TokenAmountError, TokenId},
};
use num_bigint::BigInt;
use std::cmp::Ordering;
use thiserror::Error;

use crate::grid::multigrid_order::{
    FillMultiGridOrders, GridOrderEntries, GridOrderEntry, MultiGridOrder, MultiGridRef, OrderState,
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

/// The state of the order matching process for a specific multi-grid order
/// This is needed since the order matching process can only occur in one direction
enum OrderMatchingState<'a> {
    /// The order has not been matched yet, meaning that the bid and ask are still available
    NotMatched(&'a GridOrderEntries),
    /// The bid has been matched, meaning that the order can only be filled by more buys
    MatchedBid(GridOrderEntries),
    /// The ask has been matched, meaning that the order can only be filled by more sells
    MatchedAsk(GridOrderEntries),
}

impl OrderMatchingState<'_> {
    fn fill_bid(&mut self) {
        match self {
            OrderMatchingState::NotMatched(entries) => {
                let mut entries = entries.clone();
                entries.bid_entry_mut().unwrap().state = OrderState::Sell;
                *self = OrderMatchingState::MatchedBid(entries);
            }
            OrderMatchingState::MatchedBid(entries) => {
                entries.bid_entry_mut().unwrap().state = OrderState::Sell;
            }
            OrderMatchingState::MatchedAsk(_) => {
                panic!("Cannot fill bid when ask is already filled");
            }
        }
    }

    fn fill_ask(&mut self) {
        match self {
            OrderMatchingState::NotMatched(entries) => {
                let mut entries = entries.clone();
                entries.ask_entry_mut().unwrap().state = OrderState::Buy;
                *self = OrderMatchingState::MatchedAsk(entries);
            }
            OrderMatchingState::MatchedBid(_) => {
                panic!("Cannot fill ask when bid is already filled");
            }
            OrderMatchingState::MatchedAsk(entries) => {
                entries.ask_entry_mut().unwrap().state = OrderState::Buy;
            }
        }
    }

    fn state_surplus<T>(
        &self,
        liquidity_provider: &T,
        cur_x: i64,
        cur_y: i64,
    ) -> Option<SurplusResult>
    where
        T: LiquidityProvider,
    {
        match self {
            OrderMatchingState::NotMatched(entries) => {
                let bid_surplus = entries
                    .bid_entry()
                    .and_then(|entry| calculate_surplus(liquidity_provider, entry, cur_x, cur_y));

                let ask_surplus = entries
                    .ask_entry()
                    .and_then(|entry| calculate_surplus(liquidity_provider, entry, cur_x, cur_y));

                match (bid_surplus, ask_surplus) {
                    (None, None) => None,
                    (None, Some(ask)) => Some(ask),
                    (Some(bid), None) => Some(bid),
                    (Some(ask), Some(bid)) => {
                        if ask.surplus > bid.surplus {
                            Some(ask)
                        } else {
                            Some(bid)
                        }
                    }
                }
            }
            OrderMatchingState::MatchedBid(entries) => entries
                .bid_entry()
                .and_then(|entry| calculate_surplus(liquidity_provider, entry, cur_x, cur_y)),
            OrderMatchingState::MatchedAsk(entries) => entries
                .ask_entry()
                .and_then(|entry| calculate_surplus(liquidity_provider, entry, cur_x, cur_y)),
        }
    }
}

struct SurplusResult {
    matched_state: OrderState,
    new_x: i64,
    new_y: i64,
    surplus: i64,
}

impl SurplusResult {
    fn new(matched_state: OrderState, new_x: i64, new_y: i64, surplus: i64) -> Self {
        SurplusResult {
            matched_state,
            new_x,
            new_y,
            surplus,
        }
    }
}

fn calculate_surplus<T>(
    liquidity_provider: &T,
    entry: &GridOrderEntry,
    cur_x: i64,
    cur_y: i64,
) -> Option<SurplusResult>
where
    T: LiquidityProvider,
{
    let (new_x, new_y) = match entry.state {
        OrderState::Buy => (
            cur_x.checked_add(entry.bid_value as i64)?,
            cur_y.checked_sub(*entry.token_amount.as_u64() as i64)?,
        ),
        OrderState::Sell => (
            cur_x.checked_sub(entry.ask_value as i64)?,
            cur_y.checked_add(*entry.token_amount.as_u64() as i64)?,
        ),
    };

    match new_y.cmp(&0) {
        Ordering::Greater => {
            let input = (
                liquidity_provider.asset_y().token_id,
                (new_y as u64).try_into().expect("non-zero"),
            )
                .into();

            let output = liquidity_provider.output_amount(&input).ok()?;
            let surplus = new_x.checked_add(*output.amount.as_u64() as i64)?;

            Some(SurplusResult::new(entry.state, new_x, new_y, surplus))
        }
        Ordering::Less => {
            let output = (
                liquidity_provider.asset_y().token_id,
                new_y.unsigned_abs().try_into().expect("non-zero"),
            )
                .into();

            let input = liquidity_provider.input_amount(&output).ok()?;
            let surplus = new_x.checked_sub(*input.amount.as_u64() as i64)?;

            Some(SurplusResult::new(entry.state, new_x, new_y, surplus))
        }
        Ordering::Equal => Some(SurplusResult::new(entry.state, new_x, new_y, new_x)),
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
        G: MultiGridRef,
    {
        let mut matched_states: Vec<_> = grid_orders
            .iter()
            .map(|order| OrderMatchingState::NotMatched(&order.order_ref().entries))
            .collect();

        let mut liquidity_x_diff = 0i64;
        let mut liquidity_y_diff = 0i64;
        let mut current_surplus = 0i64;

        loop {
            let best_order = matched_states
                .iter_mut()
                .filter_map(|state| {
                    state
                        .state_surplus(&self, liquidity_x_diff, liquidity_y_diff)
                        .map(|surplus_result| (state, surplus_result))
                })
                // Ensure that orders which have enough liquidity to overflow the liquidity
                // provider swap does not prevent other orders from being matched.
                // It is in practice impossible to make these as there can never be more than
                // i64::MAX tokens for any single asset, but a malicious user might try to
                // configure the grid to make this happen.
                .filter(|(_, surplus_result)| {
                    let y_overflow = (*self.asset_y().amount.as_u64() as i64)
                        .checked_add(surplus_result.new_y)
                        .is_none();

                    let x_overflow = (*self.asset_x().amount.as_u64() as i64)
                        .checked_add(surplus_result.new_x)
                        .is_none();

                    !y_overflow && !x_overflow
                })
                .max_by_key(|(_, surplus_result)| surplus_result.surplus);

            match best_order {
                Some((state, surplus_result)) if surplus_result.surplus > current_surplus => {
                    liquidity_x_diff = surplus_result.new_x;
                    liquidity_y_diff = surplus_result.new_y;
                    current_surplus = surplus_result.surplus;

                    match surplus_result.matched_state {
                        OrderState::Buy => {
                            state.fill_bid();
                        }
                        OrderState::Sell => {
                            state.fill_ask();
                        }
                    }
                }
                _ => break,
            }
        }

        let new_states: Vec<_> = matched_states
            .into_iter()
            .map(|state| match state {
                OrderMatchingState::NotMatched(_) => None,
                OrderMatchingState::MatchedBid(entries) => Some(entries),
                OrderMatchingState::MatchedAsk(entries) => Some(entries),
            })
            .collect();

        let filled_orders = new_states
            .into_iter()
            .zip(grid_orders)
            .filter_map(|(entries, order)| {
                entries
                    .and_then(|entries| order.order_ref().to_owned().with_entries(entries).ok())
                    .map(|filled| (order, filled))
            })
            .collect();

        match liquidity_y_diff.cmp(&0) {
            Ordering::Greater => {
                let input = (
                    self.asset_y().token_id,
                    (liquidity_y_diff as u64).try_into().expect("non-zero"),
                )
                    .into();

                let swapped = self.with_swap(&input)?;
                Ok((swapped, filled_orders))
            }
            Ordering::Less => {
                let output = (
                    self.asset_y().token_id,
                    liquidity_y_diff
                        .unsigned_abs()
                        .try_into()
                        .expect("non-zero"),
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
