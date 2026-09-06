//! The ADR-007 `provider-boundary.json` `response_contract.
//! required_validation_order` ten-step validation pipeline, steps 4-10.
//!
//! `crates/openloops-contracts` already implements steps 1-3 (strict UTF-8
//! and exactly one JSON value; duplicate-member and unknown-field rejection;
//! schema, numeric, and collection bounds) in
//! [`openloops_contracts::parse_analysis_output`]; [`validate`] calls it
//! rather than duplicating it. This module implements only the steps that
//! need application-supplied context `openloops-contracts` deliberately does
//! not have: the opaque handles the application actually issued, the
//! transient canonical message text, participant-slot membership, and
//! deterministic date reparsing.
//!
//! The model is an untrusted transient hypothesis producer
//! (`docs/adr/ADR-007-model-boundary.md`): nothing in this module can
//! authorize a mutation, and every accepted claim is
//! [`ClaimDisposition::AcceptForReview`] — review-only, never automatic,
//! regardless of `confidence_micros` — because Phase 0 has no automation
//! path at all (`model-sensitivity-v1`'s `review_thresholds.safe_default` is
//! `review_required` for every claim type, and G-MODEL/G-AUTO remain
//! unpassed).
//!
//! Every step below runs in the contract's exact order, and a claim that
//! fails one step is rejected without any later step running for that claim
//! (fail-closed short-circuiting). A document that fails steps 1-3 yields
//! zero accepted claims via [`AnalysisResult::AnalysisUnavailable`].

use std::collections::HashSet;

use openloops_contracts::{
    AmbiguityCode, AnalysisOutput, Claim, ClaimType, EvidenceComponent, Nullable, ParseRejection,
    TemporalKind as SchemaTemporalKind, parse_analysis_output,
};
use openloops_domain::deadline_parse::{self, ParseContext, TemporalKind as DomainTemporalKind};

use crate::blocks::CanonicalBlock;
use crate::error::RangeError;
use crate::message::CanonicalMessage;

// ---------------------------------------------------------------------------
// Caller-supplied context (steps 4 and 7's "opaque handles supplied by the
// application" and "actual canonical message").
// ---------------------------------------------------------------------------

/// One canonicalized message the provider payload was built from, keyed by
/// the opaque message handle the application issued for it, plus the
/// [`ParseContext`] step 8's reparse resolves this message's temporal
/// hypotheses against (`OL-DUE-002`: relative to *this* message's own
/// timestamp, never the wall-clock "now").
pub struct MessageContext<'a> {
    /// The opaque `evidence_range.source_handle` value the application
    /// issued for this message.
    pub handle: &'a str,
    pub message: &'a CanonicalMessage,
    pub temporal_context: ParseContext,
}

/// One participant slot's opaque handle, as issued by the application for
/// exactly one [`ParticipantSlot`] of exactly one message.
pub struct ParticipantHandle<'a> {
    pub handle: &'a str,
    pub message_handle: &'a str,
    pub slot: ParticipantSlot,
}

/// Which participant slot a [`ParticipantHandle`] names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParticipantSlot {
    Sender,
    To(usize),
    Cc(usize),
}

/// Every opaque handle set the application supplied for one validation call:
/// the messages actually sent in this request, the participant slots issued
/// for them, and the existing-loop candidate handles offered as
/// `related_loop_handles` targets. [`validate`] never trusts a claim's own
/// handle text beyond membership/structural checks against this context.
pub struct SuppliedContext<'a> {
    pub messages: &'a [MessageContext<'a>],
    pub participants: &'a [ParticipantHandle<'a>],
    pub loop_candidate_handles: &'a [&'a str],
}

impl<'a> SuppliedContext<'a> {
    fn find_message(&self, handle: &str) -> Option<&MessageContext<'a>> {
        self.messages
            .iter()
            .find(|context| context.handle == handle)
    }

    fn find_participant(&self, handle: &str) -> Option<&ParticipantHandle<'a>> {
        self.participants
            .iter()
            .find(|participant| participant.handle == handle)
    }

    fn has_loop_candidate(&self, handle: &str) -> bool {
        self.loop_candidate_handles.contains(&handle)
    }
}

// ---------------------------------------------------------------------------
// Typed, content-free per-claim disposition.
// ---------------------------------------------------------------------------

/// Step 4's specific membership failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandleMembershipFailure {
    /// An `evidence[].source_handle` was not among the message handles the
    /// application actually issued for this request.
    UnknownMessageHandle,
    /// A `related_loop_handles` entry was not among the existing-loop
    /// candidate handles the application actually offered.
    UnknownLoopCandidateHandle,
    /// `waiting_party_handle` was not among the participant handles the
    /// application actually issued.
    UnknownParticipantHandle,
}

