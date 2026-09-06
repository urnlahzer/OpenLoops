//! The ADR-009 `operation_protocol` durable-before-request operation ledger,
//! implemented over `openloops_persistence`'s `operation_ledger` record type.
//!
//! This is the repaired semantics from `contracts/reminder/adapter-boundary.json`
//! `operation_protocol`, not the earlier draft. ADR-009 states this exactly:
//! "This replay path is distinct from an eligible retry: replay never issues
//! a request even while a retry remains eligible, and an eligible retry
//! never reuses the replay path to skip its own durability step." That
//! means **two separate entry points**, not one function branching on stored
//! state:
//!
//! * [`begin_or_replay`] is the *only* path a duplicate external invocation
//!   ever reaches. For any `operation_id` with an existing row under the
//!   same key, it **always** returns [`BeginOutcome::Replayed`] with **zero**
//!   new writes — regardless of whether the latest state happens to be
//!   `known_no_send_retryable` with attempts remaining. A duplicate
//!   invocation can never substitute for the eligible-retry path.
//! * [`commit_eligible_retry`] is the **distinct**, separately invoked path:
//!   the caller (never `begin_or_replay` itself) decides — as a deliberate
//!   scheduled-retry action, not in response to a redelivered/duplicate
//!   invocation — to retry, and passes in the record it already holds.
//!   Only then, if that record is `known_no_send_retryable` with
//!   `attempt_count < 3`, does the next `attempt_started` row commit before
//!   the caller's one bounded request. Nothing in [`begin_or_replay`] can
//!   reach this transition; it is not reachable via replay at all.
//!
//! Same key with a *different* intent is a typed `operation_key_collision`,
//! never silently accepted.
//!
//! Like [`crate::ingestion`], every ledger row is append-only (`Store` has no
//! UPDATE primitive): "the current state of operation `operation_id`" is the
//! highest-`generation` row whose `operation_id` matches, found by scanning.
//!
//! # Honest gap: `operation_key_hmac` cannot be computed yet
//!
//! `contracts/persistence/protected-state-boundary.json`
//! `digest_suite.purpose_catalog` names `operation_ledger.operation_key_hmac`'s
//! owner as **"ADR-009 and ADR-011"**, and closes a purpose's layout only
//! "until every named owner closes them." ADR-009's `intent_hmac_input_order`
//! fully specifies this purpose's 18 components (see [`operation_key_hmac`]),
//! but ADR-011 has not addressed the operation-key layout at all, so the
//! second named owner has not closed its half; `openloops_persistence::digest`
//! therefore still reports [`PurposeTag::OperationLedgerOperationKey`] as
//! `unavailable_pending_owner_ADR` and [`operation_key_hmac`] always returns
//! [`DigestError::LayoutNotYetOwned`] as of this story. This is deliberate,
//! not an oversight: an earlier draft of this module marked the purpose
//! closed on ADR-009's authority alone, which contradicted the persistence
//! crate's own binding contract (a direct code-vs-contract inconsistency) and
//! has been reverted.
//!
//! Everything else in this module — the durability/replay/collision/retry
//! state machine below — does not need to know *how* an operation key was
//! produced; it only ever compares already-computed 32-byte keys for
//! equality (against a [`LedgerRecord`]'s stored `operation_key_hmac`, or
//! directly in [`restart_reconstruction_matches`]). So [`begin_or_replay`],
//! this module's only entry point that establishes a *new* key, takes an
//! already-computed `operation_key: [u8; 32]` rather than an
//! [`OperationIntent`] and calling [`operation_key_hmac`] itself. That keeps
//! the full ADR-009 state machine implemented and tested today against
//! synthetic keys, while the one piece that is genuinely blocked on a second
//! ADR owner — turning a real intent into that key — stays refused and is
//! tested only for that refusal. A caller that reaches this module in
//! production must call [`operation_key_hmac`] first and propagate its
//! (currently unconditional) [`DigestError`] rather than ever fabricating a
//! key another way.

use openloops_domain::deadline::UnixSeconds;
use openloops_domain::ids::OpaqueId;
use openloops_persistence::digest::{ContentDigestKey, DigestError, PurposeTag};
use openloops_persistence::envelope::EnvelopeKey;
use openloops_persistence::ids::{AccountBindingAad, RecordType, SchemaVersion};
use openloops_persistence::reservation::KeyUsageState;
use openloops_persistence::store::Store;

use crate::codec::{CodecError, Reader, push_bytes, push_option_i64, push_u8, push_u32};
use crate::ports::{AdapterOutcome, OperationKind};
use crate::seal::{ScanError, SealError, commit_batch, scan_records, seal_batch};

/// `adapter-boundary.json` `catalogs.adapter_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterCode {
    MicrosoftTodo,
    OutlookCalendar,
}

impl AdapterCode {
    const fn code(self) -> u8 {
        match self {
            Self::MicrosoftTodo => 0,
            Self::OutlookCalendar => 1,
        }
    }
}

/// `adapter-boundary.json` `catalogs.field_mask_code` (bound: at most 5).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum FieldMaskCode {
    Title,
    Body,
    Due,
    ReminderTime,
    Completion,
}

