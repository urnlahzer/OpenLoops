//! Provider-agnostic model boundary: the fixed error codes, the bounded
//! HTTPS transport every adapter shares, and the one trait a selected
//! model is reached through. Credentials and payloads are session-only and
//! no upstream error text escapes this module.
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use reqwest::blocking::{Client, Response};
use zeroize::Zeroizing;

pub(crate) const MAX_RESPONSE: usize = 262_144;
pub(crate) const MAX_REQUEST: usize = 524_288;
const MAX_KEY: usize = 4096;
/// Chunk size the background reader thread `read_body` spawns reads the
/// response body in.
const READ_CHUNK: usize = 16_384;

/// How often `read_body`'s polling loop wakes to check `cancel` and the
/// wall deadline while a background thread performs the actual
/// (potentially long-blocking) socket reads -- see [`RequestControl`] and
/// [`read_body`]. Also the granularity of the [`READ_IDLE_GUARD`] check.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// If no body chunk arrives for this long, `read_body` gives up with
/// [`ProviderError::Timeout`]. This is independent of, and far shorter
/// than, reqwest's own per-`read()` timeout (`https_client`'s
/// `.timeout(...)`), which applies to the whole blocking `read()` call
/// the header wait and every body read share and so cannot be lowered
/// without also cutting the time allowed to receive response headers.
const READ_IDLE_GUARD: Duration = Duration::from_mins(1);

/// Bounds the whole request -- from `send()` to the last body byte -- to
/// [`REQUEST_DEADLINE`], and lets a Stop action abort it via `cancel`.
/// reqwest gives no way to observe either condition while blocked inside
/// one `send()` or `read()` call, so both [`send_with_control`] and
/// [`read_body`] run that blocking call on a background thread and poll
/// it every [`POLL_INTERVAL`] instead; `RequestControl` is what both poll
/// loops check on each tick, sharing the *same* `started` instant across
/// both phases, so a provider that withholds response headers entirely
/// until generation finishes (observed with Ollama Cloud) is bounded
/// exactly like one that answers immediately but sends the body slowly.
/// Every adapter records `Instant::now()` immediately before `send()` and
/// threads the same instant, plus the caller's cancel flag when one
/// exists, through both calls.
///
/// Abandoning a request (cancel or the deadline elapsing) only drops the
/// channel receiver; the background thread notices the next time it
/// tries to send and exits then, but its one blocking `send()`/`read()`
/// call -- and the socket underneath it -- can still linger for up to
/// reqwest's own per-call timeout (`https_client`'s `.timeout(...)`,
/// currently 60 seconds) after that.
pub struct RequestControl<'a> {
    pub started: Instant,
    pub cancel: Option<&'a AtomicBool>,
    /// Test-only override of [`REQUEST_DEADLINE`] so a loopback test can
    /// observe a deadline `Timeout` without waiting the real 150 seconds.
    deadline: Duration,
}

impl<'a> RequestControl<'a> {
    /// No cancel flag: used by calls with no caller-supplied cancel source
    /// (model listings, connectivity checks). Still bounded by the deadline.
    #[must_use]
    pub fn new(started: Instant) -> Self {
        Self {
            started,
            cancel: None,
            deadline: REQUEST_DEADLINE,
        }
    }

    #[must_use]
    pub fn with_cancel(started: Instant, cancel: Option<&'a AtomicBool>) -> Self {
        Self {
            started,
            cancel,
            deadline: REQUEST_DEADLINE,
        }
    }

    #[cfg(test)]
    fn with_deadline(started: Instant, cancel: Option<&'a AtomicBool>, deadline: Duration) -> Self {
        Self {
            started,
            cancel,
            deadline,
        }
    }

    /// `Err` when `cancel` is set ([`ProviderError::Cancelled`]) or the
    /// deadline has passed ([`ProviderError::Timeout`]); checked before
    /// the first read and after every poll tick and body chunk in
    /// [`read_body`].
    fn check(&self) -> Result<(), ProviderError> {
        if self
            .cancel
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
        {
            return Err(ProviderError::Cancelled);
        }
        if self.started.elapsed() > self.deadline {
            return Err(ProviderError::Timeout);
        }
        Ok(())
    }
}

