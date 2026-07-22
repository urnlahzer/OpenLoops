//! `remote_marker_protocol.enumeration_rule`/`outcomes`/`recreate_rule`: the
//! bounded complete-enumeration reconciliation classification for an
//! ambiguous To Do write, plus the missing/removed/unsupported-marker and
//! explicit-recreate paths.
//!
//! Three closed, typed shapes, one per contract clause:
//!
//! * [`ReconciliationOutcome`] — `enumeration_rule`/`outcomes`: what a
//!   *completed* bounded enumeration over [`MarkerEnumerationSource`]
//!   classifies to. `CompleteMany` and `Incomplete` are distinct variants
//!   from `CompleteZero`/`CompleteOne` at the type level, so a caller cannot
//!   accidentally fold "many" or "incomplete" into "zero" the way an
//!   untyped count comparison could.
//! * [`MarkerCondition`]/[`classify_marker_condition`] — `outcomes.
//!   missing_removed_or_unsupported`: "user-assisted select abandon or
//!   explicit recreate; no automatic retry." [`MarkerConditionDisposition`]
//!   has exactly one variant, deliberately: there is no automatic-retry
//!   variant to reach for any of the three conditions.
//! * [`confirm_recreate`] — `recreate_rule`: "explicit user confirmation
//!   with duplicate warning increments `recreate_generation` and produces a
//!   new operation identity." Rejects (returns `None`) any attempt to reuse
//!   the prior operation identity, so "recreate" can never silently degrade
//!   into "retry the same operation."

/// One page of marker-enumeration results
/// (`remote_marker_protocol.enumeration_rule`: "bounded complete enumeration
/// of the exact account adapter and collection").
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarkerEnumerationPage {
    /// Opaque markers observed on this page, exactly as returned through the
    /// one supported opaque field — never a title/body/due value.
    pub markers: Vec<String>,
    /// Opaque continuation bytes for the next page; `None` marks the
    /// terminal page.
    pub next_cursor: Option<Vec<u8>>,
}

/// A page fetch failure. Per `enumeration_rule`: `"incomplete paging is
/// never zero"` — any error here classifies the whole enumeration as
/// [`ReconciliationOutcome::Incomplete`], never `CompleteZero`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnumerationError {
    Transport,
    Unsupported,
}

/// The injected enumeration source (`todo_boundary.reconciliation_baseline`:
/// "ordinary bounded complete list and task enumeration"). No implementation
/// ships in this crate; every test supplies a synthetic double.
pub trait MarkerEnumerationSource {
    /// Fetches the next page after `cursor` (`None` for the first page).
    ///
    /// # Errors
    ///
    /// Returns [`EnumerationError`] on any transport or unsupported-endpoint
    /// failure.
    fn next_page(
        &mut self,
        cursor: Option<&[u8]>,
    ) -> Result<MarkerEnumerationPage, EnumerationError>;
}

/// The bound on pages consulted before declaring the enumeration itself
/// incomplete rather than looping forever against a misbehaving or
/// adversarial paging sequence.
pub const MAX_ENUMERATION_PAGES: u32 = 64;

/// `remote_marker_protocol.outcomes`: the closed, bounded-enumeration
/// classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReconciliationOutcome {
    /// `outcomes.complete_zero`: "safe bounded same-marker retry only when
    /// marker support remains proven and authority is unchanged." This type
    /// only classifies the enumeration result; the retry-eligibility
    /// decision itself belongs to the caller (via
    /// `openloops_application::ledger`), not this module.
    CompleteZero,
    /// `outcomes.complete_one`: "link the single verified marker match."
    CompleteOne { matched_marker: String },
    /// `outcomes.complete_many`: "user-assisted duplicate selection; no
    /// automatic linkage or retry."
    CompleteMany { matched_markers: Vec<String> },
    /// `enumeration_rule.incomplete_classification`-equivalent for markers:
    /// "incomplete paging is never zero" — surfaced whenever the page cap is
    /// reached or a page fetch fails before a terminal page.
    Incomplete,
}

/// Runs one bounded, complete enumeration against `source`, matching every
/// observed marker equal to `target_marker`, and classifies the result per
/// [`ReconciliationOutcome`]. Never treats a page-cap or fetch failure as
/// zero matches.
#[must_use]
pub fn enumerate_and_classify(
    source: &mut dyn MarkerEnumerationSource,
    target_marker: &str,
) -> ReconciliationOutcome {
    let mut cursor: Option<Vec<u8>> = None;
    let mut matches: Vec<String> = Vec::new();
    for _ in 0..MAX_ENUMERATION_PAGES {
        let Ok(page) = source.next_page(cursor.as_deref()) else {
            return ReconciliationOutcome::Incomplete;
        };
        matches.extend(
            page.markers
                .iter()
                .filter(|marker| marker.as_str() == target_marker)
                .cloned(),
        );
        let Some(next_cursor) = page.next_cursor else {
            return match matches.len() {
                0 => ReconciliationOutcome::CompleteZero,
                1 => ReconciliationOutcome::CompleteOne {
                    matched_marker: matches.swap_remove(0),
                },
                _ => ReconciliationOutcome::CompleteMany {
                    matched_markers: matches,
                },
            };
        };
        cursor = Some(next_cursor);
    }
    // The page cap was reached without a terminal page: incomplete, never
    // zero, per `enumeration_rule`.
    ReconciliationOutcome::Incomplete
}

/// `outcomes.missing_removed_or_unsupported`'s three closed conditions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarkerCondition {
    Missing,
    Removed,
    Unsupported,
}

