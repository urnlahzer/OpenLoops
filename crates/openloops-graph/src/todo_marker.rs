//! `remote_marker_protocol.derivation`/`persisted_verifier`/`request_rule`:
//! the recoverable remote-marker derivation and its (currently refused)
//! persisted verifier.
//!
//! Two distinct values, never confused:
//!
//! * [`derive_marker`] — the **marker itself**: `"base32url of SHA-256 over
//!   remote-marker-v1 plus canonical operation_id bytes"`. This is a plain,
//!   *unkeyed* hash, not a purpose-separated HMAC — the contract calls the
//!   result "non-secret" and it is the value transmitted to Microsoft To Do
//!   through the one supported opaque field
//!   ([`crate::todo::port::TodoMarkerField`]). Being unkeyed and a pure
//!   function of `operation_id` alone is what makes it "recoverable": any
//!   party holding the same `operation_id` can recompute the identical
//!   marker without any secret key material, so a restart never needs to
//!   read a stored marker back — it re-derives it.
//! * [`remote_correlation_hmac`] — the **persisted verifier**: `"only its
//!   HMAC persisted ... no raw marker is persisted"`. This is the opposite
//!   shape — a purpose-separated, *keyed* HMAC over the marker — and it is
//!   the only form ever durable in a `reminder_link` row. This module's
//!   "Honest gap" section in `crate::todo`'s doc comment explains why this
//!   half unconditionally refuses today.
//!
//! Because the marker is unkeyed and the verifier is keyed, one can never be
//! produced from the other: knowing `remote_correlation_hmac`'s (currently
//! unreachable) output would not let a caller recover the marker even in
//! principle, since HMAC-SHA-256 is a one-way function of a secret key this
//! module never exposes — and conversely [`derive_marker`] never accepts or
//! needs that key at all, so nothing here can accidentally launder a keyed
//! value into the unkeyed marker.

use openloops_persistence::digest::{ContentDigestKey, DigestError, PurposeTag};
use openloops_persistence::ids::{AccountBindingAad, SchemaVersion};

/// `remote_marker_protocol.derivation`'s exact domain string: 16 fixed ASCII
/// bytes, mirroring `openloops_application::ledger`'s
/// `"operation-key-v1"` domain convention.
const MARKER_DOMAIN: &[u8; 16] = b"remote-marker-v1";

/// Derives the recoverable, non-secret remote marker for `operation_id`
/// (`remote_marker_protocol.derivation`). Deterministic and unkeyed: the same
/// `operation_id` always reproduces the same marker, with no digest key or
/// account binding involved at all — this is deliberate (see this module's
/// doc comment) and is what makes the marker "recoverable" after a restart
/// without ever having to read one back from storage.
///
/// `operation_id` is expected to be the same encrypted, approved opaque
/// operation identity `openloops_application::ledger` already tracks; this
/// function does not interpret its bytes beyond hashing them.
#[must_use]
pub fn derive_marker(operation_id: &[u8]) -> String {
    let mut input = Vec::with_capacity(MARKER_DOMAIN.len() + operation_id.len());
    input.extend_from_slice(MARKER_DOMAIN);
    input.extend_from_slice(operation_id);
    let digest = openloops_persistence::sha256(&input);
    crate::encoding::base32url_encode(&digest)
}

/// `remote_marker_protocol.persisted_verifier`: `"remote_correlation_hmac
/// only; no raw marker is persisted"`.
///
/// # Errors
///
/// Returns [`DigestError::LayoutNotYetOwned`] unconditionally as of this
/// story: `contracts/persistence/protected-state-boundary.json`
/// `digest_suite.purpose_catalog`'s `reminder_link.remote_correlation_hmac`
/// row keeps `input_layout` at `"unavailable_pending_owner_ADR"` (see
/// `crate::todo`'s "Honest gap" doc section), so
/// [`PurposeTag::ReminderLinkRemoteCorrelation::is_layout_closed`] reports
/// `false` and `openloops_persistence::digest::compute` refuses every call.
/// A caller must propagate this error rather than substitute a locally
/// invented framing.
pub fn remote_correlation_hmac(
    digest_key: &ContentDigestKey,
    account: AccountBindingAad,
    schema_version: SchemaVersion,
    marker: &str,
) -> Result<[u8; 32], DigestError> {
    openloops_persistence::digest::compute(
        digest_key,
        PurposeTag::ReminderLinkRemoteCorrelation,
        account,
        schema_version,
        &[marker.as_bytes()],
    )
}

