//! Public-binary checks of the two-step headless login. Every case is offline:
//! none reaches the token exchange.
#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const CALLBACK: &str = "ghostlink://oauth/callback";

fn home(name: &str) -> PathBuf {
    let home =
        std::env::temp_dir().join(format!("ghostlink-headless-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    home
}

fn pending_file(home: &Path) -> PathBuf {
    home.join("ghostlink/pending-login.json")
}

fn ghostlink(home: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ghostlink"))
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("BUNGIE_PUBLIC_CLIENT_ID", "12345")
        .env("BUNGIE_CALLBACK_URI", CALLBACK)
        .env("BUNGIE_REGISTERED_CALLBACK_URI", CALLBACK)
        .env("DESTINY_DATABASE_PATH", home.join("db"))
        .env(
            "GHOSTLINK_CREDENTIALS_FILE",
            home.join("ghostlink/credentials.json"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    if let Some(text) = stdin {
        pipe.write_all(text.as_bytes()).unwrap();
    }
    drop(pipe);
    child.wait_with_output().unwrap()
}

fn json(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).expect("one JSON object")
}

fn start(home: &Path) -> serde_json::Value {
    let out = ghostlink(home, &["--json", "login", "start"], None);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    json(&out.stdout)
}

#[test]
fn start_prints_the_pending_login_and_saves_a_private_file() {
    let home = home("start");
    let value = start(&home);
    assert_eq!(value["kind"], "login");
    assert_eq!(value["status"], "pending");
    let url = value["authorization_url"].as_str().unwrap();
    assert!(url.starts_with("https://www.bungie.net/en/oauth/authorize?"));
    assert!(url.contains("client_id=12345"));
    assert!(value["expires_at"].as_u64().unwrap() > 0);
    let mode = std::fs::metadata(pending_file(&home))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert!(!home.join("ghostlink/credentials.json").exists());
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn complete_with_a_foreign_state_is_rejected_and_ends_the_pending_login() {
    let home = home("foreign");
    start(&home);
    let callback = format!("{CALLBACK}?state=foreign-state&code=CANARY-CODE");
    let out = ghostlink(&home, &["login", "complete", &callback], None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(10), "{stderr}");
    assert!(out.stdout.is_empty());
    assert_eq!(
        stderr,
        "error: authorization state was rejected\nthe pending login was used; start again with: ghostlink login start\n"
    );
    assert!(!pending_file(&home).exists());
    assert!(!home.join("ghostlink/credentials.json").exists());
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn complete_reads_standard_input_and_keeps_the_login_after_a_malformed_paste() {
    let home = home("stdin");
    start(&home);
    let out = ghostlink(
        &home,
        &["--json", "login", "complete", "-"],
        Some("not a callback\n"),
    );
    assert_eq!(out.status.code(), Some(10));
    assert_eq!(json(&out.stderr)["error"]["code"], "authorization_state");
    assert!(pending_file(&home).exists(), "a bad paste can be corrected");

    let out = ghostlink(
        &home,
        &["login", "complete"],
        Some(&format!(
            "{CALLBACK}?state=foreign-state&code=CANARY-CODE\n"
        )),
    );
    assert_eq!(out.status.code(), Some(10));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("CANARY-CODE"));
    assert!(!pending_file(&home).exists());
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn complete_without_a_pending_login_fails_clearly() {
    let home = home("none");
    let out = ghostlink(&home, &["--json", "login", "complete", "-"], Some("x\n"));
    assert_eq!(out.status.code(), Some(10));
    assert!(out.stdout.is_empty());
    assert_eq!(
        json(&out.stderr),
        serde_json::json!({"error": {
            "code": "authorization_required",
            "message": "no pending headless login; run ghostlink login start first",
        }})
    );
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn cancel_discards_the_pending_login_and_is_idempotent() {
    let home = home("cancel");
    start(&home);
    let out = ghostlink(&home, &["login", "cancel"], None);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "pending login cancelled\n"
    );
    assert!(!pending_file(&home).exists());
    let out = ghostlink(&home, &["--json", "login", "cancel"], None);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        json(&out.stdout),
        serde_json::json!({"kind": "login", "status": "none"})
    );
    std::fs::remove_dir_all(&home).unwrap();
}
