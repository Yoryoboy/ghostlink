//! Public and confidential runtime configuration and callback validation.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use destiny_core::{AppError, ClientSecret, DefinitionVersion, FreshnessPolicy};
use url::Url;

use crate::defaults::{CompiledDefaults, OFFICIAL};

mod file;
pub(crate) mod paths;

const API_KEY: &str = "BUNGIE_API_KEY";
const CLIENT_ID: &str = "BUNGIE_PUBLIC_CLIENT_ID";
const CLIENT_SECRET: &str = "BUNGIE_CLIENT_SECRET";
const TOKEN_BROKER_URL: &str = "GHOSTLINK_TOKEN_BROKER_URL";
const CALLBACK_URI: &str = "BUNGIE_CALLBACK_URI";
const REGISTERED_CALLBACK_URI: &str = "BUNGIE_REGISTERED_CALLBACK_URI";

/// The exact custom-scheme components of the registered product callback.
///
/// The provider rejects an HTTP loopback redirect for this registered public
/// client, so the product callback is a custom-scheme URI. The interactive flow
/// renders it into the authorization URL and compares a pasted callback against
/// it, so only these exact components are accepted.
const PRODUCT_CALLBACK_SCHEME: &str = "ghostlink";
const PRODUCT_CALLBACK_HOST: &str = "oauth";
const PRODUCT_CALLBACK_PATH: &str = "/callback";
const READ_DATABASE_PATH: &str = "DESTINY_DATABASE_PATH";
const READ_FRESHNESS_TTL_SECONDS: &str = "DESTINY_FRESHNESS_TTL_SECONDS";

/// The largest freshness TTL the shared read runtime configuration accepts (30 days).
///
/// Freshness remains caller policy; this is only a defensive ceiling against a
/// nonsensical configured value, not a product TTL.
pub const MAX_READ_FRESHNESS_TTL_SECONDS: u64 = 30 * 24 * 60 * 60;

/// A validated HTTP loopback callback URI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallbackUri {
    exact: String,
    scheme: String,
    host: String,
    port: u16,
    path: String,
}

#[allow(dead_code)]
impl CallbackUri {
    pub fn parse(value: &str) -> Result<Self, AppError> {
        if value.is_empty() || value.trim() != value {
            return Err(AppError::configuration());
        }

        let url = Url::parse(value).map_err(|_| AppError::configuration())?;
        if url.scheme() != "http"
            || url.username() != ""
            || url.password().is_some()
            || url.host_str().is_none()
            || url.port().is_none()
            || url.fragment().is_some()
            || url.query().is_some()
        {
            return Err(AppError::configuration());
        }

        let host = url.host_str().ok_or_else(AppError::configuration)?;
        match is_loopback(host) {
            true => {}
            false => return Err(AppError::configuration()),
        }

        let path = url.path();
        if path.is_empty()
            || !path.starts_with('/')
            || path.contains('\0')
            || (path == "/" && !value.ends_with('/'))
        {
            return Err(AppError::configuration());
        }

        Ok(Self {
            exact: value.to_owned(),
            scheme: url.scheme().to_owned(),
            host: host.to_owned(),
            port: url.port().ok_or_else(AppError::configuration)?,
            path: path.to_owned(),
        })
    }

    pub fn exact(&self) -> &str {
        &self.exact
    }

    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

fn is_loopback(host: &str) -> bool {
    host == "localhost" || host == "127.0.0.1" || host == "::1"
}

/// Required public-client configuration. No client secret is represented.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct Config {
    api_key: String,
    public_client_id: String,
    callback_uri: CallbackUri,
    registered_callback_uri: CallbackUri,
}

#[allow(dead_code)]
impl Config {
    /// Loads exact keys with environment > per-user file > compiled defaults
    /// precedence. Present values win, even when empty or invalid.
    pub fn from_env() -> Result<Self, AppError> {
        let values =
            exact_env_values(&[API_KEY, CLIENT_ID, CALLBACK_URI, REGISTERED_CALLBACK_URI])?;
        Self::from_values(&values)
    }

    pub fn from_values(values: &HashMap<String, String>) -> Result<Self, AppError> {
        let api_key = required(values, API_KEY)?;
        let public_client_id = required(values, CLIENT_ID)?;
        let callback = CallbackUri::parse(&required(values, CALLBACK_URI)?)?;
        let registered = CallbackUri::parse(&required(values, REGISTERED_CALLBACK_URI)?)?;

        if callback.exact() != registered.exact() {
            return Err(AppError::configuration());
        }

        Ok(Self {
            api_key,
            public_client_id,
            callback_uri: callback,
            registered_callback_uri: registered,
        })
    }

    pub fn callback_uri(&self) -> &CallbackUri {
        &self.callback_uri
    }

    pub fn registered_callback_uri(&self) -> &CallbackUri {
        &self.registered_callback_uri
    }
}

/// A validated product custom-scheme callback URI.
///
/// The registered provider redirect for this product is a custom-scheme URI
/// (`ghostlink://oauth/callback`) rather than an HTTP loopback address, because
/// the portal rejects a loopback redirect for this registered public client.
/// The value is a fixed, non-secret configuration string: the interactive flow
/// renders it into the authorization URL and compares a pasted callback against
/// it, so the exact scheme, authority, and path are validated and no other
/// scheme, host, path, port, query, or fragment is accepted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoginCallbackUri {
    exact: String,
}

impl LoginCallbackUri {
    /// Parses and validates one product callback URI.
    pub fn parse(value: &str) -> Result<Self, AppError> {
        if value.is_empty() || value.trim() != value {
            return Err(AppError::configuration());
        }
        let url = Url::parse(value).map_err(|_| AppError::configuration())?;
        if url.scheme() != PRODUCT_CALLBACK_SCHEME
            || url.username() != ""
            || url.password().is_some()
            || url.host_str() != Some(PRODUCT_CALLBACK_HOST)
            || url.port().is_some()
            || url.path() != PRODUCT_CALLBACK_PATH
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(AppError::configuration());
        }
        Ok(Self {
            exact: value.to_owned(),
        })
    }

    /// The exact validated callback string.
    pub fn exact(&self) -> &str {
        &self.exact
    }
}

/// Validated configuration for the interactive login composition.
///
/// It carries the client identifier, registered custom-scheme callback URI,
/// and the selected client mode. A secret selects direct confidential exchange;
/// otherwise a broker URL selects renewable broker exchange, else public mode.
/// Login never loads the API key needed by the read runtime.
///
/// Every field is private and reachable only through a trusted accessor.
/// [`fmt::Debug`] renders no value, and every parse failure is a sanitized
/// [`AppError`] that never echoes the supplied configuration.
#[derive(Clone, Eq, PartialEq)]
pub struct LoginRuntimeConfig {
    public_client_id: String,
    callback_uri: LoginCallbackUri,
    auth_mode: ClientAuthMode,
}

