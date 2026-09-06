//! Candidate establishment and promotion.
//!
//! Implements every row of `contracts/domain/policy-state-boundary.json`
//! `establishment_policy`, matching `docs/product-spec.md` 5.1 "Candidate
//! establishment and promotion". Each [`DetectionCase`] variant carries
//! exactly the typed evidence/confidence/ambiguity fields its row requires;
//! there is no variant and no code path that can promote from confidence
//! alone, satisfying "Model confidence alone never promotes a candidate."

use crate::facets::{
    ConfidenceBucket, ProvenanceCode, ProvenanceSet, ReviewFlagCode, ReviewFlagSet,
};

/// Whether a detection's required spans/participant references passed
/// deterministic validation (`docs/product-spec.md` 5.1: "Valid evidence
/// means all required spans/participant references pass deterministic
/// validation").
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceValidity {
    /// Required spans/participant references validated.
    Valid,
    /// Validation failed.
    Invalid,
}

/// The single artifact-authority value this crate can represent while
/// `hybrid`/`automatic` reminder modes remain disabled: every promoting
/// establishment row grants review-only history, never automatic artifact
/// authority (`establishment_policy[].history`). Because this enum has one
/// variant, no code path in this crate can construct any other authority
/// level until a future ADR-011 implementation adds one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactAuthority {
    /// Review-only; grants no automatic artifact-creation authority.
    ReviewOnlyNoArtifactAuthority,
}

/// A gate this crate cannot satisfy on its own; `decide` never resolves past
/// this into an established loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequiredGate {
    /// `establishment_policy` calendar-invitation row:
    /// `open_only_after_G_CAL_and_G_MAIL_input`.
    CalendarAndMail,
}

/// One `establishment_policy` case, carrying exactly the evidence fields its
/// row's predicate needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetectionCase {
    /// "validated explicit outgoing promise; medium or high; no core
    /// ambiguity".
    ExplicitOutgoingPromise {
        /// Evidence validity.
        validity: EvidenceValidity,
        /// Calibrated confidence.
        confidence: ConfidenceBucket,
        /// Whether actor/atomicity/responsibility/quote-newness/association
        /// ambiguity is present.
        core_ambiguity: bool,
    },
    /// "validated direct incoming request or question; deterministic
    /// identity; medium or high; no core ambiguity".
    DirectIncomingRequestOrQuestion {
        /// Evidence validity.
        validity: EvidenceValidity,
        /// Whether the addressed identity resolved deterministically.
        deterministic_identity: bool,
        /// Calibrated confidence.
        confidence: ConfidenceBucket,
        /// Whether core ambiguity is present.
        core_ambiguity: bool,
    },
    /// "validated acknowledgement of associated request".
    AcknowledgementOfAssociatedRequest {
        /// Evidence validity of the association.
        validity: EvidenceValidity,
        /// Calibrated confidence.
        confidence: ConfidenceBucket,
        /// Whether future-act language supports adding `promised` too.
        future_act_evidence: bool,
    },
    /// "validated explicit deterministic attribution; high; no transfer
    /// ambiguity".
    ExplicitDeterministicAttribution {
        /// Evidence validity.
        validity: EvidenceValidity,
        /// Calibrated confidence.
        confidence: ConfidenceBucket,
        /// Whether responsibility-transfer ambiguity is present.
        transfer_ambiguity: bool,
    },
    /// "calendar invitation": always [`RequiredGate::CalendarAndMail`] in
    /// this crate; G-CAL/G-MAIL evaluation is out of scope for ADR-008.
    CalendarInvitation,
    /// "soft implied social contextual role low-confidence or
    /// core-ambiguous".
    SoftImpliedSocialOrContextual {
        /// The exact ambiguity flags this detection observed, added to
        /// `needs_review`.
        ambiguity_flags: ReviewFlagSet,
    },
    /// "user confirms candidate mine and actionable". Version-checking and
    /// transition append are [`crate::command::apply_command`]'s
    /// `confirm_mine_actionable` responsibility; this row only states the
    /// resulting facet outcome.
    UserConfirmsCandidateMineActionable,
    /// "invalid or ungrounded hypothesis".
    InvalidOrUngroundedHypothesis,
}

