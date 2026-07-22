// This crate does not carry `#![forbid(unsafe_code)]` at the source level:
// see `Cargo.toml`'s `[lints.rust]` table for why (the crate-local lint
// table sets `unsafe_code = "deny"` instead of inheriting the workspace's
// `forbid`, which is what lets `src/dpapi_ffi.rs` carry one narrowly scoped
// `#[allow(unsafe_code)]`). Every module besides `dpapi_ffi` still has zero
// `unsafe` in it, which the crate-level `deny` continues to enforce.
//! `openloops-persistence`: the ADR-005 protected-state engine.
//!
//! This crate implements `contracts/persistence/protected-state-boundary.json`
//! as library code plus tests: the AEAD envelope, the nonce-attempt
//! reservation ledger, the purpose-separated HMAC digest framing, the
//! current-user DPAPI key/blob layer, the `SQLite` envelope store, and the
//! rollback-anchor sequence. [`is_available`] still returns `false` and
//! nothing outside this crate's own tests calls into it: no capability is
//! enabled, no gate advances, and no desktop/application wiring exists yet.
//!
//! | Module | Contract section |
//! |---|---|
//! | [`ids`] | `envelope_suite.identifier_layout`, `record_type_codes` |
//! | [`aad`] | `envelope_suite.aad_encoding_rows` (the exact 78-byte AAD) |
//! | [`envelope`] | `envelope_suite` (AES-256-GCM record envelopes) |
//! | [`reservation`] | `transaction_contract.encryption_reservation_order` |
//! | [`digest`] | `digest_suite` (HMAC-SHA-256 purpose-separated framing) |
//! | [`dpapi_ffi`] | the one sanctioned unsafe module: raw `CryptProtectData`/`CryptUnprotectData`/`LocalFree` |
//! | [`dpapi`] | safe, bounded wrapper over [`dpapi_ffi`] |
//! | [`protected_file`] | `protected_blob_store` (root resolution, atomic replace) |
//! | [`state_root`] | `secret_inventory`, `rollback_recovery.anchor_format`/`anchor_protocol` |
//! | [`store`] | `database_contract`, `transaction_contract` |
//! | [`anchor`] | `rollback_recovery.anchor_format`/`anchor_protocol` (commitment math) |
//! | [`error`] | the sanitized error taxonomy shared across modules |
//!
//! # Honest scope limits (read before relying on this crate for anything real)
//!
//! * **Migration and key rotation are not implemented.** `migration_rotation`
//!   is a large, separate state machine this story's `IMPLEMENTATION` list
//!   does not include; there is exactly one active AEAD key, one content-
//!   digest key, and one anchor key per account, with no rotation or schema
//!   migration path yet.
//! * **DACL enforcement is not implemented.** [`protected_file::verify_owner_only_dacl`]
//!   always returns a typed `NotYetEnforced` error rather than silently
//!   skipping the check.
//! * **Known-folder root resolution uses `%LOCALAPPDATA%`, not
//!   `SHGetKnownFolderPath`.** See [`protected_file`]'s doc comment.
//! * **`MoveFileExW`/`ReplaceFileW` are not called; `std::fs::rename` is.**
//!   See [`protected_file`]'s doc comment for exactly what that does and does
//!   not guarantee relative to the contract's exact recipe.
//! * **Cross-record relationship validation (dangling references across
//!   record types) is out of scope.** [`store::Store::write_transaction`]
//!   only enforces the account-binding match; full graph validation belongs
//!   to the domain ADRs that own those relationships.
//! * **True cross-user/cross-machine DPAPI rejection is not exercised
//!   in-process.** [`dpapi_ffi`]'s tests cover the tamper/corruption path,
//!   which returns the same error class the OS would return for a
//!   different-principal blob, but a second real Windows principal is
//!   needed for a true end-to-end test.
//! * **[`state_root::StateRootBundle`] key material is not zeroized on
//!   drop.** The bundle (and its `encode()` output) holds the three raw
//!   32-byte keys as plain arrays; zeroization here is best-effort per the
//!   contract's deletion posture and is a recorded follow-up, not a
//!   guarantee.
//! * **`state_root::advance_after_commit`'s reopen-verify compares the
//!   reopened anchor against the just-written in-memory commitment**, not
//!   against a freshly re-derived database root; the single-writer lock
//!   makes the two equivalent today, and the literal contract step-7
//!   re-derivation is a recorded follow-up.
//! * **[`anchor::FreezeReason::KeyMismatch`] is currently unreachable**:
//!   the anchor key lives in the same protected bundle as the commitment,
//!   so key divergence surfaces as `CommitmentMismatch` instead.
//! * **`envelope::seal` maps the theoretically unreachable cipher-capacity
//!   error to `PlaintextTooLarge`**; the explicit bounds check above it is
//!   the real guard.

pub mod aad;
pub mod anchor;
pub mod digest;
pub mod dpapi;
pub mod dpapi_ffi;
pub mod envelope;
mod error;
pub mod ids;
pub mod protected_file;
pub mod reservation;
pub mod state_root;
pub mod store;

pub use error::RngError;

/// Phase 0 supplies no durable store and writes no runtime values.
#[must_use]
pub const fn is_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn persistence_is_unavailable() {
        assert!(!super::is_available());
    }
}
