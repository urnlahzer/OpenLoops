//! Desktop-owned projection of governed claims into review cards.

use openloops_contracts::{AmbiguityCode, ClaimType, Nullable};
use openloops_inference::{blocks::CanonicalBlock, expectations::ConversationMessage};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    You,
    Team,
    Unclear,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResolutionKind {
    Completed,
    Declined,
    Withdrawn,
    Superseded,
    Agreed,
}

#[derive(Clone)]
pub struct Anchor {
    pub message: String,
    pub block: usize,
    pub quote: String,
    pub context: String,
}

#[derive(Clone)]
pub struct EventPassed {
    pub name: String,
    pub end: i64,
    pub message_handle: String,
    pub from_subject: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestedUpdateKind {
    Closure,
    DeadlineChange,
    Modification,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuggestedUpdate {
    pub kind: SuggestedUpdateKind,
    pub evidence_text: String,
    pub source_message: String,
    pub source_block: usize,
    pub temporal_value: Option<String>,
    pub confidence_micros: u32,
}

#[derive(Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "projection flags remain independent fields on the desktop card model"
)]
pub struct LoopItem {
    pub action: String,
    pub action_phrase: String,
    pub owner: Owner,
    pub waiting_party: String,
    pub kind: String,
    pub evidence: Anchor,
    pub deadline: Option<Anchor>,
    pub event: Option<Anchor>,
    pub event_time: Option<Anchor>,
    pub resolution: Option<Anchor>,
    pub resolution_kind: Option<ResolutionKind>,
    pub uncertainty: String,
    pub unverified_deadline: bool,
    pub unverified_resolution: bool,
    pub cross_thread: bool,
    pub event_passed: Option<EventPassed>,
    pub suggested_update: Option<SuggestedUpdate>,
    pub from_call_summary: bool,
    pub meeting_time: Option<i64>,
    pub meeting_time_approx: bool,
}

pub struct LoopItems {
    pub items: Vec<LoopItem>,
    pub rejected: usize,
    pub rejection_reasons: Vec<&'static str>,
    pub degraded: usize,
}

/// Card metadata for a standalone governed claim type.
#[must_use]
pub fn claim_shape(claim_type: ClaimType) -> Option<(&'static str, &'static str, Owner)> {
    match claim_type {
        ClaimType::Request => Some(("request", "Requested: ", Owner::You)),
        ClaimType::Question => Some(("request", "Answer: ", Owner::You)),
        ClaimType::Promise => Some(("promise", "You promised: ", Owner::You)),
        ClaimType::Delegation => Some(("request", "Delegated: ", Owner::You)),
        ClaimType::Attribution => Some(("attributed", "Someone else owes: ", Owner::Unclear)),
        ClaimType::PossibleClosure | ClaimType::DeadlineChange | ClaimType::Modification => None,
    }
}

const ACTION_SENTENCE_MAX_SCALARS: usize = 140;
const ACTION_PHRASE_MAX_SCALARS: usize = 200;

/// Synthesizes the deterministic card title for a governed claim.
#[must_use]
pub fn card_action(prefix: &str, evidence_text: &str) -> String {
    let scalars: Vec<char> = evidence_text.chars().collect();
    let mut end = scalars.len();
    for (index, &scalar) in scalars.iter().enumerate() {
        if matches!(scalar, '.' | '?' | '!')
            && scalars
                .get(index + 1)
                .is_none_or(|next| next.is_whitespace())
        {
            end = index + 1;
            break;
        }
    }
    let truncated = end > ACTION_SENTENCE_MAX_SCALARS;
    if truncated {
        end = ACTION_SENTENCE_MAX_SCALARS;
    }
    let sentence: String = scalars[..end].iter().collect();
    let sentence = sentence.trim();
    if truncated {
        format!("{prefix}{sentence}…")
    } else {
        format!("{prefix}{sentence}")
    }
}

