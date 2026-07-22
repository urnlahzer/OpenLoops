#![forbid(unsafe_code)]
//! `openloops-graph`: the ADR-002 PKCE transaction machine and the
//! ADR-004-facing typed Graph transport.
//!
//! [`is_available`] still returns `false`: no capability is enabled, no
//! account is configured, and no real Microsoft authority or Graph origin is
//! ever contacted. Every test in this crate talks only to synthetic,
//! in-process `127.0.0.1` peers.
//!
//! | Module | Contract section |
//! |---|---|
//! | [`encoding`] | RFC 4648 §5 base64url and RFC 3986 percent-encoding helpers |
//! | [`pkce`] | `transaction_contract.pkce_verifier` (RFC 7636 `S256`) |
//! | [`transaction`] | `transaction_contract` (one-pending-transaction state machine) |
//! | [`callback`] | `redirect_contract` (the loopback callback listener/parser) |
//! | [`exchange`] | the token-exchange port; real endpoint is `unresolved_pending_G-ID` |
//! | [`transport`] | implementation-plan §2.2 `GraphTransport` |
//! | [`todo`] | ADR-009 `remote_marker_protocol`/`ownership_matrix`/`direct_edit_and_conflict`; real endpoint is `unresolved_pending_G-TODO` |
//!
//! # Dependency activation deviation (read before assuming `oauth2`/`reqwest`/`tokio` are active)
//!
//! `contracts/identity/authentication-boundary.json`'s `dependency_decisions`
//! names `oauth2` 5.0.0, `reqwest` 0.12.28, `tokio` 1.53.0, `url` 2.5.8,
//! `webbrowser` 1.2.1, and this story's brief additionally named `httparse`
//! 1.10.1. This workspace's offline registry mirror does not carry the full
//! transitive dependency closure for any of the async/HTTP candidates:
//!
//! * `httparse` is not present in the mirror at any version.
//! * `oauth2` 5.0.0 mandatorily depends on `base64`, `chrono`, `http`, and
//!   `serde_path_to_error` (none present) and on `thiserror` `^1.0` (only
//!   `2.0.19` is present).
//! * `tokio` 1.53.0 depends on `bytes` (not present) even with every
//!   optional feature disabled.
//! * `url` 2.5.8 depends on `idna`, `form_urlencoded`, and
//!   `percent-encoding` (none present).
//! * `reqwest` 0.12.28 depends on the same missing `http`/`bytes` family
//!   plus `hyper` and a TLS stack, none of which are present.
//!
//! `cargo generate-lockfile --offline` was used to confirm each of these
//! (the exact missing-package errors are quoted in this story's IC report,
//! not reproduced here). No network origin was contacted to attempt a wider
//! resolution — this crate's tests must not depend on outbound network
//! access existing at all, and none of these crates activate here.
//!
//! Given that, this story implements the ADR-002/ADR-004 *behavior* — the
//! PKCE transaction state machine, the loopback callback listener, and a
//! throttling-aware typed transport with backoff, single-flight, and
//! redirect rejection — directly on `std::net`/`std::sync`/`std::time`,
//! with randomness and `S256` hashing sourced through
//! `openloops_persistence::fill_random`/`sha256` (never hand-rolled) and a
//! small hand-written, exhaustively tested base64url/percent-encoding
//! module (see [`encoding`]) standing in for the slice of `url`'s behavior
//! this story needs. `oauth2`, `reqwest`, `tokio`, `url`, `httparse`, and
//! `webbrowser` all remain `selected_not_activated`: none of their names
//! appear in `Cargo.toml`/`Cargo.lock`, and `contracts/identity/
//! authentication-boundary.json` is unchanged by this story.

pub mod callback;
pub mod encoding;
pub mod exchange;
pub mod pkce;
pub mod todo;
pub mod transaction;
pub mod transport;

/// Phase 0 supplies no Graph transport, permission, or network path.
#[must_use]
pub const fn is_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use crate::callback::{AuthorizationCode, CallbackOutcome, LoopbackListener};
    use crate::exchange::ExchangeTransport;
    use crate::exchange::test_support::AlwaysSucceeds;
    use crate::transaction::{DEFAULT_TRANSACTION_MAX_AGE, PendingTransactionSlot};

    #[test]
    fn graph_is_unavailable() {
        assert!(!super::is_available());
    }

    /// PART A flow, composed end-to-end: bind a real loopback listener,
    /// begin a transaction against its real ephemeral port (so the built
    /// authorization URL embeds a genuine OS-assigned redirect port, not a
    /// placeholder), extract that transaction's state, hand the transaction
    /// machine a well-formed synthetic callback outcome carrying the exact
    /// matching state, complete it, and exchange the resulting code through
    /// the synthetic mock exchange.
    ///
    /// This does not dial an actual client connection into the listener —
    /// see `callback`'s module doc for exactly why no test anywhere in this
    /// crate does that (this repository's `P0-AUTHZ-CROSS-CONTRACT-001`/
    /// `P0-SYNC-CROSS-CONTRACT-001` checks ban the literal name of the
    /// standard library's TCP client-socket type repo-wide, and neither
    /// check is in this story's `EXPECTED SURFACE`). `callback`'s own tests
    /// cover the request-line
    /// parsing that a real accepted connection's bytes would go through;
    /// this test covers everything *around* that parsing — a real bound
    /// port flowing into a real authorization URL, into the real
    /// one-pending-transaction state machine, into the real (mocked)
    /// exchange step.
    #[test]
    fn synthetic_sign_in_round_trip_across_bind_transaction_and_exchange() {
        let listener = LoopbackListener::bind().expect("bind loopback listener");
        let port = listener.port();
        let slot = PendingTransactionSlot::new();
        let request = slot.begin(port).expect("begin succeeds");
        assert!(request.authorization_url.contains(&format!(
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A{port}%2Fcallback"
        )));
        // The listener itself is never accepted from in this test; dropping
        // it here exercises the same release path
        // `callback::tests::dropping_an_unaccepted_listener_releases_its_port`
        // asserts on directly.
        drop(listener);

        let query = request
            .authorization_url
            .split_once('?')
            .expect("query present")
            .1;
        let mut state = None;
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("state=") {
                state = Some(crate::encoding::percent_decode(value).expect("valid encoding"));
            }
        }
        let state = state.expect("state parameter present in authorization URL");

        let outcome = CallbackOutcome::Success {
            code: AuthorizationCode::for_test("synthetic-authorization-code"),
            state: state.clone(),
        };
        let completed = slot
            .complete(&outcome, DEFAULT_TRANSACTION_MAX_AGE)
            .expect("state matches the transaction")
            .expect("callback was a success");
        assert_eq!(completed.code.as_str(), "synthetic-authorization-code");

        let mock_transport = AlwaysSucceeds;
        let exchanged = mock_transport
            .exchange(completed.code.as_str(), &completed.verifier)
            .expect("mock exchange succeeds");
        assert!(!exchanged.access_token().is_empty());
        assert!(!slot.is_pending());
    }
}
