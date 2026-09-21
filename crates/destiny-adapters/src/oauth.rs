//! Offline-safe public-client OAuth browser and callback adapters.

use destiny_core::ports::{AuthorizationRequest, BrowserAuthorization, CallbackReceiver};
use destiny_core::{
    AppError, AuthorizationCallback, AuthorizationCode, AuthorizationState, CallbackResult,
    ErrorCategory,
};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

const MAX_REQUEST_BYTES: usize = 8 * 1024;
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const AUTHORIZE_ENDPOINT: &str = "https://www.bungie.net/en/oauth/authorize";
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
        })
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
        let deadline = Instant::now() + ACCEPT_TIMEOUT;
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
        let read_deadline = Instant::now() + READ_TIMEOUT;
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
    CallbackEndpoint::parse(callback_uri)?;
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
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let endpoint =
            CallbackEndpoint::parse(&format!("http://127.0.0.1:{port}/callback")).unwrap();
        let mut receiver =
            LoopbackCallbackReceiver::from_bound_listeners(endpoint, vec![listener]).unwrap();
        let result = receiver.receive();
        assert_eq!(result, Err(AppError::new(ErrorCategory::CallbackTimeout)));
    }

    #[test]
    fn callback_receiver_times_out_incomplete_reads_with_sanitized_failure() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let endpoint =
            CallbackEndpoint::parse(&format!("http://127.0.0.1:{port}/callback")).unwrap();
        let mut receiver =
            LoopbackCallbackReceiver::from_bound_listeners(endpoint, vec![listener]).unwrap();
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
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let endpoint =
            CallbackEndpoint::parse(&format!("http://127.0.0.1:{port}/callback")).unwrap();
        let mut receiver =
            LoopbackCallbackReceiver::from_bound_listeners(endpoint, vec![listener]).unwrap();
        let thread = std::thread::spawn(move || {
            let started = Instant::now();
            let result = receiver.receive();
            (started.elapsed(), result)
        });
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(READ_TIMEOUT + Duration::from_secs(2)))
            .unwrap();
        let drip = b"GET /callback?state=partial HTTP/1.1\r\n";
        for byte in drip {
            if stream.write_all(std::slice::from_ref(byte)).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let (elapsed, result) = thread.join().unwrap();
        assert_eq!(result, Err(AppError::new(ErrorCategory::CallbackTimeout)));
        assert!(elapsed >= READ_TIMEOUT);
        assert!(elapsed < READ_TIMEOUT + Duration::from_secs(2));
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("Authorization failed."));
    }

    #[test]
    fn callback_receiver_rejects_oversized_requests_with_sanitized_failure() {
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
}
