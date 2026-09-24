//! Production public-client Bungie authorization-code token exchange.
//!
//! This module owns the only network-facing OAuth token path. It performs one
//! blocking HTTPS `application/x-www-form-urlencoded` POST to the documented
//! public token endpoint and converts the strict JSON response into the
//! provider-neutral [`AccessCredential`] contract.
//!
//! Security posture:
//!
//! - one fixed HTTPS endpoint with rustls certificate verification;
//! - redirects disabled, proxies disabled, and no decompression features;
//! - a total request timeout plus a separate connect timeout;
//! - a small, strict response-size cap enforced from `Content-Length` and from
//!   the actual bytes read;
//! - a strict `application/json` response media-type check with optional
//!   parameters, evaluated before any response body is accepted as JSON;
//! - no client secret, scope, refresh grant, authorization header, or API-key
//!   header is ever constructed;
//! - diagnostics never expose request bodies, codes, tokens, client IDs,
//!   redirect URIs, or upstream error text.
//!
//! Tests use the [`TokenTransport`] seam instead of live HTTP.

use destiny_core::{
    AccessCredential, AccessCredentialMetadata, AppError, AuthorizationCode, Clock,
    CredentialMaterial, ErrorCategory, PublicTokenExchange, Timestamp,
};
use std::fmt;
use std::io::Read;
use std::time::Duration;
use url::form_urlencoded;

/// The documented public-client authorization-code token endpoint.
pub const TOKEN_ENDPOINT: &str = "https://www.bungie.net/platform/app/oauth/token/";

const CONTENT_TYPE_FORM: &str = "application/x-www-form-urlencoded";
const CONTENT_TYPE_JSON: &str = "application/json";
const GRANT_TYPE_AUTHORIZATION_CODE: &str = "authorization_code";

/// Strict upper bound on the token response body. Bungie token responses are
/// small; anything larger is treated as a validation failure.
const MAX_RESPONSE_BYTES: usize = 8 * 1024;

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

/// A transport-level failure, categorized before it reaches the adapter.
///
/// The variants are intentionally coarse so that no upstream text can escape.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenTransportError {
    /// The request exceeded the total timeout.
    Timeout,
    /// DNS, connection, TLS, or body-transfer failure.
    Unavailable,
    /// Local request construction or client configuration failure.
    Configuration,
}

/// The exact request this adapter asks a transport to send.
///
/// Field values are available to trusted callers through accessors but are
/// never rendered by [`fmt::Debug`], because they contain the authorization
/// code, client ID, and exact callback URI.
#[derive(Clone)]
pub struct TokenRequest {
    method: &'static str,
    url: String,
    headers: Vec<(&'static str, &'static str)>,
    fields: Vec<(&'static str, String)>,
}

impl TokenRequest {
    /// Builds the single allowed public-client exchange request.
    fn authorization_code(endpoint: &str, client_id: &str, redirect_uri: &str, code: &str) -> Self {
        Self {
            method: "POST",
            url: endpoint.to_owned(),
            headers: vec![("content-type", CONTENT_TYPE_FORM)],
            fields: vec![
                ("grant_type", GRANT_TYPE_AUTHORIZATION_CODE.to_owned()),
                ("code", code.to_owned()),
                ("client_id", client_id.to_owned()),
                ("redirect_uri", redirect_uri.to_owned()),
            ],
        }
    }

    pub fn method(&self) -> &'static str {
        self.method
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn headers(&self) -> &[(&'static str, &'static str)] {
        &self.headers
    }

