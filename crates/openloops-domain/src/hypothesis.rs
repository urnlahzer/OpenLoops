//! Closure-hypothesis identity and the loop-level closure-review projection.
//!
//! Implements `contracts/domain/policy-state-boundary.json`
//! `hypothesis_projection` and ADR-008's "Multiple hypotheses and keep-open"
//! section: several current closure/decline/delegation/moot hypotheses may
//! coexist; no new persistent hypothesis text or key is added; identity is
//! derived only from approved relation/transition fields; the loop-level
//! `closure_review_state` is a deterministic projection over current
//! hypotheses, never a separately stored enum.

use crate::facets::ClosureReviewState;
use crate::ids::{OpaqueId, Version};

/// The kind of closure-adjacent hypothesis a model may propose. ADR-008:
/// "Several current closure, decline, delegation, or moot hypotheses may
/// coexist."
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum HypothesisKind {
    /// A candidate closure (fulfillment) hypothesis.
    Closure,
    /// A candidate decline hypothesis.
    Decline,
    /// A candidate delegation hypothesis.
    Delegation,
    /// A candidate mootness hypothesis.
    Moot,
}

impl HypothesisKind {
    /// Every catalog value.
    pub const ALL: [Self; 4] = [Self::Closure, Self::Decline, Self::Delegation, Self::Moot];
}

/// One validated source reference plus the source version it was observed
/// at. Ordering by source version is what lets keep-open "remain reviewable"
/// exactly when a causally later version appears
/// (`hypothesis_projection.suppression`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VersionedSourceRef {
    /// The referenced evidence's opaque id.
    pub source_ref_id: OpaqueId,
    /// The source version this hypothesis observed.
    pub source_version: Version,
}

/// A closure-hypothesis identity: "target loop plus hypothesis kind plus
/// ordered validated source references including source versions plus
/// policy version" (`hypothesis_projection.identity`). No hypothesis text or
/// model rationale is represented anywhere in this type.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct HypothesisIdentity {
    /// The loop this hypothesis targets.
    pub target_loop: OpaqueId,
    /// The hypothesis kind.
    pub kind: HypothesisKind,
    /// Ordered validated source references (including their versions).
    pub source_refs: Vec<VersionedSourceRef>,
    /// The policy version this hypothesis was evaluated under.
    pub policy_version: Version,
}

/// The loop-level `closure_review_state` this module can project. Mirrors
/// [`ClosureReviewState`] exactly (excluding nothing) so [`project`]'s
/// return type cannot represent a fifth, uncataloged state.
pub type ClosureReviewProjection = ClosureReviewState;

/// The inputs `hypothesis_projection.projection_order` evaluates, in the
/// contract's exact priority order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectionInputs {
    /// Whether any current hypothesis for this loop is unsuppressed.
    pub unsuppressed_current_hypothesis_exists: bool,
    /// Whether an evidence request is pending for this loop.
    pub evidence_request_pending: bool,
    /// Whether at least one current hypothesis was explicitly suppressed.
    pub suppressed_current_hypothesis_exists: bool,
}

/// Projects the loop-level `closure_review_state`, exactly reproducing
/// `hypothesis_projection.projection_order`:
/// "possible if any unsuppressed current hypothesis exists; otherwise
/// `evidence_requested` if no possible hypothesis and any evidence request is
/// pending; otherwise `kept_open` if no possible or requested hypothesis and
/// at least one current hypothesis was suppressed; otherwise none."
#[must_use]
pub const fn project(inputs: ProjectionInputs) -> ClosureReviewProjection {
    if inputs.unsuppressed_current_hypothesis_exists {
        ClosureReviewProjection::Possible
    } else if inputs.evidence_request_pending {
        ClosureReviewProjection::EvidenceRequested
    } else if inputs.suppressed_current_hypothesis_exists {
        ClosureReviewProjection::KeptOpen
    } else {
        ClosureReviewProjection::None
    }
}

/// Returns whether `identity` is suppressed by any entry in `suppressed`.
///
/// `hypothesis_projection.suppression`: "`keep_open` suppresses only the exact
/// identity; causally later source version or a different loop kind source
/// tuple or policy version remains reviewable." Because [`HypothesisIdentity`]
/// derives structural equality over every one of those fields, changing any
/// single field (loop, kind, one source ref's version, or policy version)
/// makes this return `false` — there is no looser matching path.
#[must_use]
pub fn is_suppressed(identity: &HypothesisIdentity, suppressed: &[HypothesisIdentity]) -> bool {
    suppressed.iter().any(|entry| entry == identity)
}

#[cfg(test)]
mod tests {
    use super::{
        ClosureReviewProjection, HypothesisIdentity, HypothesisKind, ProjectionInputs,
        VersionedSourceRef, is_suppressed, project,
    };
    use crate::ids::{OpaqueId, Version};

    fn identity(loop_byte: u8, version: u64) -> HypothesisIdentity {
        HypothesisIdentity {
            target_loop: OpaqueId::from_bytes([loop_byte; 16]),
            kind: HypothesisKind::Closure,
            source_refs: vec![VersionedSourceRef {
                source_ref_id: OpaqueId::from_bytes([1; 16]),
                source_version: Version::new(core::num::NonZeroU64::new(1).unwrap()),
            }],
            policy_version: Version::new(core::num::NonZeroU64::new(version).unwrap()),
        }
    }

    #[test]
    fn possible_outranks_evidence_requested_and_kept_open() {
        let outcome = project(ProjectionInputs {
            unsuppressed_current_hypothesis_exists: true,
            evidence_request_pending: true,
            suppressed_current_hypothesis_exists: true,
        });
        assert_eq!(outcome, ClosureReviewProjection::Possible);
    }

    #[test]
    fn evidence_requested_only_when_no_possible_hypothesis() {
        let outcome = project(ProjectionInputs {
            unsuppressed_current_hypothesis_exists: false,
            evidence_request_pending: true,
            suppressed_current_hypothesis_exists: true,
        });
        assert_eq!(outcome, ClosureReviewProjection::EvidenceRequested);
    }

    #[test]
    fn kept_open_only_when_no_possible_or_requested_hypothesis() {
        let outcome = project(ProjectionInputs {
            unsuppressed_current_hypothesis_exists: false,
            evidence_request_pending: false,
            suppressed_current_hypothesis_exists: true,
        });
        assert_eq!(outcome, ClosureReviewProjection::KeptOpen);
    }

    #[test]
    fn none_when_nothing_current_pending_or_suppressed() {
        let outcome = project(ProjectionInputs {
            unsuppressed_current_hypothesis_exists: false,
            evidence_request_pending: false,
            suppressed_current_hypothesis_exists: false,
        });
        assert_eq!(outcome, ClosureReviewProjection::None);
    }

    #[test]
    fn keep_open_suppresses_only_the_exact_identity() {
        let suppressed = vec![identity(1, 5)];
        assert!(is_suppressed(&identity(1, 5), &suppressed));

        // A causally later policy version on the same loop/kind/source-refs
        // is a different identity and remains reviewable.
        assert!(!is_suppressed(&identity(1, 6), &suppressed));

        // A different loop is a different identity and remains reviewable.
        assert!(!is_suppressed(&identity(2, 5), &suppressed));
    }

    #[test]
    fn a_causally_later_source_version_remains_reviewable() {
        let mut later = identity(1, 5);
        later.source_refs[0].source_version = Version::new(core::num::NonZeroU64::new(2).unwrap());
        let suppressed = vec![identity(1, 5)];
        assert!(!is_suppressed(&later, &suppressed));
    }
}
