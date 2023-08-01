use std::{collections::HashMap, fmt::Display, str::FromStr};

use ergo_lib::{ergo_chain_types::Digest32, ergotree_ir::chain::token::TokenId};
use fraction::{GenericFraction, ToPrimitive};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Fraction = GenericFraction<u128>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenInfo {
    #[serde(rename = "id")]
    pub token_id: TokenId,
    pub name: String,
    pub decimals: u32,
}

impl PartialEq for TokenInfo {
    fn eq(&self, other: &Self) -> bool {
        self.token_id == other.token_id
    }
}

impl Eq for TokenInfo {}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Unit<'a> {
    Known(&'a TokenInfo),
    Unknown(TokenId),
}

impl Unit<'_> {
    pub fn base_amount(&self) -> u64 {
        match self {
            Unit::Known(info) => 10u64.pow(info.decimals),
            Unit::Unknown(_) => 1,
        }
    }

    pub fn decimals(&self) -> u32 {
        match self {
            Unit::Known(info) => info.decimals,
            Unit::Unknown(_) => 0,
        }
    }

    pub fn name(&self) -> String {
        match self {
            Unit::Known(info) => info.name.clone(),
            Unit::Unknown(token_id) => (*token_id).into(),
        }
    }

    pub fn format(&self, amount: Fraction) -> String {
        match self {
            Unit::Known(info) => {
                format!("{:.1$} {2}", amount, info.decimals as usize, info.name)
            }
            Unit::Unknown(_) => format!("{:.0}", amount),
        }
    }

    pub fn token_id(&self) -> TokenId {
        match self {
            Unit::Known(info) => info.token_id,
            Unit::Unknown(token_id) => *token_id,
        }
    }

    pub fn str_amount(&self, amount: &str) -> Option<UnitAmount> {
        Fraction::from_str(amount)
            .ok()
            .and_then(|amount| (amount * self.base_amount()).floor().to_u64())
            .map(|amount| UnitAmount::new(*self, amount))
    }
}

lazy_static! {
    pub static ref ERG_TOKEN_INFO: TokenInfo = TokenInfo {
        token_id: Digest32::zero().into(),
        name: "ERG".to_string(),
        decimals: 9,
    };
    pub static ref ERG_UNIT: Unit<'static> = Unit::Known(&ERG_TOKEN_INFO);
}

#[derive(Clone, Debug)]
pub struct UnitAmount<'a> {
    unit: Unit<'a>,
    amount: u64,
}

impl<'a> UnitAmount<'a> {
    pub fn new(unit: Unit<'a>, amount: u64) -> Self {
        Self { unit, amount }
    }

    pub fn unit(&self) -> &Unit {
        &self.unit
    }

    pub fn amount(&self) -> u64 {
        self.amount
    }

    pub fn fraction(&self) -> Fraction {
        Fraction::new(self.amount, self.unit.base_amount())
    }

    pub fn format(&self) -> String {
        self.unit
            .format(Fraction::new(self.amount, self.unit.base_amount()))
    }
}

impl<'a> Display for UnitAmount<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let precision = f.precision().unwrap_or(self.unit.decimals() as usize);

        let fraction_str = format!("{:.1$}", self.fraction(), precision);

        f.pad_integral(true, "", &fraction_str)?;

        if f.alternate() {
            return Ok(());
        }

        match self.unit() {
            Unit::Known(info) => {
                write!(f, " {}", info.name)
            }
            Unit::Unknown(token_id) => {
                write!(f, " {:?}", token_id)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Price<'a> {
    base: Unit<'a>,
    quote: Unit<'a>,
    price: Fraction,
}

impl<'a> Price<'a> {
    pub fn new(base: Unit<'a>, quote: Unit<'a>, amount: Fraction) -> Self {
        Self {
            base,
            quote,
            price: amount,
        }
    }

    pub fn indirect(&'a self) -> Price<'a> {
        Self {
            base: self.quote,
            quote: self.base,
            price: self.price.recip(),
        }
    }

    pub fn format(&self) -> String {
        format!(
            "{0:.1$} {2}/{3}",
            self.price * Fraction::new(self.base.base_amount(), self.quote.base_amount()),
            self.quote.decimals() as usize,
            self.base.name(),
            self.quote.name()
        )
    }

    pub fn convert_price(&self, other: &UnitAmount) -> Option<UnitAmount> {
        if self.base == *other.unit() {
            let amount = self.price * other.amount;
            Some(UnitAmount::new(
                self.quote,
                amount.floor().to_u64().unwrap_or_default(),
            ))
        } else if self.quote == *other.unit() {
            let self_recip = self.price.recip();
            let amount = self_recip * other.amount();
            Some(UnitAmount::new(
                self.base,
                amount.floor().to_u64().unwrap_or_default(),
            ))
        } else {
            None
        }
    }

    pub fn price(&self) -> Fraction {
        self.price * Fraction::new(self.base.base_amount(), self.quote.base_amount())
    }
}

impl Display for Price<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.format())
    }
}

#[derive(Error, Debug)]
pub enum TokenStoreError {
    #[error("Failed to load token store: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Failed to parse token store: {0}")]
    ParseError(#[from] serde_json::Error),
}

pub struct TokenStore {
    tokens: HashMap<TokenId, TokenInfo>,
}

impl Default for TokenStore {
    fn default() -> Self {
        Self {
            tokens: HashMap::from([(ERG_TOKEN_INFO.token_id, ERG_TOKEN_INFO.clone())]),
        }
    }
}

impl TokenStore {
    pub fn with_tokens(tokens: Vec<TokenInfo>) -> Self {
        let mut ret: Self = Default::default();

        ret.tokens
            .extend(tokens.into_iter().map(|token| (token.token_id, token)));

        ret
    }