impl LoginRuntimeConfig {
    /// Loads login configuration from the environment, file, and compiled defaults.
    ///
    /// Precedence per key is process environment, file, then compiled defaults.
    /// Only the exact required keys are retained, apart from the environment
    /// variables used to resolve the file path. Invalid Unicode and malformed
    /// or insecure files fail closed with a sanitized configuration error.
    pub fn from_env() -> Result<Self, AppError> {
        let values = exact_env_values(&[
            CLIENT_ID,
            CALLBACK_URI,
            REGISTERED_CALLBACK_URI,
            CLIENT_SECRET,
            TOKEN_BROKER_URL,
        ])?;
        Self::from_values(&values)
    }

    /// Validates the login configuration from an explicit value map.
    ///
    /// The configured callback and the provider-registered callback must be the
    /// exact same string, so a redirect the provider has not registered is
    /// refused before anything is opened or read.
    pub fn from_values(values: &HashMap<String, String>) -> Result<Self, AppError> {
        let public_client_id = required(values, CLIENT_ID)?;
        let callback = LoginCallbackUri::parse(&required(values, CALLBACK_URI)?)?;
        let registered = LoginCallbackUri::parse(&required(values, REGISTERED_CALLBACK_URI)?)?;
        if callback.exact() != registered.exact() {
            return Err(AppError::configuration());
        }
        Ok(Self {
            public_client_id,
            callback_uri: callback,
            auth_mode: client_auth_mode(values)?,
        })
    }

    /// The trusted public-client identifier accessor.
    ///
    /// The value is public by contract, but it is still a provider-issued
    /// identifier used in an authorization URL, so it stays behind an accessor
    /// and out of every diagnostic surface.
    pub fn public_client_id(&self) -> &str {
        &self.public_client_id
    }

    /// The confidential-client secret, or `None` for public mode.
    pub fn client_secret(&self) -> Option<&ClientSecret> {
        self.auth_mode.client_secret()
    }

    /// The explicitly selected public, confidential, or broker mode.
    pub fn auth_mode(&self) -> &ClientAuthMode {
        &self.auth_mode
    }

    /// The exact validated custom-scheme callback URI to send to the provider.
    pub fn callback_uri(&self) -> &LoginCallbackUri {
        &self.callback_uri
    }
}

impl fmt::Debug for LoginRuntimeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoginRuntimeConfig")
            .field("public_client_id", &destiny_core::redact::REDACTED)
            .field("callback_uri", &destiny_core::redact::REDACTED)
            .field("client_secret", &destiny_core::redact::REDACTED)
            .finish()
    }
}

fn optional_client_secret(
    values: &HashMap<String, String>,
) -> Result<Option<ClientSecret>, AppError> {
    values
        .get(CLIENT_SECRET)
        .map(|value| ClientSecret::from_bytes(value.as_bytes().to_vec()))
        .transpose()
}

/// Validated OAuth client mode. Values never appear in diagnostics.
#[derive(Clone, Eq, PartialEq)]
pub enum ClientAuthMode {
    Public,
    Confidential(ClientSecret),
    /// The validated HTTPS broker base URL (not the token path).
    Broker(String),
}

impl ClientAuthMode {
    pub fn client_secret(&self) -> Option<&ClientSecret> {
        match self {
            Self::Confidential(secret) => Some(secret),
            _ => None,
        }
    }
}

impl fmt::Debug for ClientAuthMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        destiny_core::redact::opaque(formatter, "ClientAuthMode")
    }
}

fn client_auth_mode(values: &HashMap<String, String>) -> Result<ClientAuthMode, AppError> {
    // Validate every supplied key, even when a secret takes precedence.
    let broker = values
        .get(TOKEN_BROKER_URL)
        .map(|value| {
            if value.is_empty()
                || value.len() > 2048
                || value.trim() != value
                || value.chars().any(char::is_control)
            {
                return Err(AppError::configuration());
            }
            let url = Url::parse(value).map_err(|_| AppError::configuration())?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || !has_plain_https_authority(value)
            {
                return Err(AppError::configuration());
            }
            Ok(value.clone())
        })
        .transpose()?;
    Ok(match (optional_client_secret(values)?, broker) {
        (Some(secret), _) => ClientAuthMode::Confidential(secret),
        (None, Some(broker)) => ClientAuthMode::Broker(broker),
        (None, None) => ClientAuthMode::Public,
    })
}

/// Checks the raw text, because the URL parser silently drops empty userinfo
/// and extra authority slashes (`https:////@host`).
fn has_plain_https_authority(value: &str) -> bool {
    value.strip_prefix("https://").is_some_and(|rest| {
        let authority = rest.split('/').next().unwrap_or_default();
        !authority.is_empty() && !authority.contains('@')
    })
}

/// Loads only the additional client identifier needed by renewable reads.
fn read_env_values<F>(mut load: F) -> Result<HashMap<String, String>, AppError>
where
    F: FnMut(&[&str]) -> Result<HashMap<String, String>, AppError>,
{
    let mut values = load(&[
        API_KEY,
        READ_DATABASE_PATH,
        READ_FRESHNESS_TTL_SECONDS,
        CLIENT_SECRET,
        TOKEN_BROKER_URL,
    ])?;
    if values.contains_key(CLIENT_SECRET) || values.contains_key(TOKEN_BROKER_URL) {
        values.extend(load(&[CLIENT_ID])?);
    }
    Ok(values)
}

fn required(values: &HashMap<String, String>, name: &str) -> Result<String, AppError> {
    match values.get(name).filter(|value| !value.is_empty()) {
        Some(value) => Ok(value.clone()),
        None => Err(AppError::configuration()),
    }
}

/// Reads named configuration keys with process environment precedence per key.
///
/// The bounded per-user file, then compiled defaults, supply absent keys. Only
/// path-selection variables and requested keys are inspected; invalid Unicode
/// fails sanitized. Explicit empty or invalid overrides never fall back.
fn exact_env_values(names: &[&str]) -> Result<HashMap<String, String>, AppError> {
    exact_source_values(names, |name| std::env::var_os(name))
}

/// Composes all sources with the official defaults and an injected environment.
/// The parsed map remains private and is never rendered in diagnostics.
fn exact_source_values<F>(
    names: &[&str],
    mut lookup: F,
) -> Result<HashMap<String, String>, AppError>
where
    F: FnMut(&str) -> Option<OsString>,
{
    exact_source_values_with_defaults(names, &OFFICIAL, &mut lookup)
}

