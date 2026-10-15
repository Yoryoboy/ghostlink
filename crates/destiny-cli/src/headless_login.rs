//! Non-interactive two-step login for a machine without a browser.
//!
//! `ghostlink login start` begins an authorization, persists it as the pending
//! login beside the credential file and prints the authorization URL; it opens no
//! browser, installs no handler and reads nothing from the terminal. The human
//! approves on any device and copies the callback address the callback page shows.
//! `ghostlink login complete` then runs in a later process: it reads that address
//! from its argument or one bounded line of standard input, claims the pending
//! login exactly once, re-enters the shared service in `Authorizing` with the
//! persisted state and expiry, and lets the service apply the same expiry, CSRF
//! state, replay and exchange rules as the interactive login.
//!
//! Ordering is the security-relevant part of `complete`:
//!
//! 1. a missing pending login fails before any input is read;
//! 2. the callback is parsed against the exact registered callback, and a
//!    malformed one fails while the pending login is kept, so a bad paste can be
//!    corrected;
//! 3. the pending login is claimed by an atomic rename before the code is
//!    exchanged, so it is single use even across concurrent processes, and any
//!    later outcome (state mismatch, expiry, denial, exchange failure) needs a
//!    new `login start`.
//!
//! The code, the callback address and the state are never rendered or stored.

use std::cell::RefCell;
use std::io::{BufRead, IsTerminal};
use std::rc::Rc;

use destiny_adapters::oauth::{
    ManualPasteBrowserAuthorization, ManualPasteCallbackReceiver, NoEchoReader,
};
use destiny_adapters::{
    PendingLogin, PendingLoginStore, SystemCommandLauncher, SystemSecureRandom,
};
use destiny_app::{
    AppError, AuthState, AuthorizationCallback, AuthorizationOutcome, AuthorizationService,
    CallbackReceiver, Clock, CredentialStore, ErrorCategory, LoginRuntimeConfig,
    PublicTokenExchange, SecureRandom, Timestamp,
};

use crate::command::{CliError, LoginAction};
use crate::login::{AUTHORIZATION_TTL_SECONDS, SharedManualBrowser, SystemTerminalEcho};
use crate::output::{OutputMode, render_login_cancelled, render_login_pending};
use crate::runner::{ProcessOutput, login_result};

/// The authorization `login start` began.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeadlessStart {
    /// The exact authorization URL to open on any device.
    pub authorization_url: String,
    /// When the pending login stops being accepted.
    pub expires_at: Timestamp,
}

/// The two process-separated halves of a headless login.
pub trait HeadlessLoginOperations {
    /// Begins authorization and persists the pending login.
    fn start(&mut self) -> Result<HeadlessStart, AppError>;
    /// Completes the pending login with one callback address read from `input`.
    fn complete(&mut self, input: &mut dyn BufRead) -> Result<AuthorizationOutcome, AppError>;
}

/// The service is driven through `handle_callback`, never through a receiver.
struct NoReceiver;

impl CallbackReceiver for NoReceiver {
    fn receive(&mut self) -> Result<AuthorizationCallback, AppError> {
        Err(AppError::new(ErrorCategory::UnsupportedOperation))
    }
}

fn no_pending_login() -> AppError {
    AppError::provider_message(
        ErrorCategory::AuthorizationRequired,
        "no pending headless login; run ghostlink login start first",
    )
}

/// One headless login process over the shared authorization service.
pub struct HeadlessLogin<R, X, S, K> {
    service: AuthorizationService<R, SharedManualBrowser, NoReceiver, X, S, K>,
    browser: Rc<RefCell<ManualPasteBrowserAuthorization<SystemCommandLauncher>>>,
    callback_uri: String,
    pending: PendingLoginStore,
}