/// The outcome [`decide`] returns for one [`DetectionCase`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EstablishmentOutcome {
    /// Promotes/establishes an open loop with the given provenance additions
    /// and artifact authority.
    Open {
        /// Provenance values to add (never overwriting existing ones).
        provenance_add: ProvenanceSet,
        /// The artifact authority this promotion grants.
        history: ArtifactAuthority,
    },
    /// Cannot resolve to an established loop until `gate` passes.
    PendingGate(RequiredGate),
    /// Remains (or becomes) a candidate with the given review flags.
    Candidate {
        /// Review flags to attach.
        review_flags: ReviewFlagSet,
    },
    /// No loop-state transition; the input never legally establishes.
    Unchanged,
}

/// Decides the establishment outcome for one detection, exactly reproducing
/// one `establishment_policy` row.
#[must_use]
pub fn decide(case: DetectionCase) -> EstablishmentOutcome {
    match case {
        DetectionCase::ExplicitOutgoingPromise {
            validity,
            confidence,
            core_ambiguity,
        } => {
            if matches!(validity, EvidenceValidity::Valid)
                && confidence.is_medium_or_high()
                && !core_ambiguity
            {
                EstablishmentOutcome::Open {
                    provenance_add: ProvenanceSet::empty().with(ProvenanceCode::Promised),
                    history: ArtifactAuthority::ReviewOnlyNoArtifactAuthority,
                }
            } else {
                EstablishmentOutcome::Candidate {
                    review_flags: ReviewFlagSet::empty().with(ReviewFlagCode::NeedsReview),
                }
            }
        }
        DetectionCase::DirectIncomingRequestOrQuestion {
            validity,
            deterministic_identity,
            confidence,
            core_ambiguity,
        } => {
            if matches!(validity, EvidenceValidity::Valid)
                && deterministic_identity
                && confidence.is_medium_or_high()
                && !core_ambiguity
            {
                EstablishmentOutcome::Open {
                    provenance_add: ProvenanceSet::empty().with(ProvenanceCode::Requested),
                    history: ArtifactAuthority::ReviewOnlyNoArtifactAuthority,
                }
            } else {
                EstablishmentOutcome::Candidate {
                    review_flags: ReviewFlagSet::empty()
                        .with(ReviewFlagCode::NeedsReview)
                        .with(ReviewFlagCode::IdentityAmbiguous),
                }
            }
        }
        DetectionCase::AcknowledgementOfAssociatedRequest {
            validity,
            confidence,
            future_act_evidence,
        } => {
            if matches!(validity, EvidenceValidity::Valid) && confidence.is_medium_or_high() {
                let mut provenance_add = ProvenanceSet::empty().with(ProvenanceCode::Acknowledged);
                if future_act_evidence {
                    provenance_add = provenance_add.with(ProvenanceCode::Promised);
                }
                EstablishmentOutcome::Open {
                    provenance_add,
                    history: ArtifactAuthority::ReviewOnlyNoArtifactAuthority,
                }
            } else {
                EstablishmentOutcome::Candidate {
                    review_flags: ReviewFlagSet::empty().with(ReviewFlagCode::NeedsReview),
                }
            }
        }
        DetectionCase::ExplicitDeterministicAttribution {
            validity,
            confidence,
            transfer_ambiguity,
        } => {
            if matches!(validity, EvidenceValidity::Valid)
                && matches!(confidence, ConfidenceBucket::High)
                && !transfer_ambiguity
            {
                EstablishmentOutcome::Open {
                    provenance_add: ProvenanceSet::empty().with(ProvenanceCode::Attributed),
                    history: ArtifactAuthority::ReviewOnlyNoArtifactAuthority,
                }
            } else {
                EstablishmentOutcome::Candidate {
                    review_flags: ReviewFlagSet::empty()
                        .with(ReviewFlagCode::NeedsReview)
                        .with(ReviewFlagCode::DelegationAmbiguous),
                }
            }
        }
        DetectionCase::CalendarInvitation => {
            EstablishmentOutcome::PendingGate(RequiredGate::CalendarAndMail)
        }
        DetectionCase::SoftImpliedSocialOrContextual { ambiguity_flags } => {
            EstablishmentOutcome::Candidate {
                review_flags: ambiguity_flags.with(ReviewFlagCode::NeedsReview),
            }
        }
        DetectionCase::UserConfirmsCandidateMineActionable => EstablishmentOutcome::Open {
            provenance_add: ProvenanceSet::empty(),
            history: ArtifactAuthority::ReviewOnlyNoArtifactAuthority,
        },
        DetectionCase::InvalidOrUngroundedHypothesis => EstablishmentOutcome::Unchanged,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ArtifactAuthority, DetectionCase, EstablishmentOutcome, EvidenceValidity, RequiredGate,
        decide,
    };
    use crate::facets::{ConfidenceBucket, ProvenanceCode, ReviewFlagCode, ReviewFlagSet};

    #[test]
    fn explicit_promise_promotes_only_when_valid_confident_and_unambiguous() {
        let promoted = decide(DetectionCase::ExplicitOutgoingPromise {
            validity: EvidenceValidity::Valid,
            confidence: ConfidenceBucket::Medium,
            core_ambiguity: false,
        });
        assert_eq!(
            promoted,
            EstablishmentOutcome::Open {
                provenance_add: super::ProvenanceSet::empty().with(ProvenanceCode::Promised),
                history: ArtifactAuthority::ReviewOnlyNoArtifactAuthority,
            }
        );
    }

    #[test]
    fn high_confidence_alone_never_promotes_when_ambiguous_or_invalid() {
        // Confidence alone must never be sufficient: pair High confidence
        // with either invalid evidence or core ambiguity and assert the
        // outcome is always Candidate, never Open.
        let ambiguous = decide(DetectionCase::ExplicitOutgoingPromise {
            validity: EvidenceValidity::Valid,
            confidence: ConfidenceBucket::High,
            core_ambiguity: true,
        });
        assert!(matches!(ambiguous, EstablishmentOutcome::Candidate { .. }));

        let invalid = decide(DetectionCase::ExplicitOutgoingPromise {
            validity: EvidenceValidity::Invalid,
            confidence: ConfidenceBucket::High,
            core_ambiguity: false,
        });
        assert!(matches!(invalid, EstablishmentOutcome::Candidate { .. }));

        let low_confidence = decide(DetectionCase::ExplicitOutgoingPromise {
            validity: EvidenceValidity::Valid,
            confidence: ConfidenceBucket::Low,
            core_ambiguity: false,
        });
        assert!(matches!(
            low_confidence,
            EstablishmentOutcome::Candidate { .. }
        ));
    }

    #[test]
    fn direct_request_requires_deterministic_identity() {
        let no_identity = decide(DetectionCase::DirectIncomingRequestOrQuestion {
            validity: EvidenceValidity::Valid,
            deterministic_identity: false,
            confidence: ConfidenceBucket::High,
            core_ambiguity: false,
        });
        assert!(matches!(
            no_identity,
            EstablishmentOutcome::Candidate { .. }
        ));

        let identified = decide(DetectionCase::DirectIncomingRequestOrQuestion {
            validity: EvidenceValidity::Valid,
            deterministic_identity: true,
            confidence: ConfidenceBucket::Medium,
            core_ambiguity: false,
        });
        assert_eq!(
            identified,
            EstablishmentOutcome::Open {
                provenance_add: super::ProvenanceSet::empty().with(ProvenanceCode::Requested),
                history: ArtifactAuthority::ReviewOnlyNoArtifactAuthority,
            }
        );
    }

    #[test]
    fn acknowledgement_adds_promised_only_with_future_act_evidence() {
        let without_future_act = decide(DetectionCase::AcknowledgementOfAssociatedRequest {
            validity: EvidenceValidity::Valid,
            confidence: ConfidenceBucket::High,
            future_act_evidence: false,
        });
        assert_eq!(
            without_future_act,
            EstablishmentOutcome::Open {
                provenance_add: super::ProvenanceSet::empty().with(ProvenanceCode::Acknowledged),
                history: ArtifactAuthority::ReviewOnlyNoArtifactAuthority,
            }
        );

        let with_future_act = decide(DetectionCase::AcknowledgementOfAssociatedRequest {
            validity: EvidenceValidity::Valid,
            confidence: ConfidenceBucket::High,
            future_act_evidence: true,
        });
        assert_eq!(
            with_future_act,
            EstablishmentOutcome::Open {
                provenance_add: super::ProvenanceSet::empty()
                    .with(ProvenanceCode::Acknowledged)
                    .with(ProvenanceCode::Promised),
                history: ArtifactAuthority::ReviewOnlyNoArtifactAuthority,
            }
        );
    }

    #[test]
    fn attribution_requires_high_confidence_exactly_not_medium() {
        let medium = decide(DetectionCase::ExplicitDeterministicAttribution {
            validity: EvidenceValidity::Valid,
            confidence: ConfidenceBucket::Medium,
            transfer_ambiguity: false,
        });
        assert!(matches!(medium, EstablishmentOutcome::Candidate { .. }));

        let high = decide(DetectionCase::ExplicitDeterministicAttribution {
            validity: EvidenceValidity::Valid,
            confidence: ConfidenceBucket::High,
            transfer_ambiguity: false,
        });
        assert_eq!(
            high,
            EstablishmentOutcome::Open {
                provenance_add: super::ProvenanceSet::empty().with(ProvenanceCode::Attributed),
                history: ArtifactAuthority::ReviewOnlyNoArtifactAuthority,
            }
        );
    }

    #[test]
    fn calendar_invitation_never_establishes_without_the_gate() {
        // Safety invariant: no evidence shape for CalendarInvitation exists
        // that reaches EstablishmentOutcome::Open in this crate; the variant
        // carries no fields to smuggle a bypass through.
        assert_eq!(
            decide(DetectionCase::CalendarInvitation),
            EstablishmentOutcome::PendingGate(RequiredGate::CalendarAndMail)
        );
    }

    #[test]
    fn soft_or_ambiguous_detection_always_remains_candidate_with_needs_review() {
        let outcome = decide(DetectionCase::SoftImpliedSocialOrContextual {
            ambiguity_flags: ReviewFlagSet::empty().with(ReviewFlagCode::AssociationAmbiguous),
        });
        let EstablishmentOutcome::Candidate { review_flags } = outcome else {
            panic!("expected Candidate outcome");
        };
        assert!(review_flags.contains(ReviewFlagCode::NeedsReview));
        assert!(review_flags.contains(ReviewFlagCode::AssociationAmbiguous));
    }

    #[test]
    fn user_confirmation_promotes_without_requiring_confidence() {
        assert_eq!(
            decide(DetectionCase::UserConfirmsCandidateMineActionable),
            EstablishmentOutcome::Open {
                provenance_add: super::ProvenanceSet::empty(),
                history: ArtifactAuthority::ReviewOnlyNoArtifactAuthority,
            }
        );
    }

    #[test]
    fn invalid_hypothesis_never_transitions_state() {
        assert_eq!(
            decide(DetectionCase::InvalidOrUngroundedHypothesis),
            EstablishmentOutcome::Unchanged
        );
    }
}