/// The one closed disposition every [`MarkerCondition`] maps to:
/// `"user-assisted select abandon or explicit recreate; no automatic
/// retry."` Deliberately a single-variant enum — there is no
/// automatic-retry value to reach for, for any condition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarkerConditionDisposition {
    UserAssisted,
}

/// Classifies a missing/removed/unsupported marker condition. Every input
/// maps to the same [`MarkerConditionDisposition::UserAssisted`] — see that
/// type's doc comment for why that is the point, not an oversight.
#[must_use]
pub const fn classify_marker_condition(_condition: MarkerCondition) -> MarkerConditionDisposition {
    MarkerConditionDisposition::UserAssisted
}

/// `recreate_rule`'s prerequisite: the `recreate_generation` this recreate is
/// building on, plus the operation identity it must not reuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecreateConfirmation {
    pub previous_recreate_generation: u32,
    pub previous_operation_id: [u8; 16],
}

/// The result of a successful [`confirm_recreate`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecreateResult {
    pub recreate_generation: u32,
}

/// `recreate_rule`: "explicit user confirmation with duplicate warning
/// increments `recreate_generation` and produces a new operation identity."
///
/// Returns `None` — rejecting the recreate — whenever `new_operation_id`
/// equals `confirmation.previous_operation_id`: recreate must never reuse
/// the old operation identity, even if every other input is otherwise
/// well-formed.
#[must_use]
pub fn confirm_recreate(
    confirmation: RecreateConfirmation,
    new_operation_id: [u8; 16],
) -> Option<RecreateResult> {
    if new_operation_id == confirmation.previous_operation_id {
        return None;
    }
    Some(RecreateResult {
        recreate_generation: confirmation.previous_recreate_generation + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        EnumerationError, MarkerCondition, MarkerConditionDisposition, MarkerEnumerationPage,
        MarkerEnumerationSource, ReconciliationOutcome, RecreateConfirmation,
        classify_marker_condition, confirm_recreate, enumerate_and_classify,
    };

    struct FixedPages(Vec<Result<MarkerEnumerationPage, EnumerationError>>);

    impl MarkerEnumerationSource for FixedPages {
        fn next_page(
            &mut self,
            _cursor: Option<&[u8]>,
        ) -> Result<MarkerEnumerationPage, EnumerationError> {
            assert!(
                !self.0.is_empty(),
                "test source exhausted without a terminal page"
            );
            self.0.remove(0)
        }
    }

    fn page(markers: &[&str], next_cursor: Option<Vec<u8>>) -> MarkerEnumerationPage {
        MarkerEnumerationPage {
            markers: markers.iter().map(|m| (*m).to_string()).collect(),
            next_cursor,
        }
    }

    #[test]
    fn complete_enumeration_with_no_match_classifies_zero() {
        let mut source = FixedPages(vec![Ok(page(&["other-marker"], None))]);
        assert_eq!(
            enumerate_and_classify(&mut source, "target"),
            ReconciliationOutcome::CompleteZero
        );
    }

    #[test]
    fn complete_enumeration_with_one_match_classifies_one() {
        let mut source = FixedPages(vec![Ok(page(&["other", "target"], None))]);
        assert_eq!(
            enumerate_and_classify(&mut source, "target"),
            ReconciliationOutcome::CompleteOne {
                matched_marker: "target".to_string()
            }
        );
    }

    #[test]
    fn complete_enumeration_across_pages_accumulates_matches() {
        let mut source = FixedPages(vec![
            Ok(page(&["target"], Some(vec![1]))),
            Ok(page(&["target"], None)),
        ]);
        assert_eq!(
            enumerate_and_classify(&mut source, "target"),
            ReconciliationOutcome::CompleteMany {
                matched_markers: vec!["target".to_string(), "target".to_string()]
            }
        );
    }

    #[test]
    fn a_page_fetch_error_is_incomplete_never_zero() {
        let mut source = FixedPages(vec![Err(EnumerationError::Transport)]);
        assert_eq!(
            enumerate_and_classify(&mut source, "target"),
            ReconciliationOutcome::Incomplete
        );
    }

    #[test]
    fn exhausting_the_page_cap_without_a_terminal_page_is_incomplete_never_zero() {
        struct NeverTerminates;
        impl MarkerEnumerationSource for NeverTerminates {
            fn next_page(
                &mut self,
                _cursor: Option<&[u8]>,
            ) -> Result<MarkerEnumerationPage, EnumerationError> {
                Ok(MarkerEnumerationPage {
                    markers: vec![],
                    next_cursor: Some(vec![0]),
                })
            }
        }
        let mut source = NeverTerminates;
        assert_eq!(
            enumerate_and_classify(&mut source, "target"),
            ReconciliationOutcome::Incomplete
        );
    }

    #[test]
    fn every_marker_condition_disposition_is_user_assisted_never_automatic_retry() {
        for condition in [
            MarkerCondition::Missing,
            MarkerCondition::Removed,
            MarkerCondition::Unsupported,
        ] {
            assert_eq!(
                classify_marker_condition(condition),
                MarkerConditionDisposition::UserAssisted
            );
        }
    }

    #[test]
    fn recreate_increments_generation_with_a_new_operation_identity() {
        let confirmation = RecreateConfirmation {
            previous_recreate_generation: 2,
            previous_operation_id: [1u8; 16],
        };
        let result = confirm_recreate(confirmation, [2u8; 16]).expect("new identity accepted");
        assert_eq!(result.recreate_generation, 3);
    }

    #[test]
    fn recreate_rejects_reusing_the_previous_operation_identity() {
        let confirmation = RecreateConfirmation {
            previous_recreate_generation: 0,
            previous_operation_id: [9u8; 16],
        };
        assert_eq!(confirm_recreate(confirmation, [9u8; 16]), None);
    }
}
