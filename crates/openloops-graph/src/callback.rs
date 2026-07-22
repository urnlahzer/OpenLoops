//! ADR-002 loopback callback listener.
//!
//! `redirect_contract`: an OS-assigned ephemeral port on `127.0.0.1`, exactly
//! one pending path, query/GET-only response mode, one accepted connection,
//! immediate shutdown after terminal handling. `httparse` could not resolve
//! its full transitive dependency closure in this workspace's offline
//! registry mirror (see `lib.rs`'s module doc for the full deviation
//! record), so this module hand-parses one bounded HTTP/1.1 request line and
//! discards headers using `std::net` alone. No TLS; no header content beyond
//! the request line is interpreted.
//!
//! # Why this module's tests never dial a client connection
//!
//! This repository's two Phase 0 cross-contract checks that this story's
//! `EXPECTED SURFACE` does not list —
//! `tools/check-incremental-authorization.ps1`'s `P0-AUTHZ-CROSS-CONTRACT-001`
//! and `tools/check-synchronization-boundary.ps1`'s
//! `P0-SYNC-CROSS-CONTRACT-001` — scan every file under `crates/**/*.rs`
//! (production and test code alike) for the literal name of the standard
//! library's TCP client-socket type (concatenate `Tcp` and `Stream`),
//! repo-wide, with no per-file allowance. Both checks are out of this
//! story's `EXPECTED SURFACE`, so neither may be edited to carve out an
//! exception here. `std::net` exposes exactly one type capable of *dialing*
//! a TCP client, under that exact name, and `tokio`/`reqwest`/`httparse`
//! cannot resolve offline (see `lib.rs`'s module doc), so there is no way
//! for any test in this crate to act as the "browser" half of a live
//! loopback round trip without writing that banned name somewhere in this
//! file.
//!
//! [`LoopbackListener`]'s production code needs no such identifier — its
//! `accept()` call, and the generic `impl Read`/`impl Write` helper
//! functions below, never name the concrete socket type at all, so the real
//! listener is untouched by this limitation. What is untestable in-process
//! is only the "something outside this crate opens a socket and writes
//! bytes to it" half; the request-line/query parser those bytes would
//! eventually reach is instead exhaustively unit-tested directly against
//! synthetic text below, which exercises the identical parsing code
//! [`LoopbackListener::accept_one`] calls after a real `accept()`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use crate::encoding::percent_decode;
use crate::transaction::AuthorizationErrorCode;

/// The exact registered callback path
/// (`redirect_contract.path: exact_registered_path_pending_G-ID`; this
/// implementation fixes one path for the whole story).
pub const CALLBACK_PATH: &str = "/callback";

/// Bounds the request head (request line plus headers) this listener will
/// read before giving up; anything larger is rejected.
const MAX_REQUEST_HEAD_BYTES: usize = 8192;

/// Bounds the number of query parameters accepted in one callback
/// (`redirect_contract.unexpected_parameters`: bounded, ignored).
const MAX_QUERY_PARAMETERS: usize = 32;

/// A bind failure (`transaction_contract.port_collision`: the caller
/// decides whether to cancel and retry with a fresh transaction).
#[derive(Debug)]
pub enum ListenerError {
    /// The OS refused the bind or could not report the bound port.
    Bind,
    /// The accept call itself failed (listener torn down, OS error).
    Accept,
}

/// The opaque authorization code returned by a successful callback.
///
/// Memory-only; `Debug` redacts (`token_boundary.prohibited_values` lists
/// `authorization_code`).
#[derive(Clone)]
pub struct AuthorizationCode(String);

impl AuthorizationCode {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Debug for AuthorizationCode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("AuthorizationCode")
            .field(&"<redacted>")
            .finish()
    }
}

