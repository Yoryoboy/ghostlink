//! Pure membership selection plus the bounded authenticated Bungie profile
//! transport contract.
//!
//! The first half of this module performs no I/O: it turns one
//! `User.GetMembershipDataForCurrentUser` payload into the single effective
//! [`DestinyMembership`] the profile gateway must use. Input is handled as
//! `serde_json::Value` only; the adapter never deserializes into provider
//! structs. Every required field is validated strictly, and every failure is a
//! sanitized [`ErrorCategory::ProviderValidation`] error that carries no
//! provider payload, identifier, or message.
//!
//! The second half adds the credential, request, response, transport, and
//! gateway contract used to acquire those payloads over HTTPS:
//!
//! - validated, redacted [`ProfileApiKey`] and [`ProfileBearerToken`] secrets;
//! - a redacted GET [`ProfileHttpRequest`] and streaming [`ProfileHttpResponse`];
//! - a [`ProfileTransport`] seam plus the hardened, redirect-free reqwest
//!   [`HttpProfileTransport`];
//! - an exact URL allowlist that admits only the current-memberships endpoint
//!   and one profile endpoint shape, evaluated before any secret is attached;
//! - a reusable [`fetch_bounded`] helper with caller-supplied size caps;
//! - [`BungieProfileGateway`], the [`ProfileGateway`] implementation that
//!   checks the injected clock against the credential expiry before every call,
//!   performs exactly one bounded request, and delegates decoding to the pure
//!   [`decode_effective_membership`] and [`decode_profile`] functions.
//!
//! The gateway holds only redacted secrets and never renders a credential,
//! token, API key, or raw provider payload: every failure is a sanitized
//! category-only [`AppError`].
//!
//! Selection rules:
//!
//! - The envelope is a JSON object with `ErrorCode == 1` and a
//!   `Response.destinyMemberships` array. `Response.primaryMembershipId` is
//!   required but nullable: a value names the primary membership of a Cross
//!   Save relationship, and JSON `null` means there is no Cross Save
//!   relationship.
//! - Every `User.CrossSaveUserMembership` entry carries a nonzero, non-`254`
//!   `membershipType`, a canonical decimal `membershipId` (string or exact
//!   `u64`), an unsigned `crossSaveOverride` that is never `254`, and an array
//!   of unique nonzero, non-`254` `applicableMembershipTypes`. There is no
//!   `isOverridden` field: unknown fields are ignored.
//! - A membership identity (`membershipType` + `membershipId`) may appear once.
//! - A playable entry (one that applies to at least one platform) that names a
//!   nonzero `crossSaveOverride` must name that type among the platforms it
//!   applies to. An overridden entry applies to no platform.
//! - With a Cross Save relationship the named primary applies to its own type
//!   and names itself as its override, every applicable platform type maps to
//!   exactly one entry, and every other entry is an overridden record whose
//!   override names the primary's type.
//! - Without a Cross Save relationship there is exactly one membership; it
//!   names no override and applies only to its own type. Zero or several
//!   memberships are an MVP ambiguity and fail closed.

use destiny_core::{
    AccessCredential, AppError, Clock, DefinitionVersion, DestinyMembership, ErrorCategory,
    InventorySnapshotInput, MembershipId, MembershipType, ProfileGateway, Timestamp,
};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;
use std::io::Read;
use std::time::Duration;

use crate::profile::decode_profile;

/// The only success code Bungie returns for a fulfilled request.
const SUCCESS_ERROR_CODE: u64 = 1;

/// Bungie's reserved "None" membership type.
const MEMBERSHIP_TYPE_NONE: u32 = 254;

/// The longest decimal identifier the membership contract accepts.
const MAX_IDENTIFIER_DIGITS: usize = 128;

fn provider_validation() -> AppError {
    AppError::new(ErrorCategory::ProviderValidation)
}

/// Selects the effective membership from a Bungie current-user payload.
///
/// The result is the Cross Save primary named by `Response.primaryMembershipId`,
/// or the single membership of an account without a Cross Save relationship. A
/// duplicate identity, an inconsistent Cross Save shape, a missing or unmatched
/// `primaryMembershipId`, or any malformed, mistyped, or out-of-range field is a
/// sanitized [`ErrorCategory::ProviderValidation`] failure.
///
/// [`ErrorCategory::ProviderValidation`]: destiny_core::ErrorCategory::ProviderValidation
pub fn decode_effective_membership(bytes: &[u8]) -> Result<DestinyMembership, AppError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| provider_validation())?;
    let root = value.as_object().ok_or_else(provider_validation)?;

    if root.get("ErrorCode").and_then(Value::as_u64) != Some(SUCCESS_ERROR_CODE) {
        return Err(provider_validation());
    }

    let response = root
        .get("Response")
        .and_then(Value::as_object)
        .ok_or_else(provider_validation)?;
    let memberships = response
        .get("destinyMemberships")
        .and_then(Value::as_array)
        .ok_or_else(provider_validation)?;

    // `primaryMembershipId` is required but nullable. A value names the primary
    // membership of an established Cross Save relationship; JSON `null` means
    // there is no Cross Save relationship at all.
    let primary = response
        .get("primaryMembershipId")
        .ok_or_else(provider_validation)?;
    let primary_id = match primary {
        Value::Null => None,
        value => Some(decode_membership_id(Some(value))?),
    };

    let mut decoded = Vec::with_capacity(memberships.len());
    let mut seen = HashSet::with_capacity(memberships.len());
    for membership in memberships {
        let entry = decode_entry(membership)?;
        if !seen.insert((entry.membership_type, entry.membership_id.clone())) {
            return Err(provider_validation());
        }
        decoded.push(entry);
    }

    match primary_id {
        Some(primary_id) => select_cross_save_primary(&decoded, &primary_id),
        None => select_without_cross_save(&decoded),
    }
}

/// One validated `User.CrossSaveUserMembership` entry.
///
/// `isOverridden` is deliberately absent: the official entry does not define it,
/// so its presence or absence never affects selection, and it is treated as an
/// ignored unknown field.
struct DecodedMembership {
    membership_type: u32,
    membership_id: String,
    cross_save_override: u32,
    applicable: Vec<u32>,
}

fn decode_entry(value: &Value) -> Result<DecodedMembership, AppError> {
    let object = value.as_object().ok_or_else(provider_validation)?;

    let membership_type = decode_membership_type(object.get("membershipType"))?;
    let membership_id = decode_membership_id(object.get("membershipId"))?;
    let cross_save_override = decode_cross_save_override(object.get("crossSaveOverride"))?;
    if cross_save_override == MEMBERSHIP_TYPE_NONE {
        return Err(provider_validation());
    }
    let applicable = decode_applicable(object.get("applicableMembershipTypes"))?;

    // A playable entry (one that applies to at least one platform) that names an
    // overriding type must name that type among the platforms it applies to. An
    // overridden entry applies to no platform and is exempt here; its override
    // is cross-checked against the primary by the selector.
    if cross_save_override != 0
        && !applicable.is_empty()
        && !applicable.contains(&cross_save_override)
    {
        return Err(provider_validation());
    }

    Ok(DecodedMembership {
        membership_type,
        membership_id,
        cross_save_override,
        applicable,
    })
}

/// Selects the primary of an established Cross Save relationship.
///
/// The primary is the unique entry whose `membershipId` equals
/// `primaryMembershipId`. It must apply to its own type and name itself as its
/// override, every applicable platform type must map to exactly one entry, and
/// every other entry must be a bare overridden record that names the primary's
/// type. The primary identity is returned.
fn select_cross_save_primary(
    entries: &[DecodedMembership],
    primary_id: &str,
) -> Result<DestinyMembership, AppError> {
    // Exactly one entry may carry the primary membership identifier, even though
    // entries are keyed by the (membershipType, membershipId) identity pair.
    let mut matches = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.membership_id == primary_id);
    let (primary_index, primary) = matches.next().ok_or_else(provider_validation)?;
    if matches.next().is_some() {
        return Err(provider_validation());
    }

    // The primary applies to its own type and names itself as its override.
    if primary.applicable.is_empty()
        || !primary.applicable.contains(&primary.membership_type)
        || primary.cross_save_override != primary.membership_type
    {
        return Err(provider_validation());
    }

    // Every applicable platform type is represented by exactly one entry, and no
    // entry sits outside the applicable set. Applicable types are already
    // unique, so an equal count plus membership and per-type uniqueness is a
    // bijection.
    if entries.len() != primary.applicable.len() {
        return Err(provider_validation());
    }
    let mut represented = HashSet::with_capacity(entries.len());
    for entry in entries {
        if !primary.applicable.contains(&entry.membership_type)
            || !represented.insert(entry.membership_type)
        {
            return Err(provider_validation());
        }
    }

    // Every other entry is an overridden record: no applicable platform and an
    // override that names the primary's membership type.
    for (index, entry) in entries.iter().enumerate() {
        if index == primary_index {
            continue;
        }
        if !entry.applicable.is_empty() || entry.cross_save_override != primary.membership_type {
            return Err(provider_validation());
        }
    }

    DestinyMembership::new(
        MembershipType::new(primary.membership_type),
        MembershipId::new(primary.membership_id.clone()),
    )
}

/// Selects the single membership of an account without a Cross Save relationship.
///
/// With no Cross Save there is exactly one membership, it names no overriding
/// type, and it applies only to its own type. Zero or several entries are an MVP
/// ambiguity and fail closed.
fn select_without_cross_save(entries: &[DecodedMembership]) -> Result<DestinyMembership, AppError> {
    if entries.len() != 1 {
        return Err(provider_validation());
    }
    let entry = entries.first().ok_or_else(provider_validation)?;
    if entry.cross_save_override != 0
        || entry.applicable.len() != 1
        || entry.applicable.first() != Some(&entry.membership_type)
    {
        return Err(provider_validation());
    }
    DestinyMembership::new(
        MembershipType::new(entry.membership_type),
        MembershipId::new(entry.membership_id.clone()),
    )
}

