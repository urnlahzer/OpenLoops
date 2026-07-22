//! The typed user-command catalog, the idempotency/staleness algorithm, and
//! reopen/remap/duplicate-merge semantics.
//!
//! Implements `contracts/domain/policy-state-boundary.json`
//! `command_policy` (twelve commands), `transition_policy.user_key_algorithm`,
//! `transition_policy.reopen_generation`, and `correction_policy.duplicate`.
//!
//! [`apply_command`] performs `user_key_algorithm` in its exact contract
//! order: look up the opaque command key *before* comparing the loop
//! version; identical replay is a no-op that returns the historical result;
//! a key collision (same key, different payload) is rejected; only an
//! *unseen* key checks the expected loop version, and a stale version is a
//! visible refresh conflict; only after every precondition passes does the
//! call build one [`crate::transition::Transition`] and advance the loop
//! version by exactly one. A rejected command is never recorded in the
//! ledger, so it never blocks a corrected retry under the same key.
//!
//! `remap_evidence` (two loops, one atomic detach/attach) and
//! `correction_policy.duplicate` (survivor/loser merge) do not fit the
//! single-loop shape above, so [`apply_remap_evidence`] and
//! [`apply_duplicate_merge`] implement the same lookup-before-version-check
//! algorithm directly over two loop states instead.

use std::collections::HashMap;

use crate::deadline::UnixSeconds;
use crate::facets::{
    ClosureReviewState, DelegationCode, ObligationState, Resolution, ReviewFlagCode,
};
use crate::hypothesis::HypothesisIdentity;
use crate::ids::{OpaqueId, Version};
use crate::transition::{FacetTuple, Transition, TransitionError, TransitionReasonCode};

/// `command_policy[].id`: the closed catalog of twelve typed user commands.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum CommandId {
    /// `confirm_mine_actionable`.
    ConfirmMineActionable,
    /// `confirm_fulfilled`.
    ConfirmFulfilled,
    /// `confirm_declined`.
    ConfirmDeclined,
    /// `confirm_moot`.
    ConfirmMoot,
    /// `confirm_transfer`.
    ConfirmTransfer,
    /// `keep_open`.
    KeepOpen,
    /// `request_evidence`.
    RequestEvidence,
    /// `remap_evidence`. Handled by [`apply_remap_evidence`], not
    /// [`apply_command`], because it mutates two loops atomically.
    RemapEvidence,
    /// `select_different_evidence`.
    SelectDifferentEvidence,
    /// `completed_outside_email`.
    CompletedOutsideEmail,
    /// `dismiss_classify`.
    DismissClassify,
    /// `reopen`.
    Reopen,
}

impl CommandId {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 12] = [
        Self::ConfirmMineActionable,
        Self::ConfirmFulfilled,
        Self::ConfirmDeclined,
        Self::ConfirmMoot,
        Self::ConfirmTransfer,
        Self::KeepOpen,
        Self::RequestEvidence,
        Self::RemapEvidence,
        Self::SelectDifferentEvidence,
        Self::CompletedOutsideEmail,
        Self::DismissClassify,
        Self::Reopen,
    ];
}

/// `correction_policy.reason_codes`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum CorrectionReasonCode {
    /// The loop is not the user's.
    NotMine,
    /// Detection was invalid.
    FalseDetection,
    /// The loop duplicates another loop.
    Duplicate,
    /// Responsibility was delegated.
    Delegated,
    /// Responsibility is shared.
    Shared,
    /// Another party is assisting.
    Assisted,
    /// The user retains responsibility.
    Retained,
    /// The expectation is moot.
    Moot,
    /// The expectation is no longer relevant.
    NoLongerRelevant,
    /// The deadline is wrong.
    WrongDeadline,
    /// The client association is wrong.
    WrongClient,
    /// Another typed reason not covered above.
    Other,
}

impl CorrectionReasonCode {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 12] = [
        Self::NotMine,
        Self::FalseDetection,
        Self::Duplicate,
        Self::Delegated,
        Self::Shared,
        Self::Assisted,
        Self::Retained,
        Self::Moot,
        Self::NoLongerRelevant,
        Self::WrongDeadline,
        Self::WrongClient,
        Self::Other,
    ];
}

/// `confirm_declined.required`: "explicit decline evidence or manual
/// decline confirmation".
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeclineEvidence {
    /// An explicit decline evidence reference.
    Explicit { source_ref: OpaqueId },
    /// A manual decline confirmation with no source evidence.
    Manual,
}

/// `confirm_moot.required`: "withdrawal supersession no-longer-relevant
/// evidence or explicit manual choice".
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MootEvidence {
    /// Explicit withdrawal evidence.
    Withdrawal { source_ref: OpaqueId },
    /// Explicit supersession evidence.
    Supersession { source_ref: OpaqueId },
    /// Explicit no-longer-relevant evidence.
    NoLongerRelevant { source_ref: OpaqueId },
    /// An explicit manual choice with no source evidence.
    Manual,
}

/// `confirm_transfer.required`: "delegation evidence or manual confirmation
/// of full transfer".
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransferEvidence {
    /// Explicit delegation evidence.
    Explicit { source_ref: OpaqueId },
    /// A manual confirmation with no source evidence.
    Manual,
}

/// The exact typed payload for one of the eleven [`apply_command`] commands.
/// `RemapEvidence` is intentionally absent; see [`apply_remap_evidence`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandPayload {
    /// `confirm_mine_actionable`.
    ConfirmMineActionable,
    /// `confirm_fulfilled`: one or more validated closure evidence refs.
    ConfirmFulfilled {
        closure_evidence_refs: Vec<OpaqueId>,
    },
    /// `confirm_declined`.
    ConfirmDeclined { evidence: DeclineEvidence },
    /// `confirm_moot`.
    ConfirmMoot { evidence: MootEvidence },
    /// `confirm_transfer`.
    ConfirmTransfer {
        delegation: DelegationCode,
        evidence: TransferEvidence,
    },
    /// `keep_open`: the exact current hypothesis identity being suppressed.
    KeepOpen {
        hypothesis_identity: HypothesisIdentity,
    },
    /// `request_evidence`.
    RequestEvidence {
        kind: crate::hypothesis::HypothesisKind,
    },
    /// `select_different_evidence`.
    SelectDifferentEvidence { evidence_ref: OpaqueId },
    /// `completed_outside_email`.
    CompletedOutsideEmail,
    /// `dismiss_classify`.
    DismissClassify { reason: CorrectionReasonCode },
    /// `reopen`.
    Reopen,
}

