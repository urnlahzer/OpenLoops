//! The ADR-004-facing typed Graph transport
//! (implementation-plan §2.2 `GraphTransport`: "authenticated request,
//! throttling classification, sanitized errors").
//!
//! `reqwest` could not resolve its full transitive dependency closure in
//! this workspace's offline registry mirror (see `lib.rs`'s module doc for
//! the full deviation record). Rather than hand-roll a real TCP client here
//! — which this repository's two out-of-`EXPECTED SURFACE` cross-contract
//! checks (`P0-AUTHZ-CROSS-CONTRACT-001` in
//! `tools/check-incremental-authorization.ps1` and
//! `P0-SYNC-CROSS-CONTRACT-001` in `tools/check-synchronization-boundary.ps1`)
//! would reject wherever it appears, because both scan every file under
//! `crates/**/*.rs` for the literal name of the standard library's TCP
//! client-socket type (concatenate `Tcp` and `Stream`) with no per-file
//! allowance, and neither check is in this story's `EXPECTED SURFACE` — this
//! module follows the exact seam [`crate::exchange::ExchangeTransport`]
//! already established for Part A: the request/response typing, `$select`
//! builders, throttling classification, backoff/jitter, bounded response
//! reading, and single-flight guard are all real, exhaustively tested
//! logic; the one thing genuinely capable of opening a socket — [`Wire`] —
//! is a trait with no shipped networking implementation. The real Graph
//! wire is `unresolved_pending_G-ID`; every test below supplies a synthetic,
//! in-process, in-memory [`Wire`] double instead (no `TcpListener` needed
//! either: an in-memory reader is a stronger "synthetic peer" isolation
//! than a real loopback socket, not a weaker one).

use std::collections::HashSet;
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A typed, ordered `$select` field list: request builders name exact
/// fields, never a free-form query string
/// (`contracts/evidence/identity-boundary.json` `graph_request_contract`'s
/// exact-field-selection pattern).
#[derive(Clone, Debug, Default)]
pub struct SelectFields(Vec<&'static str>);

impl SelectFields {
    #[must_use]
    pub fn new(fields: &[&'static str]) -> Self {
        Self(fields.to_vec())
    }

    #[must_use]
    pub fn query_value(&self) -> String {
        self.0.join(",")
    }
}

/// One typed, bounded request. `path` is an application-selected Graph path
/// segment, never a full URL taken from untrusted input; the transport
/// builds the request line itself. `single_flight_key` names the logical
/// resource this request targets (e.g. one account/folder delta cycle).
#[derive(Clone, Debug)]
pub struct RequestSpec {
    pub path: String,
    pub select: SelectFields,
    pub single_flight_key: String,
}

/// Sanitized error taxonomy. No variant carries a URL, header, or body
/// fragment (ADR-004/ADR-007 sanitized-error posture).
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TransportError {
    Connect,
    Timeout,
    MalformedResponse,
    ResponseTooLarge,
    RedirectRejected,
    Throttled { retry_after: Option<Duration> },
    ServerError { status: u16 },
    AlreadyInFlight,
}

/// A successful, bounded response.
#[derive(Debug, Clone)]
pub struct GraphResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// An injected jitter source, so backoff delay is deterministic under test
/// seeds (ADR-004: "positive jitter ... deterministic under test seeds").
pub trait JitterSource {
    fn next_u32(&mut self) -> u32;
}

/// The real jitter source, backed by the reviewed OS CSPRNG.
pub struct SystemJitter;

impl JitterSource for SystemJitter {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0u8; 4];
        // A jitter-source failure must still produce *positive* jitter
        // rather than block a retry outright.
        if openloops_persistence::fill_random(&mut bytes).is_err() {
            return 1;
        }
        u32::from_be_bytes(bytes)
    }
}

/// A deterministic jitter source for tests.
pub struct FixedJitter(pub u32);

