//! Closed facet catalogs.
//!
//! Every enum in this module reproduces one
//! `contracts/domain/policy-state-boundary.json` `facet_catalogs` entry
//! exactly: the same values, in the same order, with no extra variant and no
//! catch-all match arm anywhere in this crate. `ALL` gives tests and the
//! exhaustive legality enumeration a compile-time-checked catalog to iterate
//! that can never silently drop a variant added later, because every match
//! over these enums must be exhaustive under `#![forbid(unsafe_code)]` and
//! `clippy::pedantic`.

/// `facet_catalogs.origin_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum OriginCode {
    /// Detected from mail evidence.
    EmailEvidence,
    /// Detected from a calendar invitation (unavailable pending G-CAL/G-MAIL).
    CalendarInvitation,
    /// Created from a user-authored Microsoft artifact (section 5.8).
    UserAuthoredMicrosoftArtifact,
}

impl OriginCode {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 3] = [
        Self::EmailEvidence,
        Self::CalendarInvitation,
        Self::UserAuthoredMicrosoftArtifact,
    ];
}

/// `facet_catalogs.provenance_codes`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ProvenanceCode {
    /// A direct incoming request/question.
    Requested,
    /// An explicit acknowledgement of an existing request.
    Acknowledged,
    /// An explicit outgoing promise.
    Promised,
    /// A deterministic explicit attribution.
    Attributed,
    /// A model/system inference, never sufficient alone to promote.
    Inferred,
    /// Ambiguous attachment/interpretation.
    Ambiguous,
}

impl ProvenanceCode {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 6] = [
        Self::Requested,
        Self::Acknowledged,
        Self::Promised,
        Self::Attributed,
        Self::Inferred,
        Self::Ambiguous,
    ];

    const fn bit(self) -> u8 {
        match self {
            Self::Requested => 0,
            Self::Acknowledged => 1,
            Self::Promised => 2,
            Self::Attributed => 3,
            Self::Inferred => 4,
            Self::Ambiguous => 5,
        }
    }
}

/// A closed, duplicate-free set of [`ProvenanceCode`] values.
///
/// The catalog has exactly six values, so a bitset of six flags is an exact
/// structural bound on `record_contracts[loop].collection_bounds.provenance_codes`
/// (6): the type cannot represent more entries than the catalog has values,
/// and it cannot represent a duplicate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct ProvenanceSet(u8);

impl ProvenanceSet {
    /// The empty set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Returns a copy of this set with `code` inserted.
    #[must_use]
    pub const fn with(self, code: ProvenanceCode) -> Self {
        Self(self.0 | (1 << code.bit()))
    }

    /// Returns whether `code` is a member.
    #[must_use]
    pub const fn contains(self, code: ProvenanceCode) -> bool {
        (self.0 & (1 << code.bit())) != 0
    }

    /// Returns whether the set has no members.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Iterates members in contract order.
    pub fn iter(self) -> impl Iterator<Item = ProvenanceCode> {
        ProvenanceCode::ALL
            .into_iter()
            .filter(move |code| self.contains(*code))
    }
}

/// `facet_catalogs.review_flag_codes`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ReviewFlagCode {
    /// Generic review needed.
    NeedsReview,
    /// Discovered during history/backfill scanning.
    HistoricalBackfill,
    /// Identity resolution is ambiguous.
    IdentityAmbiguous,
    /// Loop association is ambiguous.
    AssociationAmbiguous,
    /// Quote-newness is ambiguous.
    QuoteAmbiguous,
    /// Deadline attachment/interpretation is ambiguous.
    DeadlineAmbiguous,
    /// Delegation/responsibility transfer is ambiguous.
    DelegationAmbiguous,
}

impl ReviewFlagCode {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 7] = [
        Self::NeedsReview,
        Self::HistoricalBackfill,
        Self::IdentityAmbiguous,
        Self::AssociationAmbiguous,
        Self::QuoteAmbiguous,
        Self::DeadlineAmbiguous,
        Self::DelegationAmbiguous,
    ];

    const fn bit(self) -> u8 {
        match self {
            Self::NeedsReview => 0,
            Self::HistoricalBackfill => 1,
            Self::IdentityAmbiguous => 2,
            Self::AssociationAmbiguous => 3,
            Self::QuoteAmbiguous => 4,
            Self::DeadlineAmbiguous => 5,
            Self::DelegationAmbiguous => 6,
        }
    }
}

/// A closed, duplicate-free set of [`ReviewFlagCode`] values.
///
/// Structurally bounds `record_contracts[loop].collection_bounds.review_flag_codes` (7)
/// the same way [`ProvenanceSet`] bounds provenance.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct ReviewFlagSet(u8);

