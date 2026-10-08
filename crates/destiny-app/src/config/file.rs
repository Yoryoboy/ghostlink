//! Bounded, value-free configuration file loading. No map is rendered in diagnostics.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::PathBuf;

use destiny_core::AppError;

const MAX_FILE_BYTES: usize = 64 * 1024;

/// Resolves an explicit override, absolute XDG directory, or HOME fallback.
/// Path values stay native OS strings and are never included in errors.
pub(super) fn resolve_path<F>(lookup: F) -> Option<PathBuf>
where
    F: FnMut(&str) -> Option<OsString>,
{
    resolve_path_for(super::paths::Platform::current(), lookup)
}

fn resolve_path_for<F>(platform: super::paths::Platform, mut lookup: F) -> Option<PathBuf>
where
    F: FnMut(&str) -> Option<OsString>,
{
    if let Some(path) = lookup("GHOSTLINK_CONFIG_FILE").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path));
    }
    if matches!(platform, super::paths::Platform::Windows) {
        return super::paths::windows_path(lookup("APPDATA"), r"ghostlink\config.env");
    }
    if let Some(path) = lookup("XDG_CONFIG_HOME").filter(super::paths::unix_absolute) {
        return Some(PathBuf::from(path).join("ghostlink/config.env"));
    }
    lookup("HOME").map(|home| PathBuf::from(home).join(".config/ghostlink/config.env"))
}

/// Reads at most one byte beyond the limit, checking permissions on the opened
/// file rather than a separate path lookup. Only absence is silently ignored.
pub(super) fn load(path: Option<PathBuf>) -> Result<HashMap<String, String>, AppError> {
    let Some(path) = path else {
        return Ok(HashMap::new());
    };
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(_) => return Err(AppError::configuration()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = file.metadata().map_err(|_| AppError::configuration())?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(AppError::configuration());
        }
    }
    let mut bytes = Vec::new();
    file.take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::configuration())?;
    parse(&bytes)
}