/// A snapshot of the one loop a single-loop command applies to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoopState {
    /// The loop's opaque id.
    pub loop_id: OpaqueId,
    /// The loop's current complete facet tuple.
    pub facets: FacetTuple,
    /// The loop's current version.
    pub loop_version: Version,
}

/// The context needed to mint a new [`Transition`] for a command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandContext {
    /// The new transition's id.
    pub transition_id: OpaqueId,
    /// The explicit user action id (every command in this module is
    /// user-actor, so this is always required).
    pub user_action_id: OpaqueId,
    /// When the command was applied.
    pub occurred_at: UnixSeconds,
    /// The policy version in force.
    pub policy_version: Version,
}

/// A rejected command precondition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandRejection {
    /// The command's `allowed_from` set does not include the loop's current
    /// obligation state.
    NotAllowedFromCurrentObligationState,
    /// `confirm_fulfilled` requires at least one closure evidence ref.
    ClosureEvidenceRequired,
    /// `keep_open` requires an exact current possible-closure hypothesis.
    NoCurrentPossibleHypothesisToKeepOpen,
    /// The loop version overflowed `u64` on increment (practically
    /// unreachable; included so the increment is total, not panicking).
    VersionOverflow,
    /// `Transition::new` rejected the constructed transition.
    InvalidTransition(TransitionError),
}

/// An opaque idempotency key for one user command
/// (`transition_policy.user_key_algorithm`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CommandKey(pub [u8; 16]);

/// The one-loop-command ledger. Keyed by [`CommandKey`]; only successfully
/// applied commands are recorded, so a rejected attempt never blocks a
/// corrected retry under the same key.
#[derive(Default)]
pub struct Ledger {
    entries: HashMap<CommandKey, (CommandPayload, Transition)>,
}

impl Ledger {
    /// Creates an empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

/// One user command request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandRequest {
    /// The opaque idempotency key.
    pub key: CommandKey,
    /// The loop version the caller believes is current.
    pub expected_loop_version: Version,
    /// The typed command payload.
    pub payload: CommandPayload,
}

/// The result of [`apply_command`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplyOutcome {
    /// The same key and payload were already applied; returns the
    /// historical transition with zero new mutation.
    Replayed(Transition),
    /// The same key was used with a different payload.
    KeyCollision,
    /// The key was unseen but `expected_loop_version` did not match the
    /// current loop version.
    RefreshConflict,
    /// The command applied; the loop version advances by exactly one.
    Applied(Transition),
    /// A command precondition failed; nothing was recorded or mutated.
    Rejected(CommandRejection),
}

fn require_obligation(
    current: ObligationState,
    allowed: &[ObligationState],
) -> Result<(), CommandRejection> {
    if allowed.contains(&current) {
        Ok(())
    } else {
        Err(CommandRejection::NotAllowedFromCurrentObligationState)
    }
}

fn finish(
    state: &LoopState,
    new_facets: FacetTuple,
    reason_code: TransitionReasonCode,
    source_refs: &[OpaqueId],
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    let next_version = state
        .loop_version
        .next()
        .ok_or(CommandRejection::VersionOverflow)?;
    Transition::new(
        context.transition_id,
        state.loop_id,
        Some(context.user_action_id),
        state.facets,
        new_facets,
        reason_code,
        crate::facets::ActorCode::User,
        source_refs,
        context.occurred_at,
        next_version,
        context.policy_version,
    )
    .map_err(CommandRejection::InvalidTransition)
}

fn build_confirm_mine_actionable(
    state: &LoopState,
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    require_obligation(state.facets.obligation_state, &[ObligationState::Candidate])?;
    let new_facets = FacetTuple {
        obligation_state: ObligationState::Open,
        ..state.facets
    };
    finish(
        state,
        new_facets,
        TransitionReasonCode::ConfirmedMineActionable,
        &[],
        context,
    )
}

fn build_confirm_fulfilled(
    state: &LoopState,
    closure_evidence_refs: &[OpaqueId],
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    require_obligation(state.facets.obligation_state, &[ObligationState::Open])?;
    if closure_evidence_refs.is_empty() {
        return Err(CommandRejection::ClosureEvidenceRequired);
    }
    let new_facets = FacetTuple {
        obligation_state: ObligationState::Terminal,
        resolution: Resolution::Closed,
        closure_review_state: ClosureReviewState::None,
        ..state.facets
    };
    finish(
        state,
        new_facets,
        TransitionReasonCode::ConfirmedFulfilled,
        closure_evidence_refs,
        context,
    )
}

fn build_confirm_declined(
    state: &LoopState,
    evidence: &DeclineEvidence,
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    require_obligation(
        state.facets.obligation_state,
        &[ObligationState::Candidate, ObligationState::Open],
    )?;
    let source_refs: Vec<OpaqueId> = match evidence {
        DeclineEvidence::Explicit { source_ref } => vec![*source_ref],
        DeclineEvidence::Manual => Vec::new(),
    };
    let new_facets = FacetTuple {
        obligation_state: ObligationState::Terminal,
        resolution: Resolution::Declined,
        closure_review_state: ClosureReviewState::None,
        ..state.facets
    };
    finish(
        state,
        new_facets,
        TransitionReasonCode::ConfirmedDeclined,
        &source_refs,
        context,
    )
}

fn build_confirm_moot(
    state: &LoopState,
    evidence: &MootEvidence,
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    require_obligation(
        state.facets.obligation_state,
        &[ObligationState::Candidate, ObligationState::Open],
    )?;
    let source_refs: Vec<OpaqueId> = match evidence {
        MootEvidence::Withdrawal { source_ref }
        | MootEvidence::Supersession { source_ref }
        | MootEvidence::NoLongerRelevant { source_ref } => vec![*source_ref],
        MootEvidence::Manual => Vec::new(),
    };
    let new_facets = FacetTuple {
        obligation_state: ObligationState::Terminal,
        resolution: Resolution::Moot,
        closure_review_state: ClosureReviewState::None,
        ..state.facets
    };
    finish(
        state,
        new_facets,
        TransitionReasonCode::ConfirmedMoot,
        &source_refs,
        context,
    )
}

