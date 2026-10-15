//! Wish-list acquisition: the fixed voltron endpoint and the user's local files.
use crate::clarity::read_response;
use destiny_core::wishlist::{VOLTRON_URL, WishlistFetch, WishlistGateway, WishlistSource};
use destiny_core::{AppError, ErrorCategory};
use std::{fs::File, io::Read, path::Path, time::Duration};

/// Voltron is about 28.5 MB; anything past this bound is refused whole.
pub const MAX_WISHLIST_BYTES: usize = 64 * 1024 * 1024;
const MAX_ETAG_CHARS: usize = 256;

fn unavailable() -> AppError {
    AppError::new(ErrorCategory::ProviderUnavailable)
}

/// UTF-8 text without a leading byte-order mark; invalid bytes refuse the list.
fn decode_text(bytes: Vec<u8>) -> Result<String, AppError> {
    let text = String::from_utf8(bytes).map_err(|_| unavailable())?;
    Ok(match text.strip_prefix('\u{feff}') {
        Some(stripped) => stripped.to_owned(),
        None => text,
    })
}

pub trait VoltronTransport {
    /// Sends `etag` as `If-None-Match` when present.
    fn get(&mut self, etag: Option<&str>) -> Result<WishlistFetch, AppError>;
}
impl<T: VoltronTransport + ?Sized> VoltronTransport for Box<T> {
    fn get(&mut self, etag: Option<&str>) -> Result<WishlistFetch, AppError> {
        (**self).get(etag)
    }
}

/// Stands in when no HTTP client could be built: voltron is unavailable, while
/// local lists, which need no network, still sync.
pub struct UnavailableVoltron;
impl VoltronTransport for UnavailableVoltron {
    fn get(&mut self, _: Option<&str>) -> Result<WishlistFetch, AppError> {
        Err(unavailable())
    }
}

pub struct HttpVoltronTransport {
    client: reqwest::blocking::Client,
}
impl HttpVoltronTransport {
    pub fn new() -> Result<Self, AppError> {
        let client = reqwest::blocking::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| unavailable())?;
        Ok(Self { client })
    }
}
impl VoltronTransport for HttpVoltronTransport {
    fn get(&mut self, etag: Option<&str>) -> Result<WishlistFetch, AppError> {
        let mut request = self.client.get(VOLTRON_URL);
        if let Some(etag) = etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        let response = request.send().map_err(|_| unavailable())?;
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value: &reqwest::header::HeaderValue| value.to_str().ok())
                .map(str::to_owned)
        };
        let content_type = header(reqwest::header::CONTENT_TYPE);
        let response_etag = header(reqwest::header::ETAG);
        voltron_response(
            response.status().as_u16(),
            etag.is_some(),
            response_etag,
            content_type.as_deref(),
            response.content_length(),
            response,
        )
    }
}

/// `304` counts only for a conditional request; a malformed ETag is dropped,
/// which only costs one unconditional download on the next sync.
fn voltron_response(
    status: u16,
    conditional: bool,
    etag: Option<String>,
    content_type: Option<&str>,
    content_length: Option<u64>,
    body: impl Read,
) -> Result<WishlistFetch, AppError> {
    if status == 304 {
        return if conditional {
            Ok(WishlistFetch::NotModified)
        } else {
            Err(unavailable())
        };
    }
    let bytes = read_response(
        status,
        content_type,
        content_length,
        body,
        MAX_WISHLIST_BYTES,
    )?;
    let etag = etag.filter(|etag| {
        !etag.is_empty()
            && etag.len() <= MAX_ETAG_CHARS
            && etag.bytes().all(|byte| byte.is_ascii_graphic())
    });
    Ok(WishlistFetch::Content {
        text: decode_text(bytes)?,
        etag,
    })
}

/// Reads one of the user's DIM-format files, bounded like the remote list.
pub fn read_local_wishlist(path: &Path) -> Result<String, AppError> {
    let file = File::open(path).map_err(|_| unavailable())?;
    if !file.metadata().map_err(|_| unavailable())?.is_file() {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    file.take(MAX_WISHLIST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() > MAX_WISHLIST_BYTES {
        return Err(unavailable());
    }
    decode_text(bytes)
}

pub struct WishlistAdapter<T>(pub T);
impl<T: VoltronTransport> WishlistGateway for WishlistAdapter<T> {
    fn fetch(
        &mut self,
        source: &WishlistSource,
        etag: Option<&str>,
    ) -> Result<WishlistFetch, AppError> {
        match source {
            WishlistSource::Voltron => self.0.get(etag),
            WishlistSource::Local(path) => Ok(WishlistFetch::Content {
                text: read_local_wishlist(path)?,
                etag: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn respond(
        status: u16,
        conditional: bool,
        etag: Option<&str>,
    ) -> Result<WishlistFetch, AppError> {
        voltron_response(
            status,
            conditional,
            etag.map(str::to_owned),
            Some("text/plain; charset=utf-8"),
            None,
            &b"dimwishlist:item=1\n"[..],
        )
    }

    #[test]
    fn not_modified_is_accepted_only_for_conditional_requests() {
        assert_eq!(
            respond(304, true, None).unwrap(),
            WishlistFetch::NotModified
        );
        assert!(respond(304, false, None).is_err());
        for status in [301, 404, 500] {
            assert!(respond(status, true, Some("\"x\"")).is_err());
        }
    }

    #[test]
    fn content_keeps_only_a_well_formed_etag() {
        let content = |etag: Option<&str>| match respond(200, false, etag).unwrap() {
            WishlistFetch::Content { text, etag } => (text, etag),
            WishlistFetch::NotModified => panic!("expected content"),
        };
        assert_eq!(
            content(Some("W/\"abc\"")),
            (
                "dimwishlist:item=1\n".to_owned(),
                Some("W/\"abc\"".to_owned())
            )
        );
        let long = "x".repeat(MAX_ETAG_CHARS + 1);
        for bad in ["", "has space", long.as_str()] {
            assert_eq!(content(Some(bad)).1, None, "{bad}");
        }
        assert!(voltron_response(200, false, None, Some("text/html"), None, &b"x"[..]).is_err());
    }
}
