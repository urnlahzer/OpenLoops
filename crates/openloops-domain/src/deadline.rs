//! Deadline precedence, aging, and the deadline-prompt catalog.
//!
//! Backs `contracts/domain/policy-state-boundary.json` `deadline_policy` and
//! `docs/product-spec.md` 6.6 (the operative-deadline decision table and the
//! operational-aging table). Time enters this module only through the
//! injected [`Clock`] trait — nothing here reads the system clock directly,
//! so aging decisions stay pure and deterministically testable.

use crate::facets::DeadlineState;

/// A UTC instant expressed as whole seconds since the Unix epoch.
///
/// This crate performs no calendar arithmetic (timezone/EOD/business-day/DST
/// resolution is ADR-004/settings-layer territory per
/// `deadline_policy.default_eod`); it only compares already-resolved
/// instants, so a plain integer offset is sufficient and avoids adding a
/// time-handling dependency.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct UnixSeconds(pub i64);

impl UnixSeconds {
    /// Returns `self - lead_seconds`, saturating rather than overflowing.
    #[must_use]
    pub const fn minus_lead(self, lead_seconds: u32) -> Self {
        Self(self.0.saturating_sub(lead_seconds as i64))
    }
}

/// Supplies the current instant. Injected so aging decisions never read the
/// system clock directly and stay deterministically testable.
pub trait Clock {
    /// Returns the current instant.
    fn now(&self) -> UnixSeconds;
}

/// A [`Clock`] that always returns a fixed instant, for tests and for
/// callers that resolve "now" once per evaluation batch.
#[derive(Clone, Copy, Debug)]
pub struct FixedClock(pub UnixSeconds);

impl Clock for FixedClock {
    fn now(&self) -> UnixSeconds {
        self.0
    }
}

/// The evidence [`project_aging`] needs to classify one open loop's deadline
/// state. One variant per `deadline_precision_code` plus the two states with
/// no operative deadline yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeadlineAgingEvidence {
    /// No operative deadline has been resolved yet.
    Unresolved,
    /// The user made an explicit durable "no deadline" choice.
    UndatedConfirmed,
    /// `instant` precision: ages against its evidence-derived instant.
    Instant {
        /// The evidence-derived instant.
        instant: UnixSeconds,
        /// The configured approaching-lead window, in seconds.
        lead_seconds: u32,
    },
    /// `date` precision: ages against a configured policy boundary distinct
    /// from evidence precision.
    Date {
        /// The end-of-day policy boundary for the evidence date.
        policy_boundary: UnixSeconds,
        /// The configured approaching-lead window, in seconds.
        lead_seconds: u32,
    },
    /// `business_day` precision: ages against the end of the resolved
    /// business day.
    BusinessDay {
        /// The end-of-business-day policy boundary.
        policy_boundary: UnixSeconds,
        /// The configured approaching-lead window, in seconds.
        lead_seconds: u32,
    },
    /// `week` precision: ages against the retained range end.
    Week {
        /// The end of the retained week/range.
        range_end: UnixSeconds,
        /// The configured approaching-lead window, in seconds.
        lead_seconds: u32,
    },
    /// `event_relative` precision: ages only once a unique event is
    /// correlated; `resolved_boundary` is `None` until then.
    EventRelative {
        /// The resolved event boundary, or `None` while unresolved.
        resolved_boundary: Option<UnixSeconds>,
        /// The configured approaching-lead window, in seconds.
        lead_seconds: u32,
    },
    /// `soft` precision: scheduled urgency with no aging boundary.
    Soft,
    /// `unspecified` precision: no precision was supplied.
    Unspecified,
}

/// Whether the enclosing loop's obligation is open, the only state in which
/// aging may newly classify a deadline (legality rule 4/5:
/// `approaching`/`overdue` require an open obligation; candidate never ages
/// and terminal stops aging while retaining its deadline evidence/value).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgingObligation {
    /// The obligation is open: aging may newly classify the deadline state.
    Open,
    /// The obligation is candidate or terminal: aging must not newly
    /// classify the deadline state. Callers must retain the loop's existing
    /// `deadline_state` unchanged.
    NotOpen,
}

