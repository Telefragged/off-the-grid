use ergo_lib::{ergotree_ir::chain::ergo_box::ErgoBox, wallet::box_selector::ErgoBoxId};
use std::hash::{Hash, Hasher};

use crate::units::TokenStore;

use super::describe_box::{BoxAssetDisplay, ErgoBoxDescriptors};

#[derive(Clone)]
pub struct TrackedBox<T> {
    pub ergo_box: ErgoBox,
    pub value: T,
}

impl<T, E> TryFrom<ErgoBox> for TrackedBox<T>
where
    for<'a> T: TryFrom<&'a ErgoBox, Error = E>,
{
    type Error = E;

    fn try_from(ergo_box: ErgoBox) -> Result<Self, Self::Error> {
        let value = T::try_from(&ergo_box)?;
        Ok(Self { ergo_box, value })
    }
}

impl<T, E> TryFrom<&ErgoBox> for TrackedBox<T>
where
    for<'a> T: TryFrom<&'a ErgoBox, Error = E>,
{
    type Error = E;

    fn try_from(ergo_box: &ErgoBox) -> Result<Self, Self::Error> {
        let value = T::try_from(ergo_box)?;
        Ok(Self {
            ergo_box: ergo_box.clone(),
            value,
        })
    }
}

impl<T> PartialEq for TrackedBox<T> {
    fn eq(&self, other: &Self) -> bool {
        self.ergo_box.box_id() == other.ergo_box.box_id()
    }
}

impl<T> Eq for TrackedBox<T> {}

impl<T> Hash for TrackedBox<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ergo_box.box_id().hash(state);
    }
}

impl<T> ErgoBoxDescriptors for TrackedBox<T>
where
    T: ErgoBoxDescriptors,
{
    fn box_name(&self) -> String {
        self.value.box_name()
    }

    fn assets<'a>(&self, tokens: &'a TokenStore) -> BoxAssetDisplay<'a> {
        self.value.assets(tokens)
    }
}

impl<T> ErgoBoxId for TrackedBox<T> {
    fn box_id(&self) -> ergo_lib::ergotree_ir::chain::ergo_box::BoxId {
        self.ergo_box.box_id()
    }
}