impl ReviewFlagSet {
    /// The empty set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Returns a copy of this set with `code` inserted.
    #[must_use]
    pub const fn with(self, code: ReviewFlagCode) -> Self {
        Self(self.0 | (1 << code.bit()))
    }

    /// Returns whether `code` is a member.
    #[must_use]
    pub const fn contains(self, code: ReviewFlagCode) -> bool {
        (self.0 & (1 << code.bit())) != 0
    }

    /// Returns whether the set has no members.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Iterates members in contract order.
    pub fn iter(self) -> impl Iterator<Item = ReviewFlagCode> {
        ReviewFlagCode::ALL
            .into_iter()
            .filter(move |code| self.contains(*code))
    }
}

/// `facet_catalogs.obligation_state_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ObligationState {
    /// Not yet established; may be promoted or dismissed.
    Candidate,
    /// An established, active expectation.
    Open,
    /// Closed with exactly one non-none resolution.
    Terminal,
}

impl ObligationState {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 3] = [Self::Candidate, Self::Open, Self::Terminal];
}

/// `facet_catalogs.deadline_state_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum DeadlineState {
    /// No operative deadline yet; awaiting prompt/evidence.
    Unresolved,
    /// A durable explicit "no deadline" user choice.
    UndatedConfirmed,
    /// A resolved operational boundary exists but neither lead nor boundary
    /// has passed.
    Scheduled,
    /// Within the configured lead of the operational boundary.
    Approaching,
    /// Past the operational boundary.
    Overdue,
}

impl DeadlineState {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 5] = [
        Self::Unresolved,
        Self::UndatedConfirmed,
        Self::Scheduled,
        Self::Approaching,
        Self::Overdue,
    ];
}

/// `facet_catalogs.closure_review_state_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ClosureReviewState {
    /// No current closure hypothesis or request.
    None,
    /// An unsuppressed current closure hypothesis exists.
    Possible,
    /// The current hypothesis was explicitly suppressed by the user.
    KeptOpen,
    /// An evidence request is pending.
    EvidenceRequested,
}

impl ClosureReviewState {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 4] = [
        Self::None,
        Self::Possible,
        Self::KeptOpen,
        Self::EvidenceRequested,
    ];
}

/// `facet_catalogs.reminder_state_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ReminderState {
    /// No reminder artifact exists or is proposed.
    None,
    /// A reminder is proposed but not yet confirmed.
    Proposed,
    /// A confirmed reminder write is in flight.
    PendingWrite,
    /// A reminder artifact is linked and current.
    Linked,
    /// The linked artifact changed outside expectation.
    Changed,
    /// The linked artifact was completed directly; obligation is unaffected.
    CompletedNeedsEvidence,
    /// The linked artifact is missing (e.g. deleted directly).
    Missing,
    /// A stale or ambiguous write left the artifact in conflict.
    Conflict,
    /// A write attempt returned an ambiguous outcome.
    AmbiguousWrite,
}

impl ReminderState {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 9] = [
        Self::None,
        Self::Proposed,
        Self::PendingWrite,
        Self::Linked,
        Self::Changed,
        Self::CompletedNeedsEvidence,
        Self::Missing,
        Self::Conflict,
        Self::AmbiguousWrite,
    ];
}

/// `facet_catalogs.analysis_state_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum AnalysisState {
    /// Analysis reflects the latest observed evidence.
    Current,
    /// Analysis is queued but not yet run.
    Queued,
    /// Analysis could not run.
    Unavailable,
    /// Analysis is quarantined pending a re-fetchable locator and retry/dismiss.
    Quarantined,
    /// Analysis reflects evidence that is no longer current.
    Stale,
}

impl AnalysisState {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 5] = [
        Self::Current,
        Self::Queued,
        Self::Unavailable,
        Self::Quarantined,
        Self::Stale,
    ];
}

/// `facet_catalogs.source_state_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum SourceState {
    /// The source resolves and matches its anchors.
    Available,
    /// The source resolves but only some evidence is available.
    PartiallyAvailable,
    /// The source cannot be resolved.
    Unavailable,
    /// The source resolves but its anchors differ from stored evidence.
    Changed,
}

impl SourceState {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 4] = [
        Self::Available,
        Self::PartiallyAvailable,
        Self::Unavailable,
        Self::Changed,
    ];
}

