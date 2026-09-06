//! Per-key nonce-attempt reservation ledger.
//!
//! `transaction_contract.encryption_reservation_order`: before any nonce is
//! generated, the exact number of encryption attempts per active key is
//! calculated and atomically, durably reserved for the complete canonically
//! ordered record set; every reserved slot is burned on any later failure.
//! `envelope_suite.application_rotation_limit` (2^30) and
//! `per_key_invocation_hard_limit` (2^32) bound how many attempts one key may
//! ever reserve.
//!
//! This module is pure bookkeeping over an in-memory [`KeyUsageState`]. The
//! caller (`state_root`/`dpapi`) is responsible for durably persisting the
//! new `used` value *before* any nonce is generated, which is what makes a
//! reservation "burned" rather than "rolled back" on a later failure: there
//! is no API here that ever decreases `used`.

use crate::ids::KeyId;

/// `envelope_suite.application_rotation_limit`: 2^30. A key that has reserved
/// this many attempts must rotate before reserving any more.
pub const APPLICATION_ROTATION_LIMIT: u64 = 1_073_741_824;
/// `envelope_suite.per_key_invocation_hard_limit`: 2^32. No key may ever
/// reserve this many attempts; rotation at [`APPLICATION_ROTATION_LIMIT`]
/// keeps this bound unreachable in practice.
pub const PER_KEY_INVOCATION_HARD_LIMIT: u64 = 4_294_967_296;

/// The durable per-key attempt counter.
///
/// `used` is the exact count of attempts ever reserved for this key,
/// including attempts whose encryption later failed (burned) and attempts
/// still in flight. It only ever increases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyUsageState {
    pub key_id: KeyId,
    pub used: u64,
}

/// One durable, contiguous reservation for a canonically ordered record set.
///
/// `attempt_index(i) = first_attempt + i` for `i in 0..count` gives every
/// record in the set a distinct reserved attempt, satisfying "every record
/// consumes one distinct reservation".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReservationBatch {
    pub key_id: KeyId,
    pub first_attempt: u64,
    pub count: u32,
}

impl ReservationBatch {
    /// Returns the exact attempt index reserved for record `i` of this batch.
    ///
    /// # Panics
    ///
    /// Panics if `i >= self.count`; callers only ever index within the
    /// canonically ordered record set the batch was sized for.
    #[must_use]
    pub fn attempt_index(&self, i: u32) -> u64 {
        assert!(i < self.count, "attempt index out of the reserved batch");
        self.first_attempt + u64::from(i)
    }
}

/// A rejected reservation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReservationError {
    /// The requested attempt count was zero.
    ZeroCount,
    /// `used + count` would exceed [`PER_KEY_INVOCATION_HARD_LIMIT`].
    HardLimitExceeded,
    /// The key has already reserved [`APPLICATION_ROTATION_LIMIT`] attempts
    /// and must rotate before any further reservation.
    RotationRequired,
}

/// Pure arithmetic: the new `used` value after reserving `count` attempts, or
/// the specific reason it is refused.
///
/// This is the hard safety invariant independent of the earlier, more
/// conservative [`KeyUsageState::reserve_batch`] rotation gate: rotation is
/// an early-warning threshold applications must respect, but this function
/// is what actually prevents ever exceeding NIST's random-IV invocation
/// bound even if a caller ignored the rotation requirement.
///
/// # Errors
///
/// Returns [`ReservationError::ZeroCount`] for `count == 0`, or
/// [`ReservationError::HardLimitExceeded`] if the new total would reach or
/// exceed [`PER_KEY_INVOCATION_HARD_LIMIT`].
pub fn checked_reserve(used: u64, count: u32) -> Result<u64, ReservationError> {
    if count == 0 {
        return Err(ReservationError::ZeroCount);
    }
    let Some(new_total) = used.checked_add(u64::from(count)) else {
        return Err(ReservationError::HardLimitExceeded);
    };
    if new_total > PER_KEY_INVOCATION_HARD_LIMIT {
        return Err(ReservationError::HardLimitExceeded);
    }
    Ok(new_total)
}

impl KeyUsageState {
    /// Starts a fresh usage record for a newly created key.
    #[must_use]
    pub const fn fresh(key_id: KeyId) -> Self {
        Self { key_id, used: 0 }
    }

    /// Returns whether this key has crossed [`APPLICATION_ROTATION_LIMIT`]
    /// and must rotate before any further reservation.
    #[must_use]
    pub const fn needs_rotation(&self) -> bool {
        self.used >= APPLICATION_ROTATION_LIMIT
    }

    /// Atomically, durably reserves `count` attempts for one canonically
    /// ordered record set.
    ///
    /// The returned [`ReservationBatch`] is the only way to obtain an
    /// attempt index in this module; `self.used` is advanced before this
    /// call returns, so the caller must persist the new state (as part of
    /// `state-root.dpapi`) before generating any nonce. If the caller
    /// crashes or fails after this call returns but before encryption
    /// completes, the reservation is never reused: there is no rollback API.
    ///
    /// # Errors
    ///
    /// Returns [`ReservationError::RotationRequired`] if the key has already
    /// crossed [`APPLICATION_ROTATION_LIMIT`], otherwise the result of
    /// [`checked_reserve`].
    pub fn reserve_batch(&mut self, count: u32) -> Result<ReservationBatch, ReservationError> {
        if self.needs_rotation() {
            return Err(ReservationError::RotationRequired);
        }
        let new_total = checked_reserve(self.used, count)?;
        let first_attempt = self.used;
        self.used = new_total;
        Ok(ReservationBatch {
            key_id: self.key_id,
            first_attempt,
            count,
        })
    }
}

