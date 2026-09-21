//! Provider integrations and local OAuth adapters.

pub mod credential_store;
pub mod manifest;
pub mod oauth;
pub mod token;

pub use credential_store::{CREDENTIAL_SERVICE, CREDENTIAL_USER, NativeCredentialStore};
pub use oauth::{
    CommandLauncher, DesktopBrowserAuthorization, HeadlessAuthorizationInstruction,
    HeadlessBrowserAuthorization, LoopbackCallbackReceiver, SshTarget, SystemCommandLauncher,
    authorization_url, browser_command,
};
pub use token::{
    BungieTokenExchange, HttpTokenTransport, TOKEN_ENDPOINT, TokenHttpResponse, TokenRequest,
    TokenTransport, TokenTransportError,
};