impl FieldMaskCode {
    const fn code(self) -> u8 {
        match self {
            Self::Title => 0,
            Self::Body => 1,
            Self::Due => 2,
            Self::ReminderTime => 3,
            Self::Completion => 4,
        }
    }
}

/// `adapter-boundary.json` `catalogs.operation_state_code`, in exact catalog
/// order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationState {
    Pending,
    AttemptStarted,
    KnownNoSendRetryable,
    Ambiguous,
    Reconciling,
    UserAssisted,
    Succeeded,
    DefinitiveFailed,
    Abandoned,
}

impl OperationState {
    const fn code(self) -> u8 {
        match self {
            Self::Pending => 0,
            Self::AttemptStarted => 1,
            Self::KnownNoSendRetryable => 2,
            Self::Ambiguous => 3,
            Self::Reconciling => 4,
            Self::UserAssisted => 5,
            Self::Succeeded => 6,
            Self::DefinitiveFailed => 7,
            Self::Abandoned => 8,
        }
    }

    const fn from_code(code: u8) -> Result<Self, CodecError> {
        Ok(match code {
            0 => Self::Pending,
            1 => Self::AttemptStarted,
            2 => Self::KnownNoSendRetryable,
            3 => Self::Ambiguous,
            4 => Self::Reconciling,
            5 => Self::UserAssisted,
            6 => Self::Succeeded,
            7 => Self::DefinitiveFailed,
            8 => Self::Abandoned,
            other => return Err(CodecError::UnknownTag(other)),
        })
    }

    /// `operation_protocol.cursor_rule`: "pending `attempt_started` ambiguous
    /// reconciling and `user_assisted` are nonterminal and prevent dependent
    /// cursor advancement."
    #[must_use]
    pub const fn blocks_dependent_cursor(self) -> bool {
        matches!(
            self,
            Self::Pending
                | Self::AttemptStarted
                | Self::Ambiguous
                | Self::Reconciling
                | Self::UserAssisted
        )
    }
}

/// `adapter-boundary.json` `catalogs.ambiguity_or_failure_code`, in exact
/// catalog order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmbiguityOrFailureCode {
    None,
    TimeoutUnknown,
    TransportKnownNoSend,
    ThrottledKnownNoSend,
    CompleteZero,
    CompleteMany,
    EnumerationIncomplete,
    MarkerMissing,
    MarkerUnsupported,
    MarkerRemoved,
    StaleVersion,
    OwnershipConflict,
    AccountMismatch,
    AuthorityChanged,
    SecureStoreUnavailable,
    RollbackSuspected,
    ManualDraftUnrecoverable,
}

impl AmbiguityOrFailureCode {
    const fn code(self) -> u8 {
        match self {
            Self::None => 0,
            Self::TimeoutUnknown => 1,
            Self::TransportKnownNoSend => 2,
            Self::ThrottledKnownNoSend => 3,
            Self::CompleteZero => 4,
            Self::CompleteMany => 5,
            Self::EnumerationIncomplete => 6,
            Self::MarkerMissing => 7,
            Self::MarkerUnsupported => 8,
            Self::MarkerRemoved => 9,
            Self::StaleVersion => 10,
            Self::OwnershipConflict => 11,
            Self::AccountMismatch => 12,
            Self::AuthorityChanged => 13,
            Self::SecureStoreUnavailable => 14,
            Self::RollbackSuspected => 15,
            Self::ManualDraftUnrecoverable => 16,
        }
    }

    const fn from_code(code: u8) -> Result<Self, CodecError> {
        Ok(match code {
            0 => Self::None,
            1 => Self::TimeoutUnknown,
            2 => Self::TransportKnownNoSend,
            3 => Self::ThrottledKnownNoSend,
            4 => Self::CompleteZero,
            5 => Self::CompleteMany,
            6 => Self::EnumerationIncomplete,
            7 => Self::MarkerMissing,
            8 => Self::MarkerUnsupported,
            9 => Self::MarkerRemoved,
            10 => Self::StaleVersion,
            11 => Self::OwnershipConflict,
            12 => Self::AccountMismatch,
            13 => Self::AuthorityChanged,
            14 => Self::SecureStoreUnavailable,
            15 => Self::RollbackSuspected,
            16 => Self::ManualDraftUnrecoverable,
            other => return Err(CodecError::UnknownTag(other)),
        })
    }
}

/// `operation_protocol.intent_hmac_input_order`'s 18 ordered components,
/// typed. Every `..._or_none` component is `Option`; [`operation_key_hmac`]
/// frames `None` as a zero-length component and `Some` as its fixed-width
/// bytes, so presence is unambiguous without a separate tag byte (two
/// different lengths can never collide inside `digest::frame`'s own
/// length-prefixed framing).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationIntent {
    pub account_ref: OpaqueId,
    pub adapter_code: AdapterCode,
    pub collection_ref: OpaqueId,
    pub loop_id: OpaqueId,
    pub source_ref_id: Option<OpaqueId>,
    pub source_generation: u32,
    pub operation_kind: OperationKind,
    /// Canonical (sorted, duplicate-free) order; bound to at most 5 per
    /// `record_contracts.bounds.field_mask_codes`.
    pub canonical_field_mask: Vec<FieldMaskCode>,
    pub expected_loop_version: u64,
    pub expected_remote_version_hmac: Option<[u8; 32]>,
    pub transient_title_digest_hmac: Option<[u8; 32]>,
    pub transient_body_digest_hmac: Option<[u8; 32]>,
    pub transient_due_digest_hmac: Option<[u8; 32]>,
    pub transient_reminder_time_digest_hmac: Option<[u8; 32]>,
    pub policy_version: u32,
    pub authorization_generation: u32,
    pub recreate_generation: u32,
}

