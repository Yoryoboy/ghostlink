//! Offline-safe public-client OAuth browser and callback adapters.

use destiny_core::ports::{AuthorizationRequest, BrowserAuthorization, CallbackReceiver};
use destiny_core::{
    AppError, AuthorizationCallback, AuthorizationCode, AuthorizationState, CallbackResult,
    ErrorCategory,
};
use std::io::{self, BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

const MAX_REQUEST_BYTES: usize = 8 * 1024;
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const AUTHORIZE_ENDPOINT: &str = "https://www.bungie.net/en/oauth/authorize";

/// The exact custom-scheme components of the registered product callback.
///
/// The provider rejects an HTTP loopback redirect for this registered public
/// client, so the product callback is a custom-scheme URI. The manual-paste flow
/// uses these exact components for the authorization redirect and for validating
/// what the human pastes back.
const PRODUCT_CALLBACK_SCHEME: &str = "ghostlink";
const PRODUCT_CALLBACK_HOST: &str = "oauth";
const PRODUCT_CALLBACK_PATH: &str = "/callback";

/// The largest pasted callback line this receiver accepts, in bytes.
///
/// A real callback URI is a few hundred bytes; the cap is a defensive bound so a
/// runaway or hostile terminal input can never make the process buffer without
/// limit before validation rejects it.
const MAX_CALLBACK_LINE_BYTES: usize = 8 * 1024;
fn response_body(success: bool) -> &'static [u8] {
    if success {
        b"<html><body>Authorization complete. You may close this window.</body></html>".as_slice()
    } else {
        b"<html><body>Authorization failed. You may close this window.</body></html>".as_slice()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CallbackEndpoint {
    host: String,
    port: u16,
    path: String,
}

impl CallbackEndpoint {
    fn parse(value: &str) -> Result<Self, AppError> {
        let url = Url::parse(value).map_err(|_| AppError::configuration())?;
        let host = url
            .host_str()
            .unwrap_or_default()
            .trim_matches(['[', ']'])
            .to_owned();
        if url.scheme() != "http"
            || url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1")
            || url.port().is_none_or(|port| port == 0)
            || url.path().is_empty()
        {
            return Err(AppError::configuration());
        }
        Ok(Self {
            host,
            port: url.port().unwrap_or_default(),
            path: url.path().to_owned(),
        })
    }
}

/// A one-shot receiver bound to the configured loopback host and port.
pub struct LoopbackCallbackReceiver {
    listeners: Vec<TcpListener>,
    endpoint: CallbackEndpoint,
    accept_timeout: Duration,
    read_timeout: Duration,
}

impl LoopbackCallbackReceiver {
    pub fn bind(callback_uri: &str) -> Result<Self, AppError> {
        let endpoint = CallbackEndpoint::parse(callback_uri)?;
        let addresses: Vec<&str> = match endpoint.host.as_str() {
            "localhost" => vec!["127.0.0.1", "::1"],
            "127.0.0.1" => vec!["127.0.0.1"],
            "::1" => vec!["::1"],
            _ => return Err(AppError::configuration()),
        };
        let mut listeners = Vec::new();
        let mut port = endpoint.port;
        for address in addresses {
            if let Ok(listener) = TcpListener::bind((address, port)) {
                listener
                    .set_nonblocking(true)
                    .map_err(|_| AppError::configuration())?;
                if port == 0 {
                    port = listener
                        .local_addr()
                        .map_err(|_| AppError::configuration())?
                        .port();
                }
                listeners.push(listener);
            }
        }
        if listeners.is_empty() {
            return Err(AppError::configuration());
        }
        if endpoint.port == 0 {
            let actual_port = listeners
                .first()
                .and_then(|listener| listener.local_addr().ok())
                .map(|address| address.port())
                .ok_or_else(AppError::configuration)?;
            port = actual_port;
        }
        Self::from_bound_listeners(CallbackEndpoint { port, ..endpoint }, listeners)
    }

    fn from_bound_listeners(
        endpoint: CallbackEndpoint,
        listeners: Vec<TcpListener>,
    ) -> Result<Self, AppError> {
        for listener in &listeners {
            listener
                .set_nonblocking(true)
                .map_err(|_| AppError::configuration())?;
        }
        if listeners.is_empty() {
            return Err(AppError::configuration());
        }
        Ok(Self {
            listeners,
            endpoint,
            accept_timeout: ACCEPT_TIMEOUT,
            read_timeout: READ_TIMEOUT,
        })
    }

    #[cfg(test)]
    fn with_timeouts(mut self, accept_timeout: Duration, read_timeout: Duration) -> Self {
        self.accept_timeout = accept_timeout;
        self.read_timeout = read_timeout;
        self
    }

    pub fn local_port(&self) -> Result<u16, AppError> {
        self.listeners
            .first()
            .and_then(|listener| listener.local_addr().ok())
            .map(|address| address.port())
            .ok_or_else(AppError::configuration)
    }

    fn respond(stream: &mut TcpStream, success: bool) {
        let body = response_body(success);
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(body);
    }
}

impl CallbackReceiver for LoopbackCallbackReceiver {
    fn receive(&mut self) -> Result<AuthorizationCallback, AppError> {
        let deadline = Instant::now() + self.accept_timeout;
        let mut stream = 'accept_loop: loop {
            for listener in &self.listeners {
                match listener.accept() {
                    Ok((stream, _)) => break 'accept_loop stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(_) => return Err(AppError::configuration()),
                }
            }
            if Instant::now() >= deadline {
                return Err(AppError::new(ErrorCategory::CallbackTimeout));
            }
            thread::sleep(Duration::from_millis(10));
        };
        let read_deadline = Instant::now() + self.read_timeout;
        let mut request = Vec::with_capacity(1024);
        let mut buffer = [0u8; 1024];
        loop {
            let remaining = read_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                Self::respond(&mut stream, false);
                return Err(AppError::new(ErrorCategory::CallbackTimeout));
            }
            stream.set_read_timeout(Some(remaining)).map_err(|error| {
                if error.kind() == std::io::ErrorKind::TimedOut {
                    AppError::new(ErrorCategory::CallbackTimeout)
                } else {
                    AppError::configuration()
                }
            })?;
            let read = match stream.read(&mut buffer) {
                Ok(read) => read,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    Self::respond(&mut stream, false);
                    return Err(AppError::new(ErrorCategory::CallbackTimeout));
                }
                Err(_) => {
                    Self::respond(&mut stream, false);
                    return Err(AppError::configuration());
                }
            };
            if read == 0 {
                break;
            }
            request.extend(buffer.iter().take(read).copied());
            if request.len() > MAX_REQUEST_BYTES {
                Self::respond(&mut stream, false);
                return Err(AppError::new(ErrorCategory::UnsupportedOperation));
            }
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let parsed = parse_request(&request, &self.endpoint.path);
        match parsed {
            Ok((state, result)) => {
                Self::respond(&mut stream, matches!(result, CallbackResult::Code(_)));
                Ok(AuthorizationCallback { state, result })
            }
            Err(error) => {
                Self::respond(&mut stream, false);
                Err(error)
            }
        }
    }
}

fn parse_request(
    request: &[u8],
    expected_path: &str,
) -> Result<(AuthorizationState, CallbackResult), AppError> {
    let text = std::str::from_utf8(request).map_err(|_| AppError::configuration())?;
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(AppError::configuration)?;
    let headers = &text[..header_end];
    let mut lines = headers.split("\r\n");
    let line = lines.next().ok_or_else(AppError::configuration)?;
    if lines.any(|header| header.is_empty() || !header.contains(':')) {
        return Err(AppError::configuration());
    }
    let parts: Vec<_> = line.split(' ').collect();
    if parts.len() != 3 || parts[0] != "GET" || parts[2] != "HTTP/1.1" {
        return Err(AppError::new(ErrorCategory::UnsupportedOperation));
    }
    let target = parts[1];
    if !target.starts_with('/')
        || target.starts_with("//")
        || target.contains('#')
        || target.to_ascii_lowercase().contains("%23")
    {
        return Err(AppError::new(ErrorCategory::UnsupportedOperation));
    }
    let url =
        Url::parse(&format!("http://loopback{target}")).map_err(|_| AppError::configuration())?;
    if url.path() != expected_path {
        return Err(AppError::new(ErrorCategory::UnsupportedOperation));
    }
    let mut state = None;
    let mut code = None;
    let mut error = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "state" if state.is_none() => state = Some(value.into_owned()),
            "code" if code.is_none() => code = Some(value.into_owned()),
            "error" if error.is_none() => error = Some(value.into_owned()),
            "state" | "code" | "error" => return Err(AppError::configuration()),
            _ => {}
        }
    }
    if code.is_some() && error.is_some() {
        return Err(AppError::configuration());
    }
    let state = AuthorizationState::from_bytes(
        state.ok_or_else(|| AppError::new(ErrorCategory::CsrfStateMismatch))?,
    );
    let result = match error.as_deref() {
        Some("cancelled") | Some("canceled") => CallbackResult::Cancelled,
        Some(_) => CallbackResult::Denied,
        None => code
            .map(|value| CallbackResult::Code(AuthorizationCode::from_bytes(value)))
            .ok_or_else(|| AppError::new(ErrorCategory::AuthorizationDenied))?,
    };
    Ok((state, result))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub trait CommandLauncher {
    fn launch(&mut self, program: &str, args: &[&str]) -> Result<(), AppError>;
}

#[derive(Default)]
pub struct SystemCommandLauncher;

impl CommandLauncher for SystemCommandLauncher {
    fn launch(&mut self, program: &str, args: &[&str]) -> Result<(), AppError> {
        Command::new(program)
            .args(args)
            .status()
            .map_err(|_| AppError::new(ErrorCategory::ProviderUnavailable))
            .and_then(|status| {
                if status.success() {
                    Ok(())
                } else {
                    Err(AppError::new(ErrorCategory::ProviderUnavailable))
                }
            })
    }
}

pub struct DesktopBrowserAuthorization<L> {
    client_id: String,
    callback_uri: String,
    launcher: L,
}

