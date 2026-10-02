//! Provider integrations and local OAuth adapters.

pub mod clock;
pub mod credential_store;
pub mod manifest;
pub mod oauth;
pub mod profile;
pub mod profile_gateway;
mod public_milestones;
pub mod random;
pub mod sqlite;
pub mod token;
pub mod transfer;

pub use clock::SystemClock;
pub use credential_store::{CREDENTIAL_SERVICE, CREDENTIAL_USER, NativeCredentialStore};
pub use oauth::{
    CommandLauncher, DesktopBrowserAuthorization, HeadlessAuthorizationInstruction,
    HeadlessBrowserAuthorization, LoopbackCallbackReceiver, SshTarget, SystemCommandLauncher,
    authorization_url, browser_command,
};
pub use profile::decode_profile;
pub use profile_gateway::{
    BungieProfileGateway, HttpProfileTransport, MEMBERSHIP_ENDPOINT, MEMBERSHIP_RESPONSE_LIMIT,
    PROFILE_ORIGIN, PROFILE_RESPONSE_LIMIT, ProfileApiKey, ProfileBearerToken, ProfileHttpRequest,
    ProfileHttpResponse, ProfileTransport, ProfileTransportError, decode_effective_membership,
    fetch_bounded, is_allowed_profile_url, profile_endpoint,
};
pub use random::SystemSecureRandom;
pub use token::{
    BungieTokenExchange, HttpTokenTransport, TOKEN_ENDPOINT, TokenHttpResponse, TokenRequest,
    TokenTransport, TokenTransportError,
};
pub use transfer::{
    BungieTransferGateway, HttpTransferTransport, TRANSFER_ENDPOINT, TRANSFER_RESPONSE_LIMIT,
    TransferApiKey, TransferBearerToken, TransferHttpRequest, TransferHttpResponse,
    TransferTransport, TransferTransportError, is_allowed_transfer_url,
};