fn exact_source_values_with_defaults<F>(
    names: &[&str],
    defaults: &CompiledDefaults,
    mut lookup: F,
) -> Result<HashMap<String, String>, AppError>
where
    F: FnMut(&str) -> Option<OsString>,
{
    let values = file::load(file::resolve_path(&mut lookup))?;
    exact_key_values(names, |name| {
        lookup(name)
            .or_else(|| values.get(name).map(OsString::from))
            .or_else(|| {
                if name == READ_DATABASE_PATH {
                    return paths::database_path(paths::Platform::current(), &mut lookup)
                        .map(PathBuf::into_os_string);
                }
                let value = match name {
                    API_KEY => defaults.api_key,
                    CLIENT_ID => defaults.public_client_id,
                    TOKEN_BROKER_URL => defaults.token_broker_url,
                    CALLBACK_URI => defaults.callback_uri,
                    REGISTERED_CALLBACK_URI => defaults.registered_callback_uri,
                    READ_FRESHNESS_TTL_SECONDS => defaults.freshness_ttl_seconds,
                    _ => None,
                };
                value.map(OsString::from)
            })
    })
}

/// The testable core of [`exact_env_values`] over an injected key lookup.
///
/// Absent keys are omitted so [`required`] reports them; a present key with
/// invalid Unicode fails closed with the sanitized configuration category and
/// is never rendered.
fn exact_key_values<F>(names: &[&str], mut lookup: F) -> Result<HashMap<String, String>, AppError>
where
    F: FnMut(&str) -> Option<OsString>,
{
    let mut values = HashMap::with_capacity(names.len());
    for name in names {
        if let Some(value) = lookup(name) {
            let value = value.into_string().map_err(|_| AppError::configuration())?;
            values.insert((*name).to_owned(), value);
        }
    }
    Ok(values)
}

/// A validated, value-redacted Bungie API key for the shared CLI/MCP read
/// composition roots.
///
/// The value is accepted only when it is non-empty visible ASCII, mirroring the
/// adapter-level key contract without importing it. Bytes are reachable only
/// through [`Self::expose_secret`] and are never rendered by [`fmt::Debug`].
#[derive(Clone, Eq, PartialEq)]
pub struct ReadApiKey(destiny_core::redact::Redacted<String>);

impl ReadApiKey {
    /// Validates a candidate API key.
    ///
    /// Empty, whitespace-bearing, control-bearing, or non-ASCII values are
    /// rejected as sanitized configuration failures; the rejected value never
    /// appears in the error.
    pub fn new(value: impl Into<String>) -> Result<Self, AppError> {
        let value = value.into();
        if !value.is_empty() && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
            return Ok(Self(destiny_core::redact::Redacted::new(value)));
        }
        Err(AppError::configuration())
    }

    /// The trusted accessor for the composition root. The value is never
    /// rendered by any diagnostic surface.
    pub fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for ReadApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        destiny_core::redact::opaque(formatter, "ReadApiKey")
    }
}

/// Validated renewable refresh client; never renders its values.
#[derive(Clone, Eq, PartialEq)]
pub struct RefreshClientConfig {
    client_id: String,
    auth_mode: ClientAuthMode,
}

impl RefreshClientConfig {
    /// The trusted client identifier accessor.
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// The trusted confidential-client secret accessor, absent in broker mode.
    pub fn client_secret(&self) -> Option<&ClientSecret> {
        self.auth_mode.client_secret()
    }

    /// The selected renewable mode (confidential or broker).
    pub fn auth_mode(&self) -> &ClientAuthMode {
        &self.auth_mode
    }
}

impl fmt::Debug for RefreshClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        destiny_core::redact::opaque(formatter, "RefreshClientConfig")
    }
}

/// Application-owned runtime configuration for the shared CLI/MCP read
/// composition roots.
///
/// It carries exactly the typed values those roots need to build the read
/// services — a validated API key, the SQLite database path,
/// a positive, bounded freshness TTL, and an optional
/// confidential refresh client. Every field is private and reachable only through a trusted accessor.
/// [`fmt::Debug`] and [`fmt::Display`] render no value, and every parse failure
/// is a sanitized [`AppError`] that never echoes the supplied configuration.
#[derive(Clone, Eq, PartialEq)]
pub struct ReadRuntimeConfig {
    api_key: ReadApiKey,
    database_path: PathBuf,
    freshness_ttl_seconds: u64,
    refresh_client: Option<RefreshClientConfig>,
}

impl ReadRuntimeConfig {
    /// Loads read configuration from the environment, file, and compiled defaults.
    ///
    /// Precedence per key is process environment, file, then compiled defaults.
    /// Only the exact required keys are retained, apart from the environment
    /// variables used to resolve the file path. Invalid Unicode and malformed
    /// or insecure files fail closed with a sanitized configuration error.
    pub fn from_env() -> Result<Self, AppError> {
        let values = read_env_values(exact_env_values)?;
        Self::from_values(&values)
    }

    /// Validates the shared read runtime configuration from an explicit value map.
    /// Source loading supplies OS defaults; this validator never consults the
    /// host environment, so absent paths in explicit maps still fail closed.
    pub fn from_values(values: &HashMap<String, String>) -> Result<Self, AppError> {
        let api_key = ReadApiKey::new(required(values, API_KEY)?)?;
        let database_path = read_database_path(&required(values, READ_DATABASE_PATH)?)?;
        let freshness_ttl_seconds =
            read_freshness_ttl(&required(values, READ_FRESHNESS_TTL_SECONDS)?)?;
        let auth_mode = client_auth_mode(values)?;
        let refresh_client = match auth_mode {
            ClientAuthMode::Public => None,
            auth_mode => Some(RefreshClientConfig {
                client_id: required(values, CLIENT_ID)?,
                auth_mode,
            }),
        };
        Ok(Self {
            api_key,
            database_path,
            freshness_ttl_seconds,
            refresh_client,
        })
    }

    /// The renewable refresh client, or `None` for public reads.
    /// A client identifier is required for confidential and broker modes.
    pub fn refresh_client(&self) -> Option<&RefreshClientConfig> {
        self.refresh_client.as_ref()
    }

    /// The trusted API-key accessor; the value is never rendered.
    pub fn api_key(&self) -> &ReadApiKey {
        &self.api_key
    }

    /// The trusted database-path accessor.
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// Resolve from committed Manifest metadata, never from profile metadata.
    /// An absent catalog is distinct from a corrupt/unreadable store.
    pub fn resolve_definition_version<R: destiny_core::DefinitionRepository>(
        &self,
        repository: &R,
    ) -> Result<DefinitionVersion, AppError> {
        repository
            .current_metadata()?
            .map(|metadata| metadata.definition_version)
            .ok_or_else(|| AppError::new(destiny_core::ErrorCategory::DefinitionsUnavailable))
    }

    /// The validated positive, bounded freshness TTL in seconds.
    pub const fn freshness_ttl_seconds(&self) -> u64 {
        self.freshness_ttl_seconds
    }

    /// The validated TTL as the shared freshness policy the read services take.
    pub fn freshness_policy(&self) -> FreshnessPolicy {
        FreshnessPolicy {
            ttl_seconds: self.freshness_ttl_seconds,
        }
    }
}

impl fmt::Debug for ReadRuntimeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReadRuntimeConfig")
            .field("api_key", &destiny_core::redact::REDACTED)
            .field("database_path", &destiny_core::redact::REDACTED)
            .field("freshness_ttl_seconds", &destiny_core::redact::REDACTED)
            .field("refresh_client", &destiny_core::redact::REDACTED)
            .finish()
    }
}

