//! Safe interactive login composition over a custom-scheme manual paste.
//!
//! This module composes the existing public-client authorization service with the
//! provider adapters into exactly one login attempt. The provider rejects an HTTP
//! loopback redirect for this registered public client, so the registered
//! callback is the custom-scheme URI `ghostlink://oauth/callback`. Nothing on the
//! machine listens for that scheme, and this product deliberately registers no
//! operating-system URI handler: the human authorizes in the browser, then copies
//! the callback URI the provider redirected to and pastes it into the terminal.
//!
//! Exactly one decision is made here that the baseline did not make: the callback
//! is received from the terminal instead of a local socket. Everything else has an
//! owner and stays there. The application service still owns unpredictable state,
//! expiry, CSRF comparison, replay rejection, and the code exchange; the adapters
//! own the authorization URL, the optional browser launch, the bounded paste
//! reader, the token transport, and the operating-system keyring.
//!
//! Security-relevant order, which the composition makes structural rather than
//! incidental:
//!
//! 1. configuration is validated first — the public client identifier and the
//!    exact custom-scheme callback the provider has registered — so nothing is
//!    launched or read against a configuration that is already known to be wrong;
//! 2. the authorization request starts, so the authorization URL exists;
//! 3. the operator instruction that carries that URL is presented before the
//!    terminal read blocks, because a human cannot act on a URL that only appears
//!    after the program is already waiting; delivery is fallible, so an
//!    instruction that cannot reach the operator fails closed with a sanitized
//!    error and the terminal read is never entered;
//! 4. only then is exactly one bounded callback line read from the terminal.
//!
//! Nothing here collects, reads, or renders a secret. The pasted callback is read
//! from standard input while terminal echo is disabled, so the authorization code
//! is never written back to the screen; it is also never rendered, never logged,
//! and never placed on the command line, so it never reaches shell history. Echo
//! is restored when the read completes, when it fails, and when the reader unwinds,
//! and — on Unix — when a terminating signal arrives while echo is still
//! suppressed. The handler is installed before echo is turned off, and the
//! transition between arming and disabling records any signal that arrives while
//! echo is being turned off and re-raises it only after echo is off again, so a
//! signal delivered on any thread cannot leave the terminal without echo. A signal
//! that kills the process before any handler can run is the one path the process
//! itself cannot repair.
//!
//! On Unix, echo suppression also replaces two process-wide signal dispositions and
//! publishes one process-wide restoration pointer, and neither can be held twice at
//! once. The armed suppression therefore claims exclusive ownership of that
//! process-wide state for exactly the window between arming and disarming — the
//! whole echo-off, terminal-read, echo-restore, and disarm region — and a second
//! overlapping attempt fails closed with a sanitized error before it can suppress
//! anything, leaving the first attempt's terminal, dispositions, and restoration
//! pointer untouched. Ownership is an atomic claim rather than a lock, so the
//! asynchronous signal handler never blocks on it.
//!
//! One limitation is documented rather than papered over: the terminal read has no
//! in-process deadline. The shared authorization TTL bounds what a late callback is
//! worth, but the process blocks until a line arrives, the input ends, or a signal
//! arrives. An operator who abandons an attempt must interrupt it with Ctrl-C,
//! which restores echo and exits, instead of waiting for the attempt to expire on
//! its own.
//!
//! The only credential that crosses this module is the opaque access credential the
//! exchange adapter returns, which the shared service hands straight to the
//! credential store; the CLI's rendering only ever receives expiry metadata.

use std::cell::RefCell;
use std::rc::Rc;

use destiny_adapters::oauth::{
    ManualPasteBrowserAuthorization, ManualPasteCallbackReceiver, NoEchoReader, TerminalEcho,
};
use destiny_adapters::{
    BungieTokenExchange, HttpTokenTransport, NativeCredentialStore, SystemClock,
    SystemCommandLauncher, SystemSecureRandom,
};
use destiny_app::auth_ux::{AuthorizationMode, EnvironmentGuiDetector, select_mode};
use destiny_app::{
    AppError, AuthorizationOutcome, AuthorizationRequest, AuthorizationService,
    BrowserAuthorization, CallbackReceiver, Clock, CredentialStore, ErrorCategory,
    LoginRuntimeConfig, PublicTokenExchange, SecureRandom,
};

use crate::command::{CallbackMode, CliError};

/// The bounded window one interactive authorization may stay pending, in seconds.
///
/// A human completes the browser step well inside five minutes, and a shorter
/// window means a forgotten or abandoned attempt expires instead of leaving a
/// pending state behind. The shared service enforces it; this constant only
/// supplies the bound. A paste that arrives after the bound is rejected as an
/// expired authorization before the code is ever exchanged.
const AUTHORIZATION_TTL_SECONDS: u64 = 300;

/// The operator instruction one interactive login produces.
///
/// It carries the authorization URL the human must open and the callback
/// strategy this attempt uses. The URL contains the CSRF state, which the
/// provider sends to the browser anyway; it is not secret material. The callback
/// the human pastes back, or the handler forwards, is never part of an
/// instruction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoginInstruction {
    /// The exact authorization URL the human must open, already rendered.
    pub authorization_url: String,
    /// How the callback for this attempt reaches the waiting process.
    pub callback_mode: CallbackMode,
}

/// The exact authorization operations one login attempt performs.
///
/// It is the narrow seam between the CLI dispatch and the composed application
/// service: the CLI owns ordering, exit statuses and rendering, while the composed
/// service owns unpredictable state, expiry, replay rejection and the exchange.
/// Keeping it this narrow is what lets the whole login path run offline in tests.
pub trait LoginOperations {
    /// Starts authorization, opening the browser or recording the instruction.
    fn begin(&mut self) -> Result<AuthorizationOutcome, AppError>;
    /// Awaits and consumes exactly one pasted authorization callback.
    fn receive_callback(&mut self) -> Result<AuthorizationOutcome, AppError>;
    /// The operator instruction the pending attempt produced, when one is needed.
    fn instruction(&self) -> Option<LoginInstruction>;
}

/// Supplies the operator instruction a selected mode may require.
pub trait LoginInstructionSource {
    /// The instruction recorded for the pending attempt, if the mode needs one.
    fn instruction(&self) -> Option<LoginInstruction>;
}

/// The instruction source of a mode that never needs a manual step.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoInstruction;

impl LoginInstructionSource for NoInstruction {
    fn instruction(&self) -> Option<LoginInstruction> {
        None
    }
}

/// The instruction source over the browser adapter that recorded the URL.
#[derive(Clone)]
struct SharedInstructionSource {
    browser: Rc<RefCell<ManualPasteBrowserAuthorization<SystemCommandLauncher>>>,
    callback_mode: CallbackMode,
}

impl LoginInstructionSource for SharedInstructionSource {
    fn instruction(&self) -> Option<LoginInstruction> {
        self.browser
            .borrow()
            .instruction()
            .map(|instruction| LoginInstruction {
                authorization_url: instruction.authorization_url.to_string(),
                callback_mode: self.callback_mode,
            })
    }
}

/// One composed login: the shared authorization service plus its instruction
/// source.
///
/// The service takes its browser by value, and the manual-paste browser records
/// the authorization URL inside `open`, so the instruction is read through a
/// shared handle rather than by reaching into the service.
pub struct LoginSession<R, B, C, X, S, K, I> {
    service: AuthorizationService<R, B, C, X, S, K>,
    instructions: I,
}

impl<R, B, C, X, S, K, I> LoginSession<R, B, C, X, S, K, I> {
    /// Pairs one composed authorization service with one instruction source.
    pub fn new(service: AuthorizationService<R, B, C, X, S, K>, instructions: I) -> Self {
        Self {
            service,
            instructions,
        }
    }
}

impl<R, B, C, X, S, K, I> LoginOperations for LoginSession<R, B, C, X, S, K, I>
where
    R: SecureRandom,
    B: BrowserAuthorization,
    C: CallbackReceiver,
    X: PublicTokenExchange,
    S: CredentialStore,
    K: Clock,
    I: LoginInstructionSource,
{
    fn begin(&mut self) -> Result<AuthorizationOutcome, AppError> {
        self.service.begin()
    }

    fn receive_callback(&mut self) -> Result<AuthorizationOutcome, AppError> {
        self.service.receive_callback()
    }

    fn instruction(&self) -> Option<LoginInstruction> {
        self.instructions.instruction()
    }
}

/// A shared handle around the manual-paste browser adapter.
///
/// The authorization service owns its browser, and the manual-paste adapter
/// records the authorization URL inside `open`. Passing a shared handle lets the
/// composition root read that instruction after `begin` without adding an
/// accessor to the service or duplicating the adapter.
#[derive(Clone)]
struct SharedManualBrowser(Rc<RefCell<ManualPasteBrowserAuthorization<SystemCommandLauncher>>>);

impl BrowserAuthorization for SharedManualBrowser {
    fn open(&mut self, request: AuthorizationRequest) -> Result<(), AppError> {
        self.0.borrow_mut().open(request)
    }
}

/// Composes one login for the selected authorization mode and callback strategy.
///
/// The mode is resolved here rather than at the parsing boundary because
/// resolution inspects the environment, which is a composition concern.
pub trait LoginFactory {
    /// Validates configuration and composes one login for `mode` and the
    /// selected `callback` strategy.
    fn build(
        &mut self,
        mode: AuthorizationMode,
        callback: CallbackMode,
    ) -> Result<Box<dyn LoginOperations>, CliError>;
}

/// The production login factory.
///
/// Construction acquires nothing: configuration is read, the paste reader is
/// built, and adapters are assembled only inside [`LoginFactory::build`], which
/// the process boundary calls only after a login command line parsed. A rejected
/// command line therefore never reads configuration, locks standard input, touches
/// the keyring, or launches a browser.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProductionLoginFactory;

impl ProductionLoginFactory {
    /// Builds the production factory. Construction is infallible.
    pub const fn new() -> Self {
        Self
    }
}

impl LoginFactory for ProductionLoginFactory {
    fn build(
        &mut self,
        mode: AuthorizationMode,
        callback: CallbackMode,
    ) -> Result<Box<dyn LoginOperations>, CliError> {
        let config = LoginRuntimeConfig::from_env().map_err(CliError::from)?;
        let resolved =
            select_mode(mode, &EnvironmentGuiDetector::default()).map_err(CliError::from)?;
        match resolved {
            AuthorizationMode::Browser => compose_login(&config, callback, true),
            AuthorizationMode::Headless => compose_login(&config, callback, false),
            // `select_mode` resolves every `Auto` into a concrete mode, so an
            // unresolved mode here is a programming error, not a user mistake.
            AuthorizationMode::Auto => {
                Err(CliError::from_category(ErrorCategory::ConfigurationError))
            }
        }
    }
}

/// Composes the login for one resolved browser mode and callback strategy.
fn compose_login(
    config: &LoginRuntimeConfig,
    callback: CallbackMode,
    open_browser: bool,
) -> Result<Box<dyn LoginOperations>, CliError> {
    match callback {
        CallbackMode::Paste => composed_login(config, open_browser).map(boxed),
        CallbackMode::Handler => composed_handler_login(config, open_browser),
    }
}

/// Erases one composed session behind the login seam.
fn boxed<L: LoginOperations + 'static>(login: L) -> Box<dyn LoginOperations> {
    Box::new(login)
}

/// A terminal-control failure, reported as a sanitized unsupported operation.
///
/// The paste reader fails closed when echo cannot be disabled, so this category
/// reaches the operator only as the reason a login could not start to read the
/// callback; it never carries any part of the pasted value.
fn terminal_error() -> AppError {
    AppError::new(ErrorCategory::UnsupportedOperation)
}