/// `facet_catalogs.resolution_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Resolution {
    /// No resolution; only legal for candidate/open loops.
    None,
    /// Fulfilled with validated closure evidence.
    Closed,
    /// Declined with explicit decline evidence or manual confirmation.
    Declined,
    /// Responsibility fully transferred.
    Delegated,
    /// Withdrawn, superseded, or no longer relevant.
    Moot,
    /// Dismissed with a typed correction reason.
    Dismissed,
    /// Manually confirmed complete outside email.
    CompletedOutsideEmail,
}

impl Resolution {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Closed,
        Self::Declined,
        Self::Delegated,
        Self::Moot,
        Self::Dismissed,
        Self::CompletedOutsideEmail,
    ];

    /// Returns whether this resolution is anything other than
    /// [`Resolution::None`].
    #[must_use]
    pub const fn is_some(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// `facet_catalogs.confidence_bucket`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ConfidenceBucket {
    /// Low calibrated confidence.
    Low,
    /// Medium calibrated confidence.
    Medium,
    /// High calibrated confidence.
    High,
}

impl ConfidenceBucket {
    /// Every catalog value, in ascending order (the contract lists
    /// `high, medium, low`; this crate orders them ascending so
    /// `ConfidenceBucket::Medium >= ConfidenceBucket::Low` reads naturally at
    /// call sites that compare a "medium or high" threshold).
    pub const ALL: [Self; 3] = [Self::Low, Self::Medium, Self::High];

    /// Returns whether this bucket is medium or high, the establishment
    /// threshold used throughout `establishment_policy`.
    #[must_use]
    pub const fn is_medium_or_high(self) -> bool {
        matches!(self, Self::Medium | Self::High)
    }
}

/// `facet_catalogs.deadline_source_type_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum DeadlineSourceType {
    /// A direct request's deadline.
    Requested,
    /// An explicit outgoing promise's deadline.
    Promised,
    /// A model-inferred deadline (never operative alone).
    Inferred,
    /// A deadline the user typed directly.
    UserSupplied,
}

impl DeadlineSourceType {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 4] = [
        Self::Requested,
        Self::Promised,
        Self::Inferred,
        Self::UserSupplied,
    ];
}

/// `facet_catalogs.deadline_precision_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum DeadlinePrecision {
    /// An exact instant.
    Instant,
    /// A calendar date without a time.
    Date,
    /// The end of a resolved business day.
    BusinessDay,
    /// A week/range.
    Week,
    /// Relative to an event that may or may not be uniquely correlated.
    EventRelative,
    /// Soft urgency ("ASAP") with no aging boundary.
    Soft,
    /// No precision was supplied.
    Unspecified,
}

impl DeadlinePrecision {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 7] = [
        Self::Instant,
        Self::Date,
        Self::BusinessDay,
        Self::Week,
        Self::EventRelative,
        Self::Soft,
        Self::Unspecified,
    ];
}

/// `facet_catalogs.deadline_interpretation_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum DeadlineInterpretation {
    /// A single unambiguous meaning was resolved.
    Resolved,
    /// More than one plausible interpretation exists.
    Ambiguous,
    /// The interpretation requires explicit user input.
    NeedsUserInput,
}

impl DeadlineInterpretation {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 3] = [Self::Resolved, Self::Ambiguous, Self::NeedsUserInput];
}

/// `facet_catalogs.operative_selection_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum OperativeSelection {
    /// Not currently the operative deadline for its loop.
    NotSelected,
    /// Currently the operative deadline.
    Selected,
    /// Previously operative but superseded by causally later evidence.
    Superseded,
}

impl OperativeSelection {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 3] = [Self::NotSelected, Self::Selected, Self::Superseded];
}

/// `facet_catalogs.actor_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ActorCode {
    /// An automated system/model-derived transition.
    System,
    /// An explicit user action.
    User,
}

impl ActorCode {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 2] = [Self::System, Self::User];
}

/// `facet_catalogs.delegation_code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum DelegationCode {
    /// Full responsibility transferred; the only variant that yields
    /// `resolution=delegated`.
    Transferred,
    /// Responsibility is shared; the loop stays open.
    Shared,
    /// Another party is assisting; the loop stays open.
    Assisted,
    /// The user retains responsibility; the loop stays open.
    Retained,
    /// Responsibility transfer is unclear; the loop stays open.
    Unclear,
}

impl DelegationCode {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 5] = [
        Self::Transferred,
        Self::Shared,
        Self::Assisted,
        Self::Retained,
        Self::Unclear,
    ];

    /// Returns whether this delegation code alone yields
    /// `resolution=delegated` (only full `Transferred` responsibility does;
    /// OL-DELEG-001).
    #[must_use]
    pub const fn yields_delegated_resolution(self) -> bool {
        matches!(self, Self::Transferred)
    }
}

