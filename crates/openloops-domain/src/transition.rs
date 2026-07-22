//! The transition record: the sole way this crate represents a loop-state
//! change.
//!
//! Implements `record_contracts[transition]` and enforces, at construction
//! time rather than only in a test, legality rule 2 ("a system or model
//! hypothesis can never produce terminal obligation state or a non-none
//! resolution"), legality rule 10 ("later evidence on a terminal loop ...
//! never changes resolution"), and the `user_action_rule` ("required for
//! actor user and prohibited for actor system"). Every [`Transition`] this
//! crate can construct also has `new_facets` that pass
//! [`crate::legality::validate`]: there is no code path that builds an
//! illegal transition.
//!
//! The system-actor check compares `prior_facets` to `new_facets` rather
//! than inspecting `new_facets` alone, because rule 10 also permits a system
//! actor to add `needs_review` to a loop that was *already* terminal (a
//! later-evidence review note) — only a system actor *newly making* a loop
//! terminal, or *changing* an existing resolution, is rejected.

use crate::deadline::UnixSeconds;
use crate::facets::{
    ActorCode, AnalysisState, ClosureReviewState, ConfidenceBucket, DeadlineState, ObligationState,
    ProvenanceSet, ReminderState, Resolution, ReviewFlagSet, SourceState,
};
use crate::ids::{BoundedIdList, BoundedInsertError, OpaqueId, Version};
use crate::legality::{self, LegalityViolation, LoopLegalityFacets};

/// A complete snapshot of every independent loop facet ADR-008 names
/// ("origin, provenance set, obligation, review flags, deadline, closure
/// review, reminder, analysis, source, resolution, and confidence"), minus
/// `origin` (fixed at loop creation and never part of a transition's
/// prior/new pair). `record_contracts[transition].facet_rule`: "complete
/// prior and new facet tuples; no partial patch or generic map" — this
/// struct has no optional/patch fields, so a [`Transition`] always carries
/// the complete tuple on both sides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FacetTuple {
    /// `provenance_codes`.
    pub provenance: ProvenanceSet,
    /// `obligation_state_code`.
    pub obligation_state: ObligationState,
    /// `review_flag_codes`.
    pub review_flags: ReviewFlagSet,
    /// `deadline_state_code`.
    pub deadline_state: DeadlineState,
    /// `closure_review_state_code`.
    pub closure_review_state: ClosureReviewState,
    /// `reminder_state_code`.
    pub reminder_state: ReminderState,
    /// `analysis_state_code`.
    pub analysis_state: AnalysisState,
    /// `source_state_code`.
    pub source_state: SourceState,
    /// `resolution_code`.
    pub resolution: Resolution,
    /// `confidence_bucket`.
    pub confidence_bucket: ConfidenceBucket,
}

impl FacetTuple {
    /// Projects the four facets [`crate::legality::validate`] checks.
    #[must_use]
    pub const fn legality_view(&self) -> LoopLegalityFacets {
        LoopLegalityFacets {
            obligation_state: self.obligation_state,
            resolution: self.resolution,
            closure_review_state: self.closure_review_state,
            deadline_state: self.deadline_state,
        }
    }
}

