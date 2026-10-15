//! Provider integrations and local OAuth adapters.

pub mod agents;
pub mod artifact;
pub mod bungie_throttle;
pub mod calendar;
pub mod clarity;
pub mod clock;
pub mod dim_loadout;
pub mod equip;
pub mod equip_items;
pub mod file_credential_store;
pub use equip_items::{
    BungieEquipItemsGateway, EQUIP_ITEMS_ENDPOINT, EQUIP_ITEMS_RESPONSE_LIMIT, EquipItemsApiKey,
    EquipItemsBearerToken, EquipItemsHttpRequest, EquipItemsHttpResponse, EquipItemsTransport,
    EquipItemsTransportError, HttpEquipItemsTransport, is_allowed_equip_items_url,
};
pub mod equipment_capacity_policy;
pub mod insert_plug;
pub use insert_plug::{
    BungieInsertPlugGateway, HttpInsertPlugTransport, INSERT_PLUG_ENDPOINT,
    INSERT_PLUG_MIN_INTERVAL, INSERT_PLUG_RESPONSE_LIMIT, InsertPlugApiKey, InsertPlugBearerToken,
    InsertPlugHttpRequest, InsertPlugHttpResponse, InsertPlugTransport, InsertPlugTransportError,
    is_allowed_insert_plug_url,
};
pub mod instanced_pull;
pub mod stack_pull;
pub use stack_pull::{
    BungieStackPullGateway, HttpStackPullTransport, STACK_PULL_ENDPOINT, STACK_PULL_RESPONSE_LIMIT,
    StackPullApiKey, StackPullBearerToken, StackPullHttpRequest, StackPullHttpResponse,
    StackPullTransport, StackPullTransportError, is_allowed_stack_pull_url,
};
pub mod loadout;
pub mod loadout_action;
pub use loadout_action::{
    BungieLoadoutActionGateway, HttpLoadoutActionTransport, LOADOUT_ACTION_ENDPOINT,
    LOADOUT_ACTION_RESPONSE_LIMIT, LoadoutActionApiKey, LoadoutActionBearerToken,
    LoadoutActionHttpRequest, LoadoutActionHttpResponse, LoadoutActionTransport,
    LoadoutActionTransportError, is_allowed_loadout_action_url,
};
pub mod lock;
pub mod manifest;
pub mod oauth;
pub mod pending_login;
pub mod profile;
pub mod profile_gateway;
mod public_milestones;
pub mod random;
pub mod sqlite;
pub mod stack_bucket_capacity_policy;
pub mod wishlist;
pub use stack_bucket_capacity_policy::{
    PINNED_STACK_BUCKET_CAPACITY, PINNED_STACK_BUCKET_CAPACITY_VERSION, PINNED_STACK_BUCKET_PATH,
    StackBucketCapacityQualificationError, pinned_stack_bucket_capacity_policy,
    qualify_pinned_stack_bucket_capacity,
};
mod bungie_errors;
pub mod token;
pub mod transfer;
pub mod uri_handler;
pub mod weekly_profile;

pub use clock::SystemClock;
pub use equip::{
    BungieEquipGateway, EQUIP_ENDPOINT, EQUIP_RESPONSE_LIMIT, EquipApiKey, EquipBearerToken,
    EquipHttpRequest, EquipHttpResponse, EquipTransport, EquipTransportError, HttpEquipTransport,
    is_allowed_equip_url,
};
pub use equipment_capacity_policy::{
    EquipmentCapacityQualificationError, PINNED_EQUIPMENT_BUCKET_PATH, PINNED_EQUIPMENT_CAPACITY,
    PINNED_EQUIPMENT_CAPACITY_VERSION, decode_equipment_capacity_evidence,
    pinned_equipment_capacity_policy, qualify_pinned_equipment_capacity,
};
pub use file_credential_store::FileCredentialStore;
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
pub use pending_login::{PendingLogin, PendingLoginStore};
pub use profile::{decode_profile, decode_profile_with_observations};
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
