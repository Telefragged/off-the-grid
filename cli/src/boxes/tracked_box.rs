use ergo_lib::ergotree_ir::chain::ergo_box::ErgoBox;

#[derive(Clone)]
pub struct TrackedBox<T>
{
    pub ergo_box: ErgoBox,
    pub value: T,
}

impl<T, E> TryFrom<ErgoBox> for TrackedBox<T>
where
    for <'a> T: TryFrom<&'a ErgoBox, Error = E>,
{
    type Error = E;

    fn try_from(ergo_box: ErgoBox) -> Result<Self, Self::Error> {
        let value = T::try_from(&ergo_box)?;
        Ok(Self {
            ergo_box,
            value,
        })
    }
}