/// The closed catalog of transition reasons this crate produces. Contract
/// policy-state-boundary.json does not pin an exact reason-code catalog (the
/// `reason_code` field is abstract); this is the ADR-008 implementation's own
/// closed catalog, and every variant traces to one contract clause named in
/// its doc comment. Nothing in this catalog carries free text
/// (`record_contracts[transition].text_rule`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum TransitionReasonCode {
    /// `establishment_policy`: explicit outgoing promise established.
    EstablishedFromPromise,
    /// `establishment_policy`: direct incoming request/question established.
    EstablishedFromRequest,
    /// `establishment_policy`: acknowledgement of an associated request.
    EstablishedFromAcknowledgement,
    /// `establishment_policy`: explicit deterministic attribution.
    EstablishedFromAttribution,
    /// `establishment_policy`: soft/ambiguous detection flagged for review.
    FlaggedCandidateForReview,
    /// `command_policy.confirm_mine_actionable`.
    ConfirmedMineActionable,
    /// `command_policy.confirm_fulfilled`.
    ConfirmedFulfilled,
    /// `command_policy.confirm_declined`.
    ConfirmedDeclined,
    /// `command_policy.confirm_moot`.
    ConfirmedMoot,
    /// `command_policy.confirm_transfer` (full transfer only).
    ConfirmedTransferred,
    /// `command_policy.confirm_transfer` / OL-DELEG-001: shared, assisted,
    /// retained, or unclear responsibility is noted without terminalizing.
    DelegationNoted,
    /// `command_policy.keep_open`.
    KeptOpenSuppressed,
    /// `command_policy.request_evidence`.
    EvidenceRequested,
    /// `command_policy.remap_evidence` (detach side).
    EvidenceRemappedFrom,
    /// `command_policy.remap_evidence` (attach side).
    EvidenceRemappedTo,
    /// `command_policy.select_different_evidence`.
    EvidenceSelectedDifferent,
    /// `command_policy.completed_outside_email`.
    CompletedOutsideEmail,
    /// `command_policy.dismiss_classify`.
    DismissedClassified,
    /// `command_policy.reopen`.
    Reopened,
    /// `correction_policy.duplicate` (survivor side).
    DuplicateMergeSurvivorTransfer,
    /// `correction_policy.duplicate` (loser side).
    DuplicateMergeLoserDismissed,
    /// ADR-008 "Later evidence may add `needs_review` ... to a terminal
    /// loop, but it never silently reopens or replaces the resolution."
    TerminalReviewNoteAdded,
    /// `deadline_policy.operative_rules`: a causally later user extension
    /// became operative (`renegotiated`).
    DeadlineRenegotiated,
    /// `deadline_policy.prompt_commands.no_deadline`.
    DeadlineUndatedConfirmed,
    /// `deadline_policy.defer_reminder_rule`.
    DeadlinePromptDeferred,
}

/// A rejected [`Transition::new`] call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionError {
    /// Legality rule 2 or 10: a system actor newly made the loop terminal,
    /// or changed an already-set resolution.
    SystemActorCannotProduceTerminalState,
    /// `user_action_rule`: actor is `user` but no `user_action_id` was
    /// supplied.
    UserActionRequiredForUserActor,
    /// `user_action_rule`: actor is `system` but a `user_action_id` was
    /// supplied.
    UserActionProhibitedForSystemActor,
    /// `source_ref_bound` (32) was exceeded.
    TooManySourceRefs,
    /// A duplicate source ref id was supplied.
    DuplicateSourceRef,
    /// The resulting `new_facets` failed Cartesian legality validation.
    IllegalNewFacets(LegalityViolation),
}

/// One committed loop-state change. `record_contracts[transition]`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Transition {
    /// `transition_id`.
    pub transition_id: OpaqueId,
    /// `loop_id`.
    pub loop_id: OpaqueId,
    /// `user_action_id`; required for `actor=user`, prohibited for
    /// `actor=system`.
    pub user_action_id: Option<OpaqueId>,
    /// `prior_facet_codes`.
    pub prior_facets: FacetTuple,
    /// `new_facet_codes`.
    pub new_facets: FacetTuple,
    /// `reason_code`.
    pub reason_code: TransitionReasonCode,
    /// `actor_code`.
    pub actor: ActorCode,
    /// `source_ref_ids`, bounded at exactly 32.
    pub source_ref_ids: BoundedIdList,
    /// `occurred_at`.
    pub occurred_at: UnixSeconds,
    /// `loop_version`: the version this transition commits *to* (the
    /// post-increment value).
    pub loop_version: Version,
    /// `policy_version`.
    pub policy_version: Version,
}

impl Transition {
    /// The exact `record_contracts[transition].source_ref_bound`.
    pub const SOURCE_REF_BOUND: usize = 32;

