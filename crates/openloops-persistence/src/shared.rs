//! Small reviewed primitives re-exported for sibling crates.
//!
//! `openloops-graph`'s ADR-002 PKCE transaction machine
//! (`contracts/identity/authentication-boundary.json` `transaction_contract`)
//! needs a CSPRNG and the mechanical RFC 7636 `S256` SHA-256 primitive.
//! `tools/check-protected-state-boundary.ps1`'s confinement scan reserves
//! direct `getrandom::`/`sha2::` source-code use to this crate. Rather than
//! widen that scan or hand-roll randomness/hashing in `openloops-graph`
//! (both prohibited by this story's brief), this module exposes exactly two
//! narrow, non-secret-bearing functions and nothing else: no envelope,
//! DPAPI, key, or `SQLite` type is reachable from here, so this is not a
//! path into the protected-state persistence API.

use crate::RngError;

/// Fills `bytes` with output from the reviewed OS CSPRNG (`getrandom` 0.4.3,
/// pinned in this crate's `Cargo.toml`).
///
/// # Errors
///
/// Returns [`RngError::CsprngUnavailable`] when the underlying call fails;
/// callers must fail closed rather than substitute a weaker source.
pub fn fill_random(bytes: &mut [u8]) -> Result<(), RngError> {
    getrandom::fill(bytes).map_err(|_| RngError::CsprngUnavailable)
}

/// Returns the unkeyed SHA-256 digest of `input`.
///
/// This is the mechanical RFC 7636 `S256` transform only. It is not a
/// purpose-separated HMAC and must never stand in for this crate's own
/// `digest_suite` content or account digests.
#[must_use]
pub fn sha256(input: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(input);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::{fill_random, sha256};

    #[test]
    fn fill_random_produces_distinct_nonzero_output() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        fill_random(&mut a).expect("csprng available in tests");
        fill_random(&mut b).expect("csprng available in tests");
        assert_ne!(a, [0u8; 32]);
        assert_ne!(a, b);
    }

    #[test]
    fn sha256_matches_known_empty_vector() {
        // SHA-256("") — FIPS 180-4 / widely published test vector.
        let digest = sha256(b"");
        assert_eq!(
            digest,
            [
                0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
                0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
                0x78, 0x52, 0xb8, 0x55
            ]
        );
    }

    // The RFC 7636 Appendix B `code_verifier` -> `S256` `code_challenge`
    // vector is exercised end-to-end in `openloops-graph`'s `encoding`
    // module (this crate has no base64url encoder to compare against, and
    // hand-transcribing raw digest bytes here would itself be an unverified
    // "ad hoc" value; the base64url string form is the widely published,
    // independently checkable representation of that vector).
}
