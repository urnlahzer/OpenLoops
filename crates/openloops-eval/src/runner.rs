//! Runs the real, deterministic ADR-006/ADR-007/ADR-008 pipeline — the
//! canonicalizer, the ADR-007 validation steps with a mock provider
//! replaying each fixture's `mock_model_output_json`, then
//! `openloops_domain::establishment::decide` for every accepted claim that
//! declares an establishment case — over one fixture, and folds the result
//! into [`CategoryTally`]. This module calls the real
//! `openloops_inference`/`openloops_domain` entry points directly; it does
//! not reimplement any part of the pipeline.

use openloops_domain::deadline_parse::{
    DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT, ParseContext, TimezoneContext, Weekday,
};
use openloops_domain::establishment::{
    self, DetectionCase, EstablishmentOutcome, EvidenceValidity,
};
use openloops_domain::facets::{ReviewFlagCode, ReviewFlagSet};
use openloops_inference::message::{
    AttachmentPage, RawMessageInput, RawRecipient, canonicalize_message,
};
use openloops_inference::validation::{
    AnalysisResult, MessageContext, ParticipantHandle, ParticipantSlot, SuppliedContext, validate,
};

use crate::schema::{
    Category, EstablishmentInput, ExpectedTopLevel, ExpectedValidation, Fixture, FixtureSlot,
};

/// The fixed message handle every fixture's single message is issued under.
/// Fixtures never choose their own handle text (there is only ever one
/// message per fixture; see `schema.rs`'s `FixtureMessage` doc comment).
const MESSAGE_HANDLE: &str = "msg-1";

/// A deterministic, fixed baseline timestamp every fixture's temporal
/// hypotheses resolve against (`OL-DUE-002`: relative to the message's own
/// timestamp, never wall-clock "now" — using a fixed epoch keeps this
/// report reproducible across runs and machines).
fn parse_context() -> ParseContext {
    ParseContext {
        message_timestamp: openloops_domain::deadline::UnixSeconds(0),
        timezone: TimezoneContext {
            base_offset_seconds: 0,
            transition: None,
        },
        eod_seconds_since_midnight: DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT,
        week_start: Weekday::Monday,
    }
}

/// One category's aggregate, content-free counts.
#[derive(Clone, Copy, Debug, Default)]
pub struct CategoryTally {
    pub fixtures: u32,
    pub canonicalize_failed: u32,
    pub top_level_expected_match: u32,
    pub top_level_expected_mismatch: u32,
    pub claim_count_mismatch: u32,
    pub claim_validation_match: u32,
    pub claim_validation_mismatch: u32,
    pub establishment_open: u32,
    pub establishment_candidate: u32,
    pub establishment_pending_gate: u32,
    pub establishment_unchanged: u32,
    pub establishment_not_modeled: u32,
}

impl CategoryTally {
    fn merge(&mut self, other: &Self) {
        self.fixtures += other.fixtures;
        self.canonicalize_failed += other.canonicalize_failed;
        self.top_level_expected_match += other.top_level_expected_match;
        self.top_level_expected_mismatch += other.top_level_expected_mismatch;
        self.claim_count_mismatch += other.claim_count_mismatch;
        self.claim_validation_match += other.claim_validation_match;
        self.claim_validation_mismatch += other.claim_validation_mismatch;
        self.establishment_open += other.establishment_open;
        self.establishment_candidate += other.establishment_candidate;
        self.establishment_pending_gate += other.establishment_pending_gate;
        self.establishment_unchanged += other.establishment_unchanged;
        self.establishment_not_modeled += other.establishment_not_modeled;
    }
}

/// The complete report: one [`CategoryTally`] per [`Category`], in
/// [`Category::ALL`] order, plus the grand total.
pub struct Report {
    pub per_category: Vec<(Category, CategoryTally)>,
    pub total: CategoryTally,
}

fn build_recipients(source: &[crate::schema::FixtureRecipient]) -> Vec<RawRecipient<'_>> {
    source
        .iter()
        .map(|recipient| RawRecipient {
            name: &recipient.name,
            address: &recipient.address,
        })
        .collect()
}

