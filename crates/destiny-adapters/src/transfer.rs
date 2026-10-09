//! Strict, credential-safe Bungie one-item transfer adapter.
//!
//! This module owns the transfer network-facing mutation path (equip has its
//! own exact allowlist in `equip`): one blocking
//! HTTPS `application/json` POST to the documented `TransferItem` action
//! endpoint, mapped from the provider-neutral [`TransferRequest`] and classified
//! into the committed [`TransferOutcome`] contract.
//!
//! Security posture:
//!
//! - one fixed HTTPS endpoint with rustls certificate verification;
//! - redirects disabled, proxies disabled, and no decompression features;
//! - a total request timeout plus a separate connect timeout;
//! - validated, redacted [`TransferApiKey`] and [`TransferBearerToken`] secrets;
//! - an exact URL allowlist evaluated before any secret value is read or
//!   attached (the request already holds opaque secret references; the allowlist
//!   prevents their values from being read or attached to a non-allowlisted
//!   request);
//! - a small, strict response-size cap enforced from `Content-Length` and from
//!   the actual bytes read;
//! - a strict `application/json` response media-type check with optional
//!   parameters, evaluated before any body is accepted as an envelope;
//! - diagnostics never expose credentials, provider bodies, provider messages,
//!   item instance identifiers, or character identifiers.
//!
//! # Result contract
//!
//! The core [`TransferGateway`] contract keeps uncertainty honest: `Err` is
//! reserved for failures in which no provider mutation can have occurred. This
//! adapter splits its failure surface exactly there:
//!
//! - `Err(AppError)` — an expired or rejected credential, a missing or invalid
//!   credential, an unrepresentable request, or a transport failure whose
//!   dispatch was never observed to begin. Nothing was mutated.
//! - `Ok(TransferOutcome::Uncertain)` — the request was dispatched but the
//!   effect could not be established: a lost response, an unclassifiable or
//!   malformed envelope, an unexpected status, or a body that could not be read
//!   under the cap. The step may or may not have applied.
//!
//! # Classification policy
//!
//! Bungie returns a standard `ErrorCode` / `ErrorStatus` envelope. A `200`
//! response is decoded as JSON and classified as:
//!
//! - `ErrorCode == 1` → [`TransferOutcome::Transferred`];
//! - `ErrorStatus` equal to one of the recorded transfer statuses
//!   `ItemIsEquipped`, `NotTransferrable`, or `NoRoomInDestination` →
//!   [`TransferOutcome::Rejected`] with the matching deterministic
//!   [`TransferRejection`];
//! - anything else, including a missing, mistyped, or unknown code/status, a
//!   non-JSON media type, or an unreadable or oversized body →
//!   [`TransferOutcome::Uncertain`].
//!
//! Non-`200` statuses are classified at the HTTP layer with the same honesty
//! rule: `401`/`403` is an authentication rejection observed after the request
//! was dispatched: [`TransferOutcome::AuthenticationRefused`] preserves the
//! attempted call without claiming any mutation; `429` is a [`TransferOutcome::Transient`] failure because
//! the throttle status itself is a deterministic provider refusal to process the
//! submitted request; `5xx` is [`TransferOutcome::Uncertain`] because a server
//! error on a dispatched mutation does not prove the mutation did not apply;
//! another `4xx` is a deterministic rejection of the submitted request; and
//! every other status is uncertain.
//!
//! Only the request-side wire shape is treated as stable here. The exact
//! response envelope is reconciled against live evidence in WU-015; until then
//! the classify-unknown-as-uncertain rule is what keeps a possibly-applied step
//! from ever being reported as a clean failure or a false success.

use destiny_core::{
    AppError, CharacterId, Clock, CredentialSource, ErrorCategory, ItemLocation,
    StaticCredentialSource, Timestamp, TransferGateway, TransferOutcome, TransferRejection,
    TransferRequest,
};
use serde_json::{Map, Value};
use std::fmt;
use std::io::Read;
use std::time::Duration;

/// The documented one-item transfer action endpoint.
///
/// The trailing slash is part of the documented URL and is required by the
/// exact allowlist in [`is_allowed_transfer_url`].
pub const TRANSFER_ENDPOINT: &str =
    "https://www.bungie.net/Platform/Destiny2/Actions/Items/TransferItem/";

/// Strict upper bound on one transfer response body. Transfer envelopes are
/// small; anything larger is treated as an unclassifiable response.
pub const TRANSFER_RESPONSE_LIMIT: usize = 8 * 1024;

/// The only success code Bungie returns for a fulfilled request.
const SUCCESS_ERROR_CODE: u64 = 1;

/// The longest decimal identifier the request contract accepts.
const MAX_IDENTIFIER_DIGITS: usize = 128;

const CONTENT_TYPE_JSON: &str = "application/json";
const ACCEPT_JSON: &str = "application/json";
const API_KEY_HEADER: &str = "x-api-key";
const AUTHORIZATION_HEADER: &str = "authorization";
const ACCEPT_HEADER: &str = "accept";
const CONTENT_TYPE_HEADER: &str = "content-type";

/// One total timeout covering connect, request, and body read.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// A tighter bound on the connection phase only.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

fn provider_validation() -> AppError {
    AppError::new(ErrorCategory::ProviderValidation)
}

fn provider_unavailable() -> AppError {
    AppError::new(ErrorCategory::ProviderUnavailable)
}

fn configuration() -> AppError {
    AppError::new(ErrorCategory::ConfigurationError)
}

/// Accepts only a non-empty string of visible ASCII (`0x21..=0x7e`).
///
/// Visible ASCII excludes space, every whitespace character, and every control
/// character, so a credential can never carry a blank byte or smuggle a header
/// separator into a request header value.
fn is_valid_secret(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

/// A validated Bungie API key.
///
/// The value is accepted only when it is non-empty visible ASCII, so it can
/// never smuggle a header-injection payload or a blank credential. The bytes
/// are reachable only through the trusted `expose` boundary and are never
/// rendered by [`fmt::Debug`].
pub struct TransferApiKey(String);

impl TransferApiKey {
    /// Validates a candidate API key.
    ///
    /// Empty, whitespace-bearing, control-bearing, or non-ASCII values are
    /// rejected as sanitized configuration failures.
    pub fn new(value: impl Into<String>) -> Result<Self, AppError> {
        let value = value.into();
        if is_valid_secret(&value) {
            return Ok(Self(value));
        }
        Err(configuration())
    }

    /// Exposes the secret to the trusted production transport only.
    fn expose(&self) -> &str {
        #[cfg(test)]
        CREDENTIAL_ACCESSES.with(|count| count.set(count.get() + 1));
        &self.0
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static CREDENTIAL_ACCESSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn record_request_credentials(request: &TransferHttpRequest<'_>) {
    request.headers().expect("credentials available");
}

impl fmt::Debug for TransferApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        destiny_core::redact::opaque(formatter, "TransferApiKey")
    }
}

/// A validated Bungie OAuth access token used as a Bearer credential.
///
/// It applies the same strict visible-ASCII rule as [`TransferApiKey`] and is
/// likewise value-redacted in [`fmt::Debug`] and reachable only through the
/// trusted `expose` boundary.
pub struct TransferBearerToken(String);

impl TransferBearerToken {
    /// Validates a candidate access token.
    ///
    /// Empty, whitespace-bearing, control-bearing, or non-ASCII values are
    /// rejected as sanitized configuration failures.
    pub fn new(value: impl Into<String>) -> Result<Self, AppError> {
        let value = value.into();
        if is_valid_secret(&value) {
            return Ok(Self(value));
        }
        Err(configuration())
    }

    /// Exposes the secret to the trusted production transport only.
    fn expose(&self) -> &str {
        #[cfg(test)]
        CREDENTIAL_ACCESSES.with(|count| count.set(count.get() + 1));
        &self.0
    }
}

impl fmt::Debug for TransferBearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        destiny_core::redact::opaque(formatter, "TransferBearerToken")
    }
}

/// Accepts only the fixed one-item transfer action URL.
///
/// This predicate is the pre-flight gate: the production transport evaluates it
/// before it reads or attaches any credential value, so a rejected URL never
/// receives a secret. The request already holds opaque secret references; the
/// allowlist prevents their values from being read or attached, not the
/// references themselves from being carried.
pub fn is_allowed_transfer_url(url: &str) -> bool {
    url == TRANSFER_ENDPOINT
}

/// The exact one-item transfer POST this adapter asks a transport to send.
///
/// The API key and Bearer token are carried by reference and are never rendered
/// by [`fmt::Debug`]. The URL is always validated against the allowlist by the
/// transport before any secret is attached.
pub struct TransferHttpRequest<'a> {
    url: &'a str,
    body: String,
    api_key: Option<&'a TransferApiKey>,
    bearer: Option<&'a TransferBearerToken>,
}