/// `operation_protocol.durability_order` step 2: "compute `operation_key_hmac`
/// without retaining readable intent" — this function returns only the
/// 32-byte digest; nothing here stores or returns the intent's raw field
/// values alongside it.
///
/// # Errors
///
/// Returns [`DigestError::LayoutNotYetOwned`] unconditionally as of this
/// story: [`PurposeTag::OperationLedgerOperationKey`] is co-owned by ADR-009
/// and ADR-011 per `contracts/persistence/protected-state-boundary.json`, and
/// ADR-011 has not closed its half yet (see this module's "Honest gap" doc
/// section). Also returns [`DigestError::ComponentTooLarge`] if a component
/// is pathologically oversized, once the layout is eventually closed.
pub fn operation_key_hmac(
    digest_key: &ContentDigestKey,
    account: AccountBindingAad,
    schema_version: SchemaVersion,
    intent: &OperationIntent,
) -> Result<[u8; 32], DigestError> {
    let empty: &[u8] = &[];
    let adapter_byte = [intent.adapter_code.code()];
    let kind_byte = [intent.operation_kind.code()];
    let mask_bytes: Vec<u8> = intent
        .canonical_field_mask
        .iter()
        .map(|m| m.code())
        .collect();
    let source_generation = intent.source_generation.to_be_bytes();
    let expected_loop_version = intent.expected_loop_version.to_be_bytes();
    let policy_version = intent.policy_version.to_be_bytes();
    let authorization_generation = intent.authorization_generation.to_be_bytes();
    let recreate_generation = intent.recreate_generation.to_be_bytes();

    let components: [&[u8]; 18] = [
        b"operation-key-v1",
        &intent.account_ref.0,
        &adapter_byte,
        &intent.collection_ref.0,
        &intent.loop_id.0,
        intent.source_ref_id.as_ref().map_or(empty, |id| &id.0),
        &source_generation,
        &kind_byte,
        &mask_bytes,
        &expected_loop_version,
        intent
            .expected_remote_version_hmac
            .as_ref()
            .map_or(empty, |d| d.as_slice()),
        intent
            .transient_title_digest_hmac
            .as_ref()
            .map_or(empty, |d| d.as_slice()),
        intent
            .transient_body_digest_hmac
            .as_ref()
            .map_or(empty, |d| d.as_slice()),
        intent
            .transient_due_digest_hmac
            .as_ref()
            .map_or(empty, |d| d.as_slice()),
        intent
            .transient_reminder_time_digest_hmac
            .as_ref()
            .map_or(empty, |d| d.as_slice()),
        &policy_version,
        &authorization_generation,
        &recreate_generation,
    ];
    openloops_persistence::digest::compute(
        digest_key,
        PurposeTag::OperationLedgerOperationKey,
        account,
        schema_version,
        &components,
    )
}

/// One append-only `operation_ledger` row's decrypted content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerRecord {
    pub operation_id: [u8; 16],
    pub operation_key_hmac: [u8; 32],
    pub state: OperationState,
    pub attempt_count: u8,
    pub retry_at: Option<UnixSeconds>,
    pub ambiguity_or_failure: AmbiguityOrFailureCode,
    pub destination_locator: Vec<u8>,
    pub generation: u32,
}

impl LedgerRecord {
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.operation_id);
        buf.extend_from_slice(&self.operation_key_hmac);
        push_u8(&mut buf, self.state.code());
        push_u8(&mut buf, self.attempt_count);
        push_option_i64(&mut buf, self.retry_at.map(|t| t.0));
        push_u8(&mut buf, self.ambiguity_or_failure.code());
        push_bytes(&mut buf, &self.destination_locator).expect("locator fits in u32");
        push_u32(&mut buf, self.generation);
        buf
    }

    fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut reader = Reader::new(bytes);
        let operation_id = reader.read_array16()?;
        let operation_key_hmac = reader.read_array32()?;
        let state = OperationState::from_code(reader.read_u8()?)?;
        let attempt_count = reader.read_u8()?;
        let retry_at = reader.read_option_i64()?.map(UnixSeconds);
        let ambiguity_or_failure = AmbiguityOrFailureCode::from_code(reader.read_u8()?)?;
        let destination_locator = reader.read_bytes()?.to_vec();
        let generation = reader.read_u32()?;
        reader.finish()?;
        Ok(Self {
            operation_id,
            operation_key_hmac,
            state,
            attempt_count,
            retry_at,
            ambiguity_or_failure,
            destination_locator,
            generation,
        })
    }
}