fn establishment_case(input: &EstablishmentInput) -> DetectionCase {
    match input {
        EstablishmentInput::ExplicitOutgoingPromise {
            validity,
            confidence,
            core_ambiguity,
        } => DetectionCase::ExplicitOutgoingPromise {
            validity: bool_to_validity(*validity),
            confidence: (*confidence).into(),
            core_ambiguity: *core_ambiguity,
        },
        EstablishmentInput::DirectIncomingRequestOrQuestion {
            validity,
            deterministic_identity,
            confidence,
            core_ambiguity,
        } => DetectionCase::DirectIncomingRequestOrQuestion {
            validity: bool_to_validity(*validity),
            deterministic_identity: *deterministic_identity,
            confidence: (*confidence).into(),
            core_ambiguity: *core_ambiguity,
        },
        EstablishmentInput::AcknowledgementOfAssociatedRequest {
            validity,
            confidence,
            future_act_evidence,
        } => DetectionCase::AcknowledgementOfAssociatedRequest {
            validity: bool_to_validity(*validity),
            confidence: (*confidence).into(),
            future_act_evidence: *future_act_evidence,
        },
        EstablishmentInput::ExplicitDeterministicAttribution {
            validity,
            confidence,
            transfer_ambiguity,
        } => DetectionCase::ExplicitDeterministicAttribution {
            validity: bool_to_validity(*validity),
            confidence: (*confidence).into(),
            transfer_ambiguity: *transfer_ambiguity,
        },
        EstablishmentInput::CalendarInvitation => DetectionCase::CalendarInvitation,
        EstablishmentInput::SoftImpliedSocialOrContextual { ambiguity_flags } => {
            let mut flags = ReviewFlagSet::empty();
            for flag in ambiguity_flags {
                flags = flags.with(ReviewFlagCode::from(*flag));
            }
            DetectionCase::SoftImpliedSocialOrContextual {
                ambiguity_flags: flags,
            }
        }
        EstablishmentInput::UserConfirmsCandidateMineActionable => {
            DetectionCase::UserConfirmsCandidateMineActionable
        }
        EstablishmentInput::InvalidOrUngroundedHypothesis => {
            DetectionCase::InvalidOrUngroundedHypothesis
        }
    }
}

const fn bool_to_validity(validity: bool) -> EvidenceValidity {
    if validity {
        EvidenceValidity::Valid
    } else {
        EvidenceValidity::Invalid
    }
}

fn record_establishment(tally: &mut CategoryTally, outcome: EstablishmentOutcome) {
    match outcome {
        EstablishmentOutcome::Open { .. } => tally.establishment_open += 1,
        EstablishmentOutcome::Candidate { .. } => tally.establishment_candidate += 1,
        EstablishmentOutcome::PendingGate(_) => tally.establishment_pending_gate += 1,
        EstablishmentOutcome::Unchanged => tally.establishment_unchanged += 1,
    }
}

/// Canonicalizes `fixture`'s message and builds the [`SuppliedContext`]
/// [`validate`] needs, owning every backing `Vec` the borrowed context
/// slices point into.
struct FixtureContext<'a> {
    canonical_message: openloops_inference::message::CanonicalMessage,
    participants: Vec<ParticipantHandle<'a>>,
    loop_candidate_handles: Vec<&'a str>,
    temporal_context: ParseContext,
}

fn build_fixture_context(fixture: &Fixture) -> Option<FixtureContext<'_>> {
    let to = build_recipients(&fixture.message.to);
    let cc = build_recipients(&fixture.message.cc);
    let attachment_pages = [AttachmentPage {
        terminal: true,
        attachments: &[],
    }];
    let input = RawMessageInput {
        subject: &fixture.message.subject,
        body_html: &fixture.message.body_html,
        sender: Some(RawRecipient {
            name: &fixture.message.sender_name,
            address: &fixture.message.sender_address,
        }),
        from: None,
        to: &to,
        cc: &cc,
        attachment_pages: &attachment_pages,
    };
    let canonical_message = canonicalize_message(&input).ok()?;

    let participants = fixture
        .participants
        .iter()
        .map(|participant| ParticipantHandle {
            handle: participant.handle.as_str(),
            message_handle: MESSAGE_HANDLE,
            slot: match participant.slot {
                FixtureSlot::Sender => ParticipantSlot::Sender,
                FixtureSlot::To { index } => ParticipantSlot::To(usize::from(index)),
                FixtureSlot::Cc { index } => ParticipantSlot::Cc(usize::from(index)),
            },
        })
        .collect();
    let loop_candidate_handles = fixture
        .loop_candidate_handles
        .iter()
        .map(String::as_str)
        .collect();

    Some(FixtureContext {
        canonical_message,
        participants,
        loop_candidate_handles,
        temporal_context: parse_context(),
    })
}

