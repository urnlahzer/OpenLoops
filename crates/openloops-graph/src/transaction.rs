//! The ADR-002 one-pending-transaction PKCE state machine.
//!
//! `contracts/identity/authentication-boundary.json` `transaction_contract`:
//! fresh high-entropy state and PKCE verifier, exactly one pending
//! transaction, one-use, exact-match. Missing, mismatched, duplicated,
//! replayed, expired, cancelled, cross-process, or account-switched
//! callbacks fail closed and tear down the ephemeral transaction. A second
//! same-process sign-in attempt is rejected *without* cancelling the
//! incumbent. An unsolicited or malformed callback cannot replace or
//! consume a valid pending transaction.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::callback::{AuthorizationCode, CallbackOutcome};
use crate::encoding::{base64url_encode, percent_encode_query_value};
use crate::pkce::{CodeChallenge, CodeVerifier};

/// The Entra authority/audience is `unresolved_pending_G-ID`
/// (`account_contract.authority_selection`/`audience_selection`). This
/// placeholder uses the IANA-reserved `.invalid` TLD (RFC 2606 §2), which
/// can never resolve or be registered; nothing in this crate ever opens a
/// socket to it — it exists only as inert text inside a built URL string
/// that a real system-browser launch (out of this story's scope) would
/// receive.
pub const UNRESOLVED_AUTHORITY_PLACEHOLDER: &str =
    "https://authority.unresolved-pending-g-id.invalid/authorize";

/// Default transaction lifetime before a callback is treated as expired.
/// The exact value is a G-ID experiment parameter; this is a conservative
/// placeholder, not a reviewed final bound.
pub const DEFAULT_TRANSACTION_MAX_AGE: Duration = Duration::from_mins(10);

/// Fresh, high-entropy, memory-only, one-use CSRF state
/// (`transaction_contract.state`).
pub struct TransactionState(String);

impl TransactionState {
    const RANDOM_BYTES: usize = 32;

    fn generate() -> Result<Self, openloops_persistence::RngError> {
        let mut bytes = [0u8; Self::RANDOM_BYTES];
        openloops_persistence::fill_random(&mut bytes)?;
        Ok(Self(base64url_encode(&bytes)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Debug for TransactionState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("TransactionState")
            .field(&"<redacted>")
            .finish()
    }
}

/// A bounded, sanitized OAuth authorization error classification
/// (`redirect_contract.error_detail_policy`: "bounded allowlist ...
/// sanitized, not logged"). Any value outside the closed catalog becomes
/// [`Self::Other`], which carries no attacker-controlled text.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthorizationErrorCode {
    AccessDenied,
    InvalidRequest,
    UnauthorizedClient,
    UnsupportedResponseType,
    InvalidScope,
    ServerError,
    TemporarilyUnavailable,
    Other,
}

impl AuthorizationErrorCode {
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "access_denied" => Self::AccessDenied,
            "invalid_request" => Self::InvalidRequest,
            "unauthorized_client" => Self::UnauthorizedClient,
            "unsupported_response_type" => Self::UnsupportedResponseType,
            "invalid_scope" => Self::InvalidScope,
            "server_error" => Self::ServerError,
            "temporarily_unavailable" => Self::TemporarilyUnavailable,
            _ => Self::Other,
        }
    }
}

/// A fresh authorization request, ready for a (not-yet-implemented)
/// system-browser launch.
pub struct AuthorizationRequest {
    pub authorization_url: String,
}

/// A transaction that reached a valid, one-use terminal success.
pub struct CompletedTransaction {
    pub code: AuthorizationCode,
    pub verifier: CodeVerifier,
}

