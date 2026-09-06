//! Application-layer ports (implementation-plan §2.2): the trait boundaries
//! `ingestion`/`ledger`/`scheduler` are written against, so every call site
//! is mockable without a real Graph transport, adapter, or system clock.
//!
//! # Why there is no `StateStore` trait
//!
//! `openloops_persistence::store::Store` already *is* the tested, atomic,
//! encrypted-envelope port implementation (`database_contract`,
//! `transaction_contract`). Introducing `trait StateStore { .. }` over it
//! would force a choice between two bad options: (a) an associated type or
//! generic parameter that just leaks `Store`'s concrete type back through
//! the "port", defeating the point, or (b) re-declaring `Store`'s already
//! reviewed, pragma-verified, crash-tested method set behind a second,
//! untested indirection some future mock could silently drift from — a
//! duplicated, unverified copy of semantics this crate did not write and
//! should not re-certify. Every module here that needs durable state
//! therefore takes `&mut openloops_persistence::store::Store` directly, and
//! `Store::open_in_memory` is this crate's own test double; `MailSource`,
//! `ReminderAdapter`, and `JobRng` are the ports that gate genuinely
//! not-yet-implemented external systems (a Graph transport, an adapter
//! endpoint, an RNG source), which is exactly what implementation-plan §2.2
//! lists a port for.

pub use openloops_domain::deadline::{Clock, FixedClock};

/// `mail-sync-boundary.json` `eligibility_policy`: incoming vs. outgoing
/// (saved sent copy) vs. draft/compose. ADR-004: "drafts and compose
/// activity never activate loops."
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Incoming,
    OutgoingSavedSentCopy,
    DraftOrCompose,
}

/// The read-transition shapes `eligibility_policy.post_baseline_incoming`
/// names explicitly. Meaningful only when [`Direction::Incoming`]; ADR-004:
/// "`isRead` is a trigger, not evidence of attention."
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservedReadState {
    /// A previously-observed-unread item is now observed read.
    UnreadToRead,
    /// The first observation of this item is already read (polling may have
    /// coalesced the unread state away).
    FirstObservedRead,
    /// The first observation of this item is unread; wait for a later read
    /// observation.
    FirstObservedUnread,
    /// Observed unread again; no eligible transition.
    StillUnread,
}

/// One delta-page item exactly as [`MailSource`] returns it: every field is
/// already the bounded, opaque handle `page_transaction` step 4 requires
/// ("fetch only the exact G-MAIL and ADR-006 field contract transiently").
/// No participant, subject, body, or raw Graph identifier crosses this
/// boundary; that projection belongs to a future ADR-006/G-MAIL adapter, not
/// this port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeltaPageItem {
    /// `observation_digest_contract` `source_version_v1`-shaped input: the
    /// tested, mailbox-bound message locator bytes (opaque; never a raw
    /// Graph ID).
    pub tested_locator: Vec<u8>,
    /// The paired observed-version component for the same digest.
    pub observed_version: Vec<u8>,
    pub direction: Direction,
    /// Meaningful only when `direction == Direction::Incoming`.
    pub read_state: ObservedReadState,
    /// Meaningful only when `direction == Direction::OutgoingSavedSentCopy`:
    /// `eligibility_policy.post_baseline_outgoing.saved_sent_copy_first_observed`.
    pub first_observed: bool,
    /// True while this item is part of the initial/backfill history batch
    /// (`eligibility_policy.initial_or_backfill`); false once the collection
    /// has a committed baseline.
    pub in_backfill_batch: bool,
    /// ADR-PRIV-001-approved encrypted re-fetch locator bytes, retained on
    /// quarantine (`quarantine_policy.approved_fields.quarantined_locator`).
    /// Opaque here; this port's future implementor owns the actual
    /// encryption.
    pub re_fetch_locator: Vec<u8>,
}

/// One returned delta page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeltaPage {
    pub items: Vec<DeltaPageItem>,
    /// Opaque encrypted continuation bytes; never parsed, reconstructed,
    /// edited, or reused across a binding (`delta_protocol.continuations`).
    pub next_cursor: Vec<u8>,
    /// True only when `next_cursor` is a closing `deltaLink`
    /// (`delta_protocol.completion`): "marks a completed round only after
    /// every page item has a durable safe outcome."
    pub closes_round: bool,
}

/// `page_transaction.whole_page_failure`: "transport parse or schema failure
/// blocks checkpoint replacement because no safe per-item disposition
/// exists."
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailSourceError {
    Transport,
    Parse,
    Schema,
}

/// Delta pages, one bounded fetch at a time (implementation-plan §2.2:
/// "initial window, folder delta, bounded message fetch, link resolution").
pub trait MailSource {
    /// Returns the next page after `cursor` (`None` for a collection's very
    /// first page).
    ///
    /// # Errors
    ///
    /// Returns [`MailSourceError`] on a whole-page transport, parse, or
    /// schema failure; [`crate::ingestion`] never advances the checkpoint on
    /// this path.
    fn fetch_next_page(&mut self, cursor: Option<&[u8]>) -> Result<DeltaPage, MailSourceError>;
}

