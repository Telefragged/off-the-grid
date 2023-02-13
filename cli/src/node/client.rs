use reqwest::{
    header::{HeaderMap, HeaderValue, InvalidHeaderValue},
    Client, ClientBuilder, Url,
};
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display};
use thiserror::Error;

#[derive(Serialize, Deserialize, Debug, Error)]
pub struct ApiError {
    error: i32,
    reason: String,
    detail: String,
}

impl Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum ApiResponse<T> {
    Ok(T),
    Err(ApiError),
}

#[derive(Error, Debug)]
pub enum ErgoNodeError {
    #[error("Invalid header value")]
    InvalidHeaderValue(#[from] InvalidHeaderValue),

    #[error(transparent)]
    ReqwestError(#[from] reqwest::Error),

    #[error(transparent)]
    ApiError(#[from] ApiError),
}

pub struct NodeClient {
    client: Client,
    base_url: Url,
}

impl NodeClient {
    pub fn new(base_url: Url, api_key: &[u8]) -> Result<Self, ErgoNodeError> {
        let mut headers = HeaderMap::new();
        headers.insert("api_key", HeaderValue::from_bytes(api_key)?);
        let client = ClientBuilder::new().default_headers(headers).build()?;

        Ok(Self { client, base_url })
    }

    pub(super) async fn request_get<T>(&self, path: &str) -> Result<T, ErgoNodeError>
    where
        for<'a> T: Deserialize<'a> + Debug,
    {
        let request_url = format!("{}{}", self.base_url, path);

        let parsed = self
            .client
            .get(request_url)
            .send()
            .await?
            .json::<ApiResponse<T>>()
            .await?;

        match parsed {
            ApiResponse::Ok(t) => Ok(t),
            ApiResponse::Err(api_error) => Err(api_error.into()),
        }
    }

    pub(super) async fn request_post<Req, Resp>(
        &self,
        path: &str,
        body: &Req,
    ) -> Result<Resp, ErgoNodeError>
    where
        for<'a> Resp: Deserialize<'a> + Debug,
        Req: Serialize,
    {
        let request_url = format!("{}{}", self.base_url, path);

        let parsed = self
            .client
            .post(request_url)
            .json(body)
            .send()
            .await?
            .json::<ApiResponse<Resp>>()
            .await?;

        match parsed {
            ApiResponse::Ok(t) => Ok(t),
            ApiResponse::Err(api_error) => Err(api_error.into()),
        }
    }
}
