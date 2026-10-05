//! First-run marker uses the OS default data directory, independently of any
//! database override. Reuses the pure OS resolver without loading config.env.
use crate::config::paths;

pub fn marker_path_with<F>(lookup: F) -> Option<std::path::PathBuf>
where
    F: FnMut(&str) -> Option<std::ffi::OsString>,
{
    let database = paths::database_path(paths::Platform::current(), lookup)?;
    Some(database.parent()?.join("setup-complete"))
}
pub fn marker_path() -> Option<std::path::PathBuf> {
    marker_path_with(|key| std::env::var_os(key))
}

#[cfg(test)]
mod tests {
    #[test]
    fn marker_is_next_to_default_database_and_ignores_overrides() {
        let base = if cfg!(windows) { r"C:\data" } else { "/data" };
        let path = super::marker_path_with(|key| match key {
            "XDG_DATA_HOME" | "LOCALAPPDATA" => Some(base.into()),
            _ => panic!("unexpected lookup: {key}"),
        })
        .unwrap();
        assert_eq!(
            path,
            std::path::PathBuf::from(base).join("ghostlink/setup-complete")
        );
    }
}