/// Durable state and keys this module's ledger row plumbing (append/scan)
/// composes. Deliberately does **not** carry a [`ContentDigestKey`]: nothing
/// in this struct's consumers derives an operation key from an intent (see
/// this module's "Honest gap" doc section) — [`operation_key_hmac`] takes its
/// own digest key explicitly, as a step the caller runs before ever
/// constructing a [`LedgerContext`].
pub struct LedgerContext<'a> {
    pub store: &'a mut Store,
    pub account: AccountBindingAad,
    pub envelope_key: &'a EnvelopeKey,
    pub key_usage: &'a mut KeyUsageState,
    pub schema_version: SchemaVersion,
}

/// A rejected ledger call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LedgerError {
    /// Same `operation_id`, but the freshly computed operation key does not
    /// match the previously committed one: `operation_protocol.same_key`'s
    /// "same key with different intent is an `operation_key_collision`."
    Collision {
        previous_key: [u8; 32],
        attempted_key: [u8; 32],
    },
    /// The maximum three attempts are already recorded; no further eligible
    /// retry may be committed.
    AttemptsExhausted,
    /// [`commit_eligible_retry`] was called against a record that is not
    /// (or is no longer) `known_no_send_retryable` — the caller misused the
    /// distinct retry entry point instead of [`begin_or_replay`].
    NotEligibleForRetry,
    /// Not constructed anywhere in this module: [`operation_key_hmac`] is
    /// called separately from (and before) every [`LedgerContext`]-taking
    /// function here, so this crate never has both a [`DigestError`] and a
    /// [`LedgerError`] to unify internally. Kept as a convenience variant for
    /// a future caller that wants one error type spanning "derive the key"
    /// and "run the ledger state machine" — e.g. `operation_key_hmac(..)`
    /// `.map_err(LedgerError::Digest)` `.and_then(|key| begin_or_replay(ctx,
    /// id, key))`.
    Digest(DigestError),
    Seal(SealError),
    Scan(ScanError),
}

/// The result of [`begin_or_replay`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BeginOutcome {
    /// No prior entry existed; a fresh `pending` row is now durably
    /// committed. The caller may proceed to `mark_attempt_started`.
    FreshPending(LedgerRecord),
    /// A duplicate external invocation of the same key and intent: the prior
    /// recorded state is returned and **zero** new rows were written. This
    /// holds unconditionally for *any* existing state — including
    /// `known_no_send_retryable` with an eligible retry still standing — per
    /// ADR-009's "replay never issues a request even while a retry remains
    /// eligible." A caller that wants to actually commit that eligible retry
    /// must call [`commit_eligible_retry`] itself; `begin_or_replay` never
    /// does so on the caller's behalf.
    Replayed(LedgerRecord),
}

fn latest_for_operation(
    ctx: &mut LedgerContext<'_>,
    operation_id: [u8; 16],
) -> Result<Option<LedgerRecord>, LedgerError> {
    let rows = scan_records(
        ctx.store,
        ctx.account,
        ctx.envelope_key,
        RecordType::OperationLedger,
        LedgerRecord::decode,
    )
    .map_err(LedgerError::Scan)?;
    Ok(rows
        .into_iter()
        .filter(|row| row.operation_id == operation_id)
        .max_by_key(|row| row.generation))
}

fn append(ctx: &mut LedgerContext<'_>, record: &LedgerRecord) -> Result<(), LedgerError> {
    let envelopes = seal_batch(
        ctx.envelope_key,
        ctx.key_usage,
        ctx.account,
        ctx.schema_version,
        vec![(RecordType::OperationLedger, record.encode())],
    )
    .map_err(LedgerError::Seal)?;
    commit_batch(ctx.store, ctx.account, &envelopes).map_err(LedgerError::Seal)
}

/// `operation_protocol.durability_order` steps 1-3 plus the repaired
/// `same_key` semantics: validates against the latest recorded state for
/// `operation_id` and either commits a fresh `pending` row, replays the
/// prior state with **zero** writes, or reports a typed collision.
///
/// This function alone is the duplicate-external-invocation path and it
/// never commits an eligible retry's next attempt, even when one stands
/// (see this module's doc comment). Committing that retry is a distinct,
/// separately invoked operation: [`commit_eligible_retry`].
///
/// Takes an already-computed `operation_key` rather than an
/// [`OperationIntent`] (see this module's "Honest gap" doc section): the
/// caller must derive it via [`operation_key_hmac`] first and propagate that
/// call's [`DigestError`] — this function has no way to compute one itself.
///
/// # Errors
///
/// Returns [`LedgerError::Collision`] on same-key-different-intent, or a
/// seal/scan failure.
pub fn begin_or_replay(
    ctx: &mut LedgerContext<'_>,
    operation_id: [u8; 16],
    operation_key: [u8; 32],
) -> Result<BeginOutcome, LedgerError> {
    let key = operation_key;

    match latest_for_operation(ctx, operation_id)? {
        None => {
            let record = LedgerRecord {
                operation_id,
                operation_key_hmac: key,
                state: OperationState::Pending,
                attempt_count: 0,
                retry_at: None,
                ambiguity_or_failure: AmbiguityOrFailureCode::None,
                destination_locator: Vec::new(),
                generation: 0,
            };
            append(ctx, &record)?;
            Ok(BeginOutcome::FreshPending(record))
        }
        Some(prev) if prev.operation_key_hmac != key => Err(LedgerError::Collision {
            previous_key: prev.operation_key_hmac,
            attempted_key: key,
        }),
        // Unconditional replay: no branch on `prev.state`/`attempt_count`
        // here at all. Whatever the latest recorded state is — including an
        // eligible `known_no_send_retryable` — a duplicate invocation only
        // ever replays it. This is what makes replay genuinely zero-request
        // "even while a retry remains eligible."
        Some(prev) => Ok(BeginOutcome::Replayed(prev)),
    }
}