/// Stable source-derived action phrase used by decision fingerprints.
#[must_use]
pub fn action_phrase(evidence_text: &str) -> String {
    evidence_text
        .chars()
        .take(ACTION_PHRASE_MAX_SCALARS)
        .collect()
}

/// Downgrades ownership that the evidence message does not support.
#[must_use]
pub fn resolve_owner(owner: Owner, source: &ConversationMessage) -> Owner {
    if owner == Owner::You && !source.from_user && !source.to_user {
        return if source.team {
            Owner::Team
        } else {
            Owner::Unclear
        };
    }
    if owner == Owner::Team && !source.team {
        return Owner::Unclear;
    }
    owner
}

/// Resolves a governed participant handle to its desktop display text.
#[must_use]
pub fn waiting_party_display(
    handle: &Nullable<String>,
    conversation: &[ConversationMessage],
) -> String {
    let Nullable::Value(handle) = handle else {
        return "Not established".to_string();
    };
    for message in conversation {
        if handle == &format!("{}-sender", message.handle) {
            return message
                .message
                .sender
                .as_ref()
                .map_or_else(|| "Not established".to_string(), CanonicalBlock::as_string);
        }
        if let Some(index) = handle
            .strip_prefix(&format!("{}-to-", message.handle))
            .and_then(|index| index.parse::<usize>().ok())
        {
            return message
                .message
                .to
                .get(index)
                .map_or_else(|| "Not established".to_string(), CanonicalBlock::as_string);
        }
        if let Some(index) = handle
            .strip_prefix(&format!("{}-cc-", message.handle))
            .and_then(|index| index.parse::<usize>().ok())
        {
            return message
                .message
                .cc
                .get(index)
                .map_or_else(|| "Not established".to_string(), CanonicalBlock::as_string);
        }
    }
    "Not established".to_string()
}

/// Fixed uncertainty pill text for each governed ambiguity code.
#[must_use]
pub const fn ambiguity_label(code: AmbiguityCode) -> &'static str {
    match code {
        AmbiguityCode::QuoteScope => "the cited text covers more than this item",
        AmbiguityCode::Identity => "unclear who asks or who owes",
        AmbiguityCode::Delegation => "may have been handed off",
        AmbiguityCode::Deadline => "a time is implied but not stated",
        AmbiguityCode::Relation => "unclear which loop this affects",
        AmbiguityCode::CrossMessage => "evidence spans messages",
        AmbiguityCode::InsufficientContext => "the conversation may not show enough",
        AmbiguityCode::SemanticConflict => "messages disagree",
    }
}

pub(crate) const DELEGATION_UNCERTAINTY: &str =
    "You handed this to someone else; the requester is still waiting on you.";

/// Builds the card's closed-vocabulary uncertainty callout.
#[must_use]
pub fn uncertainty_text(claim_type: ClaimType, codes: &[AmbiguityCode]) -> String {
    let is_delegation = claim_type == ClaimType::Delegation;
    let mut parts = Vec::new();
    if is_delegation {
        parts.push(DELEGATION_UNCERTAINTY);
    }
    for code in codes {
        if is_delegation && *code == AmbiguityCode::Delegation {
            continue;
        }
        parts.push(ambiguity_label(*code));
    }
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ambiguity_code_has_a_fixed_label() {
        let cases = [
            (
                AmbiguityCode::QuoteScope,
                "the cited text covers more than this item",
            ),
            (AmbiguityCode::Identity, "unclear who asks or who owes"),
            (AmbiguityCode::Delegation, "may have been handed off"),
            (AmbiguityCode::Deadline, "a time is implied but not stated"),
            (AmbiguityCode::Relation, "unclear which loop this affects"),
            (AmbiguityCode::CrossMessage, "evidence spans messages"),
            (
                AmbiguityCode::InsufficientContext,
                "the conversation may not show enough",
            ),
            (AmbiguityCode::SemanticConflict, "messages disagree"),
        ];
        for (code, expected) in cases {
            assert_eq!(ambiguity_label(code), expected);
        }
    }
}
