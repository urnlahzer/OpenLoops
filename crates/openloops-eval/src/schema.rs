//! The typed, closed fixture schema `fixtures/synthetic/*.json` files parse
//! against. Every struct/enum here `deny_unknown_fields` (or is a closed
//! enum) so a malformed or drifted fixture fails the parse loudly rather
//! than silently dropping a field, matching this workspace's established
//! `openloops-contracts` pattern rather than reinventing a laxer one.
//!
//! This is development-tooling schema, not a product contract: it has no
//! `contracts/*.json` counterpart and backs no acceptance criterion or
//! gate.

use serde::Deserialize;

/// The brief's closed category catalog. `ALL` gives the report an
/// exhaustive, compile-time-checked iteration order so a category can never
/// silently go unreported.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    DirectRequest,
    Promise,
    Acknowledgment,
    SoftImplied,
    QuoteDuplicateTrap,
    ForwardedReassignment,
    MultiActionSplit,
    DeadlineRelative,
    DeadlineAmbiguous,
    DeadlineEventRelative,
    ClosureCandidate,
    Delegation,
    HostilePromptInjection,
    InvalidModelOutput,
}

impl Category {
    pub const ALL: [Self; 14] = [
        Self::DirectRequest,
        Self::Promise,
        Self::Acknowledgment,
        Self::SoftImplied,
        Self::QuoteDuplicateTrap,
        Self::ForwardedReassignment,
        Self::MultiActionSplit,
        Self::DeadlineRelative,
        Self::DeadlineAmbiguous,
        Self::DeadlineEventRelative,
        Self::ClosureCandidate,
        Self::Delegation,
        Self::HostilePromptInjection,
        Self::InvalidModelOutput,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::DirectRequest => "direct_request",
            Self::Promise => "promise",
            Self::Acknowledgment => "acknowledgment",
            Self::SoftImplied => "soft_implied",
            Self::QuoteDuplicateTrap => "quote_duplicate_trap",
            Self::ForwardedReassignment => "forwarded_reassignment",
            Self::MultiActionSplit => "multi_action_split",
            Self::DeadlineRelative => "deadline_relative",
            Self::DeadlineAmbiguous => "deadline_ambiguous",
            Self::DeadlineEventRelative => "deadline_event_relative",
            Self::ClosureCandidate => "closure_candidate",
            Self::Delegation => "delegation",
            Self::HostilePromptInjection => "hostile_prompt_injection",
            Self::InvalidModelOutput => "invalid_model_output",
        }
    }
}

/// One raw participant, matching `openloops_inference::message::RawRecipient`
/// field-for-field but owned (`String`) so it can be parsed from JSON.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureRecipient {
    pub name: String,
    pub address: String,
}

/// The raw message one fixture canonicalizes. Deliberately restricted to at
/// most one body paragraph and one blockquote (see this crate's
/// `corpus.rs`): every fixture's `body_html` is exactly `<p>SENTENCE</p>` or
/// `<p>SENTENCE</p><blockquote>SENTENCE</blockquote>`, so its canonical
/// `body_blocks[0]`/`quote_blocks[0]` text is always exactly the sentence
/// text with no whitespace-merge arithmetic to reproduce by hand.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureMessage {
    pub subject: String,
    pub body_html: String,
    pub sender_name: String,
    pub sender_address: String,
    #[serde(default)]
    pub to: Vec<FixtureRecipient>,
    #[serde(default)]
    pub cc: Vec<FixtureRecipient>,
}

/// One participant handle the fixture's mock context "issues", naming a
/// slot the *actual* canonicalized message may or may not structurally
/// have (letting a fixture deliberately exercise
/// `ParticipantSlotFailure::SlotDoesNotExist`, mirroring
/// `openloops-inference`'s own
/// `participant_ref_to_a_nonexistent_slot_is_rejected` test).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureParticipant {
    pub handle: String,
    pub slot: FixtureSlot,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FixtureSlot {
    Sender,
    To { index: u16 },
    Cc { index: u16 },
}

/// The top-level ADR-007 validation outcome a fixture expects.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedTopLevel {
    AnalysisUnavailable,
    Reviewed,
}

