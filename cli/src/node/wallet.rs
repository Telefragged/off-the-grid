use ergo_lib::{
    chain::transaction::{unsigned::UnsignedTransaction, Transaction},
    ergotree_ir::chain::{
        address::{Address, AddressEncoder, NetworkPrefix},
        ergo_box::ErgoBox,
    },
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::boxes::wallet_box::WalletBox;
use crate::node::client::NodeClient;

use super::client::ErgoNodeError;

#[derive(Deserialize, Debug)]
pub(super) struct ApiWalletBox {
    #[serde(rename = "box")]
    pub ergo_box: ErgoBox,
}

#[derive(Serialize)]
struct SignTransactionRequest {
    tx: UnsignedTransaction,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct WalletStatusDto {
    is_initialized: bool,
    is_unlocked: bool,
    change_address: String,
    wallet_height: i32,
    error: String,
}

#[derive(Error, Debug)]
pub enum WalletStatusError {
    // #[error("Wallet not initialized")]
    // WalletNotInitialized,
    #[error("Wallet is locked")]
    WalletLocked,

    #[error("No change address")]
    NoChangeAddress,
}

pub struct WalletStatus {
    pub is_initialized: bool,
    pub is_unlocked: bool,
    pub change_address: Option<Address>,
    pub wallet_height: i32,
    pub error: String,
}

impl WalletStatus {
    pub fn error_if_locked(&self) -> Result<(), WalletStatusError> {
        if self.is_unlocked {
            Ok(())
        } else {
            Err(WalletStatusError::WalletLocked)
        }
    }

    pub fn change_address(&self) -> Result<Address, WalletStatusError> {
        self.change_address
            .clone()
            .ok_or(WalletStatusError::NoChangeAddress)
    }
}

impl NodeClient {
    pub async fn wallet_boxes_unspent(&self) -> Result<Vec<WalletBox<ErgoBox>>, ErgoNodeError> {
        let path = "wallet/boxes/unspent";

        let boxes: Vec<ApiWalletBox> = self.request_get(path).await?;

        Ok(boxes
            .into_iter()
            .map(|wb| WalletBox::new(wb.ergo_box))
            .collect())
    }

    pub async fn wallet_transaction_sign(
        &self,
        unsigned_tx: &UnsignedTransaction,
    ) -> Result<Transaction, ErgoNodeError> {
        let path = "wallet/transaction/sign";
        let body = SignTransactionRequest {
            tx: unsigned_tx.clone(),
        };

        let result = self.request_post(path, &body).await?;
        Ok(result)
    }
    pub async fn wallet_status(&self) -> Result<WalletStatus, ErgoNodeError> {
        let path = "wallet/status";
        let result: WalletStatusDto = self.request_get(path).await?;
        let change_address = AddressEncoder::new(NetworkPrefix::Mainnet)
            .parse_address_from_str(&result.change_address)
            .ok();

        Ok(WalletStatus {
            is_initialized: result.is_initialized,
            is_unlocked: result.is_unlocked,
            wallet_height: result.wallet_height,
            error: result.error,
            change_address,
        })
    }
}