/// `operation_protocol`'s **distinct** eligible-retry path (ADR-009: "an
/// eligible retry never reuses the replay path to skip its own durability
/// step"). The caller invokes this only when it has itself decided — as a
/// deliberate scheduled-retry action, never merely because
/// [`begin_or_replay`] was called again — to retry a `known_no_send`-proven
/// operation. `current` must be the latest record the caller already holds
/// (e.g. returned from an earlier [`begin_or_replay`]/[`classify_outcome`]
/// call); this function does not itself re-scan for it, so a caller cannot
/// reach this transition through `begin_or_replay`'s duplicate-invocation
/// surface at all.
///
/// # Errors
///
/// Returns [`LedgerError::NotEligibleForRetry`] if `current.state` is not
/// [`OperationState::KnownNoSendRetryable`], [`LedgerError::AttemptsExhausted`]
/// if `current.attempt_count >= 3`, or a seal/store failure.
pub fn commit_eligible_retry(
    ctx: &mut LedgerContext<'_>,
    current: LedgerRecord,
) -> Result<LedgerRecord, LedgerError> {
    if current.state != OperationState::KnownNoSendRetryable {
        return Err(LedgerError::NotEligibleForRetry);
    }
    if current.attempt_count >= 3 {
        return Err(LedgerError::AttemptsExhausted);
    }
    let next = LedgerRecord {
        state: OperationState::AttemptStarted,
        attempt_count: current.attempt_count + 1,
        retry_at: None,
        ambiguity_or_failure: AmbiguityOrFailureCode::None,
        generation: current.generation + 1,
        ..current
    };
    append(ctx, &next)?;
    Ok(next)
}

/// `durability_order` step 5: "mark `attempt_started` atomically" for a fresh
/// `pending` row, immediately before the caller's one bounded adapter
/// request.
///
/// # Errors
///
/// Returns a seal/store failure.
pub fn mark_attempt_started(
    ctx: &mut LedgerContext<'_>,
    pending: LedgerRecord,
) -> Result<LedgerRecord, LedgerError> {
    let next = LedgerRecord {
        state: OperationState::AttemptStarted,
        attempt_count: pending.attempt_count + 1,
        generation: pending.generation + 1,
        ..pending
    };
    append(ctx, &next)?;
    Ok(next)
}

/// `durability_order` step 7: "classify response before any retry or cursor
/// advancement," applied to the one adapter response for `in_flight`
/// (previously moved to `attempt_started`).
///
/// `failure_hint` supplies the exact [`AmbiguityOrFailureCode`] for
/// [`AdapterOutcome::Ambiguous`]/[`AdapterOutcome::DefinitiveFailure`]; only
/// the adapter (a future story) knows which of the 17 catalog reasons
/// applies, so this module does not guess one.
///
/// # Errors
///
/// Returns a seal/store failure.
pub fn classify_outcome(
    ctx: &mut LedgerContext<'_>,
    in_flight: LedgerRecord,
    outcome: &AdapterOutcome,
    failure_hint: AmbiguityOrFailureCode,
) -> Result<LedgerRecord, LedgerError> {
    let next = match outcome {
        AdapterOutcome::Succeeded {
            destination_locator,
        } => LedgerRecord {
            state: OperationState::Succeeded,
            ambiguity_or_failure: AmbiguityOrFailureCode::None,
            destination_locator: destination_locator.clone(),
            generation: in_flight.generation + 1,
            ..in_flight
        },
        AdapterOutcome::KnownNoSend if in_flight.attempt_count < 3 => LedgerRecord {
            state: OperationState::KnownNoSendRetryable,
            ambiguity_or_failure: AmbiguityOrFailureCode::TransportKnownNoSend,
            generation: in_flight.generation + 1,
            ..in_flight
        },
        AdapterOutcome::KnownNoSend => LedgerRecord {
            state: OperationState::DefinitiveFailed,
            ambiguity_or_failure: AmbiguityOrFailureCode::TransportKnownNoSend,
            generation: in_flight.generation + 1,
            ..in_flight
        },
        AdapterOutcome::Ambiguous => LedgerRecord {
            state: OperationState::Ambiguous,
            ambiguity_or_failure: failure_hint,
            generation: in_flight.generation + 1,
            ..in_flight
        },
        AdapterOutcome::DefinitiveFailure => LedgerRecord {
            state: OperationState::DefinitiveFailed,
            ambiguity_or_failure: failure_hint,
            generation: in_flight.generation + 1,
            ..in_flight
        },
    };
    append(ctx, &next)?;
    Ok(next)
}