/// Applies terminal attributes, retrying when the call was interrupted.
///
/// `tcsetattr` reports `EINTR` when a signal arrives between its entry and the
/// change; that is not a terminal failure, so a genuine echo restoration is not
/// given up on because of it. Any other failure is reported by returning false.
#[cfg(unix)]
fn apply_termios(fd: std::os::fd::RawFd, attributes: &libc::termios) -> bool {
    loop {
        // SAFETY: `attributes` is a fully initialized attribute set for `fd`.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, attributes) } == 0 {
            return true;
        }
        // SAFETY: reading the thread-local `errno` is always valid.
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return false;
        }
    }
}

/// The production terminal-echo control.
///
/// On a real terminal it clears the terminal's echo flag for exactly as long as a
/// pasted callback is being read, and restores the saved attributes when the read
/// completes, when it fails, when the reader unwinds, and (on Unix) when a
/// terminating signal arrives while echo is still suppressed. When the input is
/// not a terminal — a pipe or a redirected file, which is how non-interactive use
/// hands over a callback — it changes nothing and reports success, so the bounded
/// paste read proceeds unmodified.
///
/// On Unix the suppression owns process-wide signal state, so it is exclusive
/// across the whole process: a second control that tries to suppress echo while
/// another one still holds that state fails closed before changing anything, and
/// the declined control can still succeed on a later attempt once the first one has
/// restored and released.
struct SystemTerminalEcho {
    #[cfg(unix)]
    fd: std::os::fd::RawFd,
    #[cfg(unix)]
    saved: Option<libc::termios>,
    #[cfg(unix)]
    previous: Option<terminal::PreviousDispositions>,
    #[cfg(windows)]
    handle: *mut core::ffi::c_void,
    #[cfg(windows)]
    saved_mode: Option<u32>,
    #[cfg(not(any(unix, windows)))]
    unsupported: core::marker::PhantomData<()>,
}

impl SystemTerminalEcho {
    /// The echo control for the process's standard input.
    #[cfg(unix)]
    fn stdin() -> Self {
        Self {
            fd: libc::STDIN_FILENO,
            saved: None,
            previous: None,
        }
    }

    /// The echo control for an explicit descriptor, used by terminal tests.
    #[cfg(unix)]
    #[cfg(test)]
    fn for_fd(fd: std::os::fd::RawFd) -> Self {
        Self {
            fd,
            saved: None,
            previous: None,
        }
    }

    /// Reports whether this control currently suppresses echo.
    #[cfg(unix)]
    #[cfg(test)]
    fn is_suppressed(&self) -> bool {
        self.saved.is_some()
    }

    /// The echo control for the process's standard input.
    #[cfg(windows)]
    fn stdin() -> Self {
        Self {
            handle: windows_console::stdin_handle(),
            saved_mode: None,
        }
    }

    /// The echo control for the process's standard input.
    #[cfg(not(any(unix, windows)))]
    fn stdin() -> Self {
        Self {
            unsupported: core::marker::PhantomData,
        }
    }
}

#[cfg(unix)]
impl TerminalEcho for SystemTerminalEcho {
    fn disable(&mut self) -> Result<(), AppError> {
        if self.saved.is_some() {
            return Ok(());
        }
        // A redirected or piped input has no terminal echo to suppress; the
        // bounded paste read still applies.
        // SAFETY: `isatty` only inspects the descriptor.
        if unsafe { libc::isatty(self.fd) } != 1 {
            return Ok(());
        }
        let mut current = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: `current` is a valid out-pointer for `tcgetattr`.
        if unsafe { libc::tcgetattr(self.fd, current.as_mut_ptr()) } != 0 {
            return Err(terminal_error());
        }
        // SAFETY: `tcgetattr` succeeded, so the attributes are initialized.
        let original = unsafe { current.assume_init() };
        // Block the terminating signals on this thread for the whole transition.
        // Other threads cannot be blocked from here, so they are covered by the
        // transition protocol in `terminal` instead: arming opens a deferring
        // window before the handler exists, and completing the transition closes
        // it only once echo is actually off, so a signal that lands in between is
        // recorded rather than acted on.
        let blocked = terminal::block_terminating_signals()?;
        // Arm interrupt restoration before echo is actually off, so there is no
        // window where echo is suppressed and no handler can restore it.
        let mut previous = terminal::arm(self.fd, &original)?;
        let mut silenced = original;
        silenced.c_lflag &= !libc::ECHO;
        if !apply_termios(self.fd, &silenced) {
            // Echo is still on, so the transition is aborted rather than
            // completed. Aborting closes the deferring window and re-raises any
            // signal the handler recorded while it was open, so a signal that
            // arrived during the transition is honored instead of dropped. When
            // the replaced dispositions cannot be put back, the armed state is
            // retained so `Drop` retries and no later attempt can inherit a
            // handler this attempt still owns.
            if terminal::abort(&mut previous).is_err() {
                self.previous = Some(previous);
            }
            return Err(terminal_error());
        }
        // Echo is off now. Completing the transition either marks it armed — from
        // which point the handler may restore and re-raise — or, when a signal was
        // deferred while the window was open, restores echo and re-raises that
        // signal here. In both cases this thread never writes the terminal again
        // after the handler could have repaired it.
        terminal::confirm_echo_suppressed();
        self.saved = Some(original);
        self.previous = Some(previous);
        // A signal that arrived while blocked is delivered here, past the window;
        // the handler now restores echo and re-raises.
        drop(blocked);
        Ok(())
    }

    fn restore(&mut self) -> Result<(), AppError> {
        if let Some(original) = self.saved {
            if !apply_termios(self.fd, &original) {
                // Echo is still off. The armed handler must stay installed and
                // the claim held: the handler is the remaining path that can
                // restore echo on a terminating signal, and a later attempt must
                // fail closed rather than inherit this handler. `Drop` retries
                // this restore, so a transient failure is not the last word.
                return Err(terminal_error());
            }
            self.saved = None;
        }
        // The descriptor is restored, so the remaining work only puts the
        // process-wide dispositions and pointer back. `disarm` releases the
        // process-wide claim only once the dispositions are actually back; a
        // failed restore keeps the claim alive so `Drop` retries instead of
        // leaking a handler it owns.
        if let Some(previous) = self.previous.as_mut() {
            terminal::disarm(previous)?;
            self.previous = None;
        }
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for SystemTerminalEcho {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// The Unix terminal primitives the echo control is built on.
///
/// Echo suppression alone is not enough on Unix: a terminating signal kills the
/// process without running any destructor, which would leave the terminal without
/// echo. The handler here reapplies the saved attributes and then re-raises the
/// signal, so the terminal is restored before the process dies.
///
/// The state the handler reads is a single immutable allocation published through
/// one atomic pointer. An earlier version kept the attributes in a mutable global
/// cell and a descriptor in a second atomic; a handler that loaded the descriptor
/// before `disarm` cleared it could then read attributes the next `arm` had already
/// overwritten, and the shared cell was written and read concurrently without a
/// data race being excluded. Publishing one fully initialized, never-mutated,
/// never-freed [`ArmedEcho`] per arming removes both hazards: an in-flight handler
/// keeps using the exact state it loaded, and a later login cannot overwrite it.
///
/// Arming also has to close a second window, one the pointer alone does not cover.
/// Because a terminating signal kills the process without running any destructor,
/// the handler must already be installed when echo is turned off. But a handler
/// that restores and re-raises the instant it runs can be overtaken: it restores
/// echo, re-raising terminates the process asynchronously, and the disabling
/// thread's own `tcsetattr` can still run afterwards and leave echo off on a
/// dead process. The fix is a two-phase transition. [`PHASE`] stays deferring from
/// the moment the handler is installed until the disabling thread has actually
/// turned echo off, and only then becomes armed. A handler that runs while the
/// window is open records the signal in [`PENDING`] and returns; the disabling
/// thread re-raises it in [`confirm_echo_suppressed`] after echo is off. Both
/// sides use `SeqCst`, so the handler and the transition thread cannot each miss
/// the other's store and lose the signal. The disabling thread also blocks the two
/// signals on itself across the transition, so its own delivery is serialized
/// after the window closes; signals delivered to other threads, which cannot be
/// blocked from here, are exactly what the deferring window handles.
///
/// Two things here are process-wide and single: the two signal dispositions the
/// handler replaces and the one [`ARMED_ECHO`] pointer. Arming a second of two
/// concurrent attempts would capture the first attempt's handler as the "previous"
/// disposition to restore, and would overwrite a pointer the first attempt still
/// needs, so a signal during the overlap could leave the terminal without echo.
/// [`ECHO_OWNERSHIP`] makes arming exclusive for the whole process: `arm` claims it
/// before touching any process-wide state and fails closed when another attempt
/// already owns it, and the claim is released only after the pointer is cleared and
/// the replaced dispositions are back. The handler reads only [`ARMED_ECHO`], never
/// the claim, so it stays async-signal-safe.
#[cfg(unix)]
mod terminal {
    use super::{AppError, ErrorCategory, terminal_error};
    use std::os::fd::RawFd;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU8, Ordering};

    /// The terminal attributes to reapply if a terminating signal arrives.
    ///
    /// `arm` allocates one instance, fully initializes it, and only then publishes
    /// it through [`ARMED_ECHO`]. The instance is never mutated and never freed, so
    /// the handler can dereference a pointer it loaded without racing a writer and
    /// without a later arming overwriting storage it is still reading. Both the
    /// atomic load and `tcsetattr` are async-signal-safe, which is what makes the
    /// handler legal.
    struct ArmedEcho {
        fd: RawFd,
        saved: libc::termios,
    }

    /// The currently armed restoration, or null while none is armed.
    ///
    /// `arm` publishes a fresh immutable [`ArmedEcho`] with release ordering;
    /// `disarm` clears the pointer when it restores the descriptor itself. The
    /// handler loads the pointer with acquire ordering, so it observes the fully
    /// initialized value, and a handler that loaded the previous pointer keeps
    /// using that immutable value even after `disarm` clears the pointer.
    static ARMED_ECHO: AtomicPtr<ArmedEcho> = AtomicPtr::new(std::ptr::null_mut());

    /// The process-wide claim one armed echo holds for its whole window.
    ///
    /// The signal dispositions `arm` replaces and the [`ARMED_ECHO`] slot are both
    /// process-wide singletons, so exactly one attempt may hold them at a time. The
    /// flag is claimed with a compare-and-swap and never touched by the signal
    /// handler, which keeps the handler free of locks.
    static ECHO_OWNERSHIP: AtomicBool = AtomicBool::new(false);

    /// The phase of the echo-off transition, written by the disabling thread and
    /// read by the signal handler.
    ///
    /// The handler must exist before echo is turned off, because a signal in the
    /// gap between the two would otherwise terminate with echo still disabled. But
    /// a handler that restores and re-raises the moment it runs can be overtaken:
    /// it restores echo, the re-raise is asynchronous, and the disabling thread's
    /// own `tcsetattr` still runs afterwards, leaving echo off on a dying process.
    /// The phase closes that window. It stays [`PHASE_DEFERRING`] from the instant
    /// the handler is installed until the disabling thread has actually turned echo
    /// off, and only then becomes [`PHASE_ARMED`]; a handler that runs before that
    /// point records the signal in [`PENDING`] and returns instead of restoring.
    static PHASE: AtomicU8 = AtomicU8::new(PHASE_IDLE);

    /// The signal a handler deferred because the transition was still open.
    ///
    /// The handler stores unconditionally and then reads [`PHASE`]; the transition
    /// thread stores [`PHASE_ARMED`] and then swaps this value. Both use `SeqCst`,
    /// so the two stores and the two loads are totally ordered and one side always
    /// observes the other: a deferred signal is either acted on by the handler
    /// itself (when it observed `Armed`) or re-raised by the transition thread.
    static PENDING: AtomicI32 = AtomicI32::new(0);

    /// No transition is open and no handler is trusted to restore.
    const PHASE_IDLE: u8 = 0;
    /// The handler is installed but echo is not off yet; signals must be deferred.
    const PHASE_DEFERRING: u8 = 1;
    /// Echo is off; the handler may restore and re-raise.
    const PHASE_ARMED: u8 = 2;

    /// The signal dispositions replaced while echo is suppressed.
    ///
    /// The value carries the process-wide claim, so the claim cannot outlive the
    /// armed state it protects: `disarm` and `abort` mark it releasable only once
    /// the replaced dispositions are actually back, and a partial install inside
    /// `install_handler` marks it releasable only when no disposition it replaced
    /// is still installed. A value dropped without that mark keeps the claim, so a
    /// later attempt fails closed instead of inheriting a handler this one still
    /// owns.
    pub(super) struct PreviousDispositions {
        interrupt: libc::sigaction,
        terminate: libc::sigaction,
        _ownership: EchoOwnership,
    }

    /// The exclusive process-wide claim an armed echo holds.
    ///
    /// Releasing the flag is the whole contract, and tying the release to a value
    /// makes it follow the lifetime of the armed state without a separate unlock
    /// call that a path could forget. The release is deliberately conditional: the
    /// flag is cleared only once the replaced dispositions are actually back. When
    /// they cannot be put back, dropping this value keeps the claim, so a later
    /// attempt fails closed instead of capturing a handler this attempt still owns
    /// as the disposition to restore and reinstalling that stale handler forever.
    /// Retaining the claim is the fail-closed direction in a short-lived CLI.
    struct EchoOwnership {
        /// Set only once the replaced dispositions are back.
        release_on_drop: bool,
    }

    impl EchoOwnership {
        /// Permits the process-wide claim to be released when this value drops.
        fn release(&mut self) {
            self.release_on_drop = true;
        }
    }

    impl Drop for EchoOwnership {
        fn drop(&mut self) {
            if self.release_on_drop {
                ECHO_OWNERSHIP.store(false, Ordering::Release);
            }
        }
    }

    /// Claims exclusive ownership of the process-wide armed state.
    ///
    /// A second concurrent attempt fails closed here, before it replaces a signal
    /// disposition or publishes a restoration pointer, so it can neither arm a
    /// second terminal nor destroy the first attempt's ability to restore the one it
    /// already owns.
    fn claim_ownership() -> Result<EchoOwnership, AppError> {
        ECHO_OWNERSHIP
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| EchoOwnership {
                release_on_drop: false,
            })
            .map_err(|_| ownership_conflict())
    }

    /// The sanitized error one overlapping attempt reports.
    ///
    /// The suppression is a second authorization trying to take state the first one
    /// already owns in this process, so the shared "already authorizing" category is
    /// the closest existing one; the error carries no terminal detail.
    fn ownership_conflict() -> AppError {
        AppError::new(ErrorCategory::AlreadyAuthorizing)
    }

    /// Reapplies the saved attributes and re-raises the delivered signal.
    ///
    /// While the transition is still open the handler must not touch the terminal:
    /// the disabling thread has not turned echo off yet, and restoring here would
    /// race that thread's own `tcsetattr`, which could then leave echo off after
    /// this handler has already re-raised. The handler therefore records the
    /// signal and returns; the transition thread re-raises it once echo is off.
    ///
    /// The store precedes the phase load on purpose. Together with the transition
    /// thread's `Armed`-then-swap order and `SeqCst` on both, it makes the pair of
    /// stores totally ordered: a handler that observes `Deferring` stored the
    /// signal first, so the transition thread's later swap must observe it.
    extern "C" fn restore_echo_and_reraise(signal: libc::c_int) {
        PENDING.store(signal, Ordering::SeqCst);
        if PHASE.load(Ordering::SeqCst) == PHASE_DEFERRING {
            return;
        }
        terminate_by_signal(signal);
    }

    /// Restores echo and terminates the process by `signal`.
    ///
    /// Called from the signal handler and from the transition thread after it has
    /// turned echo off. Every call is async-signal-safe: the only library calls
    /// are `signal`, `kill`, and `getpid`, and `restore_armed_echo` performs one
    /// atomic load and one `tcsetattr`.
    fn terminate_by_signal(signal: libc::c_int) {
        restore_armed_echo();
        // SAFETY: restoring the default disposition and re-raising the delivered
        // signal preserves ordinary termination semantics; `signal`, `kill`, and
        // `getpid` are async-signal-safe.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::kill(libc::getpid(), signal);
        }
    }