impl fmt::Display for ReadRuntimeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "ReadRuntimeConfig({})",
            destiny_core::redact::REDACTED
        )
    }
}

/// Requires a non-empty path with no interior NUL byte and no surrounding
/// whitespace.
fn read_database_path(value: &str) -> Result<PathBuf, AppError> {
    if value.is_empty() || value.trim() != value || value.as_bytes().contains(&0) {
        return Err(AppError::configuration());
    }
    Ok(PathBuf::from(value))
}

/// Requires a strictly positive decimal TTL within the accepted ceiling.
fn read_freshness_ttl(value: &str) -> Result<u64, AppError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AppError::configuration());
    }
    let ttl = value
        .parse::<u64>()
        .map_err(|_| AppError::configuration())?;
    if ttl == 0 || ttl > MAX_READ_FRESHNESS_TTL_SECONDS {
        return Err(AppError::configuration());
    }
    Ok(ttl)
}

#[cfg(test)]
mod tests {
    use super::{
        CallbackUri, Config, LoginRuntimeConfig, MAX_READ_FRESHNESS_TTL_SECONDS, ReadApiKey,
        ReadRuntimeConfig, exact_key_values,
    };
    use destiny_core::ErrorCategory;
    use std::collections::HashMap;
    use std::ffi::OsString;

    pub(super) fn injected_defaults() -> super::CompiledDefaults {
        super::CompiledDefaults {
            api_key: Some("compiled-key"),
            public_client_id: Some("compiled-client"),
            token_broker_url: Some("https://compiled.test"),
            callback_uri: Some("ghostlink://oauth/callback"),
            registered_callback_uri: Some("ghostlink://oauth/callback"),
            freshness_ttl_seconds: Some("321"),
        }
    }

    #[test]
    fn read_sources_supply_database_default_or_sanitized_missing_configuration() {
        let defaults = injected_defaults();
        let load = |resolved| {
            super::exact_source_values_with_defaults(
                &[
                    super::API_KEY,
                    super::READ_DATABASE_PATH,
                    super::READ_FRESHNESS_TTL_SECONDS,
                ],
                &defaults,
                |name| match name {
                    "DESTINY_DEFINITION_VERSION" => Some("manifest-test".into()),
                    "HOME" if resolved => Some("/synthetic-home".into()),
                    "LOCALAPPDATA" if resolved => Some(r"C:\synthetic-home".into()),
                    _ => None,
                },
            )
            .unwrap()
        };
        let read = ReadRuntimeConfig::from_values(&load(true)).unwrap();
        assert!(read.database_path().ends_with("ghostlink.db"));
        let error = ReadRuntimeConfig::from_values(&load(false)).unwrap_err();
        assert_eq!(error.category(), ErrorCategory::ConfigurationError);
        assert_eq!(
            error.to_string(),
            "required runtime configuration is missing or invalid"
        );
    }

    #[test]
    fn compiled_defaults_supply_absent_keys_and_enable_both_runtimes() {
        let defaults = injected_defaults();
        let load = |names: &[&str]| {
            super::exact_source_values_with_defaults(names, &defaults, |name| match name {
                "DESTINY_DATABASE_PATH" => Some("synthetic.db".into()),
                "DESTINY_DEFINITION_VERSION" => Some("manifest-test".into()),
                _ => None,
            })
        };
        let read = ReadRuntimeConfig::from_values(&super::read_env_values(load).unwrap()).unwrap();
        assert_eq!(read.api_key().expose_secret(), "compiled-key");
        assert_eq!(read.freshness_ttl_seconds(), 321);
        assert_eq!(
            read.refresh_client().unwrap().client_id(),
            "compiled-client"
        );
        let login = LoginRuntimeConfig::from_values(
            &load(&[
                super::CLIENT_ID,
                super::CALLBACK_URI,
                super::REGISTERED_CALLBACK_URI,
                super::TOKEN_BROKER_URL,
            ])
            .unwrap(),
        )
        .unwrap();
        assert_eq!(login.public_client_id(), "compiled-client");
        assert!(
            matches!(login.auth_mode(), super::ClientAuthMode::Broker(url) if url == "https://compiled.test")
        );
    }

    #[test]
    fn absent_optional_defaults_keep_sanitized_missing_key_errors() {
        let defaults = super::CompiledDefaults {
            api_key: None,
            public_client_id: None,
            token_broker_url: None,
            ..injected_defaults()
        };
        let values = super::exact_source_values_with_defaults(
            &[
                super::API_KEY,
                super::CLIENT_ID,
                super::CALLBACK_URI,
                super::REGISTERED_CALLBACK_URI,
                super::CLIENT_SECRET,
                super::TOKEN_BROKER_URL,
                super::READ_DATABASE_PATH,
            ],
            &defaults,
            |_| None,
        )
        .unwrap();
        for key in [
            super::API_KEY,
            super::CLIENT_ID,
            super::CLIENT_SECRET,
            super::TOKEN_BROKER_URL,
            super::READ_DATABASE_PATH,
        ] {
            assert!(!values.contains_key(key));
        }
        for error in [
            LoginRuntimeConfig::from_values(&values).unwrap_err(),
            ReadRuntimeConfig::from_values(&values).unwrap_err(),
        ] {
            assert_eq!(error.category(), ErrorCategory::ConfigurationError);
            assert_eq!(
                error.to_string(),
                "required runtime configuration is missing or invalid"
            );
        }
    }

    #[test]
    fn secret_beats_compiled_broker_in_both_runtimes() {
        let defaults = injected_defaults();
        let load = |names: &[&str]| {
            super::exact_source_values_with_defaults(names, &defaults, |name| match name {
                "BUNGIE_CLIENT_SECRET" => Some("synthetic-secret".into()),
                "DESTINY_DATABASE_PATH" => Some("synthetic.db".into()),
                "DESTINY_DEFINITION_VERSION" => Some("manifest-test".into()),
                _ => None,
            })
        };
        let login = LoginRuntimeConfig::from_values(
            &load(&[
                super::CLIENT_ID,
                super::CALLBACK_URI,
                super::REGISTERED_CALLBACK_URI,
                super::CLIENT_SECRET,
                super::TOKEN_BROKER_URL,
            ])
            .unwrap(),
        )
        .unwrap();
        let read = ReadRuntimeConfig::from_values(&super::read_env_values(load).unwrap()).unwrap();
        for mode in [
            login.auth_mode(),
            read.refresh_client().unwrap().auth_mode(),
        ] {
            assert!(matches!(mode, super::ClientAuthMode::Confidential(_)));
        }
    }

