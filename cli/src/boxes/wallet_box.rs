use ergo_lib::{
    ergotree_ir::chain::{ergo_box::{box_value::BoxValue, BoxTokens, ErgoBox, BoxId}, address::Address},
    wallet::box_selector::{ErgoBoxAssets, ErgoBoxId},
};

use crate::units::{TokenStore, UnitAmount, ERG_UNIT};

use super::describe_box::{BoxAssetDisplay, ErgoBoxDescriptors};

#[derive(Clone)]
pub struct WalletBox<T: ErgoBoxAssets>
{
    pub assets: T,
    pub address: Address,
}

impl<T> WalletBox<T>
where
    T: ErgoBoxAssets + Clone,
{
    pub fn new(assets: T, address: Address) -> Self {
        Self {
            assets,
            address,
        }
    }
}

impl<T> ErgoBoxAssets for WalletBox<T>
where
    T: ErgoBoxAssets,
{
    fn value(&self) -> BoxValue {
        self.assets.value()
    }

    fn tokens(&self) -> Option<BoxTokens> {
        self.assets.tokens()
    }
}

impl<T> ErgoBoxDescriptors for WalletBox<T>
where
    T: ErgoBoxAssets,
{
    fn box_name(&self) -> String {
        "Wallet".to_string()
    }

    fn assets<'a>(&self, _: &'a TokenStore) -> BoxAssetDisplay<'a> {
        let amount = UnitAmount::new(*ERG_UNIT, *self.value().as_u64());
        let num_tokens = self.tokens().map(|tokens| tokens.len()).unwrap_or(0);

        BoxAssetDisplay::Many(amount, num_tokens)
    }
}

impl ErgoBoxId for WalletBox<ErgoBox> {
    fn box_id(&self) -> BoxId {
        self.assets.box_id()
    }
}
