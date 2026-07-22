//! Full-state legality validation over the closed facet catalogs.
//!
//! `contracts/domain/policy-state-boundary.json` states twelve
//! `legality_rules`, refining ADR-008's seven numbered "mandatory" legality
//! rules. `tools/check-policy-state-boundary.ps1` (`P0-POLICY-LEGALITY-001`)
//! derives an exact legal-projection count — 41 legal projections out of the
//! 420-tuple Cartesian product of `obligation_state_code` (3) ×
//! `resolution_code` (7) × `closure_review_state_code` (4) ×
//! `deadline_state_code` (5) — from three structural constraints. This module
//! reproduces that same arithmetic from the same three constraints
//! ([`validate`]), and [`enumerate_legal_combinations`] derives the count
//! from those rules rather than hard-coding the expected number.
//!
//! Not every legality rule is a Cartesian facet-combination constraint; some
//! are transition/API-shape rules enforced elsewhere. The full mapping:
//!
//! | # | `legality_rules` text (abridged) | ADR-008 rule | Enforced by |
//! |---|---|---|---|
//! | 1 | resolution none iff candidate/open | 1 | [`validate`] (this module) |
//! | 2 | system/model hypothesis never terminal | 1 | [`crate::transition::Transition::new`] rejects `actor=System` with a terminal `new_facets.obligation_state` |
//! | 3 | possible/kept_open/evidence_requested require open | 2 | [`validate`] (this module) |
//! | 4 | approaching/overdue require open + resolved boundary | 3 | [`validate`] (this module) |
//! | 5 | candidate never ages; terminal stops aging, retains value | 3 | [`crate::deadline::project_aging`] returns `None` unless `obligation_state=open` |
//! | 6 | `undated_confirmed` is an explicit user transition | 4 | [`crate::deadline::apply_prompt_command`] only reaches `undated_confirmed` through `PromptCommand::NoDeadline`, which the transition layer requires `ActorCode::User` for |
//! | 7 | quarantine requires locator + retry/dismiss | 5 | [`crate::facets::AnalysisState`] callers must supply `QuarantineEvidence` (see `establishment` module docs) before quarantine is representable |
//! | 8 | changed/partial/unavailable evidence freezes automation | 5 | [`crate::establishment::decide`] and [`crate::command::apply_command`] require [`crate::facets::SourceState::Available`] before any automated promotion/command precondition can pass |
//! | 9 | reminder events never change obligation/resolution | 6 | [`crate::facets::ReminderState`] carries no obligation/resolution field, and no function in this crate takes a reminder-state input and returns an obligation/resolution output |
//! | 10 | later terminal evidence adds only `needs_review` + relation | 7 | [`crate::command::apply_terminal_review_note`] |
//! | 11 | model confidence alone never promotes | establishment section | [`crate::establishment::decide`] requires an [`crate::establishment::EvidenceValidity`] input distinct from confidence; confidence alone cannot select a promoting arm |
//! | 12 | user correction dominates unless causally later evidence | correction section | [`crate::command::apply_command`] command preconditions always require [`crate::facets::ActorCode::User`] for terminal/correcting commands; system actors can only ever produce [`crate::transition::TransitionReasonCode`] values that keep the loop non-terminal |

use crate::facets::{ClosureReviewState, DeadlineState, ObligationState, Resolution};

/// The four facets whose Cartesian legality this module validates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoopLegalityFacets {
    /// `loop.obligation_state_code`.
    pub obligation_state: ObligationState,
    /// `loop.resolution_code`.
    pub resolution: Resolution,
    /// `loop.closure_review_state_code`.
    pub closure_review_state: ClosureReviewState,
    /// `loop.deadline_state_code`.
    pub deadline_state: DeadlineState,
}

/// A violated structural legality rule, with the exact `legality_rules`
/// index (1-based, matching the contract array) that rejected the tuple.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegalityViolation {
    /// Rule 1: `resolution_code=none` iff `obligation_state` is
    /// `candidate`/`open`; every terminal loop has exactly one non-none
    /// resolution.
    ResolutionObligationMismatch,
    /// Rule 3: `possible`/`kept_open`/`evidence_requested` closure review
    /// requires an open obligation.
    ClosureReviewRequiresOpen,
    /// Rule 4: `approaching`/`overdue` deadline state requires an open
    /// obligation.
    DeadlineAgingRequiresOpen,
}

/// Validates one facet tuple against the three Cartesian legality rules.
///
/// # Errors
///
/// Returns the first [`LegalityViolation`] found, checked in contract rule
/// order.
pub const fn validate(facets: LoopLegalityFacets) -> Result<(), LegalityViolation> {
    let is_terminal = matches!(facets.obligation_state, ObligationState::Terminal);
    let has_resolution = facets.resolution.is_some();
    if is_terminal != has_resolution {
        return Err(LegalityViolation::ResolutionObligationMismatch);
    }
    let is_open = matches!(facets.obligation_state, ObligationState::Open);
    let has_closure_review = !matches!(facets.closure_review_state, ClosureReviewState::None);
    if has_closure_review && !is_open {
        return Err(LegalityViolation::ClosureReviewRequiresOpen);
    }
    let is_aging = matches!(
        facets.deadline_state,
        DeadlineState::Approaching | DeadlineState::Overdue
    );
    if is_aging && !is_open {
        return Err(LegalityViolation::DeadlineAgingRequiresOpen);
    }
    Ok(())
}