impl<L> DesktopBrowserAuthorization<L> {
    pub fn new(client_id: impl Into<String>, callback_uri: impl Into<String>, launcher: L) -> Self {
        Self {
            client_id: client_id.into(),
            callback_uri: callback_uri.into(),
            launcher,
        }
    }
    pub fn into_launcher(self) -> L {
        self.launcher
    }
}

impl<L: CommandLauncher> BrowserAuthorization for DesktopBrowserAuthorization<L> {
    fn open(&mut self, request: AuthorizationRequest) -> Result<(), AppError> {
        let url = authorization_url(&self.client_id, &self.callback_uri, &request.state)?;
        let (program, args) = browser_command(std::env::consts::OS, url.as_str());
        self.launcher.launch(program, &args)
    }
}

pub fn authorization_url(
    client_id: &str,
    callback_uri: &str,
    state: &AuthorizationState,
) -> Result<Url, AppError> {
    // The authorization redirect is the exact callback the provider has
    // registered. Both the baseline HTTP loopback callback and the product
    // custom-scheme callback are accepted here; nothing else is.
    if CallbackEndpoint::parse(callback_uri).is_err()
        && ProductCallback::parse(callback_uri).is_err()
    {
        return Err(AppError::configuration());
    }
    let mut url = Url::parse(AUTHORIZE_ENDPOINT).map_err(|_| AppError::configuration())?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", callback_uri)
        .append_pair(
            "state",
            std::str::from_utf8(state.as_bytes()).map_err(|_| AppError::configuration())?,
        );
    Ok(url)
}

pub fn browser_command<'a>(os: &str, url: &'a str) -> (&'static str, Vec<&'a str>) {
    match os {
        "windows" => ("explorer.exe", vec![url]),
        "macos" => ("open", vec![url]),
        _ => ("xdg-open", vec![url]),
    }
}

/// A rejected pasted callback.
///
/// The pasted value is a short-lived authorization code carried in a URI. Every
/// failure of the manual-paste path reports this one fixed category, so no
/// supplied value, code, or state can reach an error message.
fn callback_rejected() -> AppError {
    AppError::new(ErrorCategory::CsrfStateMismatch)
}

/// A validated product custom-scheme callback.
///
/// The registered redirect is `ghostlink://oauth/callback`. The value is not a
/// secret: it is rendered into the authorization URL and compared against the
/// URI the human pastes back, so the exact scheme, authority, and path are
/// required and every other component is refused.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ProductCallback {
    canonical: String,
}

impl ProductCallback {
    fn parse(value: &str) -> Result<Self, AppError> {
        if value.is_empty() || value.trim() != value {
            return Err(AppError::configuration());
        }
        let url = Url::parse(value).map_err(|_| AppError::configuration())?;
        if url.scheme() != PRODUCT_CALLBACK_SCHEME
            || url.username() != ""
            || url.password().is_some()
            || url.host_str() != Some(PRODUCT_CALLBACK_HOST)
            || url.port().is_some()
            || url.path() != PRODUCT_CALLBACK_PATH
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(AppError::configuration());
        }
        Ok(Self {
            canonical: url.as_str().to_owned(),
        })
    }

    fn canonical(&self) -> &str {
        &self.canonical
    }
}

/// The authorization URL one manual-paste attempt recorded for the operator.
///
/// It carries only the authorization URL the human must open. The URL contains
/// the CSRF state, which the provider sends to the browser anyway; no credential
/// or code is part of it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualAuthorizationInstruction {
    pub authorization_url: Url,
}

/// Records the authorization URL and optionally opens the system browser.
///
/// The manual-paste flow always prints the authorization URL, because the human
/// has to copy the custom-scheme callback back into the terminal even when the
/// browser opens. The optional launch is the only difference between the browser
/// and headless modes.
pub struct ManualPasteBrowserAuthorization<L> {
    client_id: String,
    callback_uri: String,
    launcher: L,
    open_browser: bool,
    instruction: Option<ManualAuthorizationInstruction>,
}

impl<L> ManualPasteBrowserAuthorization<L> {
    pub fn new(
        client_id: impl Into<String>,
        callback_uri: impl Into<String>,
        launcher: L,
        open_browser: bool,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            callback_uri: callback_uri.into(),
            launcher,
            open_browser,
            instruction: None,
        }
    }

    pub fn instruction(&self) -> Option<&ManualAuthorizationInstruction> {
        self.instruction.as_ref()
    }
}

impl<L: CommandLauncher> BrowserAuthorization for ManualPasteBrowserAuthorization<L> {
    fn open(&mut self, request: AuthorizationRequest) -> Result<(), AppError> {
        let url = authorization_url(&self.client_id, &self.callback_uri, &request.state)?;
        self.instruction = Some(ManualAuthorizationInstruction {
            authorization_url: url.clone(),
        });
        if self.open_browser {
            let (program, args) = browser_command(std::env::consts::OS, url.as_str());
            self.launcher.launch(program, &args)?;
        }
        Ok(())
    }
}

/// Controls whether a terminal echoes the bytes typed into it.
///
/// The paste receiver consumes a [`BufRead`], which cannot express terminal echo;
/// echo is a property of the device, not of the byte stream. It is therefore a
/// port: the adapter owns exactly when echo must be off, and the composition root
/// supplies the platform binding.
///
/// `disable` and `restore` must both be idempotent. An implementation that
/// disables a real terminal must also restore it when dropped, because the
/// adapter's drop is the safety net for the error and unwind paths it never
/// observes.
pub trait TerminalEcho {
    /// Stops the terminal from echoing typed input without changing line editing.
    fn disable(&mut self) -> Result<(), AppError>;
    /// Returns the terminal to the echo state it had before [`Self::disable`].
    fn restore(&mut self) -> Result<(), AppError>;
}

/// Wraps a line reader so a pasted secret is not echoed by the terminal.
///
/// Echo suppression is tied to this reader's [`BufRead`] calls, which is the only
/// seam the paste receiver consumes. Echo is disabled before the reader may block
/// for input, and restored as soon as the complete line is buffered, the input
/// ends, or the underlying read fails. [`TerminalEcho::disable`] is requested at
/// most once per pending read, so a caller that consumes one line never pays for
/// two terminal-mode changes.
///
/// A read that cannot disable echo fails closed: no byte is consumed, so a
/// terminal the process cannot silence never receives the authorization code in
/// the first place.
pub struct NoEchoReader<R, E: TerminalEcho> {
    reader: R,
    echo: E,
    suppressed: bool,
}

impl<R, E: TerminalEcho> NoEchoReader<R, E> {
    /// Pairs one line reader with one echo control.
    pub fn new(reader: R, echo: E) -> Self {
        Self {
            reader,
            echo,
            suppressed: false,
        }
    }

    /// Disables echo before the reader is allowed to block for input.
    fn suppress(&mut self) -> io::Result<()> {
        if self.suppressed {
            return Ok(());
        }
        self.echo.disable().map_err(io::Error::other)?;
        self.suppressed = true;
        Ok(())
    }

    /// Restores the terminal and clears the suppression state.
    ///
    /// The state is cleared only after the control confirms restoration, so a
    /// failed restore leaves the reader in the suppressed state and the final
    /// drop retries it instead of forgetting an unprotected terminal.
    fn release(&mut self) -> Result<(), AppError> {
        if !self.suppressed {
            return Ok(());
        }
        self.echo.restore()?;
        self.suppressed = false;
        Ok(())
    }
}

impl<R: BufRead, E: TerminalEcho> BufRead for NoEchoReader<R, E> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.suppress()?;
        let complete = match self.reader.fill_buf() {
            Ok(available) => available.is_empty() || available.contains(&b'\n'),
            Err(error) => {
                let _ = self.release();
                return Err(error);
            }
        };
        if complete {
            let _ = self.release();
        }
        self.reader.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.reader.consume(amount);
    }
}

impl<R: BufRead, E: TerminalEcho> Read for NoEchoReader<R, E> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let amount = {
            let available = self.fill_buf()?;
            let amount = available.len().min(buffer.len());
            buffer[..amount].copy_from_slice(&available[..amount]);
            amount
        };
        self.consume(amount);
        Ok(amount)
    }
}

impl<R, E: TerminalEcho> Drop for NoEchoReader<R, E> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

/// Reads one pasted callback URI and validates it against the registered one.
///
/// This is the production replacement for the baseline loopback listener on a
/// provider that rejects an HTTP loopback redirect. It reads exactly one bounded
/// line from the supplied reader, never renders what it read, and never places it
/// in an error. Because the value is read from standard input rather than an
/// argument, it never reaches shell history.
///
/// Terminal echo is a device concern this receiver cannot control: the reader it
/// is handed must itself suppress echo for the duration of the read. Production
/// composes it over [`NoEchoReader`], so a pasted authorization code is not
/// written back to the screen while the receiver validates it. Passing a plain
/// reader leaves echo untouched and is only appropriate for non-interactive input.
///
/// The receiver validates the callback shape and extracts the opaque state and
/// result; the shared authorization service still owns CSRF comparison, replay
/// rejection, and expiry. The receiver therefore never trusts the pasted state:
/// it only carries it into the service's cryptographic check.
pub struct ManualPasteCallbackReceiver<R> {
    callback: ProductCallback,
    reader: R,
    max_bytes: usize,
}

impl<R> ManualPasteCallbackReceiver<R> {
    /// Validates the registered callback and wraps one line reader.
    pub fn new(callback_uri: &str, reader: R) -> Result<Self, AppError> {
        Ok(Self {
            callback: ProductCallback::parse(callback_uri)?,
            reader,
            max_bytes: MAX_CALLBACK_LINE_BYTES,
        })
    }

    /// Lowers the accepted line bound for a focused test.
    #[cfg(test)]
    fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }
}

impl<R: BufRead> CallbackReceiver for ManualPasteCallbackReceiver<R> {
    fn receive(&mut self) -> Result<AuthorizationCallback, AppError> {
        let line = read_bounded_callback_line(&mut self.reader, self.max_bytes)?;
        parse_pasted_callback(&line, self.callback.canonical())
    }
}

