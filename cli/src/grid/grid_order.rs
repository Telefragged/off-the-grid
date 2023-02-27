use ergo_lib::{
    ergo_chain_types::EcPoint,
    ergotree_ir::{
        chain::{
            address::Address,
            ergo_box::{
                box_value::{BoxValue, BoxValueError},
                ErgoBox, ErgoBoxCandidate, NonMandatoryRegisterId,
            },
            token::{Token, TokenAmount, TokenAmountError, TokenId},
        },
        mir::constant::{Constant, Literal, TryExtractFrom, TryExtractInto},
    },
};
use lazy_static::lazy_static;
use std::collections::HashMap;
use thiserror::Error;

const MIN_BOX_VALUE: u64 = 1000000;
pub const MAX_FEE: u64 = 2000000;

const GRID_ORDER_BASE16_BYTES: &str = "100a040001010500040204000400040005000e691005040004000e36100204a00b08cd0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798ea02d192a39a8cc7a701730073011001020402d19683030193a38cc7b2a57300000193c2b2a57301007473027303830108cdeeac93b1a57304058092f401d809d601e4c6a70407d602b2a5db6508fe00d603e4c6a70559d604e4c6a7064d0ed605c6a7070ed60693b1db6308a77300d6078c720402d6089572068c7203018c720302d60995720699c1a7c1720299c17202c1a7eb02cd7201d1edededededededed93c27202c2a793e4c672020407720193e4c672020559720393e4c67202064d0e720495e6720593e4c67202070ee4720573019572069072099c720772089272099c720772089172097302957206d801d60adb63087202eded93b1720a7303938cb2720a730400018c720401938cb2720a73050002720793b1db63087202730693b0a57307d9010a4163d802d60c8c720a02d60d8c720a019593c2720c73089a720dc1720c720d7309";
lazy_static! {
    /// Grid order P2S address
    pub static ref GRID_ORDER_ADDRESS: Address =
        #[allow(clippy::unwrap_used)]
        Address::P2S(base16::decode(GRID_ORDER_BASE16_BYTES).unwrap());

}

#[derive(Error, Debug)]
pub enum GridConfigurationError {
    #[error("TokenId {0:?} expected, got {1:?}")]
    TokenId(TokenId, TokenId),

    #[error("Exactly {0:?} tokens expected, got {1:?}")]
    TokenAmount(TokenAmount, TokenAmount),

    #[error("Expected exactly one token, got {0}")]
    TokenLength(usize),

    #[error("Insufficient value to cover bid tx, {0} < {1}")]
    BidValue(u64, u64),
}