    /// The exact ordered form field set, including values.
    pub fn fields(&self) -> &[(&'static str, String)] {
        &self.fields
    }

    /// Encodes the form body in the declared field order.
    fn encoded_body(&self) -> String {
        let mut serializer = form_urlencoded::Serializer::new(String::new());
        for (name, value) in &self.fields {
            serializer.append_pair(name, value);
        }
        serializer.finish()
    }
}

impl fmt::Debug for TokenRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names: Vec<&str> = self.headers.iter().map(|(name, _)| *name).collect();
        let field_names: Vec<&str> = self.fields.iter().map(|(name, _)| *name).collect();
        formatter
            .debug_struct("TokenRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("header_names", &header_names)
            .field("field_names", &field_names)
            .finish()
    }
}

/// A transport response with a bounded, streaming body.
///
/// The body is a reader so the adapter can enforce the response-size cap
/// without trusting the transport to buffer everything first.
pub struct TokenHttpResponse {
    status: u16,
    content_length: Option<u64>,
    content_type: Option<String>,
    body: Box<dyn Read>,
}

impl TokenHttpResponse {
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
}

impl fmt::Debug for TokenHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenHttpResponse")
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

/// The seam between the exchange logic and an HTTP implementation.
///
/// Production uses [`HttpTokenTransport`]; tests inject a fake that records the
/// request and returns crafted responses.
pub trait TokenTransport {
    fn send(&mut self, request: &TokenRequest) -> Result<TokenHttpResponse, TokenTransportError>;
}

/// The production blocking HTTPS transport.
///
/// Construct with [`HttpTokenTransport::new`]; construction failures are
/// reported as `ConfigurationError` and never surface transport text.
pub struct HttpTokenTransport {
    client: reqwest::blocking::Client,
    endpoint: String,
}

impl HttpTokenTransport {
    /// Builds a hardened client for the fixed token endpoint.
    pub fn new() -> Result<Self, AppError> {
        Self::for_endpoint(TOKEN_ENDPOINT)
    }

    fn for_endpoint(endpoint: &str) -> Result<Self, AppError> {
        if endpoint != TOKEN_ENDPOINT {
            return Err(configuration());
        }
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|_| configuration())?;
        Ok(Self {
            client,
            endpoint: endpoint.to_owned(),
        })
    }
}

impl fmt::Debug for HttpTokenTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpTokenTransport")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl TokenTransport for HttpTokenTransport {
    fn send(&mut self, request: &TokenRequest) -> Result<TokenHttpResponse, TokenTransportError> {
        if request.method() != "POST" || request.url() != self.endpoint {
            return Err(TokenTransportError::Configuration);
        }
        let mut builder = self.client.post(&self.endpoint);
        for (name, value) in request.headers() {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| TokenTransportError::Configuration)?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| TokenTransportError::Configuration)?;
            builder = builder.header(name, value);
        }
        let response = builder
            .body(request.encoded_body())
            .send()
            .map_err(classify_transport_error)?;
        let status = response.status().as_u16();
        let content_length = response.content_length();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Ok(TokenHttpResponse {
            status,
            content_length,
            content_type,
            body: Box::new(response),
        })
    }
}

fn classify_transport_error(error: reqwest::Error) -> TokenTransportError {
    if error.is_timeout() {
        TokenTransportError::Timeout
    } else if error.is_builder() {
        TokenTransportError::Configuration
    } else {
        TokenTransportError::Unavailable
    }
}

/// The production public-client exchange adapter.
///
/// It holds non-secret configuration (client ID and exact callback URI), a
/// transport, and the injected application clock used to compute absolute
/// access-token expiry.
pub struct BungieTokenExchange<T, C> {
    client_id: String,
    redirect_uri: String,
    transport: T,
    clock: C,
}

impl<T, C> BungieTokenExchange<T, C> {
    /// Builds the adapter, rejecting empty or control-bearing configuration.
    pub fn new(
        client_id: impl Into<String>,
        redirect_uri: impl Into<String>,
        transport: T,
        clock: C,
    ) -> Result<Self, AppError> {
        let client_id = client_id.into();
        let redirect_uri = redirect_uri.into();
        if !is_safe_public_value(&client_id) || !is_safe_public_value(&redirect_uri) {
            return Err(configuration());
        }
        Ok(Self {
            client_id,
            redirect_uri,
            transport,
            clock,
        })
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn into_transport(self) -> T {
        self.transport
    }
}

impl<T, C> fmt::Debug for BungieTokenExchange<T, C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BungieTokenExchange")
            .finish_non_exhaustive()
    }
}

fn is_safe_public_value(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && !value.chars().any(|character| character.is_control())
}