impl<'a> TransferHttpRequest<'a> {
    pub fn new(
        url: &'a str,
        body: String,
        api_key: Option<&'a TransferApiKey>,
        bearer: Option<&'a TransferBearerToken>,
    ) -> Self {
        Self {
            url,
            body,
            api_key,
            bearer,
        }
    }

    pub fn url(&self) -> &'a str {
        self.url
    }

    /// The exact JSON request body.
    pub fn body(&self) -> &str {
        &self.body
    }

    /// The trusted API-key accessor. The key value is never rendered.
    pub fn api_key(&self) -> Option<&'a TransferApiKey> {
        self.api_key
    }

    /// The trusted Bearer accessor. The token value is never rendered.
    pub fn bearer(&self) -> Option<&'a TransferBearerToken> {
        self.bearer
    }

    /// The exact ordered header set for the one action POST.
    ///
    /// The production transport and the contract tests share this builder so the
    /// header contract has one source of truth. `None` means a required secret
    /// was absent, which is a configuration failure.
    fn headers(&self) -> Option<Vec<(&'static str, String)>> {
        let api_key = self.api_key?;
        let bearer = self.bearer?;
        Some(vec![
            (CONTENT_TYPE_HEADER, CONTENT_TYPE_JSON.to_owned()),
            (ACCEPT_HEADER, ACCEPT_JSON.to_owned()),
            (API_KEY_HEADER, api_key.expose().to_owned()),
            (AUTHORIZATION_HEADER, format!("Bearer {}", bearer.expose())),
        ])
    }
}

impl fmt::Debug for TransferHttpRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let endpoint = match self.url {
            TRANSFER_ENDPOINT => "TransferItem",
            crate::equip::EQUIP_ENDPOINT => "EquipItem",
            crate::lock::LOCK_ENDPOINT => "SetLockState",
            _ => "<unallowlisted>",
        };
        formatter
            .debug_struct("TransferHttpRequest")
            .field("method", &"POST")
            .field("endpoint", &endpoint)
            .field("body", &destiny_core::redact::REDACTED)
            .field(
                "api_key",
                &destiny_core::redact::redact_optional(self.api_key),
            )
            .field(
                "bearer",
                &destiny_core::redact::redact_optional(self.bearer),
            )
            .finish()
    }
}

/// A transport response with a bounded, streaming body.
///
/// The body is a reader so the adapter can enforce the response-size cap
/// without trusting the transport to buffer everything first.
pub struct TransferHttpResponse {
    status: u16,
    content_length: Option<u64>,
    content_type: Option<String>,
    body: Box<dyn Read>,
    throttle: Option<std::sync::Arc<crate::bungie_throttle::BungieThrottle>>,
}

impl TransferHttpResponse {
    /// Builds a response for a transport implementation or contract test.
    ///
    /// `content_type` is the raw upstream `Content-Type` header value. It is
    /// only inspected to accept or reject the response; it is never rendered by
    /// [`fmt::Debug`] and never reaches an error or diagnostic.
    pub fn new(
        status: u16,
        content_length: Option<u64>,
        content_type: Option<String>,
        body: Box<dyn Read>,
    ) -> Self {
        Self {
            status,
            content_length,
            content_type,
            body,
            throttle: None,
        }
    }

    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    /// Shared bounded JSON read policy for single-action mutation envelopes.
    pub(crate) fn json_body(&mut self, content_length: Option<u64>) -> Option<Vec<u8>> {
        if !self.content_type().is_some_and(is_json_content_type) {
            return None;
        }
        let bytes = read_bounded_body(&mut self.body, content_length)?;
        if let Some(throttle) = &self.throttle {
            throttle.observe_bytes(&bytes);
        }
        Some(bytes)
    }
}

impl fmt::Debug for TransferHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransferHttpResponse")
            .field("status", &self.status)
            .field("content_length", &self.content_length)
            .field(
                "content_type",
                &destiny_core::redact::redact_optional(self.content_type.as_ref()),
            )
            .field("body", &destiny_core::redact::REDACTED)
            .finish()
    }
}

/// A transport-level failure, categorized by whether dispatch was observed.
///
/// The variants are intentionally coarse so that no upstream text can escape,
/// and the pre-dispatch/indeterminate split is what lets the gateway keep the
/// mutation contract honest: only a failure in which dispatch was never observed
/// to begin may become an `Err`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferTransportError {
    /// Adaptive pacing refused before dispatch; no mutation was attempted.
    Throttled(u64),
    /// The connection could not be established, so no request bytes reached the
    /// provider and no mutation can have occurred.
    NotDispatched,
    /// The request was dispatched but no trustworthy response was observed: a
    /// timeout, a reset, or a body-transfer failure. The effect is unknown.
    Indeterminate,
    /// Local request construction or client configuration failure. The request
    /// was never dispatched.
    Configuration,
}

/// The seam between the transfer logic and an HTTP implementation.
///
/// Production uses [`HttpTransferTransport`]; tests inject a fake that records
/// the request and returns crafted responses.
pub trait TransferTransport {
    fn post(
        &mut self,
        request: &TransferHttpRequest<'_>,
    ) -> Result<TransferHttpResponse, TransferTransportError>;
}

/// The production blocking HTTPS transport.
///
/// Construction failures are reported as `ConfigurationError` and never surface
/// transport text. The client refuses redirects, ignores proxies, and bounds
/// both the total request and the connection phase.
pub struct HttpTransferTransport {
    client: reqwest::blocking::Client,
    throttle: std::sync::Arc<crate::bungie_throttle::BungieThrottle>,
}

impl HttpTransferTransport {
    /// Builds a hardened client for the fixed transfer endpoint.
    pub fn new() -> Result<Self, AppError> {
        // Hyper's canceled-request retry is pre-write, not an application post-dispatch retry.
        let client = Self::hardened_client_builder()
            .build()
            .map_err(|_| configuration())?;
        Ok(Self {
            client,
            throttle: crate::bungie_throttle::shared(),
        })
    }

    fn hardened_client_builder() -> reqwest::blocking::ClientBuilder {
        reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
    }

    /// Injects isolated pacing/time for deterministic transport tests.
    pub fn with_throttle(
        mut self,
        throttle: std::sync::Arc<crate::bungie_throttle::BungieThrottle>,
    ) -> Self {
        self.throttle = throttle;
        self
    }

    /// Pull-only policy: preserve encoded bytes and declared wire length for
    /// the bounded reader, and forbid reqwest retries regardless of features.
    /// The public transfer/equip/lock constructor retains its existing policy.
    pub(crate) fn new_instanced_pull() -> Result<Self, AppError> {
        let client = Self::hardened_client_builder()
            .no_gzip()
            .no_brotli()
            .no_zstd()
            .no_deflate()
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| configuration())?;
        Ok(Self {
            client,
            throttle: crate::bungie_throttle::shared(),
        })
    }
}

impl fmt::Debug for HttpTransferTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpTransferTransport")
            .finish_non_exhaustive()
    }
}

/// Confirms the request URL is on the fixed allowlist before a secret value is
/// read or attached.
///
/// The allowlist is evaluated before any credential value is read, so a rejected
/// URL can never carry a secret value. The request already holds the opaque
/// secret references; this gate stops their values from being read.
fn ensure_allowlisted(url: &str) -> Result<(), TransferTransportError> {
    if is_allowed_transfer_url(url) {
        return Ok(());
    }
    Err(TransferTransportError::Configuration)
}

impl TransferTransport for HttpTransferTransport {
    fn post(
        &mut self,
        request: &TransferHttpRequest<'_>,
    ) -> Result<TransferHttpResponse, TransferTransportError> {
        // The allowlist runs first so a rejected URL can never receive a secret.
        ensure_allowlisted(request.url())?;
        self.post_allowlisted(request)
    }
}

impl HttpTransferTransport {
    /// Shared wire machinery. Callers must enforce their action's exact allowlist
    /// before entering this trusted boundary (which reads credential values).
    pub(crate) fn post_allowlisted(
        &mut self,
        request: &TransferHttpRequest<'_>,
    ) -> Result<TransferHttpResponse, TransferTransportError> {
        self.throttle
            .before_request()
            .map_err(TransferTransportError::Throttled)?;
        let headers = request
            .headers()
            .ok_or(TransferTransportError::Configuration)?;

        let mut builder = self.client.post(request.url());
        for (name, value) in &headers {
            builder = builder.header(*name, value.as_str());
        }
        let response = builder
            .body(request.body().to_owned())
            .send()
            .map_err(classify_transport_error)?;

        let status = response.status().as_u16();
        let content_length = response.content_length();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Ok(TransferHttpResponse {
            status,
            content_length,
            content_type,
            body: Box::new(response),
            throttle: Some(self.throttle.clone()),
        })
    }
}