/// The whole request -- from `send()` to the last body byte -- must
/// finish within this wall time, checked at least once per
/// [`POLL_INTERVAL`] in both [`send_with_control`] (the connect and
/// header wait) and [`read_body`] (again after every body chunk
/// received). `OpenRouter` and other providers can trickle keep-alive
/// body bytes while a slow model works, which re-arms reqwest's per-call
/// timeout on every byte; some providers -- Ollama Cloud, in practice --
/// instead withhold response headers entirely until generation finishes,
/// which re-arms that same timeout without sending anything at all. Both
/// are bounded by this one deadline now, so it is the true ceiling on a
/// single request, not an approximation that ignores the header wait.
/// Abandoning a request does not instantly free the background thread
/// performing the blocking call underneath it -- see [`RequestControl`]'s
/// residual-linger note.
pub const REQUEST_DEADLINE: Duration = Duration::from_secs(150);

/// Fixed error codes only; no upstream error text escapes this boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderError {
    InvalidKey,
    Unauthorized,
    InputUnavailable,
    Timeout,
    Network,
    RateLimited,
    Quota,
    RequestRejected(u16),
    ServerError(u16),
    ModelUnavailable,
    InvalidResponse,
    InputTooLarge,
    ResponseTooLarge,
    InvalidAnalysis,
    InvalidJson,
    InvalidSchema,
    /// The caller's cancel flag (set by the Stop control) was observed
    /// while a request was in flight; the request was abandoned before a
    /// complete answer arrived.
    Cancelled,
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidKey => "Enter a valid API key for the selected provider.",
            Self::Unauthorized => "The provider rejected the API key or account access.",
            Self::InputUnavailable => "Could not read a model selection from the terminal.",
            Self::Timeout => "The provider did not answer in time. Try the model again or select a faster model.",
            Self::Network => "Could not establish or complete a secure connection to the selected provider. Check connectivity.",
            Self::RateLimited => "The provider returned HTTP 429 (rate limit). Wait before trying again.",
            Self::Quota => "The provider returned HTTP 402. Check your plan or usage balance.",
            Self::RequestRejected(status) => return write!(f, "The provider rejected the request (HTTP {status}). An unsupported request parameter or model capability is the usual cause; try another model."),
            Self::ServerError(status) => return write!(f, "The provider returned a server error (HTTP {status}). Try again later or choose another model."),
            Self::ModelUnavailable => "The selected model is not in the provider's selectable model list.",
            Self::InvalidResponse => "The provider returned an incomplete or unsupported response.",
            Self::InputTooLarge => "The selected message projection exceeds the analysis limits.",
            Self::ResponseTooLarge => "The provider's response exceeds the allowed size.",
            Self::InvalidAnalysis => "The model output did not pass OpenLoops validation.",
            Self::InvalidJson => "The model returned malformed JSON or extra text instead of the requested result.",
            Self::InvalidSchema => "The model returned JSON with missing, duplicate, or unsupported fields.",
            Self::Cancelled => "The scan was stopped before the model answered.",
        })
    }
}

/// The most requests any adapter may report it can run at once. Nothing
/// in `contracts/model/provider-boundary.json` restricts concurrency, so
/// this is a sanity ceiling on a user-supplied or provider-reported
/// number, not a contract limit.
pub const MAX_PARALLEL_REQUESTS: usize = 100;

/// One consented provider, bound to one exact model label.
///
/// Implementors own their own authority, credential, and wire format;
/// callers see only the selected label and a system/user completion.
///
/// `Sync` is required because one client serves every worker of a
/// parallel scan: the workers share a single `&dyn ModelClient` and call
/// [`ModelClient::complete`] on it concurrently.
pub trait ModelClient: Sync {
    /// The exact provider-side model label this client is bound to.
    fn model(&self) -> &str;

