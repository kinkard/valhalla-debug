//! Requests to the Valhalla service at `--valhalla-url`.

use std::time::Instant;

use axum::{
    Json,
    body::Bytes,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use prost::Message;
use serde_json::Value;
use tracing::info;
use valhalla::proto;

use crate::AppState;

/// Error from Valhalla or from talking to it, rendered as `{"error": "..."}` with Valhalla's status.
#[derive(Debug)]
pub struct Error {
    pub status: StatusCode,
    pub message: String,
    /// Valhalla's `error_code`.
    pub code: Option<u64>,
}

impl Error {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            code: None,
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let error = self.to_string();
        (self.status, Json(serde_json::json!({ "error": error }))).into_response()
    }
}

/// `<message> (error_code <n>)`.
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(code) = self.code {
            write!(f, " (error_code {code})")?;
        }
        Ok(())
    }
}

impl AppState {
    /// POSTs `api` as protobuf, the only format with `pbf_field_selector`; non-2xx answers become an [`Error`].
    pub async fn post(&self, endpoint: &str, api: &proto::Api) -> Result<Bytes, Error> {
        let url = format!("{}/{endpoint}", self.valhalla_url);
        let gateway = |message: String| Error::new(StatusCode::BAD_GATEWAY, message);
        let begin = Instant::now();
        let response = self
            .http_client
            .post(&url)
            .header(header::CONTENT_TYPE, "application/x-protobuf")
            .body(api.encode_to_vec())
            .send()
            .await
            .map_err(|err| {
                gateway(format!(
                    "Failed to reach Valhalla at {}: {err}",
                    self.valhalla_url
                ))
            })?;
        let status = response.status();
        let bytes = (response.bytes().await)
            .map_err(|err| gateway(format!("Failed to read the Valhalla response: {err}")))?;
        info!(
            "POST {url} -> {status} {}B in {}ms",
            bytes.len(),
            begin.elapsed().as_millis()
        );

        if !status.is_success() {
            // Valhalla errors look like {"error_code":171,"error":"No suitable edges near location",...},
            // or are an `Api` with `info.errors` for format=pbf
            let (message, code) = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|v| {
                    let error = v.get("error")?.as_str()?.to_owned();
                    Some((error, v.get("error_code").and_then(Value::as_u64)))
                })
                .or_else(|| {
                    let info = proto::Api::decode(bytes.as_ref()).ok()?.info?;
                    let error = info.errors.into_iter().next()?;
                    Some((error.description, Some(error.code)))
                })
                .unwrap_or_else(|| (String::from_utf8_lossy(&bytes).into_owned(), None));
            return Err(Error {
                status: StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                message,
                code,
            });
        }
        Ok(bytes)
    }
}
