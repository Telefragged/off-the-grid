use std::collections::HashMap;

use ergo_lib::{
    ergo_chain_types::Digest32,
    ergotree_ir::{
        chain::{
            ergo_box::{
                box_value::BoxValueError, BoxId, ErgoBox, ErgoBoxCandidate, NonMandatoryRegisterId,
            },
            token::{Token, TokenAmount, TokenAmountError, TokenId},
        },
        mir::constant::{Constant, TryExtractInto},
    },
};
use lazy_static::lazy_static;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use thiserror::Error;

lazy_static! {
    /// Spectrum ERG token id
    pub static ref ERG_TOKEN_ID: TokenId =
        TokenId::from(Digest32::zero());
}

#[derive(Error, Debug)]
pub enum SpectrumPoolError {
    #[error("Box parsing failed {0:?}")]
    BoxParseFailure(BoxId),

    #[error("Cannot convert {0} to u64")]
    BigIntTruncated(BigInt),

    #[error(transparent)]
    TokenAmountError(#[from] TokenAmountError),

    #[error(transparent)]
    BoxValueError(#[from] BoxValueError),
}

#[derive(Clone)]
pub struct SpectrumPool {
    pub pool_nft: Token,
    pub asset_lp: Token,
    pub asset_x: Token,
    pub asset_y: Token,
    pub fee_num: i32,
    pub fee_denom: i32,
    pub pool_box: ErgoBox,
}

impl SpectrumPool {
    pub fn pure_price(&self) -> u64 {
        let x_amount = *self.asset_x.amount.as_u64();
        let y_amount = *self.asset_y.amount.as_u64();

        x_amount / y_amount
    }

    pub fn amm_factor(&self) -> BigInt {
        let x_amount: BigInt = (*self.asset_x.amount.as_u64()).into();
        let y_amount: BigInt = (*self.asset_y.amount.as_u64()).into();

        x_amount * y_amount
    }

    pub fn output_amount(&self, input: &Token) -> Result<Token, SpectrumPoolError> {
        let (from, to) = if input.token_id == self.asset_x.token_id {
            (&self.asset_x, &self.asset_y)
        } else {
            (&self.asset_y, &self.asset_x)
        };
        let from_amount = BigInt::from(*from.amount.as_u64());
        let to_amount = BigInt::from(*to.amount.as_u64());
        let input_amount = &BigInt::from(*input.amount.as_u64());

        let output_amount = (to_amount * input_amount * self.fee_num)
            / (from_amount * self.fee_denom + input_amount * self.fee_num);

        let token_amount: TokenAmount = output_amount
            .to_u64()
            .ok_or_else(|| SpectrumPoolError::BigIntTruncated(output_amount))?
            .try_into()?;

        Ok((to.token_id.clone(), token_amount).into())
    }

    pub fn input_amount(&self, output: &Token) -> Result<Token, SpectrumPoolError> {
        let (from, to) = if output.token_id == self.asset_y.token_id {
            (&self.asset_x, &self.asset_y)
        } else {
            (&self.asset_y, &self.asset_x)
        };
        let from_amount = BigInt::from(*from.amount.as_u64());
        let to_amount = BigInt::from(*to.amount.as_u64());
        let output_amount = &BigInt::from(*output.amount.as_u64());

        let input_amount: BigInt = (from_amount * output_amount * self.fee_denom)
            / ((to_amount - output_amount) * self.fee_num)
            + 1;

        let token_amount: TokenAmount = input_amount
            .to_u64()
            .ok_or_else(|| SpectrumPoolError::BigIntTruncated(input_amount))?
            .try_into()?;

        Ok((from.token_id.clone(), token_amount).into())
    }

    pub fn with_swap(self, input: &Token) -> Result<Self, SpectrumPoolError> {
        let output = self.output_amount(input)?;

        let (x_amount, y_amount): (TokenAmount, TokenAmount) =
            if input.token_id == self.asset_x.token_id {
                (
                    self.asset_x.amount.checked_add(&input.amount)?,
                    self.asset_y.amount.checked_sub(&output.amount)?,
                )
            } else {
                (
                    self.asset_x.amount.checked_sub(&input.amount)?,
                    self.asset_y.amount.checked_add(&output.amount)?,
                )
            };

        let asset_x = (self.asset_x.token_id, x_amount).into();
        let asset_y = (self.asset_y.token_id, y_amount).into();

        Ok(Self {
            asset_x,
            asset_y,
            ..self
        })
    }

    pub fn to_box_candidate(
        &self,
        creation_height: u32,
    ) -> Result<ErgoBoxCandidate, SpectrumPoolError> {
        let registers: HashMap<NonMandatoryRegisterId, Constant> =
            HashMap::from([(NonMandatoryRegisterId::R4, self.fee_num.into())]);

        let tokens = Some(
            vec![
                self.pool_nft.clone(),
                self.asset_lp.clone(),
                self.asset_y.clone(),
            ]
            .try_into()
            .unwrap(),
        );

        let value = (*self.asset_x.amount.as_u64()).try_into()?;

        Ok(ErgoBoxCandidate {
            value,
            ergo_tree: self.pool_box.ergo_tree.clone(),
            tokens,
            additional_registers: registers.try_into().unwrap(),
            creation_height,
        })
    }
}

impl TryFrom<&ErgoBox> for SpectrumPool {
    type Error = SpectrumPoolError;

    fn try_from(pool_box: &ErgoBox) -> Result<Self, Self::Error> {
        let fee_value = pool_box
            .additional_registers
            .get(NonMandatoryRegisterId::R4)
            .and_then(|x| x.clone().try_extract_into::<i32>().ok());

        let tokens = pool_box.tokens.as_ref().map(|v| v.as_slice());

        match (tokens, fee_value) {
            (Some([pool_nft, pool_lp, pool_y]), Some(fee)) => {
                let x_amount = TokenAmount::try_from(*pool_box.value.as_u64()).unwrap();
                let pool = Self {
                    pool_nft: pool_nft.clone(),
                    asset_lp: pool_lp.clone(),
                    asset_x: (ERG_TOKEN_ID.clone(), x_amount).into(),
                    asset_y: pool_y.clone(),
                    fee_num: fee,
                    fee_denom: 1000,
                    pool_box: pool_box.clone(),
                };
                Ok(pool)
            }
            _ => Err(SpectrumPoolError::BoxParseFailure(pool_box.box_id())),
        }
    }
}