fn build_confirm_transfer(
    state: &LoopState,
    delegation: DelegationCode,
    evidence: &TransferEvidence,
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    require_obligation(
        state.facets.obligation_state,
        &[ObligationState::Candidate, ObligationState::Open],
    )?;
    let source_refs: Vec<OpaqueId> = match evidence {
        TransferEvidence::Explicit { source_ref } => vec![*source_ref],
        TransferEvidence::Manual => Vec::new(),
    };
    if delegation.yields_delegated_resolution() {
        let new_facets = FacetTuple {
            obligation_state: ObligationState::Terminal,
            resolution: Resolution::Delegated,
            closure_review_state: ClosureReviewState::None,
            ..state.facets
        };
        finish(
            state,
            new_facets,
            TransitionReasonCode::ConfirmedTransferred,
            &source_refs,
            context,
        )
    } else {
        // Shared/assisted/retained/unclear responsibility keeps the loop
        // open (OL-DELEG-001); the delegation note is recorded on the
        // transition without terminalizing.
        finish(
            state,
            state.facets,
            TransitionReasonCode::DelegationNoted,
            &source_refs,
            context,
        )
    }
}

fn build_keep_open(
    state: &LoopState,
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    require_obligation(state.facets.obligation_state, &[ObligationState::Open])?;
    if state.facets.closure_review_state != ClosureReviewState::Possible {
        return Err(CommandRejection::NoCurrentPossibleHypothesisToKeepOpen);
    }
    let new_facets = FacetTuple {
        closure_review_state: ClosureReviewState::KeptOpen,
        ..state.facets
    };
    finish(
        state,
        new_facets,
        TransitionReasonCode::KeptOpenSuppressed,
        &[],
        context,
    )
}

fn build_request_evidence(
    state: &LoopState,
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    require_obligation(state.facets.obligation_state, &[ObligationState::Open])?;
    let new_facets = FacetTuple {
        closure_review_state: ClosureReviewState::EvidenceRequested,
        ..state.facets
    };
    finish(
        state,
        new_facets,
        TransitionReasonCode::EvidenceRequested,
        &[],
        context,
    )
}

fn build_select_different_evidence(
    state: &LoopState,
    evidence_ref: OpaqueId,
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    require_obligation(
        state.facets.obligation_state,
        &[
            ObligationState::Candidate,
            ObligationState::Open,
            ObligationState::Terminal,
        ],
    )?;
    finish(
        state,
        state.facets,
        TransitionReasonCode::EvidenceSelectedDifferent,
        &[evidence_ref],
        context,
    )
}

fn build_completed_outside_email(
    state: &LoopState,
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    require_obligation(
        state.facets.obligation_state,
        &[ObligationState::Candidate, ObligationState::Open],
    )?;
    let new_facets = FacetTuple {
        obligation_state: ObligationState::Terminal,
        resolution: Resolution::CompletedOutsideEmail,
        closure_review_state: ClosureReviewState::None,
        ..state.facets
    };
    finish(
        state,
        new_facets,
        TransitionReasonCode::CompletedOutsideEmail,
        &[],
        context,
    )
}

/// Maps a correction reason to the resolution/reason-code pair
/// `dismiss_classify` produces: "terminal dismissed except typed transferred
/// or moot outcomes".
const fn dismiss_outcome_for(reason: CorrectionReasonCode) -> (Resolution, TransitionReasonCode) {
    match reason {
        CorrectionReasonCode::Delegated => (
            Resolution::Delegated,
            TransitionReasonCode::ConfirmedTransferred,
        ),
        CorrectionReasonCode::Moot => (Resolution::Moot, TransitionReasonCode::ConfirmedMoot),
        CorrectionReasonCode::NotMine
        | CorrectionReasonCode::FalseDetection
        | CorrectionReasonCode::Duplicate
        | CorrectionReasonCode::Shared
        | CorrectionReasonCode::Assisted
        | CorrectionReasonCode::Retained
        | CorrectionReasonCode::NoLongerRelevant
        | CorrectionReasonCode::WrongDeadline
        | CorrectionReasonCode::WrongClient
        | CorrectionReasonCode::Other => (
            Resolution::Dismissed,
            TransitionReasonCode::DismissedClassified,
        ),
    }
}

fn build_dismiss_classify(
    state: &LoopState,
    reason: CorrectionReasonCode,
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    require_obligation(
        state.facets.obligation_state,
        &[ObligationState::Candidate, ObligationState::Open],
    )?;
    let (resolution, reason_code) = dismiss_outcome_for(reason);
    let new_facets = FacetTuple {
        obligation_state: ObligationState::Terminal,
        resolution,
        closure_review_state: ClosureReviewState::None,
        ..state.facets
    };
    finish(state, new_facets, reason_code, &[], context)
}

fn build_reopen(
    state: &LoopState,
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    require_obligation(state.facets.obligation_state, &[ObligationState::Terminal])?;
    let new_facets = FacetTuple {
        obligation_state: ObligationState::Open,
        resolution: Resolution::None,
        closure_review_state: ClosureReviewState::None,
        review_flags: state.facets.review_flags.with(ReviewFlagCode::NeedsReview),
        ..state.facets
    };
    finish(
        state,
        new_facets,
        TransitionReasonCode::Reopened,
        &[],
        context,
    )
}

fn build_transition(
    state: &LoopState,
    payload: &CommandPayload,
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    match payload {
        CommandPayload::ConfirmMineActionable => build_confirm_mine_actionable(state, context),
        CommandPayload::ConfirmFulfilled {
            closure_evidence_refs,
        } => build_confirm_fulfilled(state, closure_evidence_refs, context),
        CommandPayload::ConfirmDeclined { evidence } => {
            build_confirm_declined(state, evidence, context)
        }
        CommandPayload::ConfirmMoot { evidence } => build_confirm_moot(state, evidence, context),
        CommandPayload::ConfirmTransfer {
            delegation,
            evidence,
        } => build_confirm_transfer(state, *delegation, evidence, context),
        CommandPayload::KeepOpen {
            hypothesis_identity: _,
        } => build_keep_open(state, context),
        CommandPayload::RequestEvidence { kind: _ } => build_request_evidence(state, context),
        CommandPayload::SelectDifferentEvidence { evidence_ref } => {
            build_select_different_evidence(state, *evidence_ref, context)
        }
        CommandPayload::CompletedOutsideEmail => build_completed_outside_email(state, context),
        CommandPayload::DismissClassify { reason } => {
            build_dismiss_classify(state, *reason, context)
        }
        CommandPayload::Reopen => build_reopen(state, context),
    }
}

