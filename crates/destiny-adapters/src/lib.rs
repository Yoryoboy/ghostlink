//! Provider integrations and local OAuth adapters.

pub mod agents;
pub mod artifact;
pub mod clarity;
pub mod clock;
pub mod credential_store;
pub mod equip;
pub mod instanced_pull;
pub mod loadout;
pub mod lock;
pub mod manifest;
pub mod oauth;
pub mod profile;
pub mod profile_gateway;
mod public_milestones;
pub mod random;
pub mod sqlite;
pub mod token;
pub mod transfer;
pub mod uri_handler;
pub mod weekly_profile;

pub use clock::SystemClock;
pub use credential_store::{CREDENTIAL_SERVICE, CREDENTIAL_USER, NativeCredentialStore};
pub use equip::{
    BungieEquipGateway, EQUIP_ENDPOINT, EQUIP_RESPONSE_LIMIT, EquipApiKey, EquipBearerToken,
    EquipHttpRequest, EquipHttpResponse, EquipTransport, EquipTransportError, HttpEquipTransport,
    is_allowed_equip_url,
};
pub use instanced_pull::{
    BungieInstancedPullGateway, HttpInstancedPullTransport, INSTANCED_PULL_ENDPOINT,
    INSTANCED_PULL_RESPONSE_LIMIT, InstancedPullApiKey, InstancedPullBearerToken,
    InstancedPullHttpRequest, InstancedPullHttpResponse, InstancedPullTransport,
    InstancedPullTransportError, is_allowed_instanced_pull_url,
};
pub use lock::{
    BungieLockGateway, HttpLockTransport, LOCK_ENDPOINT, LOCK_RESPONSE_LIMIT, LockApiKey,
    LockBearerToken, LockHttpRequest, LockHttpResponse, LockTransport, LockTransportError,
    is_allowed_lock_url,
};
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
pub use public_milestones::format_reset_timestamp;
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
