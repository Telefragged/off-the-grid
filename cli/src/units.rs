use std::{collections::HashMap, fmt::Display};

use ergo_lib::{ergo_chain_types::Digest32, ergotree_ir::chain::token::TokenId};
use fraction::{BigFraction, ToPrimitive};
use serde::{Deserialize, Serialize};

pub type Fraction = BigFraction;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unit {
    Known(TokenInfo),
    Unknown(TokenId),
}

impl Unit {
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
}

#[derive(Clone, Debug)]
pub struct UnitAmount {
    unit: Unit,
    amount: u64,
}

impl UnitAmount {
    pub fn new(unit: Unit, amount: u64) -> Self {
        Self { unit, amount }
    }

    pub fn unit(&self) -> &Unit {
        &self.unit
    }

    pub fn amount(&self) -> u64 {
        self.amount
    }

    pub fn format(&self) -> String {
        self.unit
            .format(Fraction::new(self.amount, self.unit.base_amount()))
    }
}

impl Display for UnitAmount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.format())
    }
}

#[derive(Clone, Debug)]
pub struct Price {
    base: Unit,
    quote: Unit,
    price: Fraction,
}

impl Price {
    pub fn new(base: Unit, quote: Unit, amount: Fraction) -> Self {
        Self {
            base,
            quote,
            price: amount,
        }
    }

    pub fn indirect(&self) -> Self {
        Self {
            base: self.quote.clone(),
            quote: self.base.clone(),
            price: self.price.recip(),
        }
    }

    pub fn format(&self) -> String {
        format!(
            "{0:.1$} {2}/{3}",
            self.price.clone() * Fraction::new(self.base.base_amount(), self.quote.base_amount()),
            self.quote.decimals() as usize,
            self.base.name(),
            self.quote.name()
        )
    }

    pub fn convert_price(&self, other: &UnitAmount) -> Option<UnitAmount> {
        if self.base == *other.unit() {
            let amount = self.price.clone() * other.amount;
            Some(UnitAmount::new(
                self.quote.clone(),
                amount.floor().to_u64().unwrap_or_default(),
            ))
        } else if self.quote == *other.unit() {
            let self_recip = self.price.recip();
            let amount = self_recip * other.amount();
            Some(UnitAmount::new(
                self.base.clone(),
                amount.floor().to_u64().unwrap_or_default(),
            ))
        } else {
            None
        }
    }
}

impl Display for Price {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.format())
    }
}

pub struct TokenStore {
    tokens: HashMap<TokenId, TokenInfo>,
}

impl TokenStore {
    pub fn with_tokens(tokens: Vec<TokenInfo>) -> Self {
        let erg_token = TokenInfo {
            token_id: Digest32::zero().into(),
            name: "ERG".to_string(),
            decimals: 9,
        };

        let mut tokens = tokens;
        tokens.push(erg_token);

        let tokens = tokens
            .into_iter()
            .map(|token| (token.token_id, token))
            .collect();

        Self { tokens }
    }

    pub fn get_unit(&self, token_id: &TokenId) -> Unit {
        self.tokens
            .get(token_id)
            .map(|token| Unit::Known(token.clone()))
            .unwrap_or(Unit::Unknown(*token_id))
    }

    pub fn erg_unit(&self) -> Unit {
        self.get_unit(&Digest32::zero().into())
    }

    pub fn save(&self, path: Option<String>) -> Result<(), std::io::Error> {
        let path = path.unwrap_or("tokens.json".to_string());
        let file = std::fs::File::create(path)?;
        let writer = std::io::BufWriter::new(file);
        let tokens_vec = self.tokens.values().cloned().collect::<Vec<_>>();
        serde_json::to_writer_pretty(writer, &tokens_vec)?;
        Ok(())
    }

    pub fn load(path: Option<String>) -> Result<Self, std::io::Error> {
        let path = path.unwrap_or("tokens.json".to_string());
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let tokens_vec: Vec<TokenInfo> = serde_json::from_reader(reader)?;
        Ok(Self::with_tokens(tokens_vec))
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
            price1 in  any::<u64>(),
            price2 in  any::<u64>(),
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

        let price = Price::new(unit1.clone(), unit2.clone(), Fraction::new(1u64, 13u64));

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

        let unit1 = Unit::Known(TokenInfo {
            token_id: Digest32::zero().into(),
            name: "A".to_string(),
            decimals: 9,
        });
        let unit2 = Unit::Unknown(Digest::<32>(token_bytes).into());

        let price = Price::new(unit1.clone(), unit2.clone(), Fraction::new(1u64, 13u64));

        let unit_amount = UnitAmount::new(unit1, amount);
        let unit_amount2 = price.convert_price(&unit_amount).unwrap();

        assert_eq!(unit_amount2.unit, unit2);
        assert_eq!(unit_amount2.amount(), 2000 / 13);
    }

    fn convert_price(decimals1: u32, decimals2: u32, price1: u64, price2: u64, amount: u64) {
        let mut token_bytes = [0u8; 32];
        token_bytes[0] = 1;

        let unit1 = Unit::Known(TokenInfo {
            token_id: Digest::<32>(token_bytes).into(),
            name: "A".to_string(),
            decimals: decimals1,
        });

        let unit2 = Unit::Known(TokenInfo {
            token_id: Digest32::zero().into(),
            name: "B".to_string(),
            decimals: decimals2,
        });

        let price = Price::new(unit1.clone(), unit2.clone(), Fraction::new(price1, price2));

        let unit_amount = UnitAmount::new(unit1.clone(), amount);
        let unit_amount2 = price
            .convert_price(&unit_amount)
            .expect("price conversion failed");
        let unit_amount3 = price
            .indirect()
            .convert_price(&unit_amount2)
            .expect("price conversion failed");

        assert_eq!(unit_amount3.unit, unit1);
        assert_eq!(unit_amount2.unit, unit2);
    }
}
