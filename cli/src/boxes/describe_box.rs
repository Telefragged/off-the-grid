use std::fmt::Display;

use crate::units::{TokenStore, UnitAmount};

pub enum BoxAssetDisplay<'a> {
    Single(UnitAmount<'a>),
    Double(UnitAmount<'a>, UnitAmount<'a>),
    Many(UnitAmount<'a>, usize),
}

impl BoxAssetDisplay<'_> {
    pub fn strings(&self, precision: Option<usize>) -> (String, String) {
        let first = match self {
            BoxAssetDisplay::Single(amount) => amount,
            BoxAssetDisplay::Double(amount, _) => amount,
            BoxAssetDisplay::Many(amount, _) => amount,
        };

        let first_str = match precision {
            Some(p) => format!("{:.p$}", first),
            None => first.to_string(),
        };

        let second = match (self, precision) {
            (BoxAssetDisplay::Double(_, amount), Some(p)) => format!("{:.p$}", amount, p = p),
            (BoxAssetDisplay::Double(_, amount), None) => amount.to_string(),
            _ => String::new(),
        };

        (first_str, second)
    }
}

impl Display for BoxAssetDisplay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let width = f.width().unwrap_or(32);

        match self {
            BoxAssetDisplay::Single(amount) => write!(f, "{:>width$} {:>width$}", amount, ""),
            BoxAssetDisplay::Double(amount1, amount2) => {
                write!(f, "{:>width$} {:>width$}", amount1, amount2)
            }
            BoxAssetDisplay::Many(amount, num) => {
                write!(f, "{:>width$}", amount)?;
                match num {
                    0 => write!(f, " {:>width$}", ""),
                    1 => write!(f, " {:>width$}", "1 token"),
                    tokens => write!(f, " {:>width$}", format!(" {} tokens", tokens)),
                }
            }
        }
    }
}

pub trait ErgoBoxDescriptors {
    fn box_name(&self) -> String;

    fn assets<'a>(&self, tokens: &'a TokenStore) -> BoxAssetDisplay<'a>;
}