/// `restart_rule`: "detected-loop intent must be reconstructed from current
/// evidence and policy then match `operation_key_hmac`; mismatch or
/// unavailable evidence makes zero request." Pure comparison; the caller is
/// responsible for actually making zero requests when this returns `false`.
#[must_use]
pub fn restart_reconstruction_matches(previous: &LedgerRecord, recomputed_key: [u8; 32]) -> bool {
    previous.operation_key_hmac == recomputed_key
}

#[cfg(test)]
mod tests {
    use super::{
        AdapterCode, AmbiguityOrFailureCode, BeginOutcome, FieldMaskCode, LedgerContext,
        LedgerError, LedgerRecord, OperationIntent, OperationState, begin_or_replay,
        classify_outcome, commit_eligible_retry, mark_attempt_started, operation_key_hmac,
        restart_reconstruction_matches,
    };
    use crate::ports::{AdapterOutcome, OperationKind};
    use openloops_domain::ids::OpaqueId;
    use openloops_persistence::digest::{ContentDigestKey, DigestError};
    use openloops_persistence::envelope::EnvelopeKey;
    use openloops_persistence::ids::{RandomId, SchemaVersion};
    use openloops_persistence::reservation::KeyUsageState;
    use openloops_persistence::store::Store;

    fn schema() -> SchemaVersion {
        SchemaVersion::new(core::num::NonZeroU32::new(1).unwrap())
    }

    fn intent(loop_byte: u8) -> OperationIntent {
        OperationIntent {
            account_ref: OpaqueId::from_bytes([1u8; 16]),
            adapter_code: AdapterCode::MicrosoftTodo,
            collection_ref: OpaqueId::from_bytes([2u8; 16]),
            loop_id: OpaqueId::from_bytes([loop_byte; 16]),
            source_ref_id: None,
            source_generation: 1,
            operation_kind: OperationKind::TodoCreate,
            canonical_field_mask: vec![FieldMaskCode::Title, FieldMaskCode::Due],
            expected_loop_version: 1,
            expected_remote_version_hmac: None,
            transient_title_digest_hmac: Some([9u8; 32]),
            transient_body_digest_hmac: None,
            transient_due_digest_hmac: Some([8u8; 32]),
            transient_reminder_time_digest_hmac: None,
            policy_version: 1,
            authorization_generation: 1,
            recreate_generation: 0,
        }
    }

    fn ctx_pieces() -> (Store, EnvelopeKey, KeyUsageState) {
        let key = EnvelopeKey::new(RandomId::from_random_bytes([5u8; 16]).unwrap(), [3u8; 32]);
        let usage = KeyUsageState::fresh(key.id());
        (Store::open_in_memory().unwrap(), key, usage)
    }

    fn account() -> openloops_persistence::ids::AccountBindingAad {
        RandomId::from_random_bytes([6u8; 16]).unwrap()
    }

    /// A stand-in for an already-computed `operation_key_hmac` output. This
    /// module's state machine (`begin_or_replay` and friends) only ever
    /// compares two such 32-byte keys for equality — see this module's
    /// "Honest gap" doc section — so its tests exercise that machine with
    /// opaque synthetic keys distinguished only by `tag`, standing in for
    /// "whatever `operation_key_hmac` would have produced for this intent"
    /// without actually calling it (it unconditionally refuses today; see
    /// [`operation_key_hmac_fails_closed_pending_the_second_owner`]).
    fn synthetic_key(tag: u8) -> [u8; 32] {
        [tag; 32]
    }

    #[test]
    fn operation_key_hmac_fails_closed_pending_the_second_owner() {
        // `contracts/persistence/protected-state-boundary.json`
        // `digest_suite.purpose_catalog`'s `operation_ledger.operation_key_hmac`
        // row names owner "ADR-009 and ADR-011"; ADR-011 has not closed its
        // half. `operation_key_hmac` must refuse every intent — including a
        // fully-formed, realistic one, not just an empty edge case — until
        // both named owners close the layout.
        let digest_key = ContentDigestKey::new([1u8; 32]);
        assert_eq!(
            operation_key_hmac(&digest_key, account(), schema(), &intent(1)),
            Err(DigestError::LayoutNotYetOwned)
        );
        let mut different = intent(1);
        different.policy_version = 99;
        assert_eq!(
            operation_key_hmac(&digest_key, account(), schema(), &different),
            Err(DigestError::LayoutNotYetOwned)
        );
    }

