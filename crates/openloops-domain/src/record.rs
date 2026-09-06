//! The `loop` and `deadline_evidence` records.
//!
//! `crate::transition::Transition` already implements
//! `record_contracts[transition]`; this module implements the other two:
//! `record_contracts[loop]` ([`Loop`]) and `record_contracts[deadline_evidence]`
//! ([`DeadlineEvidence`]). Both are plain Rust structs with no optional
//! generic-map escape hatch, so `"unknown_fields":"reject"` holds structurally
//! — there is no field a caller could add that the type would silently
//! accept.
//!
//! This crate performs no calendar arithmetic (no timezone/DST/business-day
//! resolution library, and ADR-008 explicitly leaves that to policy
//! configuration): every temporal value here is an already-resolved
//! [`UnixSeconds`] instant or instant pair. That is consistent with
//! `docs/product-spec.md` 5.3 ("The encrypted structured value is
//! operational metadata needed to schedule and sort"); reconstructing the
//! original phrase is explicitly out of scope for any persisted field.

use crate::deadline::UnixSeconds;
use crate::facets::{
    AnalysisState, ClosureReviewState, ConfidenceBucket, DeadlineInterpretation, DeadlinePrecision,
    DeadlineSourceType, DeadlineState, ObligationState, OperativeSelection, OriginCode,
    ProvenanceSet, ReminderState, Resolution, ReviewFlagSet, SourceState,
};
use crate::ids::{
    BoundedIdList, BoundedInsertError, ModelLabelCode, OpaqueId, SourceRoleRefList, TimezoneId,
    Version,
};
use crate::legality::{self, LegalityViolation, LoopLegalityFacets};

/// `deadline_evidence.value_or_range`: a strict tagged temporal value whose
/// variant is always exactly its precision. There is no separate
/// `precision_code` field to fall out of sync with the value, because
/// [`DeadlineEvidence::new`] derives `precision_code` from this enum instead
/// of accepting it as an independent, possibly-inconsistent input — the
/// contract's `"value_rule"` ("strict tagged temporal value matching
/// precision") is therefore a type-system guarantee, not a runtime check
/// that could be skipped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeadlineValue {
    /// `instant` precision.
    Instant(UnixSeconds),
    /// `date` precision: the evidence-derived date, represented as its
    /// referenced instant.
    Date(UnixSeconds),
    /// `business_day` precision: the evidence-derived business day.
    BusinessDay(UnixSeconds),
    /// `week` precision: the retained range, start inclusive through end
    /// exclusive.
    Week {
        /// Range start.
        start: UnixSeconds,
        /// Range end (after start).
        end: UnixSeconds,
    },
    /// `event_relative` precision: the correlated event instant, or `None`
    /// while no unique event is yet selected.
    EventRelative(Option<UnixSeconds>),
    /// `soft` precision: no operational boundary.
    Soft,
    /// `unspecified` precision: no value was supplied.
    Unspecified,
}

impl DeadlineValue {
    /// The precision this value's variant represents.
    #[must_use]
    pub const fn precision(&self) -> DeadlinePrecision {
        match self {
            Self::Instant(_) => DeadlinePrecision::Instant,
            Self::Date(_) => DeadlinePrecision::Date,
            Self::BusinessDay(_) => DeadlinePrecision::BusinessDay,
            Self::Week { .. } => DeadlinePrecision::Week,
            Self::EventRelative(_) => DeadlinePrecision::EventRelative,
            Self::Soft => DeadlinePrecision::Soft,
            Self::Unspecified => DeadlinePrecision::Unspecified,
        }
    }
}

/// A rejected [`DeadlineEvidence::new`] call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeadlineEvidenceError {
    /// `source_ref_rule`: a source ref is required except for explicit
    /// `user_supplied` evidence.
    SourceRefRequiredExceptUserSupplied,
    /// A `Week` value had `start >= end`.
    InvalidWeekRange,
}