impl<R, X, S, K> HeadlessLogin<R, X, S, K>
where
    R: SecureRandom,
    X: PublicTokenExchange,
    S: CredentialStore,
    K: Clock,
{
    /// Composes one process; the browser is never opened.
    pub fn new(
        client_id: &str,
        callback_uri: &str,
        random: R,
        exchange: X,
        store: S,
        clock: K,
        pending: PendingLoginStore,
    ) -> Self {
        let browser = Rc::new(RefCell::new(ManualPasteBrowserAuthorization::new(
            client_id,
            callback_uri,
            SystemCommandLauncher,
            false,
        )));
        let service = AuthorizationService::new(
            random,
            SharedManualBrowser(Rc::clone(&browser)),
            NoReceiver,
            exchange,
            store,
            clock,
            AUTHORIZATION_TTL_SECONDS,
        );
        Self {
            service,
            browser,
            callback_uri: callback_uri.to_owned(),
            pending,
        }
    }
}

impl<R, X, S, K> HeadlessLoginOperations for HeadlessLogin<R, X, S, K>
where
    R: SecureRandom,
    X: PublicTokenExchange,
    S: CredentialStore,
    K: Clock,
{
    fn start(&mut self) -> Result<HeadlessStart, AppError> {
        self.service.begin()?;
        let AuthState::Authorizing { state, expires_at } = self.service.state().clone() else {
            return Err(AppError::new(ErrorCategory::AuthorizationRequired));
        };
        let authorization_url = self
            .browser
            .borrow()
            .instruction()
            .map(|instruction| instruction.authorization_url.to_string())
            .ok_or_else(|| AppError::new(ErrorCategory::AuthorizationRequired))?;
        self.pending.save(&PendingLogin { state, expires_at })?;
        Ok(HeadlessStart {
            authorization_url,
            expires_at,
        })
    }

    fn complete(&mut self, input: &mut dyn BufRead) -> Result<AuthorizationOutcome, AppError> {
        if self.pending.load()?.is_none() {
            return Err(no_pending_login());
        }
        let callback = ManualPasteCallbackReceiver::new(&self.callback_uri, input)?.receive()?;
        let pending = self.pending.take()?.ok_or_else(no_pending_login)?;
        self.service.resume(pending.state, pending.expires_at)?;
        self.service.handle_callback(callback)
    }
}

/// Runs `login start` and renders the pending login.
pub fn run_login_start(login: &mut dyn HeadlessLoginOperations, mode: OutputMode) -> ProcessOutput {
    match login.start() {
        Ok(start) => ProcessOutput::success(render_login_pending(mode, &start)),
        Err(error) => ProcessOutput::failure(mode, error.into()),
    }
}

/// Runs `login complete` over one callback address read from `input`.
///
/// Once the pending login was claimed, a failed outcome cannot be retried, so
/// human output says how to start again.
pub fn run_login_complete(
    login: &mut dyn HeadlessLoginOperations,
    input: &mut dyn BufRead,
    mode: OutputMode,
) -> ProcessOutput {
    match login.complete(input) {
        Ok(outcome) => {
            let mut output = login_result(mode, outcome);
            if output.exit_code != 0 && mode == OutputMode::Human {
                output.stderr.push_str(
                    "the pending login was used; start again with: ghostlink login start\n",
                );
            }
            output
        }
        Err(error) => ProcessOutput::failure(mode, error.into()),
    }
}

/// The pending login beside the configured credential file.
fn production_pending() -> Result<PendingLoginStore, AppError> {
    PendingLoginStore::beside(&destiny_app::credentials_file()?)
}

/// Composes one production process from validated login configuration.
fn production_login() -> Result<Box<dyn HeadlessLoginOperations>, CliError> {
    let config = LoginRuntimeConfig::from_env().map_err(CliError::from)?;
    let adapters = crate::login::login_adapters(&config)?;
    Ok(Box::new(HeadlessLogin::new(
        config.public_client_id(),
        config.callback_uri().exact(),
        SystemSecureRandom::new(),
        adapters.exchange,
        adapters.store,
        adapters.clock,
        production_pending().map_err(CliError::from)?,
    )))
}

