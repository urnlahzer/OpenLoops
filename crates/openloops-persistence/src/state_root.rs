//! `state-root.dpapi`: the versioned protected bundle holding every key and
//! the rollback anchor, and the commit-then-recompute-then-replace-then-
//! reopen-verify sequence built on top of it.
//!
//! `secret_inventory`: the AEAD keyring, the content-digest HMAC keyring,
//! and the rollback-anchor key all live in this one file, "with a distinct
//! key and purpose" for the anchor. `protected_blob_store.canonical_files`
//! bounds `state-root.dpapi` at 65536 bytes; the fixed encoding below is 194
//! bytes, far inside that bound with no growth path that could approach it
//! for this story's scope (key rotation/migration bundles are out of scope;
//! see the crate root doc comment for the full list of honest gaps).

use crate::anchor::{self, AnchorKey, AnchorState, FreezeReason};
use crate::dpapi;
use crate::envelope::Envelope;
use crate::ids::{AccountBindingAad, KeyId, RandomId};
use crate::protected_file;
use crate::reservation::{KeyUsageState, ReservationBatch, ReservationError};

/// `protected_blob_store.canonical_files[state-root.dpapi].maximum_bytes`.
pub const MAXIMUM_BYTES: usize = 65536;
/// The exact fixed wire size this crate's `StateRootBundle` encodes to.
pub const WIRE_BYTES: usize = 194;
const MAGIC: &[u8; 8] = b"OLSTATE1";
const VERSION: u16 = 1;

/// The decoded, in-memory contents of `state-root.dpapi`.
///
/// Every key field is raw bytes rather than an [`crate::envelope::EnvelopeKey`]/
/// [`crate::digest::ContentDigestKey`]/[`AnchorKey`] so this type can
/// round-trip through encode/decode without those types' zeroize-on-drop
/// semantics fighting a `Copy`-free byte buffer; callers construct the typed
/// key wrapper immediately before use and let it drop (and zeroize)
/// immediately after.
#[derive(Clone, Eq, PartialEq)]
pub struct StateRootBundle {
    pub aead_key_id: KeyId,
    pub aead_key_bytes: [u8; 32],
    pub aead_usage: u64,
    pub content_digest_key_bytes: [u8; 32],
    pub anchor_key_id: KeyId,
    pub anchor_key_bytes: [u8; 32],
    pub anchor_generation: core::num::NonZeroU64,
    pub anchor_commitment: [u8; 32],
    pub last_observed_unix_seconds: u64,
}

impl core::fmt::Debug for StateRootBundle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StateRootBundle")
            .field("aead_key_id", &self.aead_key_id)
            .field("anchor_key_id", &self.anchor_key_id)
            .field("anchor_generation", &self.anchor_generation)
            .field(
                "last_observed_unix_seconds",
                &self.last_observed_unix_seconds,
            )
            .finish_non_exhaustive()
    }
}

/// A rejected decode of a `state-root.dpapi` bundle.
///
/// `os_protection.blob_validation`: "validate exact key-bundle magic version
/// length unique key IDs algorithms statuses and bounded entries before
/// use". Every branch below corresponds to one of those checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateRootError {
    Dpapi(dpapi::DpapiError),
    File(protected_file::ProtectedFileError),
    WrongLength,
    WrongMagic,
    UnknownVersion,
    DuplicateKeyId,
    InvalidField,
    GenerationExhausted,
    ReopenMismatch,
    Reservation(ReservationError),
    Anchor(anchor::AnchorError),
}

impl From<dpapi::DpapiError> for StateRootError {
    fn from(err: dpapi::DpapiError) -> Self {
        Self::Dpapi(err)
    }
}

impl From<protected_file::ProtectedFileError> for StateRootError {
    fn from(err: protected_file::ProtectedFileError) -> Self {
        Self::File(err)
    }
}