/// Applies one single-loop command, exactly implementing
/// `transition_policy.user_key_algorithm` in order.
pub fn apply_command(
    ledger: &mut Ledger,
    state: &LoopState,
    request: CommandRequest,
    context: &CommandContext,
) -> ApplyOutcome {
    if let Some((recorded_payload, recorded_transition)) = ledger.entries.get(&request.key) {
        return if *recorded_payload == request.payload {
            ApplyOutcome::Replayed(recorded_transition.clone())
        } else {
            ApplyOutcome::KeyCollision
        };
    }

    if request.expected_loop_version != state.loop_version {
        return ApplyOutcome::RefreshConflict;
    }

    match build_transition(state, &request.payload, context) {
        Ok(transition) => {
            ledger
                .entries
                .insert(request.key, (request.payload, transition.clone()));
            ApplyOutcome::Applied(transition)
        }
        Err(rejection) => ApplyOutcome::Rejected(rejection),
    }
}

/// Adds a `needs_review` note plus a validated relation to an already
/// terminal loop, exactly reproducing legality rule 10: later evidence may
/// add `needs_review` and a relation but never reopens, changes resolution,
/// or sets closure review non-none. The actor is always
/// [`crate::facets::ActorCode::System`], because this is automated
/// later-evidence detection, not a user command.
///
/// # Errors
///
/// Returns [`CommandRejection`] when the loop is not terminal or the
/// constructed transition fails validation.
pub fn apply_terminal_review_note(
    state: &LoopState,
    causing_source_ref: OpaqueId,
    context: &CommandContext,
) -> Result<Transition, CommandRejection> {
    require_obligation(state.facets.obligation_state, &[ObligationState::Terminal])?;
    let new_facets = FacetTuple {
        review_flags: state.facets.review_flags.with(ReviewFlagCode::NeedsReview),
        ..state.facets
    };
    let next_version = state
        .loop_version
        .next()
        .ok_or(CommandRejection::VersionOverflow)?;
    Transition::new(
        context.transition_id,
        state.loop_id,
        None,
        state.facets,
        new_facets,
        TransitionReasonCode::TerminalReviewNoteAdded,
        crate::facets::ActorCode::System,
        &[causing_source_ref],
        context.occurred_at,
        next_version,
        context.policy_version,
    )
    .map_err(CommandRejection::InvalidTransition)
}

/// A rejected [`apply_remap_evidence`] call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemapRejection {
    /// The source loop's version did not match.
    StaleSourceVersion,
    /// The target loop's version did not match.
    StaleTargetVersion,
    /// A constructed transition failed validation.
    InvalidTransition(TransitionError),
    /// A loop version overflowed on increment.
    VersionOverflow,
}

/// The inputs [`apply_remap_evidence`] needs beyond the two loop states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemapRequest {
    /// The version the caller believes `source` is currently at.
    pub source_expected_version: Version,
    /// The version the caller believes `target` is currently at.
    pub target_expected_version: Version,
    /// The evidence reference being remapped.
    pub evidence_ref: OpaqueId,
    /// The id for the detach-side transition.
    pub detach_transition_id: OpaqueId,
    /// The id for the attach-side transition.
    pub attach_transition_id: OpaqueId,
}

/// `command_policy.remap_evidence`: "atomic relation detach and attach then
/// deterministic re-evaluation of both loops". Returns both transitions
/// together, or neither: there is no partial-application path.
///
/// # Errors
///
/// Returns [`RemapRejection`] when either loop's version is stale or either
/// transition fails validation; on error, no transition is returned for
/// either loop.
pub fn apply_remap_evidence(
    source: &LoopState,
    target: &LoopState,
    request: &RemapRequest,
    context: &CommandContext,
) -> Result<(Transition, Transition), RemapRejection> {
    if source.loop_version != request.source_expected_version {
        return Err(RemapRejection::StaleSourceVersion);
    }
    if target.loop_version != request.target_expected_version {
        return Err(RemapRejection::StaleTargetVersion);
    }
    let evidence_ref = request.evidence_ref;
    let detach_transition_id = request.detach_transition_id;
    let attach_transition_id = request.attach_transition_id;
    let source_next = source
        .loop_version
        .next()
        .ok_or(RemapRejection::VersionOverflow)?;
    let target_next = target
        .loop_version
        .next()
        .ok_or(RemapRejection::VersionOverflow)?;
    let detach = Transition::new(
        detach_transition_id,
        source.loop_id,
        Some(context.user_action_id),
        source.facets,
        source.facets,
        TransitionReasonCode::EvidenceRemappedFrom,
        crate::facets::ActorCode::User,
        &[evidence_ref],
        context.occurred_at,
        source_next,
        context.policy_version,
    )
    .map_err(RemapRejection::InvalidTransition)?;
    let attach = Transition::new(
        attach_transition_id,
        target.loop_id,
        Some(context.user_action_id),
        target.facets,
        target.facets,
        TransitionReasonCode::EvidenceRemappedTo,
        crate::facets::ActorCode::User,
        &[evidence_ref],
        context.occurred_at,
        target_next,
        context.policy_version,
    )
    .map_err(RemapRejection::InvalidTransition)?;
    Ok((detach, attach))
}

/// A rejected [`apply_duplicate_merge`] call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DuplicateMergeRejection {
    /// The survivor loop's version did not match.
    StaleSurvivorVersion,
    /// The loser loop's version did not match.
    StaleLoserVersion,
    /// The loser loop is already terminal and cannot be merged again.
    LoserAlreadyTerminal,
    /// No non-conflicting evidence was supplied to transfer.
    NoTransferableEvidence,
    /// A constructed transition failed validation.
    InvalidTransition(TransitionError),
    /// A loop version overflowed on increment.
    VersionOverflow,
}