/// Classifies a `reqwest` send failure by whether dispatch was observed.
///
/// Only a confirmed connect failure is treated as pre-dispatch. Every other
/// failure after `send()` was called is conservative: the request may already
/// have reached the provider, so its effect is unknown.
fn classify_transport_error(error: reqwest::Error) -> TransferTransportError {
    if error.is_builder() {
        TransferTransportError::Configuration
    } else if error.is_connect() {
        TransferTransportError::NotDispatched
    } else {
        TransferTransportError::Indeterminate
    }
}

/// The production authenticated Bungie transfer gateway.
///
/// It owns the transport, the injected application clock, a credential source,
/// and the API key. Each request validates fetched material as a
/// [`TransferBearerToken`]; invalid material is a sanitized configuration failure.
///
/// Every call refuses to act once the credential is at or past its expiry, then
/// issues exactly one request and never surfaces a credential, token, API key,
/// or raw provider payload.
pub struct BungieTransferGateway<T, C, S = StaticCredentialSource> {
    transport: T,
    clock: C,
    source: S,
    api_key: TransferApiKey,
}

impl<T, C, S> BungieTransferGateway<T, C, S> {
    /// Builds the gateway without consulting the source until a request.
    ///
    /// Per-request Bearer validation rejects empty, non-UTF-8, whitespace,
    /// control, or non-ASCII material as a sanitized `ConfigurationError`.
    pub fn new(
        transport: T,
        clock: C,
        source: S,
        api_key: TransferApiKey,
    ) -> Result<Self, AppError> {
        Ok(Self {
            transport,
            clock,
            source,
            api_key,
        })
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn into_transport(self) -> T {
        self.transport
    }
}

impl<T, C, S> fmt::Debug for BungieTransferGateway<T, C, S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BungieTransferGateway")
            .finish_non_exhaustive()
    }
}

/// Refuses to proceed when the credential is already at or past its expiry.
///
/// The comparison is inclusive so a credential that expires exactly now is
/// treated as expired, and it runs before any request is constructed.
pub(crate) fn ensure_unexpired(now: Timestamp, expires_at: Timestamp) -> Result<(), AppError> {
    if now >= expires_at {
        return Err(AppError::new(ErrorCategory::ProviderAuthentication));
    }
    Ok(())
}

impl<T, C, S> TransferGateway for BungieTransferGateway<T, C, S>
where
    T: TransferTransport,
    C: Clock,
    S: CredentialSource,
{
    fn transfer(&mut self, request: &TransferRequest) -> Result<TransferOutcome, AppError> {
        let credential = self.source.current()?;
        let bearer = std::str::from_utf8(credential.material.expose_secret())
            .map_err(|_| configuration())
            .and_then(|token| TransferBearerToken::new(token.to_owned()))?;
        // An unrepresentable request never dispatches, so it is an `Err`, not an
        // uncertain outcome.
        let body = build_body(request)?;
        let http_request =
            TransferHttpRequest::new(TRANSFER_ENDPOINT, body, Some(&self.api_key), Some(&bearer));
        let now = self.clock.now()?;
        ensure_unexpired(now, credential.metadata.expires_at)?;

        let mut response = match self.transport.post(&http_request) {
            Ok(response) => response,
            Err(TransferTransportError::Throttled(seconds)) => {
                return Err(AppError::bungie_rate_limit(seconds));
            }
            Err(TransferTransportError::NotDispatched) => return Err(provider_unavailable()),
            Err(TransferTransportError::Configuration) => return Err(configuration()),
            Err(TransferTransportError::Indeterminate) => return Ok(TransferOutcome::Uncertain),
        };
        classify_response(&mut response)
    }
}

/// Maps the non-Vault endpoint of a transfer request onto the provider
/// `characterId`, or `None` when the request has no unique character endpoint.
///
/// The provider action expresses exactly one Vault/character pair: it names one
/// `characterId` and a `transferToVault` direction. A request with two character
/// endpoints has no provider representation, so it is refused before dispatch.
fn transfer_character(request: &TransferRequest) -> Option<&CharacterId> {
    match (request.from(), request.to()) {
        (ItemLocation::Character(character), ItemLocation::Vault) => Some(character),
        (ItemLocation::Vault, ItemLocation::Character(character)) => Some(character),
        _ => None,
    }
}

/// Builds the exact JSON action body from a validated request.
///
/// The body carries exactly the six documented request fields — no more, no
/// fewer — and each value is bounded to the official schema type:
/// `itemReferenceHash` (`uint32`), `stackSize` (`int32`), `transferToVault`
/// (`boolean`), `itemId` (`int64` as a decimal string), `characterId` (`int64`
/// as a decimal string), and `membershipType` (`int32`). JSON object member
/// order is not semantically meaningful and is never asserted: the semantic
/// six-field set and each field's type are the contract. A request whose values
/// cannot be represented is a sanitized [`ErrorCategory::ProviderValidation`]
/// failure that never dispatches.
fn build_body(request: &TransferRequest) -> Result<String, AppError> {
    let item_hash =
        u32::try_from(request.item_hash().value()).map_err(|_| provider_validation())?;
    let stack_size = i32::try_from(request.quantity()).map_err(|_| provider_validation())?;
    let membership_type = i32::try_from(request.membership().membership_type().value())
        .map_err(|_| provider_validation())?;
    let character = transfer_character(request).ok_or_else(provider_validation)?;
    if !is_canonical_identifier(request.item().as_str())
        || !is_canonical_identifier(character.as_str())
    {
        return Err(provider_validation());
    }

    let mut body = Map::new();
    body.insert("itemReferenceHash".to_owned(), Value::from(item_hash));
    body.insert("stackSize".to_owned(), Value::from(stack_size));
    body.insert(
        "transferToVault".to_owned(),
        Value::Bool(request.to_vault()),
    );
    body.insert("itemId".to_owned(), Value::from(request.item().as_str()));
    body.insert("characterId".to_owned(), Value::from(character.as_str()));
    body.insert("membershipType".to_owned(), Value::from(membership_type));
    Ok(Value::Object(body).to_string())
}

/// Requires a non-empty, canonical, unsigned decimal identifier of at most 128
/// digits.
///
/// Only ASCII digits are accepted; signs, whitespace, separators, and any
/// non-ASCII digit are rejected. Leading zeros are non-canonical, so `"0"` is
/// the only accepted identifier that starts with a zero.
pub(crate) fn is_canonical_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_DIGITS
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value.len() == 1 || !value.starts_with('0'))
}

/// Classifies one observed transport response into the committed outcome.
///
/// A received authentication refusal records dispatch without claiming a
/// mutation. Every post-dispatch condition a caller cannot resolve becomes
/// [`TransferOutcome::Uncertain`]: a dispatched mutation that returns a server
/// error does not prove the mutation did not apply, so `5xx` is uncertain, and
/// only `429` — a deterministic throttle refusal to process the request — is
/// [`TransferOutcome::Transient`].
fn classify_response(response: &mut TransferHttpResponse) -> Result<TransferOutcome, AppError> {
    let status = response.status();
    let content_length = response.content_length();
    match status {
        200 => {}
        401 | 403 => return Ok(TransferOutcome::AuthenticationRefused),
        429 => return Ok(TransferOutcome::Transient),
        // A dispatched mutation that returns a server error cannot establish
        // non-application, so it is uncertain rather than transient.
        500..=599 => return Ok(TransferOutcome::Uncertain),
        400..=499 => {
            return Ok(TransferOutcome::Rejected(TransferRejection::InvalidRequest));
        }
        _ => return Ok(TransferOutcome::Uncertain),
    }
    let Some(body) = response.json_body(content_length) else {
        return Ok(TransferOutcome::Uncertain);
    };
    Ok(classify_envelope(&body))
}

/// Classifies a JSON action envelope.
///
/// Success is the shared `ErrorCode == 1` convention. A non-success envelope is
/// a deterministic refusal only for a recognized numeric code;
/// anything else, or a missing, mistyped, or non-integral
/// code, is uncertain. The body is never surfaced.
fn classify_envelope(bytes: &[u8]) -> TransferOutcome {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return TransferOutcome::Uncertain;
    };
    let Some(object) = value.as_object() else {
        return TransferOutcome::Uncertain;
    };
    match object.get("ErrorCode").and_then(Value::as_u64) {
        Some(SUCCESS_ERROR_CODE) => TransferOutcome::Transferred,
        Some(code) => crate::bungie_errors::rejection(code)
            .map_or(TransferOutcome::Uncertain, |reason| {
                TransferOutcome::Rejected(TransferRejection::Provider(reason))
            }),
        None => TransferOutcome::Uncertain,
    }
}

