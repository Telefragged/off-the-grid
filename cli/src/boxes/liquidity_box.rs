use ergo_lib::ergotree_ir::chain::{
    ergo_box::{box_value::BoxValueError, ErgoBoxCandidate},
    token::{Token, TokenAmountError, TokenId},
};
use itertools::Itertools;
use num_bigint::BigInt;
use std::{cmp::Ordering, ops::Deref};
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

impl<T> FillGridOrders for T
where
    T: LiquidityProvider,
{
    type Error = LiquidityProviderError;

    fn fill_orders<G>(self, grid_orders: Vec<G>) -> Result<(Self, Vec<(G, GridOrder)>), Self::Error>
    where
        G: Deref<Target = GridOrder>,
    {
        let mut orders = grid_orders;
        let mut filled_orders = vec![];
        let mut liquidity_x_diff = 0i64;
        let mut liquidity_y_diff = 0i64;
        let mut current_surplus = 0i64;

        loop {
            let mut orders_with_surplus = orders
                .into_iter()
                .map(|order| {
                    let (new_x, new_y) = match order.state {
                        OrderState::Buy => (
                            liquidity_x_diff + order.bid_value() as i64,
                            liquidity_y_diff - *order.token.amount.as_u64() as i64,
                        ),
                        OrderState::Sell => (
                            liquidity_x_diff - order.ask_value() as i64,
                            liquidity_y_diff + *order.token.amount.as_u64() as i64,
                        ),
                    };
                    match new_y.cmp(&0) {
                        Ordering::Greater => {
                            let input =
                                (self.asset_y().token_id, (new_y as u64).try_into().unwrap())
                                    .into();
                            if let Ok(output) = self.output_amount(&input) {
                                let surplus = new_x + *output.amount.as_u64() as i64;

                                Ok((order, new_x, new_y, surplus))
                            } else {
                                Err(order)
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

                                Ok((order, new_x, new_y, surplus))
                            } else {
                                Err(order)
                            }
                        }
                        Ordering::Equal => Ok((order, new_x, new_y, new_x)),
                    }
                })
                // Put successful orders with the highest surplus at the end
                .sorted_unstable_by(|lhs, rhs| match (lhs, rhs) {
                    (Ok(_), Err(_)) => Ordering::Greater,
                    (Err(_), Ok(_)) => Ordering::Less,
                    (Ok((_, _, _, lhs)), Ok((_, _, _, rhs))) => lhs.cmp(rhs),
                    (Err(_), Err(_)) => Ordering::Equal,
                })
                .collect::<Vec<_>>();

            match orders_with_surplus.last() {
                Some(Ok((_, _, _, surplus))) if *surplus > current_surplus => {
                    let Ok((order, new_x, new_y, surplus)) =
                        orders_with_surplus.pop().unwrap() else {
                            panic!("Unreachable")
                        };
                    if let Ok(filled) = order.clone().into_filled() {
                        liquidity_x_diff = new_x;
                        liquidity_y_diff = new_y;
                        current_surplus = surplus;
                        filled_orders.push((order, filled));
                    }

                    orders = orders_with_surplus
                        .into_iter()
                        .map(|choice| match choice {
                            Ok((order, _, _, _)) => order,
                            Err(order) => order,
                        })
                        .collect();
                }
                _ => break,
            }
        }

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