/// One claim's expected validation disposition. `reason_debug_contains` is
/// matched as a substring against `format!("{:?}", rejection)` — the
/// rejection enum's `Debug` output is itself content-free (variant names
/// only; see `openloops_inference::validation::ClaimRejectionReason`), so
/// this never echoes fixture content.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpectedValidation {
    AcceptForReview,
    Reject { reason_debug_contains: String },
}

/// A confidence label mirroring `openloops_domain::facets::ConfidenceBucket`,
/// owned/parseable from JSON.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceLabel {
    Low,
    Medium,
    High,
}

impl From<ConfidenceLabel> for openloops_domain::facets::ConfidenceBucket {
    fn from(label: ConfidenceLabel) -> Self {
        match label {
            ConfidenceLabel::Low => Self::Low,
            ConfidenceLabel::Medium => Self::Medium,
            ConfidenceLabel::High => Self::High,
        }
    }
}

/// A review-flag label mirroring `openloops_domain::facets::ReviewFlagCode`.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewFlagLabel {
    NeedsReview,
    HistoricalBackfill,
    IdentityAmbiguous,
    AssociationAmbiguous,
    QuoteAmbiguous,
    DeadlineAmbiguous,
    DelegationAmbiguous,
}

impl From<ReviewFlagLabel> for openloops_domain::facets::ReviewFlagCode {
    fn from(label: ReviewFlagLabel) -> Self {
        match label {
            ReviewFlagLabel::NeedsReview => Self::NeedsReview,
            ReviewFlagLabel::HistoricalBackfill => Self::HistoricalBackfill,
            ReviewFlagLabel::IdentityAmbiguous => Self::IdentityAmbiguous,
            ReviewFlagLabel::AssociationAmbiguous => Self::AssociationAmbiguous,
            ReviewFlagLabel::QuoteAmbiguous => Self::QuoteAmbiguous,
            ReviewFlagLabel::DeadlineAmbiguous => Self::DeadlineAmbiguous,
            ReviewFlagLabel::DelegationAmbiguous => Self::DelegationAmbiguous,
        }
    }
}

/// The exact `establishment_policy` row (`openloops_domain::establishment::
/// DetectionCase`) a fixture asserts applies to one accepted claim, with the
/// typed inputs that row's predicate reads. Present only for claim types the
/// domain crate models an establishment row for; a fixture that leaves this
/// `null` is declaring "this claim type has no establishment-policy row in
/// this crate" (e.g. `possible_closure`, `question`, `modification` — an
/// honest, documented gap, not an oversight; see this crate's `README.md`).
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "case", rename_all = "snake_case", deny_unknown_fields)]
pub enum EstablishmentInput {
    ExplicitOutgoingPromise {
        validity: bool,
        confidence: ConfidenceLabel,
        core_ambiguity: bool,
    },
    DirectIncomingRequestOrQuestion {
        validity: bool,
        deterministic_identity: bool,
        confidence: ConfidenceLabel,
        core_ambiguity: bool,
    },
    AcknowledgementOfAssociatedRequest {
        validity: bool,
        confidence: ConfidenceLabel,
        future_act_evidence: bool,
    },
    ExplicitDeterministicAttribution {
        validity: bool,
        confidence: ConfidenceLabel,
        transfer_ambiguity: bool,
    },
    CalendarInvitation,
    SoftImpliedSocialOrContextual {
        ambiguity_flags: Vec<ReviewFlagLabel>,
    },
    UserConfirmsCandidateMineActionable,
    InvalidOrUngroundedHypothesis,
}

/// One expected claim verdict, in the same order as `mock_model_output_json`
/// declares its `claims` array.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedClaim {
    pub validation: ExpectedValidation,
    #[serde(default)]
    pub establishment: Option<EstablishmentInput>,
}

/// One complete fixture file.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub id: String,
    pub category: Category,
    pub message: FixtureMessage,
    #[serde(default)]
    pub participants: Vec<FixtureParticipant>,
    #[serde(default)]
    pub loop_candidate_handles: Vec<String>,
    /// The exact bytes a mock model provider "replays" for this fixture,
    /// already serialized as `analysis-output-v1` JSON text (schema
    /// version plus a `claims` array) — see
    /// `contracts/model/analysis-output.schema.json`.
    pub mock_model_output_json: String,
    pub expected_top_level: ExpectedTopLevel,
    #[serde(default)]
    pub expected_claims: Vec<ExpectedClaim>,
}