/// Reads one line, bounded to `max_bytes` including the terminator.
///
/// The bound is enforced while reading, so an unterminated or hostile line can
/// never make the process allocate without limit. An empty read at end of input
/// returns an empty line, which the parser rejects.
fn read_bounded_callback_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Vec<u8>, AppError> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        let (consumed, complete) = {
            let available = reader.fill_buf().map_err(|_| callback_rejected())?;
            if available.is_empty() {
                break;
            }
            let (chunk, complete) = match available.iter().position(|byte| *byte == b'\n') {
                Some(index) => (&available[..=index], true),
                None => (available, false),
            };
            if line.len() + chunk.len() > max_bytes {
                return Err(callback_rejected());
            }
            line.extend_from_slice(chunk);
            (chunk.len(), complete)
        };
        reader.consume(consumed);
        if complete {
            break;
        }
    }
    while line.last().is_some_and(u8::is_ascii_whitespace) {
        line.pop();
    }
    let start = line
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(line.len());
    line.drain(..start);
    Ok(line)
}

/// Parses one pasted callback against the exact registered callback.
fn parse_pasted_callback(raw: &[u8], expected: &str) -> Result<AuthorizationCallback, AppError> {
    let text = std::str::from_utf8(raw).map_err(|_| callback_rejected())?;
    let url = Url::parse(text).map_err(|_| callback_rejected())?;
    if url.scheme() != PRODUCT_CALLBACK_SCHEME
        || url.username() != ""
        || url.password().is_some()
        || url.host_str() != Some(PRODUCT_CALLBACK_HOST)
        || url.port().is_some()
        || url.path() != PRODUCT_CALLBACK_PATH
        || url.fragment().is_some()
    {
        return Err(callback_rejected());
    }
    // The scheme, authority, and path must be exactly the registered callback.
    let mut base = url.clone();
    base.set_query(None);
    if base.as_str() != expected {
        return Err(callback_rejected());
    }
    let mut state = None;
    let mut code = None;
    let mut error = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "state" if state.is_none() => state = Some(value.into_owned()),
            "code" if code.is_none() => code = Some(value.into_owned()),
            "error" if error.is_none() => error = Some(value.into_owned()),
            "state" | "code" | "error" => return Err(callback_rejected()),
            _ => {}
        }
    }
    if code.is_some() && error.is_some() {
        return Err(callback_rejected());
    }
    let state = AuthorizationState::from_bytes(state.ok_or_else(callback_rejected)?.into_bytes());
    let result = match error.as_deref() {
        Some("cancelled") | Some("canceled") => CallbackResult::Cancelled,
        Some(_) => CallbackResult::Denied,
        None => code
            .map(|value| CallbackResult::Code(AuthorizationCode::from_bytes(value.into_bytes())))
            .ok_or_else(callback_rejected)?,
    };
    Ok(AuthorizationCallback { state, result })
}

// The Linux custom-scheme callback rendezvous.
//
// A registered custom-scheme callback is delivered by the operating system to a
// short-lived handler process, never to the waiting login. The two processes
// therefore need one bounded channel. The login binds one per-attempt
// Unix-domain socket inside a private directory under `XDG_RUNTIME_DIR` and
// publishes a pointer that names it; the installed desktop handler runs
// `ghostlink callback <uri>`, resolves the pointer, and writes the one callback
// URI to that socket.
//
// Threat model: other local users and accidental collisions, not a hostile
// process already running as this same user. The directory and files are created
// owner-only (`0700`/`0600`) under the per-user runtime directory, so no other
// local user can read the pointer or connect to the socket. The per-attempt
// socket name carries a fresh 128-bit random token, but that token is a
// uniqueness and anti-collision measure, not authentication: this user can read
// it through the pointer, and it does not prove that the handler reached the
// login that published it.
//
// The same-user limit is explicit and is not hidden. A process already running
// as this user can read the pointer, replace the `login.lock` path, or repoint
// `endpoint` at a socket it controls. The singleton, the pointer, and the
// best-effort cleanup below are all pathname-based, so same-user replacement can
// defeat them. The design does not claim resistance to a hostile same-user
// process; it defends exclusive use against other local users and against
// accidental concurrent logins by this user.
//
// Exactly one login attempts to own the rendezvous at a time. A fixed lock file
// inside the directory is held for the whole attempt, so a second concurrent
// login fails closed instead of overwriting the first attempt's pointer and
// stealing the callback. The lock is an operating-system file lock, so it is
// released automatically when the owning process exits, however it exits.
//
// The pointer is a symlink whose target is the per-attempt socket name, not a
// regular file of bytes. The handler resolves it with `readlink`, which never
// opens the target, so a prepared FIFO or symlink left in the directory cannot
// make the resolution block while the handler holds the one-use code in its
// argv. The name is still validated against the exact socket-name shape after
// resolution, so a pointer that names anything but a per-attempt socket is
// refused before it is ever connected to.
//
// The receiver validates only the callback shape. The shared authorization
// service still owns the cryptographic state comparison, replay rejection, and
// expiry, so a forwarded callback cannot authorize anything without the pending
// random state, even if some process reaches the socket.
//
// Two exposures are documented rather than hidden. A desktop `%u` argument
// necessarily carries the one-use authorization code through the handler's
// process argv for the moment it forwards it. And because this is a
// same-user-scoped design, a hostile process already running as this user could
// receive or redirect that callback; such a process is outside the threat model
// and is not defended against. Both are inherent to desktop custom-scheme
// handling and to the trusted-same-user scoping; this product only bounds the
// argv window, validates the URI shape, and refuses to log, print, or persist
// the code.

/// The per-user runtime directory that holds the callback rendezvous.
#[cfg(target_os = "linux")]
const RENDEZVOUS_DIR_NAME: &str = "ghostlink";
/// The fixed pointer that names the current attempt's socket.
///
/// It is created as a symlink to the per-attempt socket name; the handler reads
/// the link target with `readlink` and never opens the pointer as a file.
#[cfg(target_os = "linux")]
const RENDEZVOUS_ENDPOINT_FILE: &str = "endpoint";
/// The fixed lock file that admits one login per user at a time.
#[cfg(target_os = "linux")]
const RENDEZVOUS_LOCK_FILE: &str = "login.lock";
/// The per-attempt socket name prefix.
#[cfg(target_os = "linux")]
const RENDEZVOUS_SOCKET_PREFIX: &str = "callback-";
/// The per-attempt socket name suffix.
#[cfg(target_os = "linux")]
const RENDEZVOUS_SOCKET_SUFFIX: &str = ".sock";
/// Random bytes behind one per-attempt socket name (128 bits, 32 hex chars).
#[cfg(target_os = "linux")]
const RENDEZVOUS_TOKEN_BYTES: usize = 16;
/// The owner-only directory mode.
#[cfg(target_os = "linux")]
const RENDEZVOUS_DIR_MODE: u32 = 0o700;
/// The owner-only file mode for the pointer and the socket.
#[cfg(target_os = "linux")]
const RENDEZVOUS_FILE_MODE: u32 = 0o600;
/// The bounded window one login waits for a forwarded callback.
#[cfg(target_os = "linux")]
const RENDEZVOUS_WAIT: Duration = Duration::from_secs(300);
/// The bounded connect and write window for one forwarding handler.
#[cfg(target_os = "linux")]
const RENDEZVOUS_HANDLER_TIMEOUT: Duration = Duration::from_secs(5);

/// Encodes bytes as lowercase hexadecimal.
#[cfg(target_os = "linux")]
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// Resolves the private rendezvous directory under the per-user runtime dir.
///
/// `XDG_RUNTIME_DIR` is the operating-system per-user runtime directory; it is
/// required to be absolute and is created and consumed owner-only. When it is
/// absent this product does not fall back to a world-writable temporary
/// directory: the handler path is unavailable and the terminal paste remains.
#[cfg(target_os = "linux")]
fn callback_rendezvous_dir() -> Result<std::path::PathBuf, AppError> {
    let base = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(AppError::configuration)?;
    let base = std::path::PathBuf::from(base);
    if !base.is_absolute() {
        return Err(AppError::configuration());
    }
    Ok(base.join(RENDEZVOUS_DIR_NAME))
}

/// Creates the rendezvous directory owner-only, or refuses an unsafe one.
#[cfg(target_os = "linux")]
fn ensure_private_dir(path: &std::path::Path) -> Result<(), AppError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(RENDEZVOUS_DIR_MODE);
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(AppError::configuration()),
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| AppError::configuration())?;
    if !metadata.file_type().is_dir()
        || metadata.permissions().mode() & 0o777 != RENDEZVOUS_DIR_MODE
    {
        return Err(AppError::configuration());
    }
    Ok(())
}

/// Restricts one rendezvous file to its owner.
#[cfg(target_os = "linux")]
fn set_private_file_mode(path: &std::path::Path) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(RENDEZVOUS_FILE_MODE))
        .map_err(|_| AppError::configuration())
}

/// Claims the single per-user login slot for the whole rendezvous lifetime.
///
/// The claim is an operating-system file lock on a fixed file in the private
/// rendezvous directory. It is held for as long as the returned handle lives and
/// is released by the kernel when the owning process exits, however it exits, so
/// a killed or crashed login never leaves a stale lock behind that would block
/// the next attempt forever. A second concurrent login fails closed here — with
/// the "already authorizing" outcome — before it binds a socket or publishes a
/// pointer, so it can neither overwrite the first attempt's pointer nor steal
/// its callback. The lock file itself is never removed while locked: unlinking
/// it would let a new login create a fresh file at the same path and lock it,
/// admitting two logins at once. This guarantee is pathname-based and holds
/// within the trusted same-user model: a hostile process already running as this
/// user could replace the lock path and is not defended against (see the
/// rendezvous note above).
#[cfg(target_os = "linux")]
fn acquire_singleton(directory: &std::path::Path) -> Result<std::fs::File, AppError> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = directory.join(RENDEZVOUS_LOCK_FILE);
    if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(AppError::configuration());
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(RENDEZVOUS_FILE_MODE)
        .open(&path)
        .map_err(|_| AppError::configuration())?;
    match lock.try_lock() {
        Ok(()) => {
            set_private_file_mode(&path)?;
            Ok(lock)
        }
        Err(std::fs::TryLockError::WouldBlock) => {
            Err(AppError::new(ErrorCategory::AlreadyAuthorizing))
        }
        Err(std::fs::TryLockError::Error(_)) => Err(AppError::configuration()),
    }
}