    #[test]
    fn duplicate_invocation_replays_with_zero_new_writes() {
        let (mut store, key, mut usage) = ctx_pieces();
        let acct = account();
        let mut ctx = LedgerContext {
            store: &mut store,
            account: acct,
            envelope_key: &key,
            key_usage: &mut usage,
            schema_version: schema(),
        };
        let first = begin_or_replay(&mut ctx, [1u8; 16], synthetic_key(1)).unwrap();
        assert!(matches!(first, BeginOutcome::FreshPending(_)));
        let rows_after_first = ctx.store.read_all(acct).unwrap().len();

        let second = begin_or_replay(&mut ctx, [1u8; 16], synthetic_key(1)).unwrap();
        let BeginOutcome::Replayed(record) = &second else {
            panic!("expected Replayed, got {second:?}");
        };
        assert_eq!(record.state, OperationState::Pending);
        assert_eq!(
            ctx.store.read_all(acct).unwrap().len(),
            rows_after_first,
            "a duplicate invocation with no eligible retry writes nothing new"
        );
    }

    #[test]
    fn same_key_different_intent_is_a_typed_collision() {
        let (mut store, key, mut usage) = ctx_pieces();
        let mut ctx = LedgerContext {
            store: &mut store,
            account: account(),
            envelope_key: &key,
            key_usage: &mut usage,
            schema_version: schema(),
        };
        begin_or_replay(&mut ctx, [2u8; 16], synthetic_key(1)).unwrap();
        // A different intent hashes to a different operation key; same
        // `operation_id`, different key, must be a typed collision.
        let result = begin_or_replay(&mut ctx, [2u8; 16], synthetic_key(2));
        assert!(matches!(result, Err(LedgerError::Collision { .. })));
    }

    #[test]
    fn duplicate_invocation_replays_zero_requests_even_while_a_retry_is_eligible() {
        // ADR-009: "replay never issues a request even while a retry remains
        // eligible." A *duplicate external invocation* — i.e. a second call
        // to `begin_or_replay` for the same operation/intent, not a
        // deliberate scheduled retry — must never substitute for the
        // eligible-retry path, no matter what state stands.
        let (mut store, key, mut usage) = ctx_pieces();
        let acct = account();
        let mut ctx = LedgerContext {
            store: &mut store,
            account: acct,
            envelope_key: &key,
            key_usage: &mut usage,
            schema_version: schema(),
        };
        let BeginOutcome::FreshPending(pending) =
            begin_or_replay(&mut ctx, [3u8; 16], synthetic_key(1)).unwrap()
        else {
            panic!("expected fresh pending");
        };
        let attempt_started = mark_attempt_started(&mut ctx, pending).unwrap();
        assert_eq!(attempt_started.attempt_count, 1);
        let after_no_send = classify_outcome(
            &mut ctx,
            attempt_started,
            &AdapterOutcome::KnownNoSend,
            AmbiguityOrFailureCode::None,
        )
        .unwrap();
        assert_eq!(after_no_send.state, OperationState::KnownNoSendRetryable);
        let rows_before_duplicate = ctx.store.read_all(acct).unwrap().len();

        // A duplicate external invocation of the *same* intent arrives while
        // this eligible retry stands. It must replay with zero new writes,
        // not commit the next attempt.
        let duplicate = begin_or_replay(&mut ctx, [3u8; 16], synthetic_key(1)).unwrap();
        let BeginOutcome::Replayed(record) = &duplicate else {
            panic!("expected Replayed, got {duplicate:?}");
        };
        assert_eq!(record.state, OperationState::KnownNoSendRetryable);
        assert_eq!(record.attempt_count, 1);
        assert_eq!(
            ctx.store.read_all(acct).unwrap().len(),
            rows_before_duplicate,
            "a duplicate invocation must issue zero requests even while a retry is eligible"
        );
    }

    #[test]
    fn commit_eligible_retry_is_a_distinct_path_from_replay() {
        // The *separate*, deliberately-invoked retry path: the caller holds
        // the `known_no_send_retryable` record itself (never obtained by
        // treating a duplicate invocation as a retry) and explicitly commits
        // the next attempt before its own bounded request.
        let (mut store, key, mut usage) = ctx_pieces();
        let mut ctx = LedgerContext {
            store: &mut store,
            account: account(),
            envelope_key: &key,
            key_usage: &mut usage,
            schema_version: schema(),
        };
        let BeginOutcome::FreshPending(pending) =
            begin_or_replay(&mut ctx, [3u8; 16], synthetic_key(1)).unwrap()
        else {
            panic!("expected fresh pending");
        };
        let attempt_started = mark_attempt_started(&mut ctx, pending).unwrap();
        let after_no_send = classify_outcome(
            &mut ctx,
            attempt_started,
            &AdapterOutcome::KnownNoSend,
            AmbiguityOrFailureCode::None,
        )
        .unwrap();

        let retried = commit_eligible_retry(&mut ctx, after_no_send).unwrap();
        assert_eq!(retried.state, OperationState::AttemptStarted);
        assert_eq!(retried.attempt_count, 2);
    }

    #[test]
    fn commit_eligible_retry_rejects_a_record_that_is_not_known_no_send_retryable() {
        let (mut store, key, mut usage) = ctx_pieces();
        let mut ctx = LedgerContext {
            store: &mut store,
            account: account(),
            envelope_key: &key,
            key_usage: &mut usage,
            schema_version: schema(),
        };
        let BeginOutcome::FreshPending(pending) =
            begin_or_replay(&mut ctx, [3u8; 16], synthetic_key(1)).unwrap()
        else {
            panic!("expected fresh pending");
        };
        assert_eq!(
            commit_eligible_retry(&mut ctx, pending),
            Err(LedgerError::NotEligibleForRetry)
        );
    }

