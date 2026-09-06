//! Sanitized error taxonomy.
//!
//! IMPLEMENTATION step 7 requires "sanitized error taxonomy (no paths, no
//! SQL text, no key material in errors)". Every error type in this crate is a
//! plain closed enum of typed reasons; none of them carry a `String`, a
//! `std::io::Error`, a `SQLite` message, a file path, or any byte buffer that
//! could hold plaintext, a key, or a nonce. Where an underlying library error
//! (`std::io::Error`, `rusqlite::Error`) is unavoidable at a call site, this
//! crate classifies it into one of these closed reasons and drops the
//! underlying value.

/// The OS CSPRNG failed or returned an unusable value.
///
/// `envelope_suite.rng_failure`: "fail closed before logical-record
/// serialization database persistence or outward mutation; a previously
/// durable protected usage reservation remains burned and is never rolled
/// back or reused".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RngError {
    /// `getrandom` returned an error, or returned all-zero bytes twice in a
    /// row (astronomically unlikely with a functioning CSPRNG and therefore
    /// treated as a CSPRNG fault rather than retried further).
    CsprngUnavailable,
}