/// Runs one headless login subcommand in production.
///
/// `complete` reads the callback from its argument, or one bounded line of
/// standard input when the argument is absent or `-`. A terminal gets a prompt
/// on standard error and has echo turned off for the read; a pipe is read as is.
pub fn run_production(action: &LoginAction, mode: OutputMode) -> ProcessOutput {
    match action {
        LoginAction::Cancel => match production_pending() {
            Ok(pending) => run_login_cancel(&pending, mode),
            Err(error) => ProcessOutput::failure(mode, error.into()),
        },
        LoginAction::Start => match production_login() {
            Ok(mut login) => run_login_start(login.as_mut(), mode),
            Err(error) => ProcessOutput::failure(mode, error),
        },
        LoginAction::Complete(arguments) => {
            let mut login = match production_login() {
                Ok(login) => login,
                Err(error) => return ProcessOutput::failure(mode, error),
            };
            match arguments.callback_uri.as_deref() {
                Some(uri) if uri != "-" => {
                    let mut input = std::io::Cursor::new(uri.as_bytes());
                    run_login_complete(login.as_mut(), &mut input, mode)
                }
                _ => {
                    let stdin = std::io::stdin();
                    if stdin.is_terminal() && mode == OutputMode::Human {
                        eprintln!("paste the callback url and press enter:");
                    }
                    let mut input = NoEchoReader::new(stdin.lock(), SystemTerminalEcho::stdin());
                    run_login_complete(login.as_mut(), &mut input, mode)
                }
            }
        }
    }
}