/// Scores one claim verdict against its fixture expectation, updating
/// `tally` and running `establishment::decide` for a matched, accepted
/// claim that declares an establishment case.
fn score_claim(
    tally: &mut CategoryTally,
    verdict: openloops_inference::validation::ClaimVerdict,
    expected: &crate::schema::ExpectedClaim,
) {
    use openloops_inference::validation::ClaimDisposition;

    let matches_expectation = match (&verdict.disposition, &expected.validation) {
        (ClaimDisposition::AcceptForReview, ExpectedValidation::AcceptForReview) => true,
        (
            ClaimDisposition::Reject(reason),
            ExpectedValidation::Reject {
                reason_debug_contains,
            },
        ) => format!("{reason:?}").contains(reason_debug_contains.as_str()),
        _ => false,
    };
    if !matches_expectation {
        tally.claim_validation_mismatch += 1;
        return;
    }
    tally.claim_validation_match += 1;

    if matches!(verdict.disposition, ClaimDisposition::AcceptForReview) {
        match &expected.establishment {
            Some(establishment_input) => {
                let decided = establishment::decide(establishment_case(establishment_input));
                record_establishment(tally, decided);
            }
            None => tally.establishment_not_modeled += 1,
        }
    }
}

/// Runs the real pipeline over one fixture and returns its [`CategoryTally`]
/// contribution. Never panics on a malformed fixture body — a canonicalize
/// failure is recorded, not propagated, so one bad fixture cannot abort the
/// whole corpus run.
#[must_use]
pub fn run_fixture(fixture: &Fixture) -> CategoryTally {
    let mut tally = CategoryTally {
        fixtures: 1,
        ..CategoryTally::default()
    };

    let Some(built) = build_fixture_context(fixture) else {
        tally.canonicalize_failed = 1;
        return tally;
    };
    let messages = [MessageContext {
        handle: MESSAGE_HANDLE,
        message: &built.canonical_message,
        temporal_context: built.temporal_context,
    }];
    let context = SuppliedContext {
        messages: &messages,
        participants: &built.participants,
        loop_candidate_handles: &built.loop_candidate_handles,
    };

    let outcome = validate(fixture.mock_model_output_json.as_bytes(), &context);

    let expected_reviewed = matches!(fixture.expected_top_level, ExpectedTopLevel::Reviewed);
    let actually_reviewed = matches!(outcome, AnalysisResult::Reviewed(_));
    if expected_reviewed != actually_reviewed {
        tally.top_level_expected_mismatch = 1;
        return tally;
    }
    tally.top_level_expected_match = 1;

    let AnalysisResult::Reviewed(reviewed) = outcome else {
        return tally;
    };
    if reviewed.verdicts.len() != fixture.expected_claims.len() {
        tally.claim_count_mismatch = 1;
        return tally;
    }

    for (verdict, expected) in reviewed.verdicts.iter().zip(fixture.expected_claims.iter()) {
        score_claim(&mut tally, *verdict, expected);
    }

    tally
}