/// Publishes the current attempt's socket name through the fixed pointer.
///
/// The pointer is a symlink whose target is the bare per-attempt socket name.
/// It is created under a private temporary name and renamed over the fixed name,
/// so the handler observes either the previous target or the whole new one and
/// never a partial value. Publishing a symlink is deliberate: the handler reads
/// it with `readlink`, which does not open the target and therefore cannot block
/// on a FIFO or follow a symlink that points off the rendezvous.
#[cfg(target_os = "linux")]
fn publish_endpoint(
    directory: &std::path::Path,
    socket_name: &str,
    token: &str,
) -> Result<(), AppError> {
    let temporary = directory.join(format!("{RENDEZVOUS_ENDPOINT_FILE}-{token}.tmp"));
    std::os::unix::fs::symlink(socket_name, &temporary).map_err(|_| AppError::configuration())?;
    std::fs::rename(&temporary, directory.join(RENDEZVOUS_ENDPOINT_FILE)).map_err(|_| {
        let _ = std::fs::remove_file(&temporary);
        AppError::configuration()
    })
}

/// Accepts exactly one randomly named per-attempt socket.
#[cfg(target_os = "linux")]
fn is_rendezvous_socket_name(name: &str) -> bool {
    let Some(token) = name
        .strip_prefix(RENDEZVOUS_SOCKET_PREFIX)
        .and_then(|rest| rest.strip_suffix(RENDEZVOUS_SOCKET_SUFFIX))
    else {
        return false;
    };
    token.len() == RENDEZVOUS_TOKEN_BYTES * 2
        && token
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Resolves the pointer to the current attempt's socket name.
///
/// The pointer is a symlink, so it is read with `readlink` instead of opened as
/// a file: a prepared FIFO or symlink-to-FIFO left in the directory can neither
/// make this call block nor redirect the read, which keeps the handler's argv
/// exposure to the one-use code as short as possible. A missing pointer means no
/// login is waiting; any other failure is a malformed or hostile pointer.
/// The resolved name must be exactly a per-attempt socket name, so a target that
/// points anywhere off the rendezvous is refused before it is ever connected to.
#[cfg(target_os = "linux")]
fn read_endpoint_name(directory: &std::path::Path) -> Result<String, AppError> {
    let target = std::fs::read_link(directory.join(RENDEZVOUS_ENDPOINT_FILE)).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AppError::new(ErrorCategory::CallbackTimeout)
        } else {
            AppError::configuration()
        }
    })?;
    let name = target
        .into_os_string()
        .into_string()
        .map_err(|_| AppError::configuration())?;
    if !is_rendezvous_socket_name(&name) {
        return Err(AppError::configuration());
    }
    Ok(name)
}

/// Validates that a handler-supplied value is exactly the product callback.
#[cfg(target_os = "linux")]
fn forwarded_callback_shape(value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.len() > MAX_CALLBACK_LINE_BYTES || value.trim() != value {
        return Err(callback_rejected());
    }
    let url = Url::parse(value).map_err(|_| callback_rejected())?;
    if url.scheme() != PRODUCT_CALLBACK_SCHEME
        || url.username() != ""
        || url.password().is_some()
        || url.host_str() != Some(PRODUCT_CALLBACK_HOST)
        || url.port().is_some()
        || url.path() != PRODUCT_CALLBACK_PATH
        || url.fragment().is_some()
    {
        return Err(callback_rejected());
    }
    Ok(())
}

/// Forwards one handler-supplied callback to a waiting login in `directory`.
///
/// The value is validated for shape and bounded before any file is read, then
/// written once to the socket named by the pointer. Nothing is rendered,
/// logged, or persisted; the only artifact is the transient socket write. The
/// connect is bounded (see [`connect_bounded`]), so the handler never waits
/// without limit while it carries the one-use code in its argv.
#[cfg(target_os = "linux")]
pub fn forward_callback_in(
    directory: &std::path::Path,
    callback_uri: &str,
) -> Result<(), AppError> {
    forward_callback_in_with_timeout(directory, callback_uri, RENDEZVOUS_HANDLER_TIMEOUT)
}

/// Forwards one handler-supplied callback with an explicit connect and write
/// bound.
#[cfg(target_os = "linux")]
fn forward_callback_in_with_timeout(
    directory: &std::path::Path,
    callback_uri: &str,
    timeout: Duration,
) -> Result<(), AppError> {
    forwarded_callback_shape(callback_uri)?;
    let name = read_endpoint_name(directory)?;
    let mut stream = connect_bounded(directory.join(name), timeout)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|_| AppError::configuration())?;
    stream
        .write_all(callback_uri.as_bytes())
        .and_then(|()| stream.write_all(b"\n"))
        .and_then(|()| stream.flush())
        .map_err(|_| AppError::new(ErrorCategory::CallbackTimeout))
}

/// Connects to one rendezvous socket within `timeout`.
///
/// A blocking Unix-domain connect is not bounded: when the listener's accept
/// queue is full the kernel parks the call with no deadline, so a handler that
/// held the one-use code in its argv would wait for as long as the endpoint
/// stayed unavailable. The connect therefore runs on a worker thread and the
/// handler waits only until `timeout`. A connect that has not finished by then
/// is abandoned and reported as a timeout; the handler is a short-lived process,
/// so an abandoned worker ends with it and the argv window closes there.
#[cfg(target_os = "linux")]
fn connect_bounded(
    path: std::path::PathBuf,
    timeout: Duration,
) -> Result<std::os::unix::net::UnixStream, AppError> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("ghostlink-callback-forward".to_owned())
        .spawn(move || {
            let _ = sender.send(std::os::unix::net::UnixStream::connect(path));
        })
        .map_err(|_| AppError::new(ErrorCategory::ProviderUnavailable))?;
    match receiver.recv_timeout(timeout) {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(_)) | Err(_) => Err(AppError::new(ErrorCategory::CallbackTimeout)),
    }
}

/// Forwards one handler-supplied callback to a waiting login.
///
/// This is the entry point the hidden `ghostlink callback` subcommand calls. It
/// resolves the per-user rendezvous directory from `XDG_RUNTIME_DIR`; when the
/// variable is absent or unsafe the forward fails closed and the terminal paste
/// remains the fallback.
#[cfg(target_os = "linux")]
pub fn forward_callback(callback_uri: &str) -> Result<(), AppError> {
    forward_callback_in(&callback_rendezvous_dir()?, callback_uri)
}

/// The login-side end of the Linux custom-scheme callback rendezvous.
///
/// It claims the single per-user login slot, binds one per-attempt socket, and
/// removes that socket, together with its pointer, when it is dropped. The
/// receiver owns no secret: it extracts the opaque state and result for the
/// shared service to check.
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct CallbackRendezvous {
    listener: std::os::unix::net::UnixListener,
    /// Holds the per-user login lock for the lifetime of this rendezvous.
    ///
    /// It is never read: its only job is to stay alive so the operating-system
    /// file lock is held, and to release it when this value drops. The leading
    /// underscore records that intent for the unused-field lint.
    _singleton: std::fs::File,
    canonical: String,
    directory: std::path::PathBuf,
    socket_path: std::path::PathBuf,
    socket_name: String,
    wait: Duration,
}

#[cfg(target_os = "linux")]
impl CallbackRendezvous {
    /// Binds a fresh per-attempt rendezvous under `XDG_RUNTIME_DIR`.
    pub fn bind(callback_uri: &str) -> Result<Self, AppError> {
        Self::bind_in(callback_rendezvous_dir()?, callback_uri)
    }

    /// Binds a fresh per-attempt rendezvous in an explicit directory.
    ///
    /// The directory is created owner-only if it does not exist and is refused
    /// when it exists with any group or other permission, so a prepared or
    /// hostile path never becomes a rendezvous. The per-user login slot is
    /// claimed first, so a second concurrent login fails closed here — before it
    /// binds a socket or publishes a pointer — rather than overwriting the first
    /// attempt's pointer and stealing its callback.
    pub fn bind_in(directory: std::path::PathBuf, callback_uri: &str) -> Result<Self, AppError> {
        let canonical = ProductCallback::parse(callback_uri)?.canonical().to_owned();
        ensure_private_dir(&directory)?;
        let singleton = acquire_singleton(&directory)?;
        let mut token_bytes = [0u8; RENDEZVOUS_TOKEN_BYTES];
        getrandom::fill(&mut token_bytes)
            .map_err(|_| AppError::new(ErrorCategory::LocalStorage))?;
        let token = hex_lower(token_bytes.as_slice());
        let socket_name = format!("{RENDEZVOUS_SOCKET_PREFIX}{token}{RENDEZVOUS_SOCKET_SUFFIX}");
        let socket_path = directory.join(&socket_name);
        let listener = std::os::unix::net::UnixListener::bind(&socket_path)
            .map_err(|_| AppError::configuration())?;
        let setup = (|| {
            listener
                .set_nonblocking(true)
                .map_err(|_| AppError::configuration())?;
            set_private_file_mode(&socket_path)?;
            publish_endpoint(&directory, &socket_name, &token)
        })();
        if let Err(error) = setup {
            let _ = std::fs::remove_file(&socket_path);
            return Err(error);
        }
        Ok(Self {
            listener,
            _singleton: singleton,
            canonical,
            directory,
            socket_path,
            socket_name,
            wait: RENDEZVOUS_WAIT,
        })
    }

    /// Lowers the wait bound for a focused test.
    #[cfg(test)]
    fn with_wait(mut self, wait: Duration) -> Self {
        self.wait = wait;
        self
    }

