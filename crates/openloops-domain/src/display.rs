//! The single primary display label and its strict precedence.
//!
//! Implements `docs/product-spec.md` 5.1's primary display precedence table
//! and `facet_catalogs.primary_label_precedence`. Secondary facets (evidence,
//! reminder, analysis, provenance, deadline chips) remain visible regardless
//! of which primary label wins; this module decides only which one label is
//! primary.

use crate::facets::{
    AnalysisState, ClosureReviewState, DeadlineState, ObligationState, PrimaryLabel, ReminderState,
    ReviewFlagSet, SourceState,
};

/// The facets [`primary_label`] needs to resolve one loop's primary label.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayFacets {
    /// `obligation_state_code`.
    pub obligation_state: ObligationState,
    /// `closure_review_state_code`.
    pub closure_review_state: ClosureReviewState,
    /// `deadline_state_code`.
    pub deadline_state: DeadlineState,
    /// `review_flag_codes`.
    pub review_flags: ReviewFlagSet,
    /// `analysis_state_code`.
    pub analysis_state: AnalysisState,
    /// `source_state_code`.
    pub source_state: SourceState,
    /// `reminder_state_code`.
    pub reminder_state: ReminderState,
}

const fn analysis_requires_action(analysis: AnalysisState) -> bool {
    match analysis {
        AnalysisState::Quarantined => true,
        AnalysisState::Current
        | AnalysisState::Queued
        | AnalysisState::Unavailable
        | AnalysisState::Stale => false,
    }
}

const fn source_requires_action(source: SourceState) -> bool {
    match source {
        SourceState::Changed | SourceState::Unavailable | SourceState::PartiallyAvailable => true,
        SourceState::Available => false,
    }
}

const fn reminder_requires_action(reminder: ReminderState) -> bool {
    match reminder {
        ReminderState::Conflict
        | ReminderState::AmbiguousWrite
        | ReminderState::Missing
        | ReminderState::Changed => true,
        ReminderState::None
        | ReminderState::Proposed
        | ReminderState::PendingWrite
        | ReminderState::Linked
        | ReminderState::CompletedNeedsEvidence => false,
    }
}

/// Resolves the single primary display label for `facets`, exactly
/// reproducing the `docs/product-spec.md` 5.1 precedence table (index 0 of
/// [`PrimaryLabel::ALL`] is checked first).
#[must_use]
pub const fn primary_label(facets: DisplayFacets) -> PrimaryLabel {
    if matches!(facets.obligation_state, ObligationState::Terminal) {
        return PrimaryLabel::TerminalResolution;
    }
    if matches!(facets.closure_review_state, ClosureReviewState::Possible) {
        return PrimaryLabel::PossibleClosure;
    }
    let needs_review = !facets.review_flags.is_empty()
        || analysis_requires_action(facets.analysis_state)
        || source_requires_action(facets.source_state)
        || reminder_requires_action(facets.reminder_state);
    if needs_review {
        return PrimaryLabel::NeedsReview;
    }
    match facets.deadline_state {
        DeadlineState::Overdue => return PrimaryLabel::Overdue,
        DeadlineState::Approaching => return PrimaryLabel::ApproachingDeadline,
        DeadlineState::Unresolved => return PrimaryLabel::NeedsDeadline,
        DeadlineState::UndatedConfirmed | DeadlineState::Scheduled => {}
    }
    if matches!(facets.obligation_state, ObligationState::Candidate) {
        return PrimaryLabel::Candidate;
    }
    PrimaryLabel::Active
}

#[cfg(test)]
mod tests {
    use super::{DisplayFacets, primary_label};
    use crate::facets::{
        AnalysisState, ClosureReviewState, DeadlineState, ObligationState, PrimaryLabel,
        ReminderState, ReviewFlagCode, ReviewFlagSet, SourceState,
    };

    fn base() -> DisplayFacets {
        DisplayFacets {
            obligation_state: ObligationState::Open,
            closure_review_state: ClosureReviewState::None,
            deadline_state: DeadlineState::Scheduled,
            review_flags: ReviewFlagSet::empty(),
            analysis_state: AnalysisState::Current,
            source_state: SourceState::Available,
            reminder_state: ReminderState::None,
        }
    }

    #[test]
    fn terminal_outranks_every_other_label() {
        let facets = DisplayFacets {
            obligation_state: ObligationState::Terminal,
            closure_review_state: ClosureReviewState::Possible,
            deadline_state: DeadlineState::Overdue,
            review_flags: ReviewFlagSet::empty().with(ReviewFlagCode::NeedsReview),
            ..base()
        };
        assert_eq!(primary_label(facets), PrimaryLabel::TerminalResolution);
    }

    #[test]
    fn possible_closure_outranks_needs_review_and_deadline_labels() {
        let facets = DisplayFacets {
            closure_review_state: ClosureReviewState::Possible,
            deadline_state: DeadlineState::Overdue,
            review_flags: ReviewFlagSet::empty().with(ReviewFlagCode::NeedsReview),
            ..base()
        };
        assert_eq!(primary_label(facets), PrimaryLabel::PossibleClosure);
    }

    #[test]
    fn needs_review_outranks_overdue_and_approaching() {
        let flagged = DisplayFacets {
            deadline_state: DeadlineState::Overdue,
            review_flags: ReviewFlagSet::empty().with(ReviewFlagCode::NeedsReview),
            ..base()
        };
        assert_eq!(primary_label(flagged), PrimaryLabel::NeedsReview);

        let quarantined = DisplayFacets {
            deadline_state: DeadlineState::Approaching,
            analysis_state: AnalysisState::Quarantined,
            ..base()
        };
        assert_eq!(primary_label(quarantined), PrimaryLabel::NeedsReview);

        let source_changed = DisplayFacets {
            source_state: SourceState::Changed,
            ..base()
        };
        assert_eq!(primary_label(source_changed), PrimaryLabel::NeedsReview);

        let reminder_conflict = DisplayFacets {
            reminder_state: ReminderState::Conflict,
            ..base()
        };
        assert_eq!(primary_label(reminder_conflict), PrimaryLabel::NeedsReview);
    }

    #[test]
    fn overdue_outranks_approaching_and_needs_deadline() {
        let facets = DisplayFacets {
            deadline_state: DeadlineState::Overdue,
            ..base()
        };
        assert_eq!(primary_label(facets), PrimaryLabel::Overdue);
    }

    #[test]
    fn approaching_outranks_needs_deadline() {
        let facets = DisplayFacets {
            deadline_state: DeadlineState::Approaching,
            ..base()
        };
        assert_eq!(primary_label(facets), PrimaryLabel::ApproachingDeadline);
    }

    #[test]
    fn needs_deadline_outranks_candidate_and_active() {
        let facets = DisplayFacets {
            deadline_state: DeadlineState::Unresolved,
            ..base()
        };
        assert_eq!(primary_label(facets), PrimaryLabel::NeedsDeadline);
    }

    #[test]
    fn candidate_shows_when_nothing_higher_applies() {
        let facets = DisplayFacets {
            obligation_state: ObligationState::Candidate,
            deadline_state: DeadlineState::Scheduled,
            ..base()
        };
        assert_eq!(primary_label(facets), PrimaryLabel::Candidate);
    }

    #[test]
    fn active_is_the_final_fallback() {
        let facets = base();
        assert_eq!(primary_label(facets), PrimaryLabel::Active);
    }
}
