use ergo_lib::ergotree_ir::chain::ergo_box::ErgoBox;
use std::{
    hash::{Hash, Hasher},
    ops::Deref,
};

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

impl<T> Deref for TrackedBox<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
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