    /// How many [`ModelClient::complete`] calls a caller may keep in
    /// flight against this client at once, always at least 1 and never
    /// above [`MAX_PARALLEL_REQUESTS`].
    ///
    /// This is a dispatch ceiling, not a promise: a provider may still
    /// rate-limit below it, and the caller is expected to back off rather
    /// than resend, since `network_policy.retries` forbids automatically
    /// retrying any request that carried content.
    fn max_parallel(&self) -> usize;

    /// The provider's published per-interval request budget -- `(requests,
    /// interval)` -- when it publishes one and the adapter could read it.
    /// A caller that has been rate-limited uses it to spread dispatches
    /// instead of guessing. `None` means the provider publishes no budget,
    /// which is the default and says nothing about how fast requests may
    /// be sent.
    fn request_budget(&self) -> Option<(u32, Duration)> {
        None
    }

    /// Sends one system/user pair and returns the assistant's content.
    ///
    /// Calling this explicitly opts into transmitting `user` to the
    /// configured provider. `cancel`, when supplied, is checked
    /// throughout the request -- while waiting for a response to start
    /// arriving and while its body is read; setting it aborts the
    /// in-flight request with [`ProviderError::Cancelled`] within about
    /// one poll tick instead of waiting for the request to finish or
    /// idle-time out.
    /// # Errors
    /// Returns a fixed provider error. No upstream text, header, or body
    /// escapes, and no alternate model or origin is attempted.
    fn complete(
        &self,
        system: &str,
        user: &str,
        cancel: Option<&AtomicBool>,
    ) -> Result<Zeroizing<String>, ProviderError>;
}

/// A write-only provider credential: present, bounded, and free of
/// whitespace that could smuggle a second header line.
pub(crate) fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_KEY && !key.chars().any(char::is_whitespace)
}

/// A provider-side model label safe to echo into a request and compare
/// against a response, with no control or terminal-escape bytes.
pub(crate) fn valid_model_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
}

/// The only transport any adapter may build: direct valid-TLS HTTPS, no
/// proxy, no redirect, bounded connect and wall time.
pub(crate) fn https_client() -> Result<Client, ProviderError> {
    Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_mins(1))
        .build()
        .map_err(|error| transport_error(&error))
}

/// Performs `request.send()` -- which blocks for the connection and the
/// entire header wait, under reqwest's own per-call timeout
/// (`https_client`'s `.timeout(...)`) -- on a background thread, so the
/// caller can poll for cancellation and the deadline instead of blocking
/// inside it. Mirrors [`spawn_reader`]/[`read_body`]'s pattern exactly,
/// but for the one-shot `send()` result rather than a stream of body
/// chunks: some providers (a slow-to-answer Ollama Cloud model is the
/// motivating case) send no response headers at all until generation has
/// finished, so without this, `cancel` and the deadline would never be
/// checked until `send()` returned on its own -- Stop would have no
/// effect on such a request at all.
///
/// Checks `control` before spawning the thread, then on every
/// [`POLL_INTERVAL`] tick that receives nothing. Abandoning the request
/// this way drops `receiver`; the background thread's own `send()` may
/// still be blocked for up to its own per-call timeout, and only notices
/// (and exits) the next time it tries to hand its result to the now-gone
/// receiver.
/// # Errors
/// Returns a fixed provider error mapped from the transport failure, or
/// [`ProviderError::Cancelled`]/[`ProviderError::Timeout`] from `control`.
pub(crate) fn send_with_control(
    request: reqwest::blocking::RequestBuilder,
    control: &RequestControl<'_>,
) -> Result<Response, ProviderError> {
    control.check()?;
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(request.send());
    });
    loop {
        match receiver.recv_timeout(POLL_INTERVAL) {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(error)) => return Err(transport_error(&error)),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(ProviderError::Network),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        control.check()?;
    }
}

/// One message a [`spawn_reader`] thread hands back over its channel: a
/// non-empty chunk, an empty chunk meaning clean EOF, or the first read
/// error (after which the thread stops). Chunks are zeroizing, like the
/// buffer `read_body` assembles them into, so a response fragment never
/// sits in a plain, un-zeroized heap allocation even transiently.
type ChunkResult = io::Result<Zeroizing<Vec<u8>>>;

