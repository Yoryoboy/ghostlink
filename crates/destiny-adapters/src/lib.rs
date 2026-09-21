//! Provider integrations and local OAuth adapters.

pub mod oauth;

pub use oauth::{
    CommandLauncher, DesktopBrowserAuthorization, HeadlessAuthorizationInstruction,
    HeadlessBrowserAuthorization, LoopbackCallbackReceiver, SshTarget, SystemCommandLauncher,
    authorization_url, browser_command,
};
