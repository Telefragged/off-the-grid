use ergo_lib::ergotree_ir::chain::{
    ergo_box::{box_value::BoxValueError, ErgoBoxCandidate},
    token::{Token, TokenAmountError, TokenId},
};
use num_bigint::BigInt;
use thiserror::Error;

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
