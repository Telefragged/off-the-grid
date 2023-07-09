use ergo_lib::{
    ergotree_ir::chain::ergo_box::{box_value::BoxValue, BoxTokens},
    wallet::box_selector::ErgoBoxAssets,
};

use crate::units::{TokenStore, UnitAmount, ERG_UNIT};

use super::describe_box::{BoxAssetDisplay, ErgoBoxDescriptors};

#[derive(Clone)]
pub struct WalletBox<T: ErgoBoxAssets>(pub T);

impl<T> WalletBox<T>
where
    T: ErgoBoxAssets + Clone,
{
    pub fn new(assets: T) -> Self {
        Self(assets)
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> ErgoBoxAssets for WalletBox<T>
where
    T: ErgoBoxAssets,
{
    fn value(&self) -> BoxValue {
        self.0.value()
    }

    fn tokens(&self) -> Option<BoxTokens> {
        self.0.tokens()
    }
}

impl<T> ErgoBoxDescriptors for WalletBox<T>
where
    T: ErgoBoxAssets,
{
    fn box_name(&self) -> String {
        "Wallet".to_string()
    }

    fn assets(&self, _: &TokenStore) -> super::describe_box::BoxAssetDisplay {
        let amount = UnitAmount::new(ERG_UNIT.clone(), *self.value().as_u64());
        let num_tokens = self.tokens().map(|tokens| tokens.len()).unwrap_or(0);

        BoxAssetDisplay::Many(amount, num_tokens)
    }
}
