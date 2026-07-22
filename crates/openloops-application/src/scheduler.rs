//! Job scheduler skeleton: single-flight per collection key, injected
//! `Clock`/`JobRng`, bounded retry/backoff, quarantine-lane handoff, and the
//! `mail-sync-boundary.json` `poll_policy.cadence_seconds` bounds.
//!
//! No threads and no async runtime: the offline registry mirror this
//! workspace builds from lacks `tokio` (`AGENTS.md`/build-skeleton pin the
//! selected dependency set, and neither this crate's `Cargo.toml` nor the
//! workspace lockfile adds one). Instead this is a tick-driven, synchronous
//! design: a future desktop host owns the actual timer/thread and calls
//! [`JobScheduler::run_due`] once per tick. That call is the composition
//! seam this story leaves for that host; nothing here spawns anything.

use std::collections::HashSet;

use crate::ports::{Clock, JobRng};

/// `poll_policy.cadence_seconds.default`.
pub const POLL_CADENCE_DEFAULT_SECONDS: u32 = 60;
/// `poll_policy.cadence_seconds.minimum`.
pub const POLL_CADENCE_MINIMUM_SECONDS: u32 = 60;
/// `poll_policy.cadence_seconds.maximum`.
pub const POLL_CADENCE_MAXIMUM_SECONDS: u32 = 900;

/// Clamps a requested cadence into the closed
/// `[POLL_CADENCE_MINIMUM_SECONDS, POLL_CADENCE_MAXIMUM_SECONDS]` range.
#[must_use]
pub const fn clamp_poll_cadence_seconds(requested: u32) -> u32 {
    if requested < POLL_CADENCE_MINIMUM_SECONDS {
        POLL_CADENCE_MINIMUM_SECONDS
    } else if requested > POLL_CADENCE_MAXIMUM_SECONDS {
        POLL_CADENCE_MAXIMUM_SECONDS
    } else {
        requested
    }
}

/// An opaque per-account-and-collection identity used only for single-flight
/// bookkeeping; never a Graph ID or readable label.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CollectionKey(pub [u8; 16]);

/// `poll_policy.single_flight`: "exactly one active poll or reconciliation
/// job per account and collection." Deliberately *not* RAII/`Drop`-based:
/// an auto-releasing lease would have to borrow the whole guard for its
/// lifetime, which would make two different collections' leases mutually
/// exclusive too (Rust cannot see that two different `HashSet` keys are
/// disjoint) — exactly backwards from "single-flight *per collection key*".
/// Callers pair [`SingleFlightGuard::try_acquire`] with
/// [`SingleFlightGuard::release`] instead; [`JobScheduler::run_due`] does
/// this pairing itself so its own callers never see the raw guard.
#[derive(Debug, Default)]
pub struct SingleFlightGuard {
    in_flight: HashSet<[u8; 16]>,
}

/// A rejected single-flight acquisition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerError {
    AlreadyInFlight,
}

