use std::collections::HashMap;

use ergo_lib::{
    ergo_chain_types::Digest32,
    ergotree_ir::{
        chain::{
            address::Address,
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

use crate::boxes::liquidity_box::{LiquidityProviderError, LiquidityProvider};

const N2T_POOL_ERGO_TREE_BASE16: &str = "1999030f0400040204020404040405feffffffffffffffff0105feffffffffffffffff01050004d00f040004000406050005000580dac409d819d601b2a5730000d602e4c6a70404d603db63087201d604db6308a7d605b27203730100d606b27204730200d607b27203730300d608b27204730400d6099973058c720602d60a999973068c7205027209d60bc17201d60cc1a7d60d99720b720cd60e91720d7307d60f8c720802d6107e720f06d6117e720d06d612998c720702720fd6137e720c06d6147308d6157e721206d6167e720a06d6177e720906d6189c72117217d6199c72157217d1ededededededed93c27201c2a793e4c672010404720293b27203730900b27204730a00938c7205018c720601938c7207018c72080193b17203730b9593720a730c95720e929c9c721072117e7202069c7ef07212069a9c72137e7214067e9c720d7e72020506929c9c721372157e7202069c7ef0720d069a9c72107e7214067e9c72127e7202050695ed720e917212730d907216a19d721872139d72197210ed9272189c721672139272199c7216721091720b730e";

lazy_static! {
    /// Spectrum ERG token id
    pub static ref ERG_TOKEN_ID: TokenId =
        TokenId::from(Digest32::zero());

    pub static ref N2T_POOL_ADDRESS: Address =
        #[allow(clippy::unwrap_used)]
        Address::P2S(base16::decode(N2T_POOL_ERGO_TREE_BASE16).unwrap());
}

#[derive(Clone)]
pub enum PoolType {
    N2T,
}

#[derive(Error, Debug)]
pub enum SpectrumSwapError {
    #[error("Cannot convert {0} to u64")]
    BigIntTruncated(BigInt),
    #[error(transparent)]
    TokenAmountError(#[from] TokenAmountError),
    #[error("Cannot swap token {0:?}")]
    InvalidToken(TokenId),
}

impl From<SpectrumSwapError> for LiquidityProviderError {
    fn from(e: SpectrumSwapError) -> Self {
        match e {
            SpectrumSwapError::BigIntTruncated(_) => LiquidityProviderError::InsufficientLiquidity,
            SpectrumSwapError::TokenAmountError(_) => LiquidityProviderError::Other(e.to_string()),
            SpectrumSwapError::InvalidToken(token_id) => {
                LiquidityProviderError::MissingToken(token_id)
            }
        }
    }
}

#[derive(Error, Debug)]
pub enum SpectrumPoolError {
    #[error("Box parsing failed {0:?}")]
    BoxParseFailure(BoxId),
    #[error(transparent)]
    BoxValueError(#[from] BoxValueError),
    #[error(transparent)]
    TokenAmountError(#[from] TokenAmountError),
}

#[derive(Clone)]
pub struct SpectrumPool {
    pub pool_nft: Token,
    pub asset_lp: Token,
    pub asset_x: Token,
    pub asset_y: Token,
    pub fee_num: i32,
    pub fee_denom: i32,
    pub pool_type: PoolType,
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
                let x_amount = TokenAmount::try_from(*pool_box.value.as_u64())?;
                let pool = Self {
                    pool_nft: pool_nft.clone(),
                    asset_lp: pool_lp.clone(),
                    asset_x: (ERG_TOKEN_ID.clone(), x_amount).into(),
                    asset_y: pool_y.clone(),
                    fee_num: fee,
                    fee_denom: 1000,
                    pool_type: PoolType::N2T,
                };
                Ok(pool)
            }
            _ => Err(SpectrumPoolError::BoxParseFailure(pool_box.box_id())),
        }
    }
}

impl LiquidityProvider for SpectrumPool {
    fn can_swap(&self, token_id: &TokenId) -> bool {
        token_id == &self.asset_x.token_id || token_id == &self.asset_y.token_id
    }

    fn with_swap(self, input: &Token) -> Result<Self, LiquidityProviderError> {
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

    fn output_amount(&self, input: &Token) -> Result<Token, LiquidityProviderError> {
        let (from, to) = if input.token_id == self.asset_x.token_id {
            Ok((&self.asset_x, &self.asset_y))
        } else if input.token_id == self.asset_y.token_id {
            Ok((&self.asset_y, &self.asset_x))
        } else {
            Err(SpectrumSwapError::InvalidToken(input.token_id.clone()))
        }?;
        let from_amount = BigInt::from(*from.amount.as_u64());
        let to_amount = BigInt::from(*to.amount.as_u64());
        let input_amount = &BigInt::from(*input.amount.as_u64());

        let output_amount = (to_amount * input_amount * self.fee_num)
            / (from_amount * self.fee_denom + input_amount * self.fee_num);

        let token_amount: TokenAmount = output_amount
            .to_u64()
            .ok_or_else(|| SpectrumSwapError::BigIntTruncated(output_amount))?
            .try_into()?;

        Ok((to.token_id.clone(), token_amount).into())
    }

    fn input_amount(&self, output: &Token) -> Result<Token, LiquidityProviderError> {
        let (from, to) = if output.token_id == self.asset_y.token_id {
            Ok((&self.asset_x, &self.asset_y))
        } else if output.token_id == self.asset_x.token_id {
            Ok((&self.asset_y, &self.asset_x))
        } else {
            Err(SpectrumSwapError::InvalidToken(output.token_id.clone()))
        }?;
        let from_amount = BigInt::from(*from.amount.as_u64());
        let to_amount = BigInt::from(*to.amount.as_u64());
        let output_amount = &BigInt::from(*output.amount.as_u64());

        let input_amount: BigInt = (from_amount * output_amount * self.fee_denom)
            / ((to_amount - output_amount) * self.fee_num)
            + 1;

        let token_amount: TokenAmount = input_amount
            .to_u64()
            .ok_or_else(|| LiquidityProviderError::BigIntTruncated(input_amount))?
            .try_into()?;

        Ok((from.token_id.clone(), token_amount).into())
    }

    fn into_box_candidate(
        self,
        creation_height: u32,
    ) -> Result<ErgoBoxCandidate, LiquidityProviderError> {
        let registers: HashMap<NonMandatoryRegisterId, Constant> =
            HashMap::from([(NonMandatoryRegisterId::R4, self.fee_num.into())]);

        let tokens = Some(
            vec![self.pool_nft, self.asset_lp, self.asset_y]
                .try_into()
                // Safe to unwrap because we know the vector has 3 elements
                .unwrap(),
        );

        let value = (*self.asset_x.amount.as_u64()).try_into()?;

        let ergo_tree = match self.pool_type {
            PoolType::N2T => N2T_POOL_ADDRESS.script().unwrap(),
        };

        Ok(ErgoBoxCandidate {
            value,
            ergo_tree,
            tokens,
            // Safe to unwrap because we know the hashmap conforms to the
            // register requirements
            additional_registers: registers.try_into().unwrap(),
            creation_height,
        })
    }

    fn asset_x(&self) -> &Token {
        &self.asset_x
    }

    fn asset_y(&self) -> &Token {
        &self.asset_y
    }
}
