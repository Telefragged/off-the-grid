use std::collections::HashMap;

use ergo_lib::{
    ergo_chain_types::Digest32,
    ergotree_ir::{
        chain::{
            ergo_box::{
                box_value::BoxValueError, BoxId, ErgoBox, ErgoBoxCandidate, NonMandatoryRegisterId,
            },
            token::{Token, TokenAmount, TokenAmountError, TokenId}, address::Address,
        },
        mir::constant::{Constant, TryExtractInto},
    },
};
use lazy_static::lazy_static;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use thiserror::Error;

const N2T_POOL_ERGO_TREE_BASE16: &str = "1999030f0400040204020404040405feffffffffffffffff0105feffffffffffffffff01050004d00f040004000406050005000580dac409d819d601b2a5730000d602e4c6a70404d603db63087201d604db6308a7d605b27203730100d606b27204730200d607b27203730300d608b27204730400d6099973058c720602d60a999973068c7205027209d60bc17201d60cc1a7d60d99720b720cd60e91720d7307d60f8c720802d6107e720f06d6117e720d06d612998c720702720fd6137e720c06d6147308d6157e721206d6167e720a06d6177e720906d6189c72117217d6199c72157217d1ededededededed93c27201c2a793e4c672010404720293b27203730900b27204730a00938c7205018c720601938c7207018c72080193b17203730b9593720a730c95720e929c9c721072117e7202069c7ef07212069a9c72137e7214067e9c720d7e72020506929c9c721372157e7202069c7ef0720d069a9c72107e7214067e9c72127e7202050695ed720e917212730d907216a19d721872139d72197210ed9272189c721672139272199c7216721091720b730e";

lazy_static! {
    /// Spectrum ERG token id
    pub static ref ERG_TOKEN_ID: TokenId =
        TokenId::from(Digest32::zero());

    pub static ref N2T_POOL_ADDRESS: Address =
        #[allow(clippy::unwrap_used)]
        Address::P2S(base16::decode(N2T_POOL_ERGO_TREE_BASE16).unwrap());
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