/// Decodes one required, non-null, nonzero, non-`254` membership type.
fn decode_membership_type(value: Option<&Value>) -> Result<u32, AppError> {
    let raw = value
        .and_then(Value::as_u64)
        .ok_or_else(provider_validation)?;
    let value = u32::try_from(raw).map_err(|_| provider_validation())?;
    if value == 0 || value == MEMBERSHIP_TYPE_NONE {
        return Err(provider_validation());
    }
    Ok(value)
}

/// Decodes a required, non-null, unsigned 32-bit Cross Save override.
///
/// `0` is the documented "no override" value, so it is accepted here and its
/// consistency with the entry shape is enforced by [`decode_entry`].
fn decode_cross_save_override(value: Option<&Value>) -> Result<u32, AppError> {
    let raw = value
        .and_then(Value::as_u64)
        .ok_or_else(provider_validation)?;
    u32::try_from(raw).map_err(|_| provider_validation())
}

/// Decodes the unique, nonzero, non-`254` `applicableMembershipTypes` array.
fn decode_applicable(value: Option<&Value>) -> Result<Vec<u32>, AppError> {
    let array = value
        .and_then(Value::as_array)
        .ok_or_else(provider_validation)?;

    let mut decoded = Vec::with_capacity(array.len());
    let mut seen = HashSet::with_capacity(array.len());
    for item in array {
        let membership_type = decode_membership_type(Some(item))?;
        if !seen.insert(membership_type) {
            return Err(provider_validation());
        }
        decoded.push(membership_type);
    }
    Ok(decoded)
}

/// Decodes a canonical decimal membership identifier.
///
/// The provider may serialize the identifier as a decimal string or as an exact
/// nonnegative JSON integer. A string is validated directly; an exact `u64` is
/// canonicalized to its decimal representation, which is never leading-zero
/// padded. Floating-point and negative numbers are rejected because
/// `Value::as_u64` returns `None` for them.
fn decode_membership_id(value: Option<&Value>) -> Result<String, AppError> {
    match value {
        Some(Value::String(text)) => {
            validate_canonical_id(text)?;
            Ok(text.clone())
        }
        Some(Value::Number(number)) => {
            let raw = number.as_u64().ok_or_else(provider_validation)?;
            Ok(raw.to_string())
        }
        _ => Err(provider_validation()),
    }
}

/// Requires a non-empty, canonical, unsigned decimal identifier of at most 128
/// digits.
///
/// Only ASCII digits are accepted; signs, whitespace, separators, and any
/// non-ASCII digit are rejected. Leading zeros are non-canonical, so `"0"` is
/// the only accepted identifier that starts with a zero.
fn validate_canonical_id(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_DIGITS
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(provider_validation());
    }
    Ok(())
}

/// The documented Bungie current-user membership endpoint.
///
/// This is the only non-profile URL the production transport may contact.
pub const MEMBERSHIP_ENDPOINT: &str =
    "https://www.bungie.net/Platform/User/GetMembershipsForCurrentUser/";

/// The only origin the production transport is permitted to contact.
pub const PROFILE_ORIGIN: &str = "https://www.bungie.net";

/// The exact path prefix of the profile endpoint.
const PROFILE_PATH_PREFIX: &str = "/Platform/Destiny2/";

/// The exact path separator between membership type and membership identifier.
const PROFILE_PATH_INFIX: &str = "/Profile/";

/// The exact, ordered profile component query this adapter may request.
const PROFILE_COMPONENTS_QUERY: &str = "components=102,200,201,205,300,302";

/// Strict upper bound on the current-user membership response body (256 KiB).
pub const MEMBERSHIP_RESPONSE_LIMIT: usize = 256 * 1024;

/// Strict upper bound on one profile response body (16 MiB).
pub const PROFILE_RESPONSE_LIMIT: usize = 16 * 1024 * 1024;

const CONTENT_TYPE_JSON: &str = "application/json";
const ACCEPT_JSON: &str = "application/json";
const API_KEY_HEADER: &str = "x-api-key";
const AUTHORIZATION_HEADER: &str = "authorization";
const ACCEPT_HEADER: &str = "accept";

/// One total timeout covering connect, request, and body read.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// A tighter bound on the connection phase only.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

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
pub struct ProfileApiKey(String);

impl ProfileApiKey {
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
        &self.0
    }
}

impl fmt::Debug for ProfileApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        destiny_core::redact::opaque(formatter, "ProfileApiKey")
    }
}

/// A validated Bungie OAuth access token used as a Bearer credential.
///
/// It applies the same strict visible-ASCII rule as [`ProfileApiKey`] and is
/// likewise value-redacted in [`fmt::Debug`] and reachable only through the
/// trusted `expose` boundary.
pub struct ProfileBearerToken(String);

impl ProfileBearerToken {
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
        &self.0
    }
}

impl fmt::Debug for ProfileBearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        destiny_core::redact::opaque(formatter, "ProfileBearerToken")
    }
}

/// Builds the one exact, allowlisted profile GET URL for a resolved membership.
///
/// The membership type is rendered in canonical decimal and the membership
/// identifier is used verbatim, so a validated [`DestinyMembership`] always
/// yields the exact endpoint shape [`is_allowed_profile_url`] accepts. The
/// ordered component query is fixed by this module.
pub fn profile_endpoint(membership: &DestinyMembership) -> String {
    format!(
        "{PROFILE_ORIGIN}{PROFILE_PATH_PREFIX}{}{PROFILE_PATH_INFIX}{}/?{PROFILE_COMPONENTS_QUERY}",
        membership.membership_type().value(),
        membership.membership_id().as_str(),
    )
}

/// Accepts only the fixed current-memberships endpoint or the one exact profile
/// endpoint shape, so the production transport can never be pointed elsewhere.
///
/// This predicate is the pre-flight gate: the transport evaluates it before it
/// reads or attaches any credential, so a rejected URL never receives a secret.
pub fn is_allowed_profile_url(url: &str) -> bool {
    url == MEMBERSHIP_ENDPOINT || is_allowed_profile_get(url)
}

/// Accepts exactly `{origin}/Platform/Destiny2/{type}/Profile/{id}/?components=…`.
///
/// The origin is matched case-sensitively and without userinfo or port, the
/// membership type and identifier must be canonical decimal, and the query must
/// be the exact ordered component list. A missing trailing slash, an extra or
/// reordered component, percent-encoding, a fragment, or any additional query
/// parameter is rejected.
fn is_allowed_profile_get(url: &str) -> bool {
    let Some(path) = url.strip_prefix(PROFILE_ORIGIN) else {
        return false;
    };
    let Some(path) = path.strip_prefix(PROFILE_PATH_PREFIX) else {
        return false;
    };
    let Some((membership_type, path)) = path.split_once(PROFILE_PATH_INFIX) else {
        return false;
    };
    if canonical_membership_type(membership_type).is_none() {
        return false;
    }
    let Some((membership_id, query)) = path.split_once("/?") else {
        return false;
    };
    validate_canonical_id(membership_id).is_ok() && query == PROFILE_COMPONENTS_QUERY
}

/// Decodes a canonical decimal, nonzero, non-`254` membership type segment.
fn canonical_membership_type(value: &str) -> Option<u32> {
    parse_canonical_u32(value)
        .filter(|membership_type| *membership_type != 0 && *membership_type != MEMBERSHIP_TYPE_NONE)
}

/// Parses a canonical unsigned decimal string.
///
/// Only ASCII digits are accepted; signs, whitespace, separators, and leading
/// zeros are rejected, and the value must fit a `u32`.
fn parse_canonical_u32(value: &str) -> Option<u32> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return None;
    }
    value.parse::<u32>().ok()
}

/// The exact read-only GET request this adapter asks a transport to send.
///
/// The API key and Bearer token are carried by reference and are never rendered
/// by [`fmt::Debug`]. The URL is always validated against the allowlist by the
/// transport before any secret is attached.
pub struct ProfileHttpRequest<'a> {
    url: &'a str,
    api_key: Option<&'a ProfileApiKey>,
    bearer: Option<&'a ProfileBearerToken>,
}

impl<'a> ProfileHttpRequest<'a> {
    pub fn new(
        url: &'a str,
        api_key: Option<&'a ProfileApiKey>,
        bearer: Option<&'a ProfileBearerToken>,
    ) -> Self {
        Self {
            url,
            api_key,
            bearer,
        }
    }

    pub fn url(&self) -> &'a str {
        self.url
    }

    /// The trusted API-key accessor. The key value is never rendered.
    pub fn api_key(&self) -> Option<&'a ProfileApiKey> {
        self.api_key
    }

    /// The trusted Bearer accessor. The token value is never rendered.
    pub fn bearer(&self) -> Option<&'a ProfileBearerToken> {
        self.bearer
    }
}

impl fmt::Debug for ProfileHttpRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProfileHttpRequest")
            .field("method", &"GET")
            .field("url", &self.url)
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
pub struct ProfileHttpResponse {
    status: u16,
    content_length: Option<u64>,
    content_type: Option<String>,
    body: Box<dyn Read>,
}

impl ProfileHttpResponse {
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

impl fmt::Debug for ProfileHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProfileHttpResponse")
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

/// A transport-level failure, categorized before it reaches the adapter.
///
/// The variants are intentionally coarse so that no upstream text can escape.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileTransportError {
    /// The request exceeded the total timeout.
    Timeout,
    /// DNS, connection, TLS, or body-transfer failure.
    Unavailable,
    /// Local request construction or client configuration failure.
    Configuration,
}

/// The seam between the profile acquisition logic and an HTTP implementation.
///
/// Production uses [`HttpProfileTransport`]; tests inject a fake that records
/// the request and returns crafted responses.
pub trait ProfileTransport {
    fn get(
        &mut self,
        request: &ProfileHttpRequest<'_>,
    ) -> Result<ProfileHttpResponse, ProfileTransportError>;
}

/// The production blocking HTTPS transport.
///
/// Construction failures are reported as `ConfigurationError` and never surface
/// transport text. The client refuses redirects, ignores proxies, and bounds
/// both the total request and the connection phase.
pub struct HttpProfileTransport {
    client: reqwest::blocking::Client,
}