/// Projects the [`DeadlineState`] an open loop's deadline evidence implies
/// at `now`.
///
/// Returns `None` when `obligation` is [`AgingObligation::NotOpen`]: per
/// legality rules 4/5, a candidate or terminal loop never receives a new
/// aging classification, so callers must leave the existing facet value
/// alone rather than treat `None` as "unresolved".
#[must_use]
pub fn project_aging(
    obligation: AgingObligation,
    evidence: DeadlineAgingEvidence,
    now: UnixSeconds,
) -> Option<DeadlineState> {
    if matches!(obligation, AgingObligation::NotOpen) {
        return None;
    }
    Some(match evidence {
        DeadlineAgingEvidence::Unresolved => DeadlineState::Unresolved,
        DeadlineAgingEvidence::UndatedConfirmed
        | DeadlineAgingEvidence::Soft
        | DeadlineAgingEvidence::Unspecified => {
            // undated_confirmed, soft, and unspecified never age (legality
            // rule 6 and deadline_policy.aging_rules).
            if matches!(evidence, DeadlineAgingEvidence::UndatedConfirmed) {
                DeadlineState::UndatedConfirmed
            } else {
                DeadlineState::Scheduled
            }
        }
        DeadlineAgingEvidence::Instant {
            instant,
            lead_seconds,
        } => classify_boundary(instant, lead_seconds, now),
        DeadlineAgingEvidence::Date {
            policy_boundary,
            lead_seconds,
        }
        | DeadlineAgingEvidence::BusinessDay {
            policy_boundary,
            lead_seconds,
        } => classify_boundary(policy_boundary, lead_seconds, now),
        DeadlineAgingEvidence::Week {
            range_end,
            lead_seconds,
        } => classify_boundary(range_end, lead_seconds, now),
        DeadlineAgingEvidence::EventRelative {
            resolved_boundary: None,
            ..
        } => {
            // Unresolved event-relative evidence never ages until one event
            // is selected.
            DeadlineState::Unresolved
        }
        DeadlineAgingEvidence::EventRelative {
            resolved_boundary: Some(boundary),
            lead_seconds,
        } => classify_boundary(boundary, lead_seconds, now),
    })
}

fn classify_boundary(boundary: UnixSeconds, lead_seconds: u32, now: UnixSeconds) -> DeadlineState {
    if now >= boundary {
        DeadlineState::Overdue
    } else if now >= boundary.minus_lead(lead_seconds) {
        DeadlineState::Approaching
    } else {
        DeadlineState::Scheduled
    }
}

/// One row's evidence shape from the `docs/product-spec.md` 6.6
/// operative-deadline decision table / `deadline_policy.operative_rules`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperativeEvidenceEvent {
    /// Row: same message contains both a requested and a user-promised
    /// deadline for one atomic action.
    SameMessageRequestedAndPromised {
        /// Whether actor or scope is ambiguous for this atomic action.
        actor_or_scope_ambiguous: bool,
    },
    /// Row: a later, confidently associated user message proposes "I need
    /// until X".
    LaterUserExtension,
    /// Row: a later, confidently associated requester message restates or
    /// counters the deadline with directive/expectation language.
    LaterRequesterDirectiveCounter,
    /// Row: a later message merely mentions a date, without directive or
    /// modification meaning.
    LaterNonDirectiveDateMention,
    /// Row: multiple deadlines apply to independently separable actions.
    IndependentActionsSeparable,
    /// Row: multiple deadlines plausibly apply to one action (ambiguous
    /// attachment/scope).
    AmbiguousAttachmentToOneAction,
    /// Row: the user explicitly selects "no deadline".
    ExplicitNoDeadline,
    /// Row: "before the meeting" without a unique correlated event.
    UnresolvedEventRelative,
    /// Row: "before the meeting" with a unique correlated event.
    EventRelativeCorrelated,
    /// Row: an explicit but imprecise deadline (date-only, "this week").
    ImprecisePreserved,
    /// Row: a cross-timezone relative date or DST boundary with more than
    /// one reasonable interpretation.
    CrossTimezoneAmbiguous,
}