/// Parses the entire bounded file fail-closed, without echoing keys or values.
/// Quotes are literal delimiters; no expansion or shell evaluation occurs.
fn parse(bytes: &[u8]) -> Result<HashMap<String, String>, AppError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(AppError::configuration());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| AppError::configuration())?;
    let mut values = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (key, value) = line.split_once('=').ok_or_else(AppError::configuration)?;
        let key = key.trim();
        let mut chars = key.bytes();
        if !chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            || !chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return Err(AppError::configuration());
        }
        let value = value.trim();
        let value = if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        values.insert(key.to_owned(), value.to_owned());
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::{load, parse};
    use crate::config::{LoginRuntimeConfig, ReadRuntimeConfig, exact_source_values};
    use destiny_core::{AppError, ErrorCategory};
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    // Synthetic fixtures only; never consult the actual user configuration.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new(bytes: &[u8]) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "ghostlink-config-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).unwrap();
            let path = dir.join("config.env");
            std::fs::write(&path, bytes).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
            Self(path)
        }

        fn env(&self, name: &str) -> Option<OsString> {
            (name == "GHOSTLINK_CONFIG_FILE").then(|| self.0.clone().into_os_string())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_file(&self.0).unwrap();
            std::fs::remove_dir(self.0.parent().unwrap()).unwrap();
        }
    }

    fn sanitized(error: AppError) {
        assert_eq!(error.category(), ErrorCategory::ConfigurationError);
        assert_eq!(
            error.to_string(),
            "required runtime configuration is missing or invalid"
        );
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains("sentinel"));
        assert!(!rendered.contains("config.env"));
    }

    #[test]
    fn legacy_definition_version_in_file_and_environment_is_ignored() {
        use crate::test_support::InMemoryDefinitionRepository;
        use destiny_core::DefinitionVersion;

        let fixture = Fixture::new(b"BUNGIE_API_KEY=file-key\nDESTINY_DATABASE_PATH=synthetic.db\nDESTINY_FRESHNESS_TTL_SECONDS=900\nDESTINY_DEFINITION_VERSION=live-v1\n");
        let file = load(Some(fixture.0.clone())).unwrap();
        let repository = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("stored-manifest-v1"),
            vec![destiny_core::ItemDefinition {
                hash: destiny_core::ItemHash::new(1),
                name: "item".into(),
                bucket_hash: None,
                item_type: None,
                tier: None,
                element: None,
                slot: None,
                class_type: None,
                plug_category: None,
                tags: vec![],
                version: DefinitionVersion::new("stored-manifest-v1"),
                does_postmaster_pull_have_side_effects: None,
                is_instance_item: None,
                max_stack_size: None,
                stack_unique_label: None,
            }],
        );
        let config = ReadRuntimeConfig::from_values(&file).unwrap();
        assert_eq!(
            config
                .resolve_definition_version(&repository)
                .unwrap()
                .as_str(),
            "stored-manifest-v1"
        );

        let values = crate::config::read_env_values(|names| {
            assert!(!names.contains(&"DESTINY_DEFINITION_VERSION"));
            crate::config::exact_key_values(names, |name| file.get(name).map(OsString::from))
        })
        .unwrap();
        assert!(!values.contains_key("DESTINY_DEFINITION_VERSION"));
        let config = ReadRuntimeConfig::from_values(&values).unwrap();
        assert_eq!(
            config
                .resolve_definition_version(&repository)
                .unwrap()
                .as_str(),
            "stored-manifest-v1"
        );
    }

    #[test]
    fn windows_config_uses_appdata_and_explicit_override() {
        use crate::config::paths::Platform;
        let resolve = |explicit: Option<&str>, appdata: &str| {
            super::resolve_path_for(Platform::Windows, |name| match name {
                "GHOSTLINK_CONFIG_FILE" => explicit.map(OsString::from),
                "APPDATA" => Some(appdata.into()),
                _ => panic!("unexpected lookup"),
            })
        };
        assert_eq!(
            resolve(None, r"C:\Users\test\AppData\Roaming"),
            Some(PathBuf::from(
                r"C:\Users\test\AppData\Roaming\ghostlink\config.env"
            ))
        );
        assert_eq!(
            resolve(Some("override.env"), "relative"),
            Some("override.env".into())
        );
        for invalid in ["", "relative", r"C:relative", r"\rooted"] {
            assert!(resolve(None, invalid).is_none());
        }
    }

    #[test]
    fn database_file_and_env_override_os_default() {
        let fixture = Fixture::new(b"DESTINY_DATABASE_PATH=file.db\n");
        for override_value in [None, Some("env.db"), Some("")] {
            let values = exact_source_values(&["DESTINY_DATABASE_PATH"], |name| match name {
                "DESTINY_DATABASE_PATH" => override_value.map(OsString::from),
                "HOME" => Some("/synthetic-home".into()),
                "LOCALAPPDATA" => Some(r"C:\synthetic-home".into()),
                _ => fixture.env(name),
            })
            .unwrap();
            assert_eq!(
                values["DESTINY_DATABASE_PATH"],
                override_value.unwrap_or("file.db")
            );
        }
    }

    #[test]
    fn parsing_supports_comments_export_quotes_and_whitespace() {
        let parsed = parse(b"\n  # ignored\n export FIRST = \"first value\" \nSECOND='second'\n_THIRD9 = plain=tail\nEMPTY=\n").unwrap();
        assert_eq!(parsed.get("FIRST").map(String::as_str), Some("first value"));
        assert_eq!(parsed.get("SECOND").map(String::as_str), Some("second"));
        assert_eq!(
            parsed.get("_THIRD9").map(String::as_str),
            Some("plain=tail")
        );
        assert_eq!(parsed.get("EMPTY").map(String::as_str), Some(""));
    }

    #[test]
    fn malformed_and_non_utf8_input_fails_closed_without_contents() {
        for input in [
            &b"line-sentinel"[..],
            &b"=value-sentinel"[..],
            &b"9KEY=value-sentinel"[..],
            &b"BAD KEY=value-sentinel"[..],
            &b"BAD-KEY=value-sentinel"[..],
            &b"KEY=\xff"[..],
        ] {
            sanitized(parse(input).unwrap_err());
        }
    }

    #[test]
    fn size_limit_is_inclusive_and_bounded_on_disk() {
        let mut bytes = b"KEY=".to_vec();
        bytes.resize(64 * 1024, b'x');
        assert!(parse(&bytes).is_ok());
        let fixture = Fixture::new(&bytes);
        assert!(load(Some(fixture.0.clone())).is_ok());
        bytes.push(b'x');
        sanitized(parse(&bytes).unwrap_err());
        std::fs::write(&fixture.0, &bytes).unwrap();
        sanitized(load(Some(fixture.0.clone())).unwrap_err());
    }

    #[test]
    fn path_resolution_obeys_precedence_and_ignores_relative_xdg() {
        let resolve = |explicit: &str, xdg: &str, home: Option<&str>| {
            super::resolve_path_for(crate::config::paths::Platform::Unix, |name| match name {
                "GHOSTLINK_CONFIG_FILE" => Some(explicit.into()),
                "XDG_CONFIG_HOME" => Some(xdg.into()),
                "HOME" => home.map(OsString::from),
                _ => panic!("unexpected lookup"),
            })
        };
        assert_eq!(
            resolve("override.env", "/xdg", Some("/home")),
            Some("override.env".into())
        );
        assert_eq!(
            resolve("", "/xdg", Some("/home")),
            Some(PathBuf::from("/xdg/ghostlink/config.env"))
        );
        for xdg in ["", "relative"] {
            assert_eq!(
                resolve("", xdg, Some("/home")),
                Some(PathBuf::from("/home/.config/ghostlink/config.env"))
            );
        }
        assert_eq!(resolve("", "relative", None), None);
        assert!(super::resolve_path(|_| None).is_none());
    }

    #[test]
    fn missing_file_and_no_path_are_empty_but_other_read_failures_are_sanitized() {
        let fixture = Fixture::new(b"KEY=value");
        assert!(
            load(Some(fixture.0.with_file_name("absent.env")))
                .unwrap()
                .is_empty()
        );
        assert!(load(None).unwrap().is_empty());
        sanitized(load(Some(fixture.0.parent().unwrap().to_path_buf())).unwrap_err());
    }

    #[test]
    fn composed_lookup_feeds_both_runtimes_and_env_wins_per_key() {
        let fixture = Fixture::new(b"BUNGIE_API_KEY=file-key\nDESTINY_DATABASE_PATH=/tmp/synthetic.db\nDESTINY_DEFINITION_VERSION=v1\nDESTINY_FRESHNESS_TTL_SECONDS=900\nBUNGIE_PUBLIC_CLIENT_ID=file-client\nBUNGIE_CALLBACK_URI=ghostlink://oauth/callback\nBUNGIE_REGISTERED_CALLBACK_URI=ghostlink://oauth/callback\nUNKNOWN=ignored\n");
        let names = [
            "BUNGIE_API_KEY",
            "DESTINY_DATABASE_PATH",
            "DESTINY_DEFINITION_VERSION",
            "DESTINY_FRESHNESS_TTL_SECONDS",
        ];
        let values = exact_source_values(&names, |name| {
            if name == "BUNGIE_API_KEY" {
                Some("env-key".into())
            } else {
                fixture.env(name)
            }
        })
        .unwrap();
        assert_eq!(values.len(), names.len());
        let read = ReadRuntimeConfig::from_values(&values).unwrap();
        assert_eq!(read.api_key().expose_secret(), "env-key");
        assert_eq!(read.freshness_ttl_seconds(), 900);
        let login = exact_source_values(
            &[
                "BUNGIE_PUBLIC_CLIENT_ID",
                "BUNGIE_CALLBACK_URI",
                "BUNGIE_REGISTERED_CALLBACK_URI",
            ],
            |name| fixture.env(name),
        )
        .unwrap();
        assert_eq!(
            LoginRuntimeConfig::from_values(&login)
                .unwrap()
                .public_client_id(),
            "file-client"
        );
        let empty = exact_source_values(&["BUNGIE_API_KEY"], |name| {
            if name == "BUNGIE_API_KEY" {
                Some("".into())
            } else {
                fixture.env(name)
            }
        })
        .unwrap();
        assert_eq!(empty["BUNGIE_API_KEY"], "");
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            sanitized(
                exact_source_values(&["BUNGIE_API_KEY"], |name| {
                    if name == "BUNGIE_API_KEY" {
                        Some(OsString::from_vec(vec![0xff]))
                    } else {
                        fixture.env(name)
                    }
                })
                .unwrap_err(),
            );
        }
    }

    #[test]
    fn file_and_env_override_each_compiled_default() {
        let defaults = super::super::tests::injected_defaults();
        let names = [
            "BUNGIE_API_KEY",
            "BUNGIE_PUBLIC_CLIENT_ID",
            "BUNGIE_CALLBACK_URI",
            "BUNGIE_REGISTERED_CALLBACK_URI",
            "GHOSTLINK_TOKEN_BROKER_URL",
            "DESTINY_FRESHNESS_TTL_SECONDS",
        ];
        let fixture = Fixture::new(b"BUNGIE_API_KEY=file-key\nBUNGIE_PUBLIC_CLIENT_ID=file-client\nBUNGIE_CALLBACK_URI=file-callback\nBUNGIE_REGISTERED_CALLBACK_URI=file-registered\nGHOSTLINK_TOKEN_BROKER_URL=https://file.test\nDESTINY_FRESHNESS_TTL_SECONDS=456\n");
        let expected = [
            "file-key",
            "file-client",
            "file-callback",
            "file-registered",
            "https://file.test",
            "456",
        ];
        let file_values =
            super::super::exact_source_values_with_defaults(&names, &defaults, |name| {
                fixture.env(name)
            })
            .unwrap();
        for (key, value) in names.iter().zip(expected) {
            assert_eq!(file_values[*key], value);
            let env_values =
                super::super::exact_source_values_with_defaults(&names, &defaults, |name| {
                    if name == *key {
                        Some("env-value".into())
                    } else {
                        fixture.env(name)
                    }
                })
                .unwrap();
            assert_eq!(env_values[*key], "env-value");
        }
    }

    #[test]
    fn broker_uses_file_fallback_and_environment_precedence() {
        let fixture = Fixture::new(b"BUNGIE_API_KEY=file-key\nDESTINY_DATABASE_PATH=/tmp/synthetic.db\nDESTINY_DEFINITION_VERSION=v1\nDESTINY_FRESHNESS_TTL_SECONDS=900\nBUNGIE_PUBLIC_CLIENT_ID=file-client\nGHOSTLINK_TOKEN_BROKER_URL=https://file.test/\nBUNGIE_CALLBACK_URI=ghostlink://oauth/callback\nBUNGIE_REGISTERED_CALLBACK_URI=ghostlink://oauth/callback\n");
        for override_url in [None, Some("https://env.test/")] {
            let lookup = |name: &str| {
                if name == "GHOSTLINK_TOKEN_BROKER_URL" {
                    override_url
                        .map(OsString::from)
                        .or_else(|| fixture.env(name))
                } else {
                    fixture.env(name)
                }
            };
            let read =
                super::super::read_env_values(|names| exact_source_values(names, lookup)).unwrap();
            let read = ReadRuntimeConfig::from_values(&read).unwrap();
            let login = exact_source_values(
                &[
                    "BUNGIE_PUBLIC_CLIENT_ID",
                    "BUNGIE_CALLBACK_URI",
                    "BUNGIE_REGISTERED_CALLBACK_URI",
                    "GHOSTLINK_TOKEN_BROKER_URL",
                ],
                lookup,
            )
            .unwrap();
            let login = LoginRuntimeConfig::from_values(&login).unwrap();
            for mode in [
                login.auth_mode(),
                read.refresh_client().unwrap().auth_mode(),
            ] {
                match mode {
                    super::super::ClientAuthMode::Broker(base) => {
                        assert_eq!(base, override_url.unwrap_or("https://file.test/"))
                    }
                    _ => panic!("expected broker mode"),
                }
            }
        }
    }

    #[test]
    fn confidential_secret_uses_file_fallback_and_environment_precedence() {
        let fixture = Fixture::new(b"BUNGIE_API_KEY=file-key\nDESTINY_DATABASE_PATH=/tmp/synthetic.db\nDESTINY_DEFINITION_VERSION=v1\nDESTINY_FRESHNESS_TTL_SECONDS=900\nBUNGIE_PUBLIC_CLIENT_ID=file-client\nBUNGIE_CLIENT_SECRET=file-secret-sentinel\nBUNGIE_CALLBACK_URI=ghostlink://oauth/callback\nBUNGIE_REGISTERED_CALLBACK_URI=ghostlink://oauth/callback\n");
        for override_secret in [None, Some("env-secret-sentinel")] {
            let lookup = |name: &str| {
                if name == "BUNGIE_CLIENT_SECRET" {
                    override_secret
                        .map(OsString::from)
                        .or_else(|| fixture.env(name))
                } else {
                    fixture.env(name)
                }
            };
            let read =
                super::super::read_env_values(|names| exact_source_values(names, lookup)).unwrap();
            let read = ReadRuntimeConfig::from_values(&read).unwrap();
            let login = exact_source_values(
                &[
                    "BUNGIE_PUBLIC_CLIENT_ID",
                    "BUNGIE_CALLBACK_URI",
                    "BUNGIE_REGISTERED_CALLBACK_URI",
                    "BUNGIE_CLIENT_SECRET",
                ],
                lookup,
            )
            .unwrap();
            let login = LoginRuntimeConfig::from_values(&login).unwrap();
            let expected = override_secret.unwrap_or("file-secret-sentinel").as_bytes();
            assert_eq!(login.client_secret().unwrap().as_bytes(), expected);
            assert_eq!(
                read.refresh_client()
                    .unwrap()
                    .client_secret()
                    .unwrap()
                    .as_bytes(),
                expected
            );
            assert!(!format!("{login:?} {read:?}").contains("secret-sentinel"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_permissions_refuse_group_or_other_access() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new(b"KEY=value-sentinel");
        for mode in [0o640, 0o604] {
            std::fs::set_permissions(&fixture.0, std::fs::Permissions::from_mode(mode)).unwrap();
            sanitized(load(Some(fixture.0.clone())).unwrap_err());
        }
        std::fs::set_permissions(&fixture.0, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            load(Some(fixture.0.clone())).unwrap()["KEY"],
            "value-sentinel"
        );
    }
}