/// Returns whether `facets` satisfies every Cartesian legality rule.
#[must_use]
pub const fn is_legal(facets: LoopLegalityFacets) -> bool {
    validate(facets).is_ok()
}

/// Enumerates every legal facet tuple by exhaustively walking the full
/// Cartesian product of the four catalogs and keeping only the tuples
/// [`validate`] accepts.
///
/// This is the exhaustive-enumeration function `P0-POLICY-LEGALITY-001`
/// requires: the legal count it produces is *derived* from [`validate`]'s
/// rules, not asserted as a literal.
#[must_use]
pub fn enumerate_legal_combinations() -> Vec<LoopLegalityFacets> {
    let mut legal = Vec::new();
    for obligation_state in ObligationState::ALL {
        for resolution in Resolution::ALL {
            for closure_review_state in ClosureReviewState::ALL {
                for deadline_state in DeadlineState::ALL {
                    let facets = LoopLegalityFacets {
                        obligation_state,
                        resolution,
                        closure_review_state,
                        deadline_state,
                    };
                    if is_legal(facets) {
                        legal.push(facets);
                    }
                }
            }
        }
    }
    legal
}

#[cfg(test)]
mod tests {
    use super::{
        ClosureReviewState, DeadlineState, LegalityViolation, LoopLegalityFacets, ObligationState,
        Resolution, enumerate_legal_combinations, is_legal,
    };

    fn total_cartesian_size() -> usize {
        ObligationState::ALL.len()
            * Resolution::ALL.len()
            * ClosureReviewState::ALL.len()
            * DeadlineState::ALL.len()
    }

    #[test]
    fn cartesian_product_has_exactly_420_tuples() {
        assert_eq!(total_cartesian_size(), 420);
    }

    #[test]
    fn exactly_41_of_420_tuples_are_legal() {
        // P0-POLICY-LEGALITY-001 pins this exact number. It must fall out of
        // `enumerate_legal_combinations`, which walks the full 420-tuple
        // product and keeps only what `validate` accepts — never a hardcoded
        // list matching the expected count.
        let legal = enumerate_legal_combinations();
        assert_eq!(legal.len(), 41);
    }

    #[test]
    fn candidate_and_terminal_require_resolution_none_iff_appropriate() {
        let candidate_with_resolution = LoopLegalityFacets {
            obligation_state: ObligationState::Candidate,
            resolution: Resolution::Closed,
            closure_review_state: ClosureReviewState::None,
            deadline_state: DeadlineState::Unresolved,
        };
        assert_eq!(
            super::validate(candidate_with_resolution),
            Err(LegalityViolation::ResolutionObligationMismatch)
        );

        let terminal_without_resolution = LoopLegalityFacets {
            obligation_state: ObligationState::Terminal,
            resolution: Resolution::None,
            closure_review_state: ClosureReviewState::None,
            deadline_state: DeadlineState::Unresolved,
        };
        assert_eq!(
            super::validate(terminal_without_resolution),
            Err(LegalityViolation::ResolutionObligationMismatch)
        );

        let terminal_with_resolution = LoopLegalityFacets {
            obligation_state: ObligationState::Terminal,
            resolution: Resolution::Closed,
            closure_review_state: ClosureReviewState::None,
            deadline_state: DeadlineState::Unresolved,
        };
        assert!(is_legal(terminal_with_resolution));
    }

    #[test]
    fn closure_review_state_requires_open_obligation() {
        for obligation_state in [ObligationState::Candidate, ObligationState::Terminal] {
            let resolution = if obligation_state == ObligationState::Terminal {
                Resolution::Closed
            } else {
                Resolution::None
            };
            let facets = LoopLegalityFacets {
                obligation_state,
                resolution,
                closure_review_state: ClosureReviewState::Possible,
                deadline_state: DeadlineState::Unresolved,
            };
            assert_eq!(
                super::validate(facets),
                Err(LegalityViolation::ClosureReviewRequiresOpen)
            );
        }

        let open_with_closure_review = LoopLegalityFacets {
            obligation_state: ObligationState::Open,
            resolution: Resolution::None,
            closure_review_state: ClosureReviewState::Possible,
            deadline_state: DeadlineState::Unresolved,
        };
        assert!(is_legal(open_with_closure_review));
    }

    #[test]
    fn approaching_and_overdue_require_open_obligation() {
        for deadline_state in [DeadlineState::Approaching, DeadlineState::Overdue] {
            let candidate = LoopLegalityFacets {
                obligation_state: ObligationState::Candidate,
                resolution: Resolution::None,
                closure_review_state: ClosureReviewState::None,
                deadline_state,
            };
            assert_eq!(
                super::validate(candidate),
                Err(LegalityViolation::DeadlineAgingRequiresOpen)
            );

            let terminal = LoopLegalityFacets {
                obligation_state: ObligationState::Terminal,
                resolution: Resolution::Closed,
                closure_review_state: ClosureReviewState::None,
                deadline_state,
            };
            assert_eq!(
                super::validate(terminal),
                Err(LegalityViolation::DeadlineAgingRequiresOpen)
            );

            let open = LoopLegalityFacets {
                obligation_state: ObligationState::Open,
                resolution: Resolution::None,
                closure_review_state: ClosureReviewState::None,
                deadline_state,
            };
            assert!(is_legal(open));
        }
    }

    #[test]
    fn every_enumerated_tuple_is_independently_revalidated_as_legal() {
        for facets in enumerate_legal_combinations() {
            assert!(is_legal(facets));
        }
    }
}