#[derive(Error, Debug)]
pub enum GridOrderError {
    #[error(transparent)]
    BoxValueError(#[from] BoxValueError),

    #[error(transparent)]
    TokenAmountError(#[from] TokenAmountError),

    #[error("Invalid grid configuration: {0}")]
    InvalidConfiguration(#[from] GridConfigurationError),

    #[error("Missing register value at {0:?}")]
    MissingRegisterValue(NonMandatoryRegisterId),

    #[error("Invalid register value at {0:?}: {1}")]
    InvalidRegisterValue(NonMandatoryRegisterId, String),

    #[error("{0} when converting number")]
    TryFromIntError(#[from] std::num::TryFromIntError),
}

#[derive(Clone, Copy, PartialEq)]
pub enum OrderState {
    Buy,
    Sell,
}

#[derive(Clone)]
pub struct GridOrder {
    owner_ec_point: EcPoint,
    bid: u64,
    ask: u64,
    metadata: Option<Vec<u8>>,
    pub token: Token,
    pub state: OrderState,
    pub value: BoxValue,
}

impl GridOrder {
    pub fn new(
        owner_ec_point: EcPoint,
        bid: u64,
        ask: u64,
        token: Token,
        state: OrderState,
        metadata: Option<Vec<u8>>,
    ) -> Result<Self, GridOrderError> {
        let order_amount = token.amount.as_u64();
        let value = match state {
            OrderState::Sell => MIN_BOX_VALUE,
            OrderState::Buy => MIN_BOX_VALUE + bid * order_amount,
        }
        .try_into()?;

        let order = Self {
            owner_ec_point,
            bid,
            ask,
            token,
            state,
            value,
            metadata,
        };

        Ok(order)
    }

    pub fn order_amount(&self) -> u64 {
        *self.token.amount.as_u64()
    }

    pub fn bid_value(&self) -> u64 {
        self.order_amount() * self.bid
    }

    pub fn into_filled(self) -> Result<Self, GridOrderError> {
        let order_amount = self.order_amount();

        let value = match self.state {
            OrderState::Sell => self.value.as_u64() + self.ask * order_amount,
            OrderState::Buy => self.value.as_u64() - self.bid * order_amount,
        }
        .try_into()?;

        let state = match self.state {
            OrderState::Sell => OrderState::Buy,
            OrderState::Buy => OrderState::Sell,
        };

        Ok(Self {
            value,
            state,
            ..self
        })
    }

    pub fn into_box_candidate(
        self,
        creation_height: u32,
    ) -> Result<ErgoBoxCandidate, GridOrderError> {
        let token_pair = (
            self.token.token_id.clone(),
            i64::try_from(*self.token.amount.as_u64())?,
        );

        let mut registers: HashMap<NonMandatoryRegisterId, Constant> = HashMap::from([
            (NonMandatoryRegisterId::R4, self.owner_ec_point.into()),
            (
                NonMandatoryRegisterId::R5,
                (i64::try_from(self.bid)?, i64::try_from(self.ask)?).into(),
            ),
            (NonMandatoryRegisterId::R6, token_pair.into()),
        ]);

        if let Some(metadata) = self.metadata {
            registers.insert(NonMandatoryRegisterId::R7, metadata.into());
        }

        let tokens = match self.state {
            OrderState::Buy => None,
            OrderState::Sell => Some(vec![self.token].try_into().unwrap()),
        };

        let order_box = ErgoBoxCandidate {
            value: self.value,
            ergo_tree: GRID_ORDER_ADDRESS.script().unwrap(),
            tokens,
            additional_registers: registers.try_into().unwrap(),
            creation_height,
        };

        Ok(order_box)
    }
}

impl TryFrom<&ErgoBox> for GridOrder {
    type Error = GridOrderError;

    fn try_from(ergo_box: &ErgoBox) -> Result<Self, Self::Error> {
        fn get_register_extract<T>(
            value: &ErgoBox,
            register: NonMandatoryRegisterId,
        ) -> Result<T, GridOrderError>
        where
            T: TryExtractFrom<Literal>,
        {
            value
                .additional_registers
                .get(register)
                .ok_or(GridOrderError::MissingRegisterValue(register))
                .and_then(|c| {
                    c.clone()
                        .try_extract_into::<T>()
                        .map_err(|e| GridOrderError::InvalidRegisterValue(register, e.0))
                })
        }

        let owner_ec_point: EcPoint = get_register_extract(ergo_box, NonMandatoryRegisterId::R4)?;
        let (bid, ask): (i64, i64) = get_register_extract(ergo_box, NonMandatoryRegisterId::R5)?;
        let (token_id, order_amount): (TokenId, i64) =
            get_register_extract(ergo_box, NonMandatoryRegisterId::R6)?;
        let metadata: Option<Vec<u8>> =
            get_register_extract(ergo_box, NonMandatoryRegisterId::R7).ok();

        let state: OrderState = if ergo_box.tokens.is_none() {
            OrderState::Buy
        } else {
            OrderState::Sell
        };

        let bid = bid.try_into()?;
        let ask = ask.try_into()?;
        let order_amount: u64 = order_amount.try_into()?;

        let order_token_amount: TokenAmount = order_amount.try_into()?;

        let order = Self {
            owner_ec_point,
            bid,
            ask,
            token: (token_id.clone(), order_token_amount).into(),
            state,
            metadata,
            value: ergo_box.value,
        };

        let bid_value = *ergo_box.value.as_u64();
        let min_value = MIN_BOX_VALUE + bid * order_amount;

        // Validate order state
        match (state, &ergo_box.tokens) {
            (OrderState::Buy, Some(v)) => Err(GridConfigurationError::TokenLength(v.len())),
            (OrderState::Buy, None) if bid_value < min_value => {
                Err(GridConfigurationError::BidValue(bid_value, min_value))
            }
            (OrderState::Sell, None) => Err(GridConfigurationError::TokenLength(0)),
            (OrderState::Sell, Some(v)) => {
                if let [token] = v.as_slice() {
                    if token.token_id != token_id {
                        Err(GridConfigurationError::TokenId(
                            token_id,
                            token.token_id.clone(),
                        ))
                    } else if token.amount != order_token_amount {
                        Err(GridConfigurationError::TokenAmount(
                            order_token_amount,
                            token.amount,
                        ))
                    } else {
                        Ok(order)
                    }
                } else {
                    Err(GridConfigurationError::TokenLength(v.len()))
                }
            }
            _ => Ok(order),
        }
        .map_err(|e| e.into())
    }
}