    /// Reapplies the saved attributes while echo is still suppressed.
    ///
    /// This is the body the signal handler runs. It is separated so a test can
    /// exercise the restoration path without delivering a real signal.
    pub(super) fn restore_armed_echo() {
        let armed = ARMED_ECHO.load(Ordering::Acquire);
        if armed.is_null() {
            return;
        }
        // SAFETY: `armed` was published by `arm` only after the pointee was fully
        // initialized, and the pointee is never mutated or freed while the process
        // lives. That gives the acquire load a happens-before edge over the
        // initialization store, so this shared read cannot race a writer, and the
        // loaded pointer stays valid for the whole call even if `disarm` clears the
        // global in the meantime. `tcsetattr` is async-signal-safe.
        unsafe {
            let armed = &*armed;
            libc::tcsetattr(armed.fd, libc::TCSANOW, &armed.saved);
        }
    }

    /// Publishes immutable restoration state and installs the terminating-signal handler.
    ///
    /// Ownership is claimed first, so a second overlapping attempt fails closed
    /// before `install_handler` can capture the owner's handler as the disposition to
    /// restore or before a pointer can be published over the owner's. The claim
    /// travels inside the returned value and is released only when that value is
    /// dropped after the replaced dispositions are back, which `disarm` and
    /// `abort` mark. A value that is dropped without them keeps the claim.
    pub(super) fn arm(fd: RawFd, saved: &libc::termios) -> Result<PreviousDispositions, AppError> {
        let ownership = claim_ownership()?;
        // Open the deferring window before the handler exists, so the handler can
        // never observe an unopened window. From here until
        // `confirm_echo_suppressed` runs, a terminating signal delivered to any
        // thread is recorded instead of acted on, because echo has not been turned
        // off yet.
        PENDING.store(0, Ordering::SeqCst);
        PHASE.store(PHASE_DEFERRING, Ordering::SeqCst);
        let previous = match install_handler(ownership) {
            Ok(previous) => previous,
            Err(error) => {
                // Close the window again. A handler that was installed before the
                // second `sigaction` failed may already have deferred a signal, so
                // the window closes through `close_transition` and that signal is
                // re-raised rather than dropped. Nothing was published through
                // `ARMED_ECHO`; `install_handler` releases the claim only when no
                // disposition it replaced is still installed, and retains it
                // otherwise.
                let deferred = close_transition();
                if deferred != 0 {
                    terminate_by_signal(deferred);
                }
                return Err(error);
            }
        };
        // Leak the state on purpose: a signal handler may still be reading an
        // earlier instance, and freeing the storage of a value an asynchronous
        // reader could touch cannot be made safe without blocking delivery on every
        // thread. The allocation is one small `termios` per interactive attempt in
        // a short-lived CLI process, so the bounded leak is the price of never
        // invalidating a pointer a handler might hold.
        let armed = Box::leak(Box::new(ArmedEcho { fd, saved: *saved }));
        // The release store publishes the fully initialized allocation; a handler
        // that acquires it therefore reads initialized attributes only.
        ARMED_ECHO.store(armed, Ordering::Release);
        Ok(previous)
    }

    /// Closes the transition window once the caller has turned echo off.
    ///
    /// Setting the armed phase makes every subsequent handler restore echo and
    /// re-raise, and then the deferred signal, if any, is re-raised here. Restoring
    /// echo first means the terminal is usable even though this thread, not the
    /// handler, delivers the termination.
    pub(super) fn confirm_echo_suppressed() {
        PHASE.store(PHASE_ARMED, Ordering::SeqCst);
        let deferred = PENDING.swap(0, Ordering::SeqCst);
        if deferred != 0 {
            terminate_by_signal(deferred);
        }
    }

    /// Closes the deferring window, clears the restoration pointer, and returns
    /// any signal the handler recorded while the window was open.
    ///
    /// The phase store precedes the pending swap and both sides use `SeqCst`, so a
    /// handler's `PENDING` store is either observed by this swap or the handler
    /// observes the idle phase and terminates itself instead of deferring. A
    /// nonzero return must be honored by the caller: the signal was deferred only
    /// because the transition was still open, and dropping it would silently
    /// change the process's termination semantics. The pointer is cleared before
    /// the swap, so a handler the swap wakes cannot double-restore a descriptor
    /// this transition is already tearing down; the leaked allocation stays valid
    /// for any handler that loaded it earlier.
    fn close_transition() -> libc::c_int {
        PHASE.store(PHASE_IDLE, Ordering::SeqCst);
        ARMED_ECHO.store(std::ptr::null_mut(), Ordering::Release);
        PENDING.swap(0, Ordering::SeqCst)
    }

    /// Puts the two replaced dispositions back, attempting both even when the
    /// first fails, and reports whether both succeeded.
    fn restore_dispositions(previous: &PreviousDispositions) -> bool {
        let mut restored = true;
        // Both are attempted even when the first fails.
        if !restore_disposition(libc::SIGINT, &previous.interrupt) {
            restored = false;
        }
        if !restore_disposition(libc::SIGTERM, &previous.terminate) {
            restored = false;
        }
        restored
    }

    /// Stops signal restoration and puts the replaced dispositions back.
    ///
    /// The caller restores the descriptor before calling this, so moving the phase
    /// to idle and clearing the pointer first is safe: a handler that races this
    /// call observes no armed state, restores nothing, and terminates with echo
    /// already on. When the dispositions cannot be put back, the claim is kept so a
    /// later attempt fails closed and the caller can retry through `Drop`.
    pub(super) fn disarm(previous: &mut PreviousDispositions) -> Result<(), AppError> {
        // Close the window and clear the restoration pointer. Any deferral is
        // discarded here: this is the completed-transition path, where
        // `confirm_echo_suppressed` has already consumed it, and failure exits
        // that must honor one go through `abort` instead.
        let _ = close_transition();
        if restore_dispositions(previous) {
            // The dispositions are back, so the claim may be released.
            previous._ownership.release();
            Ok(())
        } else {
            // Keep the claim: the handler is still installed and a later attempt
            // must fail closed rather than inherit it as its previous
            // disposition.
            Err(terminal_error())
        }
    }

    /// Aborts an armed transition that never turned echo off and honors any
    /// deferred signal.
    ///
    /// The caller has established that echo is on — the disabling `tcsetattr`
    /// failed, or the descriptor was restored — so tearing the armed state down is
    /// safe. The window is closed and any signal recorded while it was open is
    /// re-raised after the dispositions are put back, so the signal terminates the
    /// process instead of being silently dropped. When the dispositions cannot be
    /// put back, the claim is kept so a later attempt fails closed rather than
    /// inheriting this attempt's still-installed handler.
    pub(super) fn abort(previous: &mut PreviousDispositions) -> Result<(), AppError> {
        let deferred = close_transition();
        let restored = restore_dispositions(previous);
        if restored {
            previous._ownership.release();
        }
        if deferred != 0 {
            // Echo is on, so re-raising the deferred signal is safe. This does
            // not return.
            terminate_by_signal(deferred);
        }
        if restored {
            Ok(())
        } else {
            Err(terminal_error())
        }
    }

    /// The signal mask this thread had before the transition blocked two signals.
    ///
    /// Dropping the guard restores that mask, so every early return from the
    /// transition — including the fail-closed ones — unblocks the two terminating
    /// signals again.
    pub(super) struct SignalBlock {
        previous: libc::sigset_t,
    }

    impl Drop for SignalBlock {
        fn drop(&mut self) {
            // SAFETY: `previous` is the mask captured by `block_terminating_signals`.
            unsafe {
                libc::pthread_sigmask(libc::SIG_SETMASK, &self.previous, std::ptr::null_mut());
            }
        }
    }