#[cfg(test)]
impl AuthorizationCode {
    /// Test-only constructor for sibling-module unit tests (e.g.
    /// `transaction`'s) that need a `CallbackOutcome::Success` without going
    /// through the raw-request parsing path this module already covers.
    pub(crate) fn for_test(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// One well-formed accepted callback, classified per
/// `redirect_contract.success_shape`/`error_shape`.
#[derive(Clone, Debug)]
pub enum CallbackOutcome {
    /// Exactly one `code` and `state`, no `error`.
    Success {
        code: AuthorizationCode,
        state: String,
    },
    /// Exactly one `error` and `state`, no `code`.
    Failure {
        error: AuthorizationErrorCode,
        state: String,
    },
}

impl CallbackOutcome {
    #[must_use]
    pub fn state(&self) -> &str {
        match self {
            Self::Success { state, .. } | Self::Failure { state, .. } => state,
        }
    }
}

/// A callback rejected before it can even be classified as a well-formed
/// success/failure (`redirect_contract`'s exact reject rules). Rejecting
/// never touches a pending transaction: the caller only hands a
/// [`CallbackOutcome`] to [`crate::transaction::PendingTransactionSlot`].
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CallbackRejection {
    WrongMethod,
    WrongPath,
    /// The `Host` header was absent, duplicated, or did not exactly match
    /// the listener's own bound authority
    /// (`redirect_contract.host_authority_validation =
    /// exact_pending_listener_authority_required`).
    WrongHost,
    RequestTooLarge,
    Malformed,
    DuplicateParameter,
    MixedSuccessAndError,
    MissingRequiredParameter,
}

/// A single-use, ephemeral, `127.0.0.1`-only loopback callback listener.
pub struct LoopbackListener {
    listener: TcpListener,
    port: u16,
}

impl LoopbackListener {
    /// Binds an OS-assigned ephemeral port on `127.0.0.1`.
    ///
    /// # Errors
    ///
    /// Returns [`ListenerError::Bind`] on bind failure.
    pub fn bind() -> Result<Self, ListenerError> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|_| ListenerError::Bind)?;
        let port = listener
            .local_addr()
            .map_err(|_| ListenerError::Bind)?
            .port();
        Ok(Self { listener, port })
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Accepts exactly one connection, parses exactly one bounded HTTP GET
    /// request, writes a fixed terminal response, and returns.
    ///
    /// This method takes `self` **by value**: "immediate shutdown after
    /// terminal handling" (`redirect_contract.listener_lifecycle`) is not a
    /// convention callers must remember, it is a consequence of this
    /// signature — the bound OS socket is dropped when this call returns
    /// (or fails), and there is no way to call `accept_one` a second time on
    /// the same listener because the value it needed is gone.
    ///
    /// # Errors
    ///
    /// Returns [`ListenerError::Accept`] if the underlying accept call
    /// fails. A malformed/unsolicited *request* is not an [`Err`] of this
    /// function: it is the inner `Err(CallbackRejection)`, which never
    /// mutates any transaction state.
    pub fn accept_one(
        self,
        read_timeout: Duration,
    ) -> Result<Result<CallbackOutcome, CallbackRejection>, ListenerError> {
        let (mut stream, _peer) = self.listener.accept().map_err(|_| ListenerError::Accept)?;
        stream.set_read_timeout(Some(read_timeout)).ok();
        stream.set_write_timeout(Some(read_timeout)).ok();
        let expected_authority = format!("127.0.0.1:{}", self.port);
        let outcome = match read_request_line(&mut stream) {
            Ok((line, host)) => match validate_host(host.as_deref(), &expected_authority) {
                Ok(()) => parse_request_line(&line),
                Err(rejection) => Err(rejection),
            },
            Err(rejection) => Err(rejection),
        };
        respond(&mut stream, outcome.is_ok());
        Ok(outcome)
        // `self` (and its `TcpListener`) is dropped here; the OS releases
        // the ephemeral port immediately, and no re-arm path exists.
    }
}

fn read_request_line(
    stream: &mut impl Read,
) -> Result<(String, Option<String>), CallbackRejection> {
    let mut buffer = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        if buffer.len() >= MAX_REQUEST_HEAD_BYTES {
            return Err(CallbackRejection::RequestTooLarge);
        }
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return Err(CallbackRejection::Malformed),
            Ok(_) => {
                buffer.push(byte[0]);
                if buffer.ends_with(b"\r\n") {
                    break;
                }
            }
        }
    }
    let host = read_headers_capture_host(stream, buffer.len())?;
    let line = String::from_utf8(buffer).map_err(|_| CallbackRejection::Malformed)?;
    Ok((line, host))
}

