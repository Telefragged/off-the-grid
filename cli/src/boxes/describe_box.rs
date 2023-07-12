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
            (BoxAssetDisplay::Many(_, num), _) => {
                match num {
                    0 => String::new(),
                    1 => "1 token".to_string(),
                    tokens => format!("{} tokens", tokens),
                }
            }
            _ => String::new(),
        };

        (first_str, second)
    }
}

pub trait ErgoBoxDescriptors {
    fn box_name(&self) -> String;

    fn assets<'a>(&self, tokens: &'a TokenStore) -> BoxAssetDisplay<'a>;
}