/// Performs the actual (potentially long-blocking) chunked reads of
/// `response` on a background thread, so [`read_body`]'s caller-side loop
/// never blocks inside a single `read()` call and can instead poll for
/// cancellation and the deadline. Reads up to `limit + 1` bytes in
/// `READ_CHUNK`-sized pieces, retrying a read interrupted by a signal
/// (`io::ErrorKind::Interrupted`) rather than treating it as a failure,
/// and sends each chunk -- or the first error, or a final empty chunk for
/// EOF -- over `sender`. Stops as soon as a send fails: the caller
/// dropped its receiver because it cancelled, hit the deadline, or the
/// response was already complete, so there is nothing left to do with
/// whatever this thread reads next (which may not return for as long as
/// reqwest's own per-read timeout allows).
fn spawn_reader(response: Response, limit: usize, sender: mpsc::SyncSender<ChunkResult>) {
    std::thread::spawn(move || {
        let mut reader = response.take(limit as u64 + 1);
        let mut buffer = [0u8; READ_CHUNK];
        loop {
            let read = loop {
                match reader.read(&mut buffer) {
                    Ok(read) => break Ok(read),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => break Err(error),
                }
            };
            let (message, done) = match read {
                Ok(0) => (Ok(Zeroizing::new(Vec::new())), true),
                Ok(read) => (Ok(Zeroizing::new(buffer[..read].to_vec())), false),
                Err(error) => (Err(error), true),
            };
            if sender.send(message).is_err() || done {
                return;
            }
        }
    });
}

/// `Timeout` when `error` is a per-read idle timeout (either reqwest's own
/// `io::ErrorKind::TimedOut` or a wrapped `reqwest::Error` that
/// `is_timeout()`), otherwise `Network`.
fn read_error(error: &io::Error) -> ProviderError {
    if error.kind() == io::ErrorKind::TimedOut
        || error
            .get_ref()
            .and_then(|source| source.downcast_ref::<reqwest::Error>())
            .is_some_and(reqwest::Error::is_timeout)
    {
        ProviderError::Timeout
    } else {
        ProviderError::Network
    }
}

