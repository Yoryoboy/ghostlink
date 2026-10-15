//! Public-binary checks of the two-step headless login. Every case is offline:
//! none reaches the token exchange.
#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const CALLBACK: &str = "ghostlink://oauth/callback";
const HTTPS_CALLBACK: &str = "https://ghostlink-token-broker.93jads.workers.dev/callback";

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

/// Runs the binary with no callback environment, so the compiled defaults apply.
fn ghostlink_defaults(home: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ghostlink"))
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
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

fn start_defaults(home: &Path) -> serde_json::Value {
    let out = ghostlink_defaults(home, &["--json", "login", "start"], None);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    json(&out.stdout)
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
fn compiled_https_defaults_accept_both_callback_bases_and_reject_a_foreign_host() {
    let home = home("compiled-https");
    let value = start_defaults(&home);
    let url = value["authorization_url"].as_str().unwrap();
    assert!(
        url.contains(
            "redirect_uri=https%3A%2F%2Fghostlink-token-broker.93jads.workers.dev%2Fcallback"
        ),
        "{url}"
    );

    // The configured HTTPS callback with a foreign state reaches state
    // validation, so the shape was accepted and the pending login is consumed.
    let configured = format!("{HTTPS_CALLBACK}?state=foreign-state&code=CANARY-CODE");
    let out = ghostlink_defaults(&home, &["login", "complete", &configured], None);
    assert_eq!(out.status.code(), Some(10));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("CANARY-CODE"));
    assert!(
        !pending_file(&home).exists(),
        "the configured HTTPS callback was accepted"
    );

    // The custom-scheme link the broker page offers is accepted too.
    start_defaults(&home);
    let custom = format!("{CALLBACK}?state=foreign-state&code=CANARY-CODE");
    let out = ghostlink_defaults(&home, &["login", "complete", &custom], None);
    assert_eq!(out.status.code(), Some(10));
    assert!(
        !pending_file(&home).exists(),
        "the ghostlink callback was accepted"
    );

    // A foreign host is rejected as a malformed callback, so the pending login
    // is kept and can be corrected, with no network exchange.
    start_defaults(&home);
    let foreign = "https://evil.example/callback?state=foreign-state&code=CANARY-CODE";
    let out = ghostlink_defaults(&home, &["login", "complete", foreign], None);
    assert_eq!(out.status.code(), Some(10));
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, b"error: authorization state was rejected\n");
    assert!(pending_file(&home).exists(), "a bad paste can be corrected");
    assert!(!home.join("ghostlink/credentials.json").exists());
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn config_env_ghostlink_callback_still_completes_offline() {
    use std::os::unix::fs::OpenOptionsExt;
    let home = home("config-env-ghostlink");
    let dir = home.join("config/ghostlink");
    std::fs::create_dir_all(&dir).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(dir.join("config.env"))
        .unwrap();
    file.write_all(
        b"BUNGIE_PUBLIC_CLIENT_ID=12345\nBUNGIE_CALLBACK_URI=ghostlink://oauth/callback\nBUNGIE_REGISTERED_CALLBACK_URI=ghostlink://oauth/callback\n",
    )
    .unwrap();
    drop(file);

    let value = start_defaults(&home);
    let url = value["authorization_url"].as_str().unwrap();
    assert!(
        url.contains("redirect_uri=ghostlink%3A%2F%2Foauth%2Fcallback"),
        "{url}"
    );

    let configured = format!("{CALLBACK}?state=foreign-state&code=CANARY-CODE");
    let out = ghostlink_defaults(&home, &["login", "complete", &configured], None);
    assert_eq!(out.status.code(), Some(10));
    assert!(!pending_file(&home).exists());
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

#[test]
fn rejected_input_preserves_private_pending_bytes() {
    let home = home("verifier-rejections");
    start(&home);
    let path = pending_file(&home);
    let before = std::fs::read(&path).unwrap();
    for input in [
        "".to_owned(),
        "ghostlink://oauth/callback?state=x&state=y&code=CANARY-CODE\n".to_owned(),
        "ghostlink://oauth/callback?state=x&code=CANARY-CODE&error=access_denied\n".to_owned(),
        "x".repeat(8193),
    ] {
        let out = ghostlink(&home, &["login", "complete", "-"], Some(&input));
        assert_eq!(out.status.code(), Some(10));
        assert!(out.stdout.is_empty());
        assert_eq!(out.stderr, b"error: authorization state was rejected\n");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    for mode in [0o640, 0o604, 0o644] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        let out = ghostlink(
            &home,
            &["login", "complete", "-"],
            Some("ghostlink://oauth/callback?state=foreign&code=CANARY-CODE\n"),
        );
        assert_eq!(out.status.code(), Some(51));
        assert!(out.stdout.is_empty());
        assert_eq!(out.stderr, b"error: local storage failed\n");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    assert!(!home.join("ghostlink/credentials.json").exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn concurrent_complete_claims_only_once() {
    let home = home("verifier-concurrency");
    for _ in 0..25 {
        start(&home);
        let barrier = std::sync::Barrier::new(2);
        let mut outputs = std::thread::scope(|scope| {
            let invoke = || {
                barrier.wait();
                ghostlink(
                    &home,
                    &["login", "complete", "-"],
                    Some("ghostlink://oauth/callback?state=foreign&code=CANARY-CODE\n"),
                )
            };
            let first = scope.spawn(invoke);
            let second = scope.spawn(invoke);
            [first.join().unwrap(), second.join().unwrap()]
        });
        assert!(
            outputs
                .iter()
                .all(|out| out.status.code() == Some(10) && out.stdout.is_empty())
        );
        outputs.sort_by(|a, b| a.stderr.cmp(&b.stderr));
        assert_eq!(
            outputs[0].stderr,
            b"error: authorization state was rejected\nthe pending login was used; start again with: ghostlink login start\n"
        );
        assert_eq!(
            outputs[1].stderr,
            b"error: no pending headless login; run ghostlink login start first\n"
        );
        assert!(!pending_file(&home).exists());
        assert!(!home.join("ghostlink/credentials.json").exists());
        assert_eq!(
            std::fs::read_dir(home.join("ghostlink")).unwrap().count(),
            0
        );
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn callback_boundaries_keep_pending_until_the_base_is_accepted() {
    let home = home("https-boundaries");
    start_defaults(&home);
    let before = std::fs::read(pending_file(&home)).unwrap();
    for base in [
        "https://evil.example/callback",
        "https://ghostlink-token-broker.93jads.workers.dev/foreign",
        "https://user@ghostlink-token-broker.93jads.workers.dev/callback",
        "https://@ghostlink-token-broker.93jads.workers.dev/callback",
        "https:////@ghostlink-token-broker.93jads.workers.dev/callback",
        "HTTPS://ghostlink-token-broker.93jads.workers.dev/callback",
        "https://ghostlink-token-broker.93jads.workers.dev/callback/",
        "https://ghostlink-token-broker.93jads.workers.dev/%63allback",
        "https://ghostlink-token-broker.93jads.workers.dev:0/callback",
    ] {
        let uri = format!("{base}?state=foreign&code=CANARY");
        let out = ghostlink_defaults(&home, &["login", "complete", &uri], None);
        assert_eq!(out.status.code(), Some(10), "{base}");
        assert!(out.stdout.is_empty());
        assert_eq!(out.stderr, b"error: authorization state was rejected\n");
        assert_eq!(std::fs::read(pending_file(&home)).unwrap(), before);
    }
    for base in [
        HTTPS_CALLBACK,
        CALLBACK,
        "https://GHOSTLINK-TOKEN-BROKER.93jads.workers.dev/callback",
        "https://ghostlink-token-broker.93jads.workers.dev:443/callback",
    ] {
        start_defaults(&home);
        let uri = format!("{base}?state=foreign&code=CANARY");
        let out = ghostlink_defaults(&home, &["login", "complete", &uri], None);
        assert_eq!(out.status.code(), Some(10), "{base}");
        assert!(out.stdout.is_empty());
        assert_eq!(
            out.stderr,
            b"error: authorization state was rejected\n\
the pending login was used; start again with: ghostlink login start\n"
        );
        assert!(!pending_file(&home).exists());
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn https_config_validation_preserves_pending_on_rejection() {
    let home = home("https-config-boundaries");
    let dir = home.join("config/ghostlink");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.env");
    let write_config = |base: &str| {
        std::fs::write(
            &path,
            format!("BUNGIE_CALLBACK_URI={base}\nBUNGIE_REGISTERED_CALLBACK_URI={base}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    };
    for base in [
        CALLBACK,
        HTTPS_CALLBACK,
        "https://GHOSTLINK-TOKEN-BROKER.93jads.workers.dev/callback",
        "https://ghostlink-token-broker.93jads.workers.dev:443/callback",
        "https://ghostlink-token-broker.93jads.workers.dev/callback/",
        "https://ghostlink-token-broker.93jads.workers.dev/%63allback",
    ] {
        write_config(base);
        start_defaults(&home);
        let uri = format!("{base}?state=foreign&code=CANARY");
        let out = ghostlink_defaults(&home, &["login", "complete", &uri], None);
        assert_eq!(out.status.code(), Some(10), "{base}");
        assert!(!pending_file(&home).exists());
    }
    write_config(HTTPS_CALLBACK);
    start_defaults(&home);
    let before = std::fs::read(pending_file(&home)).unwrap();
    for base in [
        "http://example.test/callback",
        "https://user@example.test/callback",
        "https://@example.test/callback",
        "https://example.test/callback?",
        "https://example.test/callback#",
        "https://example.test:0/callback",
        "https:////@example.test/callback",
        "HTTPS://example.test/callback",
    ] {
        write_config(base);
        let out = ghostlink_defaults(&home, &["login", "start"], None);
        assert_eq!(out.status.code(), Some(52), "{base}");
        assert!(out.stdout.is_empty());
        assert_eq!(
            out.stderr,
            b"error: required runtime configuration is missing or invalid\n"
        );
        assert_eq!(std::fs::read(pending_file(&home)).unwrap(), before);
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn human_start_points_at_the_callback_page_not_the_custom_scheme() {
    let home = home("human-start");
    let out = ghostlink_defaults(&home, &["login", "start"], None);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "login pending");
    assert_eq!(lines[1], "open this url on any device and approve:");
    assert!(lines[2].starts_with("  https://www.bungie.net/en/oauth/authorize?"));
    assert!(lines[3].starts_with("expires_at: "));
    assert_eq!(
        &lines[4..],
        [
            "then copy the full callback address shown after approval (the Ghostlink callback page has a Copy button) and run:",
            "  ghostlink login complete",
            "and paste it when asked, or pass it as the argument",
        ]
    );
    assert!(!text.contains("ghostlink://"));
    std::fs::remove_dir_all(&home).unwrap();
}