/// Step 5/6's specific canonical-bounds/correspondence failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceBoundsFailure {
    /// `component`/`block_ordinal` does not address a real block in the
    /// referenced message's canonical projection.
    BlockOrdinalOutOfRange,
    /// `range_start >= range_end` against the addressed block.
    EmptyRange,
    /// `range_end` exceeds the addressed block's Unicode scalar count.
    RangeOutOfBounds,
    /// The range is in bounds and non-empty, but every scalar in it is
    /// whitespace: there is no actual evidentiary content at this range.
    WhitespaceOnlyEvidence,
}

/// Step 7's specific participant-slot failure (existence in the supplied
/// handle catalog was already step 4's job; this is the structural
/// re-derivation against the *current* canonical message).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParticipantSlotFailure {
    /// The participant handle's own `message_handle` is not a message
    /// handle the application actually issued.
    UnknownMessageHandle,
    /// The named slot does not exist in that message's canonical
    /// projection (e.g. `To(3)` when the message has only two `to`
    /// recipients).
    SlotDoesNotExist,
}

/// Step 8's failure: the deterministic parser could not reproduce the
/// model's temporal interpretation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemporalReparseFailure {
    /// `text_evidence_index` did not address a real entry in this claim's
    /// own `evidence` array.
    TextEvidenceIndexOutOfRange,
    /// `openloops_domain::deadline_parse::reparse` rejected the value under
    /// the claim's declared `kind`.
    Unreproducible,
}

/// Step 9's specific internal-consistency failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsistencyFailure {
    /// This claim is structurally identical to an earlier claim in the same
    /// document (same type, evidence, participant, temporal hypothesis).
    DuplicateClaim,
    /// This claim's evidence spans more than one distinct message handle
    /// without disclosing `ambiguity_codes: [.., cross_message, ..]`.
    UndisclosedCrossMessageEvidence,
    /// Two `deadline_change` claims share a non-empty, identical
    /// `related_loop_handles` set but the deterministic parser resolves
    /// their temporal hypotheses to different instants.
    ContradictoryDeadlineRelation,
}

/// Every closed reason one claim can be rejected for, one variant per
/// pipeline step. Every variant is content-free: none carries source text,
/// a handle value, or a range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimRejectionReason {
    /// Step 4.
    HandleMembership(HandleMembershipFailure),
    /// Steps 5-6.
    EvidenceBounds(EvidenceBoundsFailure),
    /// Step 7.
    ParticipantSlot(ParticipantSlotFailure),
    /// Step 8.
    TemporalReparse(TemporalReparseFailure),
    /// Step 9.
    Consistency(ConsistencyFailure),
}

/// One claim's final disposition. `AcceptForReview` is never a mutation
/// authority: it means only that a human may be shown this candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimDisposition {
    AcceptForReview,
    Reject(ClaimRejectionReason),
}

/// One claim's disposition, alongside its already-validated, closed
/// `claim_type` (categorical only; never source text).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClaimVerdict {
    pub claim_type: ClaimType,
    pub disposition: ClaimDisposition,
}

/// Every claim's [`ClaimVerdict`], in the same order as the parsed
/// document's `claims` array.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationOutcome {
    pub verdicts: Vec<ClaimVerdict>,
}

impl ValidationOutcome {
    /// The number of claims with [`ClaimDisposition::AcceptForReview`].
    #[must_use]
    pub fn accepted_count(&self) -> usize {
        self.verdicts
            .iter()
            .filter(|verdict| matches!(verdict.disposition, ClaimDisposition::AcceptForReview))
            .count()
    }
}

/// The top-level result of validating one provider response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnalysisResult {
    /// Steps 1-3 (`openloops_contracts::parse_analysis_output`) rejected the
    /// input: malformed, oversized, or schema-invalid. Zero claims are ever
    /// considered; nothing here carries the parse rejection's own content
    /// (it is already content-free) beyond the fact that it happened.
    AnalysisUnavailable,
    /// The document parsed; every claim received an individual
    /// [`ClaimVerdict`] from steps 4-10.
    Reviewed(ValidationOutcome),
}

// ---------------------------------------------------------------------------
// Steps 5-6: canonical-block/range resolution, reusing the canonicalizer's
// own bounded types rather than parallel definitions.
// ---------------------------------------------------------------------------

pub(crate) fn resolve_block(
    message: &CanonicalMessage,
    component: EvidenceComponent,
    block_ordinal: u16,
) -> Option<&CanonicalBlock> {
    let ordinal = usize::from(block_ordinal);
    match component {
        EvidenceComponent::Subject => (ordinal == 0).then_some(&message.subject),
        EvidenceComponent::BodyBlock => message.body_blocks.get(ordinal),
        EvidenceComponent::QuoteBlock => message.quote_blocks.get(ordinal),
        EvidenceComponent::Sender => {
            if ordinal == 0 {
                message.sender.as_ref()
            } else {
                None
            }
        }
        EvidenceComponent::To => message.to.get(ordinal),
        EvidenceComponent::Cc => message.cc.get(ordinal),
        EvidenceComponent::AttachmentName => message.attachment_names.get(ordinal),
        EvidenceComponent::LinkLabel => message.link_labels.get(ordinal),
    }
}