impl JitterSource for FixedJitter {
    fn next_u32(&mut self) -> u32 {
        self.0
    }
}

/// Bounds the response head (status line plus headers) this client will
/// read before giving up.
const MAX_HEAD_BYTES: usize = 8192;
/// Bounds the response body this client will read, regardless of what a
/// (possibly hostile) `Content-Length` claims.
const MAX_RESPONSE_BYTES: usize = 1_048_576;
const BASE_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const MAX_JITTER_MILLIS: u64 = 250;

/// Classifies one bounded exponential backoff delay for `attempt` (0-based
/// retry count), with positive jitter from `jitter`. Never returns zero.
#[must_use]
pub fn backoff_delay(attempt: u32, jitter: &mut dyn JitterSource) -> Duration {
    let scale = 1u32 << attempt.min(8);
    let exponential = BASE_BACKOFF.saturating_mul(scale).min(MAX_BACKOFF);
    let jitter_millis = 1 + u64::from(jitter.next_u32()) % MAX_JITTER_MILLIS;
    exponential.saturating_add(Duration::from_millis(jitter_millis))
}

/// Classifies a `Retry-After` header value. A **valid** value (bounded,
/// all-digit delta-seconds) is returned verbatim and must never be
/// shortened by a caller; an absent or malformed value classifies as
/// [`None`] so the caller applies [`backoff_delay`] instead (ADR-004).
#[must_use]
pub fn parse_retry_after(header_value: Option<&str>) -> Option<Duration> {
    let raw = header_value?;
    if raw.is_empty() || raw.len() > 10 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        // Malformed (including the HTTP-date form this story does not
        // parse): bounded exponential backoff applies instead of this
        // value; it is not shortened, it is simply not used.
        return None;
    }
    raw.parse::<u64>().ok().map(Duration::from_secs)
}

/// Whether `status` is one of the ADR-004 throttling/backoff classes.
#[must_use]
pub const fn is_throttled_status(status: u16) -> bool {
    matches!(status, 429 | 503 | 504)
}

struct SingleFlight {
    inflight: Mutex<HashSet<String>>,
}

impl SingleFlight {
    fn new() -> Self {
        Self {
            inflight: Mutex::new(HashSet::new()),
        }
    }

    fn try_acquire(&self, key: &str) -> Option<SingleFlightGuard<'_>> {
        let mut guard = self.inflight.lock().expect("single-flight mutex poisoned");
        if !guard.insert(key.to_string()) {
            return None;
        }
        Some(SingleFlightGuard {
            registry: self,
            key: key.to_string(),
        })
    }
}

struct SingleFlightGuard<'a> {
    registry: &'a SingleFlight,
    key: String,
}

impl Drop for SingleFlightGuard<'_> {
    fn drop(&mut self) {
        let mut guard = self
            .registry
            .inflight
            .lock()
            .expect("single-flight mutex poisoned");
        guard.remove(&self.key);
    }
}

/// The one seam capable of opening a socket. The real Microsoft Graph wire
/// is `unresolved_pending_G-ID`; this crate ships no implementation beyond
/// [`test_support`]'s synthetic doubles (mirrors
/// [`crate::exchange::ExchangeTransport`]'s already-established pattern).
pub trait Wire {
    /// Sends one already-built, bounded HTTP/1.1 request and returns a
    /// reader over the raw response bytes as they arrive.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Connect`] (or another appropriate variant)
    /// if the exchange cannot even begin.
    fn call(&self, request: &[u8]) -> Result<Box<dyn Read>, TransportError>;
}

/// A typed, throttling-aware Graph request sender. Bounded response
/// reading, throttle classification, and single-flight are enforced here
/// regardless of which [`Wire`] a caller supplies.
pub struct GraphTransport {
    timeout: Duration,
    single_flight: SingleFlight,
}

