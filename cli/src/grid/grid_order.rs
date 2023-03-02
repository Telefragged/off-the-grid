use ergo_lib::ergotree_ir::{
    chain::{
        address::Address,
        ergo_box::{
            box_value::{BoxValue, BoxValueError},
            ErgoBox, ErgoBoxCandidate, NonMandatoryRegisterId,
        },
        token::{Token, TokenAmount, TokenAmountError, TokenId},
    },
    mir::constant::{Constant, Literal, TryExtractFrom, TryExtractInto},
    sigma_protocol::sigma_boolean::{ProveDlog, SigmaProp},
};
use lazy_static::lazy_static;
use std::collections::HashMap;
use thiserror::Error;

const MIN_BOX_VALUE: u64 = 1000000;
pub const MAX_FEE: u64 = 2000000;

const GRID_ORDER_BASE16_BYTES: &str = "100a040004000500040204000400040005000e691005040004000e36100204a00b08cd0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798ea02d192a39a8cc7a701730073011001020402d19683030193a38cc7b2a57300000193c2b2a57301007473027303830108cdeeac93b1a57304058092f401d808d601e4c6a70408d602b2a5dc0c1aa402a7730000d603e4c6a70559d604e4c6a7064d0ed60593b1db6308a77301d6068c720402d6079572058c7203018c720302d60895720599c1a7c1720299c17202c1a7eb027201d1ededededededed93c27202c2a793e4c672020408720193e4c672020559720393e4c67202064d0e72049572059072089c720672079272089c720672079172087302957205d801d609db63087202eded93b172097303938cb27209730400018c720401938cb2720973050002720693b1db63087202730693b0a57307d901094163d802d60b8c720902d60c8c7209019593c2720b73089a720cc1720b720c7309";
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
    BidValue(i64, i64),
}

#[derive(Error, Debug)]
pub enum GridOrderError {
    #[error(transparent)]
    BoxValueError(#[from] BoxValueError),

    #[error(transparent)]
    TokenAmountError(#[from] TokenAmountError),

    #[error("Failed to extract dlog from sigma proposition")]
    ConvesionError,

    #[error("Invalid grid configuration: {0}")]
    InvalidConfiguration(#[from] GridConfigurationError),

    #[error("Missing register value at {0:?}")]
    MissingRegisterValue(NonMandatoryRegisterId),

    #[error("Invalid register value at {0:?}: {1}")]
    InvalidRegisterValue(NonMandatoryRegisterId, String),
}

#[derive(Clone, Copy)]
pub enum OrderState {
    Buy,
    Sell,
}

pub struct GridOrder {
    owner_dlog: ProveDlog,
    bid: i64,
    ask: i64,
    token: Token,
    state: OrderState,
    pub value: BoxValue,
}

impl GridOrder {
    pub fn new(
        owner_dlog: ProveDlog,
        bid: i64,
        ask: i64,
        token: Token,
        state: OrderState,
    ) -> Result<Self, GridOrderError> {
        let order_amount = *token.amount.as_u64() as i64;
        let value = match state {
            OrderState::Sell => MIN_BOX_VALUE as i64,
            OrderState::Buy => MIN_BOX_VALUE as i64 + bid * order_amount,
        }
        .try_into()?;

        let order = Self {
            owner_dlog,
            bid,
            ask,
            token,
            state,
            value,
        };

        Ok(order)
    }

    pub fn order_amount(&self) -> i64 {
        *self.token.amount.as_u64() as i64
    }

    pub fn bid_value(&self) -> i64 {
        self.order_amount() * self.bid
    }

    pub fn into_filled(self) -> Result<Self, GridOrderError> {
        let order_amount = self.order_amount();

        let value = match self.state {
            OrderState::Sell => self.value.as_i64() + self.ask * order_amount,
            OrderState::Buy => self.value.as_i64() - self.bid * order_amount,
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

    pub fn to_box_candidate(
        &self,
        creation_height: u32,
    ) -> Result<ErgoBoxCandidate, GridOrderError> {
        let token_pair = (
            self.token.token_id.clone(),
            *self.token.amount.as_u64() as i64,
        );

        let registers: HashMap<NonMandatoryRegisterId, Constant> = HashMap::from([
            (NonMandatoryRegisterId::R4, self.owner_dlog.clone().into()),
            (NonMandatoryRegisterId::R5, (self.bid, self.ask).into()),
            (NonMandatoryRegisterId::R6, token_pair.into()),
        ]);

        let tokens = match self.state {
            OrderState::Buy => None,
            OrderState::Sell => Some(vec![self.token.clone()].try_into().unwrap()),
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

        let owner_prop: SigmaProp = get_register_extract(ergo_box, NonMandatoryRegisterId::R4)?;
        let owner_dlog: ProveDlog = owner_prop
            .value()
            .clone()
            .try_into()
            .map_err(|_| GridOrderError::ConvesionError)?;
        let (bid, ask): (i64, i64) = get_register_extract(ergo_box, NonMandatoryRegisterId::R5)?;
        let (token_id, order_amount): (TokenId, i64) =
            get_register_extract(ergo_box, NonMandatoryRegisterId::R6)?;

        let state: OrderState = if ergo_box.tokens.is_none() {
            OrderState::Buy
        } else {
            OrderState::Sell
        };

        let order_token_amount: TokenAmount = (order_amount as u64).try_into()?;

        let order = Self {
            owner_dlog,
            bid,
            ask,
            token: (token_id.clone(), order_token_amount).into(),
            state,
            value: ergo_box.value,
        };

        let bid_value = ergo_box.value.as_i64();
        let min_value = MIN_BOX_VALUE as i64 + bid * order_amount;

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
