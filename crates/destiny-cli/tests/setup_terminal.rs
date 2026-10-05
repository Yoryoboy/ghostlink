//! Unix prompt smoke tests use an isolated PTY and fake wizard effects, never
//! the production setup entry, login, real agent files, or agent executables.
#![cfg(unix)]
use destiny_app::agents::{Agent, AgentReport, Outcome, Plan};
use destiny_cli::setup::{AgentEngine, MarkerStore, TerminalPrompter, wizard};

#[test]
fn terminal_prompt_child() {
    if std::env::var_os("GHOSTLINK_TEST_PROMPT_CHILD").is_none() {
        return;
    }
    struct Engine(usize);
    impl AgentEngine for Engine {
        fn detected(&self) -> Vec<Agent> {
            vec![Agent::Pi]
        }
        fn install(&mut self, _: &[Agent], _: Plan) -> Vec<AgentReport> {
            self.0 += 1;
            vec![AgentReport {
                agent: Agent::Pi,
                detected: true,
                skill: Some(Outcome::Installed),
                mcp: Some(Outcome::Installed),
            }]
        }
    }
    struct Marker(bool);
    impl MarkerStore for Marker {
        fn complete(&self) -> bool {
            self.0
        }
        fn write(&mut self) -> Result<(), std::io::Error> {
            self.0 = true;
            Ok(())
        }
    }
    let mut engine = Engine(0);
    let mut marker = Marker(false);
    let output = wizard(&mut TerminalPrompter, &mut engine, &mut marker, &mut || {
        panic!("test must never reach login")
    });
    assert_eq!(output.exit_code, 130);
    assert!(!marker.0);
    assert!(output.stderr.contains("Setup cancelled"));
    println!("cancelled exit=130 marker=false installs={}", engine.0);
}

#[test]
fn esc_and_ctrl_c_abort_every_real_prompt_without_saving_marker() {
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            // Only this test's own child; reap even on assertion failure.
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn until(master: &mut std::fs::File, text: &mut String, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !text.contains(needle) {
            assert!(Instant::now() < deadline, "PTY missing {needle}: {text}");
            let mut fd = libc::pollfd {
                fd: master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: live descriptor and one initialized pollfd.
            if unsafe { libc::poll(&mut fd, 1, 100) } <= 0 {
                continue;
            }
            let mut bytes = [0; 4096];
            let size = master.read(&mut bytes).expect("PTY read");
            assert!(size > 0, "PTY closed before {needle}: {text}");
            text.push_str(&String::from_utf8_lossy(&bytes[..size]));
        }
        // Dialoguer prints before console enters raw mode for the next key.
        std::thread::sleep(Duration::from_millis(30));
    }
    for prompt in 1..=3 {
        for key in *b"\x1b\x03" {
            let mut master = -1;
            let mut slave = -1;
            // SAFETY: valid output pointers; null selects default PTY settings.
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut master,
                        &mut slave,
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                },
                0
            );
            // SAFETY: openpty returned two uniquely owned descriptors.
            let (mut master, slave) = unsafe {
                (
                    std::fs::File::from_raw_fd(master),
                    std::fs::File::from_raw_fd(slave),
                )
            };
            let mut child = Child(
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "terminal_prompt_child", "--nocapture"])
                    .env_clear()
                    .env("GHOSTLINK_TEST_PROMPT_CHILD", "1")
                    .env("TERM", "xterm")
                    .stdin(Stdio::from(slave.try_clone().unwrap()))
                    .stdout(Stdio::from(slave.try_clone().unwrap()))
                    .stderr(Stdio::from(slave))
                    .spawn()
                    .unwrap(),
            );
            let mut text = String::new();
            until(
                &mut master,
                &mut text,
                "Which agents should Ghostlink connect to?",
            );
            if prompt > 1 {
                master.write_all(b"\r").unwrap();
                until(&mut master, &mut text, "Install");
            }
            if prompt > 2 {
                master.write_all(b"\r").unwrap();
                until(&mut master, &mut text, "Sign in to Bungie now?");
            }
            master.write_all(&[key]).unwrap();
            until(&mut master, &mut text, "cancelled exit=130 marker=false");
            assert!(text.contains(&format!("installs={}", usize::from(prompt == 3))));
            assert!(child.0.wait().unwrap().success(), "{text}");
        }
    }
}