fn participant_slot_exists(message: &CanonicalMessage, slot: ParticipantSlot) -> bool {
    match slot {
        ParticipantSlot::Sender => message.sender.is_some(),
        ParticipantSlot::To(index) => index < message.to.len(),
        ParticipantSlot::Cc(index) => index < message.cc.len(),
    }
}

// ---------------------------------------------------------------------------
// Step 8's kind bridge: maps the schema-derived catalog onto the
// dependency-free domain catalog through an exhaustive match with no
// wildcard arm, keeping the two variant sets in lockstep at compile time.
// ---------------------------------------------------------------------------

const fn map_temporal_kind(kind: SchemaTemporalKind) -> DomainTemporalKind {
    match kind {
        SchemaTemporalKind::Date => DomainTemporalKind::Date,
        SchemaTemporalKind::LocalDatetime => DomainTemporalKind::LocalDatetime,
        SchemaTemporalKind::Relative => DomainTemporalKind::Relative,
        SchemaTemporalKind::EventRelative => DomainTemporalKind::EventRelative,
        SchemaTemporalKind::SoftWindow => DomainTemporalKind::SoftWindow,
    }
}

// ---------------------------------------------------------------------------
// Steps 4-8: per-claim, short-circuiting at the first failing step.
// ---------------------------------------------------------------------------

