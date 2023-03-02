use ergo_lib::ergotree_ir::chain::ergo_box::ErgoBox;

use crate::node::client::NodeClient;

use super::{client::ErgoNodeError, wallet::WalletBox};

impl NodeClient {
    pub async fn get_scan_unspent(&self, scan_id: i32) -> Result<Vec<ErgoBox>, ErgoNodeError> {
        let path = format!("scan/unspentBoxes/{scan_id}");
        let result: Vec<WalletBox> = self.request_get(&path).await?;
        Ok(result.into_iter().map(|wb| wb.ergo_box).collect())
    }
}