    /// Builds a transition, enforcing legality rule 2, the
    /// `user_action_rule`, the source-ref bound, and full Cartesian legality
    /// of `new_facets`.
    ///
    /// # Errors
    ///
    /// Returns [`TransitionError`] when any of those checks fails. No
    /// `Transition` value can exist in a state any of these checks would
    /// reject.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        transition_id: OpaqueId,
        loop_id: OpaqueId,
        user_action_id: Option<OpaqueId>,
        prior_facets: FacetTuple,
        new_facets: FacetTuple,
        reason_code: TransitionReasonCode,
        actor: ActorCode,
        source_refs: &[OpaqueId],
        occurred_at: UnixSeconds,
        loop_version: Version,
        policy_version: Version,
    ) -> Result<Self, TransitionError> {
        match (actor, user_action_id) {
            (ActorCode::User, None) => return Err(TransitionError::UserActionRequiredForUserActor),
            (ActorCode::System, Some(_)) => {
                return Err(TransitionError::UserActionProhibitedForSystemActor);
            }
            (ActorCode::User, Some(_)) | (ActorCode::System, None) => {}
        }

        let obligation_became_terminal =
            !matches!(prior_facets.obligation_state, ObligationState::Terminal)
                && matches!(new_facets.obligation_state, ObligationState::Terminal);
        let resolution_changed = prior_facets.resolution != new_facets.resolution;
        if matches!(actor, ActorCode::System) && (obligation_became_terminal || resolution_changed)
        {
            return Err(TransitionError::SystemActorCannotProduceTerminalState);
        }

        let mut source_ref_ids = BoundedIdList::with_capacity(Self::SOURCE_REF_BOUND);
        for source_ref in source_refs {
            match source_ref_ids.push(*source_ref) {
                Ok(()) => {}
                Err(BoundedInsertError::CapacityExceeded) => {
                    return Err(TransitionError::TooManySourceRefs);
                }
                Err(BoundedInsertError::Duplicate) => {
                    return Err(TransitionError::DuplicateSourceRef);
                }
            }
        }

        legality::validate(new_facets.legality_view())
            .map_err(TransitionError::IllegalNewFacets)?;

        Ok(Self {
            transition_id,
            loop_id,
            user_action_id,
            prior_facets,
            new_facets,
            reason_code,
            actor,
            source_ref_ids,
            occurred_at,
            loop_version,
            policy_version,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{FacetTuple, Transition, TransitionError, TransitionReasonCode};
    use crate::deadline::UnixSeconds;
    use crate::facets::{
        ActorCode, AnalysisState, ClosureReviewState, ConfidenceBucket, DeadlineState,
        ObligationState, ProvenanceSet, ReminderState, Resolution, ReviewFlagSet, SourceState,
    };
    use crate::ids::{OpaqueId, Version};

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

    fn terminal_facets() -> FacetTuple {
        FacetTuple {
            obligation_state: ObligationState::Terminal,
            resolution: Resolution::Closed,
            ..open_facets()
        }
    }

    fn ids(byte: u8) -> OpaqueId {
        OpaqueId::from_bytes([byte; 16])
    }

    fn version(value: u64) -> Version {
        Version::new(core::num::NonZeroU64::new(value).expect("nonzero test version"))
    }

    #[test]
    fn system_actor_cannot_produce_terminal_state() {
        let result = Transition::new(
            ids(1),
            ids(2),
            None,
            open_facets(),
            terminal_facets(),
            TransitionReasonCode::ConfirmedFulfilled,
            ActorCode::System,
            &[],
            UnixSeconds(0),
            version(2),
            version(1),
        );
        assert_eq!(
            result,
            Err(TransitionError::SystemActorCannotProduceTerminalState)
        );
    }

    #[test]
    fn user_actor_can_produce_terminal_state_with_a_user_action_id() {
        let result = Transition::new(
            ids(1),
            ids(2),
            Some(ids(3)),
            open_facets(),
            terminal_facets(),
            TransitionReasonCode::ConfirmedFulfilled,
            ActorCode::User,
            &[],
            UnixSeconds(0),
            version(2),
            version(1),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn user_actor_requires_a_user_action_id() {
        let result = Transition::new(
            ids(1),
            ids(2),
            None,
            open_facets(),
            open_facets(),
            TransitionReasonCode::ConfirmedMineActionable,
            ActorCode::User,
            &[],
            UnixSeconds(0),
            version(2),
            version(1),
        );
        assert_eq!(result, Err(TransitionError::UserActionRequiredForUserActor));
    }

    #[test]
    fn system_actor_prohibits_a_user_action_id() {
        let result = Transition::new(
            ids(1),
            ids(2),
            Some(ids(3)),
            open_facets(),
            open_facets(),
            TransitionReasonCode::EvidenceRequested,
            ActorCode::System,
            &[],
            UnixSeconds(0),
            version(2),
            version(1),
        );
        assert_eq!(
            result,
            Err(TransitionError::UserActionProhibitedForSystemActor)
        );
    }

    #[test]
    fn source_ref_bound_of_32_is_enforced() {
        let refs: Vec<OpaqueId> = (0..33u8).map(ids).collect();
        let result = Transition::new(
            ids(1),
            ids(2),
            Some(ids(3)),
            open_facets(),
            open_facets(),
            TransitionReasonCode::EvidenceRequested,
            ActorCode::User,
            &refs,
            UnixSeconds(0),
            version(2),
            version(1),
        );
        assert_eq!(result, Err(TransitionError::TooManySourceRefs));
    }

    #[test]
    fn duplicate_source_ref_is_rejected() {
        let result = Transition::new(
            ids(1),
            ids(2),
            Some(ids(3)),
            open_facets(),
            open_facets(),
            TransitionReasonCode::EvidenceRequested,
            ActorCode::User,
            &[ids(9), ids(9)],
            UnixSeconds(0),
            version(2),
            version(1),
        );
        assert_eq!(result, Err(TransitionError::DuplicateSourceRef));
    }

    #[test]
    fn system_actor_may_add_a_review_flag_to_an_already_terminal_loop() {
        // Rule 10: later evidence on a terminal loop may add needs_review
        // without changing resolution; the system actor is allowed here
        // because the loop was already terminal before this transition.
        use crate::facets::ReviewFlagCode;
        let review_noted = FacetTuple {
            review_flags: terminal_facets()
                .review_flags
                .with(ReviewFlagCode::NeedsReview),
            ..terminal_facets()
        };
        let result = Transition::new(
            ids(1),
            ids(2),
            None,
            terminal_facets(),
            review_noted,
            TransitionReasonCode::TerminalReviewNoteAdded,
            ActorCode::System,
            &[ids(9)],
            UnixSeconds(0),
            version(2),
            version(1),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn system_actor_cannot_change_an_already_set_resolution() {
        // Rule 10: later evidence never replaces the resolution, even
        // between two non-none values.
        let declined = FacetTuple {
            resolution: Resolution::Declined,
            ..terminal_facets()
        };
        let result = Transition::new(
            ids(1),
            ids(2),
            None,
            terminal_facets(),
            declined,
            TransitionReasonCode::TerminalReviewNoteAdded,
            ActorCode::System,
            &[],
            UnixSeconds(0),
            version(2),
            version(1),
        );
        assert_eq!(
            result,
            Err(TransitionError::SystemActorCannotProduceTerminalState)
        );
    }

    #[test]
    fn illegal_new_facets_are_rejected_at_construction() {
        let illegal = FacetTuple {
            obligation_state: ObligationState::Candidate,
            resolution: Resolution::Closed,
            ..open_facets()
        };
        let result = Transition::new(
            ids(1),
            ids(2),
            Some(ids(3)),
            open_facets(),
            illegal,
            TransitionReasonCode::ConfirmedFulfilled,
            ActorCode::User,
            &[],
            UnixSeconds(0),
            version(2),
            version(1),
        );
        assert!(matches!(result, Err(TransitionError::IllegalNewFacets(_))));
    }
}