/// `record_contracts[deadline_evidence]`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeadlineEvidence {
    /// `deadline_evidence_id`.
    pub deadline_evidence_id: OpaqueId,
    /// `loop_id`.
    pub loop_id: OpaqueId,
    /// `source_ref_id`; required except for `user_supplied` evidence.
    pub source_ref_id: Option<OpaqueId>,
    /// `source_type_code`.
    pub source_type_code: DeadlineSourceType,
    /// `precision_code`, derived from `value_or_range`.
    pub precision_code: DeadlinePrecision,
    /// `interpretation_code`.
    pub interpretation_code: DeadlineInterpretation,
    /// `operative_selection_code`.
    pub operative_selection_code: OperativeSelection,
    /// `value_or_range`.
    pub value_or_range: DeadlineValue,
    /// `timezone_id`.
    pub timezone_id: TimezoneId,
}

impl DeadlineEvidence {
    /// Builds one deadline-evidence record.
    ///
    /// # Errors
    ///
    /// Returns [`DeadlineEvidenceError`] when `source_ref_id` is absent for
    /// non-`user_supplied` evidence, or a `Week` value's range is not
    /// `start < end`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        deadline_evidence_id: OpaqueId,
        loop_id: OpaqueId,
        source_ref_id: Option<OpaqueId>,
        source_type_code: DeadlineSourceType,
        interpretation_code: DeadlineInterpretation,
        operative_selection_code: OperativeSelection,
        value_or_range: DeadlineValue,
        timezone_id: TimezoneId,
    ) -> Result<Self, DeadlineEvidenceError> {
        if source_ref_id.is_none() && !matches!(source_type_code, DeadlineSourceType::UserSupplied)
        {
            return Err(DeadlineEvidenceError::SourceRefRequiredExceptUserSupplied);
        }
        if let DeadlineValue::Week { start, end } = value_or_range
            && start >= end
        {
            return Err(DeadlineEvidenceError::InvalidWeekRange);
        }
        Ok(Self {
            deadline_evidence_id,
            loop_id,
            source_ref_id,
            source_type_code,
            precision_code: value_or_range.precision(),
            interpretation_code,
            operative_selection_code,
            value_or_range,
            timezone_id,
        })
    }
}

/// A rejected [`Loop::new`] call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoopError {
    /// The four Cartesian legality facets did not pass
    /// [`crate::legality::validate`].
    IllegalFacets(LegalityViolation),
    /// `collection_bounds.source_ref_ids_by_role` (256) was exceeded.
    TooManySourceRefsByRole,
    /// A duplicate `(role, source_ref_id)` pair was supplied.
    DuplicateSourceRefByRole,
    /// `collection_bounds.reminder_ref_ids` (16) was exceeded.
    TooManyReminderRefs,
    /// A duplicate `reminder_ref_id` was supplied.
    DuplicateReminderRef,
    /// `collection_bounds.transition_ref_ids` (4096) was exceeded.
    TooManyTransitionRefs,
    /// A duplicate `transition_ref_id` was supplied.
    DuplicateTransitionRef,
    /// ADR-008 legality rule 3 (contract rule 4): `approaching|overdue`
    /// requires a resolved operational boundary, so both operative-deadline
    /// fields were `None` while `deadline_state` was aging.
    AgingWithoutOperativeBoundary,
}