    /// Blocks the terminating signals on the calling thread across the transition.
    ///
    /// Only the thread that turns echo off can be blocked here; a signal delivered
    /// to any other thread is handled by the deferring window instead. Blocking is
    /// still worth doing: it keeps a signal from interrupting this thread's own
    /// `tcsetattr`, and a signal that arrives meanwhile is delivered after the
    /// window closes, when the handler knows to restore.
    pub(super) fn block_terminating_signals() -> Result<SignalBlock, AppError> {
        // SAFETY: a zeroed `sigset_t` is the documented empty set; the `sigaddset`
        // calls fill it and `pthread_sigmask` applies it, reporting failure through
        // its return value rather than by touching the out-parameter.
        let mut blocked: libc::sigset_t = unsafe { std::mem::zeroed() };
        let mut previous: libc::sigset_t = unsafe { std::mem::zeroed() };
        let failed = unsafe {
            libc::sigemptyset(&mut blocked);
            libc::sigaddset(&mut blocked, libc::SIGINT);
            libc::sigaddset(&mut blocked, libc::SIGTERM);
            libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut previous) != 0
        };
        if failed {
            return Err(terminal_error());
        }
        Ok(SignalBlock { previous })
    }

    /// Delivers `signal` through the real handler body, for tests that must observe
    /// the deferred state without sending a process signal.
    #[cfg(test)]
    pub(super) fn deliver_terminating_signal(signal: libc::c_int) {
        restore_echo_and_reraise(signal);
    }

    /// The signal a handler deferred, or zero when none is pending.
    #[cfg(test)]
    pub(super) fn deferred_signal() -> libc::c_int {
        PENDING.load(Ordering::SeqCst)
    }

    /// A test-only switch that makes one `sigaction` step report failure.
    ///
    /// Zero disables injection. It exists because the kernel cannot be asked to
    /// reject a valid `sigaction`, and the partial-install failure path it drives
    /// is otherwise unreachable. The serialized terminal tests set and clear it, so
    /// no other test observes a fault.
    #[cfg(test)]
    static INSTALL_FAULT: AtomicU8 = AtomicU8::new(0);

    /// Makes the termination install report failure.
    #[cfg(test)]
    const FAULT_SECOND_INSTALL: u8 = 0b01;
    /// Makes the interruption rollback report failure as well.
    #[cfg(test)]
    const FAULT_INTERRUPT_ROLLBACK: u8 = 0b10;

    /// Injects the `sigaction` faults the partial-install tests drive.
    ///
    /// `rollback_fails` selects between the two outcomes the failure path has: the
    /// interruption disposition comes back (the claim is released) or it does not
    /// (the claim is retained).
    #[cfg(test)]
    pub(super) fn inject_install_fault(rollback_fails: bool) {
        let mut fault = FAULT_SECOND_INSTALL;
        if rollback_fails {
            fault |= FAULT_INTERRUPT_ROLLBACK;
        }
        INSTALL_FAULT.store(fault, Ordering::SeqCst);
    }

    /// Clears the test-only install fault.
    #[cfg(test)]
    pub(super) fn clear_install_fault() {
        INSTALL_FAULT.store(0, Ordering::SeqCst);
    }

    /// Releases the process-wide claim after a test deliberately drove the
    /// retain-the-claim failure path, which leaves it held by design.
    #[cfg(test)]
    pub(super) fn release_claim_for_test() {
        ECHO_OWNERSHIP.store(false, Ordering::Release);
    }

    /// Installs one terminating-signal disposition and reports whether it took.
    ///
    /// The syscall lives behind this seam so the partial-install failure path can
    /// be driven in tests, where the kernel cannot be asked to reject a valid
    /// request.
    fn install_disposition(
        signal: libc::c_int,
        action: &libc::sigaction,
        previous: &mut libc::sigaction,
    ) -> bool {
        #[cfg(test)]
        {
            if signal == libc::SIGTERM
                && INSTALL_FAULT.load(Ordering::SeqCst) & FAULT_SECOND_INSTALL != 0
            {
                return false;
            }
        }
        // SAFETY: `action` is a valid handler for the duration of the process and
        // `previous` is valid writable memory for the replaced disposition.
        unsafe { libc::sigaction(signal, action, previous) == 0 }
    }

    /// Puts one terminating-signal disposition back and reports whether it took.
    fn restore_disposition(signal: libc::c_int, previous: &libc::sigaction) -> bool {
        #[cfg(test)]
        {
            if signal == libc::SIGINT
                && INSTALL_FAULT.load(Ordering::SeqCst) & FAULT_INTERRUPT_ROLLBACK != 0
            {
                return false;
            }
        }
        // SAFETY: `previous` was captured for this same signal.
        unsafe { libc::sigaction(signal, previous, std::ptr::null_mut()) == 0 }
    }

    fn install_handler(mut ownership: EchoOwnership) -> Result<PreviousDispositions, AppError> {
        // SAFETY: `sigaction` is a plain C struct whose zero value is the
        // documented state of no handler and an empty mask.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = restore_echo_and_reraise as *const () as libc::sighandler_t;
        action.sa_flags = 0;
        // SAFETY: `sa_mask` is an initialized field of `action`.
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
        }
        // SAFETY: the out-parameters receive the replaced dispositions.
        let mut interrupt: libc::sigaction = unsafe { std::mem::zeroed() };
        let mut terminate: libc::sigaction = unsafe { std::mem::zeroed() };
        if !install_disposition(libc::SIGINT, &action, &mut interrupt) {
            // Nothing was installed, so the claim may be released.
            ownership.release();
            return Err(terminal_error());
        }
        if !install_disposition(libc::SIGTERM, &action, &mut terminate) {
            // Only the interruption disposition was replaced, so it is the only
            // one to put back. The claim may be released only if that rollback
            // actually succeeds: when it fails the handler this attempt installed
            // is still live, so the claim is retained and a later attempt fails
            // closed instead of capturing that stale handler as the disposition it
            // would restore and reinstalling it for the process lifetime.
            if restore_disposition(libc::SIGINT, &interrupt) {
                ownership.release();
            }
            return Err(terminal_error());
        }
        Ok(PreviousDispositions {
            interrupt,
            terminate,
            _ownership: ownership,
        })
    }
}

/// The Windows console primitives the echo control is built on.
#[cfg(windows)]
mod windows_console {
    use core::ffi::c_void;

    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(kind: u32) -> *mut c_void;
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
    }

    // SAFETY: `GetStdHandle` only queries the standard input handle.
    pub(super) fn stdin_handle() -> *mut c_void {
        unsafe { GetStdHandle(STD_INPUT_HANDLE) }
    }

    pub(super) fn mode(handle: *mut c_void) -> Option<u32> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut mode = 0u32;
        // SAFETY: `mode` is a valid out-pointer; a failed call means the handle
        // is not a console, which callers treat as non-interactive input.
        if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
            None
        } else {
            Some(mode)
        }
    }

    pub(super) fn set_mode(handle: *mut c_void, mode: u32) -> bool {
        // SAFETY: the handle comes from `GetStdHandle` and was accepted by
        // `GetConsoleMode`, so it is a live console handle.
        unsafe { SetConsoleMode(handle, mode) != 0 }
    }

    pub(super) fn echo_flag() -> u32 {
        ENABLE_ECHO_INPUT
    }

    const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;
}

#[cfg(windows)]
impl TerminalEcho for SystemTerminalEcho {
    fn disable(&mut self) -> Result<(), AppError> {
        if self.saved_mode.is_some() {
            return Ok(());
        }
        // A redirected input has no console mode to change; the bounded paste
        // read still applies.
        let Some(current) = windows_console::mode(self.handle) else {
            return Ok(());
        };
        if !windows_console::set_mode(self.handle, current & !windows_console::echo_flag()) {
            return Err(terminal_error());
        }
        self.saved_mode = Some(current);
        Ok(())
    }