/// The decision [`decide_operative`] returns for one
/// [`OperativeEvidenceEvent`], matching the decision table's "Result"
/// column exactly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperativeOutcome {
    /// The promised deadline becomes operative; the requested deadline
    /// remains visible but not operative.
    PromisedOperativeOverRequested,
    /// Actor or scope is ambiguous: review is required, neither deadline
    /// becomes operative automatically.
    ActorOrScopeAmbiguousRequiresReview,
    /// The later user extension becomes operative without requester
    /// acceptance; recorded as renegotiated.
    LaterUserExtensionOperativeRenegotiated,
    /// The later requester directive/counter becomes operative; the user's
    /// prior extension is preserved, not discarded.
    LaterRequesterDirectiveOperativePreservingPriorExtension,
    /// A mere date mention is not directive; the operative deadline does
    /// not change.
    NonDirectiveDateMentionNoChange,
    /// Deadlines attach to independently separable actions; loop splitting
    /// itself is OL-DET-003's upstream responsibility, this module records
    /// only that the evidence requires a split.
    IndependentActionsSplit,
    /// Ambiguous attachment to one action: alternatives are preserved and
    /// review is required.
    AmbiguousAttachmentPreservesAlternativesRequiresReview,
    /// The explicit "no deadline" choice sets `undated_confirmed`.
    ExplicitNoDeadlineConfirmed,
    /// No unique event is correlated: stays unresolved and reviewable.
    EventRelativeUnresolved,
    /// A unique event is correlated: the reference resolves and may age.
    EventRelativeResolved,
    /// An imprecise-but-explicit deadline keeps its precision; no
    /// fabricated exact instant is produced.
    PreservePrecisionNoUpgrade,
    /// Cross-timezone/DST ambiguity: alternatives are presented and no
    /// automatic reminder fires.
    CrossTimezoneAmbiguousRequiresReview,
}

/// Decides the operative-deadline outcome for one evidence event, exactly
/// reproducing one row of the `docs/product-spec.md` 6.6 decision table.
#[must_use]
pub const fn decide_operative(event: OperativeEvidenceEvent) -> OperativeOutcome {
    match event {
        OperativeEvidenceEvent::SameMessageRequestedAndPromised {
            actor_or_scope_ambiguous: false,
        } => OperativeOutcome::PromisedOperativeOverRequested,
        OperativeEvidenceEvent::SameMessageRequestedAndPromised {
            actor_or_scope_ambiguous: true,
        } => OperativeOutcome::ActorOrScopeAmbiguousRequiresReview,
        OperativeEvidenceEvent::LaterUserExtension => {
            OperativeOutcome::LaterUserExtensionOperativeRenegotiated
        }
        OperativeEvidenceEvent::LaterRequesterDirectiveCounter => {
            OperativeOutcome::LaterRequesterDirectiveOperativePreservingPriorExtension
        }
        OperativeEvidenceEvent::LaterNonDirectiveDateMention => {
            OperativeOutcome::NonDirectiveDateMentionNoChange
        }
        OperativeEvidenceEvent::IndependentActionsSeparable => {
            OperativeOutcome::IndependentActionsSplit
        }
        OperativeEvidenceEvent::AmbiguousAttachmentToOneAction => {
            OperativeOutcome::AmbiguousAttachmentPreservesAlternativesRequiresReview
        }
        OperativeEvidenceEvent::ExplicitNoDeadline => OperativeOutcome::ExplicitNoDeadlineConfirmed,
        OperativeEvidenceEvent::UnresolvedEventRelative => {
            OperativeOutcome::EventRelativeUnresolved
        }
        OperativeEvidenceEvent::EventRelativeCorrelated => OperativeOutcome::EventRelativeResolved,
        OperativeEvidenceEvent::ImprecisePreserved => OperativeOutcome::PreservePrecisionNoUpgrade,
        OperativeEvidenceEvent::CrossTimezoneAmbiguous => {
            OperativeOutcome::CrossTimezoneAmbiguousRequiresReview
        }
    }
}

/// `deadline_policy.prompt_commands`: the five actions OL-DUE-007 offers on
/// an unresolved-deadline prompt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeadlinePromptCommand {
    /// The user supplies a due date/time.
    SetDeadline,
    /// The user explicitly chooses no deadline.
    NoDeadline,
    /// The user defers only the current prompt version.
    DeferReminder,
    /// The user dismisses the prompt/loop.
    Dismiss,
    /// The user marks the loop not theirs.
    NotMine,
}

impl DeadlinePromptCommand {
    /// Every catalog value, in contract order.
    pub const ALL: [Self; 5] = [
        Self::SetDeadline,
        Self::NoDeadline,
        Self::DeferReminder,
        Self::Dismiss,
        Self::NotMine,
    ];
}