fn check_steps_4_to_8(
    claim: &Claim,
    context: &SuppliedContext,
) -> Result<(), ClaimRejectionReason> {
    // Step 4: supplied-handle membership.
    for evidence in &claim.evidence {
        if context.find_message(&evidence.source_handle).is_none() {
            return Err(ClaimRejectionReason::HandleMembership(
                HandleMembershipFailure::UnknownMessageHandle,
            ));
        }
    }
    for related in &claim.related_loop_handles {
        if !context.has_loop_candidate(related) {
            return Err(ClaimRejectionReason::HandleMembership(
                HandleMembershipFailure::UnknownLoopCandidateHandle,
            ));
        }
    }
    if let Nullable::Value(handle) = &claim.waiting_party_handle
        && context.find_participant(handle).is_none()
    {
        return Err(ClaimRejectionReason::HandleMembership(
            HandleMembershipFailure::UnknownParticipantHandle,
        ));
    }

    // Steps 5-6: canonical-block/range bounds, then evidence-text
    // correspondence (the extracted range must be non-empty content, not
    // only whitespace).
    for evidence in &claim.evidence {
        let message_context = context
            .find_message(&evidence.source_handle)
            .expect("step 4 already proved every evidence source_handle resolves");
        let block = resolve_block(
            message_context.message,
            evidence.component,
            evidence.block_ordinal,
        )
        .ok_or(ClaimRejectionReason::EvidenceBounds(
            EvidenceBoundsFailure::BlockOrdinalOutOfRange,
        ))?;
        let extracted = block
            .range_text(evidence.range_start, evidence.range_end)
            .map_err(|range_error| {
                ClaimRejectionReason::EvidenceBounds(match range_error {
                    RangeError::EmptyRange => EvidenceBoundsFailure::EmptyRange,
                    RangeError::OutOfBounds => EvidenceBoundsFailure::RangeOutOfBounds,
                })
            })?;
        if extracted.trim().is_empty() {
            return Err(ClaimRejectionReason::EvidenceBounds(
                EvidenceBoundsFailure::WhitespaceOnlyEvidence,
            ));
        }
    }

    // Step 7: participant-position membership. Existence in the supplied
    // handle catalog was step 4; this re-derives that the handle's slot
    // still exists structurally in the actual canonical message.
    if let Nullable::Value(handle) = &claim.waiting_party_handle {
        let participant = context
            .find_participant(handle)
            .expect("step 4 already proved this handle resolves");
        let message_context = context.find_message(participant.message_handle).ok_or(
            ClaimRejectionReason::ParticipantSlot(ParticipantSlotFailure::UnknownMessageHandle),
        )?;
        if !participant_slot_exists(message_context.message, participant.slot) {
            return Err(ClaimRejectionReason::ParticipantSlot(
                ParticipantSlotFailure::SlotDoesNotExist,
            ));
        }
    }

    // Step 8: deterministic date reparse. A model-proposed interpretation
    // the deterministic parser cannot reproduce is never accepted on the
    // model's authority alone.
    if let Nullable::Value(temporal) = &claim.temporal {
        let evidence_index = usize::from(temporal.text_evidence_index);
        let evidence_entry =
            claim
                .evidence
                .get(evidence_index)
                .ok_or(ClaimRejectionReason::TemporalReparse(
                    TemporalReparseFailure::TextEvidenceIndexOutOfRange,
                ))?;
        let message_context = context
            .find_message(&evidence_entry.source_handle)
            .expect("step 4 already proved every evidence source_handle resolves");
        deadline_parse::reparse(
            map_temporal_kind(temporal.kind),
            &temporal.value,
            &message_context.temporal_context,
        )
        .map_err(|_| {
            ClaimRejectionReason::TemporalReparse(TemporalReparseFailure::Unreproducible)
        })?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Step 9: internal claim/relation consistency, over the claims that
// survived steps 4-8.
// ---------------------------------------------------------------------------

fn distinct_message_handles(claim: &Claim) -> HashSet<&str> {
    claim
        .evidence
        .iter()
        .map(|evidence| evidence.source_handle.as_str())
        .collect()
}

fn apply_step_9_consistency(
    claims: &[Claim],
    dispositions: &mut [Option<ClaimRejectionReason>],
    context: &SuppliedContext,
) {
    // Duplicate claims: reject every later structurally-identical claim,
    // keeping the earliest occurrence.
    for later_index in 0..claims.len() {
        if dispositions[later_index].is_some() {
            continue;
        }
        let is_duplicate = (0..later_index).any(|earlier_index| {
            dispositions[earlier_index].is_none() && claims[earlier_index] == claims[later_index]
        });
        if is_duplicate {
            dispositions[later_index] = Some(ClaimRejectionReason::Consistency(
                ConsistencyFailure::DuplicateClaim,
            ));
        }
    }

    // Undisclosed cross-message evidence: evidence spanning more than one
    // message handle must self-disclose `ambiguity_codes: cross_message`.
    for (index, claim) in claims.iter().enumerate() {
        if dispositions[index].is_some() {
            continue;
        }
        let message_handles = distinct_message_handles(claim);
        let discloses_cross_message = claim.ambiguity_codes.contains(&AmbiguityCode::CrossMessage);
        if message_handles.len() > 1 && !discloses_cross_message {
            dispositions[index] = Some(ClaimRejectionReason::Consistency(
                ConsistencyFailure::UndisclosedCrossMessageEvidence,
            ));
        }
    }

    // Contradictory deadline relations: two surviving `deadline_change`
    // claims that target the same non-empty `related_loop_handles` set but
    // deterministically reparse to different instants.
    let deadline_change_indices: Vec<usize> = (0..claims.len())
        .filter(|&index| {
            dispositions[index].is_none() && claims[index].claim_type == ClaimType::DeadlineChange
        })
        .collect();
    for (position, &first_index) in deadline_change_indices.iter().enumerate() {
        for &second_index in &deadline_change_indices[position + 1..] {
            if dispositions[first_index].is_some() || dispositions[second_index].is_some() {
                continue;
            }
            if claims[first_index].related_loop_handles.is_empty()
                || claims[first_index].related_loop_handles
                    != claims[second_index].related_loop_handles
            {
                continue;
            }
            let Some(first_instant) = resolved_instant(&claims[first_index], context) else {
                continue;
            };
            let Some(second_instant) = resolved_instant(&claims[second_index], context) else {
                continue;
            };
            if first_instant != second_instant {
                dispositions[first_index] = Some(ClaimRejectionReason::Consistency(
                    ConsistencyFailure::ContradictoryDeadlineRelation,
                ));
                dispositions[second_index] = Some(ClaimRejectionReason::Consistency(
                    ConsistencyFailure::ContradictoryDeadlineRelation,
                ));
            }
        }
    }
}

/// Reparses `claim`'s temporal hypothesis (if any) to a concrete instant,
/// for the sole purpose of comparing two `deadline_change` claims in step 9.
/// Returns `None` for a claim with no temporal hypothesis, an
/// unreproducible one (step 8 already rejects those independently), or one
/// that resolves to anything other than a single unambiguous instant
/// (`date`/`week`/`soft`/`event_relative`/ambiguous values carry nothing a
/// same-instant comparison could meaningfully contradict).
fn resolved_instant(
    claim: &Claim,
    context: &SuppliedContext,
) -> Option<openloops_domain::deadline::UnixSeconds> {
    let temporal = match &claim.temporal {
        Nullable::Value(temporal) => temporal,
        Nullable::Null => return None,
    };
    let evidence_index = usize::from(temporal.text_evidence_index);
    let evidence_entry = claim.evidence.get(evidence_index)?;
    let message_context = context.find_message(&evidence_entry.source_handle)?;
    let parsed = deadline_parse::reparse(
        map_temporal_kind(temporal.kind),
        &temporal.value,
        &message_context.temporal_context,
    )
    .ok()?;
    match parsed {
        deadline_parse::ParsedValue::Instant(deadline_parse::LocalResolution::Unambiguous(
            instant,
        )) => Some(instant),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Step 10: semantic-adversarial and deterministic positive-policy routing.
// ---------------------------------------------------------------------------

/// The closed `model-sensitivity-v1` routing decision for one claim type.
/// Every claim type routes to [`Self::ReviewRequired`]: Phase 0 has no
/// automation path (`ADR-011`/`G-AUTO` remain unimplemented), so there is no
/// positive policy this function could route a claim through instead —
/// `route` exists to make that invariant an exhaustive, closed match rather
/// than an assumption, so adding a new `ClaimType` without updating this
/// function fails to compile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoutingDecision {
    ReviewRequired,
}

/// Deliberately ignores `confidence_micros`: `provider-boundary.json`
/// `semantic_policy.schema_valid_is_not_safe` and
/// `settings_disclosure.review_thresholds.safe_default` both fix
/// `review_required` regardless of any confidence value the model reports,
/// so no confidence input exists in this function's signature at all.
#[must_use]
pub const fn route(claim_type: ClaimType) -> RoutingDecision {
    match claim_type {
        ClaimType::Request
        | ClaimType::Promise
        | ClaimType::Attribution
        | ClaimType::Question
        | ClaimType::DeadlineChange
        | ClaimType::PossibleClosure
        | ClaimType::Delegation
        | ClaimType::Modification => RoutingDecision::ReviewRequired,
    }
}

// ---------------------------------------------------------------------------
// The top-level pipeline.
// ---------------------------------------------------------------------------

/// Runs the complete ADR-007 ten-step validation pipeline over one provider
/// response.
///
/// Steps 1-3 are [`openloops_contracts::parse_analysis_output`]; a rejection
/// there yields [`AnalysisResult::AnalysisUnavailable`] with zero claims ever
/// considered. Otherwise every claim is checked against steps 4-8
/// (short-circuiting at the first failing step), then step 9 runs across
/// every claim that survived steps 4-8, then step 10 assigns the closed
/// review-routing decision to every claim that survived step 9. No code path
/// here can let a claim influence anything but its own review-candidacy.
#[must_use]
pub fn validate(bytes: &[u8], context: &SuppliedContext) -> AnalysisResult {
    let output: AnalysisOutput = match parse_analysis_output(bytes) {
        Ok(output) => output,
        Err(
            ParseRejection::ResponseTooLarge
            | ParseRejection::InvalidUtf8
            | ParseRejection::InvalidJson
            | ParseRejection::InvalidSchema,
        ) => {
            return AnalysisResult::AnalysisUnavailable;
        }
    };

    let mut dispositions: Vec<Option<ClaimRejectionReason>> = output
        .claims
        .iter()
        .map(|claim| check_steps_4_to_8(claim, context).err())
        .collect();

    apply_step_9_consistency(&output.claims, &mut dispositions, context);

    let verdicts = output
        .claims
        .iter()
        .zip(dispositions)
        .map(|(claim, rejection)| {
            let disposition = match rejection {
                Some(reason) => ClaimDisposition::Reject(reason),
                None => {
                    // Step 10: the closed routing decision confirms review;
                    // there is no path from here to anything but review.
                    match route(claim.claim_type) {
                        RoutingDecision::ReviewRequired => ClaimDisposition::AcceptForReview,
                    }
                }
            };
            ClaimVerdict {
                claim_type: claim.claim_type,
                disposition,
            }
        })
        .collect();

    AnalysisResult::Reviewed(ValidationOutcome { verdicts })
}

#[cfg(test)]
mod tests {
    use super::{
        AnalysisResult, ClaimDisposition, ClaimRejectionReason, ConsistencyFailure,
        EvidenceBoundsFailure, HandleMembershipFailure, MessageContext, ParticipantHandle,
        ParticipantSlot, ParticipantSlotFailure, SuppliedContext, TemporalReparseFailure, route,
        validate,
    };
    use openloops_contracts::ClaimType;
    use openloops_domain::deadline::UnixSeconds;
    use openloops_domain::deadline_parse::{
        DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT, ParseContext, TimezoneContext, Weekday,
    };

    use crate::message::{RawMessageInput, RawRecipient, canonicalize_message};

    fn tz() -> TimezoneContext {
        TimezoneContext {
            base_offset_seconds: 0,
            transition: None,
        }
    }

    fn parse_context(timestamp: i64) -> ParseContext {
        ParseContext {
            message_timestamp: UnixSeconds(timestamp),
            timezone: tz(),
            eod_seconds_since_midnight: DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT,
            week_start: Weekday::Monday,
        }
    }

    fn one_message(subject: &str, to: &[RawRecipient<'_>]) -> crate::message::CanonicalMessage {
        let input = RawMessageInput {
            subject,
            body_html: "<p>Please send the report by Friday.</p>",
            sender: Some(RawRecipient {
                name: "Sender",
                address: "sender@example.invalid",
            }),
            from: None,
            to,
            cc: &[],
            attachment_pages: &[crate::message::AttachmentPage {
                terminal: true,
                attachments: &[],
            }],
        };
        canonicalize_message(&input).expect("fixture message canonicalizes")
    }

    fn fixture_json(overrides: &str) -> String {
        format!("{{\"schema_version\":1,\"claims\":[{overrides}]}}")
    }

    fn valid_request_claim(source_handle: &str) -> String {
        format!(
            "{{\"claim_type\":\"request\",\"evidence\":[{{\"source_handle\":\"{source_handle}\",\"component\":\"body_block\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":6}}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":900000,\"ambiguity_codes\":[]}}"
        )
    }

    #[test]
    fn malformed_document_yields_analysis_unavailable() {
        let context = SuppliedContext {
            messages: &[],
            participants: &[],
            loop_candidate_handles: &[],
        };
        assert_eq!(
            validate(b"not json", &context),
            AnalysisResult::AnalysisUnavailable
        );
    }

    #[test]
    fn valid_claim_grounded_in_a_real_message_is_accepted_for_review() {
        let message = one_message("Weekly report", &[]);
        let messages = [MessageContext {
            handle: "msg-1",
            message: &message,
            temporal_context: parse_context(0),
        }];
        let context = SuppliedContext {
            messages: &messages,
            participants: &[],
            loop_candidate_handles: &[],
        };
        let text = fixture_json(&valid_request_claim("msg-1"));
        let AnalysisResult::Reviewed(outcome) = validate(text.as_bytes(), &context) else {
            panic!("expected a reviewed outcome");
        };
        assert_eq!(outcome.verdicts.len(), 1);
        assert_eq!(
            outcome.verdicts[0].disposition,
            ClaimDisposition::AcceptForReview
        );
        assert_eq!(outcome.accepted_count(), 1);
    }

    #[test]
    fn claim_referencing_a_handle_never_issued_is_rejected() {
        // The hostile-model case: a claim cites a message handle the
        // application never issued for this request.
        let context = SuppliedContext {
            messages: &[],
            participants: &[],
            loop_candidate_handles: &[],
        };
        let text = fixture_json(&valid_request_claim("phantom-message"));
        let AnalysisResult::Reviewed(outcome) = validate(text.as_bytes(), &context) else {
            panic!("expected a reviewed outcome");
        };
        assert_eq!(
            outcome.verdicts[0].disposition,
            ClaimDisposition::Reject(ClaimRejectionReason::HandleMembership(
                HandleMembershipFailure::UnknownMessageHandle
            ))
        );
    }

    #[test]
    fn off_by_one_block_boundary_rejects_at_the_bound() {
        let message = one_message("Weekly report", &[]);
        let messages = [MessageContext {
            handle: "msg-1",
            message: &message,
            temporal_context: parse_context(0),
        }];
        let context = SuppliedContext {
            messages: &messages,
            participants: &[],
            loop_candidate_handles: &[],
        };
        let body_len = message.body_blocks[0].scalar_len();
        let claim = format!(
            "{{\"claim_type\":\"request\",\"evidence\":[{{\"source_handle\":\"msg-1\",\"component\":\"body_block\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":{}}}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}}",
            body_len + 1
        );
        let text = fixture_json(&claim);
        let AnalysisResult::Reviewed(outcome) = validate(text.as_bytes(), &context) else {
            panic!("expected a reviewed outcome");
        };
        assert_eq!(
            outcome.verdicts[0].disposition,
            ClaimDisposition::Reject(ClaimRejectionReason::EvidenceBounds(
                EvidenceBoundsFailure::RangeOutOfBounds
            ))
        );
    }

    #[test]
    fn block_ordinal_one_past_the_last_real_block_is_rejected() {
        // The message has exactly one body block (ordinal 0); a claim
        // citing ordinal 1 addresses no real block at all, distinct from an
        // in-bounds block with an out-of-bounds range.
        let message = one_message("Weekly report", &[]);
        let messages = [MessageContext {
            handle: "msg-1",
            message: &message,
            temporal_context: parse_context(0),
        }];
        let context = SuppliedContext {
            messages: &messages,
            participants: &[],
            loop_candidate_handles: &[],
        };
        let claim = "{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"msg-1\",\"component\":\"body_block\",\"block_ordinal\":1,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}";
        let text = fixture_json(claim);
        let AnalysisResult::Reviewed(outcome) = validate(text.as_bytes(), &context) else {
            panic!("expected a reviewed outcome");
        };
        assert_eq!(
            outcome.verdicts[0].disposition,
            ClaimDisposition::Reject(ClaimRejectionReason::EvidenceBounds(
                EvidenceBoundsFailure::BlockOrdinalOutOfRange
            ))
        );
    }

    #[test]
    fn whitespace_only_evidence_range_is_rejected() {
        let message = one_message("Weekly report", &[]);
        let messages = [MessageContext {
            handle: "msg-1",
            message: &message,
            temporal_context: parse_context(0),
        }];
        let context = SuppliedContext {
            messages: &messages,
            participants: &[],
            loop_candidate_handles: &[],
        };
        // "Please send" starts at 0; a space sits at index 6 (`Please`=6
        // chars, then one space before `send`).
        let claim = "{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"msg-1\",\"component\":\"body_block\",\"block_ordinal\":0,\"range_start\":6,\"range_end\":7}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}";
        let text = fixture_json(claim);
        let AnalysisResult::Reviewed(outcome) = validate(text.as_bytes(), &context) else {
            panic!("expected a reviewed outcome");
        };
        assert_eq!(
            outcome.verdicts[0].disposition,
            ClaimDisposition::Reject(ClaimRejectionReason::EvidenceBounds(
                EvidenceBoundsFailure::WhitespaceOnlyEvidence
            ))
        );
    }

    #[test]
    fn participant_ref_to_a_nonexistent_slot_is_rejected() {
        let message = one_message("Weekly report", &[]);
        let messages = [MessageContext {
            handle: "msg-1",
            message: &message,
            temporal_context: parse_context(0),
        }];
        // The application issued a handle naming `To(0)`, but this message
        // has zero `to` recipients: the handle catalog and the actual
        // canonical message disagree, and step 7 must catch that.
        let participants = [ParticipantHandle {
            handle: "participant-1",
            message_handle: "msg-1",
            slot: ParticipantSlot::To(0),
        }];
        let context = SuppliedContext {
            messages: &messages,
            participants: &participants,
            loop_candidate_handles: &[],
        };
        let claim = "{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"msg-1\",\"component\":\"body_block\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":6}],\"waiting_party_handle\":\"participant-1\",\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}";
        let text = fixture_json(claim);
        let AnalysisResult::Reviewed(outcome) = validate(text.as_bytes(), &context) else {
            panic!("expected a reviewed outcome");
        };
        assert_eq!(
            outcome.verdicts[0].disposition,
            ClaimDisposition::Reject(ClaimRejectionReason::ParticipantSlot(
                ParticipantSlotFailure::SlotDoesNotExist
            ))
        );
    }

    #[test]
    fn temporal_hypothesis_the_deterministic_parser_cannot_reproduce_routes_to_review_rejection() {
        let message = one_message("Weekly report", &[]);
        let messages = [MessageContext {
            handle: "msg-1",
            message: &message,
            temporal_context: parse_context(0),
        }];
        let context = SuppliedContext {
            messages: &messages,
            participants: &[],
            loop_candidate_handles: &[],
        };
        // `kind: "date"` but the value is not the strict explicit-date
        // grammar: the deterministic parser cannot reproduce it either way,
        // regardless of what the model's own interpretation might have
        // been, and must route to rejection rather than accepting a guess.
        let claim = "{\"claim_type\":\"deadline_change\",\"evidence\":[{\"source_handle\":\"msg-1\",\"component\":\"body_block\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":6}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":{\"text_evidence_index\":0,\"kind\":\"date\",\"value\":\"sometime soon\"},\"confidence_micros\":0,\"ambiguity_codes\":[]}";
        let text = fixture_json(claim);
        let AnalysisResult::Reviewed(outcome) = validate(text.as_bytes(), &context) else {
            panic!("expected a reviewed outcome");
        };
        assert_eq!(
            outcome.verdicts[0].disposition,
            ClaimDisposition::Reject(ClaimRejectionReason::TemporalReparse(
                TemporalReparseFailure::Unreproducible
            ))
        );
    }

    #[test]
    fn duplicate_claims_reject_every_later_occurrence_and_keep_the_first() {
        let message = one_message("Weekly report", &[]);
        let messages = [MessageContext {
            handle: "msg-1",
            message: &message,
            temporal_context: parse_context(0),
        }];
        let context = SuppliedContext {
            messages: &messages,
            participants: &[],
            loop_candidate_handles: &[],
        };
        let claim = valid_request_claim("msg-1");
        let text = fixture_json(&format!("{claim},{claim}"));
        let AnalysisResult::Reviewed(outcome) = validate(text.as_bytes(), &context) else {
            panic!("expected a reviewed outcome");
        };
        assert_eq!(outcome.verdicts.len(), 2);
        assert_eq!(
            outcome.verdicts[0].disposition,
            ClaimDisposition::AcceptForReview
        );
        assert_eq!(
            outcome.verdicts[1].disposition,
            ClaimDisposition::Reject(ClaimRejectionReason::Consistency(
                ConsistencyFailure::DuplicateClaim
            ))
        );
    }

    #[test]
    fn cross_message_evidence_without_the_disclosed_ambiguity_code_is_rejected() {
        let first_message = one_message("Weekly report", &[]);
        let second_message = one_message("Weekly report follow-up", &[]);
        let messages = [
            MessageContext {
                handle: "msg-1",
                message: &first_message,
                temporal_context: parse_context(0),
            },
            MessageContext {
                handle: "msg-2",
                message: &second_message,
                temporal_context: parse_context(0),
            },
        ];
        let context = SuppliedContext {
            messages: &messages,
            participants: &[],
            loop_candidate_handles: &[],
        };
        // Evidence individually valid against each message, but the claim
        // never discloses that it crosses messages.
        let claim = "{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"msg-1\",\"component\":\"body_block\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":6},{\"source_handle\":\"msg-2\",\"component\":\"body_block\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":6}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}";
        let text = fixture_json(claim);
        let AnalysisResult::Reviewed(outcome) = validate(text.as_bytes(), &context) else {
            panic!("expected a reviewed outcome");
        };
        assert_eq!(
            outcome.verdicts[0].disposition,
            ClaimDisposition::Reject(ClaimRejectionReason::Consistency(
                ConsistencyFailure::UndisclosedCrossMessageEvidence
            ))
        );

        // The same shape, but disclosed, is accepted for review.
        let disclosed_claim = "{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"msg-1\",\"component\":\"body_block\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":6},{\"source_handle\":\"msg-2\",\"component\":\"body_block\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":6}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[\"cross_message\"]}";
        let disclosed_text = fixture_json(disclosed_claim);
        let AnalysisResult::Reviewed(disclosed_outcome) =
            validate(disclosed_text.as_bytes(), &context)
        else {
            panic!("expected a reviewed outcome");
        };
        assert_eq!(
            disclosed_outcome.verdicts[0].disposition,
            ClaimDisposition::AcceptForReview
        );
    }

    #[test]
    fn contradictory_deadline_relations_on_the_same_loop_are_both_rejected() {
        let message = one_message("Weekly report", &[]);
        let messages = [MessageContext {
            handle: "msg-1",
            message: &message,
            temporal_context: parse_context(0),
        }];
        let context = SuppliedContext {
            messages: &messages,
            participants: &[],
            loop_candidate_handles: &["loop-1"],
        };
        // Both claims resolve to concrete instants (via `local_datetime`)
        // so step 9 has two actual instants to compare; a `date`-precision
        // claim never resolves to an instant at all (`OL-DUE-004`) and so
        // has nothing this rule could compare.
        let first = "{\"claim_type\":\"deadline_change\",\"evidence\":[{\"source_handle\":\"msg-1\",\"component\":\"body_block\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":6}],\"waiting_party_handle\":null,\"related_loop_handles\":[\"loop-1\"],\"temporal\":{\"text_evidence_index\":0,\"kind\":\"local_datetime\",\"value\":\"2026-08-10T09:00\"},\"confidence_micros\":0,\"ambiguity_codes\":[]}";
        let second = "{\"claim_type\":\"deadline_change\",\"evidence\":[{\"source_handle\":\"msg-1\",\"component\":\"body_block\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":6}],\"waiting_party_handle\":null,\"related_loop_handles\":[\"loop-1\"],\"temporal\":{\"text_evidence_index\":0,\"kind\":\"local_datetime\",\"value\":\"2026-08-11T09:00\"},\"confidence_micros\":0,\"ambiguity_codes\":[]}";
        let text = fixture_json(&format!("{first},{second}"));
        let AnalysisResult::Reviewed(outcome) = validate(text.as_bytes(), &context) else {
            panic!("expected a reviewed outcome");
        };
        assert_eq!(outcome.verdicts.len(), 2);
        for verdict in &outcome.verdicts {
            assert_eq!(
                verdict.disposition,
                ClaimDisposition::Reject(ClaimRejectionReason::Consistency(
                    ConsistencyFailure::ContradictoryDeadlineRelation
                ))
            );
        }
    }

    #[test]
    fn maximum_confidence_never_bypasses_review_required_routing() {
        // The adversarial "schema-valid output engineered to look
        // high-confidence" case: `confidence_micros` at its absolute
        // maximum must still route to `AcceptForReview`, never anything
        // resembling automatic acceptance, because `route` never even
        // reads `confidence_micros`.
        let message = one_message("Weekly report", &[]);
        let messages = [MessageContext {
            handle: "msg-1",
            message: &message,
            temporal_context: parse_context(0),
        }];
        let context = SuppliedContext {
            messages: &messages,
            participants: &[],
            loop_candidate_handles: &[],
        };
        let claim = "{\"claim_type\":\"possible_closure\",\"evidence\":[{\"source_handle\":\"msg-1\",\"component\":\"body_block\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":6}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":1000000,\"ambiguity_codes\":[]}";
        let text = fixture_json(claim);
        let AnalysisResult::Reviewed(outcome) = validate(text.as_bytes(), &context) else {
            panic!("expected a reviewed outcome");
        };
        assert_eq!(
            outcome.verdicts[0].disposition,
            ClaimDisposition::AcceptForReview
        );
    }

    #[test]
    fn every_claim_type_routes_to_review_required() {
        for claim_type in [
            ClaimType::Request,
            ClaimType::Promise,
            ClaimType::Attribution,
            ClaimType::Question,
            ClaimType::DeadlineChange,
            ClaimType::PossibleClosure,
            ClaimType::Delegation,
            ClaimType::Modification,
        ] {
            assert_eq!(route(claim_type), super::RoutingDecision::ReviewRequired);
        }
    }
}
