//! The authorization-code token-exchange step.
//!
//! ADR-002: "the authority/exchange side is NOT implemented against any
//! real endpoint." This trait has no shipped implementation beyond the
//! synthetic doubles in [`test_support`]; the real Microsoft token endpoint
//! remains `unresolved_pending_G-ID`. Nothing in this module ever opens a
//! socket.

use crate::pkce::CodeVerifier;

/// One in-memory, non-persisted, opaque exchange result.
///
/// `token_boundary.prohibited_values` covers `access_token`/`refresh_token`;
/// this type has no `Display` and its `Debug` redacts. Nothing beyond
/// process memory stores it (protected-cache persistence is a later story,
/// per this story's brief).
pub struct ExchangeOutcome {
    access_token: String,
}

impl ExchangeOutcome {
    #[must_use]
    pub fn new(access_token: String) -> Self {
        Self { access_token }
    }

    /// Returns the raw token text. Callers that need it must call this
    /// explicitly; there is no `Display`/`Debug` path to it.
    #[must_use]
    pub fn access_token(&self) -> &str {
        &self.access_token
    }
}

impl core::fmt::Debug for ExchangeOutcome {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ExchangeOutcome")
            .field("access_token", &"<redacted>")
            .finish()
    }
}

/// A sanitized exchange failure. No variant carries a URL, header, or body
/// fragment.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ExchangeError {
    Transport,
    Rejected,
}

/// The token-exchange port
/// (implementation-plan §2.2 lists no separate exchange interface; this is
/// the ADR-002-scoped seam this story's brief asks for). The real
/// implementation is `unresolved_pending_G-ID`; only [`test_support`]'s
/// synthetic doubles exist in this crate.
pub trait ExchangeTransport {
    /// Exchanges one authorization code plus its bound PKCE verifier.
    ///
    /// # Errors
    ///
    /// Returns [`ExchangeError`] on any transport or rejection failure.
    fn exchange(
        &self,
        code: &str,
        verifier: &CodeVerifier,
    ) -> Result<ExchangeOutcome, ExchangeError>;
}

/// Synthetic test doubles only. Never contacts any endpoint.
pub mod test_support {
    use super::{CodeVerifier, ExchangeError, ExchangeOutcome, ExchangeTransport};

    /// Always succeeds with a fixed synthetic token.
    pub struct AlwaysSucceeds;

    impl ExchangeTransport for AlwaysSucceeds {
        fn exchange(
            &self,
            _code: &str,
            _verifier: &CodeVerifier,
        ) -> Result<ExchangeOutcome, ExchangeError> {
            Ok(ExchangeOutcome::new(
                "synthetic-test-access-token".to_string(),
            ))
        }
    }

    /// Always fails, for negative-path tests.
    pub struct AlwaysRejects;

    impl ExchangeTransport for AlwaysRejects {
        fn exchange(
            &self,
            _code: &str,
            _verifier: &CodeVerifier,
        ) -> Result<ExchangeOutcome, ExchangeError> {
            Err(ExchangeError::Rejected)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ExchangeTransport;
    use super::test_support::{AlwaysRejects, AlwaysSucceeds};
    use crate::pkce::CodeVerifier;

    #[test]
    fn mock_transports_never_expose_the_token_via_debug() {
        let verifier = CodeVerifier::generate().expect("csprng available in tests");
        let outcome = AlwaysSucceeds
            .exchange("synthetic-code", &verifier)
            .expect("mock succeeds");
        let rendered = format!("{outcome:?}");
        assert!(rendered.contains("redacted"));
        assert!(!rendered.contains(outcome.access_token()));
        assert!(AlwaysRejects.exchange("synthetic-code", &verifier).is_err());
    }
}