/// Reads a success body under `limit` bytes. The caller has already
/// mapped the status, so this never inspects it, and picks the limit its
/// own contract allows: `MAX_RESPONSE` for a completion, a larger
/// adapter-fixed bound for a content-free catalog listing.
///
/// The actual reads happen on a background thread (see [`spawn_reader`]);
/// this function only polls that thread's channel with
/// `recv_timeout(`[`POLL_INTERVAL`]`)`, so `control` is checked before the
/// first read, after every chunk, and on every idle poll tick -- an
/// overshoot of at most one tick past `cancel` being set or the deadline
/// elapsing, even while the connection sends nothing at all (a plain
/// per-read timeout, the only bound reqwest itself offers, cannot detect
/// that case any faster than its own multi-second-to-minute duration).
/// [`READ_IDLE_GUARD`] separately catches a connection that stops
/// sending entirely, ahead of `control`'s deadline if that deadline is
/// longer. A chunk that completes the body (a clean EOF) is returned
/// immediately without a further deadline check, so a response that
/// finished exactly as the deadline expired is not turned into an error.
/// Callers pass the same `control` (and so the same `started` instant)
/// used for [`send_with_control`], so this deadline check continues the
/// same budget rather than starting a fresh one for the body alone.
pub(crate) fn read_body(
    response: Response,
    limit: usize,
    control: &RequestControl<'_>,
) -> Result<Zeroizing<Vec<u8>>, ProviderError> {
    if response
        .content_length()
        .is_some_and(|len| len > limit as u64)
    {
        return Err(ProviderError::ResponseTooLarge);
    }
    control.check()?;
    let (sender, receiver) = mpsc::sync_channel(1);
    spawn_reader(response, limit, sender);
    let mut bytes = Zeroizing::new(Vec::new());
    let mut last_chunk = Instant::now();
    loop {
        match receiver.recv_timeout(POLL_INTERVAL) {
            Ok(Ok(chunk)) if chunk.is_empty() => break,
            Ok(Ok(chunk)) => {
                bytes.extend_from_slice(&chunk);
                if bytes.len() > limit {
                    return Err(ProviderError::ResponseTooLarge);
                }
                last_chunk = Instant::now();
            }
            Ok(Err(error)) => return Err(read_error(&error)),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(ProviderError::Network),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        control.check()?;
        if last_chunk.elapsed() > READ_IDLE_GUARD {
            return Err(ProviderError::Timeout);
        }
    }
    Ok(bytes)
}

/// Accept only an entire JSON document, optionally inside one Markdown JSON fence.
/// Never search prose for a plausible object or discard text around a fence.
pub(crate) fn json_document(bytes: &[u8]) -> Result<&[u8], ProviderError> {
    if bytes.len() > MAX_RESPONSE {
        return Err(ProviderError::ResponseTooLarge);
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ProviderError::InvalidJson)?
        .trim();
    if let Some(fenced) = text
        .strip_prefix("```json\n")
        .or_else(|| text.strip_prefix("```json\r\n"))
        .or_else(|| text.strip_prefix("```\n"))
    {
        return fenced
            .strip_suffix("```")
            .map(|value| value.trim().as_bytes())
            .ok_or(ProviderError::InvalidJson);
    }
    Ok(text.as_bytes())
}

pub(crate) fn parse_error(error: openloops_contracts::ParseRejection) -> ProviderError {
    use openloops_contracts::ParseRejection;
    match error {
        ParseRejection::ResponseTooLarge => ProviderError::ResponseTooLarge,
        ParseRejection::InvalidUtf8 | ParseRejection::InvalidJson => ProviderError::InvalidJson,
        ParseRejection::InvalidSchema => ProviderError::InvalidSchema,
    }
}

pub(crate) fn transport_error(error: &reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Timeout
    } else {
        ProviderError::Network
    }
}

