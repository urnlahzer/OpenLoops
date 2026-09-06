//! The rollback-anchor commitment: canonical row ordering, HMAC framing,
//! and the commit-then-recompute-then-replace-then-reopen-verify sequence.
//!
//! `rollback_recovery.anchor_format`: a distinct HMAC-SHA-256 key, computed
//! over every current envelope row in a fixed canonical order, with a
//! 25-byte domain tag and a nonzero 64-bit generation. This module owns the
//! commitment math and the typed freeze outcomes; [`crate::store`] owns
//! reading rows back from `SQLite` and [`crate::state_root`] owns persisting
//! the winning `(generation, commitment)` pair.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::envelope::Envelope;
use crate::ids::{AccountBindingAad, KeyId};

type HmacSha256 = Hmac<Sha256>;

/// `rollback_recovery.anchor_format.input_encoding`: 25 fixed ASCII bytes.
const DOMAIN: &[u8; 25] = b"openloops-state-anchor-v1";
/// `rollback_recovery.anchor_format.output_bytes`.
pub const COMMITMENT_BYTES: usize = 32;
/// `rollback_recovery.anchor_format.key_bytes`.
pub const KEY_BYTES: usize = 32;

/// A zeroize-on-drop rollback-anchor HMAC-SHA-256 key.
///
/// `secret_inventory[rollback_commit_anchor]`: "a distinct key and purpose"
/// from the AEAD keyring and the content-digest keyring; this is a separate
/// Rust type from [`crate::envelope::EnvelopeKey`] and
/// [`crate::digest::ContentDigestKey`] for the same reason both of those are
/// separate from each other.
pub struct AnchorKey(zeroize::Zeroizing<[u8; KEY_BYTES]>);

impl AnchorKey {
    /// Wraps raw key bytes.
    #[must_use]
    pub fn new(bytes: [u8; KEY_BYTES]) -> Self {
        Self(zeroize::Zeroizing::new(bytes))
    }
}

/// The durable `(key, key_id, generation, commitment)` tuple stored in
/// `state-root.dpapi`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnchorState {
    pub key_id: KeyId,
    pub generation: core::num::NonZeroU64,
    pub commitment: [u8; COMMITMENT_BYTES],
}

/// A rejected anchor computation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnchorError {
    /// Two rows shared the same `(record_type, record_id)` sort key.
    DuplicateSortKey,
    /// More than `u32::MAX` rows were supplied.
    TooManyRows,
    /// One ciphertext exceeded `u32::MAX` bytes.
    CiphertextTooLarge,
}

/// The typed, non-mutating outcome of startup reconciliation.
///
/// `rollback_recovery`: "missing, mismatched, corrupt, cross-account,
/// clock-reversed, or unknown state freezes cursor advancement and every
/// outward mutation". Every non-[`FreezeReason`]-free path here corresponds
/// to one of those named reasons; there is no "assume it's fine" branch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreezeReason {
    /// No anchor was present at all.
    Missing,
    /// The recomputed commitment did not match the stored one
    /// (`db_only_rollback` / `tamper_or_corruption`).
    CommitmentMismatch,
    /// A row's `account_binding_aad` did not match the expected account.
    CrossAccount,
    /// The anchor's `key_id` did not match the active anchor key.
    KeyMismatch,
    /// Row canonicalization itself failed ([`AnchorError`]).
    Corrupt,
}

/// Sorts `rows` into `row_order` ("ascending unsigned `record_type` code then
/// lexicographic unsigned 16-byte `record_id`") and rejects duplicate sort
/// keys.
///
/// # Errors
///
/// Returns [`AnchorError::DuplicateSortKey`].
pub fn canonical_order(rows: &[Envelope]) -> Result<Vec<&Envelope>, AnchorError> {
    let mut ordered: Vec<&Envelope> = rows.iter().collect();
    ordered.sort_by(|a, b| {
        a.aad
            .record_type
            .code()
            .cmp(&b.aad.record_type.code())
            .then_with(|| a.aad.record_id.as_bytes().cmp(b.aad.record_id.as_bytes()))
    });
    for pair in ordered.windows(2) {
        if pair[0].aad.record_type.code() == pair[1].aad.record_type.code()
            && pair[0].aad.record_id.as_bytes() == pair[1].aad.record_id.as_bytes()
        {
            return Err(AnchorError::DuplicateSortKey);
        }
    }
    Ok(ordered)
}