/// Generates a fresh 96-bit nonce from the OS CSPRNG.
///
/// `nonce_generation`: "fresh 96-bit output from the Windows CSPRNG for every
/// encryption attempt; never derived from time counter content identifier
/// process state or a rollbackable value" — this function takes no input
/// besides the RNG itself, so it structurally cannot derive from any of
/// those.
///
/// # Errors
///
/// Returns [`crate::RngError`] on CSPRNG failure; `envelope_suite.rng_failure`
/// requires failing closed with the reservation already burned, which the
/// caller satisfies by calling this only after [`KeyUsageState::reserve_batch`]
/// has already durably advanced `used`.
pub fn generate_nonce() -> Result<[u8; crate::envelope::NONCE_BYTES], crate::RngError> {
    let mut nonce = [0u8; crate::envelope::NONCE_BYTES];
    getrandom::fill(&mut nonce).map_err(|_| crate::RngError::CsprngUnavailable)?;
    Ok(nonce)
}

/// Rejects a nonce that collides with any currently retained nonce for the
/// same key.
///
/// `nonce_collision_defense`: "reject a duplicate nonce under the same key
/// among every retained active staging migration rollback and recovery
/// envelope; deleted-envelope nonces are not retained, so lifetime uniqueness
/// relies on the NIST random-IV probability bound plus the conservative
/// invocation cap and early rotation". `retained` is exactly that set,
/// supplied by the store for the given key; this function does not persist
/// or expand it.
#[must_use]
pub fn is_nonce_collision(
    candidate: &[u8; crate::envelope::NONCE_BYTES],
    retained: &[[u8; crate::envelope::NONCE_BYTES]],
) -> bool {
    retained.iter().any(|existing| existing == candidate)
}

#[cfg(test)]
mod tests {
    use super::{
        APPLICATION_ROTATION_LIMIT, KeyUsageState, PER_KEY_INVOCATION_HARD_LIMIT, ReservationError,
        checked_reserve, is_nonce_collision,
    };
    use crate::ids::RandomId;

    fn key_id() -> crate::ids::KeyId {
        RandomId::from_random_bytes([4u8; 16]).unwrap()
    }

    #[test]
    fn reserve_batch_rejects_zero_count() {
        let mut state = KeyUsageState::fresh(key_id());
        assert_eq!(state.reserve_batch(0), Err(ReservationError::ZeroCount));
        assert_eq!(state.used, 0, "a rejected reservation never advances used");
    }

    #[test]
    fn multi_record_transaction_gets_distinct_reservations() {
        let mut state = KeyUsageState::fresh(key_id());
        let batch = state.reserve_batch(4).unwrap();
        let indices: Vec<u64> = (0..4).map(|i| batch.attempt_index(i)).collect();
        assert_eq!(indices, vec![0, 1, 2, 3]);
        assert_eq!(state.used, 4);
    }

    /// Simulates "crash between reserve and encrypt": a reservation is taken
    /// (durably advancing `used`), the batch is dropped without ever being
    /// used to encrypt anything, and the next reservation must not reuse any
    /// index from the abandoned batch — because there is no rollback path.
    #[test]
    fn crash_between_reserve_and_encrypt_burns_the_reservation() {
        let mut state = KeyUsageState::fresh(key_id());
        let abandoned = state.reserve_batch(3).unwrap();
        let _ = abandoned; // "crash": nothing ever consumed these three attempts

        let next = state.reserve_batch(2).unwrap();
        assert_eq!(
            next.first_attempt, 3,
            "attempts 0..3 stay burned, never reused"
        );
        assert_eq!(state.used, 5);
    }

    #[test]
    fn reservation_crosses_rotation_limit_then_next_reservation_is_refused() {
        let mut state = KeyUsageState {
            key_id: key_id(),
            used: APPLICATION_ROTATION_LIMIT - 1,
        };
        assert!(!state.needs_rotation());
        let batch = state.reserve_batch(2).unwrap();
        assert_eq!(batch.first_attempt, APPLICATION_ROTATION_LIMIT - 1);
        assert!(state.needs_rotation());
        assert_eq!(
            state.reserve_batch(1),
            Err(ReservationError::RotationRequired)
        );
    }

    #[test]
    fn checked_reserve_hard_stops_before_2_32() {
        assert_eq!(
            checked_reserve(PER_KEY_INVOCATION_HARD_LIMIT - 1, 1),
            Ok(PER_KEY_INVOCATION_HARD_LIMIT)
        );
        assert_eq!(
            checked_reserve(PER_KEY_INVOCATION_HARD_LIMIT, 1),
            Err(ReservationError::HardLimitExceeded)
        );
        assert_eq!(
            checked_reserve(PER_KEY_INVOCATION_HARD_LIMIT - 1, 2),
            Err(ReservationError::HardLimitExceeded)
        );
    }

    #[test]
    fn checked_reserve_rejects_u32_count_overflowing_u64_used() {
        assert_eq!(
            checked_reserve(u64::MAX, 1),
            Err(ReservationError::HardLimitExceeded)
        );
    }

    #[test]
    fn nonce_collision_is_detected_only_within_retained_set() {
        let retained = [[1u8; 12], [2u8; 12]];
        assert!(is_nonce_collision(&[1u8; 12], &retained));
        assert!(!is_nonce_collision(&[3u8; 12], &retained));
    }
}