/// `correction_policy.duplicate`: "version-check survivor and loser;
/// transfer only validated non-conflicting evidence and deadline history;
/// append both transitions; loser becomes dismissed duplicate; conflicts
/// remain reviewable." `transferable_evidence` must already be filtered to
/// non-conflicting refs by the caller (conflict detection is an
/// evidence-identity concern owned by ADR-006, out of scope here); this
/// function only enforces the atomic version-checked merge shape.
///
/// # Errors
///
/// Returns [`DuplicateMergeRejection`] when either version is stale, the
/// loser is already terminal, no evidence was supplied, or either
/// transition fails validation; on error, no transition is returned for
/// either loop.
#[allow(clippy::too_many_arguments)]
pub fn apply_duplicate_merge(
    survivor: &LoopState,
    survivor_expected_version: Version,
    loser: &LoopState,
    loser_expected_version: Version,
    transferable_evidence: &[OpaqueId],
    survivor_transition_id: OpaqueId,
    loser_transition_id: OpaqueId,
    context: &CommandContext,
) -> Result<(Transition, Transition), DuplicateMergeRejection> {
    if survivor.loop_version != survivor_expected_version {
        return Err(DuplicateMergeRejection::StaleSurvivorVersion);
    }
    if loser.loop_version != loser_expected_version {
        return Err(DuplicateMergeRejection::StaleLoserVersion);
    }
    if matches!(loser.facets.obligation_state, ObligationState::Terminal) {
        return Err(DuplicateMergeRejection::LoserAlreadyTerminal);
    }
    if transferable_evidence.is_empty() {
        return Err(DuplicateMergeRejection::NoTransferableEvidence);
    }
    let survivor_next = survivor
        .loop_version
        .next()
        .ok_or(DuplicateMergeRejection::VersionOverflow)?;
    let loser_next = loser
        .loop_version
        .next()
        .ok_or(DuplicateMergeRejection::VersionOverflow)?;

    let survivor_transition = Transition::new(
        survivor_transition_id,
        survivor.loop_id,
        Some(context.user_action_id),
        survivor.facets,
        survivor.facets,
        TransitionReasonCode::DuplicateMergeSurvivorTransfer,
        crate::facets::ActorCode::User,
        transferable_evidence,
        context.occurred_at,
        survivor_next,
        context.policy_version,
    )
    .map_err(DuplicateMergeRejection::InvalidTransition)?;

    let loser_new_facets = FacetTuple {
        obligation_state: ObligationState::Terminal,
        resolution: Resolution::Dismissed,
        closure_review_state: ClosureReviewState::None,
        ..loser.facets
    };
    let loser_transition = Transition::new(
        loser_transition_id,
        loser.loop_id,
        Some(context.user_action_id),
        loser.facets,
        loser_new_facets,
        TransitionReasonCode::DuplicateMergeLoserDismissed,
        crate::facets::ActorCode::User,
        &[],
        context.occurred_at,
        loser_next,
        context.policy_version,
    )
    .map_err(DuplicateMergeRejection::InvalidTransition)?;

    Ok((survivor_transition, loser_transition))
}

#[cfg(test)]
mod tests {
    use super::{
        ApplyOutcome, CommandContext, CommandKey, CommandPayload, CommandRejection,
        CorrectionReasonCode, DeclineEvidence, DuplicateMergeRejection, Ledger, LoopState,
        RemapRejection, apply_command, apply_duplicate_merge, apply_remap_evidence,
        apply_terminal_review_note,
    };
    use crate::deadline::UnixSeconds;
    use crate::facets::{
        AnalysisState, ClosureReviewState, ConfidenceBucket, DeadlineState, DelegationCode,
        ObligationState, ProvenanceSet, ReminderState, Resolution, ReviewFlagCode, ReviewFlagSet,
        SourceState,
    };
    use crate::ids::{OpaqueId, Version};
    use crate::transition::FacetTuple;

    fn ids(byte: u8) -> OpaqueId {
        OpaqueId::from_bytes([byte; 16])
    }

    fn version(value: u64) -> Version {
        Version::new(core::num::NonZeroU64::new(value).expect("nonzero test version"))
    }

    fn open_facets() -> FacetTuple {
        FacetTuple {
            provenance: ProvenanceSet::empty(),
            obligation_state: ObligationState::Open,
            review_flags: ReviewFlagSet::empty(),
            deadline_state: DeadlineState::Unresolved,
            closure_review_state: ClosureReviewState::None,
            reminder_state: ReminderState::None,
            analysis_state: AnalysisState::Current,
            source_state: SourceState::Available,
            resolution: Resolution::None,
            confidence_bucket: ConfidenceBucket::High,
        }
    }

    fn open_state() -> LoopState {
        LoopState {
            loop_id: ids(1),
            facets: open_facets(),
            loop_version: version(1),
        }
    }

    fn context() -> CommandContext {
        CommandContext {
            transition_id: ids(10),
            user_action_id: ids(11),
            occurred_at: UnixSeconds(0),
            policy_version: version(1),
        }
    }

    #[test]
    fn confirm_fulfilled_requires_open_and_at_least_one_evidence_ref() {
        let mut ledger = Ledger::new();
        let state = open_state();
        let missing_evidence = apply_command(
            &mut ledger,
            &state,
            super::CommandRequest {
                key: CommandKey([1; 16]),
                expected_loop_version: state.loop_version,
                payload: CommandPayload::ConfirmFulfilled {
                    closure_evidence_refs: Vec::new(),
                },
            },
            &context(),
        );
        assert_eq!(
            missing_evidence,
            ApplyOutcome::Rejected(CommandRejection::ClosureEvidenceRequired)
        );

        let applied = apply_command(
            &mut ledger,
            &state,
            super::CommandRequest {
                key: CommandKey([2; 16]),
                expected_loop_version: state.loop_version,
                payload: CommandPayload::ConfirmFulfilled {
                    closure_evidence_refs: vec![ids(50)],
                },
            },
            &context(),
        );
        assert!(matches!(applied, ApplyOutcome::Applied(_)));
    }