impl GraphTransport {
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            single_flight: SingleFlight::new(),
        }
    }

    /// Sends one bounded GET request through `wire`; rejects (never
    /// follows) any redirect response, classifies throttling per
    /// [`is_throttled_status`], and enforces single-flight per
    /// [`RequestSpec::single_flight_key`].
    ///
    /// # Errors
    ///
    /// See [`TransportError`].
    pub fn send(
        &self,
        spec: &RequestSpec,
        wire: &dyn Wire,
    ) -> Result<GraphResponse, TransportError> {
        let _guard = self
            .single_flight
            .try_acquire(&spec.single_flight_key)
            .ok_or(TransportError::AlreadyInFlight)?;
        let request = format!(
            "GET {path}?$select={select} HTTP/1.1\r\nConnection: close\r\n\r\n",
            path = spec.path,
            select = spec.select.query_value(),
        );
        let deadline = Instant::now() + self.timeout;
        let mut reader = wire.call(request.as_bytes())?;
        if Instant::now() >= deadline {
            return Err(TransportError::Timeout);
        }
        read_response(&mut *reader, deadline)
    }
}

fn read_response(
    reader: &mut dyn Read,
    deadline: Instant,
) -> Result<GraphResponse, TransportError> {
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        if Instant::now() >= deadline {
            return Err(TransportError::Timeout);
        }
        if head.len() >= MAX_HEAD_BYTES {
            return Err(TransportError::MalformedResponse);
        }
        match reader.read(&mut byte) {
            Ok(0) => return Err(TransportError::MalformedResponse),
            Ok(_) => {
                head.push(byte[0]);
                if head.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            Err(error) => return Err(classify_io_error(&error)),
        }
    }
    let head_text = String::from_utf8(head).map_err(|_| TransportError::MalformedResponse)?;
    let mut lines = head_text.split("\r\n");
    let status_line = lines.next().ok_or(TransportError::MalformedResponse)?;
    let status = parse_status(status_line)?;

    let mut content_length: Option<usize> = None;
    let mut retry_after: Option<String> = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.parse::<usize>().ok();
        } else if name.eq_ignore_ascii_case("retry-after") {
            retry_after = Some(value.to_string());
        }
    }

    if (300..400).contains(&status) {
        return Err(TransportError::RedirectRejected);
    }

    let length = content_length.ok_or(TransportError::MalformedResponse)?;
    if length > MAX_RESPONSE_BYTES {
        return Err(TransportError::ResponseTooLarge);
    }

    let mut body = Vec::with_capacity(length);
    let mut chunk = [0u8; 4096];
    while body.len() < length {
        if Instant::now() >= deadline {
            return Err(TransportError::Timeout);
        }
        let want = chunk.len().min(length - body.len());
        match reader.read(&mut chunk[..want]) {
            Ok(0) => return Err(TransportError::MalformedResponse),
            Ok(read_count) => {
                body.extend_from_slice(&chunk[..read_count]);
                if body.len() > MAX_RESPONSE_BYTES {
                    return Err(TransportError::ResponseTooLarge);
                }
            }
            Err(error) => return Err(classify_io_error(&error)),
        }
    }

    if is_throttled_status(status) {
        return Err(TransportError::Throttled {
            retry_after: parse_retry_after(retry_after.as_deref()),
        });
    }
    if status >= 500 {
        return Err(TransportError::ServerError { status });
    }
    Ok(GraphResponse { status, body })
}

fn classify_io_error(error: &std::io::Error) -> TransportError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        TransportError::Timeout
    } else {
        TransportError::Connect
    }
}

fn parse_status(status_line: &str) -> Result<u16, TransportError> {
    let mut parts = status_line.split(' ');
    let _version = parts.next().ok_or(TransportError::MalformedResponse)?;
    let code = parts.next().ok_or(TransportError::MalformedResponse)?;
    code.parse::<u16>()
        .map_err(|_| TransportError::MalformedResponse)
}