impl<T, C> PublicTokenExchange for BungieTokenExchange<T, C>
where
    T: TokenTransport,
    C: Clock,
{
    fn exchange(&mut self, code: AuthorizationCode) -> Result<AccessCredential, AppError> {
        let code = std::str::from_utf8(code.as_bytes()).map_err(|_| provider_validation())?;
        let request = TokenRequest::authorization_code(
            TOKEN_ENDPOINT,
            &self.client_id,
            &self.redirect_uri,
            code,
        );
        let mut response = self.transport.send(&request).map_err(map_transport_error)?;
        if response.status != 200 {
            return Err(classify_status(response.status));
        }
        if !response.content_type().is_some_and(is_json_content_type) {
            return Err(provider_validation());
        }
        let body = read_bounded_body(&mut response.body, response.content_length)?;
        let value: serde_json::Value =
            serde_json::from_slice(&body).map_err(|_| provider_validation())?;
        let object = value.as_object().ok_or_else(provider_validation)?;
        let now = self.clock.now()?;
        decode_success(object, now)
    }
}

fn map_transport_error(error: TokenTransportError) -> AppError {
    match error {
        TokenTransportError::Timeout | TokenTransportError::Unavailable => provider_unavailable(),
        TokenTransportError::Configuration => configuration(),
    }
}

fn classify_status(status: u16) -> AppError {
    match status {
        401 | 403 => AppError::new(ErrorCategory::ProviderAuthentication),
        429 => AppError::new(ErrorCategory::ProviderRateLimit),
        500..=599 => provider_unavailable(),
        _ => provider_validation(),
    }
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

/// Reads the body under a strict cap, checking `Content-Length` first so an
/// announced oversize is rejected without reading any bytes.
fn read_bounded_body(
    body: &mut dyn Read,
    content_length: Option<u64>,
) -> Result<Vec<u8>, AppError> {
    if let Some(length) = content_length
        && length > MAX_RESPONSE_BYTES as u64
    {
        return Err(provider_validation());
    }
    let mut buffer = Vec::new();
    let limit = MAX_RESPONSE_BYTES as u64 + 1;
    body.take(limit)
        .read_to_end(&mut buffer)
        .map_err(|_| provider_unavailable())?;
    if buffer.len() > MAX_RESPONSE_BYTES {
        return Err(provider_validation());
    }
    Ok(buffer)
}

/// Decodes a strict 200 response into an [`AccessCredential`].
///
/// Unknown non-secret keys (including the unused membership identity) are
/// ignored, but any refresh material is treated as provider validation because
/// a public client must never receive or store it.
fn decode_success(
    object: &serde_json::Map<String, serde_json::Value>,
    now: Timestamp,
) -> Result<AccessCredential, AppError> {
    if object.contains_key("refresh_token") || object.contains_key("refresh_expires_in") {
        return Err(provider_validation());
    }
    let access_token = object
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(provider_validation)?;
    let is_bearer = object
        .get("token_type")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|token_type| token_type.eq_ignore_ascii_case("bearer"));
    if !is_bearer {
        return Err(provider_validation());
    }
    let expires_in = object
        .get("expires_in")
        .and_then(expires_in_seconds)
        .ok_or_else(provider_validation)?;
    let expires_at = now
        .seconds()
        .checked_add(expires_in)
        .map(Timestamp::new)
        .ok_or_else(provider_validation)?;
    Ok(AccessCredential {
        metadata: AccessCredentialMetadata { expires_at },
        material: CredentialMaterial::from_bytes(access_token.as_bytes().to_vec()),
    })
}