/// Reads the body under a strict cap, reporting anything untrustworthy as
/// `None`.
///
/// The announced `Content-Length`, when present, is checked first so an oversize
/// body is rejected without reading any bytes. The body is then read under a
/// `limit + 1` ceiling so an unannounced oversize still fails closed, and a
/// declared length must match the observed bytes exactly. A read error, an
/// oversize body, or a mismatched length is `None`, which the caller reports as
/// [`TransferOutcome::Uncertain`] rather than an error, because the request was
/// already dispatched.
fn read_bounded_body(body: &mut dyn Read, content_length: Option<u64>) -> Option<Vec<u8>> {
    if let Some(length) = content_length
        && length > TRANSFER_RESPONSE_LIMIT as u64
    {
        return None;
    }
    let mut buffer = Vec::new();
    body.take(TRANSFER_RESPONSE_LIMIT as u64 + 1)
        .read_to_end(&mut buffer)
        .ok()?;
    if buffer.len() > TRANSFER_RESPONSE_LIMIT {
        return None;
    }
    if let Some(length) = content_length
        && buffer.len() as u64 != length
    {
        return None;
    }
    Some(buffer)
}

/// Accepts only an `application/json` media type, case-insensitively, with an
/// optional well-formed parameter list such as `; charset=UTF-8`.
///
/// Missing, malformed, control-bearing, or non-JSON values are rejected. The
/// raw upstream value is inspected here only; it is never surfaced.
fn is_json_content_type(value: &str) -> bool {
    if value.chars().any(|character| character.is_control()) {
        return false;
    }
    let mut parts = value.split(';');
    let media_type = parts.next().unwrap_or_default().trim();
    media_type.eq_ignore_ascii_case(CONTENT_TYPE_JSON) && parts.all(is_media_parameter)
}

/// Requires one `name=value` media-type parameter with a valid token name and a
/// token or quoted-string value.
fn is_media_parameter(parameter: &str) -> bool {
    let Some((name, value)) = parameter.split_once('=') else {
        return false;
    };
    let name = name.trim();
    let value = value.trim();
    !name.is_empty() && is_token(name) && is_parameter_value(value)
}

fn is_parameter_value(value: &str) -> bool {
    if is_token(value) {
        return true;
    }
    let Some(inner) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return false;
    };
    !inner.contains('"')
        && inner
            .chars()
            .all(|character| character == '\t' || character == ' ' || character.is_ascii_graphic())
}