impl StateRootBundle {
    /// Creates a fresh bundle: two freshly generated, distinct keys, an
    /// empty AEAD usage counter, and anchor generation 1 committed over an
    /// empty row set.
    ///
    /// # Errors
    ///
    /// Returns [`crate::RngError`] on CSPRNG failure.
    ///
    /// # Panics
    ///
    /// Never in practice: framing an empty row set never fails, and
    /// `HmacSha256::new_from_slice` (inside [`anchor::compute`]) only rejects
    /// a key length that does not fit its key size.
    pub fn fresh(account_binding_aad: AccountBindingAad) -> Result<Self, crate::RngError> {
        let aead_key_id = RandomId::generate()?;
        let mut aead_key_bytes = [0u8; 32];
        getrandom::fill(&mut aead_key_bytes).map_err(|_| crate::RngError::CsprngUnavailable)?;
        let mut content_digest_key_bytes = [0u8; 32];
        getrandom::fill(&mut content_digest_key_bytes)
            .map_err(|_| crate::RngError::CsprngUnavailable)?;
        let mut anchor_key_id = RandomId::generate()?;
        while anchor_key_id.as_bytes() == aead_key_id.as_bytes() {
            anchor_key_id = RandomId::generate()?;
        }
        let mut anchor_key_bytes = [0u8; 32];
        getrandom::fill(&mut anchor_key_bytes).map_err(|_| crate::RngError::CsprngUnavailable)?;

        let anchor_key = AnchorKey::new(anchor_key_bytes);
        let generation = anchor::first_generation();
        let commitment = anchor::compute(&anchor_key, account_binding_aad, generation, &[])
            .expect("an empty row set always frames successfully");

        Ok(Self {
            aead_key_id,
            aead_key_bytes,
            aead_usage: 0,
            content_digest_key_bytes,
            anchor_key_id,
            anchor_key_bytes,
            anchor_generation: generation,
            anchor_commitment: commitment,
            last_observed_unix_seconds: 0,
        })
    }

    /// Encodes the exact fixed-width [`WIRE_BYTES`] layout.
    #[must_use]
    pub fn encode(&self) -> [u8; WIRE_BYTES] {
        let mut out = [0u8; WIRE_BYTES];
        let mut at = 0usize;
        out[at..at + 8].copy_from_slice(MAGIC);
        at += 8;
        out[at..at + 2].copy_from_slice(&VERSION.to_be_bytes());
        at += 2;
        out[at..at + 16].copy_from_slice(self.aead_key_id.as_bytes());
        at += 16;
        out[at..at + 32].copy_from_slice(&self.aead_key_bytes);
        at += 32;
        out[at..at + 8].copy_from_slice(&self.aead_usage.to_be_bytes());
        at += 8;
        out[at..at + 32].copy_from_slice(&self.content_digest_key_bytes);
        at += 32;
        out[at..at + 16].copy_from_slice(self.anchor_key_id.as_bytes());
        at += 16;
        out[at..at + 32].copy_from_slice(&self.anchor_key_bytes);
        at += 32;
        out[at..at + 8].copy_from_slice(&self.anchor_generation.get().to_be_bytes());
        at += 8;
        out[at..at + 32].copy_from_slice(&self.anchor_commitment);
        at += 32;
        out[at..at + 8].copy_from_slice(&self.last_observed_unix_seconds.to_be_bytes());
        at += 8;
        debug_assert_eq!(at, WIRE_BYTES);
        out
    }

    /// Decodes and fully validates a bundle.
    ///
    /// # Errors
    ///
    /// Returns the specific [`StateRootError`] variant.
    ///
    /// # Panics
    ///
    /// Never in practice: every fixed-width slice this function reads with
    /// `try_into().unwrap()` is checked against the exact [`WIRE_BYTES`]
    /// length immediately above, so every sub-slice offset is always in
    /// bounds and exactly the expected width.
    pub fn decode(bytes: &[u8]) -> Result<Self, StateRootError> {
        if bytes.len() != WIRE_BYTES {
            return Err(StateRootError::WrongLength);
        }
        if &bytes[0..8] != MAGIC {
            return Err(StateRootError::WrongMagic);
        }
        let version = u16::from_be_bytes([bytes[8], bytes[9]]);
        if version != VERSION {
            return Err(StateRootError::UnknownVersion);
        }
        let mut at = 10usize;
        let aead_key_id = read_id(bytes, at)?;
        at += 16;
        let mut aead_key_bytes = [0u8; 32];
        aead_key_bytes.copy_from_slice(&bytes[at..at + 32]);
        at += 32;
        let aead_usage = u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap());
        at += 8;
        let mut content_digest_key_bytes = [0u8; 32];
        content_digest_key_bytes.copy_from_slice(&bytes[at..at + 32]);
        at += 32;
        let anchor_key_id = read_id(bytes, at)?;
        at += 16;
        let mut anchor_key_bytes = [0u8; 32];
        anchor_key_bytes.copy_from_slice(&bytes[at..at + 32]);
        at += 32;
        let generation_raw = u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap());
        let anchor_generation =
            core::num::NonZeroU64::new(generation_raw).ok_or(StateRootError::InvalidField)?;
        at += 8;
        let mut anchor_commitment = [0u8; 32];
        anchor_commitment.copy_from_slice(&bytes[at..at + 32]);
        at += 32;
        let last_observed_unix_seconds = u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap());
        at += 8;
        debug_assert_eq!(at, WIRE_BYTES);

        if aead_key_id.as_bytes() == anchor_key_id.as_bytes() {
            return Err(StateRootError::DuplicateKeyId);
        }
        Ok(Self {
            aead_key_id,
            aead_key_bytes,
            aead_usage,
            content_digest_key_bytes,
            anchor_key_id,
            anchor_key_bytes,
            anchor_generation,
            anchor_commitment,
            last_observed_unix_seconds,
        })
    }
}

