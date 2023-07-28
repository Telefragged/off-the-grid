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

    fn assets<'a>(&self, token_store: &'a TokenStore) -> BoxAssetDisplay<'a> {
        let amount = UnitAmount::new(*ERG_UNIT, *self.value().as_u64());

        match self.tokens().as_ref().map(|tokens| tokens.as_slice()) {
            None => {
                return BoxAssetDisplay::Single(amount);
            }
            Some([token]) => {
                let unit = token_store.get_unit(&token.token_id);

                let token_amount = UnitAmount::new(unit, *token.amount.as_u64());
                return BoxAssetDisplay::Double(amount, token_amount);
            }
            Some(tokens) => BoxAssetDisplay::Many(amount, tokens.len()),
        }
    }
}

impl ErgoBoxId for WalletBox<ErgoBox> {
    fn box_id(&self) -> BoxId {
        self.assets.box_id()
    }
}