/// `facet_catalogs.primary_label_precedence`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PrimaryLabel {
    /// Precedence 1: a terminal resolution label.
    TerminalResolution,
    /// Precedence 2: possible closure.
    PossibleClosure,
    /// Precedence 3: needs review.
    NeedsReview,
    /// Precedence 4: overdue.
    Overdue,
    /// Precedence 5: approaching deadline.
    ApproachingDeadline,
    /// Precedence 6: needs deadline.
    NeedsDeadline,
    /// Precedence 7: candidate.
    Candidate,
    /// Precedence 8: active.
    Active,
}

impl PrimaryLabel {
    /// Every catalog value, in strict display precedence order (index 0 is
    /// highest precedence).
    pub const ALL: [Self; 8] = [
        Self::TerminalResolution,
        Self::PossibleClosure,
        Self::NeedsReview,
        Self::Overdue,
        Self::ApproachingDeadline,
        Self::NeedsDeadline,
        Self::Candidate,
        Self::Active,
    ];
}

#[cfg(test)]
mod tests {
    use super::{
        ActorCode, AnalysisState, ClosureReviewState, ConfidenceBucket, DeadlineInterpretation,
        DeadlinePrecision, DeadlineSourceType, DeadlineState, DelegationCode, ObligationState,
        OperativeSelection, OriginCode, PrimaryLabel, ProvenanceCode, ProvenanceSet, Resolution,
        ReviewFlagCode, ReviewFlagSet, SourceState,
    };

    #[test]
    fn every_catalog_matches_the_contracts_exact_length() {
        assert_eq!(OriginCode::ALL.len(), 3);
        assert_eq!(ProvenanceCode::ALL.len(), 6);
        assert_eq!(ObligationState::ALL.len(), 3);
        assert_eq!(ReviewFlagCode::ALL.len(), 7);
        assert_eq!(DeadlineState::ALL.len(), 5);
        assert_eq!(ClosureReviewState::ALL.len(), 4);
        assert_eq!(super::ReminderState::ALL.len(), 9);
        assert_eq!(AnalysisState::ALL.len(), 5);
        assert_eq!(SourceState::ALL.len(), 4);
        assert_eq!(Resolution::ALL.len(), 7);
        assert_eq!(ConfidenceBucket::ALL.len(), 3);
        assert_eq!(DeadlineSourceType::ALL.len(), 4);
        assert_eq!(DeadlinePrecision::ALL.len(), 7);
        assert_eq!(DeadlineInterpretation::ALL.len(), 3);
        assert_eq!(OperativeSelection::ALL.len(), 3);
        assert_eq!(ActorCode::ALL.len(), 2);
        assert_eq!(DelegationCode::ALL.len(), 5);
        assert_eq!(PrimaryLabel::ALL.len(), 8);
    }

    #[test]
    fn provenance_set_has_no_duplicate_and_bounds_at_the_catalog_size() {
        let mut set = ProvenanceSet::empty();
        for code in ProvenanceCode::ALL {
            set = set.with(code);
        }
        assert_eq!(set.iter().count(), ProvenanceCode::ALL.len());
        // Re-inserting is a no-op, not growth: the set can never exceed the
        // catalog's six values, matching collection_bounds.provenance_codes.
        set = set.with(ProvenanceCode::Requested);
        assert_eq!(set.iter().count(), ProvenanceCode::ALL.len());
    }

    #[test]
    fn review_flag_set_has_no_duplicate_and_bounds_at_the_catalog_size() {
        let mut set = ReviewFlagSet::empty();
        for code in ReviewFlagCode::ALL {
            set = set.with(code);
        }
        assert_eq!(set.iter().count(), ReviewFlagCode::ALL.len());
    }

    #[test]
    fn only_transferred_delegation_yields_delegated_resolution() {
        for code in DelegationCode::ALL {
            assert_eq!(
                code.yields_delegated_resolution(),
                matches!(code, DelegationCode::Transferred)
            );
        }
    }

    #[test]
    fn resolution_none_is_exactly_resolution_none() {
        for resolution in Resolution::ALL {
            assert_eq!(resolution.is_some(), resolution != Resolution::None);
        }
    }

    #[test]
    fn confidence_medium_or_high_excludes_only_low() {
        for bucket in ConfidenceBucket::ALL {
            assert_eq!(bucket.is_medium_or_high(), bucket != ConfidenceBucket::Low);
        }
    }
}
