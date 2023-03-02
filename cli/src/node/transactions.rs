use ergo_lib::chain::transaction::{Transaction, TxId};

use crate::node::client::NodeClient;

use super::client::ErgoNodeError;

impl NodeClient {
    pub async fn transaction_submit(
        &self,
        transaction: &Transaction,
    ) -> Result<TxId, ErgoNodeError> {
        let path = "transactions";
        let result = self.request_post(path, transaction).await?;
        Ok(result)
    }
}