    #[test]
    fn fourth_attempt_is_never_committed() {
        let (mut store, key, mut usage) = ctx_pieces();
        let mut ctx = LedgerContext {
            store: &mut store,
            account: account(),
            envelope_key: &key,
            key_usage: &mut usage,
            schema_version: schema(),
        };
        let BeginOutcome::FreshPending(pending) =
            begin_or_replay(&mut ctx, [4u8; 16], synthetic_key(1)).unwrap()
        else {
            panic!("expected fresh pending");
        };
        let mut current = mark_attempt_started(&mut ctx, pending).unwrap();
        for _ in 0..3 {
            current = classify_outcome(
                &mut ctx,
                current,
                &AdapterOutcome::KnownNoSend,
                AmbiguityOrFailureCode::None,
            )
            .unwrap();
            if current.state == OperationState::KnownNoSendRetryable && current.attempt_count < 3 {
                // The deliberate retry path, invoked directly by the caller
                // holding the current record — never through
                // `begin_or_replay`, which would only replay it.
                current = commit_eligible_retry(&mut ctx, current).unwrap();
            }
        }
        // `current` now has attempt_count == 3 and state definitive_failed
        // (the third `known_no_send` exhausts retries per this module's
        // `classify_outcome`); a further attempt must be rejected by
        // `commit_eligible_retry` itself, and a duplicate invocation of
        // `begin_or_replay` must remain a plain, zero-write replay.
        assert_eq!(current.attempt_count, 3);
        assert_eq!(current.state, OperationState::DefinitiveFailed);
        assert_eq!(
            commit_eligible_retry(&mut ctx, current.clone()),
            Err(LedgerError::NotEligibleForRetry),
            "definitive_failed is not known_no_send_retryable at all"
        );
        let after = begin_or_replay(&mut ctx, [4u8; 16], synthetic_key(1)).unwrap();
        let BeginOutcome::Replayed(record) = &after else {
            panic!("a 4th attempt must never be committed, got {after:?}");
        };
        assert_eq!(record.attempt_count, 3);
    }

    #[test]
    fn commit_eligible_retry_rejects_the_fourth_attempt_when_state_is_still_retryable() {
        // A record that is `known_no_send_retryable` but has already reached
        // `attempt_count == 3` must be rejected as exhausted, distinct from
        // the wrong-state rejection above.
        let (mut store, key, mut usage) = ctx_pieces();
        let mut ctx = LedgerContext {
            store: &mut store,
            account: account(),
            envelope_key: &key,
            key_usage: &mut usage,
            schema_version: schema(),
        };
        let exhausted = LedgerRecord {
            operation_id: [9u8; 16],
            operation_key_hmac: [0u8; 32],
            state: OperationState::KnownNoSendRetryable,
            attempt_count: 3,
            retry_at: None,
            ambiguity_or_failure: AmbiguityOrFailureCode::None,
            destination_locator: Vec::new(),
            generation: 5,
        };
        assert_eq!(
            commit_eligible_retry(&mut ctx, exhausted),
            Err(LedgerError::AttemptsExhausted)
        );
    }

    #[test]
    fn ambiguous_and_pending_states_block_the_dependent_cursor() {
        assert!(OperationState::Pending.blocks_dependent_cursor());
        assert!(OperationState::AttemptStarted.blocks_dependent_cursor());
        assert!(OperationState::Ambiguous.blocks_dependent_cursor());
        assert!(OperationState::Reconciling.blocks_dependent_cursor());
        assert!(OperationState::UserAssisted.blocks_dependent_cursor());
        assert!(!OperationState::Succeeded.blocks_dependent_cursor());
        assert!(!OperationState::DefinitiveFailed.blocks_dependent_cursor());
        assert!(!OperationState::Abandoned.blocks_dependent_cursor());
        assert!(!OperationState::KnownNoSendRetryable.blocks_dependent_cursor());
    }

    #[test]
    fn restart_mismatch_never_matches() {
        // `restart_reconstruction_matches` is a pure comparison over two
        // already-computed keys (see this module's "Honest gap" doc
        // section); it is exercised here with synthetic keys standing in for
        // "recomputed from current evidence," matching or drifting, exactly
        // as `restart_rule` describes, without depending on
        // `operation_key_hmac` (which refuses unconditionally today).
        let (mut store, key, mut usage) = ctx_pieces();
        let mut ctx = LedgerContext {
            store: &mut store,
            account: account(),
            envelope_key: &key,
            key_usage: &mut usage,
            schema_version: schema(),
        };
        let BeginOutcome::FreshPending(pending) =
            begin_or_replay(&mut ctx, [5u8; 16], synthetic_key(1)).unwrap()
        else {
            panic!("expected fresh pending");
        };
        assert!(restart_reconstruction_matches(&pending, synthetic_key(1)));
        // Current evidence now disagrees (e.g. `expected_loop_version`
        // drifted), so the reconstructed key differs.
        assert!(!restart_reconstruction_matches(&pending, synthetic_key(2)));
    }
}