/// Which domain a [`DeadlinePromptCommand`] affects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeadlinePromptEffect {
    /// Records new deadline evidence and may change the operative deadline.
    DeadlineEvidence,
    /// Sets `deadline_state=undated_confirmed` with no operational boundary.
    UndatedConfirmed,
    /// Suppresses only the current prompt version; changes no evidence, sets
    /// no deadline state, and authorizes no artifact
    /// (`deadline_policy.defer_reminder_rule`).
    SuppressPromptOnly,
    /// An obligation-lifecycle command owned by [`crate::command`]
    /// (`dismiss_classify`, reason `not_mine` or a dismissal reason); this
    /// module only records that the prompt routes there.
    RoutesToCommandModule,
}

/// Returns which domain `command` affects, exhaustively over
/// [`DeadlinePromptCommand`].
#[must_use]
pub const fn effect_of(command: DeadlinePromptCommand) -> DeadlinePromptEffect {
    match command {
        DeadlinePromptCommand::SetDeadline => DeadlinePromptEffect::DeadlineEvidence,
        DeadlinePromptCommand::NoDeadline => DeadlinePromptEffect::UndatedConfirmed,
        DeadlinePromptCommand::DeferReminder => DeadlinePromptEffect::SuppressPromptOnly,
        DeadlinePromptCommand::Dismiss | DeadlinePromptCommand::NotMine => {
            DeadlinePromptEffect::RoutesToCommandModule
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgingObligation, DeadlineAgingEvidence, DeadlinePromptCommand, DeadlinePromptEffect,
        OperativeEvidenceEvent, OperativeOutcome, UnixSeconds, decide_operative, effect_of,
        project_aging,
    };
    use crate::facets::DeadlineState;

    #[test]
    fn candidate_and_terminal_never_receive_a_new_aging_classification() {
        let evidence = DeadlineAgingEvidence::Instant {
            instant: UnixSeconds(1_000),
            lead_seconds: 60,
        };
        assert_eq!(
            project_aging(AgingObligation::NotOpen, evidence, UnixSeconds(1_000)),
            None
        );
    }

    #[test]
    fn instant_precision_ages_exactly_at_and_after_its_boundary() {
        let evidence = DeadlineAgingEvidence::Instant {
            instant: UnixSeconds(1_000),
            lead_seconds: 100,
        };
        assert_eq!(
            project_aging(AgingObligation::Open, evidence, UnixSeconds(800)),
            Some(DeadlineState::Scheduled)
        );
        assert_eq!(
            project_aging(AgingObligation::Open, evidence, UnixSeconds(900)),
            Some(DeadlineState::Approaching)
        );
        assert_eq!(
            project_aging(AgingObligation::Open, evidence, UnixSeconds(1_000)),
            Some(DeadlineState::Overdue)
        );
    }

    #[test]
    fn date_and_business_day_age_against_a_policy_boundary() {
        let date_evidence = DeadlineAgingEvidence::Date {
            policy_boundary: UnixSeconds(2_000),
            lead_seconds: 500,
        };
        assert_eq!(
            project_aging(AgingObligation::Open, date_evidence, UnixSeconds(2_100)),
            Some(DeadlineState::Overdue)
        );
        let business_day_evidence = DeadlineAgingEvidence::BusinessDay {
            policy_boundary: UnixSeconds(2_000),
            lead_seconds: 500,
        };
        assert_eq!(
            project_aging(
                AgingObligation::Open,
                business_day_evidence,
                UnixSeconds(1_600)
            ),
            Some(DeadlineState::Approaching)
        );
    }

    #[test]
    fn week_ages_against_the_retained_range_end() {
        let evidence = DeadlineAgingEvidence::Week {
            range_end: UnixSeconds(3_000),
            lead_seconds: 0,
        };
        assert_eq!(
            project_aging(AgingObligation::Open, evidence, UnixSeconds(2_999)),
            Some(DeadlineState::Scheduled)
        );
        assert_eq!(
            project_aging(AgingObligation::Open, evidence, UnixSeconds(3_000)),
            Some(DeadlineState::Overdue)
        );
    }

    #[test]
    fn event_relative_never_ages_until_a_unique_event_is_correlated() {
        let unresolved = DeadlineAgingEvidence::EventRelative {
            resolved_boundary: None,
            lead_seconds: 60,
        };
        assert_eq!(
            project_aging(AgingObligation::Open, unresolved, UnixSeconds(1_000_000)),
            Some(DeadlineState::Unresolved)
        );
        let resolved = DeadlineAgingEvidence::EventRelative {
            resolved_boundary: Some(UnixSeconds(500)),
            lead_seconds: 60,
        };
        assert_eq!(
            project_aging(AgingObligation::Open, resolved, UnixSeconds(500)),
            Some(DeadlineState::Overdue)
        );
    }

    #[test]
    fn soft_unspecified_and_undated_confirmed_never_age() {
        for evidence in [
            DeadlineAgingEvidence::Soft,
            DeadlineAgingEvidence::Unspecified,
        ] {
            assert_eq!(
                project_aging(AgingObligation::Open, evidence, UnixSeconds(i64::MAX)),
                Some(DeadlineState::Scheduled)
            );
        }
        assert_eq!(
            project_aging(
                AgingObligation::Open,
                DeadlineAgingEvidence::UndatedConfirmed,
                UnixSeconds(i64::MAX)
            ),
            Some(DeadlineState::UndatedConfirmed)
        );
    }

    #[test]
    fn every_operative_decision_table_row_matches_its_contract_outcome() {
        let rows = [
            (
                OperativeEvidenceEvent::SameMessageRequestedAndPromised {
                    actor_or_scope_ambiguous: false,
                },
                OperativeOutcome::PromisedOperativeOverRequested,
            ),
            (
                OperativeEvidenceEvent::SameMessageRequestedAndPromised {
                    actor_or_scope_ambiguous: true,
                },
                OperativeOutcome::ActorOrScopeAmbiguousRequiresReview,
            ),
            (
                OperativeEvidenceEvent::LaterUserExtension,
                OperativeOutcome::LaterUserExtensionOperativeRenegotiated,
            ),
            (
                OperativeEvidenceEvent::LaterRequesterDirectiveCounter,
                OperativeOutcome::LaterRequesterDirectiveOperativePreservingPriorExtension,
            ),
            (
                OperativeEvidenceEvent::LaterNonDirectiveDateMention,
                OperativeOutcome::NonDirectiveDateMentionNoChange,
            ),
            (
                OperativeEvidenceEvent::IndependentActionsSeparable,
                OperativeOutcome::IndependentActionsSplit,
            ),
            (
                OperativeEvidenceEvent::AmbiguousAttachmentToOneAction,
                OperativeOutcome::AmbiguousAttachmentPreservesAlternativesRequiresReview,
            ),
            (
                OperativeEvidenceEvent::ExplicitNoDeadline,
                OperativeOutcome::ExplicitNoDeadlineConfirmed,
            ),
            (
                OperativeEvidenceEvent::UnresolvedEventRelative,
                OperativeOutcome::EventRelativeUnresolved,
            ),
            (
                OperativeEvidenceEvent::EventRelativeCorrelated,
                OperativeOutcome::EventRelativeResolved,
            ),
            (
                OperativeEvidenceEvent::ImprecisePreserved,
                OperativeOutcome::PreservePrecisionNoUpgrade,
            ),
            (
                OperativeEvidenceEvent::CrossTimezoneAmbiguous,
                OperativeOutcome::CrossTimezoneAmbiguousRequiresReview,
            ),
        ];
        for (event, expected) in rows {
            assert_eq!(decide_operative(event), expected);
        }
    }

    #[test]
    fn defer_reminder_only_suppresses_the_prompt() {
        assert_eq!(
            effect_of(DeadlinePromptCommand::DeferReminder),
            DeadlinePromptEffect::SuppressPromptOnly
        );
    }

    #[test]
    fn every_prompt_command_maps_to_exactly_one_effect_domain() {
        for command in DeadlinePromptCommand::ALL {
            let effect = effect_of(command);
            match command {
                DeadlinePromptCommand::SetDeadline => {
                    assert_eq!(effect, DeadlinePromptEffect::DeadlineEvidence);
                }
                DeadlinePromptCommand::NoDeadline => {
                    assert_eq!(effect, DeadlinePromptEffect::UndatedConfirmed);
                }
                DeadlinePromptCommand::DeferReminder => {
                    assert_eq!(effect, DeadlinePromptEffect::SuppressPromptOnly);
                }
                DeadlinePromptCommand::Dismiss | DeadlinePromptCommand::NotMine => {
                    assert_eq!(effect, DeadlinePromptEffect::RoutesToCommandModule);
                }
            }
        }
    }
}