#[cfg(test)]
mod tests {
    use super::{derive_marker, remote_correlation_hmac};
    use openloops_persistence::digest::{ContentDigestKey, DigestError};
    use openloops_persistence::ids::{RandomId, SchemaVersion};

    fn account() -> openloops_persistence::ids::AccountBindingAad {
        RandomId::from_random_bytes([7u8; 16]).unwrap()
    }

    fn schema() -> SchemaVersion {
        SchemaVersion::new(core::num::NonZeroU32::new(1).unwrap())
    }

    #[test]
    fn marker_is_deterministic_for_the_same_operation_id() {
        let a = derive_marker(&[1u8; 16]);
        let b = derive_marker(&[1u8; 16]);
        assert_eq!(
            a, b,
            "same operation_id must recompute the identical marker"
        );
    }

    #[test]
    fn marker_differs_across_operation_ids() {
        let a = derive_marker(&[1u8; 16]);
        let b = derive_marker(&[2u8; 16]);
        assert_ne!(a, b);
    }

    #[test]
    fn single_byte_change_in_operation_id_changes_the_marker() {
        // A minimal-diff property smoke test: SHA-256 is not a linear or
        // trivially predictable transform, so a one-byte change in the
        // hashed input must not merely shift the output by a small amount.
        let mut operation_id = [0u8; 16];
        let base = derive_marker(&operation_id);
        operation_id[15] = 1;
        let flipped = derive_marker(&operation_id);
        assert_ne!(base, flipped);
        // Not just different at one position: an avalanche-ish sanity check
        // that most characters differ, not merely the last one.
        let differing = base
            .chars()
            .zip(flipped.chars())
            .filter(|(x, y)| x != y)
            .count();
        assert!(
            differing > base.len() / 2,
            "expected a broad avalanche of differences, got {differing} of {}",
            base.len()
        );
    }

    #[test]
    fn marker_never_starts_with_padding_or_lowercase() {
        let marker = derive_marker(&[9u8; 16]);
        assert!(!marker.contains('='));
        assert!(
            marker
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        );
    }

    /// Pinned known-answer vector: base32url(SHA-256("remote-marker-v1" ++
    /// `operation_id`)) for a fixed all-zero 16-byte `operation_id`, computed
    /// once with an independent out-of-band SHA-256 oracle plus a
    /// from-scratch base32 encoder (not this crate's own), and pinned here
    /// so any future change to the domain string, hashing, or base32url
    /// encoding is caught. `sha256` itself is `openloops-persistence`'s
    /// already-reviewed `sha2` crate, and `base32url_encode`'s RFC 4648 §10
    /// conformance is independently checked in `encoding`'s own tests.
    #[test]
    fn marker_known_answer_vector_is_stable() {
        let operation_id = [0u8; 16];
        assert_eq!(
            derive_marker(&operation_id),
            "ID5SXGXOEWN7L4BKNAWNN2QMS7OMB3MZEKVGI7YIN7XOGILBGUSQ"
        );
    }

    #[test]
    fn remote_correlation_hmac_fails_closed_pending_the_persistence_catalog_row() {
        let key = ContentDigestKey::new([1u8; 32]);
        for marker in [
            String::new(),
            derive_marker(&[0u8; 16]),
            derive_marker(&[0xFFu8; 16]),
        ] {
            assert_eq!(
                remote_correlation_hmac(&key, account(), schema(), &marker),
                Err(DigestError::LayoutNotYetOwned)
            );
        }
    }
}
