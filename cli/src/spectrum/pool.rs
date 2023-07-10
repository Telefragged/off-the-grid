use std::collections::HashMap;

use ergo_lib::{
    ergo_chain_types::Digest32,
    ergotree_ir::{
        chain::{
            address::Address,
            ergo_box::{
                box_value::BoxValueError, BoxId, ErgoBox, ErgoBoxCandidate, NonMandatoryRegisterId,
                NonMandatoryRegisters,
            },
            token::{Token, TokenAmount, TokenAmountError, TokenId},
        },
        ergo_tree::ErgoTree,
        mir::constant::{Constant, TryExtractInto},
    },
};
use lazy_static::lazy_static;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use thiserror::Error;

use crate::{
    boxes::{
        describe_box::{BoxAssetDisplay, ErgoBoxDescriptors},
        liquidity_box::{LiquidityProvider, LiquidityProviderError},
    },
    units::{TokenStore, UnitAmount, ERG_UNIT},
};

const N2T_POOL_ERGO_TREE_BASE16: &str = "1999030f0400040204020404040405feffffffffffffffff0105feffffffffffffffff01050004d00f040004000406050005000580dac409d819d601b2a5730000d602e4c6a70404d603db63087201d604db6308a7d605b27203730100d606b27204730200d607b27203730300d608b27204730400d6099973058c720602d60a999973068c7205027209d60bc17201d60cc1a7d60d99720b720cd60e91720d7307d60f8c720802d6107e720f06d6117e720d06d612998c720702720fd6137e720c06d6147308d6157e721206d6167e720a06d6177e720906d6189c72117217d6199c72157217d1ededededededed93c27201c2a793e4c672010404720293b27203730900b27204730a00938c7205018c720601938c7207018c72080193b17203730b9593720a730c95720e929c9c721072117e7202069c7ef07212069a9c72137e7214067e9c720d7e72020506929c9c721372157e7202069c7ef0720d069a9c72107e7214067e9c72127e7202050695ed720e917212730d907216a19d721872139d72197210ed9272189c721672139272199c7216721091720b730e";

lazy_static! {
    /// Spectrum ERG token id
    pub static ref ERG_TOKEN_ID: TokenId =
        TokenId::from(Digest32::zero());

    pub static ref N2T_POOL_ADDRESS: Address =
        Address::P2S(base16::decode(N2T_POOL_ERGO_TREE_BASE16).expect("String is a valid base16"));

    pub static ref N2T_POOL_SCRIPT: ErgoTree =
        N2T_POOL_ADDRESS.script().expect("Pool address is a valid script");
}

#[derive(Clone)]
pub enum PoolType {
    N2T,
}

impl PoolType {
    pub fn as_str(&self) -> &str {
        match self {
            PoolType::N2T => "N2T",
        }
    }
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
            .get_constant(NonMandatoryRegisterId::R4)
            .and_then(|x| x.clone().try_extract_into::<i32>().ok());

        let tokens = pool_box.tokens.as_ref().map(|v| v.as_slice());