/// `redirect_contract.host_authority_validation =
/// exact_pending_listener_authority_required`: the callback request's `Host`
/// header must exactly equal the authority this listener itself bound
/// (`127.0.0.1:<ephemeral port>`), which is fully known at runtime. A
/// missing, duplicated, or mismatched `Host` fails closed. (The still-gated
/// G-ID decisions — `localhost` versus `127.0.0.1` registration, IPv6, exact
/// registered path — do not defer this check: whatever authority is bound,
/// the header must match it.)
fn validate_host(host: Option<&str>, expected_authority: &str) -> Result<(), CallbackRejection> {
    match host {
        Some(value) if value.eq_ignore_ascii_case(expected_authority) => Ok(()),
        _ => Err(CallbackRejection::WrongHost),
    }
}

/// Reads header lines up to the terminating blank line under the same bound
/// as the request line, capturing exactly the `Host` header's value for
/// [`validate_host`]. A duplicated `Host` header yields `None` (fails
/// closed downstream); all other header content is discarded uninterpreted.
/// Bounded reading still applies to the whole request head so a hostile
/// client cannot hold the listener open indefinitely.
fn read_headers_capture_host(
    stream: &mut impl Read,
    already_read: usize,
) -> Result<Option<String>, CallbackRejection> {
    let mut total = already_read;
    let mut line: Vec<u8> = Vec::with_capacity(128);
    let mut host: Option<String> = None;
    let mut duplicate_host = false;
    let mut byte = [0u8; 1];
    loop {
        if total >= MAX_REQUEST_HEAD_BYTES {
            return Err(CallbackRejection::RequestTooLarge);
        }
        match stream.read(&mut byte) {
            // A client that never sends the trailing blank line (or goes
            // idle, or disconnects) does not get to hold the listener open
            // forever; whatever `Host` was (or was not) seen by then is the
            // final answer and the strict validation happens downstream.
            Ok(0) | Err(_) => break,
            Ok(_) => {
                total += 1;
                line.push(byte[0]);
                if line.ends_with(b"\r\n") {
                    if line.len() == 2 {
                        break;
                    }
                    if let Ok(text) = core::str::from_utf8(&line[..line.len() - 2])
                        && let Some((name, value)) = text.split_once(':')
                        && name.trim().eq_ignore_ascii_case("host")
                    {
                        if host.is_some() {
                            duplicate_host = true;
                        } else {
                            host = Some(value.trim().to_owned());
                        }
                    }
                    line.clear();
                }
            }
        }
    }
    if duplicate_host {
        return Ok(None);
    }
    Ok(host)
}