pub(crate) fn status_error(status: u16) -> ProviderError {
    match status {
        401 | 403 => ProviderError::Unauthorized,
        402 => ProviderError::Quota,
        429 => ProviderError::RateLimited,
        500..=599 => ProviderError::ServerError(status),
        _ => ProviderError::RequestRejected(status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_model_labels_reject_whitespace_and_terminal_escapes() {
        assert!(valid_key("synthetic-key"));
        assert!(!valid_key(""));
        assert!(!valid_key("synthetic key"));
        assert!(!valid_key("synthetic\r\nX-Injected: 1"));
        assert!(!valid_key(&"x".repeat(MAX_KEY + 1)));
        assert!(valid_model_name("vendor/model-1.2:free"));
        assert!(!valid_model_name(""));
        assert!(!valid_model_name("bad\u{1b}[2J"));
        assert!(!valid_model_name(&"x".repeat(257)));
    }

    #[test]
    fn timeout_while_reading_response_body_is_reported_as_timeout() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (release, hold) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = [0; 4096];
            let _bytes_read = socket.read(&mut buffer).unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\n")
                .unwrap();
            let _ = hold.recv_timeout(Duration::from_secs(2));
        });
        let response = Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(300))
            .build()
            .unwrap()
            .get(format!("http://{address}/"))
            .send()
            .unwrap();
        let result = read_body(response, MAX_RESPONSE, &RequestControl::new(Instant::now()));
        drop(release);
        server.join().unwrap();
        assert_eq!(result.err(), Some(ProviderError::Timeout));
    }

    /// Starts a loopback server that writes `head`, then trickles one byte
    /// of `body` every `interval` until `body` is exhausted (an empty
    /// `body` sends nothing at all after `head`, simulating a connection
    /// that goes completely silent), then holds the connection open (so a
    /// passing test never depends on the peer eventually closing it).
    /// Returns the connected `Response` and the server's join handle.
    fn trickling_response(
        head: &str,
        body: &'static [u8],
        interval: Duration,
    ) -> (Response, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let head = head.to_owned();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 4096];
            let _bytes_read = socket.read(&mut buffer).unwrap();
            socket.write_all(head.as_bytes()).unwrap();
            for byte in body {
                std::thread::sleep(interval);
                if socket.write_all(std::slice::from_ref(byte)).is_err() {
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(5));
        });
        let response = Client::builder()
            .no_proxy()
            // Idle guard only: far longer than the trickle interval and the
            // test-only deadline below, so it never fires first.
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
            .get(format!("http://{address}/"))
            .send()
            .unwrap();
        (response, server)
    }

    #[test]
    fn a_trickling_body_past_the_deadline_reports_timeout_not_idle_timeout() {
        // Content-Length is deliberately larger than what the server ever
        // sends, so read_body would otherwise block on read() up to the
        // client's idle timeout; the (short, test-only) deadline must cut
        // it off first.
        let (response, server) = trickling_response(
            "HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\n",
            b"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            Duration::from_millis(200),
        );
        let control =
            RequestControl::with_deadline(Instant::now(), None, Duration::from_millis(500));
        let result = read_body(response, MAX_RESPONSE, &control);
        assert_eq!(result.err(), Some(ProviderError::Timeout));
        drop(server);
    }

    #[test]
    fn cancelling_mid_body_returns_cancelled_promptly_not_at_body_completion() {
        // 100 bytes at 100ms apart is ~10 seconds of trickling body: long
        // enough that the assertion below (well under one second) only
        // passes if cancellation is actually observed promptly, not because
        // it happened to coincide with the body finishing on its own.
        let (response, server) = trickling_response(
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n",
            &[b'x'; 100],
            Duration::from_millis(100),
        );
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let setter = std::thread::spawn({
            let cancel = cancel.clone();
            move || {
                std::thread::sleep(Duration::from_millis(150));
                cancel.store(true, Ordering::Relaxed);
            }
        });
        let control = RequestControl::with_deadline(
            Instant::now(),
            Some(cancel.as_ref()),
            Duration::from_mins(2),
        );
        let started = Instant::now();
        let result = read_body(response, MAX_RESPONSE, &control);
        let elapsed = started.elapsed();
        assert_eq!(result.err(), Some(ProviderError::Cancelled));
        assert!(elapsed < Duration::from_secs(1), "cancel took {elapsed:?}");
        setter.join().unwrap();
        drop(server);
    }

    #[test]
    fn cancel_is_observed_within_1500ms_while_the_server_stays_silent_mid_body() {
        // No body bytes at all after the headers: a plain per-chunk check
        // would never run because no chunk ever arrives. Only polling a
        // background reader thread (rather than blocking on its `read()`)
        // can observe `cancel` promptly here.
        let (response, server) = trickling_response(
            "HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n",
            b"",
            Duration::from_millis(0),
        );
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let setter = std::thread::spawn({
            let cancel = cancel.clone();
            move || {
                std::thread::sleep(Duration::from_millis(100));
                cancel.store(true, Ordering::Relaxed);
            }
        });
        let control = RequestControl::with_deadline(
            Instant::now(),
            Some(cancel.as_ref()),
            Duration::from_mins(2),
        );
        let started = Instant::now();
        let result = read_body(response, MAX_RESPONSE, &control);
        let elapsed = started.elapsed();
        assert_eq!(result.err(), Some(ProviderError::Cancelled));
        assert!(
            elapsed < Duration::from_millis(1500),
            "cancel took {elapsed:?}"
        );
        setter.join().unwrap();
        drop(server);
    }

    #[test]
    fn deadline_is_observed_within_1500ms_of_expiry_while_the_server_stays_silent_mid_body() {
        let (response, server) = trickling_response(
            "HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n",
            b"",
            Duration::from_millis(0),
        );
        let control =
            RequestControl::with_deadline(Instant::now(), None, Duration::from_millis(150));
        let started = Instant::now();
        let result = read_body(response, MAX_RESPONSE, &control);
        let elapsed = started.elapsed();
        assert_eq!(result.err(), Some(ProviderError::Timeout));
        assert!(
            elapsed < Duration::from_millis(1500),
            "deadline took {elapsed:?}"
        );
        drop(server);
    }

    /// Starts a loopback server that accepts the connection, reads the
    /// request, and then sends nothing at all -- not even a status line
    /// -- simulating a provider that withholds response headers entirely
    /// until it finishes generating (the case `send_with_control` exists
    /// for). Returns the request URL and the server's join handle.
    fn silent_before_headers_server() -> (String, std::thread::JoinHandle<()>) {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 4096];
            let _bytes_read = socket.read(&mut buffer);
            std::thread::sleep(Duration::from_secs(5));
        });
        (format!("http://{address}/"), server)
    }

    #[test]
    fn send_with_control_observes_cancel_within_1500ms_while_headers_never_arrive() {
        let (url, server) = silent_before_headers_server();
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let setter = std::thread::spawn({
            let cancel = cancel.clone();
            move || {
                std::thread::sleep(Duration::from_millis(200));
                cancel.store(true, Ordering::Relaxed);
            }
        });
        let control = RequestControl::with_deadline(
            Instant::now(),
            Some(cancel.as_ref()),
            Duration::from_mins(2),
        );
        let started = Instant::now();
        let result = send_with_control(client.get(&url), &control);
        let elapsed = started.elapsed();
        assert_eq!(result.err(), Some(ProviderError::Cancelled));
        assert!(
            elapsed < Duration::from_millis(1500),
            "cancel took {elapsed:?}"
        );
        setter.join().unwrap();
        drop(server);
    }

    #[test]
    fn send_with_control_observes_the_deadline_within_1500ms_of_expiry_while_headers_never_arrive()
    {
        let (url, server) = silent_before_headers_server();
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let control =
            RequestControl::with_deadline(Instant::now(), None, Duration::from_millis(300));
        let started = Instant::now();
        let result = send_with_control(client.get(&url), &control);
        let elapsed = started.elapsed();
        assert_eq!(result.err(), Some(ProviderError::Timeout));
        assert!(
            elapsed < Duration::from_millis(1500),
            "deadline took {elapsed:?}"
        );
        drop(server);
    }

    #[test]
    fn send_with_control_and_read_body_share_one_budget_for_a_normal_response() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 4096];
            let _bytes_read = socket.read(&mut buffer);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello")
                .unwrap();
        });
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let control = RequestControl::new(Instant::now());
        let response = send_with_control(client.get(format!("http://{address}/")), &control)
            .expect("send_with_control should succeed for a normal response");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = read_body(response, MAX_RESPONSE, &control).unwrap();
        server.join().unwrap();
        assert_eq!(&*body, b"hello");
    }

    #[test]
    fn a_normal_response_is_unaffected_by_the_deadline_or_cancel_checks() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 4096];
            let _bytes_read = socket.read(&mut buffer).unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello")
                .unwrap();
        });
        let response = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(format!("http://{address}/"))
            .send()
            .unwrap();
        let cancel = AtomicBool::new(false);
        let control =
            RequestControl::with_deadline(Instant::now(), Some(&cancel), Duration::from_mins(2));
        let result = read_body(response, MAX_RESPONSE, &control).unwrap();
        server.join().unwrap();
        assert_eq!(&*result, b"hello");
    }

    #[test]
    fn error_text_names_no_provider_upstream_payload_or_endpoint() {
        for error in [
            ProviderError::InvalidKey,
            ProviderError::Unauthorized,
            ProviderError::Network,
            ProviderError::ModelUnavailable,
            ProviderError::InvalidResponse,
        ] {
            let text = error.to_string();
            assert!(!text.contains("http"), "error text must name no endpoint");
        }
        assert!(
            ProviderError::Timeout
                .to_string()
                .contains("did not answer in time")
        );
        assert!(
            ProviderError::ServerError(502)
                .to_string()
                .contains("HTTP 502")
        );
    }
}