/// Runs every fixture in `fixtures`, grouping by [`Category`] in
/// [`Category::ALL`] order.
#[must_use]
pub fn run_corpus(fixtures: &[Fixture]) -> Report {
    let mut per_category: Vec<(Category, CategoryTally)> = Category::ALL
        .into_iter()
        .map(|category| (category, CategoryTally::default()))
        .collect();
    let mut total = CategoryTally::default();

    for fixture in fixtures {
        let tally = run_fixture(fixture);
        total.merge(&tally);
        if let Some((_, entry)) = per_category
            .iter_mut()
            .find(|(category, _)| *category == fixture.category)
        {
            entry.merge(&tally);
        }
    }

    Report {
        per_category,
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::run_fixture;
    use crate::schema::{
        Category, ConfidenceLabel, EstablishmentInput, ExpectedClaim, ExpectedTopLevel,
        ExpectedValidation, Fixture, FixtureMessage,
    };

    fn base_message() -> FixtureMessage {
        FixtureMessage {
            subject: "Weekly report".to_string(),
            body_html: "<p>Please send the report by Friday.</p>".to_string(),
            sender_name: "Alex Synthetic".to_string(),
            sender_address: "alex@synthetic.invalid".to_string(),
            to: Vec::new(),
            cc: Vec::new(),
        }
    }

    #[test]
    fn an_accepted_claim_runs_canonicalizer_validation_and_establishment_for_real() {
        let fixture = Fixture {
            id: "test-accept".to_string(),
            category: Category::DirectRequest,
            message: base_message(),
            participants: Vec::new(),
            loop_candidate_handles: Vec::new(),
            mock_model_output_json: r#"{"schema_version":1,"claims":[{"claim_type":"request","evidence":[{"source_handle":"msg-1","component":"body_block","block_ordinal":0,"range_start":0,"range_end":6}],"waiting_party_handle":null,"related_loop_handles":[],"temporal":null,"confidence_micros":900000,"ambiguity_codes":[]}]}"#.to_string(),
            expected_top_level: ExpectedTopLevel::Reviewed,
            expected_claims: vec![ExpectedClaim {
                validation: ExpectedValidation::AcceptForReview,
                establishment: Some(EstablishmentInput::DirectIncomingRequestOrQuestion {
                    validity: true,
                    deterministic_identity: true,
                    confidence: ConfidenceLabel::High,
                    core_ambiguity: false,
                }),
            }],
        };
        let tally = run_fixture(&fixture);
        assert_eq!(tally.canonicalize_failed, 0);
        assert_eq!(tally.top_level_expected_match, 1);
        assert_eq!(tally.claim_validation_match, 1);
        assert_eq!(tally.claim_validation_mismatch, 0);
        assert_eq!(tally.establishment_open, 1);
    }

    #[test]
    fn malformed_provider_bytes_are_recognized_as_analysis_unavailable() {
        let fixture = Fixture {
            id: "test-malformed".to_string(),
            category: Category::InvalidModelOutput,
            message: base_message(),
            participants: Vec::new(),
            loop_candidate_handles: Vec::new(),
            mock_model_output_json: "not json".to_string(),
            expected_top_level: ExpectedTopLevel::AnalysisUnavailable,
            expected_claims: Vec::new(),
        };
        let tally = run_fixture(&fixture);
        assert_eq!(tally.top_level_expected_match, 1);
        assert_eq!(tally.top_level_expected_mismatch, 0);
    }

    #[test]
    fn a_wrong_expectation_is_recorded_as_a_mismatch_not_a_panic() {
        // Evidence cites a message handle never issued for this request
        // (the hostile/malformed-model case); the fixture nonetheless
        // (incorrectly) expects acceptance, so this proves a wrong
        // expectation is tallied, never silently accepted or panicked on.
        let fixture = Fixture {
            id: "test-mismatch".to_string(),
            category: Category::HostilePromptInjection,
            message: base_message(),
            participants: Vec::new(),
            loop_candidate_handles: Vec::new(),
            mock_model_output_json: r#"{"schema_version":1,"claims":[{"claim_type":"request","evidence":[{"source_handle":"phantom-message","component":"body_block","block_ordinal":0,"range_start":0,"range_end":6}],"waiting_party_handle":null,"related_loop_handles":[],"temporal":null,"confidence_micros":900000,"ambiguity_codes":[]}]}"#.to_string(),
            expected_top_level: ExpectedTopLevel::Reviewed,
            expected_claims: vec![ExpectedClaim {
                validation: ExpectedValidation::AcceptForReview,
                establishment: None,
            }],
        };
        let tally = run_fixture(&fixture);
        assert_eq!(tally.top_level_expected_match, 1);
        assert_eq!(tally.claim_validation_match, 0);
        assert_eq!(tally.claim_validation_mismatch, 1);
    }
}
