use ergo_lib::ergotree_ir::chain::{
    ergo_box::{box_value::BoxValueError, ErgoBoxCandidate},
    token::{Token, TokenAmountError, TokenId},
};
use itertools::Itertools;
use num_bigint::BigInt;
use std::ops::Deref;
use thiserror::Error;

use crate::grid::grid_order::{FillGridOrders, GridOrder, OrderState};

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

type OrderFill<T> = (T, i64, Token, Token);

fn filter_buy_orders<T: Deref<Target = GridOrder>, G: LiquidityProvider>(
    order: T,
    pool: &G,
    input_amount: u64,
    output_amount: u64,
) -> Option<OrderFill<T>> {
    (order.token.amount.as_u64() + output_amount)
        .try_into()
        .map(|output_amount| {
            let output_id = order.token.token_id.clone();
            (output_id, output_amount).into()
        })
        .ok()
        .and_then(|output| pool.input_amount(&output).map(|input| (input, output)).ok())
        .map(|(input, output)| {
            let surplus =
                order.bid_value() as i64 - (*input.amount.as_u64() as i64 - input_amount as i64);

            (order, surplus, input, output)
        })
}

fn filter_sell_orders<T: Deref<Target = GridOrder>, G: LiquidityProvider>(
    order: T,
    pool: &G,
    input_amount: u64,
    output_amount: u64,
) -> Option<OrderFill<T>> {
    (order.token.amount.as_u64() + input_amount)
        .try_into()
        .map(|input_amount| {
            let input_id = order.token.token_id.clone();
            (input_id, input_amount).into()
        })
        .ok()
        .and_then(|input| {
            pool.output_amount(&input)
                .map(|output| (input, output))
                .ok()
        })
        .map(|(input, output)| {
            let surplus =
                (*output.amount.as_u64() as i64 - output_amount as i64) - order.ask_value() as i64;

            (order, surplus, input, output)
        })
}

impl<T> FillGridOrders for T
where
    T: LiquidityProvider,
{
    type Error = LiquidityProviderError;

    fn fill_orders<G>(
        self,
        grid_orders: Vec<G>,
        order_state: OrderState,
    ) -> Result<(Self, Vec<(G, GridOrder)>), Self::Error>
    where
        G: Deref<Target = GridOrder>,
    {
        let mut orders = grid_orders;
        let mut filled_orders = vec![];
        let mut total_input_amount = 0;
        let mut total_output_amount = 0u64;

        orders.retain(|order| order.state == order_state);

        let filter_map_order = match order_state {
            OrderState::Buy => filter_buy_orders,
            OrderState::Sell => filter_sell_orders,
        };

        loop {
            let mut orders_with_surplus = orders
                .into_iter()
                .filter_map(|order| {
                    filter_map_order(order, &self, total_input_amount, total_output_amount)
                })
                .filter(|(_, surplus, _, _)| *surplus > 0)
                .sorted_unstable_by_key(|(_, surplus, _, _)| *surplus)
                .collect::<Vec<_>>();

            if let Some((order, _, input, output)) = orders_with_surplus.pop() {
                if let Ok(filled) = order.clone().into_filled() {
                    filled_orders.push((order, filled));
                    total_input_amount = *input.amount.as_u64();
                    total_output_amount = *output.amount.as_u64();
                }

                orders = orders_with_surplus
                    .into_iter()
                    .map(|(order, _, _, _)| order)
                    .collect();
            } else {
                break;
            }
        }

        let swapped = if total_input_amount > 0 {
            let input = match order_state {
                OrderState::Buy => (
                    self.asset_x().token_id.clone(),
                    // Safe to unwrap because we know the amount is greater than 0
                    total_input_amount.try_into().unwrap(),
                )
                    .into(),
                OrderState::Sell => (
                    self.asset_y().token_id.clone(),
                    total_input_amount.try_into().unwrap(),
                )
                    .into(),
            };

            self.with_swap(&input)?
        } else {
            self
        };

        Ok((swapped, filled_orders))
    }
}