/// Builds the exact anchor input framing over already-canonicalized `rows`.
///
/// # Errors
///
/// Returns [`AnchorError::TooManyRows`] or [`AnchorError::CiphertextTooLarge`].
fn frame(
    account_binding_aad: AccountBindingAad,
    generation: core::num::NonZeroU64,
    rows: &[&Envelope],
) -> Result<Vec<u8>, AnchorError> {
    let row_count = u32::try_from(rows.len()).map_err(|_| AnchorError::TooManyRows)?;
    let mut framed = Vec::new();
    framed.extend_from_slice(DOMAIN);
    framed.extend_from_slice(account_binding_aad.as_bytes());
    framed.extend_from_slice(&generation.get().to_be_bytes());
    framed.extend_from_slice(&row_count.to_be_bytes());
    for row in rows {
        framed.extend_from_slice(row.aad.record_id.as_bytes());
        framed.extend_from_slice(row.aad.account_binding_aad.as_bytes());
        framed.extend_from_slice(&row.aad.record_type.code().to_be_bytes());
        framed.extend_from_slice(&row.aad.schema_version.get().to_be_bytes());
        framed.extend_from_slice(&crate::ids::CIPHERTEXT_VERSION.to_be_bytes());
        framed.extend_from_slice(row.aad.key_id.as_bytes());
        framed.extend_from_slice(&row.nonce);
        framed.extend_from_slice(&row.tag);
        let ciphertext_len =
            u32::try_from(row.ciphertext.len()).map_err(|_| AnchorError::CiphertextTooLarge)?;
        framed.extend_from_slice(&ciphertext_len.to_be_bytes());
        framed.extend_from_slice(&row.ciphertext);
    }
    Ok(framed)
}

/// Recomputes the exact database-root commitment over every current row.
///
/// `anchor_protocol` step 2: "recompute the exact logical database-root HMAC
/// ... [and] compare ... with the protected anchor". This function only
/// computes; callers compare with [`AnchorState::commitment`] using
/// constant-time equality (see [`verify`]).
///
/// # Errors
///
/// Returns [`AnchorError`] from canonicalization or framing.
///
/// # Panics
///
/// Never in practice: `HmacSha256::new_from_slice` only rejects a key
/// length that does not fit its key size, and [`AnchorKey`] is always
/// exactly [`KEY_BYTES`] long.
pub fn compute(
    key: &AnchorKey,
    account_binding_aad: AccountBindingAad,
    generation: core::num::NonZeroU64,
    rows: &[Envelope],
) -> Result<[u8; COMMITMENT_BYTES], AnchorError> {
    let ordered = canonical_order(rows)?;
    let framed = frame(account_binding_aad, generation, &ordered)?;
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(key.0.as_slice())
        .expect("HMAC accepts any key length");
    Mac::update(&mut mac, &framed);
    let mut out = [0u8; COMMITMENT_BYTES];
    out.copy_from_slice(&Mac::finalize(mac).into_bytes());
    Ok(out)
}

/// Verifies `rows` (at `account_binding_aad`/`generation`) against a stored
/// [`AnchorState`], returning the exact [`FreezeReason`] on any mismatch.
///
/// This is the read side of `anchor_protocol`: "treat commitment mismatch
/// unexpected sidecar missing key unknown version reserved-usage regression
/// clock reversal tamper corruption or an interrupted transaction as
/// recovery". Sidecar/reserved-usage/clock checks live in
/// [`crate::state_root`], which has the rest of the durable state this
/// function does not see.
///
/// # Errors
///
/// Returns the specific [`FreezeReason`].
///
/// # Panics
///
/// Never in practice; see [`compute`]'s `# Panics` section.
pub fn verify(
    key: &AnchorKey,
    stored: &AnchorState,
    account_binding_aad: AccountBindingAad,
    rows: &[Envelope],
) -> Result<(), FreezeReason> {
    if rows
        .iter()
        .any(|row| row.aad.account_binding_aad.as_bytes() != account_binding_aad.as_bytes())
    {
        return Err(FreezeReason::CrossAccount);
    }
    // Built directly from `frame` (rather than calling `compute` and then
    // comparing the two `[u8; 32]` outputs with `==`) so the final
    // comparison always goes through `Mac::verify_slice`'s constant-time
    // `ct_eq`, never an ordinary — and potentially early-exiting —
    // `PartialEq` comparison of secret-derived bytes.
    let ordered = canonical_order(rows).map_err(|_| FreezeReason::Corrupt)?;
    let framed = frame(account_binding_aad, stored.generation, &ordered)
        .map_err(|_| FreezeReason::Corrupt)?;
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(key.0.as_slice())
        .expect("HMAC accepts any key length");
    Mac::update(&mut mac, &framed);
    Mac::verify_slice(mac, &stored.commitment).map_err(|_| FreezeReason::CommitmentMismatch)
}

/// The first anchor generation a freshly created account binding uses.
///
/// # Panics
///
/// Never: `1` is trivially nonzero.
#[must_use]
pub fn first_generation() -> core::num::NonZeroU64 {
    core::num::NonZeroU64::new(1).expect("1 is nonzero")
}

/// The next anchor generation after `current`, or `None` on `u64` exhaustion.
#[must_use]
pub fn next_generation(current: core::num::NonZeroU64) -> Option<core::num::NonZeroU64> {
    current.checked_add(1)
}