    fn restore(&mut self) -> Result<(), AppError> {
        let Some(current) = self.saved_mode else {
            return Ok(());
        };
        if !windows_console::set_mode(self.handle, current) {
            return Err(terminal_error());
        }
        self.saved_mode = None;
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for SystemTerminalEcho {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// The platforms without a terminal-echo primitive leave input unchanged.
#[cfg(not(any(unix, windows)))]
impl TerminalEcho for SystemTerminalEcho {
    fn disable(&mut self) -> Result<(), AppError> {
        Ok(())
    }

    fn restore(&mut self) -> Result<(), AppError> {
        Ok(())
    }
}

/// The adapters every login mode shares.
///
/// Each composition builds its callback receiver before these adapters, so a
/// configuration whose callback cannot be the registered product callback fails
/// here, before the token transport or the credential store is constructed.
struct LoginAdapters {
    exchange: BungieTokenExchange<HttpTokenTransport, SystemClock>,
    store: NativeCredentialStore,
    clock: SystemClock,
}

/// Builds the shared adapters over the validated configuration.
fn login_adapters(config: &LoginRuntimeConfig) -> Result<LoginAdapters, CliError> {
    let callback_uri = config.callback_uri().exact();
    let transport = HttpTokenTransport::new().map_err(CliError::from)?;
    let exchange = BungieTokenExchange::new(
        config.public_client_id(),
        callback_uri,
        transport,
        SystemClock::new(),
    )
    .map_err(CliError::from)?;
    let store = NativeCredentialStore::new().map_err(CliError::from)?;
    Ok(LoginAdapters {
        exchange,
        store,
        clock: SystemClock::new(),
    })
}

/// Composes the manual-paste login, optionally opening the system browser.
///
/// Both modes always record and present the authorization URL; `open_browser`
/// only decides whether the adapter also asks the operating system to open it.
/// The pasted callback is read from the terminal in either case.
fn composed_login(
    config: &LoginRuntimeConfig,
    open_browser: bool,
) -> Result<ComposedLogin, CliError> {
    let callback = ManualPasteCallbackReceiver::new(
        config.callback_uri().exact(),
        NoEchoReader::new(std::io::stdin().lock(), SystemTerminalEcho::stdin()),
    )
    .map_err(CliError::from)?;
    let adapters = login_adapters(config)?;
    let browser = Rc::new(RefCell::new(ManualPasteBrowserAuthorization::new(
        config.public_client_id(),
        config.callback_uri().exact(),
        SystemCommandLauncher,
        open_browser,
    )));
    let service = AuthorizationService::new(
        SystemSecureRandom::new(),
        SharedManualBrowser(Rc::clone(&browser)),
        callback,
        adapters.exchange,
        adapters.store,
        adapters.clock,
        AUTHORIZATION_TTL_SECONDS,
    );
    Ok(LoginSession::new(
        service,
        SharedInstructionSource {
            browser,
            callback_mode: CallbackMode::Paste,
        },
    ))
}

/// The composed production paste login.
type ComposedLogin = LoginSession<
    SystemSecureRandom,
    SharedManualBrowser,
    ManualPasteCallbackReceiver<NoEchoReader<std::io::StdinLock<'static>, SystemTerminalEcho>>,
    BungieTokenExchange<HttpTokenTransport, SystemClock>,
    NativeCredentialStore,
    SystemClock,
    SharedInstructionSource,
>;

/// Composes the Linux handler login over the per-user callback rendezvous.
///
/// The browser still records and presents the authorization URL; the callback is
/// delivered by the installed desktop handler instead of the terminal. The
/// receiver binds the per-attempt rendezvous before the token transport and the
/// credential store are built, so an unavailable rendezvous fails closed before
/// anything else is acquired.
#[cfg(target_os = "linux")]
fn composed_handler_session(
    config: &LoginRuntimeConfig,
    open_browser: bool,
) -> Result<ComposedHandlerLogin, CliError> {
    let callback = destiny_adapters::oauth::CallbackRendezvous::bind(config.callback_uri().exact())
        .map_err(CliError::from)?;
    let adapters = login_adapters(config)?;
    let browser = Rc::new(RefCell::new(ManualPasteBrowserAuthorization::new(
        config.public_client_id(),
        config.callback_uri().exact(),
        SystemCommandLauncher,
        open_browser,
    )));
    let service = AuthorizationService::new(
        SystemSecureRandom::new(),
        SharedManualBrowser(Rc::clone(&browser)),
        callback,
        adapters.exchange,
        adapters.store,
        adapters.clock,
        AUTHORIZATION_TTL_SECONDS,
    );
    Ok(LoginSession::new(
        service,
        SharedInstructionSource {
            browser,
            callback_mode: CallbackMode::Handler,
        },
    ))
}

/// The composed Linux handler login.
#[cfg(target_os = "linux")]
type ComposedHandlerLogin = LoginSession<
    SystemSecureRandom,
    SharedManualBrowser,
    destiny_adapters::oauth::CallbackRendezvous,
    BungieTokenExchange<HttpTokenTransport, SystemClock>,
    NativeCredentialStore,
    SystemClock,
    SharedInstructionSource,
>;

/// Composes the handler login, or fails closed on a platform without it.
///
/// The Linux handler path has no equivalent on other systems, so the strategy is
/// an explicit unsupported operation there and the terminal paste stays the
/// fallback.
fn composed_handler_login(
    config: &LoginRuntimeConfig,
    open_browser: bool,
) -> Result<Box<dyn LoginOperations>, CliError> {
    #[cfg(target_os = "linux")]
    {
        composed_handler_session(config, open_browser).map(boxed)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, open_browser);
        Err(CliError::from_category(ErrorCategory::UnsupportedOperation))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AUTHORIZATION_TTL_SECONDS, LoginInstruction, LoginInstructionSource, LoginSession,
        NoInstruction, SharedInstructionSource, SharedManualBrowser,
    };
    use crate::command::CallbackMode;
    use crate::output::OutputMode;
    use crate::runner::{ProcessOutput, run_login};
    use destiny_adapters::SystemCommandLauncher;
    use destiny_adapters::oauth::{ManualPasteBrowserAuthorization, ManualPasteCallbackReceiver};
    use destiny_app::auth_ux::{AuthorizationMode, GuiDetector, GuiEnvironment, select_mode};
    use destiny_app::{
        AccessCredential, AccessCredentialMetadata, AppError, AuthorizationCallback,
        AuthorizationCode, AuthorizationRequest, AuthorizationService, AuthorizationState,
        BrowserAuthorization, CallbackReceiver, CallbackResult, Clock, CredentialMaterial,
        CredentialStore, ErrorCategory, PublicTokenExchange, SecureRandom, Timestamp,
    };
    use std::cell::{Cell, RefCell};
    use std::io::Cursor;
    use std::rc::Rc;

    const CALLBACK: &str = "ghostlink://oauth/callback";
    const CLIENT_ID: &str = "public-client-id";
    const STATE: &[u8] = b"fixed-test-state".as_slice();
    const CODE: &[u8] = b"fixed-test-code".as_slice();
    const MATERIAL: &[u8] = b"credential-sentinel".as_slice();
    const NOW: u64 = 1_700_000_000;
    const EXPIRES_AT: u64 = 1_724_000_000;

    /// A deterministic state generator: the same 256-bit contract, no entropy.
    struct FixedRandom;

    impl SecureRandom for FixedRandom {
        fn generate_state(&mut self) -> Result<AuthorizationState, AppError> {
            Ok(AuthorizationState::from_bytes(STATE))
        }
    }

    /// A browser that records how often it was asked to open a page.
    #[derive(Clone, Default)]
    struct RecordingBrowser(Rc<Cell<usize>>);

    impl RecordingBrowser {
        fn opens(&self) -> usize {
            self.0.get()
        }
    }

    impl BrowserAuthorization for RecordingBrowser {
        fn open(&mut self, _request: AuthorizationRequest) -> Result<(), AppError> {
            self.0.set(self.0.get() + 1);
            Ok(())
        }
    }

    /// A receiver that hands back exactly one callback for the pending state.
    struct FixedCallback;

    impl CallbackReceiver for FixedCallback {
        fn receive(&mut self) -> Result<AuthorizationCallback, AppError> {
            Ok(AuthorizationCallback {
                state: AuthorizationState::from_bytes(STATE),
                result: CallbackResult::Code(AuthorizationCode::from_bytes(CODE)),
            })
        }
    }

    /// An exchange that returns one credential with diagnostic-bearing material.
    struct FixedExchange;

    impl PublicTokenExchange for FixedExchange {
        fn exchange(&mut self, _code: AuthorizationCode) -> Result<AccessCredential, AppError> {
            Ok(AccessCredential {
                metadata: AccessCredentialMetadata {
                    expires_at: Timestamp::new(EXPIRES_AT),
                },
                material: CredentialMaterial::from_bytes(MATERIAL),
            })
        }
    }

    /// A store that records how often a credential was persisted.
    #[derive(Clone, Default)]
    struct RecordingStore(Rc<Cell<usize>>);

    impl RecordingStore {
        fn saves(&self) -> usize {
            self.0.get()
        }
    }

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

    /// A clock fixed before every expiry in these tests.
    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            Ok(Timestamp::new(NOW))
        }
    }

    /// An instruction source that always reports one instruction.
    #[derive(Clone, Default)]
    struct FixedInstruction(Rc<RefCell<Option<LoginInstruction>>>);

    impl LoginInstructionSource for FixedInstruction {
        fn instruction(&self) -> Option<LoginInstruction> {
            self.0.borrow().clone()
        }
    }

    fn instruction() -> LoginInstruction {
        LoginInstruction {
            authorization_url: format!(
                "https://www.bungie.net/en/oauth/authorize?client_id={CLIENT_ID}&state=fixed-test-state"
            ),
            callback_mode: CallbackMode::Paste,
        }
    }

    /// Drives one log over a real authorization service and injected ports.
    fn composed(
        store: RecordingStore,
        browser: RecordingBrowser,
        instructions: FixedInstruction,
    ) -> LoginSession<
        FixedRandom,
        RecordingBrowser,
        FixedCallback,
        FixedExchange,
        RecordingStore,
        FixedClock,
        FixedInstruction,
    > {
        let service = AuthorizationService::new(
            FixedRandom,
            browser,
            FixedCallback,
            FixedExchange,
            store,
            FixedClock,
            AUTHORIZATION_TTL_SECONDS,
        );
        LoginSession::new(service, instructions)
    }

    /// One login composed over the real paste receiver and injected ports.
    type PasteLogin = LoginSession<
        FixedRandom,
        RecordingBrowser,
        ManualPasteCallbackReceiver<Cursor<Vec<u8>>>,
        FixedExchange,
        RecordingStore,
        FixedClock,
        FixedInstruction,
    >;

    /// Drives one log through the real paste receiver over an injected line.
    fn composed_paste(
        pasted: &str,
        store: RecordingStore,
        browser: RecordingBrowser,
        instructions: FixedInstruction,
    ) -> PasteLogin {
        let receiver =
            ManualPasteCallbackReceiver::new(CALLBACK, Cursor::new(pasted.as_bytes().to_vec()))
                .expect("the registered custom-scheme callback validates");
        let service = AuthorizationService::new(
            FixedRandom,
            browser,
            receiver,
            FixedExchange,
            store,
            FixedClock,
            AUTHORIZATION_TTL_SECONDS,
        );
        LoginSession::new(service, instructions)
    }

    #[test]
    fn a_composed_login_runs_the_state_machine_end_to_end_offline() {
        let store = RecordingStore::default();
        let browser = RecordingBrowser::default();
        let mut login = composed(store.clone(), browser.clone(), FixedInstruction::default());

        let mut presented: Vec<String> = Vec::new();
        let output: ProcessOutput = run_login(&mut login, OutputMode::Human, &mut |text: &str| {
            presented.push(text.to_string());
            Ok(())
        });

        assert_eq!(output.exit_code, 0);
        assert!(output.stderr.is_empty());
        assert!(output.stdout.contains("authorized"));
        assert!(output.stdout.contains(&format!("expires_at: {EXPIRES_AT}")));
        // The service generated, compared, exchanged and persisted exactly once.
        assert_eq!(browser.opens(), 1);
        assert_eq!(store.saves(), 1);
        // No instruction source fired for this mode, and no secret was rendered.
        assert!(presented.is_empty());
        let rendered = format!("{}{}", output.stdout, output.stderr);
        assert!(!rendered.contains("credential-sentinel"));
        assert!(!rendered.contains(std::str::from_utf8(STATE).expect("ascii state")));
    }

    #[test]
    fn the_instruction_is_presented_before_the_callback_is_awaited() {
        let instructions = FixedInstruction::default();
        *instructions.0.borrow_mut() = Some(instruction());
        let mut login = composed(
            RecordingStore::default(),
            RecordingBrowser::default(),
            instructions,
        );

        let mut presented: Vec<String> = Vec::new();
        let output = run_login(&mut login, OutputMode::Human, &mut |text: &str| {
            presented.push(text.to_string());
            Ok(())
        });

        assert_eq!(output.exit_code, 0);
        assert_eq!(presented.len(), 1, "one instruction block is presented");
        let rendered = &presented[0];
        assert!(rendered.contains("authorization required"));
        assert!(rendered.contains("state=fixed-test-state"));
        assert!(rendered.contains("paste the callback url"));
        assert!(rendered.contains("waiting for the pasted callback"));
        // The instruction reached the operator before the terminal outcome.
        assert!(rendered.find("waiting for").is_some());
        assert!(output.stdout.contains("authorized"));
    }

    #[test]
    fn json_mode_emits_a_bounded_instruction_object_then_the_outcome_object() {
        let instructions = FixedInstruction::default();
        *instructions.0.borrow_mut() = Some(instruction());
        let mut login = composed(
            RecordingStore::default(),
            RecordingBrowser::default(),
            instructions,
        );

        let mut presented: Vec<String> = Vec::new();
        let output = run_login(&mut login, OutputMode::Json, &mut |text: &str| {
            presented.push(text.to_string());
            Ok(())
        });

        assert_eq!(output.exit_code, 0);
        assert_eq!(presented.len(), 1);
        let authorizing: serde_json::Value =
            serde_json::from_str(&presented[0]).expect("one bounded JSON object");
        assert_eq!(authorizing["kind"], "login");
        assert_eq!(authorizing["status"], "authorizing");
        assert!(
            authorizing["authorization_url"]
                .as_str()
                .expect("a rendered url")
                .starts_with("https://www.bungie.net/en/oauth/authorize")
        );
        let outcome: serde_json::Value =
            serde_json::from_str(output.stdout.trim()).expect("one bounded JSON object");
        assert_eq!(outcome["kind"], "login");
        assert_eq!(outcome["status"], "authorized");
        assert_eq!(outcome["expires_at"], EXPIRES_AT);
    }