/// Every way [`PendingTransactionSlot::begin`]/`complete` fails closed.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TransactionError {
    /// `transaction_contract.second_same_process_attempt`: a second
    /// same-process sign-in attempt while one is already pending. The
    /// incumbent transaction is left completely untouched.
    AlreadyPending,
    /// No transaction is pending: a fresh unsolicited callback, or a
    /// replay after the pending transaction already reached a terminal
    /// state (`transaction_contract.replayed_callback`).
    NoPendingTransaction,
    /// The callback's `state` does not match the pending transaction's
    /// state. The incumbent transaction is left completely untouched
    /// (`transaction_contract.unsolicited_or_malformed_callback`).
    StateMismatch,
    /// The pending transaction outlived [`DEFAULT_TRANSACTION_MAX_AGE`] (or
    /// a caller-supplied bound); it is torn down as part of failing this
    /// call closed.
    Expired,
    /// The CSPRNG failed while generating state/verifier entropy.
    RngUnavailable,
}

impl From<openloops_persistence::RngError> for TransactionError {
    fn from(_: openloops_persistence::RngError) -> Self {
        Self::RngUnavailable
    }
}

struct Transaction {
    state: TransactionState,
    verifier: CodeVerifier,
    created_at: Instant,
}

/// The one-pending-transaction slot
/// (`transaction_contract.pending_transactions: 1`).
pub struct PendingTransactionSlot(Mutex<Option<Transaction>>);

impl Default for PendingTransactionSlot {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingTransactionSlot {
    #[must_use]
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// Starts a fresh transaction bound to a loopback listener already
    /// bound on `redirect_port`.
    ///
    /// # Errors
    ///
    /// Returns [`TransactionError::AlreadyPending`] without mutating the
    /// incumbent transaction when one is already pending.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (a prior panic while the
    /// lock was held), matching this crate's fail-loud-on-corruption
    /// posture rather than silently continuing with undefined state.
    pub fn begin(&self, redirect_port: u16) -> Result<AuthorizationRequest, TransactionError> {
        let mut guard = self.0.lock().expect("transaction mutex poisoned");
        if guard.is_some() {
            return Err(TransactionError::AlreadyPending);
        }
        let state = TransactionState::generate()?;
        let verifier = CodeVerifier::generate()?;
        let challenge = verifier.challenge();
        let authorization_url = build_authorization_url(&state, &challenge, redirect_port);
        *guard = Some(Transaction {
            state,
            verifier,
            created_at: Instant::now(),
        });
        Ok(AuthorizationRequest { authorization_url })
    }

    /// Consumes the pending transaction against one well-formed callback
    /// outcome.
    ///
    /// A callback whose `state` does not match the incumbent transaction
    /// never mutates or clears it (simulates cross-process/unsolicited
    /// callback injection). A callback that arrives after `max_age` clears
    /// the ephemeral transaction and fails closed. Any exact-match callback
    /// — success or failure — consumes the single pending slot exactly
    /// once; a repeat of the same callback afterward sees
    /// [`TransactionError::NoPendingTransaction`].
    ///
    /// # Errors
    ///
    /// See [`TransactionError`]. The `Ok(Err(..))` case is a **successful**
    /// consumption whose OAuth result was itself a sanitized authorization
    /// failure (`error=...`); [`TransactionError`] is reserved for
    /// transaction-machine-level rejection.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned; see [`Self::begin`].
    pub fn complete(
        &self,
        outcome: &CallbackOutcome,
        max_age: Duration,
    ) -> Result<Result<CompletedTransaction, AuthorizationErrorCode>, TransactionError> {
        let mut guard = self.0.lock().expect("transaction mutex poisoned");
        let Some(transaction) = guard.as_ref() else {
            return Err(TransactionError::NoPendingTransaction);
        };
        if !constant_time_eq(
            transaction.state.as_str().as_bytes(),
            outcome.state().as_bytes(),
        ) {
            return Err(TransactionError::StateMismatch);
        }
        if transaction.created_at.elapsed() > max_age {
            *guard = None;
            return Err(TransactionError::Expired);
        }
        let transaction = guard.take().expect("checked Some above");
        match outcome {
            CallbackOutcome::Success { code, .. } => Ok(Ok(CompletedTransaction {
                code: code.clone(),
                verifier: transaction.verifier,
            })),
            CallbackOutcome::Failure { error, .. } => Ok(Err(*error)),
        }
    }

