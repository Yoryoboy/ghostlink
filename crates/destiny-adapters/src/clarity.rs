//! Credential-free, fixed-endpoint community data acquisition.
use destiny_core::{AppError, ClarityBatch, ClarityGateway, ClarityInsight, ErrorCategory};
use serde::Deserialize;
use std::{collections::BTreeMap, io::Read, time::Duration};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClarityResource {
    Versions,
    Descriptions,
}
impl ClarityResource {
    pub fn url(self) -> &'static str {
        match self {
            Self::Versions => {
                "https://raw.githubusercontent.com/Database-Clarity/Live-Clarity-Database/live/versions.json"
            }
            Self::Descriptions => {
                "https://raw.githubusercontent.com/Database-Clarity/Live-Clarity-Database/live/descriptions/vaultZero.json"
            }
        }
    }
    pub fn limit(self) -> usize {
        match self {
            Self::Versions => 4096,
            Self::Descriptions => 16 * 1024 * 1024,
        }
    }
}
fn unavailable() -> AppError {
    AppError::new(ErrorCategory::ProviderUnavailable)
}

pub trait ClarityTransport {
    fn get(&mut self, resource: ClarityResource) -> Result<Vec<u8>, AppError>;
}
impl<T: ClarityTransport + ?Sized> ClarityTransport for Box<T> {
    fn get(&mut self, resource: ClarityResource) -> Result<Vec<u8>, AppError> {
        (**self).get(resource)
    }
}
pub struct HttpClarityTransport {
    client: reqwest::blocking::Client,
}
impl HttpClarityTransport {
    pub fn new() -> Result<Self, AppError> {
        let client = reqwest::blocking::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| unavailable())?;
        Ok(Self { client })
    }
}
impl ClarityTransport for HttpClarityTransport {
    fn get(&mut self, resource: ClarityResource) -> Result<Vec<u8>, AppError> {
        let response = self
            .client
            .get(resource.url())
            .send()
            .map_err(|_| unavailable())?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        read_response(
            response.status().as_u16(),
            content_type.as_deref(),
            response.content_length(),
            response,
            resource.limit(),
        )
    }
}
fn read_response(
    status: u16,
    content_type: Option<&str>,
    content_length: Option<u64>,
    body: impl Read,
    limit: usize,
) -> Result<Vec<u8>, AppError> {
    let media_type = content_type
        .and_then(|s| s.split(';').next())
        .map(str::trim);
    if status != 200
        || !matches!(media_type, Some("text/plain" | "application/json"))
        || content_length.is_some_and(|n| n > limit as u64)
    {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    body.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() > limit {
        return Err(unavailable());
    }
    Ok(bytes)
}

pub struct ClarityAdapter<T>(pub T);
impl<T: ClarityTransport> ClarityGateway for ClarityAdapter<T> {
    fn version(&mut self) -> Result<String, AppError> {
        decode_version(&self.0.get(ClarityResource::Versions)?)
    }
    fn descriptions(&mut self) -> Result<ClarityBatch, AppError> {
        decode_descriptions(&self.0.get(ClarityResource::Descriptions)?)
    }
}

/// RawValue retains the numeric token, including trailing zeros and exponent.
pub fn decode_version(bytes: &[u8]) -> Result<String, AppError> {
    #[derive(Deserialize)]
    struct Versions<'a> {
        #[serde(borrow)]
        descriptions: &'a serde_json::value::RawValue,
    }
    if bytes.len() > ClarityResource::Versions.limit() {
        return Err(unavailable());
    }
    let versions: Versions<'_> = serde_json::from_slice(bytes).map_err(|_| unavailable())?;
    let token = versions.descriptions.get();
    if token.len() > 128 || !token.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(unavailable());
    }
    Ok(token.to_owned())
}

pub fn decode_descriptions(bytes: &[u8]) -> Result<ClarityBatch, AppError> {
    if bytes.len() > ClarityResource::Descriptions.limit() {
        return Err(unavailable());
    }
    let entries: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(bytes).map_err(|_| unavailable())?;
    if entries.len() > 20_000 {
        return Err(unavailable());
    }
    let mut batch = ClarityBatch {
        entries: Vec::new(),
        skipped: 0,
    };
    for (key, value) in entries {
        match decode_entry(&key, value) {
            Some(entry) => batch.entries.push(entry),
            None => batch.skipped += 1,
        }
    }
    Ok(batch)
}
fn decode_entry(key: &str, value: serde_json::Value) -> Option<ClarityInsight> {
    let hash = key.parse::<u32>().ok()?;
    if hash.to_string() != key || value.get("hash")?.as_u64()? != u64::from(hash) {
        return None;
    }
    let text = value.get("descriptions")?.get("en")?.as_str()?;
    if text.trim().is_empty()
        || text.chars().count() > 8192
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return None;
    }
    let item_hash = match value.get("itemHash") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(u32::try_from(v.as_u64()?).ok()?),
    };
    let item_name = match value.get("itemName") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(v.as_str()?.to_owned()),
    };
    Some(ClarityInsight {
        hash,
        name: value.get("name")?.as_str()?.to_owned(),
        kind: value.get("type")?.as_str()?.to_owned(),
        item_hash,
        item_name,
        text: text.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_response_policy_bounds_headers_and_stream() {
        for status in [301, 401, 500] {
            assert!(read_response(status, Some("text/plain"), None, &b"{}"[..], 4).is_err());
        }
        for content_type in [None, Some("text/html"), Some("application/octet-stream")] {
            assert!(read_response(200, content_type, None, &b"{}"[..], 4).is_err());
        }
        assert!(read_response(200, Some("text/plain"), Some(5), &b"{}"[..], 4).is_err());
        assert!(read_response(200, Some("text/plain"), None, &b"12345"[..], 4).is_err());
        assert_eq!(
            read_response(
                200,
                Some("text/plain; charset=utf-8"),
                None,
                &b"1234"[..],
                4
            )
            .unwrap(),
            b"1234"
        );
        assert!(read_response(200, Some("application/json"), Some(2), &b"{}"[..], 4).is_ok());
    }
}
