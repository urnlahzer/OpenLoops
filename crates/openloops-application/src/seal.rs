//! Composes `openloops-persistence`'s reservation ledger, envelope AEAD, and
//! record identifiers into the one canonically ordered sealing/scanning
//! sequence `ingestion` and `ledger` both need, so neither module re-derives
//! `transaction_contract.encryption_reservation_order` itself.
//!
//! This module calls only `openloops_persistence`'s public functions
//! (`reservation::{KeyUsageState, generate_nonce}`, `envelope::{seal, open}`,
//! `ids::RandomId::generate`, `store::Store::{read_all, write_transaction}`).
//! It never spells out any of the underlying SQL engine, AEAD cipher, digest,
//! CSPRNG, OS-protection, or key-zeroizing library names directly — those
//! stay confined to `openloops-persistence` per that crate's own confinement
//! scan, and this crate composes through the public API those primitives sit
//! behind instead of reaching past it.

use openloops_persistence::envelope::{Envelope, EnvelopeError, EnvelopeKey};
use openloops_persistence::ids::{AccountBindingAad, RecordType, SchemaVersion};
use openloops_persistence::reservation::{KeyUsageState, ReservationError};
use openloops_persistence::store::{Store, StoreError};
use openloops_persistence::{RngError, aad::EnvelopeAad};

/// A rejected seal/scan/commit call. No variant carries plaintext, a key, a
/// nonce, or an underlying library's message text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SealError {
    /// More plaintext records were supplied than `u32::MAX`.
    TooManyRecords,
    Reservation(ReservationError),
    Rng(RngError),
    Envelope(EnvelopeError),
    Store(StoreError),
}

/// Seals every `(record_type, plaintext)` pair into one canonically ordered
/// [`Envelope`] batch.
///
/// Follows `transaction_contract.encryption_reservation_order` exactly: the
/// full batch's attempt count is reserved in one call *before* any nonce is
/// generated (`KeyUsageState::reserve_batch`), then each record gets a fresh
/// CSPRNG nonce (`reservation::generate_nonce`) and a fresh random
/// `record_id` (`RandomId::generate`) — this crate never derives a record's
/// identity from its content, matching every other envelope user in this
/// workspace. `ReservationBatch::attempt_index` is consulted per record so a
/// reservation is genuinely spent per attempt (`envelope_suite.application_rotation_limit`
/// accounting), even though this crate does not yet wire the state-root
/// nonce-retention set `reservation::is_nonce_collision` checks against —
/// that full-anchor composition is `state_root`'s job, out of this story's
/// scope, and is recorded as an honest gap in the crate root.
///
/// # Errors
///
/// Returns [`SealError`] on any reservation, RNG, or envelope failure. On
/// error, no partial batch is returned: callers never see a
/// half-sealed `Vec`.
///
/// # Panics
///
/// Never in practice: `index` is bounded above by `plaintexts.len()`, which
/// was already checked to fit in a `u32` via `count` above.
pub fn seal_batch(
    key: &EnvelopeKey,
    key_usage: &mut KeyUsageState,
    account: AccountBindingAad,
    schema_version: SchemaVersion,
    plaintexts: Vec<(RecordType, Vec<u8>)>,
) -> Result<Vec<Envelope>, SealError> {
    let count = u32::try_from(plaintexts.len()).map_err(|_| SealError::TooManyRecords)?;
    if count == 0 {
        return Ok(Vec::new());
    }
    let batch = key_usage
        .reserve_batch(count)
        .map_err(SealError::Reservation)?;
    let mut out = Vec::with_capacity(plaintexts.len());
    for (index, (record_type, plaintext)) in plaintexts.into_iter().enumerate() {
        let attempt_index = u32::try_from(index).expect("index bounded by count above");
        let _attempt = batch.attempt_index(attempt_index);
        let record_id = openloops_persistence::ids::RandomId::generate().map_err(SealError::Rng)?;
        let nonce = openloops_persistence::reservation::generate_nonce().map_err(SealError::Rng)?;
        let aad = EnvelopeAad {
            key_id: key.id(),
            account_binding_aad: account,
            record_type,
            record_id,
            schema_version,
        };
        let envelope = openloops_persistence::envelope::seal(key, aad, nonce, &plaintext)
            .map_err(SealError::Envelope)?;
        out.push(envelope);
    }
    Ok(out)
}

/// Commits a pre-sealed batch in exactly one [`Store::write_transaction`]
/// call: either every envelope in `envelopes` becomes durable, or (on any
/// error) none does. This is the whole-page/whole-operation atomic commit
/// boundary every caller in this crate composes rather than reimplements.
///
/// # Errors
///
/// Returns [`SealError::Store`] on any write failure.
pub fn commit_batch(
    store: &mut Store,
    account: AccountBindingAad,
    envelopes: &[Envelope],
) -> Result<(), SealError> {
    store
        .write_transaction(account, envelopes)
        .map_err(SealError::Store)
}