fn read_id(bytes: &[u8], at: usize) -> Result<KeyId, StateRootError> {
    let mut raw = [0u8; 16];
    raw.copy_from_slice(&bytes[at..at + 16]);
    RandomId::from_random_bytes(raw).map_err(|_| StateRootError::InvalidField)
}

/// Protects and atomically writes `bundle` to `target`.
///
/// # Errors
///
/// Returns [`StateRootError::Dpapi`] or [`StateRootError::File`].
pub fn protect_and_write(
    target: &std::path::Path,
    bundle: &StateRootBundle,
) -> Result<(), StateRootError> {
    let plaintext = bundle.encode();
    let protected = dpapi::protect_bounded(&plaintext, MAXIMUM_BYTES)?;
    protected_file::write_atomic(target, &protected)?;
    Ok(())
}

/// Reads and unprotects `target`; `Ok(None)` means no protected state exists
/// yet (the ordinary fresh-account case).
///
/// # Errors
///
/// Returns [`StateRootError::File`], [`StateRootError::Dpapi`], or a decode
/// error.
pub fn read_and_unprotect(
    target: &std::path::Path,
) -> Result<Option<StateRootBundle>, StateRootError> {
    let Some(blob) = protected_file::read_bounded(target, MAXIMUM_BYTES as u64)? else {
        return Ok(None);
    };
    let plaintext = dpapi::unprotect_bounded(&blob, MAXIMUM_BYTES)?;
    Ok(Some(StateRootBundle::decode(&plaintext)?))
}

/// The typed, non-mutating outcome of startup reconciliation
/// (`anchor_protocol`, `rollback_recovery`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreezeCause {
    /// No `state-root.dpapi` exists yet.
    Missing,
    /// Persisted wall time regressed (`clock_rollback`).
    ClockReversed,
    /// The anchor/database disagreed; see [`FreezeReason`].
    Anchor(FreezeReason),
}

/// Reads `state-root.dpapi`, checks the persisted clock high-water mark, and
/// verifies the anchor against `rows`. Never mutates anything.
///
/// # Errors
///
/// Returns [`StateRootError`] for a malformed bundle; a coherent-but-frozen
/// outcome is `Ok(Err(FreezeCause))`, not an `Err`, because it is not a
/// failure of this function — it is the correct, expected recovery signal.
pub fn startup_reconcile(
    target: &std::path::Path,
    account_binding_aad: AccountBindingAad,
    rows: &[Envelope],
    now_unix_seconds: u64,
) -> Result<Result<StateRootBundle, FreezeCause>, StateRootError> {
    let Some(bundle) = read_and_unprotect(target)? else {
        return Ok(Err(FreezeCause::Missing));
    };
    if now_unix_seconds < bundle.last_observed_unix_seconds {
        return Ok(Err(FreezeCause::ClockReversed));
    }
    let anchor_key = AnchorKey::new(bundle.anchor_key_bytes);
    let stored = AnchorState {
        key_id: bundle.anchor_key_id,
        generation: bundle.anchor_generation,
        commitment: bundle.anchor_commitment,
    };
    match anchor::verify(&anchor_key, &stored, account_binding_aad, rows) {
        Ok(()) => Ok(Ok(bundle)),
        Err(reason) => Ok(Err(FreezeCause::Anchor(reason))),
    }
}

