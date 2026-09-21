//! Provider integrations and local OAuth adapters.

pub mod oauth;
pub mod token;

pub use oauth::{
    CommandLauncher, DesktopBrowserAuthorization, HeadlessAuthorizationInstruction,
    HeadlessBrowserAuthorization, LoopbackCallbackReceiver, SshTarget, SystemCommandLauncher,
    authorization_url, browser_command,
};
pub use token::{
    BungieTokenExchange, HttpTokenTransport, TOKEN_ENDPOINT, TokenHttpResponse, TokenRequest,
    TokenTransport, TokenTransportError,
};