        match (tokens, fee_value) {
            (Some([pool_nft, pool_lp, pool_y]), Some(fee))
                if pool_box.ergo_tree == *N2T_POOL_SCRIPT =>
            {
                let x_amount = TokenAmount::try_from(*pool_box.value.as_u64())?;
                let pool = Self {
                    pool_nft: pool_nft.clone(),
                    asset_lp: pool_lp.clone(),
                    asset_x: (*ERG_TOKEN_ID, x_amount).into(),
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
                    self.asset_x.amount.checked_sub(&output.amount)?,
                    self.asset_y.amount.checked_add(&input.amount)?,
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
            Err(SpectrumSwapError::InvalidToken(input.token_id))
        }?;
        let from_amount = BigInt::from(*from.amount.as_u64());
        let to_amount = BigInt::from(*to.amount.as_u64());
        let input_amount = &BigInt::from(*input.amount.as_u64());

        let output_amount = (to_amount * input_amount * self.fee_num)
            / (from_amount * self.fee_denom + input_amount * self.fee_num);

        let token_amount: TokenAmount = output_amount
            .to_u64()
            .ok_or(SpectrumSwapError::BigIntTruncated(output_amount))?
            .try_into()?;

        Ok((to.token_id, token_amount).into())
    }

    fn input_amount(&self, output: &Token) -> Result<Token, LiquidityProviderError> {
        let (from, to) = if output.token_id == self.asset_y.token_id {
            Ok((&self.asset_x, &self.asset_y))
        } else if output.token_id == self.asset_x.token_id {
            Ok((&self.asset_y, &self.asset_x))
        } else {
            Err(SpectrumSwapError::InvalidToken(output.token_id))
        }?;
        let from_amount = BigInt::from(*from.amount.as_u64());
        let to_amount = BigInt::from(*to.amount.as_u64());
        let output_amount = &BigInt::from(*output.amount.as_u64());

        let input_amount: BigInt = (from_amount * output_amount * self.fee_denom)
            / ((to_amount - output_amount) * self.fee_num)
            + 1;

        let token_amount: TokenAmount = input_amount
            .to_u64()
            .ok_or(LiquidityProviderError::BigIntTruncated(input_amount))?
            .try_into()?;

        Ok((from.token_id, token_amount).into())
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
                .expect("Token BoundedVec requires >1 tokens"),
        );

        let value = (*self.asset_x.amount.as_u64()).try_into()?;

        let ergo_tree = match self.pool_type {
            PoolType::N2T => N2T_POOL_SCRIPT.clone(),
        };

        Ok(ErgoBoxCandidate {
            value,
            ergo_tree,
            tokens,
            additional_registers: NonMandatoryRegisters::new(registers)
                .expect("Only R4 is used which is always tightly packed"),
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

impl ErgoBoxDescriptors for SpectrumPool {
    fn box_name(&self) -> String {
        format!("Spectrum {}", self.pool_type.as_str())
    }

    fn assets<'a>(&self, tokens: &'a TokenStore) -> BoxAssetDisplay<'a> {
        match self.pool_type {
            PoolType::N2T => {
                let x_unit = *ERG_UNIT;
                let y_unit = tokens.get_unit(&self.asset_y().token_id);

                let x_amount = UnitAmount::new(x_unit, *self.asset_x().amount.as_u64());
                let y_amount = UnitAmount::new(y_unit, *self.asset_y().amount.as_u64());

                BoxAssetDisplay::Double(x_amount, y_amount)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SpectrumPool;
    use crate::boxes::liquidity_box::LiquidityProvider;
    use ergo_lib::ergo_chain_types::Digest32;

    fn test_pool(x_amount: u64, y_amount: u64) -> SpectrumPool {
        let mut pool_nft_id = [0u8; 32];
        pool_nft_id[0] = 1;

        let mut asset_lp_id = [0u8; 32];
        asset_lp_id[0] = 2;

        let mut asset_y_id = [0u8; 32];
        asset_y_id[0] = 3;

        SpectrumPool {
            pool_nft: (Digest32::from(pool_nft_id).into(), 1.try_into().unwrap()).into(),
            asset_lp: (Digest32::from(asset_lp_id).into(), 1000.try_into().unwrap()).into(),
            asset_x: (Digest32::zero().into(), x_amount.try_into().unwrap()).into(),
            asset_y: (
                Digest32::from(asset_y_id).into(),
                y_amount.try_into().unwrap(),
            )
                .into(),
            fee_num: 998,
            fee_denom: 1000,
            pool_type: super::PoolType::N2T,
        }
    }

    #[test]
    fn swap_output() {
        let pool = test_pool(1000000000, 1000);

        let mut input = pool.asset_x.clone();
        input.amount = 500000000.try_into().unwrap();

        let output = pool.output_amount(&input).expect("Swap failed");

        assert_eq!(output.token_id, pool.asset_y.token_id);
        assert_eq!(output.amount, 332.try_into().unwrap());

        let swapped = pool.with_swap(&input).expect("Swap failed");

        assert_eq!(swapped.asset_y.amount, 668.try_into().unwrap());
        assert_eq!(swapped.asset_x.amount, 1500000000.try_into().unwrap());
    }
}