/// Requires a strictly positive integer exactly representable as `u64`.
///
/// [`serde_json::Value::as_u64`] succeeds only for JSON numbers parsed as
/// unsigned integers, which excludes fractional, exponent-form, and
/// out-of-range values such as `3600.0`, `1e20`, or `u64::MAX + 1`. A float
/// cast would instead saturate, turning `1e20` into `u64::MAX` and silently
/// accepting it at timestamp zero.
fn expires_in_seconds(value: &serde_json::Value) -> Option<u64> {
    value.as_u64().filter(|seconds| *seconds > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{self, Cursor};

    const CLIENT_SENTINEL: &str = "sentinel-client-id";
    const REDIRECT_SENTINEL: &str = "http://127.0.0.1:43127/sentinel-callback";
    const CODE_SENTINEL: &str = "sentinel-authorization-code";
    const TOKEN_SENTINEL: &str = "sentinel-access-token";

    struct FakeClock(u64);

    impl Clock for FakeClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            Ok(Timestamp::new(self.0))
        }
    }

    struct FakeTransport {
        responses: VecDeque<Result<TokenHttpResponse, TokenTransportError>>,
        recorded: Vec<TokenRequest>,
    }

    impl FakeTransport {
        fn with(response: Result<TokenHttpResponse, TokenTransportError>) -> Self {
            Self {
                responses: VecDeque::from([response]),
                recorded: Vec::new(),
            }
        }

        fn last_request(&self) -> &TokenRequest {
            self.recorded.last().expect("a request was recorded")
        }
    }

    impl TokenTransport for FakeTransport {
        fn send(
            &mut self,
            request: &TokenRequest,
        ) -> Result<TokenHttpResponse, TokenTransportError> {
            self.recorded.push(request.clone());
            self.responses
                .pop_front()
                .unwrap_or(Err(TokenTransportError::Unavailable))
        }
    }

    fn response_with_content_type(
        status: u16,
        content_type: Option<&str>,
        body: &str,
    ) -> TokenHttpResponse {
        TokenHttpResponse::new(
            status,
            Some(body.len() as u64),
            content_type.map(str::to_owned),
            Box::new(Cursor::new(body.as_bytes().to_vec())),
        )
    }

    fn json_response(status: u16, body: &str) -> TokenHttpResponse {
        response_with_content_type(status, Some(CONTENT_TYPE_JSON), body)
    }

    fn unused_transport() -> FakeTransport {
        FakeTransport::with(Err(TokenTransportError::Unavailable))
    }

    fn exchange_with(
        transport: FakeTransport,
        now: u64,
    ) -> BungieTokenExchange<FakeTransport, FakeClock> {
        BungieTokenExchange::new(
            CLIENT_SENTINEL,
            REDIRECT_SENTINEL,
            transport,
            FakeClock(now),
        )
        .expect("valid sentinel configuration")
    }

    fn run(
        transport: FakeTransport,
        now: u64,
    ) -> (
        Result<AccessCredential, AppError>,
        BungieTokenExchange<FakeTransport, FakeClock>,
    ) {
        let mut exchange = exchange_with(transport, now);
        let result = exchange.exchange(AuthorizationCode::from_bytes(CODE_SENTINEL));
        (result, exchange)
    }

    fn success_body(expires_in: &str) -> String {
        format!(
            "{{\"access_token\":\"{TOKEN_SENTINEL}\",\"token_type\":\"Bearer\",\"expires_in\":{expires_in}}}"
        )
    }

    fn failure_category(result: &Result<AccessCredential, AppError>) -> ErrorCategory {
        result.as_ref().expect_err("expected failure").category()
    }

    #[test]
    fn token_endpoint_is_the_documented_public_endpoint() {
        assert_eq!(
            TOKEN_ENDPOINT,
            "https://www.bungie.net/platform/app/oauth/token/"
        );
        assert!(TOKEN_ENDPOINT.starts_with("https://"));
    }

    #[test]
    fn request_shape_is_exact_and_forbidden_fields_are_absent() {
        let body = success_body("3600");
        let (result, exchange) = run(FakeTransport::with(Ok(json_response(200, &body))), 1000);
        result.unwrap();

        let request = exchange.transport().last_request();
        assert_eq!(request.method(), "POST");
        assert_eq!(request.url(), TOKEN_ENDPOINT);
        assert_eq!(
            request.headers(),
            &[("content-type", "application/x-www-form-urlencoded")]
        );
        let fields: Vec<(&str, &str)> = request
            .fields()
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        assert_eq!(
            fields,
            vec![
                ("grant_type", "authorization_code"),
                ("code", CODE_SENTINEL),
                ("client_id", CLIENT_SENTINEL),
                ("redirect_uri", REDIRECT_SENTINEL),
            ]
        );
        let field_names: Vec<&str> = request.fields().iter().map(|(name, _)| *name).collect();
        assert_eq!(
            field_names,
            vec!["grant_type", "code", "client_id", "redirect_uri"]
        );
        for forbidden in [
            "client_secret",
            "scope",
            "refresh_token",
            "refresh_expires_in",
            "authorization",
            "x-api-key",
            "pkce",
            "code_verifier",
        ] {
            assert!(
                !field_names.contains(&forbidden),
                "form body must not contain the {forbidden} field"
            );
        }
        let encoded = request.encoded_body();
        assert_eq!(
            encoded,
            "grant_type=authorization_code&code=sentinel-authorization-code&client_id=sentinel-client-id&redirect_uri=http%3A%2F%2F127.0.0.1%3A43127%2Fsentinel-callback"
        );
        for forbidden in [
            "client_secret=",
            "scope=",
            "refresh_token=",
            "refresh_expires_in=",
            "code_verifier=",
        ] {
            assert!(
                !encoded.contains(forbidden),
                "form body must not contain {forbidden}"
            );
        }
        assert!(!encoded.contains("grant_type=refresh_token"));
        assert!(
            !request
                .headers()
                .iter()
                .any(|(name, _)| *name == "authorization" || name.eq_ignore_ascii_case("x-api-key"))
        );
    }

    #[test]
    fn success_returns_checked_absolute_expiry_and_material() {
        let body = success_body("3600");
        let (result, _) = run(FakeTransport::with(Ok(json_response(200, &body))), 1000);
        let credential = result.unwrap();
        assert_eq!(credential.metadata.expires_at, Timestamp::new(4600));
        assert_eq!(
            credential.material.expose_secret(),
            TOKEN_SENTINEL.as_bytes()
        );
    }

    #[test]
    fn success_requires_a_json_content_type() {
        for content_type in [
            "application/json",
            "APPLICATION/JSON",
            "Application/Json",
            "application/json; charset=UTF-8",
            "application/json;charset=utf-8",
            "application/json ; charset=UTF-8",
            "application/json; charset=\"UTF-8\"",
            "application/json; charset=utf-8; boundary=next",
        ] {
            let body = success_body("3600");
            let (result, _) = run(
                FakeTransport::with(Ok(response_with_content_type(
                    200,
                    Some(content_type),
                    &body,
                ))),
                1000,
            );
            assert!(
                result.is_ok(),
                "content type {content_type:?} should be accepted"
            );
        }
    }

    #[test]
    fn missing_malformed_or_non_json_content_type_is_provider_validation() {
        for content_type in [
            None,
            Some(""),
            Some("   "),
            Some("json"),
            Some("text/html"),
            Some("text/plain; charset=UTF-8"),
            Some("application/xml"),
            Some("application/json-patch+json"),
            Some("application/jsonextra"),
            Some("application/json;"),
            Some("application/json; charset"),
            Some("application/json; charset="),
            Some("application/json; =utf-8"),
            Some("application/json; charset=utf 8"),
            Some("application/json; charset=\"utf-8"),
            Some("application/json;; charset=utf-8"),
            Some("application/json\n"),
        ] {
            let body = success_body("3600");
            let (result, _) = run(
                FakeTransport::with(Ok(response_with_content_type(200, content_type, &body))),
                1000,
            );
            assert_eq!(
                failure_category(&result),
                ErrorCategory::ProviderValidation,
                "content type {content_type:?} must be rejected"
            );
        }
    }

    #[test]
    fn token_type_is_case_insensitive() {
        for token_type in ["Bearer", "bearer", "BEARER"] {
            let body = format!(
                "{{\"access_token\":\"{TOKEN_SENTINEL}\",\"token_type\":\"{token_type}\",\"expires_in\":60}}"
            );
            let (result, _) = run(FakeTransport::with(Ok(json_response(200, &body))), 0);
            assert!(result.is_ok(), "token_type {token_type} should be accepted");
        }
    }

    #[test]
    fn expiry_overflow_is_provider_validation() {
        let body = success_body("2");
        let (result, _) = run(FakeTransport::with(Ok(json_response(200, &body))), u64::MAX);
        assert_eq!(failure_category(&result), ErrorCategory::ProviderValidation);
    }

    #[test]
    fn expiry_requires_a_positive_integral_seconds_value() {
        for expires_in in ["0", "-1", "1.5", "\"3600\"", "null", "true", "1e400"] {
            let body = success_body(expires_in);
            let (result, _) = run(FakeTransport::with(Ok(json_response(200, &body))), 0);
            assert_eq!(
                failure_category(&result),
                ErrorCategory::ProviderValidation,
                "expires_in {expires_in} must be rejected"
            );
        }
    }

    #[test]
    fn expiry_requires_a_positive_integer_representable_as_u64() {
        // `3600.0` and `1e20` are mathematically integral but not u64 JSON
        // integers. The former float-cast path saturated, so `1e20` became
        // `u64::MAX` and was accepted at timestamp zero; `as_u64` rejects it.
        for expires_in in [
            "3600.0",
            "1e19",
            "1e20",
            "1e400",
            "18446744073709551616",
            "-3600.0",
        ] {
            let body = success_body(expires_in);
            let (result, _) = run(FakeTransport::with(Ok(json_response(200, &body))), 0);
            assert_eq!(
                failure_category(&result),
                ErrorCategory::ProviderValidation,
                "expires_in {expires_in} must be rejected"
            );
        }
    }

    #[test]
    fn missing_or_empty_required_fields_are_rejected() {
        let bodies = [
            "{}".to_owned(),
            "{\"token_type\":\"Bearer\",\"expires_in\":60}".to_owned(),
            "{\"access_token\":\"\",\"token_type\":\"Bearer\",\"expires_in\":60}".to_owned(),
            format!("{{\"access_token\":\"{TOKEN_SENTINEL}\",\"expires_in\":60}}"),
            format!(
                "{{\"access_token\":\"{TOKEN_SENTINEL}\",\"token_type\":\"mac\",\"expires_in\":60}}"
            ),
            format!("{{\"access_token\":\"{TOKEN_SENTINEL}\",\"token_type\":\"Bearer\"}}"),
        ];
        for body in bodies {
            let (result, _) = run(FakeTransport::with(Ok(json_response(200, &body))), 0);
            assert_eq!(
                failure_category(&result),
                ErrorCategory::ProviderValidation,
                "body {body} must be rejected"
            );
        }
    }

    #[test]
    fn unexpected_refresh_material_is_rejected() {
        for extra in [
            "\"refresh_token\":\"sentinel-refresh-token\"",
            "\"refresh_expires_in\":3600",
            "\"refresh_token\":null",
            "\"refresh_expires_in\":null",
        ] {
            let body = format!(
                "{{\"access_token\":\"{TOKEN_SENTINEL}\",\"token_type\":\"Bearer\",\"expires_in\":60,{extra}}}"
            );
            let (result, _) = run(FakeTransport::with(Ok(json_response(200, &body))), 0);
            assert_eq!(
                failure_category(&result),
                ErrorCategory::ProviderValidation,
                "refresh material {extra} must be rejected"
            );
        }
    }

    #[test]
    fn unused_membership_identity_is_ignored() {
        let body = format!(
            "{{\"access_token\":\"{TOKEN_SENTINEL}\",\"token_type\":\"Bearer\",\"expires_in\":60,\"membership_id\":\"4611686018484639825\",\"other\":\"ignored\"}}"
        );
        let (result, _) = run(FakeTransport::with(Ok(json_response(200, &body))), 0);
        assert_eq!(
            result.unwrap().material.expose_secret(),
            TOKEN_SENTINEL.as_bytes()
        );
    }

    #[test]
    fn authentication_and_rate_limit_statuses_map_to_typed_errors() {
        for status in [401u16, 403] {
            let (result, _) = run(FakeTransport::with(Ok(json_response(status, "denied"))), 0);
            assert_eq!(
                failure_category(&result),
                ErrorCategory::ProviderAuthentication,
                "status {status} must require authentication"
            );
        }
        let (result, _) = run(FakeTransport::with(Ok(json_response(429, "slow down"))), 0);
        assert_eq!(failure_category(&result), ErrorCategory::ProviderRateLimit);
    }

    #[test]
    fn server_statuses_map_to_provider_unavailable() {
        for status in [500u16, 502, 503] {
            let (result, _) = run(FakeTransport::with(Ok(json_response(status, "boom"))), 0);
            assert_eq!(
                failure_category(&result),
                ErrorCategory::ProviderUnavailable,
                "status {status} must be unavailable"
            );
        }
    }

    #[test]
    fn redirects_and_other_non_success_statuses_are_validation() {
        for status in [301u16, 302, 307, 400, 404, 201, 204] {
            let (result, _) = run(
                FakeTransport::with(Ok(json_response(status, "unexpected"))),
                0,
            );
            assert_eq!(
                failure_category(&result),
                ErrorCategory::ProviderValidation,
                "status {status} must be validation"
            );
        }
    }

    #[test]
    fn transport_failures_map_to_unavailable_and_configuration() {
        for error in [
            TokenTransportError::Timeout,
            TokenTransportError::Unavailable,
        ] {
            let (result, _) = run(FakeTransport::with(Err(error)), 0);
            assert_eq!(
                failure_category(&result),
                ErrorCategory::ProviderUnavailable
            );
        }
        let (result, _) = run(
            FakeTransport::with(Err(TokenTransportError::Configuration)),
            0,
        );
        assert_eq!(failure_category(&result), ErrorCategory::ConfigurationError);
    }

    #[test]
    fn announced_oversize_is_rejected_without_reading_the_body() {
        struct RefusesRead;
        impl Read for RefusesRead {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("body read was attempted"))
            }
        }
        let response = TokenHttpResponse::new(
            200,
            Some(MAX_RESPONSE_BYTES as u64 + 1),
            Some(CONTENT_TYPE_JSON.to_owned()),
            Box::new(RefusesRead),
        );
        let (result, _) = run(FakeTransport::with(Ok(response)), 0);
        assert_eq!(failure_category(&result), ErrorCategory::ProviderValidation);
    }

    #[test]
    fn unannounced_oversize_is_rejected_from_bytes_read() {
        let response = TokenHttpResponse::new(
            200,
            None,
            Some(CONTENT_TYPE_JSON.to_owned()),
            Box::new(io::repeat(0x78)),
        );
        let (result, _) = run(FakeTransport::with(Ok(response)), 0);
        assert_eq!(failure_category(&result), ErrorCategory::ProviderValidation);
    }

    #[test]
    fn malformed_json_is_provider_validation() {
        for body in ["", "{not json", "[]", "null", "42", "\"token\""] {
            let (result, _) = run(FakeTransport::with(Ok(json_response(200, body))), 0);
            assert_eq!(
                failure_category(&result),
                ErrorCategory::ProviderValidation,
                "body {body:?} must be rejected"
            );
        }
    }

    #[test]
    fn constructor_rejects_invalid_public_configuration() {
        for (client_id, redirect_uri) in [
            ("", REDIRECT_SENTINEL),
            ("   ", REDIRECT_SENTINEL),
            ("client\nid", REDIRECT_SENTINEL),
            (CLIENT_SENTINEL, ""),
            (CLIENT_SENTINEL, " http://127.0.0.1:43127/callback"),
            (CLIENT_SENTINEL, "http://127.0.0.1:43127/callback\0"),
        ] {
            let error =
                BungieTokenExchange::new(client_id, redirect_uri, unused_transport(), FakeClock(0))
                    .expect_err("invalid configuration must be rejected");
            assert_eq!(error.category(), ErrorCategory::ConfigurationError);
        }
    }

    #[test]
    fn diagnostics_never_expose_secrets() {
        let secrets: &[&str] = &[
            TOKEN_SENTINEL,
            CODE_SENTINEL,
            CLIENT_SENTINEL,
            REDIRECT_SENTINEL,
            "sentinel-upstream-error-text",
        ];

        let body = success_body("3600");
        let (result, exchange) = run(FakeTransport::with(Ok(json_response(200, &body))), 1000);
        result.unwrap();
        let request = exchange.transport().last_request();
        assert_no_secrets(&format!("{request:?}"), secrets);
        assert_no_secrets(&format!("{exchange:?}"), secrets);

        let response = json_response(401, "sentinel-upstream-error-text");
        assert_no_secrets(&format!("{response:?}"), secrets);

        let transport = FakeTransport::with(Ok(json_response(200, "{not json}")));
        let (result, exchange) = run(transport, 0);
        let error = result.expect_err("malformed body must fail");
        assert_no_secrets(&format!("{error}"), secrets);
        assert_no_secrets(&format!("{error:?}"), secrets);
        assert_no_secrets(&format!("{exchange:?}"), secrets);
    }

    #[test]
    fn response_diagnostics_use_the_shared_redaction_marker() {
        // The shared core marker is the one value-hiding placeholder; the
        // response media type and streaming body must both collapse to it, and
        // a payload-ish sentinel in either must never reach the diagnostic.
        let response = response_with_content_type(
            200,
            Some("application/json; boundary=content-type-sentinel"),
            "body-sentinel",
        );
        let rendered = format!("{response:?}");
        assert!(rendered.contains(destiny_core::redact::REDACTED));
        assert!(!rendered.contains("content-type-sentinel"));
        assert!(!rendered.contains("body-sentinel"));
    }

    fn assert_no_secrets(rendered: &str, secrets: &[&str]) {
        for secret in secrets {
            assert!(
                !rendered.contains(secret),
                "diagnostic leaked a sentinel: {rendered}"
            );
        }
    }
}
