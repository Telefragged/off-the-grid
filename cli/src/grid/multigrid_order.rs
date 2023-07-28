use ergo_lib::{
    ergo_chain_types::EcPoint,
    ergotree_ir::{
        chain::{
            address::Address,
            ergo_box::{
                box_value::{BoxValue, BoxValueError},
                ErgoBox, ErgoBoxCandidate, NonMandatoryRegisterId, NonMandatoryRegisters,
            },
            token::{TokenAmount, TokenAmountError, TokenId},
        },
        ergo_tree::ErgoTree,
        mir::constant::{Constant, Literal, TryExtractFrom, TryExtractInto},
    },
};

use lazy_static::lazy_static;
use std::collections::HashMap;
use thiserror::Error;

use crate::{
    boxes::{
        describe_box::{BoxAssetDisplay, ErgoBoxDescriptors},
        tracked_box::TrackedBox,
    },
    units::{Fraction, TokenStore, UnitAmount, ERG_UNIT},
};

const MIN_BOX_VALUE: u64 = 1000000;
pub const MAX_FEE: u64 = 2000000;

pub const MULTIGRID_ORDER_BASE16_BYTES: &[u8] = include_bytes!("../../grid_multi.ergotree");

lazy_static! {
    /// Grid order P2S address
    pub static ref MULTIGRID_ORDER_ADDRESS: Address =
        #[allow(clippy::unwrap_used)]
        Address::P2S(MULTIGRID_ORDER_BASE16_BYTES.to_vec());

    /// Grid order P2S script
    pub static ref MULTIGRID_ORDER_SCRIPT: ErgoTree = MULTIGRID_ORDER_ADDRESS.script().unwrap();
}

#[derive(Error, Debug)]
pub enum MultiGridConfigurationError {
    #[error("TokenId {0:?} expected, got {1:?}")]
    TokenId(TokenId, TokenId),

    #[error("Exactly {0} tokens expected, got {1}")]
    TokenAmount(u64, u64),

    #[error("Expected exactly one token, got {0}")]
    TokenLength(usize),

    #[error("Expected no tokens, got {0}")]
    TokenLengthNonZero(usize),

    #[error("Insufficient value to cover buy orders, {0} < {1}")]
    BidValue(u64, u64),
}

#[derive(Error, Debug)]
pub enum GridOrderEntriesError {
    #[error("No ask orders found")]
    NoAskOrders,

    #[error("No bid orders found")]
    NoBidOrders,
}