/// `record_contracts[loop]`: the complete, closed loop record. Fields appear
/// in exactly `exact_fields_in_order`; the three `nullable_fields` are
/// `Option`, every other field is required by the type itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Loop {
    /// `loop_id`.
    pub loop_id: OpaqueId,
    /// `account_ref`.
    pub account_ref: OpaqueId,
    /// `origin_code`.
    pub origin_code: OriginCode,
    /// `provenance_codes`.
    pub provenance_codes: ProvenanceSet,
    /// `obligation_state_code`.
    pub obligation_state_code: ObligationState,
    /// `review_flag_codes`.
    pub review_flag_codes: ReviewFlagSet,
    /// `deadline_state_code`.
    pub deadline_state_code: DeadlineState,
    /// `closure_review_state_code`.
    pub closure_review_state_code: ClosureReviewState,
    /// `reminder_state_code`.
    pub reminder_state_code: ReminderState,
    /// `analysis_state_code`.
    pub analysis_state_code: AnalysisState,
    /// `source_state_code`.
    pub source_state_code: SourceState,
    /// `resolution_code`.
    pub resolution_code: Resolution,
    /// `confidence_bucket`.
    pub confidence_bucket: ConfidenceBucket,
    /// `source_ref_ids_by_role`, bounded at exactly 256. The role catalog
    /// itself is owned by ADR-006's evidence contract, so roles are opaque
    /// [`crate::ids::SourceRoleCode`] values here, not a fabricated enum.
    pub source_ref_ids_by_role: SourceRoleRefList,
    /// `operative_deadline_ref_id`; nullable.
    pub operative_deadline_ref_id: Option<OpaqueId>,
    /// `operative_deadline_value_or_range`; nullable.
    pub operative_deadline_value_or_range: Option<DeadlineValue>,
    /// `reminder_ref_ids`, bounded at exactly 16.
    pub reminder_ref_ids: BoundedIdList,
    /// `transition_ref_ids`, bounded at exactly 4096.
    pub transition_ref_ids: BoundedIdList,
    /// `policy_version`.
    pub policy_version: Version,
    /// `schema_version`.
    pub schema_version: Version,
    /// `model_label_code`.
    pub model_label_code: ModelLabelCode,
    /// `loop_version`.
    pub loop_version: Version,
    /// `last_evaluated_source_ref_id`; nullable.
    pub last_evaluated_source_ref_id: Option<OpaqueId>,
}

impl Loop {
    /// `collection_bounds.source_ref_ids_by_role`.
    pub const SOURCE_REF_IDS_BY_ROLE_BOUND: usize = 256;
    /// `collection_bounds.reminder_ref_ids`.
    pub const REMINDER_REF_IDS_BOUND: usize = 16;
    /// `collection_bounds.transition_ref_ids`.
    pub const TRANSITION_REF_IDS_BOUND: usize = 4096;