/// `transaction_contract.record_write_order` steps 6-8, run strictly after
/// the caller has already committed the `SQLite` transaction
/// ([`crate::store::Store::write_transaction`]) and reopened/read the
/// committed rows ([`crate::store::Store::read_all`]): recompute the
/// database-root commitment, atomically replace `state-root.dpapi` with
/// generation n+1, then reopen and verify it matches.
///
/// The caller must not publish any result, checkpoint, or outward mutation
/// until this function returns `Ok`.
///
/// # Errors
///
/// Returns [`StateRootError::GenerationExhausted`] on `u64` exhaustion (never
/// expected in practice), an anchor framing error, or
/// [`StateRootError::ReopenMismatch`] if the reopened file does not match
/// what was just written — which must be treated as [`FreezeCause::Anchor`]
/// by the caller, not retried blindly.
pub fn advance_after_commit(
    target: &std::path::Path,
    mut bundle: StateRootBundle,
    account_binding_aad: AccountBindingAad,
    rows_after_commit: &[Envelope],
    now_unix_seconds: u64,
) -> Result<StateRootBundle, StateRootError> {
    let next_generation = anchor::next_generation(bundle.anchor_generation)
        .ok_or(StateRootError::GenerationExhausted)?;
    let anchor_key = AnchorKey::new(bundle.anchor_key_bytes);
    let commitment = anchor::compute(
        &anchor_key,
        account_binding_aad,
        next_generation,
        rows_after_commit,
    )
    .map_err(StateRootError::Anchor)?;
    bundle.anchor_generation = next_generation;
    bundle.anchor_commitment = commitment;
    bundle.last_observed_unix_seconds = now_unix_seconds;
    protect_and_write(target, &bundle)?;

    let reopened = read_and_unprotect(target)?.ok_or(StateRootError::ReopenMismatch)?;
    if reopened.anchor_generation != bundle.anchor_generation
        || reopened.anchor_commitment != bundle.anchor_commitment
    {
        return Err(StateRootError::ReopenMismatch);
    }
    Ok(reopened)
}

/// Durably reserves `count` AEAD attempts against `bundle`'s active key
/// *before* the caller generates any nonce, per
/// `transaction_contract.encryption_reservation_order`: the new usage total
/// is written to disk (via [`protect_and_write`]) before this function
/// returns the batch.
///
/// # Errors
///
/// Returns [`StateRootError::Reservation`] or a write error.
pub fn reserve_aead_batch(
    target: &std::path::Path,
    mut bundle: StateRootBundle,
    count: u32,
) -> Result<(StateRootBundle, ReservationBatch), StateRootError> {
    let mut usage = KeyUsageState {
        key_id: bundle.aead_key_id,
        used: bundle.aead_usage,
    };
    let batch = usage
        .reserve_batch(count)
        .map_err(StateRootError::Reservation)?;
    bundle.aead_usage = usage.used;
    protect_and_write(target, &bundle)?;
    Ok((bundle, batch))
}

#[cfg(test)]
mod tests {
    use super::{
        FreezeCause, StateRootBundle, advance_after_commit, protect_and_write, reserve_aead_batch,
        startup_reconcile,
    };
    use crate::aad::EnvelopeAad;
    use crate::envelope::{Envelope, EnvelopeKey, seal};
    use crate::ids::{RandomId, RecordType, SchemaVersion};
    use crate::store::Store;

    fn temp_target(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "openloops-state-root-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("state-root.dpapi")
    }

    fn account() -> crate::ids::AccountBindingAad {
        RandomId::from_random_bytes([3u8; 16]).unwrap()
    }

    #[test]
    fn encode_decode_round_trips() {
        let bundle = StateRootBundle::fresh(account()).unwrap();
        let decoded = StateRootBundle::decode(&bundle.encode()).unwrap();
        assert_eq!(
            decoded.aead_key_id.as_bytes(),
            bundle.aead_key_id.as_bytes()
        );
        assert_eq!(decoded.anchor_generation, bundle.anchor_generation);
        assert_eq!(decoded.anchor_commitment, bundle.anchor_commitment);
    }

    #[test]
    fn fresh_bundle_never_reuses_the_aead_key_id_as_the_anchor_key_id() {
        let bundle = StateRootBundle::fresh(account()).unwrap();
        assert_ne!(
            bundle.aead_key_id.as_bytes(),
            bundle.anchor_key_id.as_bytes()
        );
    }

    fn envelope_for(
        store_account: crate::ids::AccountBindingAad,
        key: &EnvelopeKey,
        record_id_byte: u8,
    ) -> Envelope {
        let aad = EnvelopeAad {
            key_id: key.id(),
            account_binding_aad: store_account,
            record_type: RecordType::Loop,
            record_id: RandomId::from_random_bytes([record_id_byte; 16]).unwrap(),
            schema_version: SchemaVersion::new(core::num::NonZeroU32::new(1).unwrap()),
        };
        seal(key, aad, [record_id_byte; 12], b"synthetic").unwrap()
    }