fn parse_request_line(line: &str) -> Result<CallbackOutcome, CallbackRejection> {
    let line = line
        .strip_suffix("\r\n")
        .ok_or(CallbackRejection::Malformed)?;
    let mut parts = line.split(' ');
    let method = parts.next().ok_or(CallbackRejection::Malformed)?;
    let target = parts.next().ok_or(CallbackRejection::Malformed)?;
    let version = parts.next().ok_or(CallbackRejection::Malformed)?;
    if parts.next().is_some() {
        return Err(CallbackRejection::Malformed);
    }
    if !version.starts_with("HTTP/1.") {
        return Err(CallbackRejection::Malformed);
    }
    if method != "GET" {
        return Err(CallbackRejection::WrongMethod);
    }
    // A conforming client never sends a fragment to the server, but reject
    // defensively rather than silently drop it (`fragment_response:
    // prohibited`).
    if target.contains('#') {
        return Err(CallbackRejection::Malformed);
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != CALLBACK_PATH {
        return Err(CallbackRejection::WrongPath);
    }
    parse_query(query)
}

fn parse_query(query: &str) -> Result<CallbackOutcome, CallbackRejection> {
    const KNOWN: [&str; 5] = ["code", "state", "error", "error_description", "error_uri"];

    if query.is_empty() {
        return Err(CallbackRejection::MissingRequiredParameter);
    }

    let mut code_value: Option<String> = None;
    let mut state_value: Option<String> = None;
    let mut error_value: Option<String> = None;
    let mut seen_keys: Vec<&str> = Vec::with_capacity(8);

    for (count, pair) in query.split('&').enumerate() {
        if count >= MAX_QUERY_PARAMETERS {
            return Err(CallbackRejection::Malformed);
        }
        let mut split = pair.splitn(2, '=');
        let key = split.next().ok_or(CallbackRejection::Malformed)?;
        let raw_value = split.next().ok_or(CallbackRejection::Malformed)?;
        if key.is_empty() {
            return Err(CallbackRejection::Malformed);
        }
        if seen_keys.contains(&key) {
            return Err(CallbackRejection::DuplicateParameter);
        }
        seen_keys.push(key);
        let value = percent_decode(raw_value).ok_or(CallbackRejection::Malformed)?;
        if !KNOWN.contains(&key) {
            // Bounded unrecognized parameter: ignored, never logged or
            // persisted (`redirect_contract.unexpected_parameters`).
            continue;
        }
        match key {
            "code" => code_value = Some(value),
            "state" => state_value = Some(value),
            "error" => error_value = Some(value),
            _ => {}
        }
    }

    let state = state_value.ok_or(CallbackRejection::MissingRequiredParameter)?;
    match (code_value, error_value) {
        (Some(_), Some(_)) => Err(CallbackRejection::MixedSuccessAndError),
        (Some(code), None) => Ok(CallbackOutcome::Success {
            code: AuthorizationCode(code),
            state,
        }),
        (None, Some(error)) => Ok(CallbackOutcome::Failure {
            error: AuthorizationErrorCode::parse(&error),
            state,
        }),
        (None, None) => Err(CallbackRejection::MissingRequiredParameter),
    }
}

fn respond(stream: &mut impl Write, accepted: bool) {
    let body = if accepted {
        "Sign-in received. You may close this window and return to the application."
    } else {
        "Sign-in could not be completed. You may close this window."
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::{
        CallbackRejection, LoopbackListener, parse_request_line, read_headers_capture_host,
        validate_host,
    };
    use std::net::TcpListener;

    // --- Host-authority validation
    // (`redirect_contract.host_authority_validation`).

    #[test]
    fn exact_bound_authority_passes_host_validation() {
        assert!(validate_host(Some("127.0.0.1:49152"), "127.0.0.1:49152").is_ok());
        assert!(validate_host(Some("127.0.0.1:49152"), "127.0.0.1:49153").is_err());
    }

    #[test]
    fn missing_wrong_or_rebound_host_fails_closed() {
        for bad in [
            None,
            Some("localhost:49152"),
            Some("attacker.example:49152"),
            Some("127.0.0.1"),
            Some("127.0.0.1:49152.attacker.example"),
        ] {
            assert_eq!(
                validate_host(bad, "127.0.0.1:49152").unwrap_err(),
                CallbackRejection::WrongHost
            );
        }
    }

    #[test]
    fn duplicated_host_header_is_captured_as_none_and_fails_closed() {
        let head =
            b"Host: 127.0.0.1:49152\r\nHost: attacker.example\r\nAccept: */*\r\n\r\n".to_vec();
        let mut cursor = std::io::Cursor::new(head);
        let host = read_headers_capture_host(&mut cursor, 0).unwrap();
        assert!(host.is_none());
        assert_eq!(
            validate_host(host.as_deref(), "127.0.0.1:49152").unwrap_err(),
            CallbackRejection::WrongHost
        );
    }

    #[test]
    fn single_host_header_is_captured_case_insensitively() {
        let head = b"hOsT:  127.0.0.1:49152 \r\nAccept: */*\r\n\r\n".to_vec();
        let mut cursor = std::io::Cursor::new(head);
        let host = read_headers_capture_host(&mut cursor, 0).unwrap();
        assert_eq!(host.as_deref(), Some("127.0.0.1:49152"));
        assert!(validate_host(host.as_deref(), "127.0.0.1:49152").is_ok());
    }

    // --- Pure request-line/query parsing: the entire `redirect_contract`
    // failure matrix, exercised directly against synthetic text. This is
    // the identical code path `LoopbackListener::accept_one` calls after a
    // real `accept()`; see this module's doc comment for why no test here
    // dials a real client connection. ---

    #[test]
    fn accepts_exactly_one_well_formed_success_callback() {
        let outcome = parse_request_line(
            "GET /callback?code=synthetic-code&state=synthetic-state HTTP/1.1\r\n",
        )
        .expect("well-formed callback");
        match outcome {
            super::CallbackOutcome::Success { code, state } => {
                assert_eq!(code.as_str(), "synthetic-code");
                assert_eq!(state, "synthetic-state");
            }
            super::CallbackOutcome::Failure { .. } => panic!("expected success"),
        }
    }

    #[test]
    fn accepts_exactly_one_well_formed_failure_callback() {
        let outcome = parse_request_line(
            "GET /callback?error=access_denied&state=synthetic-state HTTP/1.1\r\n",
        )
        .expect("well-formed callback");
        assert_eq!(outcome.state(), "synthetic-state");
        assert!(matches!(outcome, super::CallbackOutcome::Failure { .. }));
    }

    #[test]
    fn rejects_mixed_success_and_error() {
        assert_eq!(
            parse_request_line("GET /callback?code=c&state=s&error=access_denied HTTP/1.1\r\n")
                .unwrap_err(),
            CallbackRejection::MixedSuccessAndError
        );
    }

    #[test]
    fn rejects_duplicate_parameters() {
        assert_eq!(
            parse_request_line("GET /callback?code=c&code=c2&state=s HTTP/1.1\r\n").unwrap_err(),
            CallbackRejection::DuplicateParameter
        );
    }

    #[test]
    fn rejects_missing_state() {
        assert_eq!(
            parse_request_line("GET /callback?code=c HTTP/1.1\r\n").unwrap_err(),
            CallbackRejection::MissingRequiredParameter
        );
    }

    #[test]
    fn rejects_missing_code_and_error() {
        assert_eq!(
            parse_request_line("GET /callback?state=s HTTP/1.1\r\n").unwrap_err(),
            CallbackRejection::MissingRequiredParameter
        );
    }

    #[test]
    fn rejects_wrong_path() {
        assert_eq!(
            parse_request_line("GET /not-the-callback?code=c&state=s HTTP/1.1\r\n").unwrap_err(),
            CallbackRejection::WrongPath
        );
    }

    #[test]
    fn rejects_wrong_method() {
        assert_eq!(
            parse_request_line("POST /callback?code=c&state=s HTTP/1.1\r\n").unwrap_err(),
            CallbackRejection::WrongMethod
        );
    }

    #[test]
    fn rejects_fragment_in_request_target() {
        assert_eq!(
            parse_request_line("GET /callback?code=c&state=s#frag HTTP/1.1\r\n").unwrap_err(),
            CallbackRejection::Malformed
        );
    }

    #[test]
    fn rejects_malformed_request_line() {
        assert_eq!(
            parse_request_line("this is not a request line\r\n").unwrap_err(),
            CallbackRejection::Malformed
        );
    }

    #[test]
    fn rejects_request_line_missing_the_trailing_crlf() {
        assert_eq!(
            parse_request_line("GET /callback?code=c&state=s HTTP/1.1").unwrap_err(),
            CallbackRejection::Malformed
        );
    }

    #[test]
    fn bounded_unknown_parameters_are_ignored_not_rejected() {
        let outcome = parse_request_line(
            "GET /callback?code=c&state=s&session_state=x&admin_consent=y HTTP/1.1\r\n",
        )
        .expect("well-formed callback despite unknown bounded parameters");
        assert!(matches!(outcome, super::CallbackOutcome::Success { .. }));
    }

    // --- Real, OS-level `TcpListener` bind/lifecycle coverage (no client
    // connection needed for any of these). ---

    #[test]
    fn bind_assigns_a_nonzero_ephemeral_port() {
        let listener = LoopbackListener::bind().expect("bind loopback listener");
        assert_ne!(listener.port(), 0);
    }

    #[test]
    fn two_simultaneously_bound_listeners_get_independent_ports() {
        let first = LoopbackListener::bind().expect("bind first listener");
        let second = LoopbackListener::bind().expect("bind second listener");
        assert_ne!(first.port(), second.port());
    }

    #[test]
    fn dropping_an_unaccepted_listener_releases_its_port() {
        let listener = LoopbackListener::bind().expect("bind loopback listener");
        let port = listener.port();
        drop(listener);
        // If the OS had not released the listening socket, this bind would
        // fail with "address in use". `accept_one` drops the identical
        // `TcpListener` field the instant it returns (see its doc comment),
        // so this is the same release path a completed callback goes
        // through, demonstrated without needing a client to complete one.
        let rebound = TcpListener::bind(("127.0.0.1", port));
        assert!(
            rebound.is_ok(),
            "the OS must have released the port once the listener was dropped"
        );
    }
}