fn is_token(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(is_token_byte)
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

#[cfg(test)]
mod tests {
    #[test]
    fn numeric_action_rejections() {
        crate::bungie_errors::assert_action_codes(|value| {
            match super::classify_envelope(value.to_string().as_bytes()) {
                TransferOutcome::Rejected(TransferRejection::Provider(reason)) => Some(reason),
                TransferOutcome::Uncertain => None,
                other => panic!("unexpected {other:?}"),
            }
        });
    }
    #[test]
    fn numeric_action_rejections_ignore_status_text() {
        for code in [
            1623, 1632, 1634, 1640, 1641, 1642, 1648, 1656, 1660, 1663, 1671, 1672,
        ] {
            let body = serde_json::json!({"ErrorCode":code,"ErrorStatus":"Success"});
            assert!(
                matches!(
                    super::classify_envelope(body.to_string().as_bytes()),
                    TransferOutcome::Rejected(_)
                ),
                "code {code}"
            );
        }
        for body in [
            serde_json::json!({"ErrorCode":999,"ErrorStatus":"ItemIsEquipped"}),
            serde_json::json!({"ErrorStatus":"NoRoomInDestination"}),
        ] {
            assert_eq!(
                super::classify_envelope(body.to_string().as_bytes()),
                TransferOutcome::Uncertain
            );
        }
    }

    use super::{
        BungieTransferGateway, HttpTransferTransport, TRANSFER_ENDPOINT, TRANSFER_RESPONSE_LIMIT,
        TransferApiKey, TransferBearerToken, TransferHttpRequest, TransferHttpResponse,
        TransferTransport, TransferTransportError, is_allowed_transfer_url,
    };
    use destiny_core::{
        AccessCredential, AccessCredentialMetadata, AppError, CharacterId, Clock,
        CredentialMaterial, DestinyMembership, ErrorCategory, ItemHash, ItemInstanceId,
        ItemLocation, MembershipId, MembershipType, Timestamp, TransferGateway, TransferOutcome,
        TransferRejection, TransferRequest,
    };
    use serde_json::{Value, json};
    use std::collections::VecDeque;
    use std::io::{self, Cursor, Read};

    // Synthetic loopback only: directly exercise the production client policy,
    // bypassing the action allowlist in this test without credentials. This is
    // wire/reader evidence, not Bungie classification or live-provider proof.
    fn instanced_pull_wire_response(bytes: Vec<u8>, encoding: &str) -> TransferHttpResponse {
        use std::io::Write;
        use std::net::{TcpListener, TcpStream};
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let transport = HttpTransferTransport::new_instanced_pull().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Encoding: {encoding}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        let server = thread::spawn(move || {
            // Exactly one accept. A bounded watchdog connection below releases
            // accept even if the client fails before opening its connection.
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8];
            while request.len() < 4096 && !request.ends_with(b"\r\n\r\n") {
                if socket.read(&mut byte).unwrap_or(0) == 0 {
                    return;
                }
                request.push(byte[0]);
            }
            assert!(request.ends_with(b"\r\n\r\n"));
            assert!(
                !request
                    .windows(13)
                    .any(|v| v.eq_ignore_ascii_case(b"authorization"))
            );
            // The cap can reject a response without reading the full body.
            let _ = socket
                .write_all(headers.as_bytes())
                .and_then(|_| socket.write_all(&bytes));
        });
        let (finished, wait) = mpsc::channel();
        let watchdog = thread::spawn(move || {
            if wait.recv_timeout(Duration::from_secs(5)).is_err() {
                let _ = TcpStream::connect_timeout(&address, Duration::from_secs(2));
            }
        });
        let received = transport
            .client
            .post(format!("http://{address}/synthetic-pull"))
            .body(Vec::new())
            .send();
        server.join().unwrap();
        let _ = finished.send(());
        watchdog.join().unwrap();
        let received = received.unwrap();
        TransferHttpResponse {
            status: received.status().as_u16(),
            content_length: received.content_length(),
            content_type: received
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
            body: Box::new(received),
            throttle: None,
        }
    }

    // Valid gzip with one uncompressed DEFLATE block and a computed IEEE CRC32.
    // FCOMMENT allows a large encoded header around a tiny decoded envelope.
    fn instanced_pull_gzip_ack(comment_length: usize) -> Vec<u8> {
        let payload = b"{\"Response\":0,\"ErrorCode\":1,\"ThrottleSeconds\":0}";
        let mut gzip = vec![0x1f, 0x8b, 8, 0x10, 0, 0, 0, 0, 0, 255];
        gzip.extend(std::iter::repeat_n(b'x', comment_length));
        gzip.push(0);
        gzip.push(1); // Final stored block, aligned to byte boundary.
        let length = payload.len() as u16;
        gzip.extend(length.to_le_bytes());
        gzip.extend((length ^ u16::MAX).to_le_bytes());
        gzip.extend(payload);
        let mut crc = u32::MAX;
        for byte in payload {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320 & 0_u32.wrapping_sub(crc & 1));
            }
        }
        gzip.extend((!crc).to_le_bytes());
        gzip.extend((payload.len() as u32).to_le_bytes());
        gzip
    }

    #[test]
    fn instanced_pull_wire_small_gzip_is_not_decoded_into_ack() {
        let encoded = instanced_pull_gzip_ack(0);
        let mut response = instanced_pull_wire_response(encoded.clone(), "gzip");
        let body = response.json_body(response.content_length()).unwrap();
        assert_eq!(
            body, encoded,
            "pull must preserve encoded wire bytes, never auto-decode an acknowledgement"
        );
        assert!(serde_json::from_slice::<Value>(&body).is_err());
    }

    #[test]
    fn instanced_pull_wire_gzip_header_cannot_hide_declared_oversize() {
        let encoded = instanced_pull_gzip_ack(TRANSFER_RESPONSE_LIMIT + 1);
        let mut response = instanced_pull_wire_response(encoded.clone(), "gzip");
        assert!(
            response.json_body(response.content_length()).is_none(),
            "encoded Content-Length above the cap must not disappear during decompression"
        );
        assert_eq!(response.content_length(), Some(encoded.len() as u64));
    }

    #[test]
    fn instanced_pull_wire_unknown_encoding_stays_raw_and_identity_stays_usable() {
        let encoded = instanced_pull_gzip_ack(0);
        let mut response = instanced_pull_wire_response(encoded.clone(), "unknown-test-encoding");
        assert_eq!(
            response.json_body(response.content_length()).unwrap(),
            encoded
        );
        let plain = b"{\"Response\":0,\"ErrorCode\":1,\"ThrottleSeconds\":0}".to_vec();
        let mut response = instanced_pull_wire_response(plain.clone(), "identity");
        assert_eq!(
            response.json_body(response.content_length()).unwrap(),
            plain
        );
    }

    const API_KEY_SENTINEL: &str = "sentinel-transfer-api-key";
    const BEARER_SENTINEL: &str = "sentinel-transfer-bearer-token";
    const MEMBERSHIP_TYPE: u32 = 3;
    const MEMBERSHIP_ID: &str = "4611686018484639825";
    const ITEM_ID: &str = "6917529027641081856";
    const ITEM_HASH: i64 = 1_234_567_890;
    const SOURCE_CHARACTER: &str = "2305843009260670444";
    const DESTINATION_CHARACTER: &str = "2305843009260670445";
    const GATEWAY_NOW: u64 = 1_724_000_000;
    const GATEWAY_EXPIRES_AT: u64 = 1_724_003_600;
    const SUCCESS_BODY: &str = "{\"ErrorCode\":1,\"ErrorStatus\":\"Success\",\"Response\":{}}";

    fn api_key() -> TransferApiKey {
        TransferApiKey::new(API_KEY_SENTINEL).expect("fixture API key")
    }

    fn bearer() -> TransferBearerToken {
        TransferBearerToken::new(BEARER_SENTINEL).expect("fixture bearer token")
    }

    fn membership() -> DestinyMembership {
        DestinyMembership::new(
            MembershipType::new(MEMBERSHIP_TYPE),
            MembershipId::new(MEMBERSHIP_ID),
        )
        .expect("fixture membership")
    }

    /// A character-to-Vault request: the non-Vault endpoint is the source.
    fn to_vault_request() -> TransferRequest {
        TransferRequest::new(
            membership(),
            ItemInstanceId::new(ITEM_ID),
            ItemHash::new(ITEM_HASH),
            1,
            ItemLocation::Character(CharacterId::new(SOURCE_CHARACTER)),
            ItemLocation::Vault,
        )
        .expect("character-to-vault request")
    }

    /// A Vault-to-character request: the non-Vault endpoint is the destination.
    fn from_vault_request() -> TransferRequest {
        TransferRequest::new(
            membership(),
            ItemInstanceId::new(ITEM_ID),
            ItemHash::new(ITEM_HASH),
            2,
            ItemLocation::Vault,
            ItemLocation::Character(CharacterId::new(DESTINATION_CHARACTER)),
        )
        .expect("vault-to-character request")
    }

    struct FakeClock(u64);

    impl Clock for FakeClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            Ok(Timestamp::new(self.0))
        }
    }

    struct RecordedRequest {
        url: String,
        body: String,
        api_key: Option<String>,
        bearer: Option<String>,
    }

    struct FakeTransferTransport {
        responses: VecDeque<Result<TransferHttpResponse, TransferTransportError>>,
        recorded: Vec<RecordedRequest>,
    }

    impl FakeTransferTransport {
        fn queue(responses: Vec<Result<TransferHttpResponse, TransferTransportError>>) -> Self {
            Self {
                responses: VecDeque::from(responses),
                recorded: Vec::new(),
            }
        }

        fn last_request(&self) -> &RecordedRequest {
            self.recorded.last().expect("a request was recorded")
        }

        fn recorded(&self) -> &[RecordedRequest] {
            &self.recorded
        }
    }

    impl TransferTransport for FakeTransferTransport {
        fn post(
            &mut self,
            request: &TransferHttpRequest<'_>,
        ) -> Result<TransferHttpResponse, TransferTransportError> {
            self.recorded.push(RecordedRequest {
                url: request.url().to_owned(),
                body: request.body().to_owned(),
                api_key: request.api_key().map(|key| key.expose().to_owned()),
                bearer: request.bearer().map(|token| token.expose().to_owned()),
            });
            self.responses
                .pop_front()
                .unwrap_or(Err(TransferTransportError::Indeterminate))
        }
    }

    /// A reader that fails if the adapter ever attempts to read the body.
    struct RefusesRead;

    impl Read for RefusesRead {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("body read was attempted"))
        }
    }

    fn response_with_content_type(
        status: u16,
        content_type: Option<&str>,
        body: &str,
    ) -> TransferHttpResponse {
        TransferHttpResponse::new(
            status,
            Some(body.len() as u64),
            content_type.map(str::to_owned),
            Box::new(Cursor::new(body.as_bytes().to_vec())),
        )
    }

    fn json_response(status: u16, body: &str) -> TransferHttpResponse {
        response_with_content_type(status, Some("application/json"), body)
    }

    fn credential(expires_at: u64) -> AccessCredential {
        AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(expires_at),
            },
            material: CredentialMaterial::from_bytes(BEARER_SENTINEL.as_bytes().to_vec()),
            refresh_grant: None,
        }
    }

    struct ChangingSource(std::sync::Mutex<VecDeque<Result<AccessCredential, AppError>>>);
    impl destiny_core::CredentialSource for ChangingSource {
        fn current(&self) -> Result<AccessCredential, AppError> {
            self.0.lock().unwrap().pop_front().expect("scripted source")
        }
    }
    #[test]
    fn gateway_fetches_source_for_each_request_and_aborts_source_errors() {
        let mut second = credential(GATEWAY_EXPIRES_AT);
        second.material = CredentialMaterial::from_bytes(b"second-access");
        let source = ChangingSource(std::sync::Mutex::new(VecDeque::from([
            Ok(credential(GATEWAY_EXPIRES_AT)),
            Ok(second),
            Err(AppError::new(ErrorCategory::ReauthorizationRequired)),
        ])));
        let mut gateway = BungieTransferGateway::new(
            FakeTransferTransport::queue(vec![
                Ok(json_response(200, SUCCESS_BODY)),
                Ok(json_response(200, SUCCESS_BODY)),
            ]),
            FakeClock(GATEWAY_NOW),
            source,
            api_key(),
        )
        .unwrap();
        gateway.transfer(&to_vault_request()).unwrap();
        gateway.transfer(&to_vault_request()).unwrap();
        assert_eq!(
            gateway.transport().recorded()[0].bearer.as_deref(),
            Some(BEARER_SENTINEL)
        );
        assert_eq!(
            gateway.transport().recorded()[1].bearer.as_deref(),
            Some("second-access")
        );
        assert_eq!(
            gateway
                .transfer(&to_vault_request())
                .unwrap_err()
                .category(),
            ErrorCategory::ReauthorizationRequired
        );
        assert_eq!(gateway.transport().recorded().len(), 2);
    }

    type TestGateway = BungieTransferGateway<FakeTransferTransport, FakeClock>;

    fn gateway(
        responses: Vec<Result<TransferHttpResponse, TransferTransportError>>,
        now: u64,
        expires_at: u64,
    ) -> TestGateway {
        BungieTransferGateway::new(
            FakeTransferTransport::queue(responses),
            FakeClock(now),
            destiny_core::StaticCredentialSource::new(credential(expires_at)),
            api_key(),
        )
        .expect("valid gateway configuration")
    }

    fn transfer_with(response: TransferHttpResponse) -> TransferOutcome {
        let mut gateway = gateway(vec![Ok(response)], GATEWAY_NOW, GATEWAY_EXPIRES_AT);
        gateway
            .transfer(&to_vault_request())
            .expect("observed provider response")
    }

    fn ensure_success_body(error_code: &str, error_status: Option<&str>) -> String {
        match error_status {
            Some(status) => format!(
                "{{\"ErrorCode\":{error_code},\"ErrorStatus\":\"{status}\",\"Response\":{{}}}}"
            ),
            None => format!("{{\"ErrorCode\":{error_code},\"Response\":{{}}}}"),
        }
    }

    #[test]
    fn transfer_endpoint_is_the_exact_documented_url() {
        assert_eq!(
            TRANSFER_ENDPOINT,
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/TransferItem/"
        );
        assert!(TRANSFER_ENDPOINT.starts_with("https://"));
        assert!(TRANSFER_ENDPOINT.ends_with('/'));
        assert_eq!(TRANSFER_RESPONSE_LIMIT, 8 * 1024);
    }

    #[test]
    fn adaptive_fail_fast_precedes_credentials_and_dispatch() {
        use std::time::Duration;
        struct Time;
        impl crate::bungie_throttle::ThrottleTime for Time {
            fn now(&self) -> Duration {
                Duration::ZERO
            }
            fn sleep(&self, _: Duration) {
                panic!("fail-fast must not sleep");
            }
        }
        let throttle = std::sync::Arc::new(crate::bungie_throttle::BungieThrottle::new(
            std::sync::Arc::new(Time),
        ));
        for _ in 0..6 {
            throttle.observe(&serde_json::json!({"ErrorCode":36,"ThrottleSeconds":45}));
        }
        let mut transport = HttpTransferTransport::new()
            .unwrap()
            .with_throttle(throttle);
        // Missing credentials guarantee no network call even if this gate regresses.
        let request = TransferHttpRequest::new(TRANSFER_ENDPOINT, "{}".into(), None, None);
        assert_eq!(
            transport.post(&request).unwrap_err(),
            TransferTransportError::Throttled(45)
        );
    }
    #[test]
    fn allowlist_accepts_only_the_exact_action_url() {
        assert!(is_allowed_transfer_url(TRANSFER_ENDPOINT));
        let negatives = [
            String::new(),
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/TransferItem".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/TransferItem//".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/TransferItem/?x=1".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/TransferItem/#f".to_owned(),
            "HTTPS://www.bungie.net/Platform/Destiny2/Actions/Items/TransferItem/".to_owned(),
            "http://www.bungie.net/Platform/Destiny2/Actions/Items/TransferItem/".to_owned(),
            "https://www.bungie.net/platform/destiny2/Actions/Items/TransferItem/".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItem/".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/ConsumeItem/".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/SetLockState/".to_owned(),
            "https://evil.example/Platform/Destiny2/Actions/Items/TransferItem/".to_owned(),
            "https://www.bungie.net.evil.example/Platform/Destiny2/Actions/Items/TransferItem/"
                .to_owned(),
            "https://evil@www.bungie.net/Platform/Destiny2/Actions/Items/TransferItem/".to_owned(),
            "https://www.bungie.net:443/Platform/Destiny2/Actions/Items/TransferItem/".to_owned(),
            "https://WWW.BUNGIE.NET/Platform/Destiny2/Actions/Items/TransferItem/".to_owned(),
        ];
        for url in negatives {
            assert!(!is_allowed_transfer_url(&url), "should reject {url}");
        }
    }

    #[test]
    fn production_transport_rejects_a_non_allowlisted_url_before_reading_a_secret() {
        // Defense in depth: the allowlist does not only live in a predicate. The
        // production transport refuses a non-allowlisted URL itself, before it
        // reads a secret value or dispatches anything, so a redirected or
        // malicious URL can never carry a credential to the wire.
        let key = api_key();
        let token = bearer();
        let mut transport = HttpTransferTransport::new().expect("hardened transport");
        for url in [
            "https://evil.example/Platform/Destiny2/Actions/Items/TransferItem/",
            "http://www.bungie.net/Platform/Destiny2/Actions/Items/TransferItem/",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/TransferItem",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItem/",
        ] {
            let request = TransferHttpRequest::new(url, "{}".to_owned(), Some(&key), Some(&token));
            let error = transport
                .post(&request)
                .expect_err("non-allowlisted URL must be refused");
            assert_eq!(error, TransferTransportError::Configuration);
            // The failure is a category only: no secret value, and no URL value,
            // can escape through it.
            let rendered = format!("{error:?}");
            assert!(!rendered.contains(API_KEY_SENTINEL));
            assert!(!rendered.contains(BEARER_SENTINEL));
            assert!(!rendered.contains("evil.example"));
        }
    }

    #[test]
    fn request_body_has_the_exact_official_fields_and_types() {
        let mut gateway = gateway(
            vec![Ok(json_response(200, SUCCESS_BODY))],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );
        gateway.transfer(&to_vault_request()).expect("transfer");

        let recorded = gateway.transport().last_request();
        assert_eq!(recorded.url, TRANSFER_ENDPOINT);
        let body: Value = serde_json::from_str(&recorded.body).expect("valid JSON body");
        let object = body.as_object().expect("body is an object");

        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "characterId",
                "itemId",
                "itemReferenceHash",
                "membershipType",
                "stackSize",
                "transferToVault",
            ],
            "the body must carry exactly the official request fields"
        );

        assert_eq!(object["itemReferenceHash"], json!(ITEM_HASH));
        assert_eq!(object["stackSize"], json!(1));
        assert_eq!(object["transferToVault"], json!(true));
        assert_eq!(object["membershipType"], json!(MEMBERSHIP_TYPE));
        assert_eq!(object["itemId"], json!(ITEM_ID));
        assert_eq!(object["characterId"], json!(SOURCE_CHARACTER));

        // 64-bit identifiers are decimal strings (Bungie's int64 convention);
        // 32-bit values are JSON numbers and the direction is a boolean.
        assert!(object["itemId"].is_string());
        assert!(object["characterId"].is_string());
        assert!(object["itemReferenceHash"].is_number());
        assert!(object["stackSize"].is_number());
        assert!(object["membershipType"].is_number());
        assert!(object["transferToVault"].is_boolean());
    }

    #[test]
    fn direction_is_encoded_by_the_vault_flag_and_character() {
        // Character -> Vault: the flag is true and the character is the source.
        let mut to_vault = gateway(
            vec![Ok(json_response(200, SUCCESS_BODY))],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );
        to_vault.transfer(&to_vault_request()).expect("transfer");
        let body: Value =
            serde_json::from_str(&to_vault.transport().last_request().body).expect("body");
        assert_eq!(body["transferToVault"], json!(true));
        assert_eq!(body["characterId"], json!(SOURCE_CHARACTER));

        // Vault -> Character: the flag is false and the character is the
        // destination.
        let mut from_vault = gateway(
            vec![Ok(json_response(200, SUCCESS_BODY))],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );
        from_vault
            .transfer(&from_vault_request())
            .expect("transfer");
        let body: Value =
            serde_json::from_str(&from_vault.transport().last_request().body).expect("body");
        assert_eq!(body["transferToVault"], json!(false));
        assert_eq!(body["characterId"], json!(DESTINATION_CHARACTER));
        assert_eq!(body["stackSize"], json!(2));
    }

    #[test]
    fn request_carries_both_credentials_through_the_trusted_accessors() {
        let mut gateway = gateway(
            vec![Ok(json_response(200, SUCCESS_BODY))],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );
        gateway.transfer(&to_vault_request()).expect("transfer");
        let recorded = gateway.transport().last_request();
        assert_eq!(recorded.api_key.as_deref(), Some(API_KEY_SENTINEL));
        assert_eq!(recorded.bearer.as_deref(), Some(BEARER_SENTINEL));
    }

    #[test]
    fn headers_carry_json_media_types_and_both_credentials() {
        let key = api_key();
        let token = bearer();
        let request =
            TransferHttpRequest::new(TRANSFER_ENDPOINT, "{}".to_owned(), Some(&key), Some(&token));
        let headers = request.headers().expect("both secrets present");
        let expected: Vec<(&'static str, String)> = vec![
            ("content-type", "application/json".to_owned()),
            ("accept", "application/json".to_owned()),
            ("x-api-key", API_KEY_SENTINEL.to_owned()),
            ("authorization", format!("Bearer {BEARER_SENTINEL}")),
        ];
        assert_eq!(headers, expected);

        // A missing required secret is a configuration failure, not a partial
        // request.
        let missing_bearer =
            TransferHttpRequest::new(TRANSFER_ENDPOINT, "{}".to_owned(), Some(&key), None);
        assert!(missing_bearer.headers().is_none());
        let missing_key =
            TransferHttpRequest::new(TRANSFER_ENDPOINT, "{}".to_owned(), None, Some(&token));
        assert!(missing_key.headers().is_none());
    }

    #[test]
    fn request_accessors_round_trip() {
        let key = api_key();
        let token = bearer();
        let request = TransferHttpRequest::new(
            TRANSFER_ENDPOINT,
            "{\"itemReferenceHash\":1}".to_owned(),
            Some(&key),
            Some(&token),
        );
        assert_eq!(request.url(), TRANSFER_ENDPOINT);
        assert_eq!(request.body(), "{\"itemReferenceHash\":1}");
        assert_eq!(
            request.api_key().map(TransferApiKey::expose),
            Some(API_KEY_SENTINEL)
        );
        assert_eq!(
            request.bearer().map(TransferBearerToken::expose),
            Some(BEARER_SENTINEL)
        );

        let absent = TransferHttpRequest::new(TRANSFER_ENDPOINT, "{}".to_owned(), None, None);
        assert!(absent.api_key().is_none());
        assert!(absent.bearer().is_none());
    }

    #[test]
    fn response_accessors_round_trip() {
        let response = TransferHttpResponse::new(
            200,
            Some(SUCCESS_BODY.len() as u64),
            Some("application/json".to_owned()),
            Box::new(Cursor::new(SUCCESS_BODY.as_bytes().to_vec())),
        );
        assert_eq!(response.status(), 200);
        assert_eq!(response.content_length(), Some(SUCCESS_BODY.len() as u64));
        assert_eq!(response.content_type(), Some("application/json"));
    }

    #[test]
    fn secrets_and_payloads_never_appear_in_debug() {
        assert_eq!(format!("{:?}", api_key()), "TransferApiKey");
        assert_eq!(format!("{:?}", bearer()), "TransferBearerToken");

        let key = api_key();
        let token = bearer();
        let request = TransferHttpRequest::new(
            TRANSFER_ENDPOINT,
            format!("{{\"itemId\":\"{ITEM_ID}\",\"characterId\":\"{SOURCE_CHARACTER}\"}}"),
            Some(&key),
            Some(&token),
        );
        let rendered = format!("{request:?}");
        assert!(rendered.contains("TransferHttpRequest"));
        assert!(rendered.contains("POST"));
        assert!(rendered.contains("TransferItem"));
        assert!(!rendered.contains(TRANSFER_ENDPOINT));
        assert!(!rendered.contains("itemId"));
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains(API_KEY_SENTINEL));
        assert!(!rendered.contains(BEARER_SENTINEL));
        assert!(!rendered.contains(ITEM_ID));
        assert!(!rendered.contains(SOURCE_CHARACTER));

        let response = TransferHttpResponse::new(
            200,
            Some(8),
            Some("application/json; boundary=sentinel-content-type".to_owned()),
            Box::new(Cursor::new(b"sentinel-body".to_vec())),
        );
        let rendered = format!("{response:?}");
        assert!(rendered.contains("TransferHttpResponse"));
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("sentinel-content-type"));
        assert!(!rendered.contains("sentinel-body"));

        let gateway = gateway(vec![], GATEWAY_NOW, GATEWAY_EXPIRES_AT);
        let rendered = format!("{gateway:?}");
        assert!(rendered.contains("BungieTransferGateway"));
        assert!(!rendered.contains(API_KEY_SENTINEL));
        assert!(!rendered.contains(BEARER_SENTINEL));

        let transport = HttpTransferTransport::new().expect("hardened transport");
        assert!(!format!("{transport:?}").contains(API_KEY_SENTINEL));
    }

    #[test]
    fn transfer_request_debug_never_prints_untrusted_url_or_body() {
        let key = api_key();
        let token = bearer();
        let request = TransferHttpRequest::new(
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/TransferItem/?access_token=URL_CANARY",
            r#"{"FIELD_CANARY":"VALUE_CANARY"}"#.into(),
            Some(&key),
            Some(&token),
        );
        let rendered = format!("{request:?}");
        for canary in [
            "URL_CANARY",
            "FIELD_CANARY",
            "VALUE_CANARY",
            API_KEY_SENTINEL,
            BEARER_SENTINEL,
        ] {
            assert!(!rendered.contains(canary), "Debug leaked {canary}");
        }
        assert!(rendered.contains("<unallowlisted>"));
    }

    #[test]
    fn invalid_secrets_are_configuration_failures() {
        for invalid in [
            "",
            " ",
            "  ",
            "\t",
            "\n",
            "\r",
            "key value",
            "key\tvalue",
            "key\nvalue",
            "key\u{7f}",
            "clé",
            "\u{0}",
            "key\u{1}",
        ] {
            assert_eq!(
                TransferApiKey::new(invalid)
                    .expect_err("invalid API key")
                    .category(),
                ErrorCategory::ConfigurationError,
                "API key {invalid:?} must be rejected"
            );
            assert_eq!(
                TransferBearerToken::new(invalid)
                    .expect_err("invalid bearer token")
                    .category(),
                ErrorCategory::ConfigurationError,
                "bearer token {invalid:?} must be rejected"
            );
        }
    }

    #[test]
    fn visible_ascii_secrets_are_accepted() {
        for valid in ["!", "~", "a", "ABC.def-123_~", "0123456789"] {
            TransferApiKey::new(valid).expect("valid API key");
            TransferBearerToken::new(valid).expect("valid bearer token");
        }
    }

    #[test]
    fn success_envelope_reports_transferred() {
        for body in [
            "{\"ErrorCode\":1}",
            "{\"ErrorCode\":1,\"ErrorStatus\":\"Success\"}",
            "{\"ErrorCode\":1,\"Response\":{\"anything\":\"ignored\"}}",
        ] {
            assert_eq!(
                transfer_with(json_response(200, body)),
                TransferOutcome::Transferred,
                "body {body} must report a transferred step"
            );
        }
    }

    #[test]
    fn status_text_never_proves_a_rejection() {
        for status in ["ItemIsEquipped", "NotTransferrable", "NoRoomInDestination"] {
            for error_code in ["0", "7", "1670"] {
                let body = ensure_success_body(error_code, Some(status));
                assert_eq!(
                    transfer_with(json_response(200, &body)),
                    TransferOutcome::Uncertain
                );
            }
        }
    }

    #[test]
    fn unknown_or_unclassifiable_error_envelopes_are_uncertain() {
        // The recorded `CanTransfer` status is not a refusal, and an unknown,
        // absent, empty, or mistyped status is never guessed.
        for body in [
            ensure_success_body("7", Some("CanTransfer")),
            ensure_success_body("7", Some("SomethingElse")),
            ensure_success_body("7", Some("")),
            ensure_success_body("7", None),
            "{\"ErrorCode\":7,\"ErrorStatus\":42}".to_owned(),
            "{\"ErrorCode\":7,\"ErrorStatus\":null}".to_owned(),
            "{\"ErrorCode\":7,\"ErrorStatus\":[\"ItemIsEquipped\"]}".to_owned(),
            "{\"ErrorCode\":\"1\",\"ErrorStatus\":\"Success\"}".to_owned(),
            "{\"ErrorCode\":1.0,\"ErrorStatus\":\"Success\"}".to_owned(),
            "{\"ErrorCode\":-1,\"ErrorStatus\":\"Success\"}".to_owned(),
            "{\"ErrorCode\":18446744073709551616}".to_owned(),
            "{\"ErrorStatus\":\"Success\"}".to_owned(),
            "{}".to_owned(),
        ] {
            assert_eq!(
                transfer_with(json_response(200, &body)),
                TransferOutcome::Uncertain,
                "body {body} must be uncertain"
            );
        }
    }

    #[test]
    fn malformed_success_bodies_are_uncertain_not_errors() {
        for body in ["", "{not json", "[]", "null", "42", "\"transferred\""] {
            assert_eq!(
                transfer_with(json_response(200, body)),
                TransferOutcome::Uncertain,
                "body {body:?} must be uncertain"
            );
        }
    }

    #[test]
    fn a_non_json_media_type_after_success_is_uncertain() {
        for content_type in [
            None,
            Some(""),
            Some("text/html"),
            Some("text/plain; charset=UTF-8"),
            Some("application/xml"),
            Some("application/json-patch+json"),
            Some("application/json; charset"),
            Some("application/json\n"),
        ] {
            assert_eq!(
                transfer_with(response_with_content_type(200, content_type, SUCCESS_BODY)),
                TransferOutcome::Uncertain,
                "content type {content_type:?} must be uncertain"
            );
        }
        // Optional well-formed media-type parameters are accepted.
        for content_type in ["application/json; charset=UTF-8", "APPLICATION/JSON"] {
            assert_eq!(
                transfer_with(response_with_content_type(
                    200,
                    Some(content_type),
                    SUCCESS_BODY
                )),
                TransferOutcome::Transferred,
                "content type {content_type:?} must be accepted"
            );
        }
    }

    #[test]
    fn authentication_statuses_are_credential_rejections_without_mutation() {
        // RC003: received 401/403 proves an attempted call, not a mutation.
        // Only authentication failures before dispatch may return Err.
        for status in [401u16, 403] {
            let mut gateway = gateway(
                vec![Ok(json_response(status, "denied"))],
                GATEWAY_NOW,
                GATEWAY_EXPIRES_AT,
            );
            let outcome = gateway.transfer(&to_vault_request()).unwrap();
            assert_eq!(outcome, TransferOutcome::AuthenticationRefused);
            assert!(!outcome.applied());
            assert_eq!(gateway.transport().recorded().len(), 1);
        }
    }

    #[test]
    fn throttling_status_is_transient() {
        // The throttle status itself is a deterministic provider refusal to
        // process the submitted request, so nothing applied.
        assert_eq!(
            transfer_with(json_response(429, "slow down")),
            TransferOutcome::Transient
        );
    }

    #[test]
    fn server_error_statuses_are_uncertain_not_transient() {
        // A dispatched mutation that returns a server error does not prove the
        // mutation did not apply, so it must never be a clean transient result.
        for status in [500u16, 502, 503, 504] {
            assert_eq!(
                transfer_with(json_response(status, "boom")),
                TransferOutcome::Uncertain,
                "status {status} cannot establish non-application"
            );
        }
    }

    #[test]
    fn client_error_statuses_are_rejected_invalid_request() {
        for status in [400u16, 404, 405, 409, 422] {
            assert_eq!(
                transfer_with(json_response(status, "rejected")),
                TransferOutcome::Rejected(TransferRejection::InvalidRequest),
                "status {status} must be a deterministic rejection"
            );
        }
    }

    #[test]
    fn unexpected_statuses_are_uncertain() {
        for status in [100u16, 201, 204, 301, 302, 307] {
            assert_eq!(
                transfer_with(json_response(status, "unexpected")),
                TransferOutcome::Uncertain,
                "status {status} must be uncertain"
            );
        }
    }

    #[test]
    fn transport_failures_distinguish_dispatch_phase() {
        // A connect failure never reached the provider: a clean `Err`.
        let mut not_dispatched = gateway(
            vec![Err(TransferTransportError::NotDispatched)],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );
        assert_eq!(
            not_dispatched
                .transfer(&to_vault_request())
                .expect_err("pre-dispatch failure")
                .category(),
            ErrorCategory::ProviderUnavailable
        );

        // A local construction failure never dispatched: configuration.
        let mut configuration = gateway(
            vec![Err(TransferTransportError::Configuration)],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );
        assert_eq!(
            configuration
                .transfer(&to_vault_request())
                .expect_err("configuration failure")
                .category(),
            ErrorCategory::ConfigurationError
        );

        // A dispatched request with no observed response is uncertain, never a
        // clean failure.
        let mut indeterminate = gateway(
            vec![Err(TransferTransportError::Indeterminate)],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );
        assert_eq!(
            indeterminate
                .transfer(&to_vault_request())
                .expect("indeterminate outcome"),
            TransferOutcome::Uncertain
        );
    }

    #[test]
    fn expired_credential_refuses_before_dispatch() {
        for now in [GATEWAY_EXPIRES_AT, GATEWAY_EXPIRES_AT + 1] {
            let mut gateway = gateway(vec![], now, GATEWAY_EXPIRES_AT);
            assert_eq!(
                gateway
                    .transfer(&to_vault_request())
                    .expect_err("expired credential")
                    .category(),
                ErrorCategory::ProviderAuthentication
            );
            assert!(gateway.transport().recorded().is_empty());
        }
    }

    #[test]
    fn unrepresentable_requests_are_refused_before_dispatch() {
        let item = ItemInstanceId::new(ITEM_ID);
        let character = CharacterId::new(SOURCE_CHARACTER);

        // `stackSize` is an int32, so a quantity beyond it cannot be represented.
        let quantity = TransferRequest::new(
            membership(),
            item.clone(),
            ItemHash::new(ITEM_HASH),
            u32::MAX,
            ItemLocation::Character(character.clone()),
            ItemLocation::Vault,
        )
        .expect("valid endpoints");
        assert_unrepresentable(quantity);

        // `itemReferenceHash` is a uint32, so a negative or oversized hash cannot
        // be represented.
        for hash in [ItemHash::new(-1), ItemHash::new(i64::MAX)] {
            let request = TransferRequest::new(
                membership(),
                item.clone(),
                hash,
                1,
                ItemLocation::Character(character.clone()),
                ItemLocation::Vault,
            )
            .expect("valid endpoints");
            assert_unrepresentable(request);
        }

        // Two character endpoints have no provider representation: the action
        // names exactly one character and one Vault direction.
        let two_characters = TransferRequest::new(
            membership(),
            item,
            ItemHash::new(ITEM_HASH),
            1,
            ItemLocation::Character(character),
            ItemLocation::Character(CharacterId::new(DESTINATION_CHARACTER)),
        )
        .expect("valid endpoints");
        assert_unrepresentable(two_characters);
    }

    fn assert_unrepresentable(request: TransferRequest) {
        let mut gateway = gateway(vec![], GATEWAY_NOW, GATEWAY_EXPIRES_AT);
        assert_eq!(
            gateway
                .transfer(&request)
                .expect_err("unrepresentable request")
                .category(),
            ErrorCategory::ProviderValidation
        );
        assert!(gateway.transport().recorded().is_empty());
    }

    #[test]
    fn announced_oversize_is_uncertain_without_reading_the_body() {
        let response = TransferHttpResponse::new(
            200,
            Some(TRANSFER_RESPONSE_LIMIT as u64 + 1),
            Some("application/json".to_owned()),
            Box::new(RefusesRead),
        );
        assert_eq!(transfer_with(response), TransferOutcome::Uncertain);
    }

    #[test]
    fn unannounced_oversize_and_read_failure_are_uncertain() {
        let oversize = TransferHttpResponse::new(
            200,
            None,
            Some("application/json".to_owned()),
            Box::new(io::repeat(0x78)),
        );
        assert_eq!(transfer_with(oversize), TransferOutcome::Uncertain);

        let unreadable = TransferHttpResponse::new(
            200,
            None,
            Some("application/json".to_owned()),
            Box::new(RefusesRead),
        );
        assert_eq!(transfer_with(unreadable), TransferOutcome::Uncertain);
    }

    #[test]
    fn declared_length_mismatch_is_uncertain() {
        for (declared, delivered) in [(10u64, 0usize), (10, 3), (10, 9), (3, 4), (0, 1)] {
            let response = TransferHttpResponse::new(
                200,
                Some(declared),
                Some("application/json".to_owned()),
                Box::new(Cursor::new(vec![b'x'; delivered])),
            );
            assert_eq!(
                transfer_with(response),
                TransferOutcome::Uncertain,
                "a {delivered}-byte body under a declared {declared} must be uncertain"
            );
        }

        // Delivering exactly the declared length is accepted.
        let body = SUCCESS_BODY;
        let response = TransferHttpResponse::new(
            200,
            Some(body.len() as u64),
            Some("application/json".to_owned()),
            Box::new(Cursor::new(body.as_bytes().to_vec())),
        );
        assert_eq!(transfer_with(response), TransferOutcome::Transferred);
    }

    #[test]
    fn gateway_request_rejects_invalid_credential_material() {
        for material in [
            b"".to_vec(),
            b"has space".to_vec(),
            b"has\ttab".to_vec(),
            vec![0xff, 0xfe],
            vec![b'k', 0x00],
        ] {
            let credential = AccessCredential {
                metadata: AccessCredentialMetadata {
                    expires_at: Timestamp::new(GATEWAY_EXPIRES_AT),
                },
                material: CredentialMaterial::from_bytes(material),
                refresh_grant: None,
            };
            let mut gateway = BungieTransferGateway::new(
                FakeTransferTransport::queue(vec![]),
                FakeClock(GATEWAY_NOW),
                destiny_core::StaticCredentialSource::new(credential),
                api_key(),
            )
            .unwrap();
            let error = gateway
                .transfer(&to_vault_request())
                .expect_err("invalid credential material must be rejected");
            assert!(gateway.transport().recorded().is_empty());
            assert_eq!(error.category(), ErrorCategory::ConfigurationError);
            assert!(!format!("{error:?}").contains(BEARER_SENTINEL));
        }
    }

    #[test]
    fn gateway_failures_never_leak_provider_sentinels() {
        let sentinel = "sentinel-provider-payload";
        let body = format!(
            "{{\"ErrorCode\":7,\"ErrorStatus\":\"{sentinel}\",\"Message\":\"{sentinel}\"}}"
        );
        let mut uncertain = gateway(
            vec![Ok(json_response(200, &body))],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );
        assert_eq!(
            uncertain.transfer(&to_vault_request()).expect("uncertain"),
            TransferOutcome::Uncertain
        );
        assert!(!format!("{uncertain:?}").contains(sentinel));
        assert!(!format!("{uncertain:?}").contains(BEARER_SENTINEL));

        // An authentication failure carries no provider body either.
        let mut denied = gateway(
            vec![Ok(json_response(403, &format!("denied {sentinel}")))],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );
        let outcome = denied.transfer(&to_vault_request()).unwrap();
        assert_eq!(outcome, TransferOutcome::AuthenticationRefused);
        assert!(!format!("{outcome:?}").contains(sentinel));
        assert!(!format!("{outcome:?}").contains(BEARER_SENTINEL));
    }
}