    /// Full happy path: reserve, write rows, commit, recompute+replace+
    /// reopen+verify the anchor, then confirm startup reconciliation on a
    /// fresh process accepts the result. "Both coherent (pass)".
    #[test]
    fn full_commit_then_anchor_sequence_then_startup_reconcile_passes() {
        let target = temp_target("happy-path");
        let account = account();
        let mut bundle = StateRootBundle::fresh(account).unwrap();
        protect_and_write(&target, &bundle).unwrap();

        let (bundle_after_reserve, batch) = reserve_aead_batch(&target, bundle.clone(), 2).unwrap();
        bundle = bundle_after_reserve;
        assert_eq!(batch.count, 2);

        let key = EnvelopeKey::new(bundle.aead_key_id, bundle.aead_key_bytes);
        let rows = vec![
            envelope_for(account, &key, 1),
            envelope_for(account, &key, 2),
        ];

        let mut store = Store::open_in_memory().unwrap();
        store.write_transaction(account, &rows).unwrap();
        let committed_rows = store.read_all(account).unwrap();

        let bundle = advance_after_commit(&target, bundle, account, &committed_rows, 100).unwrap();
        assert_eq!(bundle.anchor_generation.get(), 2);

        let outcome = startup_reconcile(&target, account, &committed_rows, 200).unwrap();
        assert!(
            outcome.is_ok(),
            "coherent database and anchor must reconcile"
        );
    }

    /// "Crash between DB commit and anchor replace": the store commit
    /// happened, but `advance_after_commit` never ran. Startup reconciliation
    /// against the *old* anchor and the *new* rows must freeze, not silently
    /// accept the newer database state.
    #[test]
    fn crash_between_commit_and_anchor_replace_freezes_on_restart() {
        let target = temp_target("crash-before-anchor");
        let account = account();
        let bundle = StateRootBundle::fresh(account).unwrap();
        protect_and_write(&target, &bundle).unwrap();

        let key = EnvelopeKey::new(bundle.aead_key_id, bundle.aead_key_bytes);
        let rows = vec![envelope_for(account, &key, 9)];
        let mut store = Store::open_in_memory().unwrap();
        store.write_transaction(account, &rows).unwrap();
        let committed_rows = store.read_all(account).unwrap();
        // "crash": advance_after_commit is never called.

        let outcome = startup_reconcile(&target, account, &committed_rows, 100).unwrap();
        assert_eq!(
            outcome,
            Err(FreezeCause::Anchor(
                crate::anchor::FreezeReason::CommitmentMismatch
            ))
        );
    }

    /// "Anchor present but DB rolled back": after a fully coherent sequence,
    /// the database loses a row out from under the anchor (e.g. a restored
    /// snapshot). Startup reconciliation must freeze.
    #[test]
    fn anchor_present_but_database_rolled_back_freezes_on_restart() {
        let target = temp_target("db-rolled-back");
        let account = account();
        let bundle = StateRootBundle::fresh(account).unwrap();
        protect_and_write(&target, &bundle).unwrap();

        let key = EnvelopeKey::new(bundle.aead_key_id, bundle.aead_key_bytes);
        let rows = vec![
            envelope_for(account, &key, 1),
            envelope_for(account, &key, 2),
        ];
        let mut store = Store::open_in_memory().unwrap();
        store.write_transaction(account, &rows).unwrap();
        let committed_rows = store.read_all(account).unwrap();
        let _ = advance_after_commit(&target, bundle, account, &committed_rows, 100).unwrap();

        // The database is now "rolled back" to a snapshot missing one row.
        let rolled_back_rows = vec![committed_rows[0].clone()];
        let outcome = startup_reconcile(&target, account, &rolled_back_rows, 200).unwrap();
        assert_eq!(
            outcome,
            Err(FreezeCause::Anchor(
                crate::anchor::FreezeReason::CommitmentMismatch
            ))
        );
    }

    #[test]
    fn missing_state_root_freezes_as_missing() {
        let target = temp_target("missing");
        let outcome = startup_reconcile(&target, account(), &[], 1).unwrap();
        assert_eq!(outcome, Err(FreezeCause::Missing));
    }

    #[test]
    fn clock_reversal_freezes_even_with_a_matching_anchor() {
        let target = temp_target("clock-reversal");
        let account = account();
        let mut bundle = StateRootBundle::fresh(account).unwrap();
        bundle.last_observed_unix_seconds = 1_000;
        protect_and_write(&target, &bundle).unwrap();

        let outcome = startup_reconcile(&target, account, &[], 500).unwrap();
        assert_eq!(outcome, Err(FreezeCause::ClockReversed));
    }
}