    #[test]
    fn compiled_defaults_are_validated_and_empty_env_does_not_fall_back() {
        for (key, invalid) in [
            (super::API_KEY, "invalid key"),
            (super::CLIENT_ID, ""),
            (super::CALLBACK_URI, "ghostlink://wrong/callback"),
            (super::REGISTERED_CALLBACK_URI, "ghostlink://oauth/other"),
            (super::TOKEN_BROKER_URL, "http://invalid.test"),
            (super::READ_FRESHNESS_TTL_SECONDS, "0"),
        ] {
            let mut defaults = injected_defaults();
            match key {
                super::API_KEY => defaults.api_key = Some(invalid),
                super::CLIENT_ID => defaults.public_client_id = Some(invalid),
                super::CALLBACK_URI => defaults.callback_uri = Some(invalid),
                super::REGISTERED_CALLBACK_URI => defaults.registered_callback_uri = Some(invalid),
                super::TOKEN_BROKER_URL => defaults.token_broker_url = Some(invalid),
                _ => defaults.freshness_ttl_seconds = Some(invalid),
            }
            let values = super::exact_source_values_with_defaults(
                &[
                    super::API_KEY,
                    super::CLIENT_ID,
                    super::CALLBACK_URI,
                    super::REGISTERED_CALLBACK_URI,
                    super::TOKEN_BROKER_URL,
                    super::READ_DATABASE_PATH,
                    super::READ_FRESHNESS_TTL_SECONDS,
                ],
                &defaults,
                |name| match name {
                    "DESTINY_DATABASE_PATH" => Some("synthetic.db".into()),
                    "DESTINY_DEFINITION_VERSION" => Some("manifest-test".into()),
                    _ => None,
                },
            )
            .unwrap();
            let error = if matches!(
                key,
                super::CALLBACK_URI | super::REGISTERED_CALLBACK_URI | super::CLIENT_ID
            ) {
                LoginRuntimeConfig::from_values(&values).unwrap_err()
            } else {
                ReadRuntimeConfig::from_values(&values).unwrap_err()
            };
            assert_eq!(error.category(), ErrorCategory::ConfigurationError);
            assert_eq!(
                error.to_string(),
                "required runtime configuration is missing or invalid"
            );
        }
        let values = super::exact_source_values_with_defaults(
            &[super::API_KEY],
            &injected_defaults(),
            |name| (name == super::API_KEY).then(|| OsString::from("")),
        )
        .unwrap();
        assert_eq!(values[super::API_KEY], "");
    }

    fn values() -> HashMap<String, String> {
        HashMap::from([
            ("BUNGIE_API_KEY".into(), "api-key-placeholder".into()),
            (
                "BUNGIE_PUBLIC_CLIENT_ID".into(),
                "client-id-placeholder".into(),
            ),
            (
                "BUNGIE_CALLBACK_URI".into(),
                "http://127.0.0.1:43127/oauth/callback".into(),
            ),
            (
                "BUNGIE_REGISTERED_CALLBACK_URI".into(),
                "http://127.0.0.1:43127/oauth/callback".into(),
            ),
        ])
    }

    #[test]
    fn parses_callback_components() {
        let callback = CallbackUri::parse("http://localhost:43127/oauth/callback").unwrap();
        assert_eq!(callback.scheme(), "http");
        assert_eq!(callback.host(), "localhost");
        assert_eq!(callback.port(), 43127);
        assert_eq!(callback.path(), "/oauth/callback");
        assert_eq!(callback.exact(), "http://localhost:43127/oauth/callback");
    }

    #[test]
    fn requires_all_public_configuration_values() {
        for key in [
            "BUNGIE_API_KEY",
            "BUNGIE_PUBLIC_CLIENT_ID",
            "BUNGIE_CALLBACK_URI",
            "BUNGIE_REGISTERED_CALLBACK_URI",
        ] {
            let mut missing = values();
            missing.remove(key);
            assert!(
                Config::from_values(&missing).is_err(),
                "{key} should be required"
            );
        }
    }

    #[test]
    fn requires_exact_case_sensitive_callback_equality() {
        let mut mismatch = values();
        mismatch.insert(
            "BUNGIE_REGISTERED_CALLBACK_URI".into(),
            "http://127.0.0.1:43127/OAuth/callback".into(),
        );
        assert!(Config::from_values(&mismatch).is_err());
    }

    #[test]
    fn rejects_invalid_callback_shapes() {
        for value in [
            "https://127.0.0.1:43127/callback",
            "http://example.test:43127/callback",
            "http://127.0.0.1/callback",
            "http://127.0.0.1:43127/callback#fragment",
            "http://user:pass@127.0.0.1:43127/callback",
            "http://127.0.0.1:43127/callback?code=secret",
            "http://127.0.0.1:43127",
        ] {
            assert!(
                CallbackUri::parse(value).is_err(),
                "{value} should be rejected"
            );
        }
    }

