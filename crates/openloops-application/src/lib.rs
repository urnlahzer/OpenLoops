#![forbid(unsafe_code)]
//! `openloops-application`: the ADR-004/ADR-008/ADR-009 use-case layer.
//!
//! This crate composes `openloops-domain` (policy/state) and
//! `openloops-persistence` (the encrypted, atomic `Store`) through the
//! `implementation-plan.md` §2.2 ports, plus the page-atomic ingestion
//! transaction, the durable-before-request operation ledger, and a
//! tick-driven job scheduler skeleton. [`is_available`] still returns
//! `false`: no capability is enabled, no gate advances, and nothing outside
//! this crate's own tests calls into it.
//!
//! | Module | Contract section |
//! |---|---|
//! | [`ports`] | implementation-plan §2.2 interface boundaries (`MailSource`, `ReminderAdapter`, `JobRng`, `Clock`) |
//! | [`codec`] | this crate's own fixed-width/length-prefixed logical-record wire format |
//! | [`seal`] | composes `openloops-persistence`'s reservation/envelope/store primitives into one sealing/scanning sequence |
//! | [`ingestion`] | ADR-004 `page_transaction` (mail-sync-boundary.json) |
//! | [`ledger`] | ADR-009 `operation_protocol` (adapter-boundary.json), including the repaired `same_key`/`known_no_send` semantics |
//! | [`scheduler`] | single-flight, cadence-bounded, tick-driven job runner skeleton |
//!
//! # Why there is no `StateStore` trait
//!
//! See [`ports`]'s module doc comment: `openloops_persistence::store::Store`
//! is used directly everywhere, never wrapped in a second trait.
//!
//! # Honest scope limits (read before relying on this crate for anything real)
//!
//! * **No `Extractor`/`PolicyEngine` is wired.** [`ingestion::SurfaceToReviewClassifier`]
//!   surfaces every eligible item to human review; autonomous analysis is a
//!   separate future story.
//! * **`sync_checkpoint` and `operation_ledger` are append-only logs, not
//!   mutable rows.** `openloops_persistence::store::Store` exposes insert
//!   and full-account scan only (no UPDATE); "current state" is always the
//!   highest-generation row for a given grouping key, found by scanning.
//!   This is the only pattern the store's genuinely tested primitives
//!   support; see [`ingestion`] and [`ledger`]'s module docs.
//! * **Lookups are full account scans.** [`seal::scan_records`] is `O(n)` in
//!   the account's total row count; a production deployment needs a
//!   queryable index, which is out of this story's scope.
//! * **Key/nonce lifecycle (`state_root`, DPAPI-backed key loading, the
//!   nonce-retention collision set) is not implemented here.** [`ingestion::IngestionContext`]
//!   and [`ledger::LedgerContext`] borrow an already-provisioned
//!   `EnvelopeKey`/`KeyUsageState`/`ContentDigestKey`; provisioning them is a
//!   future desktop-host composition seam, not this crate's job.
//! * **`ledger::classify_outcome`'s ambiguous/definitive-failure reason code
//!   is caller-supplied, not inferred.** Only a real adapter (a future
//!   story) knows which of the 17 catalog reasons applies.
//! * **The scheduler has no real RNG or timer.** [`scheduler::JobScheduler`]
//!   is generic over [`ports::Clock`] and takes a caller-supplied
//!   [`ports::JobRng`]; wiring a real CSPRNG-backed jitter source and a real
//!   host timer loop is the future desktop host's job (`scheduler`'s module
//!   doc explains why: no threads/async runtime is available in this
//!   workspace).
//! * **[`ledger::operation_key_hmac`] cannot be computed yet.**
//!   `contracts/persistence/protected-state-boundary.json`
//!   `digest_suite.purpose_catalog` names `operation_ledger.operation_key_hmac`'s
//!   owner as "ADR-009 and ADR-011"; ADR-011 has not closed its half, so
//!   `openloops_persistence::digest` still refuses this purpose and
//!   `operation_key_hmac` unconditionally returns
//!   `DigestError::LayoutNotYetOwned`. [`ledger`]'s durability/replay/
//!   collision/retry state machine does not depend on that call succeeding —
//!   every entry point takes an already-computed key — so it is fully
//!   implemented and tested against synthetic keys today; see [`ledger`]'s
//!   own "Honest gap" module doc section.

use openloops_contracts::{SkeletonStatus, synthetic_status as generated_synthetic_status};
use openloops_domain::CapabilityState;

pub mod codec;
pub mod ingestion;
pub mod ledger;
pub mod ports;
pub mod scheduler;
pub mod seal;

#[must_use]
pub fn synthetic_status() -> SkeletonStatus {
    debug_assert!(!CapabilityState::Disabled.is_enabled());
    generated_synthetic_status()
}

/// This story activates no capability and passes no gate.
#[must_use]
pub const fn is_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn synthetic_status_is_content_free_and_disabled() {
        let status = super::synthetic_status();
        assert!(openloops_contracts::has_no_claims(&status));
    }

    #[test]
    fn application_is_unavailable() {
        assert!(!super::is_available());
    }
}