impl SingleFlightGuard {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquires the lease for `key`, or refuses if one is already held.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::AlreadyInFlight`] if `key` is already
    /// leased.
    pub fn try_acquire(&mut self, key: CollectionKey) -> Result<(), SchedulerError> {
        if self.in_flight.insert(key.0) {
            Ok(())
        } else {
            Err(SchedulerError::AlreadyInFlight)
        }
    }

    /// Releases `key`'s lease. A release of a key that was never (or is no
    /// longer) held is a harmless no-op.
    pub fn release(&mut self, key: CollectionKey) {
        self.in_flight.remove(&key.0);
    }

    /// Whether `key` currently holds a lease.
    #[must_use]
    pub fn is_in_flight(&self, key: CollectionKey) -> bool {
        self.in_flight.contains(&key.0)
    }
}

/// `retry_policy.missing_or_invalid_retry_after`: "bounded exponential
/// backoff with positive jitter and finite cap." Classification (which
/// errors are retryable at all) is the caller's job — `retry_policy` already
/// splits that into `retryable_classes`/`nonretryable_classes`, and this
/// crate does not yet have a live transport to classify — so this struct
/// only owns the bounded arithmetic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackoffPolicy {
    pub base_millis: u32,
    pub cap_millis: u32,
    pub max_attempts: u8,
}

impl BackoffPolicy {
    /// Returns the next delay for `attempt` (0-indexed), or `None` once
    /// `attempt >= max_attempts` (the caller must stop retrying and hand off
    /// to quarantine/failure).
    pub fn next_delay_millis(&self, attempt: u8, rng: &mut dyn JobRng) -> Option<u32> {
        if attempt >= self.max_attempts {
            return None;
        }
        let shift = u32::from(attempt.min(16));
        let exponential = self.base_millis.saturating_mul(1u32 << shift);
        let bounded = exponential.min(self.cap_millis);
        let jitter = rng.positive_jitter_millis(bounded.max(1));
        Some(bounded.saturating_add(jitter))
    }
}

/// Retained on handoff to the recoverable quarantine lane
/// (`quarantine_policy.approved_fields`): the item's encrypted re-fetch
/// locator is never dropped just because retries were exhausted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantineHandoff {
    pub re_fetch_locator: Vec<u8>,
    pub retry_generation: u32,
}

/// Builds a [`QuarantineHandoff`] for one exhausted item, preserving its
/// re-fetch locator (`quarantine_policy.before_cursor_advance`: "persist one
/// validated recoverable record or keep the cursor blocked" — this crate's
/// half of that is never losing the locator on the way in).
#[must_use]
pub fn hand_off_to_quarantine(re_fetch_locator: &[u8], retry_generation: u32) -> QuarantineHandoff {
    QuarantineHandoff {
        re_fetch_locator: re_fetch_locator.to_vec(),
        retry_generation,
    }
}

/// The result of one [`JobScheduler::run_due`] call.
#[derive(Debug)]
pub enum RunDueOutcome<T> {
    /// The cadence has not elapsed since the last run.
    NotDue,
    /// Another job already holds this collection's single-flight lease.
    AlreadyInFlight,
    /// The job ran (`schedule_basis`: "`start_to_start` with no overlap").
    Ran(T),
}

/// Releases a [`SingleFlightGuard`] lease when dropped, scoped to exactly
/// one [`JobScheduler::run_due`] call so a panicking `job` still releases
/// the lease (unlike [`SingleFlightGuard`] itself, which is deliberately not
/// `Drop`-based at the public API level — see its doc comment).
struct ReleaseOnDrop<'a> {
    guard: &'a mut SingleFlightGuard,
    key: CollectionKey,
}

impl Drop for ReleaseOnDrop<'_> {
    fn drop(&mut self) {
        self.guard.release(self.key);
    }
}

/// A tick-driven, synchronous, single-flight job runner.
///
/// `run_due` is the only entry point; there is no background thread, timer,
/// or async task anywhere in this type. A future desktop host is expected to
/// call it from its own timer/event loop, one collection at a time.
pub struct JobScheduler<C: Clock> {
    clock: C,
    cadence_seconds: u32,
    last_run: Option<i64>,
    single_flight: SingleFlightGuard,
}

impl<C: Clock> JobScheduler<C> {
    /// Builds a scheduler; `requested_cadence_seconds` is clamped via
    /// [`clamp_poll_cadence_seconds`] rather than rejected, so a
    /// misconfigured caller still gets a safe, in-bounds cadence.
    #[must_use]
    pub fn new(clock: C, requested_cadence_seconds: u32) -> Self {
        Self {
            clock,
            cadence_seconds: clamp_poll_cadence_seconds(requested_cadence_seconds),
            last_run: None,
            single_flight: SingleFlightGuard::new(),
        }
    }

    #[must_use]
    pub const fn cadence_seconds(&self) -> u32 {
        self.cadence_seconds
    }

    /// Whether a cycle is due at `now` (`schedule_basis`: "`start_to_start`
    /// with no overlap; throttling and offline states delay the next
    /// start").
    #[must_use]
    pub fn is_due(&self, now_epoch_seconds: i64) -> bool {
        match self.last_run {
            None => true,
            Some(last) => now_epoch_seconds.saturating_sub(last) >= i64::from(self.cadence_seconds),
        }
    }

    /// Runs `job` for `collection_key` if (and only if) a cycle is due and no
    /// other job currently holds that collection's single-flight lease.
    /// Marks the cadence's `last_run` only on an actual run, never on a
    /// refused attempt. The lease is always released before returning, even
    /// if `job` panics (`release` runs in this function's own `Drop` guard,
    /// scoped to this call only — see [`SingleFlightGuard`]'s doc comment
    /// for why the lease itself is not `Drop`-based).
    pub fn run_due<T>(
        &mut self,
        collection_key: CollectionKey,
        job: impl FnOnce() -> T,
    ) -> RunDueOutcome<T> {
        let now = self.clock.now().0;
        if !self.is_due(now) {
            return RunDueOutcome::NotDue;
        }
        if self.single_flight.try_acquire(collection_key).is_err() {
            return RunDueOutcome::AlreadyInFlight;
        }
        let result = {
            let _release = ReleaseOnDrop {
                guard: &mut self.single_flight,
                key: collection_key,
            };
            job()
        };
        self.last_run = Some(now);
        RunDueOutcome::Ran(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BackoffPolicy, CollectionKey, JobScheduler, POLL_CADENCE_MAXIMUM_SECONDS,
        POLL_CADENCE_MINIMUM_SECONDS, RunDueOutcome, SchedulerError, SingleFlightGuard,
        clamp_poll_cadence_seconds, hand_off_to_quarantine,
    };
    use crate::ports::{FixedClock, FixedJobRng};
    use openloops_domain::deadline::UnixSeconds;

    #[test]
    fn cadence_clamps_to_the_contract_bounds() {
        assert_eq!(clamp_poll_cadence_seconds(1), POLL_CADENCE_MINIMUM_SECONDS);
        assert_eq!(clamp_poll_cadence_seconds(60), 60);
        assert_eq!(clamp_poll_cadence_seconds(900), 900);
        assert_eq!(
            clamp_poll_cadence_seconds(10_000),
            POLL_CADENCE_MAXIMUM_SECONDS
        );
    }

    #[test]
    fn single_flight_refuses_a_second_lease_for_the_same_key() {
        let mut guard = SingleFlightGuard::new();
        let key = CollectionKey([1u8; 16]);
        guard.try_acquire(key).unwrap();
        assert_eq!(
            guard.try_acquire(key).unwrap_err(),
            SchedulerError::AlreadyInFlight
        );
        guard.release(key);
        assert!(guard.try_acquire(key).is_ok());
    }

    #[test]
    fn single_flight_is_independent_per_key() {
        let mut guard = SingleFlightGuard::new();
        let a = CollectionKey([1u8; 16]);
        let b = CollectionKey([2u8; 16]);
        guard.try_acquire(a).unwrap();
        assert!(guard.try_acquire(b).is_ok());
        assert!(guard.is_in_flight(a));
        assert!(guard.is_in_flight(b));
    }

    #[test]
    fn backoff_stops_at_max_attempts() {
        let policy = BackoffPolicy {
            base_millis: 100,
            cap_millis: 10_000,
            max_attempts: 3,
        };
        let mut rng = FixedJobRng(0);
        assert!(policy.next_delay_millis(0, &mut rng).is_some());
        assert!(policy.next_delay_millis(2, &mut rng).is_some());
        assert_eq!(policy.next_delay_millis(3, &mut rng), None);
    }

    #[test]
    fn backoff_never_exceeds_the_cap_plus_its_own_jitter_bound() {
        let policy = BackoffPolicy {
            base_millis: 1_000,
            cap_millis: 5_000,
            max_attempts: 10,
        };
        let mut rng = FixedJobRng(u32::MAX);
        let delay = policy.next_delay_millis(9, &mut rng).unwrap();
        assert!(delay <= 5_000 + 5_000);
    }

    #[test]
    fn quarantine_handoff_preserves_the_re_fetch_locator() {
        let handoff = hand_off_to_quarantine(&[9, 8, 7], 2);
        assert_eq!(handoff.re_fetch_locator, vec![9, 8, 7]);
        assert_eq!(handoff.retry_generation, 2);
    }

    #[test]
    fn run_due_respects_cadence_and_single_flight() {
        let clock = FixedClock(UnixSeconds(1_000));
        let mut scheduler = JobScheduler::new(clock, 60);
        let key = CollectionKey([1u8; 16]);

        match scheduler.run_due(key, || 42) {
            RunDueOutcome::Ran(value) => assert_eq!(value, 42),
            other => panic!("expected Ran, got {other:?}"),
        }
        // Same instant again: not due yet.
        match scheduler.run_due(key, || 99) {
            RunDueOutcome::NotDue => {}
            other => panic!("expected NotDue, got {other:?}"),
        }
    }

    #[test]
    fn run_due_marks_last_run_only_on_an_actual_run() {
        let clock = FixedClock(UnixSeconds(0));
        let mut scheduler = JobScheduler::new(clock, 60);
        let key = CollectionKey([2u8; 16]);
        assert!(scheduler.is_due(0));
        let _ = scheduler.run_due(key, || ());
        assert!(!scheduler.is_due(30));
        assert!(scheduler.is_due(60));
    }
}