    #[test]
    fn configuration_errors_do_not_echo_values() {
        let mut invalid = values();
        invalid.insert("BUNGIE_API_KEY".into(), "super-secret-api-key".into());
        invalid.insert(
            "BUNGIE_CALLBACK_URI".into(),
            "http://example.test:43127/?code=secret".into(),
        );
        let error = match Config::from_values(&invalid) {
            Ok(_) => panic!("invalid callback should fail"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(!message.contains("super-secret-api-key"));
        assert!(!message.contains("secret"));
        assert!(!message.contains("example.test"));
    }

    fn login_values() -> HashMap<String, String> {
        HashMap::from([
            (
                "BUNGIE_PUBLIC_CLIENT_ID".into(),
                "client-id-placeholder".into(),
            ),
            (
                "BUNGIE_CALLBACK_URI".into(),
                "ghostlink://oauth/callback".into(),
            ),
            (
                "BUNGIE_REGISTERED_CALLBACK_URI".into(),
                "ghostlink://oauth/callback".into(),
            ),
        ])
    }

    #[test]
    fn login_config_reads_the_exact_product_callback_and_requires_every_value() {
        let config = LoginRuntimeConfig::from_values(&login_values()).expect("valid login config");
        assert_eq!(config.public_client_id(), "client-id-placeholder");
        assert_eq!(config.callback_uri().exact(), "ghostlink://oauth/callback");

        for key in [
            "BUNGIE_PUBLIC_CLIENT_ID",
            "BUNGIE_CALLBACK_URI",
            "BUNGIE_REGISTERED_CALLBACK_URI",
        ] {
            let mut missing = login_values();
            missing.remove(key);
            assert!(
                LoginRuntimeConfig::from_values(&missing).is_err(),
                "{key} should be required"
            );
            let mut empty = login_values();
            empty.insert(key.into(), String::new());
            assert!(
                LoginRuntimeConfig::from_values(&empty).is_err(),
                "{key} should reject an empty value"
            );
        }

        // Login never loads an API key: the read-only credential is not part of
        // this configuration, so a machine that cannot supply one still
        // configures an interactive authorization.
        assert!(!login_values().contains_key("BUNGIE_API_KEY"));
    }

    #[test]
    fn login_config_rejects_an_unregistered_or_non_product_callback() {
        let mut unregistered = login_values();
        unregistered.insert(
            "BUNGIE_REGISTERED_CALLBACK_URI".into(),
            "ghostlink://oauth/other".into(),
        );
        assert!(LoginRuntimeConfig::from_values(&unregistered).is_err());

        // Only the exact registered custom scheme, authority, and path are
        // accepted; an HTTP loopback redirect the provider rejects, a different
        // scheme or host, a port, a query, or a fragment is refused, so login
        // cannot be pointed off the registered callback.
        for value in [
            "http://127.0.0.1:43127/oauth/callback",
            "https://oauth/callback",
            "ghostlink://evil.test/callback",
            "ghostlink://oauth:1337/callback",
            "ghostlink://oauth/callback?code=secret",
            "ghostlink://oauth/callback#fragment",
        ] {
            let mut invalid = login_values();
            invalid.insert("BUNGIE_CALLBACK_URI".into(), value.into());
            invalid.insert("BUNGIE_REGISTERED_CALLBACK_URI".into(), value.into());
            assert!(
                LoginRuntimeConfig::from_values(&invalid).is_err(),
                "{value} should be rejected"
            );
        }
    }

    #[test]
    fn login_config_debug_and_errors_are_value_redacted() {
        let mut secret = login_values();
        secret.insert(
            "BUNGIE_PUBLIC_CLIENT_ID".into(),
            "client-id-sentinel".into(),
        );
        let config = LoginRuntimeConfig::from_values(&secret).expect("valid login config");
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("client-id-sentinel"), "{rendered}");
        assert!(!rendered.contains("ghostlink"), "{rendered}");

        // A rejected callback carrying a sentinel is never echoed by its error.
        let mut invalid = login_values();
        invalid.insert(
            "BUNGIE_CALLBACK_URI".into(),
            "ghostlink://oauth/callback-sentinel".into(),
        );
        let error = LoginRuntimeConfig::from_values(&invalid).expect_err("mismatched callback");
        assert_eq!(
            error.to_string(),
            "required runtime configuration is missing or invalid"
        );
        assert!(!format!("{error} {error:?}").contains("callback-sentinel"));
    }

    fn read_runtime_values() -> HashMap<String, String> {
        HashMap::from([
            ("BUNGIE_API_KEY".into(), "api-key-placeholder".into()),
            ("DESTINY_DATABASE_PATH".into(), "/tmp/destiny.db".into()),
            ("DESTINY_DEFINITION_VERSION".into(), "manifest-v1".into()),
            ("DESTINY_FRESHNESS_TTL_SECONDS".into(), "900".into()),
        ])
    }

    #[test]
    fn auth_mode_precedence_and_broker_lookup() {
        for secret in [false, true] {
            let mut login = login_values();
            login.insert(
                "GHOSTLINK_TOKEN_BROKER_URL".into(),
                "https://broker.test".into(),
            );
            let mut read = read_runtime_values();
            read.insert("BUNGIE_PUBLIC_CLIENT_ID".into(), "123".into());
            read.insert(
                "GHOSTLINK_TOKEN_BROKER_URL".into(),
                "https://broker.test".into(),
            );
            if secret {
                login.insert("BUNGIE_CLIENT_SECRET".into(), "secret-sentinel".into());
                read.insert("BUNGIE_CLIENT_SECRET".into(), "secret-sentinel".into());
            }
            let login = LoginRuntimeConfig::from_values(&login).unwrap();
            let loaded = super::read_env_values(|names| {
                exact_key_values(names, |name| read.get(name).map(OsString::from))
            })
            .unwrap();
            let read = ReadRuntimeConfig::from_values(&loaded).unwrap();
            for mode in [
                login.auth_mode(),
                read.refresh_client().unwrap().auth_mode(),
            ] {
                assert!(if secret {
                    matches!(mode, super::ClientAuthMode::Confidential(_))
                } else {
                    matches!(mode, super::ClientAuthMode::Broker(_))
                });
                assert!(!format!("{mode:?}").contains("broker.test"));
            }
        }
        assert!(matches!(
            LoginRuntimeConfig::from_values(&login_values())
                .unwrap()
                .auth_mode(),
            super::ClientAuthMode::Public
        ));
        let values =
            super::exact_source_values(&["GHOSTLINK_TOKEN_BROKER_URL"], |name| match name {
                "GHOSTLINK_CONFIG_FILE" => {
                    Some(OsString::from("/nonexistent/ghostlink-test-config.env"))
                }
                "GHOSTLINK_TOKEN_BROKER_URL" => Some(OsString::from("https://env.test")),
                _ => None,
            })
            .unwrap();
        assert_eq!(values["GHOSTLINK_TOKEN_BROKER_URL"], "https://env.test");
    }

    #[test]
    fn broker_config_enables_refresh_and_rejects_unsafe_urls() {
        let mut read = read_runtime_values();
        read.insert("BUNGIE_PUBLIC_CLIENT_ID".into(), "123".into());
        read.insert(
            "GHOSTLINK_TOKEN_BROKER_URL".into(),
            "https://broker.test/".into(),
        );
        assert!(
            ReadRuntimeConfig::from_values(&read)
                .unwrap()
                .refresh_client()
                .is_some()
        );
        for invalid in [
            "http://broker.test",
            "//broker.test",
            "https://user@broker.test",
            "https://@broker.test",
            "https:////@broker.test",
            "https:///broker.test",
            "HTTPS://@broker.test",
            "https://:pass@broker.test",
            "https://broker.test?query",
            "https://broker.test#fragment",
            "https://broker.test\n",
            "",
            &format!("https://broker.test/{}", "x".repeat(2048)),
        ] {
            read.insert("GHOSTLINK_TOKEN_BROKER_URL".into(), invalid.into());
            let mut login = login_values();
            login.insert("GHOSTLINK_TOKEN_BROKER_URL".into(), invalid.into());
            for error in [
                ReadRuntimeConfig::from_values(&read).unwrap_err(),
                LoginRuntimeConfig::from_values(&login).unwrap_err(),
            ] {
                assert_eq!(error.category(), ErrorCategory::ConfigurationError);
                assert_eq!(
                    error.to_string(),
                    "required runtime configuration is missing or invalid"
                );
            }
        }
    }

    #[test]
    fn optional_secret_preserves_public_login_and_reads() {
        assert!(
            LoginRuntimeConfig::from_values(&login_values())
                .unwrap()
                .client_secret()
                .is_none()
        );
        assert!(
            ReadRuntimeConfig::from_values(&read_runtime_values())
                .unwrap()
                .refresh_client()
                .is_none()
        );
    }

    #[test]
    fn confidential_configs_expose_secret_but_never_render_it() {
        let sentinel = "confidential-secret-sentinel";
        let mut login = login_values();
        login.insert("BUNGIE_CLIENT_SECRET".into(), sentinel.into());
        let login = LoginRuntimeConfig::from_values(&login).unwrap();
        assert_eq!(
            login.client_secret().unwrap().as_bytes(),
            sentinel.as_bytes()
        );
        let mut read = read_runtime_values();
        read.insert("BUNGIE_CLIENT_SECRET".into(), sentinel.into());
        read.insert(
            "BUNGIE_PUBLIC_CLIENT_ID".into(),
            "client-id-placeholder".into(),
        );
        let read = ReadRuntimeConfig::from_values(&read).unwrap();
        let client = read.refresh_client().unwrap();
        assert_eq!(client.client_id(), "client-id-placeholder");
        assert_eq!(
            client.client_secret().unwrap().as_bytes(),
            sentinel.as_bytes()
        );
        let rendered = format!(
            "{login:?} {read:?} {read} {client:?} {:?}",
            login.client_secret()
        );
        assert!(!rendered.contains(sentinel));
        assert!(!rendered.contains("client-id-placeholder"));
    }

    #[test]
    fn confidential_reads_require_a_nonempty_client_id() {
        for id in [None, Some("")] {
            let mut read = read_runtime_values();
            read.insert("BUNGIE_CLIENT_SECRET".into(), "secret-sentinel".into());
            if let Some(id) = id {
                read.insert("BUNGIE_PUBLIC_CLIENT_ID".into(), id.into());
            }
            let error = ReadRuntimeConfig::from_values(&read).unwrap_err();
            assert_eq!(error.category(), ErrorCategory::ConfigurationError);
            assert!(!format!("{error} {error:?}").contains("secret-sentinel"));
        }
    }

    #[test]
    fn invalid_optional_secrets_fail_sanitized_for_both_runtimes() {
        for secret in [
            String::new(),
            "invalid secret-sentinel".into(),
            "secret-sentinel\n".into(),
            "secret-sentinelé".into(),
            "x".repeat(4097),
        ] {
            let mut login = login_values();
            login.insert("BUNGIE_CLIENT_SECRET".into(), secret.clone());
            let mut read = read_runtime_values();
            read.insert(
                "BUNGIE_PUBLIC_CLIENT_ID".into(),
                "client-id-placeholder".into(),
            );
            read.insert("BUNGIE_CLIENT_SECRET".into(), secret);
            for error in [
                LoginRuntimeConfig::from_values(&login).unwrap_err(),
                ReadRuntimeConfig::from_values(&read).unwrap_err(),
            ] {
                assert_eq!(error.category(), ErrorCategory::ConfigurationError);
                assert!(!format!("{error} {error:?}").contains("secret-sentinel"));
            }
        }
    }

    #[test]
    fn read_lookup_requests_client_id_only_when_secret_is_present() {
        for confidential in [false, true] {
            let mut source = read_runtime_values();
            if confidential {
                source.insert("BUNGIE_CLIENT_SECRET".into(), "secret-sentinel".into());
                source.insert(
                    "BUNGIE_PUBLIC_CLIENT_ID".into(),
                    "client-id-placeholder".into(),
                );
            }
            let values = super::read_env_values(|names| {
                exact_key_values(names, |name| {
                    assert!(name != "BUNGIE_PUBLIC_CLIENT_ID" || confidential);
                    assert!(matches!(
                        name,
                        "BUNGIE_API_KEY"
                            | "DESTINY_DATABASE_PATH"
                            | "DESTINY_DEFINITION_VERSION"
                            | "DESTINY_FRESHNESS_TTL_SECONDS"
                            | "BUNGIE_CLIENT_SECRET"
                            | "GHOSTLINK_TOKEN_BROKER_URL"
                            | "BUNGIE_PUBLIC_CLIENT_ID"
                    ));
                    source.get(name).map(OsString::from)
                })
            })
            .unwrap();
            assert_eq!(
                ReadRuntimeConfig::from_values(&values)
                    .unwrap()
                    .refresh_client()
                    .is_some(),
                confidential
            );
        }
    }

    #[test]
    fn definition_version_resolution_ignores_legacy_override_and_types_absence() {
        use crate::test_support::InMemoryDefinitionRepository;
        let repository = InMemoryDefinitionRepository::new(
            super::DefinitionVersion::new("real-v1"),
            vec![destiny_core::ItemDefinition {
                does_postmaster_pull_have_side_effects: None,
                is_instance_item: None,
                max_stack_size: None,
                stack_unique_label: None,
                hash: destiny_core::ItemHash::new(1),
                bucket_hash: None,
                name: "item".into(),
                item_type: None,
                tier: None,
                element: None,
                slot: None,
                class_type: None,
                plug_category: None,
                tags: vec![],
                version: super::DefinitionVersion::new("real-v1"),
            }],
        );
        let mut values = read_runtime_values();
        values.insert("DESTINY_DEFINITION_VERSION".into(), "live-v1".into());
        let explicit = ReadRuntimeConfig::from_values(&values).unwrap();
        assert_eq!(
            explicit
                .resolve_definition_version(&repository)
                .unwrap()
                .as_str(),
            "real-v1"
        );
        values.remove("DESTINY_DEFINITION_VERSION");
        let automatic = ReadRuntimeConfig::from_values(&values).unwrap();
        assert_eq!(
            automatic
                .resolve_definition_version(&repository)
                .unwrap()
                .as_str(),
            "real-v1"
        );
        let empty = InMemoryDefinitionRepository::default();
        assert_eq!(
            automatic
                .resolve_definition_version(&empty)
                .unwrap_err()
                .category(),
            destiny_core::ErrorCategory::DefinitionsUnavailable
        );
        assert_eq!(
            explicit
                .resolve_definition_version(&empty)
                .unwrap_err()
                .category(),
            destiny_core::ErrorCategory::DefinitionsUnavailable
        );
        let broken = empty.with_metadata_failure(destiny_core::AppError::new(
            destiny_core::ErrorCategory::LocalStorage,
        ));
        for config in [&automatic, &explicit] {
            assert_eq!(
                config
                    .resolve_definition_version(&broken)
                    .unwrap_err()
                    .category(),
                destiny_core::ErrorCategory::LocalStorage
            );
        }
    }

    #[test]
    fn read_runtime_config_ignores_absent_or_empty_legacy_version() {
        let mut values = read_runtime_values();
        values.remove("DESTINY_DEFINITION_VERSION");
        assert!(ReadRuntimeConfig::from_values(&values).is_ok());
        values.insert("DESTINY_DEFINITION_VERSION".into(), String::new());
        assert!(ReadRuntimeConfig::from_values(&values).is_ok());
    }

    #[test]
    fn read_runtime_config_parses_every_typed_field() {
        let config = ReadRuntimeConfig::from_values(&read_runtime_values()).unwrap();
        assert_eq!(config.api_key().expose_secret(), "api-key-placeholder");
        assert_eq!(config.database_path().to_string_lossy(), "/tmp/destiny.db");
        assert_eq!(config.freshness_ttl_seconds(), 900);
        assert_eq!(config.freshness_policy().ttl_seconds, 900);
    }

    #[test]
    fn read_runtime_config_requires_every_resolved_value() {
        for key in [
            "BUNGIE_API_KEY",
            "DESTINY_DATABASE_PATH",
            "DESTINY_FRESHNESS_TTL_SECONDS",
        ] {
            let mut missing = read_runtime_values();
            missing.remove(key);
            assert!(
                ReadRuntimeConfig::from_values(&missing).is_err(),
                "{key} should be required"
            );
            let mut empty = read_runtime_values();
            empty.insert(key.into(), String::new());
            assert!(
                ReadRuntimeConfig::from_values(&empty).is_err(),
                "{key} should reject an empty value"
            );
        }
    }

    #[test]
    fn read_runtime_config_rejects_invalid_values() {
        let cases: [(&str, Vec<String>); 3] = [
            (
                "BUNGIE_API_KEY",
                vec!["with space".into(), "with\ttab".into(), "caf\u{e9}".into()],
            ),
            (
                "DESTINY_DATABASE_PATH",
                vec!["/tmp/with\0nul".into(), " \t ".into()],
            ),
            (
                "DESTINY_FRESHNESS_TTL_SECONDS",
                vec![
                    "0".into(),
                    "-1".into(),
                    "1.5".into(),
                    "nope".into(),
                    "+5".into(),
                    "18446744073709551616".into(),
                ],
            ),
        ];
        for (key, bad_values) in cases {
            for bad in bad_values {
                let mut invalid = read_runtime_values();
                invalid.insert(key.into(), bad.clone());
                assert!(
                    ReadRuntimeConfig::from_values(&invalid).is_err(),
                    "{key} should reject {bad:?}"
                );
            }
        }

        // The TTL ceiling is inclusive.
        let mut boundary = read_runtime_values();
        boundary.insert(
            "DESTINY_FRESHNESS_TTL_SECONDS".into(),
            MAX_READ_FRESHNESS_TTL_SECONDS.to_string(),
        );
        assert_eq!(
            ReadRuntimeConfig::from_values(&boundary)
                .unwrap()
                .freshness_ttl_seconds(),
            MAX_READ_FRESHNESS_TTL_SECONDS
        );
    }

    #[test]
    fn read_runtime_config_debug_and_display_are_value_redacted() {
        let mut secret = read_runtime_values();
        secret.insert("BUNGIE_API_KEY".into(), "api-key-sentinel".into());
        secret.insert(
            "DESTINY_DATABASE_PATH".into(),
            "/home/sentinel/destiny.db".into(),
        );
        secret.insert(
            "DESTINY_DEFINITION_VERSION".into(),
            "version-sentinel".into(),
        );
        secret.insert("DESTINY_FRESHNESS_TTL_SECONDS".into(), "1234".into());
        let config = ReadRuntimeConfig::from_values(&secret).unwrap();

        let rendered = format!("{config} {config:?} {:?}", config.api_key());
        for leaked in [
            "api-key-sentinel",
            "/home/sentinel/destiny.db",
            "version-sentinel",
            "1234",
        ] {
            assert!(
                !rendered.contains(leaked),
                "a formatted config leaked {leaked}: {rendered}"
            );
        }
        assert_eq!(format!("{}", config), "ReadRuntimeConfig(<redacted>)");
        assert_eq!(format!("{:?}", config.api_key()), "ReadApiKey");
    }

    #[test]
    fn read_runtime_config_errors_do_not_echo_supplied_values() {
        let mut invalid = read_runtime_values();
        invalid.insert(
            "DESTINY_DATABASE_PATH".into(),
            "/home/sentinel/destiny.db".into(),
        );
        invalid.insert(
            "DESTINY_FRESHNESS_TTL_SECONDS".into(),
            "not-a-number".into(),
        );
        let error = ReadRuntimeConfig::from_values(&invalid).unwrap_err();
        assert_eq!(
            error.to_string(),
            "required runtime configuration is missing or invalid"
        );
        assert!(!error.to_string().contains("sentinel"));
        assert!(!format!("{error:?}").contains("sentinel"));

        // The API-key wrapper itself is redacted on its own.
        assert_eq!(
            format!("{:?}", ReadApiKey::new("key-sentinel").unwrap()),
            "ReadApiKey"
        );
        assert!(ReadApiKey::new("").is_err());
        assert!(ReadApiKey::new("with space").is_err());
    }

    #[test]
    fn exact_key_values_reads_only_requested_keys_and_omits_absent_ones() {
        let names: Vec<&str> = vec!["FIRST_KEY", "SECOND_KEY", "ABSENT_KEY"];
        let values = exact_key_values(names.as_slice(), |name| {
            match name {
                "FIRST_KEY" => Some(OsString::from("first")),
                "SECOND_KEY" => Some(OsString::from("second")),
                "ABSENT_KEY" => None,
                // Any other name, including an unrelated process variable, is a
                // contract violation: the loader must never read it.
                other => panic!("unexpected key read: {other}"),
            }
        })
        .unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values.get("FIRST_KEY").map(String::as_str), Some("first"));
        assert_eq!(values.get("SECOND_KEY").map(String::as_str), Some("second"));
        assert!(!values.contains_key("ABSENT_KEY"));
    }

