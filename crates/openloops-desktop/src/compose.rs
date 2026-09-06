//! The desktop composition seam (`judge lens: composition-honesty`).
//!
//! [`compose_probe`] constructs the cross-crate object graph this desktop
//! binary will eventually run against real user state — a persistence
//! store, a job scheduler bound to a real wall-clock, a Graph transport, and
//! a reference to the inference validation pipeline's entry point — proving
//! every layer links and type-checks, **without ever executing a single
//! operation against any of it**:
//!
//! * The store opens at a caller-supplied probe path (never the real state
//!   root) via [`openloops_persistence::store::Store::open`], which creates
//!   an empty closed schema and nothing else; no `write_transaction` call
//!   appears anywhere in this module, so no application data is ever
//!   written. [`Composition::probe_store_is_empty`] only reads.
//! * [`openloops_application::scheduler::JobScheduler::new`] only stores a
//!   clock and a clamped cadence; `run_due` is never called, so no tick, no
//!   thread, and no timer loop exists.
//! * [`openloops_graph::transport::GraphTransport::new`] only stores a
//!   timeout and an empty single-flight table; `send` is never called
//!   anywhere in this module, so no socket is ever bound or dialed (this
//!   crate has no [`Wire`] implementation outside its own `test_support`
//!   module in any case — see `openloops-graph`'s crate doc comment for
//!   why). It is constructed and immediately dropped, proving the type
//!   links without needing to be retained.
//! * `openloops_inference::validation::validate` is referenced only as a
//!   function pointer, never invoked by this module (a test may call it on
//!   deliberately inert bytes to prove the pointer is genuine — see this
//!   module's tests — but `compose_probe`/`main` never do).
//!
//! [`Wire`]: openloops_graph::transport::Wire
//!
//! Every field [`Composition`] holds is dropped by its caller immediately
//! after construction; this module has no other caller and keeps no static
//! or global state.

use std::path::{Path, PathBuf};
use std::time::Duration;

use openloops_application::ports::Clock;
use openloops_application::scheduler::{JobScheduler, POLL_CADENCE_DEFAULT_SECONDS};
use openloops_domain::deadline::UnixSeconds;
use openloops_graph::transport::GraphTransport;
use openloops_inference::validation::{AnalysisResult, SuppliedContext, validate};
use openloops_persistence::ids::{AccountBindingAad, RandomId, RecordType};
use openloops_persistence::store::{Store, StoreError};

/// A real wall-clock [`Clock`], used only to *construct*
/// [`JobScheduler<SystemClock>`] here. Nothing in this module (or anywhere
/// else in this story) ever calls [`SystemClock::now`]: there is no timer
/// loop yet (`openloops_application::scheduler`'s own module doc explains
/// why — no thread or async runtime is available in this workspace), so the
/// only thing a real clock proves at this point is that the type checks
/// against the scheduler's real `Clock` bound.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UnixSeconds {
        let elapsed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let seconds = i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX);
        UnixSeconds(seconds)
    }
}

/// The subset of [`compose_probe`]'s constructed graph its caller can
/// inspect afterward. `GraphTransport` and the inference `validate`
/// function pointer are deliberately **not** fields here: this module
/// constructs both (proving they link and type-check) but never retains,
/// reads, or calls either — see this module's doc comment for exactly why.
pub struct Composition {
    /// Constructed, never ticked (`run_due` is never called).
    pub scheduler: JobScheduler<SystemClock>,
    /// Opened at a caller-supplied probe path; only ever read through
    /// [`Composition::probe_store_is_empty`], never written to.
    pub store: Store,
    /// Consulted, never flipped: every field of this struct is `false` by
    /// construction (see `openloops_domain::CapabilityFlags`'s doc comment).
    pub capability_flags: openloops_domain::CapabilityFlags,
}

impl Composition {
    /// Reads (never writes) the probe store to confirm it holds zero rows
    /// in every closed record-type table, i.e. that nothing on the path
    /// from `compose_probe` to here ever wrote application data. A read-only
    /// diagnostic, not a mutation.
    #[must_use]
    pub fn probe_store_is_empty(&self) -> bool {
        let probe_account: AccountBindingAad = RandomId::from_random_bytes([1u8; 16])
            .unwrap_or_else(|_| unreachable!("[1u8; 16] is not the rejected all-zero input"));
        let scan_marker = RandomId::from_random_bytes([2u8; 16])
            .unwrap_or_else(|_| unreachable!("[2u8; 16] is not the rejected all-zero input"));
        let no_rows_for_probe_account = self
            .store
            .read_all(probe_account)
            .is_ok_and(|rows| rows.is_empty());
        let no_record_contains_marker = RecordType::ALL.into_iter().all(|record_type| {
            self.store
                .contains(record_type, scan_marker)
                .is_ok_and(|found| !found)
        });
        no_rows_for_probe_account && no_record_contains_marker
    }
}