    /// Removes this attempt's pointer and socket.
    ///
    /// The pointer is removed only while it still names this attempt, so a
    /// later attempt's rendezvous is never deleted. The shared lock file is left
    /// in place on purpose: removing a file that is still locked would let a new
    /// login create a fresh file at the same path and lock it, running two logins
    /// at once. The operating-system lock itself, not the file's existence, is
    /// what admits exactly one login per user. The check and the unlink are
    /// separate pathname operations, so a same-user process could replace
    /// `endpoint` between them; within the trusted same-user model that race is
    /// out of scope and the removal stays best-effort.
    pub fn cleanup(&self) {
        let endpoint = self.directory.join(RENDEZVOUS_ENDPOINT_FILE);
        if std::fs::read_link(&endpoint)
            .is_ok_and(|target| target == std::path::Path::new(&self.socket_name))
        {
            let _ = std::fs::remove_file(&endpoint);
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[cfg(target_os = "linux")]
impl Drop for CallbackRendezvous {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Reads one bounded payload from an accepted forwarding connection.
#[cfg(target_os = "linux")]
fn read_forwarded_payload(
    stream: &mut std::os::unix::net::UnixStream,
    deadline: Instant,
) -> Result<Vec<u8>, AppError> {
    let mut payload: Vec<u8> = Vec::with_capacity(256);
    let mut buffer = [0u8; 1024];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AppError::new(ErrorCategory::CallbackTimeout));
        }
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|_| AppError::configuration())?;
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if payload.len() + read > MAX_CALLBACK_LINE_BYTES {
                    return Err(callback_rejected());
                }
                let chunk: &[u8] = &buffer[..read];
                let complete = chunk.contains(&b'\n');
                payload.extend_from_slice(chunk);
                if complete {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(AppError::new(ErrorCategory::CallbackTimeout));
            }
            Err(_) => return Err(callback_rejected()),
        }
    }
    Ok(payload)
}

#[cfg(target_os = "linux")]
impl CallbackReceiver for CallbackRendezvous {
    fn receive(&mut self) -> Result<AuthorizationCallback, AppError> {
        let deadline = Instant::now() + self.wait;
        let mut stream = loop {
            match self.listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(AppError::configuration()),
            }
            if Instant::now() >= deadline {
                return Err(AppError::new(ErrorCategory::CallbackTimeout));
            }
            thread::sleep(Duration::from_millis(10));
        };
        let payload = read_forwarded_payload(&mut stream, deadline)?;
        parse_pasted_callback(&payload, &self.canonical)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshTarget {
    destination: String,
    port: u16,
}

impl SshTarget {
    pub fn new(destination: impl Into<String>, port: u16) -> Result<Self, AppError> {
        let destination = destination.into();
        if port == 0
            || destination.is_empty()
            || destination.starts_with('-')
            || !destination.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'_' | b'-' | b'@' | b':' | b'[' | b']')
            })
        {
            return Err(AppError::configuration());
        }
        Ok(Self { destination, port })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeadlessAuthorizationInstruction {
    pub authorization_url: Url,
    pub ssh_local_forward: String,
}

pub struct HeadlessBrowserAuthorization {
    client_id: String,
    callback_uri: String,
    ssh_target: Option<SshTarget>,
    instruction: Option<HeadlessAuthorizationInstruction>,
}

impl HeadlessBrowserAuthorization {
    pub fn new(
        client_id: impl Into<String>,
        callback_uri: impl Into<String>,
        ssh_target: Option<SshTarget>,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            callback_uri: callback_uri.into(),
            ssh_target,
            instruction: None,
        }
    }
    pub fn instruction(&self) -> Option<&HeadlessAuthorizationInstruction> {
        self.instruction.as_ref()
    }
    pub fn take_instruction(&mut self) -> Option<HeadlessAuthorizationInstruction> {
        self.instruction.take()
    }
    fn target(&self) -> Result<SshTarget, AppError> {
        if let Some(target) = &self.ssh_target {
            return Ok(target.clone());
        }
        let connection = std::env::var("SSH_CONNECTION").map_err(|_| AppError::configuration())?;
        let user = std::env::var("USER").map_err(|_| AppError::configuration())?;
        inferred_ssh_target(&connection, &user)
    }
}

fn inferred_ssh_target(connection: &str, user: &str) -> Result<SshTarget, AppError> {
    let fields: Vec<_> = connection.split_whitespace().collect();
    if fields.len() != 4
        || user.is_empty()
        || !user
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(AppError::configuration());
    }
    let server = fields[2]
        .parse::<std::net::IpAddr>()
        .map_err(|_| AppError::configuration())?;
    let port = fields[3].parse().map_err(|_| AppError::configuration())?;
    let destination = match server {
        std::net::IpAddr::V4(address) => format!("{user}@{address}"),
        std::net::IpAddr::V6(address) => format!("{user}@[{address}]"),
    };
    SshTarget::new(destination, port)
}

impl BrowserAuthorization for HeadlessBrowserAuthorization {
    fn open(&mut self, request: AuthorizationRequest) -> Result<(), AppError> {
        let endpoint = CallbackEndpoint::parse(&self.callback_uri)?;
        let url = authorization_url(&self.client_id, &self.callback_uri, &request.state)?;
        let target = self.target()?;
        let host = if endpoint.host.contains(':') {
            format!("[{}]", endpoint.host)
        } else {
            endpoint.host
        };
        let forward_spec = format!("{}:{}:{}", endpoint.port, host, endpoint.port);
        let forward = format!(
            "ssh -N -p {} -L {} {}",
            target.port,
            shell_quote(&forward_spec),
            shell_quote(&target.destination)
        );
        self.instruction = Some(HeadlessAuthorizationInstruction {
            authorization_url: url,
            ssh_local_forward: forward,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::state::Timestamp;
    use std::sync::{Mutex, MutexGuard};

    static LOOPBACK_TEST_GUARD: Mutex<()> = Mutex::new(());
    const LOOPBACK_TEST_TIMEOUT: Duration = Duration::from_millis(250);

    fn lock_loopback_tests() -> MutexGuard<'static, ()> {
        LOOPBACK_TEST_GUARD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn authorize_url_has_only_public_client_parameters() {
        let url = authorization_url(
            "client sentinel",
            "http://127.0.0.1:43127/callback",
            &AuthorizationState::from_bytes(b"state sentinel"),
        )
        .unwrap();
        let query = url.query().unwrap();
        assert!(query.contains("response_type=code"));
        assert!(query.contains("client_id=client+sentinel"));
        assert!(query.contains("redirect_uri="));
        assert!(query.contains("state=state+sentinel"));
        assert!(!query.contains("scope"));
        assert!(!query.contains("secret"));
        assert!(!query.contains("pkce"));
    }
    #[test]
    fn command_argv_does_not_use_shell_interpolation() {
        let (_, args) = browser_command("linux", "https://example.test/?x=%26");
        assert_eq!(args, vec!["https://example.test/?x=%26"]);
    }
    #[test]
    fn explicit_headless_target_is_rendered_with_fixed_port() {
        let target = SshTarget::new("user@example.test", 22).unwrap();
        let mut browser = HeadlessBrowserAuthorization::new(
            "client",
            "http://127.0.0.1:43127/callback",
            Some(target),
        );
        browser
            .open(AuthorizationRequest {
                state: AuthorizationState::from_bytes(b"state"),
                expires_at: Timestamp::new(1),
            })
            .unwrap();
        let instruction = browser.instruction().unwrap();
        assert_eq!(
            instruction.ssh_local_forward,
            "ssh -N -p 22 -L '43127:127.0.0.1:43127' 'user@example.test'"
        );
        assert!(!instruction.ssh_local_forward.contains("state"));
    }

    #[test]
    fn callback_parser_preserves_terminal_outcomes_without_echoing_values() {
        let code = parse_request(
            b"GET /callback?state=safe-state&code=secret-code HTTP/1.1\r\nHost: localhost\r\n\r\n"
                .as_slice(),
            "/callback",
        )
        .unwrap();
        assert!(matches!(code.1, CallbackResult::Code(_)));
        assert_eq!(code.0.as_bytes(), &b"safe-state"[..]);
        let denied = parse_request(
            b"GET /callback?state=safe-state&error=access_denied&description=sentinel HTTP/1.1\r\n\r\n".as_slice(),
            "/callback",
        )
        .unwrap();
        assert_eq!(denied.1, CallbackResult::Denied);
        let cancelled = parse_request(
            b"GET /callback?state=safe-state&error=cancelled HTTP/1.1\r\n\r\n".as_slice(),
            "/callback",
        )
        .unwrap();
        assert_eq!(cancelled.1, CallbackResult::Cancelled);
        let missing_state = parse_request(
            b"GET /callback?code=secret-code HTTP/1.1\r\n\r\n".as_slice(),
            "/callback",
        );
        assert!(missing_state.is_err());
    }

    const PRODUCT_CALLBACK: &str = "ghostlink://oauth/callback";

    #[test]
    fn authorize_url_accepts_the_product_custom_scheme() {
        let url = authorization_url(
            "client",
            PRODUCT_CALLBACK,
            &AuthorizationState::from_bytes(b"state-abc"),
        )
        .expect("the product custom-scheme callback is accepted");
        let query = url.query().unwrap();
        assert!(query.contains("redirect_uri=ghostlink%3A%2F%2Foauth%2Fcallback"));
        assert!(query.contains("state=state-abc"));

        // No other scheme is accepted for the redirect.
        assert!(
            authorization_url(
                "client",
                "evil://oauth/callback",
                &AuthorizationState::from_bytes(b"state")
            )
            .is_err()
        );
    }

    #[test]
    fn manual_paste_receiver_accepts_the_registered_callback() {
        let mut receiver = ManualPasteCallbackReceiver::new(
            PRODUCT_CALLBACK,
            std::io::Cursor::new(
                b"ghostlink://oauth/callback?state=safe-state&code=secret-code\n".to_vec(),
            ),
        )
        .unwrap();

        let callback = receiver.receive().unwrap();
        assert_eq!(callback.state.as_bytes(), b"safe-state");
        assert!(matches!(callback.result, CallbackResult::Code(_)));
        // The code is carried opaquely and never rendered.
        assert!(!format!("{:?}", callback.result).contains("secret-code"));
    }

    #[test]
    fn manual_paste_receiver_preserves_terminal_outcomes() {
        for (pasted, expected) in [
            (
                "ghostlink://oauth/callback?state=s&error=cancelled",
                CallbackResult::Cancelled,
            ),
            (
                "ghostlink://oauth/callback?state=s&error=access_denied",
                CallbackResult::Denied,
            ),
        ] {
            let mut receiver = ManualPasteCallbackReceiver::new(
                PRODUCT_CALLBACK,
                std::io::Cursor::new(format!("{pasted}\n").into_bytes()),
            )
            .unwrap();
            assert_eq!(receiver.receive().unwrap().result, expected);
        }
    }

    #[test]
    fn manual_paste_receiver_rejects_foreign_and_malformed_uris() {
        for pasted in [
            "http://127.0.0.1:43127/oauth/callback?state=s&code=c",
            "ghostlink://evil.test/callback?state=s&code=c",
            "ghostlink://oauth/other?state=s&code=c",
            "ghostlink://oauth/callback?code=c",
            "ghostlink://oauth/callback?state=s&state=t&code=c",
            "ghostlink://oauth/callback?state=s&code=c&error=denied",
            "ghostlink://oauth/callback?state=s&code=c#fragment",
            "not a uri",
            "",
        ] {
            let mut receiver = ManualPasteCallbackReceiver::new(
                PRODUCT_CALLBACK,
                std::io::Cursor::new(format!("{pasted}\n").into_bytes()),
            )
            .unwrap();
            let error = receiver
                .receive()
                .expect_err("a foreign or malformed callback is rejected");
            assert_eq!(error.category(), ErrorCategory::CsrfStateMismatch);
        }
    }

    #[test]
    fn manual_paste_receiver_rejects_a_sentinel_bearing_callback_without_echoing_it() {
        let sentinel = "state-sentinel-not-registered";
        let mut receiver = ManualPasteCallbackReceiver::new(
            PRODUCT_CALLBACK,
            std::io::Cursor::new(
                format!("ghostlink://evil.test/callback?state={sentinel}&code=code-sentinel\n")
                    .into_bytes(),
            ),
        )
        .unwrap();
        let error = receiver.receive().unwrap_err();
        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));
    }

    #[test]
    fn manual_paste_receiver_bounds_the_pasted_line() {
        let long = format!(
            "ghostlink://oauth/callback?state={}&code=c\n",
            "a".repeat(64)
        );
        let mut receiver = ManualPasteCallbackReceiver::new(
            PRODUCT_CALLBACK,
            std::io::Cursor::new(long.into_bytes()),
        )
        .unwrap()
        .with_max_bytes(8);
        assert_eq!(
            receiver.receive().unwrap_err().category(),
            ErrorCategory::CsrfStateMismatch
        );
    }

    #[test]
    fn manual_paste_receiver_rejects_an_unregistered_configured_callback() {
        assert!(
            ManualPasteCallbackReceiver::new(
                "ghostlink://oauth/other",
                std::io::Cursor::new(Vec::<u8>::new()),
            )
            .is_err()
        );
    }

    /// An echo control that only records the calls it receives.
    #[derive(Clone, Default)]
    struct RecordingEcho(std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>);

    impl RecordingEcho {
        fn calls(&self) -> Vec<&'static str> {
            self.0.borrow().clone()
        }
    }

