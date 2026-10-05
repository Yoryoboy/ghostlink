//! OS defaults over an injected environment; never inspect user files.

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Clone, Copy)]
pub(crate) enum Platform {
    Unix,
    Windows,
}

impl Platform {
    pub(crate) const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

pub(crate) fn database_path<F>(platform: Platform, mut lookup: F) -> Option<PathBuf>
where
    F: FnMut(&str) -> Option<OsString>,
{
    match platform {
        Platform::Unix => {
            if let Some(base) = lookup("XDG_DATA_HOME").filter(unix_absolute) {
                return Some(PathBuf::from(base).join("ghostlink/ghostlink.db"));
            }
            lookup("HOME")
                .filter(unix_absolute)
                .map(|base| PathBuf::from(base).join(".local/share/ghostlink/ghostlink.db"))
        }
        Platform::Windows => windows_path(lookup("LOCALAPPDATA"), r"ghostlink\ghostlink.db"),
    }
}

pub(super) fn unix_absolute(value: &OsString) -> bool {
    value.to_string_lossy().starts_with('/')
}

/// Check Windows syntax independently of the host so both rules are testable.
/// Drive-relative and root-relative paths are not absolute; UNC needs a server
/// and share. Keep the original OS string when appending the fixed suffix.
pub(super) fn windows_path(base: Option<OsString>, suffix: &str) -> Option<PathBuf> {
    let mut base = base?;
    let text = base.to_string_lossy();
    let bytes = text.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    let unc = text
        .strip_prefix(r"\\")
        .or_else(|| text.strip_prefix("//"))
        .is_some_and(|rest| {
            let mut parts = rest.split(['/', '\\']);
            parts.next().is_some_and(|part| !part.is_empty())
                && parts.next().is_some_and(|part| !part.is_empty())
        });
    if !drive && !unc {
        return None;
    }
    if !text.ends_with(['/', '\\']) {
        base.push(r"\");
    }
    base.push(suffix);
    Some(PathBuf::from(base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_defaults_follow_each_platform() {
        let resolve = |platform, xdg: &str, home: &str, local: &str| {
            database_path(platform, |name| match name {
                "XDG_DATA_HOME" => Some(xdg.into()),
                "HOME" => Some(home.into()),
                "LOCALAPPDATA" => Some(local.into()),
                _ => panic!("unexpected lookup"),
            })
        };
        assert_eq!(
            resolve(Platform::Unix, "/data", "/home", ""),
            Some(PathBuf::from("/data").join("ghostlink/ghostlink.db"))
        );
        for xdg in ["relative", ""] {
            assert_eq!(
                resolve(Platform::Unix, xdg, "/home", ""),
                Some(PathBuf::from("/home").join(".local/share/ghostlink/ghostlink.db"))
            );
        }
        assert_eq!(
            resolve(
                Platform::Windows,
                "/ignored",
                "/ignored",
                r"C:\Users\test\AppData\Local"
            ),
            Some(PathBuf::from(
                r"C:\Users\test\AppData\Local\ghostlink\ghostlink.db"
            ))
        );
        for invalid in ["", "relative", r"C:relative", r"\rooted"] {
            assert!(resolve(Platform::Windows, "", "", invalid).is_none());
        }
        assert!(resolve(Platform::Unix, "relative", "", "").is_none());
        assert!(resolve(Platform::Unix, "", "relative", "").is_none());
    }
}