#[derive(Error, Debug)]
pub enum MultiGridOrderError {
    #[error(transparent)]
    BoxValueError(#[from] BoxValueError),

    #[error(transparent)]
    TokenAmountError(#[from] TokenAmountError),

    #[error("Invalid grid configuration: {0}")]
    InvalidConfiguration(#[from] MultiGridConfigurationError),

    #[error("Missing register value at {0:?}")]
    MissingRegisterValue(NonMandatoryRegisterId),

    #[error("Invalid register value at {0:?}: {1}")]
    InvalidRegisterValue(NonMandatoryRegisterId, String),

    #[error("{0} when converting number")]
    TryFromIntError(#[from] std::num::TryFromIntError),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum OrderState {
    Buy,
    Sell,
}

#[derive(Clone, Copy)]
pub struct GridOrderEntry {
    pub state: OrderState,
    pub token_amount: TokenAmount,
    pub bid_value: u64,
    pub ask_value: u64,
}

type EntryTuple = ((i64, bool), (i64, i64));

impl GridOrderEntry {
    pub fn new(
        state: OrderState,
        token_amount: TokenAmount,
        bid_value: u64,
        ask_value: u64,
    ) -> Self {
        Self {
            state,
            token_amount,
            bid_value,
            ask_value,
        }
    }

    pub fn order_amount(&self) -> u64 {
        *self.token_amount.as_u64()
    }

    pub fn bid(&self) -> Fraction {
        Fraction::new(self.bid_value, self.order_amount())
    }

    pub fn ask(&self) -> Fraction {
        Fraction::new(self.ask_value, self.order_amount())
    }

    pub fn to_register(self) -> Result<EntryTuple, MultiGridOrderError> {
        let state_bool = match self.state {
            OrderState::Buy => true,
            OrderState::Sell => false,
        };

        Ok((
            (self.order_amount().try_into()?, state_bool),
            (self.bid_value.try_into()?, self.ask_value.try_into()?),
        ))
    }

    pub fn from_register(r5_tuple: EntryTuple) -> Result<Self, MultiGridOrderError> {
        let ((amount, state_bool), (bid_value, ask_value)) = r5_tuple;

        let state = match state_bool {
            true => OrderState::Buy,
            false => OrderState::Sell,
        };

        let order_amount: u64 = amount.try_into()?;
        let bid_value = bid_value.try_into()?;
        let ask_value = ask_value.try_into()?;

        Ok(Self {
            state,
            token_amount: order_amount.try_into()?,
            bid_value,
            ask_value,
        })
    }
}

#[derive(Clone)]
pub struct GridOrderEntries(Vec<GridOrderEntry>);

impl GridOrderEntries {
    pub fn new(entries: Vec<GridOrderEntry>) -> Self {
        Self(entries)
    }

    pub fn to_registers(self) -> Result<Vec<EntryTuple>, MultiGridOrderError> {
        self.0
            .into_iter()
            .map(GridOrderEntry::to_register)
            .collect()
    }

    pub fn from_registers(registers: Vec<EntryTuple>) -> Result<Self, MultiGridOrderError> {
        let entries = registers
            .into_iter()
            .map(GridOrderEntry::from_register)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self(entries))
    }

    pub fn token_amount(&self) -> u64 {
        self.0
            .iter()
            .filter_map(|e| {
                if e.state == OrderState::Sell {
                    Some(e.order_amount())
                } else {
                    None
                }
            })
            .sum()
    }

    pub fn bid_entry(&self) -> Option<&GridOrderEntry> {
        self.0
            .iter()
            .filter(|e| e.state == OrderState::Buy)
            .max_by_key(|e| e.bid())
    }

    pub fn bid_entry_mut(&mut self) -> Option<&mut GridOrderEntry> {
        self.0
            .iter_mut()
            .filter(|e| e.state == OrderState::Buy)
            .max_by_key(|e| e.bid())
    }

    pub fn ask_entry(&self) -> Option<&GridOrderEntry> {
        self.0
            .iter()
            .filter(|e| e.state == OrderState::Sell)
            .min_by_key(|e| e.ask())
    }

    pub fn ask_entry_mut(&mut self) -> Option<&mut GridOrderEntry> {
        self.0
            .iter_mut()
            .filter(|e| e.state == OrderState::Sell)
            .min_by_key(|e| e.ask())
    }

    pub fn iter(&self) -> impl Iterator<Item = &GridOrderEntry> {
        self.0.iter()
    }

    pub fn into_fill_ask(mut self) -> Result<Self, GridOrderEntriesError> {
        if let Some(order) = self.ask_entry_mut() {
            order.state = OrderState::Buy;
            Ok(self)
        } else {
            Err(GridOrderEntriesError::NoAskOrders)
        }
    }

    pub fn into_fill_bid(mut self) -> Result<Self, GridOrderEntriesError> {
        if let Some(order) = self.bid_entry_mut() {
            order.state = OrderState::Sell;
            Ok(self)
        } else {
            Err(GridOrderEntriesError::NoBidOrders)
        }
    }
}

impl FromIterator<GridOrderEntry> for GridOrderEntries {
    fn from_iter<I: IntoIterator<Item = GridOrderEntry>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl From<Vec<GridOrderEntry>> for GridOrderEntries {
    fn from(entries: Vec<GridOrderEntry>) -> Self {
        Self(entries)
    }
}

#[derive(Clone)]
pub struct MultiGridOrder {
    owner_ec_point: EcPoint,
    pub metadata: Option<Vec<u8>>,
    pub token_id: TokenId,
    pub entries: GridOrderEntries,
    pub value: BoxValue,
}

impl MultiGridOrder {
    pub fn new(
        owner_ec_point: EcPoint,
        token_id: TokenId,
        entries: GridOrderEntries,
        metadata: Option<Vec<u8>>,
    ) -> Result<Self, MultiGridOrderError> {
        let value = entries
            .0
            .iter()
            .filter(|e| e.state == OrderState::Buy)
            .fold(MIN_BOX_VALUE, |acc, e| acc + e.bid_value)
            .try_into()?;

        Ok(Self {
            owner_ec_point,
            token_id,
            entries,
            value,
            metadata,
        })
    }

    pub fn bid_entry(&self) -> Option<&GridOrderEntry> {
        self.entries.bid_entry()
    }

    pub fn ask_entry(&self) -> Option<&GridOrderEntry> {
        self.entries.ask_entry()
    }

    pub fn bid(&self) -> Option<Fraction> {
        self.bid_entry().map(|e| e.bid())
    }

    pub fn ask(&self) -> Option<Fraction> {
        self.ask_entry().map(|e| e.ask())
    }

    pub fn with_entries(self, entries: GridOrderEntries) -> Result<Self, MultiGridOrderError> {
        let value = self.entries.0.iter().zip(entries.0.iter()).fold(
            self.value.as_i64(),
            |value, (old, new)| match (old.state, new.state) {
                (OrderState::Buy, OrderState::Sell) => value - old.bid_value as i64,
                (OrderState::Sell, OrderState::Buy) => value + old.ask_value as i64,
                _ => value,
            },
        );

        let new_order = Self {
            owner_ec_point: self.owner_ec_point,
            token_id: self.token_id,
            entries,
            value: value.try_into()?,
            metadata: self.metadata,
        };

        Ok(new_order)
    }

    /// Amount of ergs that have been collected for this order.
    /// Assumes the box was created with either MIN_BOX_VALUE or MIN_BOX_VALUE + bid_value,
    /// depending on the initial order state.
    pub fn profit(&self) -> u64 {
        let expected_value = self
            .entries
            .0
            .iter()
            .filter(|e| e.state == OrderState::Buy)
            .fold(MIN_BOX_VALUE, |acc, e| acc + e.bid_value);

        self.value.as_u64() - expected_value
    }

    pub fn into_box_candidate(
        self,
        creation_height: u32,
    ) -> Result<ErgoBoxCandidate, MultiGridOrderError> {
        let token_amount = self.entries.token_amount();

        let orders = self.entries.to_registers()?;

        let mut registers: HashMap<NonMandatoryRegisterId, Constant> = HashMap::from([
            (NonMandatoryRegisterId::R4, self.owner_ec_point.into()),
            (NonMandatoryRegisterId::R5, orders.into()),
            (NonMandatoryRegisterId::R6, self.token_id.into()),
        ]);

        if let Some(metadata) = self.metadata {
            registers.insert(NonMandatoryRegisterId::R7, metadata.into());
        }

        let tokens = if token_amount > 0 {
            let token = (self.token_id, token_amount.try_into()?).into();
            Some(vec![token].try_into().unwrap())
        } else {
            None
        };

        let order_box = ErgoBoxCandidate {
            value: self.value,
            ergo_tree: MULTIGRID_ORDER_SCRIPT.clone(),
            tokens,
            additional_registers: NonMandatoryRegisters::new(registers).unwrap(),
            creation_height,
        };

        Ok(order_box)
    }
}

impl TryFrom<&ErgoBox> for MultiGridOrder {
    type Error = MultiGridOrderError;

    fn try_from(ergo_box: &ErgoBox) -> Result<Self, Self::Error> {
        fn get_register_extract<T>(
            value: &ErgoBox,
            register: NonMandatoryRegisterId,
        ) -> Result<T, MultiGridOrderError>
        where
            T: TryExtractFrom<Literal>,
        {
            value
                .additional_registers
                .get_constant(register)
                .ok_or(MultiGridOrderError::MissingRegisterValue(register))
                .and_then(|c| {
                    c.clone()
                        .try_extract_into::<T>()
                        .map_err(|e| MultiGridOrderError::InvalidRegisterValue(register, e.0))
                })
        }

        let owner_ec_point: EcPoint = get_register_extract(ergo_box, NonMandatoryRegisterId::R4)?;
        let orders: Vec<_> = get_register_extract(ergo_box, NonMandatoryRegisterId::R5)?;

        let token_id: TokenId = get_register_extract(ergo_box, NonMandatoryRegisterId::R6)?;

        let metadata: Option<Vec<u8>> =
            get_register_extract(ergo_box, NonMandatoryRegisterId::R7).ok();

        let entries = orders
            .into_iter()
            .map(GridOrderEntry::from_register)
            .collect::<Result<Vec<_>, _>>()?
            .into();

        let order = Self {
            owner_ec_point,
            token_id,
            entries,
            metadata,
            value: ergo_box.value,
        };

        let current_value = *ergo_box.value.as_u64();
        let min_value = order.entries.0.iter().map(|e| e.bid_value).sum::<u64>() + MIN_BOX_VALUE;

        let expected_token_amount = order
            .entries
            .0
            .iter()
            .filter_map(|e| {
                if e.state == OrderState::Sell {
                    Some(e.token_amount.as_u64())
                } else {
                    None
                }
            })
            .sum::<u64>();

        // Validate order state
        match &ergo_box.tokens {
            None if expected_token_amount > 0 => Err(MultiGridConfigurationError::TokenLength(0)),
            Some(_) if expected_token_amount == 0 => {
                Err(MultiGridConfigurationError::TokenLengthNonZero(1))
            }
            Some(v) => {
                if let [token] = v.as_slice() {
                    if token.token_id != token_id {
                        Err(MultiGridConfigurationError::TokenId(
                            token_id,
                            token.token_id,
                        ))
                    } else if *token.amount.as_u64() != expected_token_amount {
                        Err(MultiGridConfigurationError::TokenAmount(
                            expected_token_amount,
                            *token.amount.as_u64(),
                        ))
                    } else {
                        Ok(order)
                    }
                } else {
                    Err(MultiGridConfigurationError::TokenLength(v.len()))
                }
            }
            _ if current_value < min_value => Err(MultiGridConfigurationError::BidValue(
                min_value,
                current_value,
            )),
            _ => Ok(order),
        }
        .map_err(|e| e.into())
    }
}

pub trait MultiGridRef: Clone {
    fn order_ref(&self) -> &MultiGridOrder;
}

impl MultiGridRef for &MultiGridOrder {
    fn order_ref(&self) -> &MultiGridOrder {
        self
    }
}

impl MultiGridRef for TrackedBox<MultiGridOrder> {
    fn order_ref(&self) -> &MultiGridOrder {
        &self.value
    }
}

pub trait FillMultiGridOrders: Sized {
    type Error;

    #[allow(clippy::type_complexity)]
    fn fill_orders<T>(
        self,
        grid_orders: Vec<T>,
    ) -> Result<(Self, Vec<(T, MultiGridOrder)>), Self::Error>
    where
        T: MultiGridRef;
}

impl ErgoBoxDescriptors for MultiGridOrder {
    fn box_name(&self) -> String {
        "MultiGrid".to_string()
    }

    fn assets<'a>(&self, tokens: &'a TokenStore) -> BoxAssetDisplay<'a> {
        let total_tokens = self
            .entries
            .iter()
            .filter_map(|o| match o.state {
                OrderState::Sell => Some(o.token_amount.as_u64()),
                OrderState::Buy => None,
            })
            .sum::<u64>();

        let token_id = self.token_id;
        let token_info = tokens.get_unit(&token_id);

        let value_amount = UnitAmount::new(*ERG_UNIT, *self.value.as_u64());
        let total_tokens = UnitAmount::new(token_info, total_tokens);

        BoxAssetDisplay::Double(value_amount, total_tokens)
    }
}