    /// Cancels a pending transaction (user cancel, account switch, or a
    /// port-collision retry before a fresh [`begin`](Self::begin)). Clears
    /// ephemeral state; a no-op if nothing is pending.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned; see [`Self::begin`].
    pub fn cancel(&self) {
        let mut guard = self.0.lock().expect("transaction mutex poisoned");
        *guard = None;
    }

    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned; see [`Self::begin`].
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.0.lock().expect("transaction mutex poisoned").is_some()
    }
}

/// A fixed-time-when-lengths-match byte comparison. `state` is not secret
/// (it is transmitted in a public URL), so this is defense in depth rather
/// than the sole protection; it avoids adding a dependency for one
/// straightforward, exhaustively tested primitive.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn build_authorization_url(
    state: &TransactionState,
    challenge: &CodeChallenge,
    redirect_port: u16,
) -> String {
    let redirect_uri = format!(
        "http://127.0.0.1:{redirect_port}{}",
        crate::callback::CALLBACK_PATH
    );
    format!(
        "{authority}?response_type=code&client_id=unresolved-pending-g-id&redirect_uri={redirect}&state={state}&code_challenge={challenge}&code_challenge_method=S256",
        authority = UNRESOLVED_AUTHORITY_PLACEHOLDER,
        redirect = percent_encode_query_value(&redirect_uri),
        state = percent_encode_query_value(state.as_str()),
        challenge = percent_encode_query_value(challenge.as_str()),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AuthorizationErrorCode, DEFAULT_TRANSACTION_MAX_AGE, PendingTransactionSlot,
        TransactionError,
    };
    use crate::callback::{AuthorizationCode, CallbackOutcome};
    use std::time::Duration;

    fn extract_state(authorization_url: &str) -> String {
        let query = authorization_url.split_once('?').expect("query present").1;
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("state=") {
                return crate::encoding::percent_decode(value).expect("valid percent-encoding");
            }
        }
        panic!("state parameter not found in {authorization_url}");
    }

    fn success(state: &str) -> CallbackOutcome {
        CallbackOutcome::Success {
            code: AuthorizationCode::for_test(state),
            state: state.to_string(),
        }
    }

    #[test]
    fn begin_then_complete_round_trips_a_matching_success_callback() {
        let slot = PendingTransactionSlot::new();
        let request = slot.begin(54321).expect("begin succeeds");
        assert!(
            request
                .authorization_url
                .contains("code_challenge_method=S256")
        );
        let state = extract_state(&request.authorization_url);
        let completed = slot
            .complete(&success(&state), DEFAULT_TRANSACTION_MAX_AGE)
            .expect("state matches")
            .expect("callback was a success");
        assert_eq!(completed.code.as_str(), &state);
        assert!(!slot.is_pending());
    }

    #[test]
    fn second_same_process_begin_is_rejected_without_mutating_incumbent() {
        let slot = PendingTransactionSlot::new();
        let first = slot.begin(1).expect("first begin succeeds");
        let first_state = extract_state(&first.authorization_url);
        let second = slot.begin(2);
        assert_eq!(second.err(), Some(TransactionError::AlreadyPending));
        // The incumbent transaction still completes correctly afterward.
        let completed = slot
            .complete(&success(&first_state), DEFAULT_TRANSACTION_MAX_AGE)
            .expect("state matches")
            .expect("callback was a success");
        assert_eq!(completed.code.as_str(), &first_state);
    }

    #[test]
    fn mismatched_state_is_rejected_without_clearing_the_incumbent() {
        let slot = PendingTransactionSlot::new();
        let request = slot.begin(1).expect("begin succeeds");
        let real_state = extract_state(&request.authorization_url);
        let injected = success("attacker-supplied-state-from-a-second-listener");
        let result = slot.complete(&injected, DEFAULT_TRANSACTION_MAX_AGE);
        assert!(matches!(result, Err(TransactionError::StateMismatch)));
        assert!(slot.is_pending());
        // The real, un-mutated transaction still completes.
        let completed = slot
            .complete(&success(&real_state), DEFAULT_TRANSACTION_MAX_AGE)
            .expect("state matches")
            .expect("callback was a success");
        assert_eq!(completed.code.as_str(), &real_state);
    }

    #[test]
    fn replayed_callback_after_consumption_is_rejected() {
        let slot = PendingTransactionSlot::new();
        let request = slot.begin(1).expect("begin succeeds");
        let state = extract_state(&request.authorization_url);
        let outcome = success(&state);
        slot.complete(&outcome, DEFAULT_TRANSACTION_MAX_AGE)
            .expect("first completion succeeds")
            .expect("callback was a success");
        let replay = slot.complete(&outcome, DEFAULT_TRANSACTION_MAX_AGE);
        assert!(matches!(
            replay,
            Err(TransactionError::NoPendingTransaction)
        ));
    }

    #[test]
    fn unsolicited_callback_with_no_pending_transaction_is_rejected() {
        let slot = PendingTransactionSlot::new();
        let result = slot.complete(&success("no-such-state"), DEFAULT_TRANSACTION_MAX_AGE);
        assert!(matches!(
            result,
            Err(TransactionError::NoPendingTransaction)
        ));
    }

    #[test]
    fn expired_transaction_fails_closed_and_tears_down() {
        let slot = PendingTransactionSlot::new();
        let request = slot.begin(1).expect("begin succeeds");
        let state = extract_state(&request.authorization_url);
        std::thread::sleep(Duration::from_millis(5));
        let result = slot.complete(&success(&state), Duration::from_millis(1));
        assert!(matches!(result, Err(TransactionError::Expired)));
        assert!(!slot.is_pending());
    }

    #[test]
    fn cancel_clears_a_pending_transaction() {
        let slot = PendingTransactionSlot::new();
        slot.begin(1).expect("begin succeeds");
        assert!(slot.is_pending());
        slot.cancel();
        assert!(!slot.is_pending());
        // A fresh sign-in (e.g. after an account switch) can now begin.
        slot.begin(2).expect("begin succeeds after cancel");
    }

    #[test]
    fn cross_process_style_state_never_crosses_slots() {
        // Simulates a second, unrelated process/listener: an entirely
        // separate slot's transaction can never satisfy this slot.
        let slot_a = PendingTransactionSlot::new();
        let slot_b = PendingTransactionSlot::new();
        let request_a = slot_a.begin(1).expect("begin succeeds");
        let request_b = slot_b.begin(2).expect("begin succeeds");
        let state_b = extract_state(&request_b.authorization_url);
        let result = slot_a.complete(&success(&state_b), DEFAULT_TRANSACTION_MAX_AGE);
        assert!(matches!(result, Err(TransactionError::StateMismatch)));
        assert!(slot_a.is_pending());
        let _ = request_a;
    }

    #[test]
    fn well_formed_authorization_failure_is_a_successful_consumption() {
        let slot = PendingTransactionSlot::new();
        let request = slot.begin(1).expect("begin succeeds");
        let state = extract_state(&request.authorization_url);
        let outcome = CallbackOutcome::Failure {
            error: AuthorizationErrorCode::AccessDenied,
            state: state.clone(),
        };
        let result = slot
            .complete(&outcome, DEFAULT_TRANSACTION_MAX_AGE)
            .expect("state matches");
        assert!(matches!(result, Err(AuthorizationErrorCode::AccessDenied)));
        assert!(!slot.is_pending());
    }

    #[test]
    fn authorization_error_code_parses_the_closed_catalog_and_falls_back_to_other() {
        assert_eq!(
            AuthorizationErrorCode::parse("access_denied"),
            AuthorizationErrorCode::AccessDenied
        );
        assert_eq!(
            AuthorizationErrorCode::parse("something_unexpected"),
            AuthorizationErrorCode::Other
        );
    }
}