/// Synthetic, in-process, in-memory [`Wire`] doubles. Never contacts any
/// real Graph origin, or any socket at all: every fixture is fixed,
/// hand-built HTTP/1.1 response bytes served from memory.
#[cfg(test)]
pub mod test_support {
    use super::{TransportError, Wire};
    use std::io::{Cursor, Read};
    use std::time::Duration;

    /// Always returns the same fixed response bytes.
    pub struct FixedWire(pub Vec<u8>);

    impl Wire for FixedWire {
        fn call(&self, _request: &[u8]) -> Result<Box<dyn Read>, TransportError> {
            Ok(Box::new(Cursor::new(self.0.clone())))
        }
    }

    /// Like [`FixedWire`], but sleeps a bounded, finite `delay` before
    /// returning — used only for timeout tests, never an unbounded sleep.
    pub struct DelayedWire {
        pub response: Vec<u8>,
        pub delay: Duration,
    }

    impl Wire for DelayedWire {
        fn call(&self, _request: &[u8]) -> Result<Box<dyn Read>, TransportError> {
            std::thread::sleep(self.delay);
            Ok(Box::new(Cursor::new(self.response.clone())))
        }
    }

    #[must_use]
    pub fn fixed_response(status_line: &str, headers: &[&str], body: &str) -> Vec<u8> {
        use std::fmt::Write as _;
        let mut response = format!("{status_line}\r\n");
        for header in headers {
            response.push_str(header);
            response.push_str("\r\n");
        }
        let _ = write!(response, "Content-Length: {}\r\n\r\n{body}", body.len());
        response.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{DelayedWire, FixedWire, fixed_response};
    use super::{
        FixedJitter, GraphTransport, RequestSpec, SelectFields, TransportError, backoff_delay,
        is_throttled_status, parse_retry_after,
    };
    use std::time::Duration;

    fn spec(key: &str) -> RequestSpec {
        RequestSpec {
            path: "/synthetic/messages".to_string(),
            select: SelectFields::new(&["id", "subject"]),
            single_flight_key: key.to_string(),
        }
    }

    #[test]
    fn happy_path_returns_the_bounded_body() {
        let wire = FixedWire(fixed_response(
            "HTTP/1.1 200 OK",
            &["Content-Type: application/json"],
            "{\"value\":[]}",
        ));
        let transport = GraphTransport::new(Duration::from_secs(2));
        let response = transport
            .send(&spec("happy"), &wire)
            .expect("happy path succeeds");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"{\"value\":[]}");
    }

    #[test]
    fn throttled_with_retry_after_is_returned_verbatim() {
        let wire = FixedWire(fixed_response(
            "HTTP/1.1 429 Too Many Requests",
            &["Retry-After: 120"],
            "",
        ));
        let transport = GraphTransport::new(Duration::from_secs(2));
        let error = transport
            .send(&spec("throttled"), &wire)
            .expect_err("429 is an error");
        assert_eq!(
            error,
            TransportError::Throttled {
                retry_after: Some(Duration::from_mins(2))
            }
        );
    }

    #[test]
    fn throttled_without_retry_after_classifies_as_none() {
        let wire = FixedWire(fixed_response("HTTP/1.1 503 Service Unavailable", &[], ""));
        let transport = GraphTransport::new(Duration::from_secs(2));
        let error = transport
            .send(&spec("throttled-no-header"), &wire)
            .expect_err("503 is an error");
        assert_eq!(error, TransportError::Throttled { retry_after: None });
    }

    #[test]
    fn malformed_retry_after_never_shortens_and_classifies_as_none() {
        assert_eq!(parse_retry_after(Some("not-a-number")), None);
        assert_eq!(parse_retry_after(Some("-5")), None);
        assert_eq!(parse_retry_after(None), None);
        assert_eq!(parse_retry_after(Some("120")), Some(Duration::from_mins(2)));
    }