impl HttpProfileTransport {
    /// Builds a hardened client for the fixed profile endpoints.
    pub fn new() -> Result<Self, AppError> {
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|_| configuration())?;
        Ok(Self { client })
    }
}

impl fmt::Debug for HttpProfileTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpProfileTransport")
            .finish_non_exhaustive()
    }
}

/// Confirms the request URL is on the fixed allowlist before a secret is read.
///
/// The allowlist is evaluated before any credential is attached, so a rejected
/// URL can never receive a secret.
fn ensure_allowlisted(url: &str) -> Result<(), ProfileTransportError> {
    if is_allowed_profile_url(url) {
        return Ok(());
    }
    Err(ProfileTransportError::Configuration)
}

impl ProfileTransport for HttpProfileTransport {
    fn get(
        &mut self,
        request: &ProfileHttpRequest<'_>,
    ) -> Result<ProfileHttpResponse, ProfileTransportError> {
        // The allowlist runs first so a rejected URL can never receive a secret.
        ensure_allowlisted(request.url())?;
        let api_key = request
            .api_key()
            .ok_or(ProfileTransportError::Configuration)?;
        let bearer = request
            .bearer()
            .ok_or(ProfileTransportError::Configuration)?;

        let api_key_value = reqwest::header::HeaderValue::from_str(api_key.expose())
            .map_err(|_| ProfileTransportError::Configuration)?;
        let authorization_value =
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", bearer.expose()))
                .map_err(|_| ProfileTransportError::Configuration)?;

        let response = self
            .client
            .get(request.url())
            .header(
                reqwest::header::HeaderName::from_static(API_KEY_HEADER),
                api_key_value,
            )
            .header(
                reqwest::header::HeaderName::from_static(AUTHORIZATION_HEADER),
                authorization_value,
            )
            .header(
                reqwest::header::HeaderName::from_static(ACCEPT_HEADER),
                reqwest::header::HeaderValue::from_static(ACCEPT_JSON),
            )
            .send()
            .map_err(classify_transport_error)?;

        let status = response.status().as_u16();
        let content_length = response.content_length();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Ok(ProfileHttpResponse {
            status,
            content_length,
            content_type,
            body: Box::new(response),
        })
    }
}

fn classify_transport_error(error: reqwest::Error) -> ProfileTransportError {
    if error.is_timeout() {
        ProfileTransportError::Timeout
    } else if error.is_builder() {
        ProfileTransportError::Configuration
    } else {
        ProfileTransportError::Unavailable
    }
}

/// Performs one allowlisted GET and returns its bounded, media-type-checked JSON
/// body.
///
/// `limit` is the caller-supplied response cap. The declared `Content-Length` is
/// checked before any byte is read, and the actual body is read under a
/// `limit + 1` ceiling so an unannounced oversize still fails closed. A body
/// that ends early is returned as-is: a truncated response is left for the
/// caller's JSON decode to reject, never padded or fabricated.
pub fn fetch_bounded<T>(
    transport: &mut T,
    request: &ProfileHttpRequest<'_>,
    limit: usize,
) -> Result<Vec<u8>, AppError>
where
    T: ProfileTransport,
{
    let mut response = transport.get(request).map_err(map_transport_error)?;
    if response.status != 200 {
        return Err(classify_status(response.status));
    }
    if !response.content_type().is_some_and(is_json_content_type) {
        return Err(provider_validation());
    }
    read_bounded_body(&mut response.body, response.content_length, limit)
}

