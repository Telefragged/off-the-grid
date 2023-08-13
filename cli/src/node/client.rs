use reqwest::{
    header::{HeaderMap, HeaderValue, InvalidHeaderValue},
    Client, ClientBuilder, RequestBuilder, Url,
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
        write!(f, "{} ({}): {}", self.reason, self.error, self.detail)
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

    #[error("Reqwest error: {reqwest_error} at {request_url}")]
    ReqwestErrorPath {
        reqwest_error: reqwest::Error,
        request_url: String,
    },

    #[error("API error: {api_error} at {request_url}")]
    ApiError {
        api_error: ApiError,
        request_url: String,
    },
}

pub struct NodeClient {
    client: Client,
    base_url: Url,
}

async fn send_request<T>(request: RequestBuilder, request_url: String) -> Result<T, ErgoNodeError>
where
    for<'a> T: Deserialize<'a> + Debug,
{
    let response_result = request.send().await;

    let response = match response_result {
        Ok(x) => x,
        Err(error) => return Err(ErgoNodeError::ReqwestErrorPath { reqwest_error: error, request_url }),
    };

    let parsed_result = response.json::<ApiResponse<T>>().await;

    let parsed = match parsed_result {
        Ok(x) => x,
        Err(error) => return Err(ErgoNodeError::ReqwestErrorPath { reqwest_error: error, request_url }),
    };

    match parsed {
        ApiResponse::Ok(t) => Ok(t),
        ApiResponse::Err(api_error) => Err(ErgoNodeError::ApiError {
            api_error,
            request_url,
        }),
    }
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

        send_request(self.client.get(&request_url), request_url).await
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

        send_request(self.client.post(&request_url).json(body), request_url).await
    }
}