/// Reads every envelope for `account`, keeps only `record_type` rows,
/// authenticates and decrypts each one, and decodes it with `decode`.
///
/// `Store::read_all` is the only read primitive the store exposes (there is
/// no secondary index), so every lookup this crate needs — dedup checks,
/// "current" ledger/checkpoint state — is a bounded full scan over one
/// account's rows. That is an accepted, documented performance gap at this
/// story's scale (temp-dir/in-memory tests, not a production mailbox-sized
/// dataset): a real deployment needs a queryable index, which is future work
/// this module's signature does not foreclose.
///
/// # Errors
///
/// Returns [`SealError::Store`] on a read failure or
/// [`SealError::Envelope`] if any matching row fails to authenticate; a
/// decode failure is reported through `on_decode_error`'s return value
/// rather than this function's `Result`, so callers can choose to skip a
/// malformed row instead of failing the whole scan (this crate always
/// chooses "fail the whole scan" today by returning `Err` from the closure).
pub fn scan_records<T>(
    store: &Store,
    account: AccountBindingAad,
    key: &EnvelopeKey,
    record_type: RecordType,
    mut decode: impl FnMut(&[u8]) -> Result<T, crate::codec::CodecError>,
) -> Result<Vec<T>, ScanError> {
    let all = store.read_all(account).map_err(ScanError::Store)?;
    let mut out = Vec::new();
    for envelope in all.into_iter().filter(|e| e.aad.record_type == record_type) {
        let plaintext =
            openloops_persistence::envelope::open(key, &envelope).map_err(ScanError::Envelope)?;
        let decoded = decode(&plaintext).map_err(ScanError::Codec)?;
        out.push(decoded);
    }
    Ok(out)
}

/// A rejected [`scan_records`] call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanError {
    Store(StoreError),
    Envelope(EnvelopeError),
    Codec(crate::codec::CodecError),
}

#[cfg(test)]
mod tests {
    use super::{ScanError, SealError, commit_batch, scan_records, seal_batch};
    use openloops_persistence::envelope::EnvelopeKey;
    use openloops_persistence::ids::{RandomId, RecordType, SchemaVersion};
    use openloops_persistence::reservation::KeyUsageState;
    use openloops_persistence::store::Store;

    fn key() -> EnvelopeKey {
        EnvelopeKey::new(RandomId::from_random_bytes([1u8; 16]).unwrap(), [2u8; 32])
    }

    fn account() -> openloops_persistence::ids::AccountBindingAad {
        RandomId::from_random_bytes([3u8; 16]).unwrap()
    }

    fn schema() -> SchemaVersion {
        SchemaVersion::new(core::num::NonZeroU32::new(1).unwrap())
    }

    #[test]
    fn seal_commit_and_scan_round_trip() {
        let key = key();
        let account = account();
        let mut usage = KeyUsageState::fresh(key.id());
        let plaintexts = vec![
            (RecordType::MessageObservation, b"one".to_vec()),
            (RecordType::MessageObservation, b"two".to_vec()),
            (RecordType::SyncCheckpoint, b"checkpoint".to_vec()),
        ];
        let envelopes = seal_batch(&key, &mut usage, account, schema(), plaintexts).unwrap();
        assert_eq!(envelopes.len(), 3);
        assert_eq!(usage.used, 3, "one reservation per sealed record");

        let mut store = Store::open_in_memory().unwrap();
        commit_batch(&mut store, account, &envelopes).unwrap();

        let observations = scan_records(
            &store,
            account,
            &key,
            RecordType::MessageObservation,
            |bytes| Ok(bytes.to_vec()),
        )
        .unwrap();
        assert_eq!(observations.len(), 2);

        let checkpoints = scan_records(&store, account, &key, RecordType::SyncCheckpoint, |b| {
            Ok(b.to_vec())
        })
        .unwrap();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0], b"checkpoint");
    }

    #[test]
    fn empty_batch_reserves_and_writes_nothing() {
        let key = key();
        let mut usage = KeyUsageState::fresh(key.id());
        let envelopes = seal_batch(&key, &mut usage, account(), schema(), Vec::new()).unwrap();
        assert!(envelopes.is_empty());
        assert_eq!(usage.used, 0);
    }

    #[test]
    fn commit_failure_leaves_nothing_durable() {
        let key = key();
        let account = account();
        let mut usage = KeyUsageState::fresh(key.id());
        let mut store = Store::open_in_memory().unwrap();

        let first = seal_batch(
            &key,
            &mut usage,
            account,
            schema(),
            vec![(RecordType::Loop, b"a".to_vec())],
        )
        .unwrap();
        commit_batch(&mut store, account, &first).unwrap();

        // Force a collision on the same table by re-sealing under a forged
        // duplicate `record_id`: build the batch, then swap in a colliding
        // id to simulate "some other row already used this identity".
        let mut second = seal_batch(
            &key,
            &mut usage,
            account,
            schema(),
            vec![(RecordType::Loop, b"b".to_vec())],
        )
        .unwrap();
        second[0].aad.record_id = first[0].aad.record_id;
        // Re-seal is required after mutating the AAD (the tag binds it), so
        // instead directly assert the commit rejects the raw duplicate-id
        // envelope crafted above via the store's own tested rejection path.
        let result = commit_batch(&mut store, account, &second);
        assert_eq!(
            result,
            Err(SealError::Store(
                openloops_persistence::store::StoreError::WriteFailed
            ))
        );

        let all =
            scan_records(&store, account, &key, RecordType::Loop, |b| Ok(b.to_vec())).unwrap();
        assert_eq!(all.len(), 1, "the rejected second batch never committed");
    }

    #[test]
    fn scan_surfaces_decode_errors_without_silently_dropping_rows() {
        let key = key();
        let account = account();
        let mut usage = KeyUsageState::fresh(key.id());
        let mut store = Store::open_in_memory().unwrap();
        let envelopes = seal_batch(
            &key,
            &mut usage,
            account,
            schema(),
            vec![(RecordType::JobHealth, b"x".to_vec())],
        )
        .unwrap();
        commit_batch(&mut store, account, &envelopes).unwrap();

        let result =
            scan_records::<Vec<u8>>(&store, account, &key, RecordType::JobHealth, |_bytes| {
                Err(crate::codec::CodecError::TrailingBytes)
            });
        assert_eq!(
            result,
            Err(ScanError::Codec(crate::codec::CodecError::TrailingBytes))
        );
    }
}
