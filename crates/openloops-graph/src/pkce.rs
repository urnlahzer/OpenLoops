//! RFC 7636 PKCE code verifier/challenge, `S256` only.
//!
//! `contracts/identity/authentication-boundary.json` `flow.pkce_method` is
//! `"S256"`; `plain` is never produced or accepted. Both the verifier and
//! its entropy source are memory-only and one-use per
//! `transaction_contract.pkce_verifier`.

use crate::encoding::base64url_encode;

/// Verifier entropy: 32 random bytes, encoding to a 43-character base64url
/// string. That is inside RFC 7636's required 43-128 character bound and
/// matches `transaction_contract.pkce_verifier`'s "fresh high-entropy"
/// requirement.
const VERIFIER_RANDOM_BYTES: usize = 32;

/// A fresh, memory-only PKCE code verifier.
///
/// `token_boundary.prohibited_values` lists `pkce_verifier`; this type's
/// `Debug` never prints it, and nothing in this crate persists it.
pub struct CodeVerifier(String);

impl CodeVerifier {
    /// Generates a fresh verifier from the reviewed OS CSPRNG (via
    /// [`openloops_persistence::fill_random`]).
    ///
    /// # Errors
    ///
    /// Returns [`openloops_persistence::RngError`] when the CSPRNG call
    /// fails; callers must fail closed rather than retry with a weaker
    /// source.
    pub fn generate() -> Result<Self, openloops_persistence::RngError> {
        let mut bytes = [0u8; VERIFIER_RANDOM_BYTES];
        openloops_persistence::fill_random(&mut bytes)?;
        Ok(Self(base64url_encode(&bytes)))
    }

    /// Derives this verifier's `S256` code challenge
    /// (`base64url(sha256(verifier))`, RFC 7636 §4.2).
    #[must_use]
    pub fn challenge(&self) -> CodeChallenge {
        let digest = openloops_persistence::sha256(self.0.as_bytes());
        CodeChallenge(base64url_encode(&digest))
    }

    /// Returns the verifier text, for the token-exchange request only.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Debug for CodeVerifier {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("CodeVerifier").field(&"<redacted>").finish()
    }
}

/// The `S256` code challenge sent in the authorization request.
///
/// Not secret (it is transmitted in the system-browser URL), but still kept
/// opaque outside this crate's own request-building code.
#[derive(Clone)]
pub struct CodeChallenge(String);

impl CodeChallenge {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::CodeVerifier;

    #[test]
    fn generate_produces_bounded_distinct_verifiers() {
        let a = CodeVerifier::generate().expect("csprng available in tests");
        let b = CodeVerifier::generate().expect("csprng available in tests");
        assert_eq!(a.as_str().len(), 43);
        assert_eq!(b.as_str().len(), 43);
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn challenge_is_deterministic_for_a_fixed_verifier() {
        let verifier = CodeVerifier::generate().expect("csprng available in tests");
        let challenge_a = verifier.challenge();
        let challenge_b = verifier.challenge();
        assert_eq!(challenge_a.as_str(), challenge_b.as_str());
    }

    #[test]
    fn debug_never_prints_the_verifier_text() {
        let verifier = CodeVerifier::generate().expect("csprng available in tests");
        let rendered = format!("{verifier:?}");
        assert!(!rendered.contains(verifier.as_str()));
        assert!(rendered.contains("redacted"));
    }
}
