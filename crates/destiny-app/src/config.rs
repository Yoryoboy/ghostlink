//! Public-client runtime configuration and callback validation.

use std::collections::HashMap;

use destiny_core::AppError;
use url::Url;

const API_KEY: &str = "BUNGIE_API_KEY";
const CLIENT_ID: &str = "BUNGIE_PUBLIC_CLIENT_ID";
const CALLBACK_URI: &str = "BUNGIE_CALLBACK_URI";
const REGISTERED_CALLBACK_URI: &str = "BUNGIE_REGISTERED_CALLBACK_URI";

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
    pub fn from_env() -> Result<Self, AppError> {
        let values = std::env::vars().collect();
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

fn required(values: &HashMap<String, String>, name: &str) -> Result<String, AppError> {
    match values.get(name).filter(|value| !value.is_empty()) {
        Some(value) => Ok(value.clone()),
        None => Err(AppError::configuration()),
    }
}

#[cfg(test)]
mod tests {
    use super::{CallbackUri, Config};
    use std::collections::HashMap;

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
}