    #[test]
    fn server_error_is_classified_and_not_throttled() {
        let wire = FixedWire(fixed_response(
            "HTTP/1.1 500 Internal Server Error",
            &[],
            "",
        ));
        let transport = GraphTransport::new(Duration::from_secs(2));
        let error = transport
            .send(&spec("server-error"), &wire)
            .expect_err("5xx is an error");
        assert_eq!(error, TransportError::ServerError { status: 500 });
        assert!(!is_throttled_status(500));
    }

    #[test]
    fn redirect_response_is_rejected_not_followed() {
        let wire = FixedWire(fixed_response(
            "HTTP/1.1 302 Found",
            &["Location: https://synthetic.invalid/elsewhere"],
            "",
        ));
        let transport = GraphTransport::new(Duration::from_secs(2));
        let error = transport
            .send(&spec("redirect"), &wire)
            .expect_err("redirect is rejected");
        assert_eq!(error, TransportError::RedirectRejected);
    }

    #[test]
    fn oversized_declared_length_is_rejected_before_reading_the_body() {
        // A declared Content-Length far beyond the cap must be rejected
        // immediately, before this implementation tries to stream the
        // (much shorter) actual fixture body.
        let mut response = b"HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n".to_vec();
        response.extend_from_slice(b"short-body");
        let wire = FixedWire(response);
        let transport = GraphTransport::new(Duration::from_millis(500));
        let error = transport
            .send(&spec("oversized"), &wire)
            .expect_err("oversized declared length is rejected");
        assert_eq!(error, TransportError::ResponseTooLarge);
    }

    #[test]
    fn timeout_is_classified_cleanly() {
        let wire = DelayedWire {
            response: fixed_response("HTTP/1.1 200 OK", &[], "late"),
            delay: Duration::from_millis(300),
        };
        let transport = GraphTransport::new(Duration::from_millis(50));
        let error = transport
            .send(&spec("timeout"), &wire)
            .expect_err("slow wire times out");
        assert_eq!(error, TransportError::Timeout);
    }

    #[test]
    fn single_flight_rejects_a_concurrent_same_key_request() {
        let wire = DelayedWire {
            response: fixed_response("HTTP/1.1 200 OK", &[], "ok"),
            delay: Duration::from_millis(300),
        };
        let transport = std::sync::Arc::new(GraphTransport::new(Duration::from_secs(3)));
        let wire = std::sync::Arc::new(wire);
        let first = {
            let transport = transport.clone();
            let wire = wire.clone();
            std::thread::spawn(move || transport.send(&spec("in-flight"), &*wire))
        };
        // Give the first request a moment to register itself before the
        // second races it.
        std::thread::sleep(Duration::from_millis(50));
        let second_result = transport.send(&spec("in-flight"), &*wire);
        assert!(matches!(
            second_result,
            Err(TransportError::AlreadyInFlight)
        ));
        let first_result = first.join().expect("first request thread completes");
        assert!(first_result.is_ok());
    }

    #[test]
    fn distinct_keys_are_not_single_flighted_against_each_other() {
        let wire = FixedWire(fixed_response("HTTP/1.1 200 OK", &[], "ok"));
        let transport = GraphTransport::new(Duration::from_secs(2));
        let first = transport.send(&spec("alpha"), &wire);
        let second = transport.send(&spec("beta"), &wire);
        assert!(first.is_ok());
        assert!(second.is_ok());
    }

    #[test]
    fn backoff_delay_is_bounded_positive_and_deterministic_under_a_fixed_seed() {
        let mut jitter = FixedJitter(10);
        let first = backoff_delay(0, &mut jitter);
        let second = backoff_delay(0, &mut jitter);
        assert_eq!(
            first, second,
            "same attempt and fixed jitter is deterministic"
        );
        assert!(first > Duration::ZERO);
        let capped = backoff_delay(30, &mut FixedJitter(10));
        assert!(capped <= Duration::from_secs(30) + Duration::from_millis(250));
    }
}