    /// Builds one loop record, enforcing full Cartesian legality
    /// ([`crate::legality::validate`]) over the four legality-relevant
    /// facets and every `collection_bounds` limit.
    ///
    /// # Errors
    ///
    /// Returns [`LoopError`] when the facet combination is illegal or a
    /// bounded collection overflows or contains a duplicate.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        loop_id: OpaqueId,
        account_ref: OpaqueId,
        origin_code: OriginCode,
        provenance_codes: ProvenanceSet,
        obligation_state_code: ObligationState,
        review_flag_codes: ReviewFlagSet,
        deadline_state_code: DeadlineState,
        closure_review_state_code: ClosureReviewState,
        reminder_state_code: ReminderState,
        analysis_state_code: AnalysisState,
        source_state_code: SourceState,
        resolution_code: Resolution,
        confidence_bucket: ConfidenceBucket,
        source_ref_ids_by_role: &[(crate::ids::SourceRoleCode, OpaqueId)],
        operative_deadline_ref_id: Option<OpaqueId>,
        operative_deadline_value_or_range: Option<DeadlineValue>,
        reminder_ref_ids: &[OpaqueId],
        transition_ref_ids: &[OpaqueId],
        policy_version: Version,
        schema_version: Version,
        model_label_code: ModelLabelCode,
        loop_version: Version,
        last_evaluated_source_ref_id: Option<OpaqueId>,
    ) -> Result<Self, LoopError> {
        legality::validate(LoopLegalityFacets {
            obligation_state: obligation_state_code,
            resolution: resolution_code,
            closure_review_state: closure_review_state_code,
            deadline_state: deadline_state_code,
        })
        .map_err(LoopError::IllegalFacets)?;

        if matches!(
            deadline_state_code,
            DeadlineState::Approaching | DeadlineState::Overdue
        ) && operative_deadline_ref_id.is_none()
            && operative_deadline_value_or_range.is_none()
        {
            return Err(LoopError::AgingWithoutOperativeBoundary);
        }

        let mut role_refs = SourceRoleRefList::with_capacity(Self::SOURCE_REF_IDS_BY_ROLE_BOUND);
        for (role, id) in source_ref_ids_by_role {
            match role_refs.push(*role, *id) {
                Ok(()) => {}
                Err(BoundedInsertError::CapacityExceeded) => {
                    return Err(LoopError::TooManySourceRefsByRole);
                }
                Err(BoundedInsertError::Duplicate) => {
                    return Err(LoopError::DuplicateSourceRefByRole);
                }
            }
        }

        let mut reminders = BoundedIdList::with_capacity(Self::REMINDER_REF_IDS_BOUND);
        for id in reminder_ref_ids {
            match reminders.push(*id) {
                Ok(()) => {}
                Err(BoundedInsertError::CapacityExceeded) => {
                    return Err(LoopError::TooManyReminderRefs);
                }
                Err(BoundedInsertError::Duplicate) => return Err(LoopError::DuplicateReminderRef),
            }
        }

        let mut transitions = BoundedIdList::with_capacity(Self::TRANSITION_REF_IDS_BOUND);
        for id in transition_ref_ids {
            match transitions.push(*id) {
                Ok(()) => {}
                Err(BoundedInsertError::CapacityExceeded) => {
                    return Err(LoopError::TooManyTransitionRefs);
                }
                Err(BoundedInsertError::Duplicate) => {
                    return Err(LoopError::DuplicateTransitionRef);
                }
            }
        }

        Ok(Self {
            loop_id,
            account_ref,
            origin_code,
            provenance_codes,
            obligation_state_code,
            review_flag_codes,
            deadline_state_code,
            closure_review_state_code,
            reminder_state_code,
            analysis_state_code,
            source_state_code,
            resolution_code,
            confidence_bucket,
            source_ref_ids_by_role: role_refs,
            operative_deadline_ref_id,
            operative_deadline_value_or_range,
            reminder_ref_ids: reminders,
            transition_ref_ids: transitions,
            policy_version,
            schema_version,
            model_label_code,
            loop_version,
            last_evaluated_source_ref_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{DeadlineEvidence, DeadlineEvidenceError, DeadlineValue, Loop, LoopError};
    use crate::deadline::UnixSeconds;
    use crate::facets::{
        AnalysisState, ClosureReviewState, ConfidenceBucket, DeadlineInterpretation,
        DeadlineSourceType, DeadlineState, ObligationState, OperativeSelection, OriginCode,
        ProvenanceSet, ReminderState, Resolution, ReviewFlagSet, SourceState,
    };
    use crate::ids::{ModelLabelCode, OpaqueId, SourceRoleCode, TimezoneId, Version};

    fn id(byte: u8) -> OpaqueId {
        OpaqueId::from_bytes([byte; 16])
    }

    fn version(value: u64) -> Version {
        Version::new(core::num::NonZeroU64::new(value).expect("nonzero test version"))
    }

    #[test]
    fn deadline_value_precision_always_matches_its_own_variant() {
        let cases = [
            (
                DeadlineValue::Instant(UnixSeconds(1)),
                crate::facets::DeadlinePrecision::Instant,
            ),
            (
                DeadlineValue::Date(UnixSeconds(1)),
                crate::facets::DeadlinePrecision::Date,
            ),
            (
                DeadlineValue::BusinessDay(UnixSeconds(1)),
                crate::facets::DeadlinePrecision::BusinessDay,
            ),
            (
                DeadlineValue::Week {
                    start: UnixSeconds(1),
                    end: UnixSeconds(2),
                },
                crate::facets::DeadlinePrecision::Week,
            ),
            (
                DeadlineValue::EventRelative(None),
                crate::facets::DeadlinePrecision::EventRelative,
            ),
            (DeadlineValue::Soft, crate::facets::DeadlinePrecision::Soft),
            (
                DeadlineValue::Unspecified,
                crate::facets::DeadlinePrecision::Unspecified,
            ),
        ];
        for (value, expected) in cases {
            assert_eq!(value.precision(), expected);
        }
    }

    #[test]
    fn deadline_evidence_requires_a_source_ref_except_for_user_supplied() {
        let missing_ref = DeadlineEvidence::new(
            id(1),
            id(2),
            None,
            DeadlineSourceType::Requested,
            DeadlineInterpretation::Resolved,
            OperativeSelection::Selected,
            DeadlineValue::Instant(UnixSeconds(10)),
            TimezoneId::new("Etc/UTC").expect("valid timezone id"),
        );
        assert_eq!(
            missing_ref,
            Err(DeadlineEvidenceError::SourceRefRequiredExceptUserSupplied)
        );

        let user_supplied_without_ref = DeadlineEvidence::new(
            id(1),
            id(2),
            None,
            DeadlineSourceType::UserSupplied,
            DeadlineInterpretation::Resolved,
            OperativeSelection::Selected,
            DeadlineValue::Instant(UnixSeconds(10)),
            TimezoneId::new("Etc/UTC").expect("valid timezone id"),
        );
        assert!(user_supplied_without_ref.is_ok());
    }

    #[test]
    fn deadline_evidence_rejects_an_invalid_week_range() {
        let backwards = DeadlineEvidence::new(
            id(1),
            id(2),
            Some(id(3)),
            DeadlineSourceType::Requested,
            DeadlineInterpretation::Resolved,
            OperativeSelection::Selected,
            DeadlineValue::Week {
                start: UnixSeconds(5),
                end: UnixSeconds(5),
            },
            TimezoneId::new("Etc/UTC").expect("valid timezone id"),
        );
        assert_eq!(backwards, Err(DeadlineEvidenceError::InvalidWeekRange));
    }

    #[test]
    fn deadline_evidence_derives_precision_from_the_value_never_from_a_separate_input() {
        let evidence = DeadlineEvidence::new(
            id(1),
            id(2),
            Some(id(3)),
            DeadlineSourceType::Requested,
            DeadlineInterpretation::Resolved,
            OperativeSelection::Selected,
            DeadlineValue::Week {
                start: UnixSeconds(1),
                end: UnixSeconds(2),
            },
            TimezoneId::new("Etc/UTC").expect("valid timezone id"),
        )
        .expect("valid week evidence");
        assert_eq!(
            evidence.precision_code,
            crate::facets::DeadlinePrecision::Week
        );
    }

    // Test-only fixture tuple; a named struct would add ceremony with no
    // reader benefit for a single private helper used by five tests below.
    #[allow(clippy::type_complexity)]
    fn base_loop_args() -> (
        OpaqueId,
        OpaqueId,
        OriginCode,
        ProvenanceSet,
        ObligationState,
        ReviewFlagSet,
        DeadlineState,
        ClosureReviewState,
        ReminderState,
        AnalysisState,
        SourceState,
        Resolution,
        ConfidenceBucket,
    ) {
        (
            id(1),
            id(2),
            OriginCode::EmailEvidence,
            ProvenanceSet::empty(),
            ObligationState::Open,
            ReviewFlagSet::empty(),
            DeadlineState::Unresolved,
            ClosureReviewState::None,
            ReminderState::None,
            AnalysisState::Current,
            SourceState::Available,
            Resolution::None,
            ConfidenceBucket::High,
        )
    }

    #[test]
    fn loop_new_rejects_illegal_facet_combinations() {
        let (
            loop_id,
            account_ref,
            origin,
            provenance,
            _obligation,
            flags,
            deadline,
            _closure,
            reminder,
            analysis,
            source,
            _resolution,
            confidence,
        ) = base_loop_args();
        let result = Loop::new(
            loop_id,
            account_ref,
            origin,
            provenance,
            ObligationState::Candidate,
            flags,
            deadline,
            ClosureReviewState::Possible,
            reminder,
            analysis,
            source,
            Resolution::None,
            confidence,
            &[],
            None,
            None,
            &[],
            &[],
            version(1),
            version(1),
            ModelLabelCode(0),
            version(1),
            None,
        );
        assert!(matches!(result, Err(LoopError::IllegalFacets(_))));
    }

    #[test]
    fn loop_new_rejects_aging_deadline_state_without_an_operative_boundary() {
        let (
            loop_id,
            account_ref,
            origin,
            provenance,
            obligation,
            flags,
            _deadline,
            closure,
            reminder,
            analysis,
            source,
            resolution,
            confidence,
        ) = base_loop_args();
        for aging in [DeadlineState::Approaching, DeadlineState::Overdue] {
            let result = Loop::new(
                loop_id,
                account_ref,
                origin,
                provenance,
                obligation,
                flags,
                aging,
                closure,
                reminder,
                analysis,
                source,
                resolution,
                confidence,
                &[],
                None,
                None,
                &[],
                &[],
                version(1),
                version(1),
                ModelLabelCode(0),
                version(1),
                None,
            );
            assert!(matches!(
                result,
                Err(LoopError::AgingWithoutOperativeBoundary)
            ));
        }
    }

    #[test]
    fn loop_new_enforces_the_reminder_ref_bound_of_16() {
        let (
            loop_id,
            account_ref,
            origin,
            provenance,
            obligation,
            flags,
            deadline,
            closure,
            reminder,
            analysis,
            source,
            resolution,
            confidence,
        ) = base_loop_args();
        let too_many: Vec<OpaqueId> = (0..17u8).map(id).collect();
        let result = Loop::new(
            loop_id,
            account_ref,
            origin,
            provenance,
            obligation,
            flags,
            deadline,
            closure,
            reminder,
            analysis,
            source,
            resolution,
            confidence,
            &[],
            None,
            None,
            &too_many,
            &[],
            version(1),
            version(1),
            ModelLabelCode(0),
            version(1),
            None,
        );
        assert_eq!(result, Err(LoopError::TooManyReminderRefs));
    }

    #[test]
    fn loop_new_enforces_the_source_ref_by_role_bound_of_256() {
        let (
            loop_id,
            account_ref,
            origin,
            provenance,
            obligation,
            flags,
            deadline,
            closure,
            reminder,
            analysis,
            source,
            resolution,
            confidence,
        ) = base_loop_args();
        let too_many: Vec<(SourceRoleCode, OpaqueId)> =
            (0..=u8::MAX).map(|n| (SourceRoleCode(1), id(n))).collect();
        // 256 distinct ids (0..=255) all under one role: exactly at the
        // bound, so this must succeed...
        let at_bound = Loop::new(
            loop_id,
            account_ref,
            origin,
            provenance,
            obligation,
            flags,
            deadline,
            closure,
            reminder,
            analysis,
            source,
            resolution,
            confidence,
            &too_many,
            None,
            None,
            &[],
            &[],
            version(1),
            version(1),
            ModelLabelCode(0),
            version(1),
            None,
        );
        assert!(at_bound.is_ok());

        // ...and one more, with a distinct role so it is not a duplicate,
        // must overflow the bound.
        let mut over_bound = too_many;
        over_bound.push((SourceRoleCode(2), id(0)));
        let result = Loop::new(
            loop_id,
            account_ref,
            origin,
            provenance,
            obligation,
            flags,
            deadline,
            closure,
            reminder,
            analysis,
            source,
            resolution,
            confidence,
            &over_bound,
            None,
            None,
            &[],
            &[],
            version(1),
            version(1),
            ModelLabelCode(0),
            version(1),
            None,
        );
        assert_eq!(result, Err(LoopError::TooManySourceRefsByRole));
    }

    #[test]
    fn loop_new_accepts_a_legal_minimal_loop() {
        let (
            loop_id,
            account_ref,
            origin,
            provenance,
            obligation,
            flags,
            deadline,
            closure,
            reminder,
            analysis,
            source,
            resolution,
            confidence,
        ) = base_loop_args();
        let result = Loop::new(
            loop_id,
            account_ref,
            origin,
            provenance,
            obligation,
            flags,
            deadline,
            closure,
            reminder,
            analysis,
            source,
            resolution,
            confidence,
            &[],
            None,
            None,
            &[],
            &[],
            version(1),
            version(1),
            ModelLabelCode(0),
            version(1),
            None,
        );
        assert!(result.is_ok());
    }
}