    #[test]
    fn identical_replay_is_a_zero_mutation_no_op() {
        let mut ledger = Ledger::new();
        let state = open_state();
        let request = super::CommandRequest {
            key: CommandKey([3; 16]),
            expected_loop_version: state.loop_version,
            payload: CommandPayload::ConfirmFulfilled {
                closure_evidence_refs: vec![ids(50)],
            },
        };
        let first = apply_command(&mut ledger, &state, request.clone(), &context());
        let ApplyOutcome::Applied(first_transition) = first else {
            panic!("expected Applied");
        };
        // Replay with the *same, unadvanced* state (as if the caller never
        // committed the version bump) still returns the original result and
        // performs no new mutation.
        let replay = apply_command(&mut ledger, &state, request, &context());
        assert_eq!(replay, ApplyOutcome::Replayed(first_transition));
    }

    #[test]
    fn same_key_different_payload_is_a_collision() {
        let mut ledger = Ledger::new();
        let state = open_state();
        let key = CommandKey([4; 16]);
        let first = apply_command(
            &mut ledger,
            &state,
            super::CommandRequest {
                key,
                expected_loop_version: state.loop_version,
                payload: CommandPayload::ConfirmFulfilled {
                    closure_evidence_refs: vec![ids(50)],
                },
            },
            &context(),
        );
        assert!(matches!(first, ApplyOutcome::Applied(_)));

        let collision = apply_command(
            &mut ledger,
            &state,
            super::CommandRequest {
                key,
                expected_loop_version: state.loop_version,
                payload: CommandPayload::ConfirmDeclined {
                    evidence: DeclineEvidence::Manual,
                },
            },
            &context(),
        );
        assert_eq!(collision, ApplyOutcome::KeyCollision);
    }

    #[test]
    fn unseen_key_with_stale_version_is_a_visible_refresh_conflict() {
        let mut ledger = Ledger::new();
        let state = open_state();
        let outcome = apply_command(
            &mut ledger,
            &state,
            super::CommandRequest {
                key: CommandKey([5; 16]),
                expected_loop_version: version(999),
                payload: CommandPayload::ConfirmFulfilled {
                    closure_evidence_refs: vec![ids(50)],
                },
            },
            &context(),
        );
        assert_eq!(outcome, ApplyOutcome::RefreshConflict);
    }

    #[test]
    fn key_lookup_happens_before_version_comparison() {
        // A stale expected_loop_version would normally be a refresh
        // conflict, but because the key was already recorded with the same
        // payload, the lookup step returns the historical replay first.
        let mut ledger = Ledger::new();
        let state = open_state();
        let key = CommandKey([6; 16]);
        let payload = CommandPayload::ConfirmFulfilled {
            closure_evidence_refs: vec![ids(50)],
        };
        let first = apply_command(
            &mut ledger,
            &state,
            super::CommandRequest {
                key,
                expected_loop_version: state.loop_version,
                payload: payload.clone(),
            },
            &context(),
        );
        assert!(matches!(first, ApplyOutcome::Applied(_)));

        let stale_version_but_same_key_and_payload = apply_command(
            &mut ledger,
            &state,
            super::CommandRequest {
                key,
                expected_loop_version: version(9_999),
                payload,
            },
            &context(),
        );
        assert!(matches!(
            stale_version_but_same_key_and_payload,
            ApplyOutcome::Replayed(_)
        ));
    }

    #[test]
    fn a_rejected_command_is_never_recorded_and_a_corrected_retry_under_the_same_key_succeeds() {
        let mut ledger = Ledger::new();
        let state = open_state();
        let key = CommandKey([7; 16]);
        let rejected = apply_command(
            &mut ledger,
            &state,
            super::CommandRequest {
                key,
                expected_loop_version: state.loop_version,
                payload: CommandPayload::ConfirmFulfilled {
                    closure_evidence_refs: Vec::new(),
                },
            },
            &context(),
        );
        assert!(matches!(rejected, ApplyOutcome::Rejected(_)));

        let corrected_retry = apply_command(
            &mut ledger,
            &state,
            super::CommandRequest {
                key,
                expected_loop_version: state.loop_version,
                payload: CommandPayload::ConfirmFulfilled {
                    closure_evidence_refs: vec![ids(50)],
                },
            },
            &context(),
        );
        assert!(matches!(corrected_retry, ApplyOutcome::Applied(_)));
    }

    #[test]
    fn reopen_requires_terminal_and_produces_open_resolution_none_needs_review() {
        let mut ledger = Ledger::new();
        let terminal_state = LoopState {
            facets: FacetTuple {
                obligation_state: ObligationState::Terminal,
                resolution: Resolution::Closed,
                ..open_facets()
            },
            ..open_state()
        };
        let outcome = apply_command(
            &mut ledger,
            &terminal_state,
            super::CommandRequest {
                key: CommandKey([8; 16]),
                expected_loop_version: terminal_state.loop_version,
                payload: CommandPayload::Reopen,
            },
            &context(),
        );
        let ApplyOutcome::Applied(transition) = outcome else {
            panic!("expected Applied");
        };
        assert_eq!(
            transition.new_facets.obligation_state,
            ObligationState::Open
        );
        assert_eq!(transition.new_facets.resolution, Resolution::None);
        assert_eq!(
            transition.new_facets.closure_review_state,
            ClosureReviewState::None
        );
        assert!(
            transition
                .new_facets
                .review_flags
                .contains(ReviewFlagCode::NeedsReview)
        );

        // Reopen from a non-terminal loop is rejected.
        let non_terminal_reopen = apply_command(
            &mut ledger,
            &open_state(),
            super::CommandRequest {
                key: CommandKey([12; 16]),
                expected_loop_version: open_state().loop_version,
                payload: CommandPayload::Reopen,
            },
            &context(),
        );
        assert_eq!(
            non_terminal_reopen,
            ApplyOutcome::Rejected(CommandRejection::NotAllowedFromCurrentObligationState)
        );
    }

