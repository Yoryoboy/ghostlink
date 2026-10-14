#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

fn home(name: &str) -> PathBuf {
    let home =
        std::env::temp_dir().join(format!("ghostlink-credfile-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    home
}

fn list_characters(home: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ghostlink"))
        .args(["list-characters", "--json"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("BUNGIE_API_KEY", "x")
        .env("DESTINY_DATABASE_PATH", home.join("db"))
        .env(
            "GHOSTLINK_CREDENTIALS_FILE",
            home.join("ghostlink/credentials.json"),
        )
        .output()
        .unwrap()
}

fn combined(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn missing_credential_file_requires_authorization_without_creating_the_directory() {
    let home = home("missing");
    let out = list_characters(&home);
    assert_eq!(out.status.code(), Some(10), "{}", combined(&out));
    assert!(combined(&out).contains("\"authorization_required\""));
    assert!(!home.join("ghostlink").exists());
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn group_readable_credential_file_is_refused_without_leaking_or_modifying_it() {
    let home = home("open-perms");
    let dir = home.join("ghostlink");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("credentials.json");
    let body = br#"{"version":1,"access_token":"CANARY-TOK","access_expires_at":1}"#;
    std::fs::write(&file, body).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = list_characters(&home);
    assert_eq!(out.status.code(), Some(50), "{}", combined(&out));
    let text = combined(&out);
    assert!(text.contains("\"credential_store_unavailable\""));
    assert!(!text.contains("CANARY"));
    assert_eq!(std::fs::read(&file).unwrap(), body);
    std::fs::remove_dir_all(&home).unwrap();
}