    pub fn get_unit(&self, token_id: &TokenId) -> Unit {
        self.tokens
            .get(token_id)
            .map(Unit::Known)
            .unwrap_or(Unit::Unknown(*token_id))
    }

    pub fn get_unit_by_id(&self, token_name: &str) -> Option<Unit> {
        self.tokens
            .values()
            .find(|token| token.name == token_name)
            .map(Unit::Known)
            .or_else(|| {
                Digest32::try_from(token_name.to_string())
                    .ok()
                    .map(|token_id| Unit::Unknown(token_id.into()))
            })
    }

    pub fn save(&self, path: Option<String>) -> Result<(), TokenStoreError> {
        let path = path.unwrap_or("tokens.json".to_string());
        let file = std::fs::File::create(path)?;
        let writer = std::io::BufWriter::new(file);
        let tokens_vec = self.tokens.values().collect::<Vec<_>>();
        serde_json::to_writer_pretty(writer, &tokens_vec)?;
        Ok(())
    }

    pub fn load(path: Option<String>) -> Result<Self, TokenStoreError> {
        let path = path.unwrap_or("tokens.json".to_string());
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let tokens_vec: Vec<TokenInfo> = serde_json::from_reader(reader)?;
        Ok(Self::with_tokens(tokens_vec))
    }

    pub fn tokens(&self) -> impl Iterator<Item = &TokenInfo> {
        self.tokens.values()
    }
}

#[cfg(test)]
mod tests {
    use ergo_lib::ergo_chain_types::{Digest, Digest32};
    use proptest::prelude::*;

    use crate::units::{Price, UnitAmount};

    use super::{Fraction, TokenInfo, Unit};

    proptest! {
        #[test]
        fn convert_price_prop(
            amount in any::<u64>(),
            price1 in any::<u64>(),
            price2 in any::<u64>(),
            decimals1 in any::<u32>(),
            decimals2 in any::<u32>(),
        ) {
            convert_price(decimals1, decimals2, price1, price2, amount);
        }
    }

    #[test]
    fn convert_price_overflow() {
        let price1 = 4612850766424834936u64;
        let price2 = 4616774163163926707u64;
        let amount = 4;
        let decimals1 = 0u32;
        let decimals2 = 0u32;

        convert_price(decimals1, decimals2, price1, price2, amount);
    }

    #[test]
    fn convert_price_zeroes() {
        let price1 = 0u64;
        let price2 = 0u64;
        let amount = 4;
        let decimals1 = 0u32;
        let decimals2 = 0u32;

        convert_price(decimals1, decimals2, price1, price2, amount);
    }

    #[test]
    fn convert_unknown() {
        let mut token_bytes = [0u8; 32];
        token_bytes[0] = 1;

        let amount = 1000;

        let unit1 = Unit::Unknown(Digest32::zero().into());
        let unit2 = Unit::Unknown(Digest::<32>(token_bytes).into());

        let price = Price::new(unit1, unit2, Fraction::new(1u64, 13u64));

        let unit_amount = UnitAmount::new(unit1, amount);
        let unit_amount2 = price.convert_price(&unit_amount).unwrap();

        assert_eq!(unit_amount2.unit, unit2);
        assert_eq!(unit_amount2.amount(), 1000 / 13);
    }

    #[test]
    fn convert_one_known() {
        let mut token_bytes = [0u8; 32];
        token_bytes[0] = 1;

        let amount = 2000;

        let unit1_info = TokenInfo {
            token_id: Digest32::zero().into(),
            name: "A".to_string(),
            decimals: 9,
        };

        let unit1 = Unit::Known(&unit1_info);
        let unit2 = Unit::Unknown(Digest::<32>(token_bytes).into());

        let price = Price::new(unit1, unit2, Fraction::new(1u64, 13u64));

        let unit_amount = UnitAmount::new(unit1, amount);
        let unit_amount2 = price.convert_price(&unit_amount).unwrap();

        assert_eq!(unit_amount2.unit, unit2);
        assert_eq!(unit_amount2.amount(), 2000 / 13);
    }

    fn convert_price(decimals1: u32, decimals2: u32, price1: u64, price2: u64, amount: u64) {
        let mut token_bytes = [0u8; 32];
        token_bytes[0] = 1;

        let unit1_info = TokenInfo {
            token_id: Digest::<32>(token_bytes).into(),
            name: "A".to_string(),
            decimals: decimals1,
        };
        let unit1 = Unit::Known(&unit1_info);

        let unit2_info = TokenInfo {
            token_id: Digest32::zero().into(),
            name: "B".to_string(),
            decimals: decimals2,
        };
        let unit2 = Unit::Known(&unit2_info);

        let price = Price::new(unit1, unit2, Fraction::new(price1, price2));

        let unit_amount = UnitAmount::new(unit1, amount);
        let unit_amount2 = price
            .convert_price(&unit_amount)
            .expect("price conversion failed");
        let binding = price.indirect();
        let unit_amount3 = binding
            .convert_price(&unit_amount2)
            .expect("price conversion failed");

        assert_eq!(unit_amount3.unit, unit1);
        assert_eq!(unit_amount2.unit, unit2);
    }
}