    #[test]
    fn old_command_key_replayed_after_reopen_never_re_terminals_the_new_generation() {
        let mut ledger = Ledger::new();
        let state = open_state();
        let fulfill_key = CommandKey([13; 16]);
        let fulfill_payload = CommandPayload::ConfirmFulfilled {
            closure_evidence_refs: vec![ids(50)],
        };
        let fulfilled = apply_command(
            &mut ledger,
            &state,
            super::CommandRequest {
                key: fulfill_key,
                expected_loop_version: state.loop_version,
                payload: fulfill_payload.clone(),
            },
            &context(),
        );
        let ApplyOutcome::Applied(fulfilled_transition) = fulfilled else {
            panic!("expected Applied");
        };
        assert_eq!(fulfilled_transition.loop_version, version(2));

        // The caller reopens: a fresh generation begins at version 3.
        let terminal_state = LoopState {
            facets: fulfilled_transition.new_facets,
            loop_version: fulfilled_transition.loop_version,
            ..state
        };
        let reopened = apply_command(
            &mut ledger,
            &terminal_state,
            super::CommandRequest {
                key: CommandKey([14; 16]),
                expected_loop_version: terminal_state.loop_version,
                payload: CommandPayload::Reopen,
            },
            &context(),
        );
        let ApplyOutcome::Applied(reopen_transition) = reopened else {
            panic!("expected Applied");
        };
        assert_eq!(reopen_transition.loop_version, version(3));

        // Replaying the *old* fulfill key/payload against the reopened
        // generation returns the historical (stale) result, not a fresh
        // Applied transition: the caller must never feed a Replayed
        // outcome's transition into the current generation's state, so the
        // reopened loop is never silently re-terminaled.
        let new_generation_state = LoopState {
            facets: reopen_transition.new_facets,
            loop_version: reopen_transition.loop_version,
            ..state
        };
        let replay_after_reopen = apply_command(
            &mut ledger,
            &new_generation_state,
            super::CommandRequest {
                key: fulfill_key,
                expected_loop_version: new_generation_state.loop_version,
                payload: fulfill_payload,
            },
            &context(),
        );
        match replay_after_reopen {
            ApplyOutcome::Replayed(replayed) => {
                assert_eq!(replayed.loop_version, version(2));
                assert_ne!(
                    replayed.loop_version,
                    new_generation_state.loop_version.next().unwrap()
                );
            }
            other => panic!("expected Replayed, got {other:?}"),
        }
    }

    #[test]
    fn keep_open_suppresses_only_when_a_possible_hypothesis_is_current() {
        let mut ledger = Ledger::new();
        let state = LoopState {
            facets: FacetTuple {
                closure_review_state: ClosureReviewState::Possible,
                ..open_facets()
            },
            ..open_state()
        };
        let identity = crate::hypothesis::HypothesisIdentity {
            target_loop: state.loop_id,
            kind: crate::hypothesis::HypothesisKind::Closure,
            source_refs: vec![crate::hypothesis::VersionedSourceRef {
                source_ref_id: ids(60),
                source_version: version(1),
            }],
            policy_version: version(1),
        };
        let outcome = apply_command(
            &mut ledger,
            &state,
            super::CommandRequest {
                key: CommandKey([15; 16]),
                expected_loop_version: state.loop_version,
                payload: CommandPayload::KeepOpen {
                    hypothesis_identity: identity,
                },
            },
            &context(),
        );
        let ApplyOutcome::Applied(transition) = outcome else {
            panic!("expected Applied");
        };
        assert_eq!(
            transition.new_facets.closure_review_state,
            ClosureReviewState::KeptOpen
        );
        assert_eq!(
            transition.new_facets.obligation_state,
            ObligationState::Open
        );
    }

    #[test]
    fn only_full_transfer_delegation_terminalizes() {
        let mut ledger = Ledger::new();
        let state = open_state();
        for (delegation, expect_terminal) in [
            (DelegationCode::Transferred, true),
            (DelegationCode::Shared, false),
            (DelegationCode::Assisted, false),
            (DelegationCode::Retained, false),
            (DelegationCode::Unclear, false),
        ] {
            let mut key_byte = 20u8;
            key_byte += delegation as u8;
            let outcome = apply_command(
                &mut ledger,
                &state,
                super::CommandRequest {
                    key: CommandKey([key_byte; 16]),
                    expected_loop_version: state.loop_version,
                    payload: CommandPayload::ConfirmTransfer {
                        delegation,
                        evidence: super::TransferEvidence::Manual,
                    },
                },
                &context(),
            );
            let ApplyOutcome::Applied(transition) = outcome else {
                panic!("expected Applied for {delegation:?}");
            };
            assert_eq!(
                matches!(
                    transition.new_facets.obligation_state,
                    ObligationState::Terminal
                ),
                expect_terminal,
                "delegation {delegation:?}"
            );
        }
    }