/// `adapter-boundary.json` `catalogs.operation_kind_code`, in exact catalog
/// order (the ordinal backs this crate's wire encoding and the operation-key
/// HMAC component in [`crate::ledger`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum OperationKind {
    TodoCreate,
    TodoUpdateOwnedFields,
    TodoComplete,
    CalendarReminderCreate,
    CalendarReminderUpdateOwnedFields,
    ManualTodoCreate,
    ManualCalendarCreate,
    RecreateAfterUserConfirmation,
}

impl OperationKind {
    /// The closed catalog ordinal (`0..=7`).
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::TodoCreate => 0,
            Self::TodoUpdateOwnedFields => 1,
            Self::TodoComplete => 2,
            Self::CalendarReminderCreate => 3,
            Self::CalendarReminderUpdateOwnedFields => 4,
            Self::ManualTodoCreate => 5,
            Self::ManualCalendarCreate => 6,
            Self::RecreateAfterUserConfirmation => 7,
        }
    }

    /// Decodes a closed catalog ordinal.
    ///
    /// # Errors
    ///
    /// Returns `Err(code)` for any value outside `0..=7`.
    pub const fn from_code(code: u8) -> Result<Self, u8> {
        Ok(match code {
            0 => Self::TodoCreate,
            1 => Self::TodoUpdateOwnedFields,
            2 => Self::TodoComplete,
            3 => Self::CalendarReminderCreate,
            4 => Self::CalendarReminderUpdateOwnedFields,
            5 => Self::ManualTodoCreate,
            6 => Self::ManualCalendarCreate,
            7 => Self::RecreateAfterUserConfirmation,
            other => return Err(other),
        })
    }
}

/// The single bounded request `operation_protocol.durability_order` allows
/// per attempt ("make at most one adapter request").
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterRequest {
    pub operation_kind: OperationKind,
    /// Opaque adapter/collection routing bytes only; never a raw Graph ID or
    /// human-readable value.
    pub destination_hint: Vec<u8>,
}

/// The result of that one bounded request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdapterOutcome {
    Succeeded {
        destination_locator: Vec<u8>,
    },
    /// `operation_protocol.known_no_send`: endpoint-independent proof that
    /// nothing was sent.
    KnownNoSend,
    /// Timeout or otherwise unknown outcome.
    Ambiguous,
    DefinitiveFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterTransportError {
    Unavailable,
}

/// Propose/create/reconcile operations (implementation-plan §2.2). The
/// [`crate::ledger`] module is this trait's only caller, and only calls it
/// after its own commit-before-request steps, so an implementor never needs
/// to enforce durability itself.
pub trait ReminderAdapter {
    /// Makes at most one bounded request.
    ///
    /// # Errors
    ///
    /// Returns [`AdapterTransportError`] when the adapter cannot be reached
    /// at all (never a partial/ambiguous result — that is
    /// [`AdapterOutcome::Ambiguous`]).
    fn attempt(
        &mut self,
        request: &AdapterRequest,
    ) -> Result<AdapterOutcome, AdapterTransportError>;
}

/// `retry_policy.missing_or_invalid_retry_after`: "bounded exponential
/// backoff with positive jitter." This port supplies only the jitter; the
/// exponential-backoff arithmetic lives in [`crate::scheduler`] so it stays
/// independently testable from whatever RNG a future desktop host wires in.
pub trait JobRng {
    /// Returns a value in `0..bound_millis` (or exactly `0` when
    /// `bound_millis == 0`).
    fn positive_jitter_millis(&mut self, bound_millis: u32) -> u32;
}

/// A fixed jitter source for tests: always returns `min(self.0, bound)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixedJobRng(pub u32);

impl JobRng for FixedJobRng {
    fn positive_jitter_millis(&mut self, bound_millis: u32) -> u32 {
        self.0.min(bound_millis)
    }
}

#[cfg(test)]
mod tests {
    use super::{FixedJobRng, JobRng, OperationKind};

    #[test]
    fn operation_kind_round_trips_every_closed_code() {
        for code in 0u8..=7 {
            let kind = OperationKind::from_code(code).expect("code is in closed catalog");
            assert_eq!(kind.code(), code);
        }
    }

    #[test]
    fn operation_kind_rejects_out_of_range_code() {
        assert_eq!(OperationKind::from_code(8), Err(8));
    }

    #[test]
    fn fixed_job_rng_never_exceeds_bound() {
        let mut rng = FixedJobRng(500);
        assert_eq!(rng.positive_jitter_millis(1_000), 500);
        assert_eq!(rng.positive_jitter_millis(100), 100);
        assert_eq!(rng.positive_jitter_millis(0), 0);
    }
}