#[cfg(test)]
mod tests {
    use super::{
        AnchorError, AnchorKey, FreezeReason, canonical_order, compute, first_generation, verify,
    };
    use crate::aad::EnvelopeAad;
    use crate::envelope::Envelope;
    use crate::ids::{RandomId, RecordType, SchemaVersion};

    fn account() -> crate::ids::AccountBindingAad {
        RandomId::from_random_bytes([1u8; 16]).unwrap()
    }

    fn envelope(record_type: RecordType, record_id_byte: u8) -> Envelope {
        Envelope {
            aad: EnvelopeAad {
                key_id: RandomId::from_random_bytes([2u8; 16]).unwrap(),
                account_binding_aad: account(),
                record_type,
                record_id: RandomId::from_random_bytes([record_id_byte; 16]).unwrap(),
                schema_version: SchemaVersion::new(core::num::NonZeroU32::new(1).unwrap()),
            },
            nonce: [0u8; 12],
            tag: [0u8; 16],
            ciphertext: vec![1, 2, 3],
        }
    }

    #[test]
    fn canonical_order_sorts_by_record_type_then_record_id() {
        let rows = vec![
            envelope(RecordType::Loop, 9),
            envelope(RecordType::AccountBinding, 5),
            envelope(RecordType::AccountBinding, 1),
        ];
        let ordered = canonical_order(&rows).unwrap();
        assert_eq!(ordered[0].aad.record_id.as_bytes(), &[1u8; 16]);
        assert_eq!(ordered[1].aad.record_id.as_bytes(), &[5u8; 16]);
        assert_eq!(ordered[2].aad.record_type, RecordType::Loop);
    }

    #[test]
    fn canonical_order_rejects_duplicate_sort_key() {
        let rows = vec![envelope(RecordType::Loop, 1), envelope(RecordType::Loop, 1)];
        assert_eq!(canonical_order(&rows), Err(AnchorError::DuplicateSortKey));
    }

    #[test]
    fn compute_is_order_independent_over_the_same_row_set() {
        let key = AnchorKey::new([9u8; 32]);
        let generation = first_generation();
        let a = vec![
            envelope(RecordType::Loop, 1),
            envelope(RecordType::AccountBinding, 2),
        ];
        let b = vec![
            envelope(RecordType::AccountBinding, 2),
            envelope(RecordType::Loop, 1),
        ];
        assert_eq!(
            compute(&key, account(), generation, &a).unwrap(),
            compute(&key, account(), generation, &b).unwrap()
        );
    }

    #[test]
    fn compute_changes_with_any_row_field() {
        let key = AnchorKey::new([9u8; 32]);
        let generation = first_generation();
        let base = vec![envelope(RecordType::Loop, 1)];
        let base_commitment = compute(&key, account(), generation, &base).unwrap();

        let mut tampered_ciphertext = base.clone();
        tampered_ciphertext[0].ciphertext[0] ^= 0xFF;
        assert_ne!(
            compute(&key, account(), generation, &tampered_ciphertext).unwrap(),
            base_commitment
        );

        let mut tampered_tag = base.clone();
        tampered_tag[0].tag[0] ^= 0xFF;
        assert_ne!(
            compute(&key, account(), generation, &tampered_tag).unwrap(),
            base_commitment
        );

        let next_gen = super::next_generation(generation).unwrap();
        assert_ne!(
            compute(&key, account(), next_gen, &base).unwrap(),
            base_commitment
        );
    }

    #[test]
    fn verify_accepts_matching_state_and_rejects_after_db_only_rollback() {
        let key = AnchorKey::new([9u8; 32]);
        let generation = first_generation();
        let rows = vec![
            envelope(RecordType::Loop, 1),
            envelope(RecordType::AccountBinding, 2),
        ];
        let commitment = compute(&key, account(), generation, &rows).unwrap();
        let stored = super::AnchorState {
            key_id: RandomId::from_random_bytes([3u8; 16]).unwrap(),
            generation,
            commitment,
        };
        assert!(verify(&key, &stored, account(), &rows).is_ok());

        // "db_only_rollback": the database silently lost a row after the
        // anchor was last recomputed. Reconciliation must freeze, never
        // silently accept the smaller row set.
        let rolled_back_rows = vec![rows[0].clone()];
        assert_eq!(
            verify(&key, &stored, account(), &rolled_back_rows),
            Err(FreezeReason::CommitmentMismatch)
        );
    }

    #[test]
    fn verify_rejects_cross_account_row() {
        let key = AnchorKey::new([9u8; 32]);
        let generation = first_generation();
        let mut foreign = envelope(RecordType::Loop, 1);
        foreign.aad.account_binding_aad = RandomId::from_random_bytes([200u8; 16]).unwrap();
        let rows = vec![foreign];
        let stored = super::AnchorState {
            key_id: RandomId::from_random_bytes([3u8; 16]).unwrap(),
            generation,
            commitment: [0u8; 32],
        };
        assert_eq!(
            verify(&key, &stored, account(), &rows),
            Err(FreezeReason::CrossAccount)
        );
    }
}