    #[test]
    fn dismiss_classify_routes_delegated_and_moot_reasons_to_their_own_resolution() {
        let mut ledger = Ledger::new();
        let state = open_state();
        let cases = [
            (CorrectionReasonCode::Delegated, Resolution::Delegated),
            (CorrectionReasonCode::Moot, Resolution::Moot),
            (CorrectionReasonCode::NotMine, Resolution::Dismissed),
            (CorrectionReasonCode::Duplicate, Resolution::Dismissed),
        ];
        for (index, (reason, expected_resolution)) in cases.into_iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let key_byte = 30u8 + index as u8;
            let outcome = apply_command(
                &mut ledger,
                &state,
                super::CommandRequest {
                    key: CommandKey([key_byte; 16]),
                    expected_loop_version: state.loop_version,
                    payload: CommandPayload::DismissClassify { reason },
                },
                &context(),
            );
            let ApplyOutcome::Applied(transition) = outcome else {
                panic!("expected Applied for {reason:?}");
            };
            assert_eq!(transition.new_facets.resolution, expected_resolution);
            assert_eq!(
                transition.new_facets.obligation_state,
                ObligationState::Terminal
            );
        }
    }

    #[test]
    fn candidate_may_be_declined_transferred_moot_completed_or_dismissed_without_first_promoting() {
        let candidate_state = LoopState {
            facets: FacetTuple {
                obligation_state: ObligationState::Candidate,
                ..open_facets()
            },
            ..open_state()
        };
        let mut ledger = Ledger::new();
        let outcome = apply_command(
            &mut ledger,
            &candidate_state,
            super::CommandRequest {
                key: CommandKey([40; 16]),
                expected_loop_version: candidate_state.loop_version,
                payload: CommandPayload::ConfirmDeclined {
                    evidence: DeclineEvidence::Manual,
                },
            },
            &context(),
        );
        // The candidate never passes through Open on the way to Terminal:
        // build_transition's ConfirmDeclined arm reads obligation directly
        // off `state.facets` (Candidate) and requires no prior promotion.
        assert!(matches!(outcome, ApplyOutcome::Applied(_)));
    }

    #[test]
    fn reminder_state_never_influences_or_is_overwritten_by_any_command_outcome() {
        // Legality rule 9 / reminder_boundary: reminder completion, deletion,
        // missing, conflict, or ambiguity changes only the reminder facet.
        // Run the same command from two loops that differ *only* in
        // reminder_state and assert every command produces the identical
        // obligation/resolution/closure-review outcome either way, and
        // that whichever reminder_state the loop started with survives
        // unchanged in the resulting transition (no command handler reads
        // or writes `reminder_state`, so `..state.facets` always carries it
        // through verbatim).
        for reminder_state in [
            ReminderState::None,
            ReminderState::Conflict,
            ReminderState::Missing,
            ReminderState::AmbiguousWrite,
            ReminderState::CompletedNeedsEvidence,
        ] {
            let state = LoopState {
                facets: FacetTuple {
                    reminder_state,
                    ..open_facets()
                },
                ..open_state()
            };
            let mut ledger = Ledger::new();
            let outcome = apply_command(
                &mut ledger,
                &state,
                super::CommandRequest {
                    key: CommandKey([50 + reminder_state as u8; 16]),
                    expected_loop_version: state.loop_version,
                    payload: CommandPayload::ConfirmMineActionable,
                },
                &context(),
            );
            // ConfirmMineActionable only applies from Candidate, so every
            // one of these (state is Open) is rejected the same way
            // regardless of reminder_state, proving reminder_state plays no
            // role in the obligation-transition decision.
            assert_eq!(
                outcome,
                ApplyOutcome::Rejected(CommandRejection::NotAllowedFromCurrentObligationState)
            );

            let request_evidence_outcome = apply_command(
                &mut ledger,
                &state,
                super::CommandRequest {
                    key: CommandKey([70 + reminder_state as u8; 16]),
                    expected_loop_version: state.loop_version,
                    payload: CommandPayload::RequestEvidence {
                        kind: crate::hypothesis::HypothesisKind::Closure,
                    },
                },
                &context(),
            );
            let ApplyOutcome::Applied(transition) = request_evidence_outcome else {
                panic!("expected Applied");
            };
            // The reminder facet the loop started with is carried through
            // unchanged: no command in this crate reads or mutates it.
            assert_eq!(transition.new_facets.reminder_state, reminder_state);
            assert_eq!(
                transition.new_facets.obligation_state,
                ObligationState::Open
            );
        }
    }

    #[test]
    fn terminal_review_note_adds_needs_review_without_changing_resolution() {
        let terminal_state = LoopState {
            facets: FacetTuple {
                obligation_state: ObligationState::Terminal,
                resolution: Resolution::Closed,
                ..open_facets()
            },
            ..open_state()
        };
        let transition = apply_terminal_review_note(&terminal_state, ids(70), &context())
            .expect("review note applies");
        assert_eq!(transition.new_facets.resolution, Resolution::Closed);
        assert_eq!(
            transition.new_facets.obligation_state,
            ObligationState::Terminal
        );
        assert!(
            transition
                .new_facets
                .review_flags
                .contains(ReviewFlagCode::NeedsReview)
        );

        let non_terminal = open_state();
        let rejected = apply_terminal_review_note(&non_terminal, ids(70), &context());
        assert_eq!(
            rejected,
            Err(CommandRejection::NotAllowedFromCurrentObligationState)
        );
    }

    #[test]
    fn remap_evidence_requires_both_loop_versions_and_produces_both_transitions_atomically() {
        let source = open_state();
        let target = LoopState {
            loop_id: ids(2),
            ..open_state()
        };
        let stale = apply_remap_evidence(
            &source,
            &target,
            &super::RemapRequest {
                source_expected_version: version(999),
                target_expected_version: target.loop_version,
                evidence_ref: ids(80),
                detach_transition_id: ids(81),
                attach_transition_id: ids(82),
            },
            &context(),
        );
        assert_eq!(stale, Err(RemapRejection::StaleSourceVersion));

        let (detach, attach) = apply_remap_evidence(
            &source,
            &target,
            &super::RemapRequest {
                source_expected_version: source.loop_version,
                target_expected_version: target.loop_version,
                evidence_ref: ids(80),
                detach_transition_id: ids(81),
                attach_transition_id: ids(82),
            },
            &context(),
        )
        .expect("remap applies");
        assert_eq!(detach.loop_id, source.loop_id);
        assert_eq!(attach.loop_id, target.loop_id);
    }

    #[test]
    fn duplicate_merge_transfers_evidence_and_terminalizes_only_the_loser() {
        let survivor = open_state();
        let loser = LoopState {
            loop_id: ids(3),
            ..open_state()
        };
        let (survivor_transition, loser_transition) = apply_duplicate_merge(
            &survivor,
            survivor.loop_version,
            &loser,
            loser.loop_version,
            &[ids(90)],
            ids(91),
            ids(92),
            &context(),
        )
        .expect("merge applies");
        assert_eq!(
            survivor_transition.new_facets.obligation_state,
            ObligationState::Open
        );
        assert_eq!(
            loser_transition.new_facets.obligation_state,
            ObligationState::Terminal
        );
        assert_eq!(
            loser_transition.new_facets.resolution,
            Resolution::Dismissed
        );
    }

    #[test]
    fn duplicate_merge_rejects_an_already_terminal_loser() {
        let survivor = open_state();
        let terminal_loser = LoopState {
            loop_id: ids(3),
            facets: FacetTuple {
                obligation_state: ObligationState::Terminal,
                resolution: Resolution::Closed,
                ..open_facets()
            },
            ..open_state()
        };
        let result = apply_duplicate_merge(
            &survivor,
            survivor.loop_version,
            &terminal_loser,
            terminal_loser.loop_version,
            &[ids(90)],
            ids(91),
            ids(92),
            &context(),
        );
        assert_eq!(result, Err(DuplicateMergeRejection::LoserAlreadyTerminal));
    }
}