fn map_transport_error(error: ProfileTransportError) -> AppError {
    match error {
        ProfileTransportError::Timeout | ProfileTransportError::Unavailable => {
            provider_unavailable()
        }
        ProfileTransportError::Configuration => configuration(),
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

/// Reads the body under a strict cap.
///
/// The announced `Content-Length`, when present, is checked first so an oversize
/// body is rejected without reading any bytes. The body is then read under a
/// `limit + 1` ceiling so an unannounced oversize still fails closed. When a
/// declared length exists, the observed body must match it exactly: a body that
/// ends early or runs long (but within the cap) is a mismatched response and
/// fails closed. An absent length simply accepts the bytes the reader produced.
fn read_bounded_body(
    body: &mut dyn Read,
    content_length: Option<u64>,
    limit: usize,
) -> Result<Vec<u8>, AppError> {
    if let Some(length) = content_length
        && length > limit as u64
    {
        return Err(provider_validation());
    }
    let mut buffer = Vec::new();
    body.take(limit as u64 + 1)
        .read_to_end(&mut buffer)
        .map_err(|_| provider_unavailable())?;
    if buffer.len() > limit {
        return Err(provider_validation());
    }
    // A declared length is a promise. Any mismatch means the response is not the
    // payload it announced, so a truncated or padded body must fail closed.
    if let Some(length) = content_length
        && buffer.len() as u64 != length
    {
        return Err(provider_validation());
    }
    Ok(buffer)
}

/// The production authenticated Bungie profile gateway.
///
/// It owns the transport, the injected application clock, the access credential,
/// the API key, and the fixed Manifest definition version. Construction converts
/// the credential material into a validated [`ProfileBearerToken`]; a
/// non-UTF-8 or otherwise invalid secret is a sanitized configuration failure.
///
/// Both operations are read-only and bounded: they refuse to act once the
/// credential is at or past its expiry, then perform exactly one request, and
/// never surface a credential, token, API key, or raw provider payload.
pub struct BungieProfileGateway<T, C> {
    transport: T,
    clock: C,
    credential: AccessCredential,
    bearer: ProfileBearerToken,
    api_key: ProfileApiKey,
    definition_version: DefinitionVersion,
}

impl<T, C> BungieProfileGateway<T, C> {
    /// Builds the gateway, validating the credential material as a Bearer token.
    ///
    /// Empty, non-UTF-8, whitespace-bearing, control-bearing, or non-ASCII
    /// credential material is a sanitized `ConfigurationError`; no secret value
    /// is included in the failure.
    pub fn new(
        transport: T,
        clock: C,
        credential: AccessCredential,
        api_key: ProfileApiKey,
        definition_version: DefinitionVersion,
    ) -> Result<Self, AppError> {
        let bearer = std::str::from_utf8(credential.material.expose_secret())
            .map_err(|_| configuration())
            .and_then(|token| ProfileBearerToken::new(token.to_owned()))?;
        Ok(Self {
            transport,
            clock,
            credential,
            bearer,
            api_key,
            definition_version,
        })
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn into_transport(self) -> T {
        self.transport
    }
}

impl<T, C> fmt::Debug for BungieProfileGateway<T, C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BungieProfileGateway")
            .finish_non_exhaustive()
    }
}

/// Refuses to proceed when the credential is already at or past its expiry.
///
/// The comparison is inclusive so a credential that expires exactly now is
/// treated as expired, and it runs before any request is constructed.
fn ensure_unexpired(now: Timestamp, expires_at: Timestamp) -> Result<(), AppError> {
    if now >= expires_at {
        return Err(AppError::new(ErrorCategory::ProviderAuthentication));
    }
    Ok(())
}

impl<T, C> ProfileGateway for BungieProfileGateway<T, C>
where
    T: ProfileTransport,
    C: Clock,
{
    fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
        let now = self.clock.now()?;
        ensure_unexpired(now, self.credential.metadata.expires_at)?;

        let request =
            ProfileHttpRequest::new(MEMBERSHIP_ENDPOINT, Some(&self.api_key), Some(&self.bearer));
        let bytes = fetch_bounded(&mut self.transport, &request, MEMBERSHIP_RESPONSE_LIMIT)?;
        decode_effective_membership(&bytes)
    }

    fn load_inventory(
        &mut self,
        membership: &DestinyMembership,
    ) -> Result<InventorySnapshotInput, AppError> {
        let now = self.clock.now()?;
        ensure_unexpired(now, self.credential.metadata.expires_at)?;

        let url = profile_endpoint(membership);
        let request = ProfileHttpRequest::new(&url, Some(&self.api_key), Some(&self.bearer));
        let bytes = fetch_bounded(&mut self.transport, &request, PROFILE_RESPONSE_LIMIT)?;
        decode_profile(&bytes, membership, self.definition_version.clone(), now)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BungieProfileGateway, CONTENT_TYPE_JSON, MEMBERSHIP_ENDPOINT, MEMBERSHIP_RESPONSE_LIMIT,
        PROFILE_ORIGIN, PROFILE_RESPONSE_LIMIT, ProfileApiKey, ProfileBearerToken,
        ProfileHttpRequest, ProfileHttpResponse, ProfileTransport, ProfileTransportError,
        decode_effective_membership, fetch_bounded, is_allowed_profile_url, profile_endpoint,
    };
    use destiny_core::{
        AccessCredential, AccessCredentialMetadata, AppError, Clock, CredentialMaterial,
        DefinitionVersion, DestinyMembership, ErrorCategory, MembershipId, MembershipType,
        ProfileGateway, Timestamp,
    };
    use serde_json::{Value, json};
    use std::collections::VecDeque;
    use std::io::{self, Cursor, Read};

    const MEMBERSHIP_TYPE: u64 = 3;
    const MEMBERSHIP_ID: &str = "4611686018484639825";
    const OVERRIDDEN_TYPE: u64 = 1;
    const OVERRIDDEN_ID: &str = "4611686018484639000";

    fn membership_entry(
        membership_type: Value,
        membership_id: Value,
        cross_save_override: Value,
        applicable: Value,
    ) -> Value {
        json!({
            "membershipType": membership_type,
            "membershipId": membership_id,
            "crossSaveOverride": cross_save_override,
            "applicableMembershipTypes": applicable,
            "isPublic": true,
        })
    }

    /// Wraps memberships in an account with no Cross Save relationship.
    fn no_cross_save_envelope(memberships: Value) -> Value {
        json!({
            "ErrorCode": 1,
            "Response": {
                "primaryMembershipId": Value::Null,
                "destinyMemberships": memberships,
            }
        })
    }

    /// Wraps memberships in an established Cross Save relationship whose primary
    /// is [`MEMBERSHIP_ID`].
    fn cross_save_envelope(memberships: Value) -> Value {
        json!({
            "ErrorCode": 1,
            "Response": {
                "primaryMembershipId": MEMBERSHIP_ID,
                "destinyMemberships": memberships,
            }
        })
    }

    fn decode(value: &Value) -> Result<DestinyMembership, AppError> {
        let bytes = serde_json::to_vec(value).expect("payload serializes");
        decode_effective_membership(&bytes)
    }

    fn decode_raw(text: &str) -> Result<DestinyMembership, AppError> {
        decode_effective_membership(text.as_bytes())
    }

    fn assert_validation(result: Result<DestinyMembership, AppError>) {
        let error = result.expect_err("expected a provider-validation failure");
        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
    }

    fn selected() -> DestinyMembership {
        DestinyMembership::new(
            MembershipType::new(MEMBERSHIP_TYPE as u32),
            MembershipId::new(MEMBERSHIP_ID),
        )
        .expect("fixture membership")
    }

    fn no_cross_save_entry() -> Value {
        membership_entry(
            json!(MEMBERSHIP_TYPE),
            json!(MEMBERSHIP_ID),
            json!(0),
            json!([MEMBERSHIP_TYPE]),
        )
    }

    fn overridden_entry() -> Value {
        membership_entry(
            json!(OVERRIDDEN_TYPE),
            json!(OVERRIDDEN_ID),
            json!(MEMBERSHIP_TYPE),
            json!([]),
        )
    }

    fn cross_save_primary_entry() -> Value {
        membership_entry(
            json!(MEMBERSHIP_TYPE),
            json!(MEMBERSHIP_ID),
            json!(MEMBERSHIP_TYPE),
            json!([MEMBERSHIP_TYPE, OVERRIDDEN_TYPE]),
        )
    }

    #[test]
    fn no_cross_save_single_membership_is_selected() {
        let membership = decode(&no_cross_save_envelope(json!([no_cross_save_entry()])))
            .expect("single membership");
        assert_eq!(membership, selected());
        assert_eq!(membership.membership_type(), MembershipType::new(3));
        assert_eq!(membership.membership_id().as_str(), MEMBERSHIP_ID);
    }

    #[test]
    fn cross_save_primary_is_selected_over_overridden_records() {
        let memberships = json!([overridden_entry(), cross_save_primary_entry()]);
        let membership = decode(&cross_save_envelope(memberships)).expect("cross save primary");
        assert_eq!(membership, selected());
    }

    #[test]
    fn cross_save_selection_is_deterministic_regardless_of_input_order() {
        let forward = cross_save_envelope(json!([overridden_entry(), cross_save_primary_entry()]));
        let reversed = cross_save_envelope(json!([cross_save_primary_entry(), overridden_entry()]));

        let first = decode(&forward).expect("forward order");
        let repeat = decode(&forward).expect("repeat");
        let other = decode(&reversed).expect("reversed order");
        assert_eq!(first, repeat);
        assert_eq!(first, other);
    }

    #[test]
    fn membership_id_accepts_canonical_string_and_exact_u64_forms() {
        for identifier in [json!(MEMBERSHIP_ID), json!(4611686018484639825u64)] {
            let entry = membership_entry(
                json!(MEMBERSHIP_TYPE),
                identifier,
                json!(0),
                json!([MEMBERSHIP_TYPE]),
            );
            let membership =
                decode(&no_cross_save_envelope(json!([entry]))).expect("canonical identifier");
            assert_eq!(membership.membership_id().as_str(), MEMBERSHIP_ID);
        }

        // The literal zero is the only accepted leading-zero identifier, in both
        // string and exact-integer form.
        for zero in [json!("0"), json!(0u64)] {
            let entry = membership_entry(
                json!(MEMBERSHIP_TYPE),
                zero,
                json!(0),
                json!([MEMBERSHIP_TYPE]),
            );
            decode(&no_cross_save_envelope(json!([entry]))).expect("literal zero identifier");
        }
    }

    #[test]
    fn primary_membership_id_accepts_canonical_string_and_exact_u64_forms() {
        for primary in [json!(MEMBERSHIP_ID), json!(4611686018484639825u64)] {
            let envelope = json!({
                "ErrorCode": 1,
                "Response": {
                    "primaryMembershipId": primary,
                    "destinyMemberships": [overridden_entry(), cross_save_primary_entry()],
                }
            });
            let membership = decode(&envelope).expect("canonical primary identifier");
            assert_eq!(membership, selected());
        }
    }

    #[test]
    fn is_overridden_is_not_required_and_is_ignored() {
        // The official entry does not define `isOverridden`, so its absence is
        // valid and any value present is an ignored unknown field.
        let without = membership_entry(
            json!(MEMBERSHIP_TYPE),
            json!(MEMBERSHIP_ID),
            json!(0),
            json!([MEMBERSHIP_TYPE]),
        );
        decode(&no_cross_save_envelope(json!([without]))).expect("entry without isOverridden");

        let with_mistyped = json!({
            "membershipType": MEMBERSHIP_TYPE,
            "membershipId": MEMBERSHIP_ID,
            "crossSaveOverride": 0,
            "applicableMembershipTypes": [MEMBERSHIP_TYPE],
            "isOverridden": 1,
        });
        decode(&no_cross_save_envelope(json!([with_mistyped])))
            .expect("isOverridden is an ignored unknown field");
    }

    #[test]
    fn zero_or_multiple_entries_fail_closed_without_cross_save() {
        // No entries at all.
        assert_validation(decode(&no_cross_save_envelope(json!([]))));

        // Two memberships are an MVP ambiguity.
        let other = membership_entry(
            json!(MEMBERSHIP_TYPE),
            json!("4611686018484639001"),
            json!(0),
            json!([MEMBERSHIP_TYPE]),
        );
        assert_validation(decode(&no_cross_save_envelope(json!([
            no_cross_save_entry(),
            other,
        ]))));
    }

    #[test]
    fn duplicate_membership_identity_is_rejected() {
        // A duplicated overridden record fails before any shape analysis, so the
        // rejection is the duplicate identity itself.
        let memberships = json!([
            overridden_entry(),
            overridden_entry(),
            cross_save_primary_entry(),
        ]);
        assert_validation(decode(&cross_save_envelope(memberships)));

        // The same identity repeated as the primary is also a duplicate.
        let memberships = json!([cross_save_primary_entry(), cross_save_primary_entry()]);
        assert_validation(decode(&cross_save_envelope(memberships)));
    }

    #[test]
    fn missing_primary_membership_id_is_rejected() {
        // `primaryMembershipId` must be present, even though it may be null.
        let envelope = json!({
            "ErrorCode": 1,
            "Response": { "destinyMemberships": [no_cross_save_entry()] },
        });
        assert_validation(decode(&envelope));
    }

    #[test]
    fn primary_membership_id_must_be_null_or_a_canonical_identifier() {
        for bad in [
            json!(true),
            json!(1.5),
            json!([MEMBERSHIP_ID]),
            json!({ "membershipId": MEMBERSHIP_ID }),
            json!(""),
            json!("01"),
            json!("not-a-number"),
        ] {
            let envelope = json!({
                "ErrorCode": 1,
                "Response": {
                    "primaryMembershipId": bad,
                    "destinyMemberships": [no_cross_save_entry()],
                }
            });
            assert_validation(decode(&envelope));
        }
    }

    #[test]
    fn empty_or_absent_membership_arrays_fail_closed() {
        assert_validation(decode(&no_cross_save_envelope(json!([]))));
        assert_validation(decode(&json!({
            "ErrorCode": 1,
            "Response": { "primaryMembershipId": Value::Null },
        })));
        assert_validation(decode(&json!({
            "ErrorCode": 1,
            "Response": {
                "primaryMembershipId": Value::Null,
                "destinyMemberships": Value::Null,
            },
        })));
        assert_validation(decode(&json!({
            "ErrorCode": 1,
            "Response": {
                "primaryMembershipId": Value::Null,
                "destinyMemberships": {},
            },
        })));
    }

    #[test]
    fn reserved_and_out_of_range_types_are_rejected() {
        // A reserved membership type is never a valid entry.
        for reserved in [0u64, 254] {
            let entry = membership_entry(
                json!(reserved),
                json!(MEMBERSHIP_ID),
                json!(0),
                json!([MEMBERSHIP_TYPE]),
            );
            assert_validation(decode(&no_cross_save_envelope(json!([entry]))));
        }

        // A reserved type is never applicable.
        for reserved in [0u64, 254] {
            let entry = membership_entry(
                json!(MEMBERSHIP_TYPE),
                json!(MEMBERSHIP_ID),
                json!(0),
                json!([MEMBERSHIP_TYPE, reserved]),
            );
            assert_validation(decode(&no_cross_save_envelope(json!([entry]))));
        }

        // `254` is never a valid Cross Save override.
        let entry = membership_entry(
            json!(OVERRIDDEN_TYPE),
            json!(OVERRIDDEN_ID),
            json!(254),
            json!([]),
        );
        assert_validation(decode(&cross_save_envelope(json!([
            cross_save_primary_entry(),
            entry,
        ]))));
    }

    #[test]
    fn membership_type_must_be_a_bounded_u32() {
        for bad in [
            json!(4294967296u64),
            json!(-1),
            json!(1.5),
            json!("3"),
            json!(true),
            Value::Null,
        ] {
            let entry = membership_entry(
                bad,
                json!(MEMBERSHIP_ID),
                json!(0),
                json!([MEMBERSHIP_TYPE]),
            );
            assert_validation(decode(&no_cross_save_envelope(json!([entry]))));
        }
    }

    #[test]
    fn cross_save_override_must_name_an_applicable_type_when_playable() {
        // Applicable is nonempty (the entry is playable) but the named override
        // is not among the platforms it applies to.
        let entry = membership_entry(
            json!(MEMBERSHIP_TYPE),
            json!(MEMBERSHIP_ID),
            json!(4),
            json!([MEMBERSHIP_TYPE]),
        );
        assert_validation(decode(&no_cross_save_envelope(json!([entry]))));
    }

    #[test]
    fn missing_or_wrongly_typed_required_fields_are_rejected() {
        // `isOverridden` is deliberately absent from this list: it is not a
        // required field.
        for field in [
            "membershipType",
            "membershipId",
            "crossSaveOverride",
            "applicableMembershipTypes",
        ] {
            let mut entry = no_cross_save_entry();
            entry.as_object_mut().expect("entry object").remove(field);
            assert_validation(decode(&no_cross_save_envelope(json!([entry]))));
        }

        for (cross_save_override, applicable) in [
            (json!("0"), json!([MEMBERSHIP_TYPE])),
            (json!(-1), json!([MEMBERSHIP_TYPE])),
            (json!(1.5), json!([MEMBERSHIP_TYPE])),
            (json!(0), json!("3")),
            (json!(0), Value::Null),
        ] {
            let entry = membership_entry(
                json!(MEMBERSHIP_TYPE),
                json!(MEMBERSHIP_ID),
                cross_save_override,
                applicable,
            );
            assert_validation(decode(&no_cross_save_envelope(json!([entry]))));
        }
    }

    #[test]
    fn without_cross_save_the_single_entry_must_apply_only_to_its_own_type() {
        for (cross_save_override, applicable) in [
            (json!(MEMBERSHIP_TYPE), json!([])),
            (json!(MEMBERSHIP_TYPE), json!([MEMBERSHIP_TYPE])),
            (json!(0), json!([])),
            (json!(0), json!([OVERRIDDEN_TYPE])),
            (json!(0), json!([MEMBERSHIP_TYPE, OVERRIDDEN_TYPE])),
        ] {
            let entry = membership_entry(
                json!(MEMBERSHIP_TYPE),
                json!(MEMBERSHIP_ID),
                cross_save_override,
                applicable,
            );
            assert_validation(decode(&no_cross_save_envelope(json!([entry]))));
        }
    }

    #[test]
    fn membership_id_must_be_canonical_and_bounded() {
        for bad in [
            json!(""),
            json!("00"),
            json!("01"),
            json!("007"),
            json!("1a"),
            json!("-1"),
            json!(" 1"),
            json!("\u{661}\u{662}\u{663}"),
            json!(4611686018484639825.0f64),
            json!(-1),
            json!(true),
            json!([1]),
            Value::Null,
        ] {
            let entry = membership_entry(
                json!(MEMBERSHIP_TYPE),
                bad,
                json!(0),
                json!([MEMBERSHIP_TYPE]),
            );
            assert_validation(decode(&no_cross_save_envelope(json!([entry]))));
        }

        // 128 digits are accepted; 129 are rejected.
        let at_limit = "9".repeat(128);
        let entry = membership_entry(
            json!(MEMBERSHIP_TYPE),
            json!(at_limit),
            json!(0),
            json!([MEMBERSHIP_TYPE]),
        );
        let membership =
            decode(&no_cross_save_envelope(json!([entry]))).expect("128-digit identifier");
        assert_eq!(membership.membership_id().as_str(), at_limit);

        let over_limit = "9".repeat(129);
        let entry = membership_entry(
            json!(MEMBERSHIP_TYPE),
            json!(over_limit),
            json!(0),
            json!([MEMBERSHIP_TYPE]),
        );
        assert_validation(decode(&no_cross_save_envelope(json!([entry]))));
    }

    #[test]
    fn applicable_membership_types_must_be_unique_and_valid() {
        let entry = membership_entry(
            json!(MEMBERSHIP_TYPE),
            json!(MEMBERSHIP_ID),
            json!(0),
            json!([MEMBERSHIP_TYPE, MEMBERSHIP_TYPE]),
        );
        assert_validation(decode(&no_cross_save_envelope(json!([entry]))));

        for bad in [
            json!(["3"]),
            json!([1.5]),
            json!([-1]),
            json!([4294967296u64]),
            json!([true]),
            json!([Value::Null]),
        ] {
            let entry =
                membership_entry(json!(MEMBERSHIP_TYPE), json!(MEMBERSHIP_ID), json!(0), bad);
            assert_validation(decode(&no_cross_save_envelope(json!([entry]))));
        }
    }

    #[test]
    fn cross_save_shapes_must_be_internally_consistent() {
        // The primary must apply to at least one platform.
        let primary = membership_entry(
            json!(MEMBERSHIP_TYPE),
            json!(MEMBERSHIP_ID),
            json!(MEMBERSHIP_TYPE),
            json!([]),
        );
        assert_validation(decode(&cross_save_envelope(json!([primary]))));

        // The primary must name its own membership type as the override.
        let primary = membership_entry(
            json!(MEMBERSHIP_TYPE),
            json!(MEMBERSHIP_ID),
            json!(OVERRIDDEN_TYPE),
            json!([MEMBERSHIP_TYPE, OVERRIDDEN_TYPE]),
        );
        assert_validation(decode(&cross_save_envelope(json!([
            overridden_entry(),
            primary,
        ]))));

        // An applicable platform with no matching entry is rejected.
        assert_validation(decode(&cross_save_envelope(json!([
            cross_save_primary_entry()
        ]))));

        // An entry outside the applicable set is rejected even when the counts
        // happen to match.
        let outside = membership_entry(
            json!(5),
            json!("4611686018484639002"),
            json!(MEMBERSHIP_TYPE),
            json!([]),
        );
        assert_validation(decode(&cross_save_envelope(json!([
            cross_save_primary_entry(),
            outside,
        ]))));

        // More entries than applicable platforms is rejected.
        assert_validation(decode(&cross_save_envelope(json!([
            overridden_entry(),
            cross_save_primary_entry(),
            membership_entry(
                json!(5),
                json!("4611686018484639002"),
                json!(MEMBERSHIP_TYPE),
                json!([]),
            ),
        ]))));

        // Two entries sharing a membership type are rejected even when the
        // counts match the applicable set.
        let duplicate_type = membership_entry(
            json!(MEMBERSHIP_TYPE),
            json!("4611686018484639003"),
            json!(MEMBERSHIP_TYPE),
            json!([]),
        );
        assert_validation(decode(&cross_save_envelope(json!([
            cross_save_primary_entry(),
            duplicate_type,
        ]))));

        // Another entry with a nonempty applicable list is rejected.
        let another = membership_entry(
            json!(OVERRIDDEN_TYPE),
            json!(OVERRIDDEN_ID),
            json!(OVERRIDDEN_TYPE),
            json!([OVERRIDDEN_TYPE]),
        );
        assert_validation(decode(&cross_save_envelope(json!([
            cross_save_primary_entry(),
            another,
        ]))));

        // Another entry whose override does not name the primary is rejected.
        let wrong_override = membership_entry(
            json!(OVERRIDDEN_TYPE),
            json!(OVERRIDDEN_ID),
            json!(0),
            json!([]),
        );
        assert_validation(decode(&cross_save_envelope(json!([
            cross_save_primary_entry(),
            wrong_override,
        ]))));
    }

    #[test]
    fn primary_membership_id_must_match_exactly_one_entry() {
        // A primary identifier that matches no entry is rejected.
        let envelope = json!({
            "ErrorCode": 1,
            "Response": {
                "primaryMembershipId": "4611686018484639999",
                "destinyMemberships": [overridden_entry(), cross_save_primary_entry()],
            }
        });
        assert_validation(decode(&envelope));

        // The same membership identifier carried by two types is ambiguous.
        let primary = membership_entry(
            json!(MEMBERSHIP_TYPE),
            json!(MEMBERSHIP_ID),
            json!(MEMBERSHIP_TYPE),
            json!([MEMBERSHIP_TYPE, OVERRIDDEN_TYPE]),
        );
        let shadow = membership_entry(
            json!(5),
            json!(MEMBERSHIP_ID),
            json!(MEMBERSHIP_TYPE),
            json!([]),
        );
        assert_validation(decode(&cross_save_envelope(json!([primary, shadow]))));
    }

    #[test]
    fn envelope_failures_are_provider_validation() {
        assert_validation(decode_raw("{not json"));
        assert_validation(decode_raw("[]"));
        assert_validation(decode_raw("{}"));
        assert_validation(decode_raw("null"));

        assert_validation(decode(&json!({
            "ErrorCode": 0,
            "Response": { "primaryMembershipId": Value::Null, "destinyMemberships": [] },
        })));
        assert_validation(decode(&json!({
            "ErrorCode": 7,
            "ErrorStatus": "ParameterParseFailure",
            "Message": "denied",
            "Response": {},
        })));
        assert_validation(decode(&json!({ "ErrorCode": "1", "Response": {} })));
        assert_validation(decode(&json!({ "Response": {} })));
        assert_validation(decode(&json!({ "ErrorCode": 1, "Response": [] })));
        assert_validation(decode(&json!({ "ErrorCode": 1, "Response": Value::Null })));
    }

    #[test]
    fn failures_never_leak_input_sentinels() {
        let sentinel = "sentinel-membership-secret";
        let entry = membership_entry(
            json!(MEMBERSHIP_TYPE),
            json!(sentinel),
            json!(0),
            json!([MEMBERSHIP_TYPE]),
        );
        let error =
            decode(&no_cross_save_envelope(json!([entry]))).expect_err("malformed identifier");
        assert_eq!(error, AppError::new(ErrorCategory::ProviderValidation));
        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));

        // A successfully decoded membership redacts the identifier in Debug.
        let membership =
            decode(&no_cross_save_envelope(json!([no_cross_save_entry()]))).expect("membership");
        let rendered = format!("{membership:?}");
        assert!(rendered.contains("DestinyMembership"));
        assert!(!rendered.contains(MEMBERSHIP_ID));
    }

    const API_KEY_SENTINEL: &str = "sentinel-profile-api-key";
    const BEARER_SENTINEL: &str = "sentinel-profile-bearer-token";
    const PROFILE_QUERY: &str = "?components=102,200,201,205,300,302";
    const PROFILE_PREFIX: &str =
        "https://www.bungie.net/Platform/Destiny2/3/Profile/4611686018484639825/";
    const RESPONSE_BODY: &str = "{\"ErrorCode\":1,\"Response\":{}}";
    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/profile-complete.json");
    const DEFINITION_VERSION: &str = "manifest-fixture-v1";
    const GATEWAY_NOW: u64 = 1_724_000_000;
    const GATEWAY_EXPIRES_AT: u64 = 1_724_003_600;

    fn api_key() -> ProfileApiKey {
        ProfileApiKey::new(API_KEY_SENTINEL).expect("fixture API key")
    }

    fn bearer() -> ProfileBearerToken {
        ProfileBearerToken::new(BEARER_SENTINEL).expect("fixture bearer token")
    }

    struct RecordedProfileRequest {
        url: String,
        api_key: Option<String>,
        bearer: Option<String>,
    }

    struct FakeProfileTransport {
        responses: VecDeque<Result<ProfileHttpResponse, ProfileTransportError>>,
        recorded: Vec<RecordedProfileRequest>,
    }

    impl FakeProfileTransport {
        fn with(response: Result<ProfileHttpResponse, ProfileTransportError>) -> Self {
            Self::queue(vec![response])
        }

        fn queue(responses: Vec<Result<ProfileHttpResponse, ProfileTransportError>>) -> Self {
            Self {
                responses: VecDeque::from(responses),
                recorded: Vec::new(),
            }
        }

        fn last_request(&self) -> &RecordedProfileRequest {
            self.recorded.last().expect("a request was recorded")
        }

        fn recorded(&self) -> &[RecordedProfileRequest] {
            &self.recorded
        }
    }

    impl ProfileTransport for FakeProfileTransport {
        fn get(
            &mut self,
            request: &ProfileHttpRequest<'_>,
        ) -> Result<ProfileHttpResponse, ProfileTransportError> {
            self.recorded.push(RecordedProfileRequest {
                url: request.url().to_owned(),
                api_key: request.api_key().map(|key| key.expose().to_owned()),
                bearer: request.bearer().map(|token| token.expose().to_owned()),
            });
            self.responses
                .pop_front()
                .unwrap_or(Err(ProfileTransportError::Unavailable))
        }
    }

    /// A reader that fails if the helper ever attempts to read the body.
    struct RefusesRead;

    impl Read for RefusesRead {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("body read was attempted"))
        }
    }

    /// A reader that yields at most two bytes per `read` call.
    struct DribbleReader {
        remaining: usize,
    }

    impl Read for DribbleReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.remaining == 0 || buffer.is_empty() {
                return Ok(0);
            }
            let count = self.remaining.min(2).min(buffer.len());
            for byte in &mut buffer[..count] {
                *byte = b'x';
            }
            self.remaining -= count;
            Ok(count)
        }
    }

    fn response_with_content_type(
        status: u16,
        content_type: Option<&str>,
        body: &str,
    ) -> ProfileHttpResponse {
        ProfileHttpResponse::new(
            status,
            Some(body.len() as u64),
            content_type.map(str::to_owned),
            Box::new(Cursor::new(body.as_bytes().to_vec())),
        )
    }

    fn json_response(status: u16, body: &str) -> ProfileHttpResponse {
        response_with_content_type(status, Some(CONTENT_TYPE_JSON), body)
    }

    /// Builds a JSON response whose declared length matches the delivered bytes.
    fn json_response_bytes(status: u16, body: &[u8]) -> ProfileHttpResponse {
        ProfileHttpResponse::new(
            status,
            Some(body.len() as u64),
            Some(CONTENT_TYPE_JSON.to_owned()),
            Box::new(Cursor::new(body.to_vec())),
        )
    }

    /// The Cross Save membership payload: one overridden entry plus the primary.
    fn membership_body() -> Vec<u8> {
        serde_json::to_vec(&cross_save_envelope(json!([
            overridden_entry(),
            cross_save_primary_entry(),
        ])))
        .expect("membership body serializes")
    }

    struct FakeClock(u64);

    impl Clock for FakeClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            Ok(Timestamp::new(self.0))
        }
    }

    fn credential(expires_at: u64) -> AccessCredential {
        AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(expires_at),
            },
            material: CredentialMaterial::from_bytes(BEARER_SENTINEL.as_bytes().to_vec()),
        }
    }

    type TestGateway = BungieProfileGateway<FakeProfileTransport, FakeClock>;

    fn gateway(
        responses: Vec<Result<ProfileHttpResponse, ProfileTransportError>>,
        now: u64,
        expires_at: u64,
    ) -> TestGateway {
        BungieProfileGateway::new(
            FakeProfileTransport::queue(responses),
            FakeClock(now),
            credential(expires_at),
            api_key(),
            DefinitionVersion::new(DEFINITION_VERSION),
        )
        .expect("valid gateway configuration")
    }

    fn run_fetch_on(
        url: &str,
        mut transport: FakeProfileTransport,
        limit: usize,
    ) -> (Result<Vec<u8>, AppError>, FakeProfileTransport) {
        let key = api_key();
        let token = bearer();
        let request = ProfileHttpRequest::new(url, Some(&key), Some(&token));
        let result = fetch_bounded(&mut transport, &request, limit);
        (result, transport)
    }

    fn fetch_category_on(
        url: &str,
        transport: FakeProfileTransport,
        limit: usize,
    ) -> ErrorCategory {
        run_fetch_on(url, transport, limit)
            .0
            .expect_err("expected a fetch failure")
            .category()
    }

    #[test]
    fn profile_endpoint_is_the_exact_allowlisted_shape() {
        let url = profile_endpoint(&selected());
        assert_eq!(url, format!("{PROFILE_PREFIX}{PROFILE_QUERY}"));
        assert!(is_allowed_profile_url(&url));
        assert_eq!(
            url,
            "https://www.bungie.net/Platform/Destiny2/3/Profile/4611686018484639825/?components=102,200,201,205,300,302"
        );
    }

    #[test]
    fn response_caps_match_the_declared_bounds() {
        assert_eq!(MEMBERSHIP_RESPONSE_LIMIT, 256 * 1024);
        assert_eq!(PROFILE_RESPONSE_LIMIT, 16 * 1024 * 1024);
        assert!(!MEMBERSHIP_ENDPOINT.is_empty());
        assert_eq!(PROFILE_ORIGIN, "https://www.bungie.net");
    }

    #[test]
    fn request_and_response_accessors_round_trip() {
        let key = api_key();
        let token = bearer();
        let url = profile_endpoint(&selected());
        let request = ProfileHttpRequest::new(&url, Some(&key), Some(&token));
        assert_eq!(request.url(), url.as_str());
        assert_eq!(
            request.api_key().map(ProfileApiKey::expose),
            Some(API_KEY_SENTINEL)
        );
        assert_eq!(
            request.bearer().map(ProfileBearerToken::expose),
            Some(BEARER_SENTINEL)
        );

        let absent = ProfileHttpRequest::new(MEMBERSHIP_ENDPOINT, None, None);
        assert_eq!(absent.url(), MEMBERSHIP_ENDPOINT);
        assert!(absent.api_key().is_none());
        assert!(absent.bearer().is_none());

        let response = ProfileHttpResponse::new(
            200,
            Some(RESPONSE_BODY.len() as u64),
            Some(CONTENT_TYPE_JSON.to_owned()),
            Box::new(Cursor::new(RESPONSE_BODY.as_bytes().to_vec())),
        );
        assert_eq!(response.status(), 200);
        assert_eq!(response.content_length(), Some(RESPONSE_BODY.len() as u64));
        assert_eq!(response.content_type(), Some(CONTENT_TYPE_JSON));
    }

    #[test]
    fn secrets_and_payloads_never_appear_in_debug() {
        assert_eq!(format!("{:?}", api_key()), "ProfileApiKey");
        assert_eq!(format!("{:?}", bearer()), "ProfileBearerToken");

        let key = api_key();
        let token = bearer();
        let request = ProfileHttpRequest::new(MEMBERSHIP_ENDPOINT, Some(&key), Some(&token));
        let rendered = format!("{request:?}");
        assert!(rendered.contains("ProfileHttpRequest"));
        assert!(rendered.contains("GET"));
        assert!(rendered.contains(MEMBERSHIP_ENDPOINT));
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains(API_KEY_SENTINEL));
        assert!(!rendered.contains(BEARER_SENTINEL));

        let response = ProfileHttpResponse::new(
            200,
            Some(8),
            Some("application/json; boundary=sentinel-content-type".to_owned()),
            Box::new(Cursor::new(b"sentinel-body".to_vec())),
        );
        let rendered = format!("{response:?}");
        assert!(rendered.contains("ProfileHttpResponse"));
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("sentinel-content-type"));
        assert!(!rendered.contains("sentinel-body"));
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
                ProfileApiKey::new(invalid)
                    .expect_err("invalid API key")
                    .category(),
                ErrorCategory::ConfigurationError,
                "API key {invalid:?} must be rejected"
            );
            assert_eq!(
                ProfileBearerToken::new(invalid)
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
            ProfileApiKey::new(valid).expect("valid API key");
            ProfileBearerToken::new(valid).expect("valid bearer token");
        }
    }

    #[test]
    fn allowlist_accepts_only_the_two_exact_endpoints() {
        assert!(is_allowed_profile_url(MEMBERSHIP_ENDPOINT));
        assert!(is_allowed_profile_url(&profile_endpoint(&selected())));

        let encoded_components =
            format!("{PROFILE_PREFIX}?components=102%2C200%2C201%2C205%2C300%2C302");
        let negatives = [
            String::new(),
            // Current-memberships endpoint variants.
            "https://www.bungie.net/Platform/User/GetMembershipsForCurrentUser".to_owned(),
            "HTTP://www.bungie.net/Platform/User/GetMembershipsForCurrentUser/".to_owned(),
            "https://www.bungie.net/platform/User/GetMembershipsForCurrentUser/".to_owned(),
            "https://www.bungie.net/Platform/User/GetMembershipsForCurrentUser/?x=1".to_owned(),
            "https://www.bungie.net/Platform/User/GetMembershipsForCurrentUser/#f".to_owned(),
            "https://www.bungie.net/Platform/User/GetMembershipsForCurrentUser/extra".to_owned(),
            "https://www.bungie.net/Platform/User/GetMembershipsForCurrentUserx".to_owned(),
            "https://evil.example/Platform/User/GetMembershipsForCurrentUser/".to_owned(),
            "https://www.bungie.net.evil.example/Platform/User/GetMembershipsForCurrentUser/"
                .to_owned(),
            "https://evil@www.bungie.net/Platform/User/GetMembershipsForCurrentUser/".to_owned(),
            "https://www.bungie.net:443/Platform/User/GetMembershipsForCurrentUser/".to_owned(),
            "https://WWW.BUNGIE.NET/Platform/User/GetMembershipsForCurrentUser/".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/Manifest/".to_owned(),
            // Profile endpoint query and path variants.
            format!("{PROFILE_PREFIX}{PROFILE_QUERY}&extra=1"),
            format!("{PROFILE_PREFIX}?components=102"),
            format!("{PROFILE_PREFIX}?components=102,200,201,205,300,302,304"),
            format!("{PROFILE_PREFIX}?components=102,200,201,205,302,300"),
            format!("{PROFILE_PREFIX}?components=102,102,200,201,205,300,302"),
            format!("{PROFILE_PREFIX}?components=102,200,201,205,300"),
            format!("{PROFILE_PREFIX}?components="),
            encoded_components,
            format!("{PROFILE_PREFIX}{PROFILE_QUERY}#fragment"),
            format!("{PROFILE_PREFIX}?components=102,200,201,205,300,302&"),
            format!("{PROFILE_PREFIX}?components=102,200,201,205,300,302 "),
            "https://www.bungie.net/Platform/Destiny2/3/Profile/4611686018484639825?components=102,200,201,205,300,302".to_owned(),
            PROFILE_PREFIX.to_owned(),
            "https://www.bungie.net/Platform/Destiny2/3/Profile//?components=102,200,201,205,300,302".to_owned(),
            "https://www.bungie.net/Platform/Destiny2//Profile/4611686018484639825/?components=102,200,201,205,300,302".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/3/Profile/4611686018484639825/extra/?components=102,200,201,205,300,302".to_owned(),
            // Non-canonical or reserved membership type and identifier.
            "https://www.bungie.net/Platform/Destiny2/03/Profile/4611686018484639825/?components=102,200,201,205,300,302".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/0/Profile/4611686018484639825/?components=102,200,201,205,300,302".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/254/Profile/4611686018484639825/?components=102,200,201,205,300,302".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/4294967296/Profile/4611686018484639825/?components=102,200,201,205,300,302".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/3/Profile/04611686018484639825/?components=102,200,201,205,300,302".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/3/Profile/46116-860184/?components=102,200,201,205,300,302".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/3/Profile/46116%2F860184/?components=102,200,201,205,300,302".to_owned(),
            "https://www.bungie.net/Platform/Destiny2/3/profile/4611686018484639825/?components=102,200,201,205,300,302".to_owned(),
        ];
        for url in negatives {
            assert!(!is_allowed_profile_url(&url), "should reject {url}");
        }
    }

    #[test]
    fn json_content_type_accepts_optional_parameters() {
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
            let (result, _) = run_fetch_on(
                MEMBERSHIP_ENDPOINT,
                FakeProfileTransport::with(Ok(response_with_content_type(
                    200,
                    Some(content_type),
                    RESPONSE_BODY,
                ))),
                MEMBERSHIP_RESPONSE_LIMIT,
            );
            assert!(
                result.is_ok(),
                "content type {content_type:?} must be accepted"
            );
        }
    }

    #[test]
    fn missing_or_malformed_content_type_is_provider_validation() {
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
            assert_eq!(
                fetch_category_on(
                    MEMBERSHIP_ENDPOINT,
                    FakeProfileTransport::with(Ok(response_with_content_type(
                        200,
                        content_type,
                        RESPONSE_BODY,
                    ))),
                    MEMBERSHIP_RESPONSE_LIMIT,
                ),
                ErrorCategory::ProviderValidation,
                "content type {content_type:?} must be rejected"
            );
        }
    }

    #[test]
    fn authentication_rate_limit_and_server_statuses_map_to_typed_errors() {
        for status in [401u16, 403] {
            assert_eq!(
                fetch_category_on(
                    MEMBERSHIP_ENDPOINT,
                    FakeProfileTransport::with(Ok(json_response(status, "denied"))),
                    MEMBERSHIP_RESPONSE_LIMIT,
                ),
                ErrorCategory::ProviderAuthentication,
                "status {status} must require authentication"
            );
        }
        assert_eq!(
            fetch_category_on(
                MEMBERSHIP_ENDPOINT,
                FakeProfileTransport::with(Ok(json_response(429, "slow down"))),
                MEMBERSHIP_RESPONSE_LIMIT,
            ),
            ErrorCategory::ProviderRateLimit
        );
        for status in [500u16, 503] {
            assert_eq!(
                fetch_category_on(
                    MEMBERSHIP_ENDPOINT,
                    FakeProfileTransport::with(Ok(json_response(status, "boom"))),
                    MEMBERSHIP_RESPONSE_LIMIT,
                ),
                ErrorCategory::ProviderUnavailable,
                "status {status} must be unavailable"
            );
        }
    }

    #[test]
    fn redirects_and_other_non_success_statuses_are_validation() {
        for status in [301u16, 302, 307, 400, 404, 201, 204] {
            assert_eq!(
                fetch_category_on(
                    MEMBERSHIP_ENDPOINT,
                    FakeProfileTransport::with(Ok(json_response(status, "unexpected"))),
                    MEMBERSHIP_RESPONSE_LIMIT,
                ),
                ErrorCategory::ProviderValidation,
                "status {status} must be validation"
            );
        }
    }

    #[test]
    fn transport_failures_map_to_unavailable_and_configuration() {
        for error in [
            ProfileTransportError::Timeout,
            ProfileTransportError::Unavailable,
        ] {
            assert_eq!(
                fetch_category_on(
                    MEMBERSHIP_ENDPOINT,
                    FakeProfileTransport::with(Err(error)),
                    MEMBERSHIP_RESPONSE_LIMIT,
                ),
                ErrorCategory::ProviderUnavailable
            );
        }
        assert_eq!(
            fetch_category_on(
                MEMBERSHIP_ENDPOINT,
                FakeProfileTransport::with(Err(ProfileTransportError::Configuration)),
                MEMBERSHIP_RESPONSE_LIMIT,
            ),
            ErrorCategory::ConfigurationError
        );
    }

    #[test]
    fn announced_oversize_is_rejected_without_reading_the_body() {
        let profile_url = profile_endpoint(&selected());
        for (url, limit) in [
            (MEMBERSHIP_ENDPOINT, MEMBERSHIP_RESPONSE_LIMIT),
            (profile_url.as_str(), PROFILE_RESPONSE_LIMIT),
        ] {
            let response = ProfileHttpResponse::new(
                200,
                Some(limit as u64 + 1),
                Some(CONTENT_TYPE_JSON.to_owned()),
                Box::new(RefusesRead),
            );
            assert_eq!(
                fetch_category_on(url, FakeProfileTransport::with(Ok(response)), limit),
                ErrorCategory::ProviderValidation,
                "declared oversize at limit {limit} must be rejected before reading"
            );
        }
    }

    #[test]
    fn unannounced_oversize_is_rejected_from_bytes_read() {
        let profile_url = profile_endpoint(&selected());
        for (url, limit) in [
            (MEMBERSHIP_ENDPOINT, MEMBERSHIP_RESPONSE_LIMIT),
            (profile_url.as_str(), PROFILE_RESPONSE_LIMIT),
        ] {
            let response = ProfileHttpResponse::new(
                200,
                None,
                Some(CONTENT_TYPE_JSON.to_owned()),
                Box::new(io::repeat(0x78)),
            );
            assert_eq!(
                fetch_category_on(url, FakeProfileTransport::with(Ok(response)), limit),
                ErrorCategory::ProviderValidation,
                "actual oversize at limit {limit} must be rejected"
            );
        }
    }

    #[test]
    fn exact_cap_boundary_is_accepted() {
        let profile_url = profile_endpoint(&selected());
        for (url, limit) in [
            (MEMBERSHIP_ENDPOINT, MEMBERSHIP_RESPONSE_LIMIT),
            (profile_url.as_str(), PROFILE_RESPONSE_LIMIT),
        ] {
            let response = ProfileHttpResponse::new(
                200,
                Some(limit as u64),
                Some(CONTENT_TYPE_JSON.to_owned()),
                Box::new(Cursor::new(vec![0x78u8; limit])),
            );
            let (result, _) = run_fetch_on(url, FakeProfileTransport::with(Ok(response)), limit);
            assert_eq!(
                result.expect("boundary body").len(),
                limit,
                "a body exactly at limit {limit} must be accepted"
            );
        }
    }

    #[test]
    fn declared_length_mismatch_is_provider_validation() {
        // A declared length is an exact promise: a body that ends early is a
        // truncated response and one that runs long is a padded response. Both
        // mismatch directions fail closed within the cap.
        for (declared, delivered) in [(10u64, 0usize), (10, 3), (10, 9), (3, 4), (0, 1)] {
            let body = vec![b'x'; delivered];
            let response = ProfileHttpResponse::new(
                200,
                Some(declared),
                Some(CONTENT_TYPE_JSON.to_owned()),
                Box::new(Cursor::new(body)),
            );
            assert_eq!(
                fetch_category_on(
                    MEMBERSHIP_ENDPOINT,
                    FakeProfileTransport::with(Ok(response)),
                    MEMBERSHIP_RESPONSE_LIMIT,
                ),
                ErrorCategory::ProviderValidation,
                "a {delivered}-byte body under a declared {declared} must fail closed"
            );
        }

        // Delivering exactly the declared length is accepted.
        let response = ProfileHttpResponse::new(
            200,
            Some(5),
            Some(CONTENT_TYPE_JSON.to_owned()),
            Box::new(Cursor::new(b"abcde".to_vec())),
        );
        let (result, _) = run_fetch_on(
            MEMBERSHIP_ENDPOINT,
            FakeProfileTransport::with(Ok(response)),
            MEMBERSHIP_RESPONSE_LIMIT,
        );
        assert_eq!(result.expect("exact declared length").len(), 5);
    }

    #[test]
    fn absent_content_length_accepts_observed_bytes() {
        // Without a declared length the reader decides the end of the body, and
        // tiny chunks are still assembled in full.
        let response = ProfileHttpResponse::new(
            200,
            None,
            Some(CONTENT_TYPE_JSON.to_owned()),
            Box::new(DribbleReader { remaining: 5 }),
        );
        let (result, _) = run_fetch_on(
            MEMBERSHIP_ENDPOINT,
            FakeProfileTransport::with(Ok(response)),
            MEMBERSHIP_RESPONSE_LIMIT,
        );
        assert_eq!(result.expect("dribbled body"), vec![b'x'; 5]);
    }

    #[test]
    fn fetch_passes_the_request_through_unchanged() {
        let (result, transport) = run_fetch_on(
            MEMBERSHIP_ENDPOINT,
            FakeProfileTransport::with(Ok(json_response(200, RESPONSE_BODY))),
            MEMBERSHIP_RESPONSE_LIMIT,
        );
        result.expect("successful fetch");
        let recorded = transport.last_request();
        assert_eq!(recorded.url, MEMBERSHIP_ENDPOINT);
        assert_eq!(recorded.api_key.as_deref(), Some(API_KEY_SENTINEL));
        assert_eq!(recorded.bearer.as_deref(), Some(BEARER_SENTINEL));
    }

    #[test]
    fn gateway_resolves_cross_save_membership_then_loads_the_complete_fixture() {
        let mut gateway = gateway(
            vec![
                Ok(json_response_bytes(200, &membership_body())),
                Ok(json_response_bytes(200, FIXTURE)),
            ],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );

        let membership = gateway.resolve_membership().expect("effective membership");
        assert_eq!(membership, selected());

        let snapshot = gateway
            .load_inventory(&membership)
            .expect("complete inventory candidate");
        assert_eq!(snapshot.membership_id.as_str(), MEMBERSHIP_ID);
        assert_eq!(snapshot.membership_type, MembershipType::new(3));
        assert_eq!(
            snapshot.definition_version,
            DefinitionVersion::new(DEFINITION_VERSION)
        );
        assert_eq!(snapshot.source, destiny_core::SourceMarker::Profile);
        assert_eq!(snapshot.characters.len(), 2);
        assert_eq!(snapshot.items.len(), 6);

        // Exactly two GET requests, each to the one exact allowlisted URL.
        let recorded = gateway.transport().recorded();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].url, MEMBERSHIP_ENDPOINT);
        assert_eq!(recorded[1].url, profile_endpoint(&selected()));
        assert!(recorded[1].url.ends_with(PROFILE_QUERY));
    }

    #[test]
    fn gateway_requests_carry_the_exact_secret_accessor_values() {
        let mut gateway = gateway(
            vec![
                Ok(json_response_bytes(200, &membership_body())),
                Ok(json_response_bytes(200, FIXTURE)),
            ],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );
        let membership = gateway.resolve_membership().expect("membership");
        gateway.load_inventory(&membership).expect("snapshot");

        // The recorded values are read through the trusted request accessors of
        // the requests the gateway actually constructed.
        for recorded in gateway.transport().recorded() {
            assert_eq!(recorded.api_key.as_deref(), Some(API_KEY_SENTINEL));
            assert_eq!(recorded.bearer.as_deref(), Some(BEARER_SENTINEL));
        }
    }

    #[test]
    fn gateway_construction_rejects_invalid_credential_material() {
        // Empty, non-UTF-8, whitespace-bearing, and control-bearing material are
        // sanitized configuration failures.
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
            };
            let error = BungieProfileGateway::new(
                FakeProfileTransport::queue(vec![]),
                FakeClock(GATEWAY_NOW),
                credential,
                api_key(),
                DefinitionVersion::new(DEFINITION_VERSION),
            )
            .expect_err("invalid credential material must be rejected");
            assert_eq!(error.category(), ErrorCategory::ConfigurationError);
            assert!(!format!("{error:?}").contains(BEARER_SENTINEL));
        }
    }

    #[test]
    fn gateway_debug_never_exposes_secrets() {
        let gateway = gateway(vec![], GATEWAY_NOW, GATEWAY_EXPIRES_AT);
        let rendered = format!("{gateway:?}");
        assert!(rendered.contains("BungieProfileGateway"));
        assert!(!rendered.contains(API_KEY_SENTINEL));
        assert!(!rendered.contains(BEARER_SENTINEL));
    }

    #[test]
    fn gateway_forwards_one_request_time_now_to_the_decoder() {
        let mut gateway = gateway(
            vec![Ok(json_response_bytes(200, FIXTURE))],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );
        let snapshot = gateway.load_inventory(&selected()).expect("snapshot");
        assert_eq!(snapshot.checked_at, Timestamp::new(GATEWAY_NOW));
        assert_eq!(
            snapshot.definition_version,
            DefinitionVersion::new(DEFINITION_VERSION)
        );
    }

    #[test]
    fn expired_credential_refuses_both_operations_without_a_request() {
        // `now == expires_at` is treated as expired, and the refusal precedes any
        // request construction.
        for now in [GATEWAY_EXPIRES_AT, GATEWAY_EXPIRES_AT + 1] {
            let mut resolver = gateway(vec![], now, GATEWAY_EXPIRES_AT);
            assert_eq!(
                resolver
                    .resolve_membership()
                    .expect_err("expired resolve")
                    .category(),
                ErrorCategory::ProviderAuthentication
            );
            assert!(resolver.transport().recorded().is_empty());

            let mut loader = gateway(vec![], now, GATEWAY_EXPIRES_AT);
            assert_eq!(
                loader
                    .load_inventory(&selected())
                    .expect_err("expired load")
                    .category(),
                ErrorCategory::ProviderAuthentication
            );
            assert!(loader.transport().recorded().is_empty());
        }
    }

    #[test]
    fn gateway_maps_representative_resolve_failures() {
        let cases: Vec<(ProfileHttpResponse, ErrorCategory)> = vec![
            (
                json_response(401, "denied"),
                ErrorCategory::ProviderAuthentication,
            ),
            (
                json_response(403, "denied"),
                ErrorCategory::ProviderAuthentication,
            ),
            (json_response(429, "slow"), ErrorCategory::ProviderRateLimit),
            (
                json_response(503, "boom"),
                ErrorCategory::ProviderUnavailable,
            ),
            (
                json_response(404, "missing"),
                ErrorCategory::ProviderValidation,
            ),
            (
                response_with_content_type(200, Some("text/html"), RESPONSE_BODY),
                ErrorCategory::ProviderValidation,
            ),
            (
                // A declared body larger than the membership cap is rejected
                // before it is read.
                ProfileHttpResponse::new(
                    200,
                    Some(MEMBERSHIP_RESPONSE_LIMIT as u64 + 1),
                    Some(CONTENT_TYPE_JSON.to_owned()),
                    Box::new(RefusesRead),
                ),
                ErrorCategory::ProviderValidation,
            ),
            (
                // A truncated body under a declared length fails closed.
                ProfileHttpResponse::new(
                    200,
                    Some(64),
                    Some(CONTENT_TYPE_JSON.to_owned()),
                    Box::new(Cursor::new(b"{}".to_vec())),
                ),
                ErrorCategory::ProviderValidation,
            ),
        ];

        for (response, expected) in cases {
            let mut gateway = gateway(vec![Ok(response)], GATEWAY_NOW, GATEWAY_EXPIRES_AT);
            assert_eq!(
                gateway
                    .resolve_membership()
                    .expect_err("resolve fails")
                    .category(),
                expected
            );
        }
    }

    #[test]
    fn gateway_maps_transport_failures() {
        for (error, expected) in [
            (
                ProfileTransportError::Timeout,
                ErrorCategory::ProviderUnavailable,
            ),
            (
                ProfileTransportError::Unavailable,
                ErrorCategory::ProviderUnavailable,
            ),
            (
                ProfileTransportError::Configuration,
                ErrorCategory::ConfigurationError,
            ),
        ] {
            let mut gateway = gateway(vec![Err(error)], GATEWAY_NOW, GATEWAY_EXPIRES_AT);
            assert_eq!(
                gateway
                    .resolve_membership()
                    .expect_err("resolve fails")
                    .category(),
                expected
            );
        }
    }

    #[test]
    fn gateway_load_uses_the_profile_cap() {
        let profile_url = profile_endpoint(&selected());
        let mut gateway = gateway(
            vec![Ok(ProfileHttpResponse::new(
                200,
                Some(PROFILE_RESPONSE_LIMIT as u64 + 1),
                Some(CONTENT_TYPE_JSON.to_owned()),
                Box::new(RefusesRead),
            ))],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );
        assert_eq!(
            gateway
                .load_inventory(&selected())
                .expect_err("oversize profile")
                .category(),
            ErrorCategory::ProviderValidation
        );
        assert_eq!(gateway.transport().last_request().url, profile_url);
    }

    #[test]
    fn gateway_failures_never_leak_provider_sentinels() {
        let sentinel = "sentinel-provider-payload";
        let bad_body =
            format!("{{\"ErrorCode\":1,\"Response\":{{\"destinyMemberships\":\"{sentinel}\"}}}}");
        let mut gateway = gateway(
            vec![Ok(json_response(200, &bad_body))],
            GATEWAY_NOW,
            GATEWAY_EXPIRES_AT,
        );
        let error = gateway
            .resolve_membership()
            .expect_err("malformed membership");
        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));
        assert!(!format!("{gateway:?}").contains(sentinel));
        assert!(!format!("{gateway:?}").contains(BEARER_SENTINEL));
    }
}