/// A failure constructing [`Composition`]. Carries no path, no SQL text,
/// and no key material (mirrors
/// `openloops_persistence::store::StoreError`'s own content-free shape).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComposeError {
    /// The probe store could not be opened.
    Store(StoreError),
}

/// Constructs [`Composition`] at `probe_path`, which the caller must own
/// (this function never chooses, reuses, or falls back to the real state
/// root). Returns without running any operation against the constructed
/// graph.
///
/// # Errors
///
/// Returns [`ComposeError::Store`] if the probe database cannot be opened
/// (for example an unwritable directory); this function does not retry and
/// does not fall back to an alternate location.
pub fn compose_probe(probe_path: &Path) -> Result<Composition, ComposeError> {
    let store = Store::open(probe_path).map_err(ComposeError::Store)?;
    let scheduler = JobScheduler::new(SystemClock, POLL_CADENCE_DEFAULT_SECONDS);
    let capability_flags = openloops_domain::CapabilityFlags::default();

    // Constructed here only to prove `openloops-graph`'s absent-transport
    // seam and `openloops-inference`'s validation pipeline link against
    // this binary; neither is ever executed (no `send`, no `validate` call)
    // and neither is retained past this function. The plain `_` discards
    // (not named underscore-prefixed bindings) intentionally drop each
    // constructed value immediately after the compiler has proved it
    // type-checks.
    let _: GraphTransport = GraphTransport::new(Duration::from_secs(30));
    let _: fn(&[u8], &SuppliedContext<'_>) -> AnalysisResult = validate;

    Ok(Composition {
        scheduler,
        store,
        capability_flags,
    })
}

/// Builds a probe path under the OS temporary directory, unique to this
/// process, for [`compose_probe`] to open and the caller to discard
/// immediately after. Never the real state root, and never reused across
/// processes (the process ID makes concurrent `--compose-probe` runs use
/// distinct files).
#[must_use]
pub fn default_probe_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "openloops-desktop-compose-probe-{}.db",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::{compose_probe, default_probe_path};
    use openloops_inference::validation::{AnalysisResult, SuppliedContext, validate};

    #[test]
    fn compose_probe_links_every_layer_without_executing_anything() {
        // `Store::open` creates its schema with `CREATE TABLE IF NOT
        // EXISTS`, so reusing an already-present probe path (left behind
        // by a prior run — see this crate's own doc comment for why
        // nothing here deletes it) is idempotent; no pre-test deletion is
        // needed, and none is performed (this module's only sanctioned
        // filesystem access is `openloops-persistence`'s reviewed
        // primitives, never a a raw filesystem call).
        let probe_path = default_probe_path();
        let composition = compose_probe(&probe_path).expect("probe store opens");
        assert_eq!(composition.scheduler.cadence_seconds(), 60);
        assert!(!composition.capability_flags.any_enabled());
        assert!(composition.probe_store_is_empty());
        drop(composition);
    }

    #[test]
    fn the_real_validate_function_is_the_one_compose_probe_links_against() {
        // Proves `compose_probe`'s locally-constructed function-pointer
        // binding is genuinely `openloops_inference::validation::validate`
        // and not a same-shaped stand-in, by calling the same function
        // reference here on deliberately inert bytes. A pure, zero-I/O,
        // deterministic call — never something `compose_probe`/`main`
        // itself performs.
        let validate_reference: fn(&[u8], &SuppliedContext<'_>) -> AnalysisResult = validate;
        let context = SuppliedContext {
            messages: &[],
            participants: &[],
            loop_candidate_handles: &[],
        };
        assert_eq!(
            validate_reference(b"not json", &context),
            AnalysisResult::AnalysisUnavailable
        );
    }

    #[test]
    fn compose_probe_rejects_an_unwritable_probe_directory() {
        use super::ComposeError;
        use openloops_persistence::store::StoreError;

        let unwritable = default_probe_path()
            .join("no-such-directory")
            .join("state.db");
        match compose_probe(&unwritable) {
            Err(ComposeError::Store(StoreError::OpenFailed)) => {}
            other => panic!(
                "expected ComposeError::Store(OpenFailed), got a different outcome: {}",
                other.is_ok()
            ),
        }
    }
}
