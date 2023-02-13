use base16::{decode, encode_lower};
use ergo_lib::ergotree_ir::chain::{ergo_box::ErgoBox, token::TokenId};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::node::client::NodeClient;

use super::{client::ErgoNodeError, wallet::WalletBox};

fn encode_base16<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&encode_lower(bytes))
}

fn decode_base16<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    decode(&s).map_err(serde::de::Error::custom)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WalletInteraction {
    Off,
    Shared,
    Forced,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", tag = "predicate")]
pub enum TrackingRule {
    ContainsAsset {
        #[serde(rename = "assetId")]
        asset_id: TokenId,
    },
    Contains {
        #[serde(serialize_with = "encode_base16", deserialize_with = "decode_base16")]
        value: Vec<u8>,
        register: String,
    },
    Equals {
        #[serde(serialize_with = "encode_base16", deserialize_with = "decode_base16")]
        value: Vec<u8>,
        register: String,
    },
    And {
        args: Vec<TrackingRule>,
    },
    Or {
        args: Vec<TrackingRule>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeScan {
    pub scan_name: String,
    pub scan_id: i32,
    pub tracking_rule: TrackingRule,
    pub wallet_interaction: WalletInteraction,
    pub remove_offchain: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateScanRequest {
    pub scan_name: String,
    pub tracking_rule: TrackingRule,
    pub wallet_interaction: WalletInteraction,
    pub remove_offchain: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateScanResponse {
    pub scan_id: i32,
}

impl NodeClient {
    pub async fn get_scan_unspent(&self, scan_id: i32) -> Result<Vec<ErgoBox>, ErgoNodeError> {
        let path = format!("scan/unspentBoxes/{scan_id}");
        let result: Vec<WalletBox> = self.request_get(&path).await?;
        Ok(result.into_iter().map(|wb| wb.ergo_box).collect())
    }

    pub async fn list_scans(&self) -> Result<Vec<NodeScan>, ErgoNodeError> {
        let path = "scan/listAll".to_string();
        let result: Vec<NodeScan> = self.request_get(&path).await?;
        Ok(result)
    }

    pub async fn create_scan(
        &self,
        create_scan_request: CreateScanRequest,
    ) -> Result<CreateScanResponse, ErgoNodeError> {
        let path = "scan/create".to_string();
        let result: CreateScanResponse = self.request_post(&path, &create_scan_request).await?;
        Ok(result)
    }
}
