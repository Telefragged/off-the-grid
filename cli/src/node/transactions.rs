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

    pub async fn transaction_unconfirmed(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Transaction>, ErgoNodeError> {
        let path = format!("transactions/unconfirmed?limit={}&offset={}", limit, offset);
        let result = self.request_get(&path).await?;
        Ok(result)
    }

    pub async fn transaction_unconfirmed_all(&self) -> Result<Vec<Transaction>, ErgoNodeError> {
        const STEP: u32 = 100;

        let mut result = vec![];
        let mut offset = 0;
        loop {
            let txs = self.transaction_unconfirmed(STEP, offset).await?;
            let tx_len = txs.len();
            result.extend(txs);
            if tx_len < STEP as usize {
                break;
            }
            offset += STEP;
        }
        Ok(result)
    }
}
