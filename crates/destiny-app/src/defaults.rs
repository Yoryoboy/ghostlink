//! Compiled configuration for the official Ghostlink application.

/// Official PUBLIC application values: safe to commit and extractable from
/// release binaries. They identify the official Bungie application and broker.
/// A client secret must NEVER go here or anywhere else in the repository.
/// Every value is overridable per key by config.env and then process environment.
pub(crate) const BUNGIE_API_KEY: Option<&'static str> = Some("8a6b807cfaf342f58bb2c45c1ed33f78");
pub(crate) const BUNGIE_PUBLIC_CLIENT_ID: Option<&'static str> = Some("55099");
pub(crate) const GHOSTLINK_TOKEN_BROKER_URL: Option<&'static str> =
    Some("https://ghostlink-token-broker.93jads.workers.dev");
pub(crate) const BUNGIE_CALLBACK_URI: Option<&'static str> =
    Some("https://ghostlink-token-broker.93jads.workers.dev/callback");
pub(crate) const BUNGIE_REGISTERED_CALLBACK_URI: Option<&'static str> =
    Some("https://ghostlink-token-broker.93jads.workers.dev/callback");
/// Freshness TTL in seconds, parsed through the same validator as overrides.
pub(crate) const DESTINY_FRESHNESS_TTL_SECONDS: Option<&'static str> = Some("900");

/// Injectable fallback values. None leaves a key absent; even compiled values
/// must pass the runtime validators. Database paths and client secrets are
/// deliberately not represented. Definition provenance is not defaulted.
#[derive(Clone, Copy)]
pub(crate) struct CompiledDefaults {
    pub api_key: Option<&'static str>,
    pub public_client_id: Option<&'static str>,
    pub token_broker_url: Option<&'static str>,
    pub callback_uri: Option<&'static str>,
    pub registered_callback_uri: Option<&'static str>,
    pub freshness_ttl_seconds: Option<&'static str>,
}

pub(crate) const OFFICIAL: CompiledDefaults = CompiledDefaults {
    api_key: BUNGIE_API_KEY,
    public_client_id: BUNGIE_PUBLIC_CLIENT_ID,
    token_broker_url: GHOSTLINK_TOKEN_BROKER_URL,
    callback_uri: BUNGIE_CALLBACK_URI,
    registered_callback_uri: BUNGIE_REGISTERED_CALLBACK_URI,
    freshness_ttl_seconds: DESTINY_FRESHNESS_TTL_SECONDS,
};