    impl TerminalEcho for RecordingEcho {
        fn disable(&mut self) -> Result<(), AppError> {
            self.0.borrow_mut().push("disable");
            Ok(())
        }

        fn restore(&mut self) -> Result<(), AppError> {
            self.0.borrow_mut().push("restore");
            Ok(())
        }
    }

    /// An echo control that refuses to silence the terminal.
    #[derive(Clone, Default)]
    struct RefusingEcho(std::rc::Rc<std::cell::Cell<usize>>);

    impl TerminalEcho for RefusingEcho {
        fn disable(&mut self) -> Result<(), AppError> {
            self.0.set(self.0.get() + 1);
            Err(AppError::new(ErrorCategory::UnsupportedOperation))
        }

        fn restore(&mut self) -> Result<(), AppError> {
            Ok(())
        }
    }

    /// A reader whose underlying buffer read always fails.
    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("underlying read failed"))
        }
    }

    impl BufRead for FailingReader {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            Err(std::io::Error::other("underlying read failed"))
        }

        fn consume(&mut self, _amount: usize) {}
    }

    #[test]
    fn no_echo_reader_silences_the_terminal_for_the_pasted_line_only() {
        let echo = RecordingEcho::default();
        let mut reader = NoEchoReader::new(
            std::io::Cursor::new(b"ghostlink://oauth/callback?state=s&code=c\n".to_vec()),
            echo.clone(),
        );
        let mut line = Vec::new();
        reader.read_until(b'\n', &mut line).unwrap();
        assert_eq!(line, b"ghostlink://oauth/callback?state=s&code=c\n");
        // Echo was disabled before the line was read and restored once the line
        // was complete; the code is never reflected to the terminal.
        assert_eq!(echo.calls(), vec!["disable", "restore"]);
    }

    #[test]
    fn no_echo_reader_restores_echo_at_end_of_input() {
        let echo = RecordingEcho::default();
        let mut reader = NoEchoReader::new(std::io::Cursor::new(Vec::<u8>::new()), echo.clone());
        let mut line = Vec::new();
        reader.read_until(b'\n', &mut line).unwrap();
        assert!(line.is_empty());
        assert_eq!(echo.calls(), vec!["disable", "restore"]);
    }

    #[test]
    fn no_echo_reader_restores_echo_when_the_underlying_read_fails() {
        let echo = RecordingEcho::default();
        let mut reader = NoEchoReader::new(FailingReader, echo.clone());
        assert!(reader.fill_buf().is_err());
        assert_eq!(echo.calls(), vec!["disable", "restore"]);
    }

    #[test]
    fn no_echo_reader_restores_echo_when_dropped_mid_line() {
        let echo = RecordingEcho::default();
        {
            let mut reader =
                NoEchoReader::new(std::io::Cursor::new(b"partial line".to_vec()), echo.clone());
            assert!(!reader.fill_buf().unwrap().is_empty());
            // A partial line leaves echo suppressed until the reader is dropped.
            assert_eq!(echo.calls(), vec!["disable"]);
        }
        assert_eq!(echo.calls(), vec!["disable", "restore"]);
    }

    #[test]
    fn no_echo_reader_fails_closed_when_echo_cannot_be_disabled() {
        let refusing = RefusingEcho::default();
        let mut reader = NoEchoReader::new(
            std::io::Cursor::new(b"ghostlink://oauth/callback?state=s&code=secret\n".to_vec()),
            refusing.clone(),
        );
        // The read fails before consuming a byte, so a terminal that cannot be
        // silenced never receives the code.
        assert!(reader.fill_buf().is_err());
        assert_eq!(refusing.0.get(), 1);
    }

    #[test]
    fn the_paste_receiver_reads_through_a_no_echo_reader() {
        let echo = RecordingEcho::default();
        let line = std::io::Cursor::new(
            b"ghostlink://oauth/callback?state=safe-state&code=secret-code\n".to_vec(),
        );
        let reader = NoEchoReader::new(line, echo.clone());
        let mut receiver = ManualPasteCallbackReceiver::new(PRODUCT_CALLBACK, reader).unwrap();
        let callback = receiver.receive().unwrap();
        assert_eq!(callback.state.as_bytes(), b"safe-state");
        assert_eq!(echo.calls(), vec!["disable", "restore"]);
    }

    #[derive(Clone, Default)]
    struct CountingLauncher(std::rc::Rc<std::cell::Cell<usize>>);

    impl CountingLauncher {
        fn launches(&self) -> usize {
            self.0.get()
        }
    }

    impl CommandLauncher for CountingLauncher {
        fn launch(&mut self, _program: &str, _args: &[&str]) -> Result<(), AppError> {
            self.0.set(self.0.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn manual_paste_browser_records_the_url_and_launches_only_when_asked() {
        for (open_browser, expected_launches) in [(false, 0usize), (true, 1usize)] {
            let launcher = CountingLauncher::default();
            let mut browser = ManualPasteBrowserAuthorization::new(
                "client",
                PRODUCT_CALLBACK,
                launcher.clone(),
                open_browser,
            );
            browser
                .open(AuthorizationRequest {
                    state: AuthorizationState::from_bytes(b"state-abc"),
                    expires_at: Timestamp::new(1),
                })
                .unwrap();
            let recorded = browser
                .instruction()
                .expect("the authorization URL is always recorded");
            let query = recorded.authorization_url.query().unwrap();
            assert!(query.contains("state=state-abc"));
            assert!(query.contains("redirect_uri=ghostlink%3A%2F%2Foauth%2Fcallback"));
            assert_eq!(launcher.launches(), expected_launches);
        }
    }

    #[test]
    fn ssh_target_rejects_shell_syntax_and_invalid_ports() {
        for destination in ["-host", "user;bad", "user'bad", "$(bad)", "user bad"] {
            assert!(SshTarget::new(destination, 22).is_err());
        }
        assert!(SshTarget::new("user@example.test", 0).is_err());
        let target = SshTarget::new("user@[::1]", 22).unwrap();
        assert_eq!(target.destination, "user@[::1]");
    }

    #[test]
    fn callback_receiver_binds_explicit_ipv6_when_available() {
        let _guard = lock_loopback_tests();
        let listener = match std::net::TcpListener::bind("[::1]:0") {
            Ok(listener) => listener,
            Err(_) => return,
        };
        let port = listener.local_addr().unwrap().port();
        let endpoint = CallbackEndpoint::parse(&format!("http://[::1]:{port}/callback")).unwrap();
        let mut receiver =
            LoopbackCallbackReceiver::from_bound_listeners(endpoint, vec![listener]).unwrap();
        let thread = std::thread::spawn(move || receiver.receive());
        let mut stream = std::net::TcpStream::connect(("::1", port)).unwrap();
        stream
            .write_all(b"GET /callback?state=safe&code=code HTTP/1.1\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(matches!(
            thread.join().unwrap(),
            Ok(AuthorizationCallback { .. })
        ));
    }

    #[test]
    fn callback_receiver_localhost_uses_available_families() {
        let _guard = lock_loopback_tests();
        let ipv4 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = ipv4.local_addr().unwrap().port();
        let mut listeners = vec![ipv4];
        if let Ok(ipv6) = std::net::TcpListener::bind(("::1", port)) {
            listeners.push(ipv6);
        }
        let endpoint =
            CallbackEndpoint::parse(&format!("http://localhost:{port}/callback")).unwrap();
        let receiver = LoopbackCallbackReceiver::from_bound_listeners(endpoint, listeners).unwrap();
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());
        if receiver.listeners.len() == 2 {
            assert!(std::net::TcpStream::connect(("::1", port)).is_ok());
        }
    }

    #[test]
    fn callback_receiver_serves_a_real_local_socket() {
        let _guard = lock_loopback_tests();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let endpoint =
            CallbackEndpoint::parse(&format!("http://127.0.0.1:{port}/callback")).unwrap();
        let mut receiver =
            LoopbackCallbackReceiver::from_bound_listeners(endpoint, vec![listener]).unwrap();
        let port = receiver.local_port().unwrap();
        let thread = std::thread::spawn(move || receiver.receive());
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(b"GET /callback?state=safe&code=code HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let callback = thread.join().unwrap().unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(matches!(callback.result, CallbackResult::Code(_)));
    }

    #[test]
    fn callback_receiver_times_out_without_a_connection() {
        let _guard = lock_loopback_tests();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let endpoint =
            CallbackEndpoint::parse(&format!("http://127.0.0.1:{port}/callback")).unwrap();
        let mut receiver = LoopbackCallbackReceiver::from_bound_listeners(endpoint, vec![listener])
            .unwrap()
            .with_timeouts(LOOPBACK_TEST_TIMEOUT, LOOPBACK_TEST_TIMEOUT);
        let result = receiver.receive();
        assert_eq!(result, Err(AppError::new(ErrorCategory::CallbackTimeout)));
    }

    #[test]
    fn callback_receiver_times_out_incomplete_reads_with_sanitized_failure() {
        let _guard = lock_loopback_tests();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let endpoint =
            CallbackEndpoint::parse(&format!("http://127.0.0.1:{port}/callback")).unwrap();
        let mut receiver = LoopbackCallbackReceiver::from_bound_listeners(endpoint, vec![listener])
            .unwrap()
            .with_timeouts(LOOPBACK_TEST_TIMEOUT, LOOPBACK_TEST_TIMEOUT);
        let thread = std::thread::spawn(move || receiver.receive());
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(b"GET /callback?state=partial HTTP/1.1\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let result = thread.join().unwrap();
        assert_eq!(result, Err(AppError::new(ErrorCategory::CallbackTimeout)));
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("Authorization failed."));
    }

    #[test]
    fn callback_receiver_enforces_one_deadline_across_slow_drip_reads() {
        let _guard = lock_loopback_tests();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let endpoint =
            CallbackEndpoint::parse(&format!("http://127.0.0.1:{port}/callback")).unwrap();
        let mut receiver = LoopbackCallbackReceiver::from_bound_listeners(endpoint, vec![listener])
            .unwrap()
            .with_timeouts(LOOPBACK_TEST_TIMEOUT, LOOPBACK_TEST_TIMEOUT);
        let thread = std::thread::spawn(move || {
            let started = Instant::now();
            let result = receiver.receive();
            (started.elapsed(), result)
        });
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(LOOPBACK_TEST_TIMEOUT + Duration::from_secs(2)))
            .unwrap();
        let drip = b"GET /callback?state=partial HTTP/1.1\r\n";
        for byte in drip {
            if stream.write_all(std::slice::from_ref(byte)).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let (elapsed, result) = thread.join().unwrap();
        assert_eq!(result, Err(AppError::new(ErrorCategory::CallbackTimeout)));
        assert!(elapsed >= LOOPBACK_TEST_TIMEOUT);
        assert!(elapsed < LOOPBACK_TEST_TIMEOUT + Duration::from_millis(750));
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("Authorization failed."));
    }

    #[test]
    fn callback_receiver_rejects_oversized_requests_with_sanitized_failure() {
        let _guard = lock_loopback_tests();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let endpoint =
            CallbackEndpoint::parse(&format!("http://127.0.0.1:{port}/callback")).unwrap();
        let mut receiver =
            LoopbackCallbackReceiver::from_bound_listeners(endpoint, vec![listener]).unwrap();
        let thread = std::thread::spawn(move || receiver.receive());
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(&vec![b'x'; MAX_REQUEST_BYTES + 1])
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let result = thread.join().unwrap();
        assert_eq!(
            result,
            Err(AppError::new(ErrorCategory::UnsupportedOperation))
        );
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("Authorization failed."));
        assert!(!response.contains("x".repeat(32).as_str()));
    }

    #[test]
    fn callback_parser_rejects_duplicate_or_conflicting_outcomes() {
        for query in [
            "state=x&state=y&code=z",
            "state=x&code=a&code=b",
            "state=x&code=a&error=denied",
        ] {
            let request = format!("GET /callback?{query} HTTP/1.1\r\n\r\n");
            assert!(parse_request(request.as_bytes(), "/callback").is_err());
        }
    }

    #[test]
    fn callback_endpoint_rejects_zero_port_before_binding_or_guidance() {
        assert!(LoopbackCallbackReceiver::bind("http://127.0.0.1:0/callback").is_err());
        assert!(
            authorization_url(
                "client",
                "http://127.0.0.1:0/callback",
                &AuthorizationState::from_bytes(b"state")
            )
            .is_err()
        );
    }

    #[test]
    fn inferred_ssh_targets_accept_safe_ipv4_and_ipv6_only() {
        assert_eq!(
            inferred_ssh_target("192.0.2.1 50000 198.51.100.2 22", "safe_user")
                .unwrap()
                .destination,
            "safe_user@198.51.100.2"
        );
        assert_eq!(
            inferred_ssh_target("2001:db8::1 50000 2001:db8::2 22", "safe-user")
                .unwrap()
                .destination,
            "safe-user@[2001:db8::2]"
        );
        for user in ["bad user", "bad'quote", "$(bad)", "-bad"] {
            assert!(inferred_ssh_target("192.0.2.1 50000 198.51.100.2 22", user).is_err());
        }
        assert!(inferred_ssh_target("192.0.2.1 50000 evil.example 22", "safe").is_err());
    }

    #[test]
    fn ssh_ipv6_forward_is_bracketed() {
        let target = SshTarget::new("safe@[2001:db8::2]", 22).unwrap();
        let mut browser = HeadlessBrowserAuthorization::new(
            "client",
            "http://[::1]:43127/callback",
            Some(target),
        );
        browser
            .open(AuthorizationRequest {
                state: AuthorizationState::from_bytes(b"state"),
                expires_at: Timestamp::new(1),
            })
            .unwrap();
        assert_eq!(
            browser.instruction().unwrap().ssh_local_forward,
            "ssh -N -p 22 -L '43127:[::1]:43127' 'safe@[2001:db8::2]'"
        );
    }

    #[test]
    fn callback_parser_rejects_malformed_targets_and_all_outcome_conflicts() {
        for request_line in [
            "GET  /callback?state=x&code=y HTTP/1.1",
            "GET /callback?state=x HTTP/1.0",
            "GET /callback?state=x&code=y extra HTTP/1.1",
            "GET /callback?state=x&code=y",
            "GET /callback?state=x&code=y#fragment HTTP/1.1",
            "GET /callback?state=x&code=y%23fragment HTTP/1.1",
        ] {
            let request = format!("{request_line}\r\n\r\n");
            let error = parse_request(request.as_bytes(), "/callback").unwrap_err();
            assert!(matches!(
                error.category(),
                ErrorCategory::ConfigurationError | ErrorCategory::UnsupportedOperation
            ));
        }
        for query in [
            "state=x&state=y",
            "state=x&code=a&code=b",
            "state=x&error=a&error=b",
            "state=x&code=a&error=b",
            "state=x&code=a&error=b&description=c",
        ] {
            let request = format!("GET /callback?{query} HTTP/1.1\r\n\r\n");
            let error = parse_request(request.as_bytes(), "/callback").unwrap_err();
            assert_eq!(error.category(), ErrorCategory::ConfigurationError);
        }
    }

    #[test]
    fn callback_parser_rejects_wrong_path_and_method() {
        assert!(
            parse_request(
                b"POST /callback?state=x&code=y HTTP/1.1\r\n\r\n",
                "/callback"
            )
            .is_err()
        );
        assert!(parse_request(b"GET /other?state=x&code=y HTTP/1.1\r\n\r\n", "/callback").is_err());
        assert!(
            parse_request(
                b"GET /callback?state=x&code=y HTTP/1.1\r\n\r\n",
                "/callback"
            )
            .is_ok()
        );
    }

    /// A unique temporary directory for one rendezvous test.
    #[cfg(target_os = "linux")]
    struct HandlerDir(std::path::PathBuf);

    #[cfg(target_os = "linux")]
    impl HandlerDir {
        fn new(label: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!(
                "ghostlink-handler-{}-{label}-{unique}",
                std::process::id()
            )))
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }

        fn join(&self, name: &str) -> std::path::PathBuf {
            self.0.join(name)
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for HandlerDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(target_os = "linux")]
    fn bound_endpoint_name(directory: &HandlerDir) -> String {
        std::fs::read_link(directory.join(RENDEZVOUS_ENDPOINT_FILE))
            .expect("the bound rendezvous publishes its endpoint")
            .to_string_lossy()
            .into_owned()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn handler_rendezvous_delivers_one_forwarded_callback_and_cleans_up() {
        let directory = HandlerDir::new("deliver");
        let mut rendezvous =
            CallbackRendezvous::bind_in(directory.path().to_path_buf(), PRODUCT_CALLBACK)
                .expect("the registered custom-scheme callback binds a rendezvous");

        // The published pointer names a random per-attempt socket, never the
        // callback or its one-use code, and never an arbitrary path.
        let name = bound_endpoint_name(&directory);
        assert!(is_rendezvous_socket_name(&name), "endpoint: {name}");
        assert!(!name.contains("state") && !name.contains("code") && !name.contains('/'));
        let socket = directory.join(&name);
        assert!(socket.exists(), "the socket exists while the login waits");

        let forward_directory = directory.path().to_path_buf();
        let forwarder = thread::spawn(move || {
            forward_callback_in(
                &forward_directory,
                "ghostlink://oauth/callback?state=safe-state&code=secret-code",
            )
        });
        let callback = rendezvous
            .receive()
            .expect("the forwarded callback arrives");
        assert!(forwarder.join().expect("the forwarder thread").is_ok());
        assert_eq!(callback.state.as_bytes(), b"safe-state");
        assert!(matches!(callback.result, CallbackResult::Code(_)));
        assert!(!format!("{:?}", callback.result).contains("secret-code"));

        drop(rendezvous);
        assert!(!socket.exists(), "the socket is removed on drop");
        assert!(
            !directory.join(RENDEZVOUS_ENDPOINT_FILE).exists(),
            "the endpoint pointer is removed on drop"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn handler_rendezvous_times_out_without_a_callback_and_cleans_up() {
        let directory = HandlerDir::new("timeout");
        let mut rendezvous =
            CallbackRendezvous::bind_in(directory.path().to_path_buf(), PRODUCT_CALLBACK)
                .expect("bind")
                .with_wait(Duration::from_millis(250));
        let socket = directory.join(&bound_endpoint_name(&directory));

        let error = rendezvous.receive().expect_err("no callback arrives");
        assert_eq!(error.category(), ErrorCategory::CallbackTimeout);

        drop(rendezvous);
        assert!(!socket.exists());
        assert!(!directory.join(RENDEZVOUS_ENDPOINT_FILE).exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn forwarding_rejects_foreign_oversized_and_whitespace_bearing_arguments() {
        let directory = HandlerDir::new("reject");
        for value in [
            "http://127.0.0.1:43127/oauth/callback?state=s&code=c",
            "ghostlink://evil.test/callback?state=s&code=c",
            "ghostlink://oauth/other?state=s&code=c",
            "ghostlink://oauth/callback?state=s&code=c#fragment",
            "ghostlink://oauth/callback?state=s&code=c\n",
            "not a uri",
            "",
        ] {
            let error = forward_callback_in(directory.path(), value)
                .expect_err("a foreign value is never forwarded");
            assert_eq!(
                error.category(),
                ErrorCategory::CsrfStateMismatch,
                "value: {value:?}"
            );
        }

        let oversized = format!(
            "ghostlink://oauth/callback?state={}",
            "a".repeat(MAX_CALLBACK_LINE_BYTES)
        );
        let error = forward_callback_in(directory.path(), &oversized)
            .expect_err("an oversized value is never forwarded");
        assert_eq!(error.category(), ErrorCategory::CsrfStateMismatch);
        // A rejected argument never creates or reads a rendezvous.
        assert!(!directory.path().exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn forwarding_fails_closed_without_a_live_rendezvous() {
        let directory = HandlerDir::new("missing");
        let error = forward_callback_in(
            directory.path(),
            "ghostlink://oauth/callback?state=s&code=c",
        )
        .expect_err("no pointer means no waiting login");
        assert_eq!(error.category(), ErrorCategory::CallbackTimeout);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn forwarding_refuses_a_pointer_that_names_an_arbitrary_path() {
        // A regular file is not the symlink pointer this product publishes, so
        // it is refused without ever being opened.
        let directory = HandlerDir::new("pointer");
        std::fs::create_dir_all(directory.path()).expect("prepare the directory");
        std::fs::write(
            directory.join(RENDEZVOUS_ENDPOINT_FILE),
            b"../../etc/passwd",
        )
        .expect("write a hostile regular-file pointer");
        let error = forward_callback_in(
            directory.path(),
            "ghostlink://oauth/callback?state=s&code=c",
        )
        .expect_err("a regular-file pointer is refused");
        assert_eq!(error.category(), ErrorCategory::ConfigurationError);

        // A symlink pointer whose target names a path off the rendezvous is
        // resolved with `readlink` and refused, never followed and never opened.
        std::fs::remove_file(directory.join(RENDEZVOUS_ENDPOINT_FILE)).expect("clear the pointer");
        std::os::unix::fs::symlink("../../etc/passwd", directory.join(RENDEZVOUS_ENDPOINT_FILE))
            .expect("write a hostile symlink pointer");
        let error = forward_callback_in(
            directory.path(),
            "ghostlink://oauth/callback?state=s&code=c",
        )
        .expect_err("a symlink off the rendezvous is refused");
        assert_eq!(error.category(), ErrorCategory::ConfigurationError);

        // A pointer whose target is a well-formed socket name that names no
        // live listener must not block the handler: `readlink` never opens the
        // target, and the subsequent connect to a dangling socket name fails
        // immediately instead of waiting on an attacker-prepared FIFO.
        std::fs::remove_file(directory.join(RENDEZVOUS_ENDPOINT_FILE)).expect("clear the pointer");
        let hostile_name = format!(
            "{}deadbeefdeadbeefdeadbeefdeadbeef{}",
            RENDEZVOUS_SOCKET_PREFIX, RENDEZVOUS_SOCKET_SUFFIX
        );
        std::os::unix::fs::symlink(&hostile_name, directory.join(RENDEZVOUS_ENDPOINT_FILE))
            .expect("point the pointer at a well-formed but unbound socket name");
        let error = forward_callback_in(
            directory.path(),
            "ghostlink://oauth/callback?state=s&code=c",
        )
        .expect_err("a dangling well-formed pointer is refused, not blocked on");
        assert_eq!(error.category(), ErrorCategory::CallbackTimeout);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn forwarding_connect_is_bounded_when_the_listener_never_accepts() {
        // A blocking Unix connect parks with no deadline once the listener's
        // accept queue is full. The handler must not inherit that wait while it
        // carries the one-use code in its argv, so the connect is bounded.
        enum Progress {
            Connecting,
            Connected,
        }
        let directory = HandlerDir::new("bounded-connect");
        std::fs::create_dir_all(directory.path()).expect("prepare the rendezvous directory");
        let socket_name = format!(
            "{}deadbeefdeadbeefdeadbeefdeadbeef{}",
            RENDEZVOUS_SOCKET_PREFIX, RENDEZVOUS_SOCKET_SUFFIX
        );
        let socket_path = directory.join(&socket_name);
        let _listener =
            std::os::unix::net::UnixListener::bind(&socket_path).expect("bind the endpoint socket");
        std::os::unix::fs::symlink(&socket_name, directory.join(RENDEZVOUS_ENDPOINT_FILE))
            .expect("publish a well-formed pointer to the socket");

        // Fill the accept queue from one worker. A `Connecting` with no
        // following `Connected` means the worker is parked inside connect, so
        // the queue is full and the next connect cannot complete.
        let (sender, receiver) = std::sync::mpsc::channel();
        let filler_path = socket_path.clone();
        let _filler = thread::spawn(move || {
            let mut held = Vec::new();
            loop {
                if sender.send(Progress::Connecting).is_err() {
                    break;
                }
                match std::os::unix::net::UnixStream::connect(&filler_path) {
                    Ok(stream) => {
                        held.push(stream);
                        if sender.send(Progress::Connected).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            held
        });
        let mut parked = false;
        loop {
            match receiver.recv_timeout(Duration::from_millis(500)) {
                Ok(Progress::Connecting) => parked = true,
                Ok(Progress::Connected) => parked = false,
                Err(_) => break,
            }
        }
        assert!(
            parked,
            "the filler parks in connect once the accept queue is full"
        );

        let started = Instant::now();
        let error = forward_callback_in_with_timeout(
            directory.path(),
            "ghostlink://oauth/callback?state=s&code=c",
            Duration::from_millis(250),
        )
        .expect_err("a full accept queue is a bounded timeout, not an unbounded wait");
        let elapsed = started.elapsed();

        assert_eq!(error.category(), ErrorCategory::CallbackTimeout);
        assert!(
            elapsed < Duration::from_secs(2),
            "the handler timeout bounded the connect: {elapsed:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_second_concurrent_login_fails_closed_until_the_first_releases() {
        let directory = HandlerDir::new("singleton");
        let first = CallbackRendezvous::bind_in(directory.path().to_path_buf(), PRODUCT_CALLBACK)
            .expect("the first login claims the per-user slot");

        // A second concurrent login must fail closed with the "already
        // authorizing" outcome, before it can bind a socket or overwrite the
        // first attempt's pointer.
        let error = CallbackRendezvous::bind_in(directory.path().to_path_buf(), PRODUCT_CALLBACK)
            .expect_err("only one login may own the rendezvous at a time");
        assert_eq!(error.category(), ErrorCategory::AlreadyAuthorizing);

        // The first attempt still owns a live, working rendezvous.
        let name = bound_endpoint_name(&directory);
        assert!(is_rendezvous_socket_name(&name));
        drop(first);

        // Once the first attempt is gone, the slot is available again.
        let second = CallbackRendezvous::bind_in(directory.path().to_path_buf(), PRODUCT_CALLBACK)
            .expect("the slot is reusable after the first attempt releases it");
        drop(second);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_group_or_world_accessible_rendezvous_directory_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let directory = HandlerDir::new("perms");
        std::fs::create_dir(directory.path()).expect("prepare the directory");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755))
            .expect("loosen the directory");
        let error = CallbackRendezvous::bind_in(directory.path().to_path_buf(), PRODUCT_CALLBACK)
            .expect_err("a shared directory is never a rendezvous");
        assert_eq!(error.category(), ErrorCategory::ConfigurationError);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_rendezvous_bounds_the_forwarded_payload() {
        let directory = HandlerDir::new("bound");
        let mut rendezvous =
            CallbackRendezvous::bind_in(directory.path().to_path_buf(), PRODUCT_CALLBACK)
                .expect("bind");
        let socket = directory.join(&bound_endpoint_name(&directory));

        let sender = thread::spawn(move || {
            let mut stream = std::os::unix::net::UnixStream::connect(&socket)
                .expect("the per-attempt socket accepts one connection");
            let _ = stream.write_all(&vec![b'x'; MAX_CALLBACK_LINE_BYTES + 1]);
            let _ = stream.flush();
        });
        let error = rendezvous
            .receive()
            .expect_err("an oversized payload is rejected");
        assert_eq!(error.category(), ErrorCategory::CsrfStateMismatch);
        let _ = sender.join();
    }
}