    #[test]
    fn a_pasted_callback_carrying_the_pending_state_authorizes_once() {
        // The request goes through the real paste reader: the pasted line must
        // satisfy the exact registered callback shape, and the state it carries
        // must then pass the service's own cryptographic comparison.
        let store = RecordingStore::default();
        let browser = RecordingBrowser::default();
        let mut login = composed_paste(
            "ghostlink://oauth/callback?state=fixed-test-state&code=fixed-test-code\n",
            store.clone(),
            browser.clone(),
            FixedInstruction::default(),
        );

        let output = run_login(&mut login, OutputMode::Human, &mut |_| Ok(()));

        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.contains("authorized"));
        assert_eq!(browser.opens(), 1);
        assert_eq!(store.saves(), 1);
        // The pasted code and state never reach either stream.
        let rendered = format!("{}{}", output.stdout, output.stderr);
        assert!(!rendered.contains("fixed-test-code"));
        assert!(!rendered.contains("fixed-test-state"));
    }

    #[test]
    fn a_pasted_callback_with_the_wrong_state_is_rejected_by_the_service() {
        let store = RecordingStore::default();
        let mut login = composed_paste(
            "ghostlink://oauth/callback?state=wrong-state&code=fixed-test-code\n",
            store.clone(),
            RecordingBrowser::default(),
            FixedInstruction::default(),
        );

        let output = run_login(&mut login, OutputMode::Json, &mut |_| Ok(()));

        assert_eq!(output.exit_code, 10);
        assert!(output.stdout.is_empty());
        let parsed: serde_json::Value =
            serde_json::from_str(output.stderr.trim()).expect("one bounded JSON object");
        assert_eq!(parsed["error"]["code"], "authorization_state");
        // Nothing was exchanged or stored for a rejected state.
        assert_eq!(store.saves(), 0);
        assert!(!output.stderr.contains("wrong-state"));
    }

    #[test]
    fn a_malformed_paste_is_rejected_before_any_exchange() {
        let store = RecordingStore::default();
        let mut login = composed_paste(
            "not a callback uri\n",
            store.clone(),
            RecordingBrowser::default(),
            FixedInstruction::default(),
        );

        let output = run_login(&mut login, OutputMode::Human, &mut |_| Ok(()));

        assert_eq!(output.exit_code, 10);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "error: authorization state was rejected\n");
        assert_eq!(store.saves(), 0);
    }

    /// A unique temporary rendezvous directory for one handler test.
    #[cfg(target_os = "linux")]
    fn handler_test_dir(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "ghostlink-login-handler-{}-{label}-{unique}",
            std::process::id()
        ))
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_forwarded_handler_callback_authorizes_through_the_service_once() {
        // The real Linux rendezvous feeds the real authorization service: the
        // forwarded URI must satisfy the exact registered callback shape, and the
        // state it carries must then pass the service's own cryptographic check.
        let directory = handler_test_dir("authorize");
        let receiver =
            destiny_adapters::oauth::CallbackRendezvous::bind_in(directory.clone(), CALLBACK)
                .expect("the registered custom-scheme callback binds a rendezvous");
        let store = RecordingStore::default();
        let service = AuthorizationService::new(
            FixedRandom,
            RecordingBrowser::default(),
            receiver,
            FixedExchange,
            store.clone(),
            FixedClock,
            AUTHORIZATION_TTL_SECONDS,
        );
        let mut login = LoginSession::new(service, FixedInstruction::default());

        let forward_directory = directory.clone();
        let forwarder = std::thread::spawn(move || {
            destiny_adapters::oauth::forward_callback_in(
                &forward_directory,
                "ghostlink://oauth/callback?state=fixed-test-state&code=fixed-test-code",
            )
        });
        let output = run_login(&mut login, OutputMode::Human, &mut |_| Ok(()));
        assert!(forwarder.join().expect("the forwarder thread").is_ok());
        let _ = std::fs::remove_dir_all(&directory);

        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.contains("authorized"));
        assert_eq!(store.saves(), 1);
        // The forwarded code and state never reach either stream.
        let rendered = format!("{}{}", output.stdout, output.stderr);
        assert!(!rendered.contains("fixed-test-code"));
        assert!(!rendered.contains("fixed-test-state"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_forwarded_handler_callback_with_the_wrong_state_is_rejected() {
        let directory = handler_test_dir("mismatch");
        let receiver =
            destiny_adapters::oauth::CallbackRendezvous::bind_in(directory.clone(), CALLBACK)
                .expect("bind");
        let store = RecordingStore::default();
        let service = AuthorizationService::new(
            FixedRandom,
            RecordingBrowser::default(),
            receiver,
            FixedExchange,
            store.clone(),
            FixedClock,
            AUTHORIZATION_TTL_SECONDS,
        );
        let mut login = LoginSession::new(service, FixedInstruction::default());

        let forward_directory = directory.clone();
        let forwarder = std::thread::spawn(move || {
            destiny_adapters::oauth::forward_callback_in(
                &forward_directory,
                "ghostlink://oauth/callback?state=wrong-state&code=fixed-test-code",
            )
        });
        let output = run_login(&mut login, OutputMode::Json, &mut |_| Ok(()));
        assert!(forwarder.join().expect("the forwarder thread").is_ok());
        let _ = std::fs::remove_dir_all(&directory);

        assert_eq!(output.exit_code, 10);
        assert!(output.stdout.is_empty());
        let parsed: serde_json::Value =
            serde_json::from_str(output.stderr.trim()).expect("one bounded JSON object");
        assert_eq!(parsed["error"]["code"], "authorization_state");
        assert_eq!(store.saves(), 0);
        assert!(!output.stderr.contains("wrong-state"));
    }

    #[test]
    fn a_failed_begin_reports_the_sanitized_category_without_a_callback() {
        struct FailingCallback;

        impl CallbackReceiver for FailingCallback {
            fn receive(&mut self) -> Result<AuthorizationCallback, AppError> {
                Err(AppError::new(ErrorCategory::CallbackTimeout))
            }
        }

        struct FailingBrowser;

        impl BrowserAuthorization for FailingBrowser {
            fn open(&mut self, _request: AuthorizationRequest) -> Result<(), AppError> {
                Err(AppError::new(ErrorCategory::ProviderUnavailable))
            }
        }

        let service = AuthorizationService::new(
            FixedRandom,
            FailingBrowser,
            FailingCallback,
            FixedExchange,
            RecordingStore::default(),
            FixedClock,
            AUTHORIZATION_TTL_SECONDS,
        );
        let mut login = LoginSession::new(service, NoInstruction);
        let mut presented: Vec<String> = Vec::new();
        let output = run_login(&mut login, OutputMode::Human, &mut |text: &str| {
            presented.push(text.to_string());
            Ok(())
        });

        assert_eq!(output.exit_code, 21);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "error: the provider is unavailable\n");
        assert!(presented.is_empty());
    }

    #[test]
    fn the_shared_manual_browser_records_the_authorization_url_it_opened() {
        // The manual-paste adapter records the authorization URL inside `open`,
        // and the composition reads it through a shared handle; this is the only
        // path that makes the operator instruction reachable without an accessor
        // on the service. The custom-scheme callback is rendered into the URL.
        let browser = Rc::new(RefCell::new(ManualPasteBrowserAuthorization::new(
            CLIENT_ID,
            CALLBACK,
            SystemCommandLauncher,
            false,
        )));
        assert!(
            SharedInstructionSource {
                browser: Rc::clone(&browser),
                callback_mode: CallbackMode::Paste,
            }
            .instruction()
            .is_none()
        );

        SharedManualBrowser(Rc::clone(&browser))
            .open(AuthorizationRequest {
                state: AuthorizationState::from_bytes(STATE),
                expires_at: Timestamp::new(EXPIRES_AT),
            })
            .expect("a manual-paste request records the URL instead of launching");

        let reported = SharedInstructionSource {
            browser: Rc::clone(&browser),
            callback_mode: CallbackMode::Paste,
        }
        .instruction()
        .expect("the recorded instruction is reported");
        assert_eq!(reported.callback_mode, CallbackMode::Paste);
        assert!(
            reported
                .authorization_url
                .contains("state=fixed-test-state")
        );
        assert!(
            reported
                .authorization_url
                .contains("redirect_uri=ghostlink%3A%2F%2Foauth%2Fcallback")
        );

        let recorded = browser
            .borrow()
            .instruction()
            .cloned()
            .expect("the adapter kept the recorded instruction");
        assert_eq!(
            recorded.authorization_url.to_string(),
            reported.authorization_url
        );
    }

    #[test]
    fn the_browser_mode_never_reports_an_instruction_without_a_recording() {
        assert_eq!(NoInstruction.instruction(), None);
    }

    #[test]
    fn auto_mode_resolves_from_the_inspected_environment() {
        // `Auto` is resolved once, by the composition, from the environment the
        // process actually has; an explicit flag is never overridden.
        struct Report(bool);

        impl GuiDetector for Report {
            fn gui_available(&self) -> bool {
                self.0
            }
        }

        assert_eq!(
            select_mode(AuthorizationMode::Auto, &Report(true)).ok(),
            Some(AuthorizationMode::Browser)
        );
        assert_eq!(
            select_mode(AuthorizationMode::Auto, &Report(false)).ok(),
            Some(AuthorizationMode::Headless)
        );
        assert_eq!(
            select_mode(AuthorizationMode::Browser, &Report(false)).ok(),
            Some(AuthorizationMode::Browser)
        );
        assert_eq!(
            select_mode(AuthorizationMode::Headless, &Report(true)).ok(),
            Some(AuthorizationMode::Headless)
        );
        // The shared detector reads only its documented environment inputs.
        let environment = GuiEnvironment::for_platform("linux");
        assert!(!environment.ssh_connection);
        assert_eq!(environment.os, "linux");
    }

    /// Terminal-echo behavior against a real pseudo-terminal.
    ///
    /// These tests exercise the actual `tcgetattr`/`tcsetattr` path, the drop
    /// restoration, and the body the signal handler runs. They do not deliver a
    /// real signal while a read is blocked, so the `sigaction`/re-raise wiring is
    /// not covered here, and nothing in this suite claims to prove the browser
    /// custom-scheme handoff, which needs a human and a live provider.
    #[cfg(unix)]
    mod terminal_tests {
        use super::super::{SystemTerminalEcho, terminal};
        use super::{CALLBACK, CallbackResult};
        use destiny_adapters::oauth::{ManualPasteCallbackReceiver, NoEchoReader, TerminalEcho};
        use destiny_app::CallbackReceiver;
        use std::fs::File;
        use std::io::{BufRead, BufReader, Read, Write};
        use std::os::fd::{AsRawFd, FromRawFd, RawFd};
        use std::os::unix::process::ExitStatusExt;
        use std::process::{Command, Stdio};
        use std::sync::{Mutex, MutexGuard};

        /// Marks a re-executed test process as the signal target.
        const CHILD_ENV: &str = "GHOSTLINK_ECHO_SIGNAL_TEST";
        /// The inherited terminal descriptor the child suppresses echo on.
        const CHILD_FD_ENV: &str = "GHOSTLINK_ECHO_SIGNAL_FD";
        /// The line the child prints once echo is suppressed.
        const CHILD_READY: &str = "ghostlink-echo-signal-ready";
        /// The exact child test, re-run alone through `--exact`.
        const CHILD_TEST: &str = "login::tests::terminal_tests::signal_child_suppresses_echo_then_raises_a_real_interrupt";

        /// Serializes the tests that arm the process-wide signal state.
        static ECHO_GUARD: Mutex<()> = Mutex::new(());

        fn guard() -> MutexGuard<'static, ()> {
            ECHO_GUARD
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        /// Opens a pseudo-terminal pair, or `None` when the host has none.
        fn open_pty() -> Option<(File, File)> {
            // SAFETY: every return value is checked before it is used, and each
            // adopted descriptor is taken over by a `File` immediately.
            unsafe {
                let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
                if master < 0 {
                    return None;
                }
                if libc::grantpt(master) != 0 || libc::unlockpt(master) != 0 {
                    libc::close(master);
                    return None;
                }
                let name = libc::ptsname(master);
                if name.is_null() {
                    libc::close(master);
                    return None;
                }
                let slave = libc::open(name, libc::O_RDWR | libc::O_NOCTTY);
                if slave < 0 {
                    libc::close(master);
                    return None;
                }
                Some((File::from_raw_fd(master), File::from_raw_fd(slave)))
            }
        }

        /// Reads the echo flag of a live terminal descriptor.
        fn echo_enabled(fd: RawFd) -> bool {
            let mut current = std::mem::MaybeUninit::<libc::termios>::uninit();
            // SAFETY: `fd` is a live terminal descriptor supplied by the caller.
            let read = unsafe { libc::tcgetattr(fd, current.as_mut_ptr()) };
            assert_eq!(read, 0, "the descriptor must expose terminal attributes");
            // SAFETY: `tcgetattr` succeeded, so the attributes are initialized.
            let current = unsafe { current.assume_init() };
            current.c_lflag & libc::ECHO != 0
        }

        /// Writes the echo flag of a live terminal descriptor.
        fn set_echo(fd: RawFd, on: bool) {
            let mut current = std::mem::MaybeUninit::<libc::termios>::uninit();
            // SAFETY: `fd` is a live terminal descriptor supplied by the caller.
            let read = unsafe { libc::tcgetattr(fd, current.as_mut_ptr()) };
            assert_eq!(read, 0, "the descriptor must expose terminal attributes");
            // SAFETY: `tcgetattr` succeeded, so the attributes are initialized.
            let mut current = unsafe { current.assume_init() };
            if on {
                current.c_lflag |= libc::ECHO;
            } else {
                current.c_lflag &= !libc::ECHO;
            }
            // SAFETY: `current` is a fully initialized attribute set for this fd.
            let written = unsafe { libc::tcsetattr(fd, libc::TCSANOW, &current) };
            assert_eq!(written, 0, "the terminal accepts the new attributes");
        }

        /// Reads the current full disposition of one signal.
        fn signal_action(signal: libc::c_int) -> libc::sigaction {
            // SAFETY: `sigaction` is a plain C struct whose zero value is the
            // documented state of no handler and an empty mask.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            // SAFETY: a null request pointer queries the current disposition into
            // a valid out-parameter.
            unsafe {
                libc::sigaction(signal, std::ptr::null(), &mut action);
            }
            action
        }

        /// Reads the current disposition of one signal.
        fn signal_handler(signal: libc::c_int) -> libc::sighandler_t {
            signal_action(signal).sa_sigaction
        }

        /// Restores the process state a deliberate install fault leaves behind.
        ///
        /// The rollback-failure fault skips the real interruption restore, so the
        /// handler the failed attempt installed is still live and the claim is held
        /// on purpose; this guard puts both back even if the test panics.
        struct InstallFaultCleanup {
            original: libc::sigaction,
        }

        impl Drop for InstallFaultCleanup {
            fn drop(&mut self) {
                terminal::clear_install_fault();
                // SAFETY: `original` was captured for `SIGINT` before the fault.
                unsafe {
                    libc::sigaction(libc::SIGINT, &self.original, std::ptr::null_mut());
                }
                terminal::release_claim_for_test();
            }
        }

        #[test]
        fn system_echo_control_suppresses_and_restores_a_real_terminal() {
            let _guard = guard();
            let Some((_master, slave)) = open_pty() else {
                return;
            };
            let fd = slave.as_raw_fd();
            let mut echo = SystemTerminalEcho::for_fd(fd);
            assert!(echo_enabled(fd), "a fresh pseudo-terminal echoes input");

            echo.disable().expect("echo is suppressed on a terminal");
            assert!(echo.is_suppressed());
            assert!(!echo_enabled(fd), "the pasted code is not echoed");

            echo.restore().expect("echo restoration succeeds");
            assert!(!echo.is_suppressed());
            assert!(echo_enabled(fd), "echo is back after the read");
        }

        #[test]
        fn system_echo_control_restores_echo_when_dropped_while_suppressed() {
            let _guard = guard();
            let Some((_master, slave)) = open_pty() else {
                return;
            };
            let fd = slave.as_raw_fd();
            {
                let mut echo = SystemTerminalEcho::for_fd(fd);
                echo.disable().expect("echo is suppressed on a terminal");
                assert!(!echo_enabled(fd));
            }
            assert!(echo_enabled(fd), "dropping the control restores echo");
        }

        /// A signal that lands while echo is being turned off must not be acted on
        /// before the disabling thread has finished.
        ///
        /// This is the interleaving the pointer alone could not close: if the
        /// handler restored and re-raised here, the asynchronous termination would
        /// race the disabling thread's own `tcsetattr` and could leave echo off on
        /// the dying process. The test drives the exact window: the signal state is
        /// armed and echo is briefly off, but the transition has not been completed,
        /// so the handler body must record the signal instead of restoring echo.
        #[test]
        fn a_signal_during_the_echo_transition_is_deferred_instead_of_restoring_early() {
            let _guard = guard();
            let Some((_master, slave)) = open_pty() else {
                return;
            };
            let fd = slave.as_raw_fd();
            // Capture the attributes the way `disable` does, then open the window
            // exactly as `disable` does before it turns echo off.
            let mut current = std::mem::MaybeUninit::<libc::termios>::uninit();
            // SAFETY: `fd` is a live terminal descriptor.
            let read = unsafe { libc::tcgetattr(fd, current.as_mut_ptr()) };
            assert_eq!(read, 0);
            // SAFETY: `tcgetattr` succeeded, so the attributes are initialized.
            let original = unsafe { current.assume_init() };
            let mut previous = terminal::arm(fd, &original).expect("the transition arms");

            // The disabling thread has written echo off but has not completed the
            // transition: this is the vulnerable window.
            set_echo(fd, false);
            assert!(!echo_enabled(fd), "echo is briefly off mid-transition");

            // A terminating signal delivered to any thread runs the handler body now.
            terminal::deliver_terminating_signal(libc::SIGINT);

            // The fix under test: the handler deferred, so it did not restore echo
            // before the disabling thread's own `tcsetattr` could be overtaken.
            assert!(
                !echo_enabled(fd),
                "a deferred signal must not re-enable echo mid-transition"
            );
            assert_eq!(
                terminal::deferred_signal(),
                libc::SIGINT,
                "the deferred signal is recorded for the transition thread"
            );

            // Clean up without completing the transition: completing it would
            // re-raise the deferred signal and terminate the test process, and the
            // end-to-end child test already covers that half.
            terminal::disarm(&mut previous).expect("the transition disarms");
            assert_eq!(
                terminal::deferred_signal(),
                0,
                "disarming clears the deferral"
            );
            set_echo(fd, true);
        }

        /// An aborted transition puts the handler back and releases the claim.
        ///
        /// `abort` is the failure exit `disable` takes when it could not turn echo
        /// off. It must clear the restoration pointer, put the replaced
        /// dispositions back, and release the process-wide claim so the next
        /// attempt starts from a clean state instead of inheriting this handler.
        #[test]
        fn an_aborted_transition_releases_the_claim_and_the_handler() {
            let _guard = guard();
            let Some((_master, slave)) = open_pty() else {
                return;
            };
            let fd = slave.as_raw_fd();
            let original = signal_handler(libc::SIGINT);
            let mut current = std::mem::MaybeUninit::<libc::termios>::uninit();
            // SAFETY: `fd` is a live terminal descriptor.
            let read = unsafe { libc::tcgetattr(fd, current.as_mut_ptr()) };
            assert_eq!(read, 0);
            // SAFETY: `tcgetattr` succeeded, so the attributes are initialized.
            let original_attrs = unsafe { current.assume_init() };

            let mut previous = terminal::arm(fd, &original_attrs).expect("the transition arms");
            assert_ne!(
                signal_handler(libc::SIGINT),
                original,
                "arming installs the restoration handler"
            );

            // Echo was never turned off on this failure exit, so the transition is
            // aborted rather than completed.
            terminal::abort(&mut previous).expect("the aborted transition cleans up");
            assert_eq!(
                signal_handler(libc::SIGINT),
                original,
                "abort puts the replaced dispositions back"
            );
            // The claim is released with the value that carried it, so a fresh
            // transition can arm again.
            drop(previous);
            let mut again = terminal::arm(fd, &original_attrs).expect("the claim was released");
            terminal::disarm(&mut again).expect("the second transition disarms cleanly");
        }

        /// A failed partial install whose rollback succeeds releases the claim.
        ///
        /// `install_handler` replaces the interruption disposition first and the
        /// termination disposition second. When the second install fails, the
        /// interruption disposition is put back; once it is actually back the claim
        /// may be released and a later attempt starts clean. This is the success
        /// side of the partial-install failure path and guards against the fix
        /// retaining the claim unconditionally.
        #[test]
        fn a_failed_partial_install_releases_the_claim_when_the_rollback_succeeds() {
            let _guard = guard();
            let Some((_master, slave)) = open_pty() else {
                return;
            };
            let fd = slave.as_raw_fd();
            let original = signal_action(libc::SIGINT);
            let _cleanup = InstallFaultCleanup {
                original: signal_action(libc::SIGINT),
            };

            terminal::inject_install_fault(false);
            let mut failed = SystemTerminalEcho::for_fd(fd);
            assert!(
                failed.disable().is_err(),
                "the failed termination install is reported"
            );
            terminal::clear_install_fault();
            assert_eq!(
                signal_handler(libc::SIGINT),
                original.sa_sigaction,
                "a successful rollback puts the interruption disposition back"
            );

            // The released claim is available to the next attempt.
            let mut next = SystemTerminalEcho::for_fd(fd);
            next.disable()
                .expect("the released claim is available again");
            assert!(!echo_enabled(fd), "the next attempt suppresses echo");
            next.restore().expect("the next attempt restores cleanly");
            assert!(echo_enabled(fd), "echo is back after the restore");
        }

        /// A failed partial install whose rollback also fails retains the claim.
        ///
        /// If the interruption disposition cannot be put back, releasing the claim
        /// would let a later attempt capture the still-installed handler as its own
        /// previous disposition and reinstall that stale handler for the process
        /// lifetime. The claim must be retained so the later attempt fails closed
        /// instead.
        #[test]
        fn a_failed_partial_install_retains_the_claim_when_the_rollback_fails() {
            let _guard = guard();
            let Some((_master, slave)) = open_pty() else {
                return;
            };
            let Some((_second_master, second_slave)) = open_pty() else {
                return;
            };
            let fd = slave.as_raw_fd();
            let second_fd = second_slave.as_raw_fd();
            let original = signal_action(libc::SIGINT);
            let _cleanup = InstallFaultCleanup {
                original: signal_action(libc::SIGINT),
            };

            terminal::inject_install_fault(true);
            let mut failed = SystemTerminalEcho::for_fd(fd);
            assert!(
                failed.disable().is_err(),
                "the failed partial install is reported"
            );
            terminal::clear_install_fault();

            // The interruption handler the failed attempt installed is still live,
            // because the injected rollback never put it back.
            let stale = signal_handler(libc::SIGINT);
            assert_ne!(stale, original.sa_sigaction);
            assert_ne!(stale, libc::SIG_DFL);

            // The claim is retained, so a later attempt fails closed and leaves both
            // the stale handler and its own terminal untouched.
            let mut later = SystemTerminalEcho::for_fd(second_fd);
            assert!(
                later.disable().is_err(),
                "a retained claim makes the later attempt fail closed"
            );
            assert!(echo_enabled(second_fd), "the later attempt changed nothing");
            assert_eq!(
                signal_handler(libc::SIGINT),
                stale,
                "the later attempt did not overwrite the retained handler"
            );

            // A deliberately released claim is available again; the stale handler
            // is still installed here, which is exactly why retaining the claim is
            // the fail-closed direction until it is put back.
            terminal::release_claim_for_test();
            let mut recovered = SystemTerminalEcho::for_fd(second_fd);
            recovered
                .disable()
                .expect("a released claim is available again");
            recovered
                .restore()
                .expect("the recovered attempt restores cleanly");
        }

        #[test]
        fn the_interrupt_restoration_body_restores_a_real_terminal() {
            let _guard = guard();
            let Some((_master, slave)) = open_pty() else {
                return;
            };
            let fd = slave.as_raw_fd();
            let mut echo = SystemTerminalEcho::for_fd(fd);
            echo.disable().expect("echo is suppressed on a terminal");
            assert!(!echo_enabled(fd));

            // This is exactly what the signal handler runs before it re-raises
            // the delivered signal, so the restoration path is exercised even
            // though no signal is delivered in the suite.
            terminal::restore_armed_echo();
            assert!(echo_enabled(fd), "the interrupt path restores echo");
            echo.restore().expect("the control still restores cleanly");
        }

        #[test]
        fn arming_installs_and_disarming_restores_the_terminating_signal_handler() {
            let _guard = guard();
            let Some((_master, slave)) = open_pty() else {
                return;
            };
            let fd = slave.as_raw_fd();
            let mut echo = SystemTerminalEcho::for_fd(fd);
            let before = signal_handler(libc::SIGINT);

            echo.disable().expect("echo is suppressed on a terminal");
            let armed = signal_handler(libc::SIGINT);
            assert_ne!(
                armed, before,
                "the interruption handler replaced the prior one"
            );
            assert_ne!(
                armed,
                libc::SIG_DFL,
                "the interruption handler is installed"
            );

            echo.restore().expect("echo restoration succeeds");
            assert_eq!(
                signal_handler(libc::SIGINT),
                before,
                "the previous signal disposition is restored"
            );
        }

        #[test]
        fn system_echo_control_leaves_non_terminal_input_untouched() {
            let _guard = guard();
            let null = File::open("/dev/null").expect("/dev/null opens");
            let mut echo = SystemTerminalEcho::for_fd(null.as_raw_fd());

            echo.disable().expect("a redirected input is not an error");
            assert!(!echo.is_suppressed(), "no terminal echo was suppressed");
            echo.restore()
                .expect("restoring a redirected input is not an error");
        }

        #[test]
        fn the_paste_reader_reads_a_real_terminal_without_leaving_echo_disabled() {
            let _guard = guard();
            let Some((mut master, slave)) = open_pty() else {
                return;
            };
            let probe = slave.try_clone().expect("the terminal descriptor clones");
            let fd = probe.as_raw_fd();
            let reader = NoEchoReader::new(BufReader::new(slave), SystemTerminalEcho::for_fd(fd));
            let mut receiver = ManualPasteCallbackReceiver::new(CALLBACK, reader)
                .expect("the registered callback validates");

            master
                .write_all(b"ghostlink://oauth/callback?state=safe-state&code=secret-code\n")
                .expect("the pasted line reaches the terminal");
            let callback = receiver.receive().expect("the pasted callback parses");
            assert!(matches!(callback.result, CallbackResult::Code(_)));
            assert!(
                echo_enabled(fd),
                "echo is restored once the pasted line is complete"
            );
        }

        /// The child half of the process-level signal test.
        ///
        /// It does nothing during an ordinary suite run. When the parent
        /// re-executes this binary with [`CHILD_ENV`] set, it takes over the
        /// inherited terminal descriptor, suppresses echo, tells the parent it is
        /// ready, waits for the go-ahead, and then raises a real `SIGINT` on
        /// itself. The installed handler must restore echo and re-raise, so the
        /// child dies by that signal with the terminal left usable.
        #[test]
        fn signal_child_suppresses_echo_then_raises_a_real_interrupt() {
            let Some(_child) = std::env::var_os(CHILD_ENV) else {
                return;
            };
            let fd_text =
                std::env::var_os(CHILD_FD_ENV).expect("the parent passes the inherited descriptor");
            let fd: RawFd = fd_text
                .to_string_lossy()
                .parse()
                .expect("the parent passes a descriptor number");
            let mut echo = SystemTerminalEcho::for_fd(fd);
            echo.disable()
                .expect("echo is suppressed on the inherited terminal");

            println!("{CHILD_READY}");
            std::io::stdout()
                .flush()
                .expect("the readiness line flushes");

            // Block until the parent has observed the suppressed terminal, so the
            // signal cannot arrive before the state under test exists.
            let mut go = [0u8; 1];
            std::io::stdin()
                .read_exact(&mut go)
                .expect("the parent sends the go-ahead");

            // SAFETY: raising a signal on this process is the entire point of the
            // child role; the installed handler restores echo and re-raises, so the
            // control flow never continues past this call.
            unsafe {
                libc::raise(libc::SIGINT);
            }
            unreachable!("the re-raised signal terminates the child process");
        }

        /// Proves the signal path end to end against a real process and signal.
        ///
        /// The in-process tests above exercise the restoration body directly but
        /// never deliver a signal, so they cannot show that the installed handler
        /// runs, restores echo, and re-raises. This test forks a fresh test process
        /// over a shared pseudo-terminal: the child suppresses echo, the parent
        /// confirms the terminal is silenced, the child raises `SIGINT` on itself,
        /// and the parent then observes a signal-terminated child with echo back on
        /// the very same terminal. That is exactly the abandoned-`Ctrl-C` path a
        /// human takes out of a pending login.
        #[test]
        fn a_real_interrupt_restores_echo_before_the_process_exits() {
            let _guard = guard();
            let Some((_master, slave)) = open_pty() else {
                return;
            };
            let fd = slave.as_raw_fd();
            assert!(echo_enabled(fd), "a fresh pseudo-terminal echoes input");

            let exe = std::env::current_exe().expect("the test binary has a path");
            let mut child = Command::new(exe)
                .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
                .env(CHILD_ENV, "1")
                .env(CHILD_FD_ENV, fd.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("the child test process starts");

            let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
            let mut line = String::new();
            let mut ready = false;
            for _ in 0..64 {
                line.clear();
                if stdout
                    .read_line(&mut line)
                    .expect("the child's output is readable")
                    == 0
                {
                    break;
                }
                if line.contains(CHILD_READY) {
                    ready = true;
                    break;
                }
            }
            assert!(ready, "the child reached the suppressed-echo state: {line}");
            assert!(
                !echo_enabled(fd),
                "echo is suppressed on the shared terminal before the signal"
            );

            child
                .stdin
                .take()
                .expect("piped stdin")
                .write_all(b"!")
                .expect("the go-ahead reaches the child");

            let status = child.wait().expect("the child is reaped");
            assert_eq!(
                status.signal(),
                Some(libc::SIGINT),
                "the handler re-raised the delivered signal"
            );
            assert!(
                echo_enabled(fd),
                "the handler restored echo before the process exited"
            );
        }

        /// Proves the process-wide signal state is exclusive across overlapping attempts.
        ///
        /// The armed suppression replaces two signal dispositions and publishes one
        /// restoration pointer, and neither can be held twice at once. A second
        /// overlapping attempt must fail closed before touching either, so the
        /// owner's terminal, its installed handler, and its restoration pointer all
        /// survive the overlap, and the declined attempt can still succeed once the
        /// owner has restored and released.
        ///
        /// The overlap is deterministic rather than raced: the second attempt is made
        /// only after the first one has been observed to be armed.
        #[test]
        fn overlapping_attempts_cannot_share_the_process_wide_signal_state() {
            let _guard = guard();
            let Some((_owner_master, owner_slave)) = open_pty() else {
                return;
            };
            let Some((_contender_master, contender_slave)) = open_pty() else {
                return;
            };
            // Two different terminals prove the exclusion is process-wide rather
            // than tied to one descriptor.
            let owner_fd = owner_slave.as_raw_fd();
            let contender_fd = contender_slave.as_raw_fd();
            let original = signal_handler(libc::SIGINT);

            let mut owner = SystemTerminalEcho::for_fd(owner_fd);
            owner
                .disable()
                .expect("the first attempt owns the terminal");
            // A repeated request from the same owner stays idempotent and does not
            // disturb the single claim.
            owner
                .disable()
                .expect("a repeated disable by the owner is idempotent");
            assert!(!echo_enabled(owner_fd));
            let armed = signal_handler(libc::SIGINT);
            assert_ne!(armed, original, "the owner installed its own handler");

            let mut contender = SystemTerminalEcho::for_fd(contender_fd);
            assert!(
                contender.disable().is_err(),
                "the overlapping attempt fails closed"
            );
            // The declined attempt changed nothing: its own terminal still echoes,
            // the owner's handler is still installed, and the owner's restoration
            // pointer was not overwritten.
            assert!(echo_enabled(contender_fd));
            assert_eq!(signal_handler(libc::SIGINT), armed);
            terminal::restore_armed_echo();
            assert!(
                echo_enabled(owner_fd),
                "the armed restoration still targets the owner's terminal"
            );

            owner.restore().expect("the owner restores its terminal");
            assert!(echo_enabled(owner_fd));
            assert_eq!(
                signal_handler(libc::SIGINT),
                original,
                "the owner's release puts the original disposition back"
            );

            // Releasing the claim makes the process-wide state available again.
            contender
                .disable()
                .expect("a released claim is available to the next attempt");
            assert!(!echo_enabled(contender_fd));
            contender
                .restore()
                .expect("the later attempt restores cleanly");
            assert!(echo_enabled(contender_fd));
        }

        /// Proves a declined overlapping attempt cannot damage the owner's armed state.
        ///
        /// The failed attempt must not clear or replace the restoration pointer, so
        /// the owner's terminal stays the one a terminating signal would repair. This
        /// is checked by running the exact restoration body the handler runs.
        #[test]
        fn a_declined_attempt_cannot_clobber_the_armed_restoration() {
            let _guard = guard();
            let Some((_owner_master, owner_slave)) = open_pty() else {
                return;
            };
            let Some((_contender_master, contender_slave)) = open_pty() else {
                return;
            };
            let owner_fd = owner_slave.as_raw_fd();
            let contender_fd = contender_slave.as_raw_fd();
            let mut owner = SystemTerminalEcho::for_fd(owner_fd);
            owner
                .disable()
                .expect("the first attempt owns the terminal");

            let mut contender = SystemTerminalEcho::for_fd(contender_fd);
            assert!(contender.disable().is_err());
            // The failed attempt left the armed restoration pointing at the owner.
            terminal::restore_armed_echo();
            assert!(
                echo_enabled(owner_fd),
                "the restoration pointer still targets the owner"
            );
            assert!(
                echo_enabled(contender_fd),
                "the declined terminal was never silenced"
            );
            owner.restore().expect("the owner still restores cleanly");
        }

        /// Proves a second paste attempt over the real receiver fails closed.
        ///
        /// The receiver's echo control is the same `SystemTerminalEcho`, so an
        /// overlapping attempt fails before `NoEchoReader` can consume a byte: the
        /// pasted line stays in the terminal, and the same receiver succeeds
        /// unchanged once the first attempt has restored and released.
        #[test]
        fn a_second_paste_attempt_fails_closed_and_consumes_nothing_until_release() {
            let _guard = guard();
            let Some((_owner_master, owner_slave)) = open_pty() else {
                return;
            };
            let Some((mut contender_master, contender_slave)) = open_pty() else {
                return;
            };
            let owner_fd = owner_slave.as_raw_fd();
            let contender_fd = contender_slave.as_raw_fd();
            let mut owner = SystemTerminalEcho::for_fd(owner_fd);
            owner
                .disable()
                .expect("the first attempt owns the terminal");

            let reader = NoEchoReader::new(
                BufReader::new(contender_slave),
                SystemTerminalEcho::for_fd(contender_fd),
            );
            let mut receiver = ManualPasteCallbackReceiver::new(CALLBACK, reader)
                .expect("the registered callback validates");
            contender_master
                .write_all(b"ghostlink://oauth/callback?state=safe-state&code=secret-code\n")
                .expect("the pasted line reaches the terminal");

            assert!(
                receiver.receive().is_err(),
                "the overlapping paste attempt fails closed"
            );

            owner
                .restore()
                .expect("the owner releases the signal state");
            let callback = receiver
                .receive()
                .expect("the pending line is read once the claim is released");
            assert!(matches!(callback.result, CallbackResult::Code(_)));
            assert!(
                echo_enabled(contender_fd),
                "echo is restored once the retried read completes"
            );
        }
    }
}