/// Runs `login cancel`; no pending login is a success too.
pub fn run_login_cancel(pending: &PendingLoginStore, mode: OutputMode) -> ProcessOutput {
    match pending.delete() {
        Ok(removed) => ProcessOutput::success(render_login_cancelled(mode, removed)),
        Err(error) => ProcessOutput::failure(mode, error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HeadlessLogin, HeadlessLoginOperations, run_login_cancel, run_login_complete,
        run_login_start,
    };
    use crate::output::OutputMode;
    use destiny_adapters::{PendingLogin, PendingLoginStore};
    use destiny_app::{
        AccessCredential, AccessCredentialMetadata, AppError, AuthorizationCode,
        AuthorizationState, Clock, CredentialMaterial, CredentialStore, PublicTokenExchange,
        SecureRandom, Timestamp,
    };
    use std::cell::Cell;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::rc::Rc;

    const CALLBACK: &str = "ghostlink://oauth/callback";
    const CLIENT_ID: &str = "public-client-id";
    const STATE: &str = "fixed-test-state";
    const CODE: &str = "fixed-test-code";
    const MATERIAL: &[u8] = b"credential-sentinel";
    const NOW: u64 = 1_700_000_000;
    const EXPIRES_AT: u64 = 1_724_000_000;

    struct FixedRandom;

    impl SecureRandom for FixedRandom {
        fn generate_state(&mut self) -> Result<AuthorizationState, AppError> {
            Ok(AuthorizationState::from_bytes(STATE.as_bytes()))
        }
    }

    /// An exchange that counts its calls and returns one access credential.
    #[derive(Clone, Default)]
    struct CountingExchange(Rc<Cell<usize>>);

    impl PublicTokenExchange for CountingExchange {
        fn exchange(&mut self, code: AuthorizationCode) -> Result<AccessCredential, AppError> {
            assert_eq!(code.as_bytes(), CODE.as_bytes());
            self.0.set(self.0.get() + 1);
            Ok(AccessCredential {
                metadata: AccessCredentialMetadata {
                    expires_at: Timestamp::new(EXPIRES_AT),
                },
                material: CredentialMaterial::from_bytes(MATERIAL),
                refresh_grant: None,
            })
        }
    }

    #[derive(Clone, Default)]
    struct RecordingStore(Rc<Cell<usize>>);

    impl CredentialStore for RecordingStore {
        fn save(&mut self, _credential: AccessCredential) -> Result<(), AppError> {
            self.0.set(self.0.get() + 1);
            Ok(())
        }

        fn load(&self) -> Result<Option<AccessCredential>, AppError> {
            Ok(None)
        }

        fn delete(&mut self) -> Result<(), AppError> {
            Ok(())
        }
    }

    struct FixedClock(u64);

    impl Clock for FixedClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            Ok(Timestamp::new(self.0))
        }
    }

    /// One isolated credential directory; never the real user configuration.
    struct Fixture {
        dir: PathBuf,
        exchange: CountingExchange,
        store: RecordingStore,
    }

    impl Fixture {
        fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "ghostlink-headless-login-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            Self {
                dir,
                exchange: CountingExchange::default(),
                store: RecordingStore::default(),
            }
        }

        fn pending(&self) -> PendingLoginStore {
            PendingLoginStore::beside(&self.dir.join("credentials.json")).unwrap()
        }

        /// One login process at `now`, as each CLI invocation composes it.
        fn login(
            &self,
            now: u64,
        ) -> HeadlessLogin<FixedRandom, CountingExchange, RecordingStore, FixedClock> {
            HeadlessLogin::new(
                CLIENT_ID,
                CALLBACK,
                FixedRandom,
                self.exchange.clone(),
                self.store.clone(),
                FixedClock(now),
                self.pending(),
            )
        }

        fn exchanges(&self) -> usize {
            self.exchange.0.get()
        }

        fn saves(&self) -> usize {
            self.store.0.get()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn callback(query: &str) -> Cursor<Vec<u8>> {
        Cursor::new(format!("{CALLBACK}?{query}\n").into_bytes())
    }

    fn code_callback() -> Cursor<Vec<u8>> {
        callback(&format!("state={STATE}&code={CODE}"))
    }

    fn assert_no_secret(text: &str) {
        assert!(!text.contains(CODE), "the code was rendered");
        assert!(
            !text.contains("credential-sentinel"),
            "material was rendered"
        );
    }

    fn json(text: &str) -> serde_json::Value {
        serde_json::from_str(text.trim()).expect("one JSON object")
    }

    #[test]
    fn start_persists_the_pending_login_and_prints_the_url_and_expiry() {
        let fixture = Fixture::new();
        let output = run_login_start(&mut fixture.login(NOW), OutputMode::Human);

        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(output.stderr.is_empty());
        assert!(
            output
                .stdout
                .contains("https://www.bungie.net/en/oauth/authorize")
        );
        assert!(output.stdout.contains(&format!("state={STATE}")));
        assert!(
            output
                .stdout
                .contains(&format!("expires_at: {}", NOW + 300))
        );
        assert!(output.stdout.contains("ghostlink login complete"));
        assert_eq!(
            fixture.pending().load().unwrap(),
            Some(PendingLogin {
                state: AuthorizationState::from_bytes(STATE.as_bytes()),
                expires_at: Timestamp::new(NOW + 300),
            })
        );
        assert_eq!(fixture.exchanges(), 0);
        assert_eq!(fixture.saves(), 0);
    }

    #[test]
    fn start_in_json_prints_one_pending_object() {
        let fixture = Fixture::new();
        let output = run_login_start(&mut fixture.login(NOW), OutputMode::Json);

        assert_eq!(output.exit_code, 0);
        let value = json(&output.stdout);
        assert_eq!(value["kind"], "login");
        assert_eq!(value["status"], "pending");
        assert_eq!(value["expires_at"], NOW + 300);
        assert!(
            value["authorization_url"]
                .as_str()
                .unwrap()
                .starts_with("https://www.bungie.net/en/oauth/authorize")
        );
        assert_eq!(value.as_object().unwrap().len(), 4);
    }

    #[test]
    fn a_new_start_replaces_the_earlier_pending_login() {
        let fixture = Fixture::new();
        run_login_start(&mut fixture.login(NOW), OutputMode::Human);
        run_login_start(&mut fixture.login(NOW + 60), OutputMode::Human);
        assert_eq!(
            fixture.pending().load().unwrap().unwrap().expires_at,
            Timestamp::new(NOW + 360)
        );
    }

    #[test]
    fn complete_in_a_later_process_authorizes_once_and_consumes_the_pending_login() {
        let fixture = Fixture::new();
        run_login_start(&mut fixture.login(NOW), OutputMode::Human);

        let output = run_login_complete(
            &mut fixture.login(NOW + 30),
            &mut code_callback(),
            OutputMode::Human,
        );

        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(output.stdout.contains("authorized"));
        assert!(output.stdout.contains(&format!("expires_at: {EXPIRES_AT}")));
        assert_eq!(fixture.exchanges(), 1);
        assert_eq!(fixture.saves(), 1);
        assert_eq!(fixture.pending().load().unwrap(), None);
        assert_no_secret(&format!("{}{}", output.stdout, output.stderr));
    }

    #[test]
    fn complete_in_json_prints_the_authorized_object() {
        let fixture = Fixture::new();
        run_login_start(&mut fixture.login(NOW), OutputMode::Json);
        let output = run_login_complete(
            &mut fixture.login(NOW + 30),
            &mut code_callback(),
            OutputMode::Json,
        );
        assert_eq!(output.exit_code, 0);
        let value = json(&output.stdout);
        assert_eq!(value["status"], "authorized");
        assert_eq!(value["expires_at"], EXPIRES_AT);
    }

    #[test]
    fn a_completed_login_cannot_be_replayed() {
        let fixture = Fixture::new();
        run_login_start(&mut fixture.login(NOW), OutputMode::Human);
        run_login_complete(
            &mut fixture.login(NOW + 1),
            &mut code_callback(),
            OutputMode::Human,
        );

        let output = run_login_complete(
            &mut fixture.login(NOW + 2),
            &mut code_callback(),
            OutputMode::Json,
        );

        assert_eq!(output.exit_code, 10);
        assert!(output.stdout.is_empty());
        assert_eq!(
            json(&output.stderr)["error"]["code"],
            "authorization_required"
        );
        assert_eq!(fixture.exchanges(), 1);
        assert_eq!(fixture.saves(), 1);
    }

    #[test]
    fn complete_without_a_pending_login_fails_clearly_before_reading_input() {
        let fixture = Fixture::new();
        let mut input = code_callback();

        let output = run_login_complete(&mut fixture.login(NOW), &mut input, OutputMode::Human);

        assert_eq!(output.exit_code, 10);
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            "error: no pending headless login; run ghostlink login start first\n"
        );
        assert_eq!(input.position(), 0, "the callback was not read");
        assert_eq!(fixture.exchanges(), 0);
    }

    #[test]
    fn a_malformed_callback_is_rejected_and_keeps_the_pending_login() {
        let fixture = Fixture::new();
        run_login_start(&mut fixture.login(NOW), OutputMode::Human);
        for input in [
            "not a callback\n",
            "https://example.com/?state=x&code=y\n",
            "",
        ] {
            let output = run_login_complete(
                &mut fixture.login(NOW + 1),
                &mut Cursor::new(input.as_bytes().to_vec()),
                OutputMode::Human,
            );
            assert_eq!(output.exit_code, 10, "{input:?}");
            assert_eq!(output.stderr, "error: authorization state was rejected\n");
        }
        assert!(fixture.pending().load().unwrap().is_some());
        let output = run_login_complete(
            &mut fixture.login(NOW + 2),
            &mut code_callback(),
            OutputMode::Human,
        );
        assert_eq!(output.exit_code, 0, "a corrected paste still completes");
    }

    #[test]
    fn a_foreign_state_is_rejected_without_an_exchange_and_ends_the_pending_login() {
        let fixture = Fixture::new();
        run_login_start(&mut fixture.login(NOW), OutputMode::Human);

        let output = run_login_complete(
            &mut fixture.login(NOW + 1),
            &mut callback(&format!("state=other-state&code={CODE}")),
            OutputMode::Human,
        );

        assert_eq!(output.exit_code, 10);
        assert_eq!(
            output.stderr,
            "error: authorization state was rejected\nthe pending login was used; start again with: ghostlink login start\n"
        );
        assert!(!output.stderr.contains("other-state"));
        assert_eq!(fixture.exchanges(), 0);
        assert_eq!(fixture.pending().load().unwrap(), None);
    }

    #[test]
    fn an_expired_pending_login_is_rejected_without_an_exchange() {
        let fixture = Fixture::new();
        run_login_start(&mut fixture.login(NOW), OutputMode::Human);

        let output = run_login_complete(
            &mut fixture.login(NOW + 300),
            &mut code_callback(),
            OutputMode::Json,
        );

        assert_eq!(output.exit_code, 21);
        assert!(output.stdout.is_empty());
        assert_eq!(
            json(&output.stderr)["error"]["code"],
            "provider_unavailable"
        );
        assert_eq!(
            json(&output.stderr)["error"]["message"],
            "the authorization callback timed out"
        );
        assert_eq!(fixture.exchanges(), 0);
        assert_eq!(fixture.pending().load().unwrap(), None);
    }

    #[test]
    fn denied_and_cancelled_callbacks_report_their_outcome_without_an_exchange() {
        for (query, code, exit) in [
            ("error=access_denied", "authorization_denied", 10),
            ("error=cancelled", "authorization_required", 10),
        ] {
            let fixture = Fixture::new();
            run_login_start(&mut fixture.login(NOW), OutputMode::Human);
            let output = run_login_complete(
                &mut fixture.login(NOW + 1),
                &mut callback(&format!("state={STATE}&{query}")),
                OutputMode::Json,
            );
            assert_eq!(output.exit_code, exit, "{query}");
            assert_eq!(json(&output.stderr)["error"]["code"], code, "{query}");
            assert_eq!(fixture.exchanges(), 0);
            assert_eq!(fixture.pending().load().unwrap(), None);
        }
    }

    #[test]
    fn cancel_removes_the_pending_login_and_is_idempotent() {
        let fixture = Fixture::new();
        run_login_start(&mut fixture.login(NOW), OutputMode::Human);

        let output = run_login_cancel(&fixture.pending(), OutputMode::Human);
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout, "pending login cancelled\n");
        assert_eq!(fixture.pending().load().unwrap(), None);

        let output = run_login_cancel(&fixture.pending(), OutputMode::Human);
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout, "no pending login\n");

        let output = run_login_cancel(&fixture.pending(), OutputMode::Json);
        assert_eq!(
            json(&output.stdout),
            serde_json::json!({"kind": "login", "status": "none"})
        );

        let output = run_login_complete(
            &mut fixture.login(NOW + 1),
            &mut code_callback(),
            OutputMode::Human,
        );
        assert_eq!(output.exit_code, 10, "a cancelled login cannot complete");
        assert_eq!(fixture.exchanges(), 0);
    }

    #[test]
    fn cancel_in_json_reports_the_cancelled_status() {
        let fixture = Fixture::new();
        run_login_start(&mut fixture.login(NOW), OutputMode::Human);
        let output = run_login_cancel(&fixture.pending(), OutputMode::Json);
        assert_eq!(
            json(&output.stdout),
            serde_json::json!({"kind": "login", "status": "cancelled"})
        );
    }

    #[test]
    fn the_trait_seam_accepts_a_trait_object() {
        let fixture = Fixture::new();
        let mut login = fixture.login(NOW);
        let login: &mut dyn HeadlessLoginOperations = &mut login;
        assert_eq!(run_login_start(login, OutputMode::Human).exit_code, 0);
    }
}