    #[test]
    fn exact_key_values_rejects_invalid_unicode_sanitized() {
        for invalid in invalid_unicode_os_strings() {
            let error = exact_key_values(&["BROKEN_KEY"], |_| Some(invalid.clone()))
                .expect_err("invalid Unicode fails closed");
            assert_eq!(error.category(), ErrorCategory::ConfigurationError);
            assert_eq!(
                error.to_string(),
                "required runtime configuration is missing or invalid"
            );
        }
    }

    /// Builds environment values that are not valid Unicode on the host platform.
    #[cfg(unix)]
    fn invalid_unicode_os_strings() -> Vec<OsString> {
        use std::os::unix::ffi::OsStringExt;
        vec![OsString::from_vec(vec![0xff, 0xfe])]
    }

    /// Windows `OsString` is always valid Unicode, so there is nothing to build.
    #[cfg(not(unix))]
    fn invalid_unicode_os_strings() -> Vec<OsString> {
        Vec::new()
    }

    #[test]
    fn legacy_definition_version_is_ignored_even_when_invalid() {
        for bad in [
            " manifest-v1",
            "manifest-v1 ",
            "manifest v1",
            "manifest\tv1",
            "manifest\nv1",
            "manifest\rv1",
            "manifest\u{0}v1",
            "manifest\u{7f}v1",
            "manifest\u{a0}v1",
        ] {
            let mut invalid = read_runtime_values();
            invalid.insert("DESTINY_DEFINITION_VERSION".into(), bad.into());
            assert!(ReadRuntimeConfig::from_values(&invalid).is_ok());
        }

        for good in ["manifest-v1", "2026-09-21", &"v".repeat(129)] {
            let mut valid = read_runtime_values();
            valid.insert("DESTINY_DEFINITION_VERSION".into(), good.into());
            assert!(ReadRuntimeConfig::from_values(&valid).is_ok());
        }
    }
}
