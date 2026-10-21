//! Closure walk: follows one open loop forward through later messages.
//! This module holds the pure parts (registry ids, request states,
//! question sets and the per-message verdict); `review_scan` sends the
//! requests and applies the verdicts to cards.

use std::collections::BTreeMap;

use openloops_inference::decision::{Answer, Answers, Questions, registry::Registry};
use openloops_inference::provider::ProviderError;

use crate::claim_view::{ResolutionKind, SuggestedUpdateKind};

pub(super) const GATE: &str = "walk.responds";
pub(super) const WP_RECEIVED: &str = "walk.wp_received";
pub(super) const WP_NO_LONGER_NEEDED: &str = "walk.wp_no_longer_needed";
pub(super) const WP_ASKS_AGAIN: &str = "walk.wp_asks_again";
pub(super) const WP_CHANGES_TERMS: &str = "walk.wp_changes_terms";
pub(super) const WP_STILL_OWED: &str = "walk.wp_still_owed";
pub(super) const WP_CHANGES_DUE: &str = "walk.wp_changes_due";
pub(super) const USER_DELIVERS: &str = "walk.user_delivers";
pub(super) const USER_PROMISES: &str = "walk.user_promises";
pub(super) const USER_DECLINES: &str = "walk.user_declines";
pub(super) const USER_HANDS_OFF: &str = "walk.user_hands_off";
pub(super) const THIRD_PARTY_DONE: &str = "walk.third_party_done";
pub(super) const NEW_DATE: &str = "walk.new_date";
pub(super) const EXPIRY_EVENT: &str = "expiry.event";
pub(super) const EXPIRY_ENDS: &str = "expiry.ends_with_event";
pub(super) const NONE_OPTION: &str = "none";

const WAITING_PARTY_IDS: [&str; 6] = [
    WP_RECEIVED,
    WP_NO_LONGER_NEEDED,
    WP_ASKS_AGAIN,
    WP_CHANGES_TERMS,
    WP_STILL_OWED,
    WP_CHANGES_DUE,
];
const USER_IDS: [&str; 4] = [USER_DELIVERS, USER_PROMISES, USER_DECLINES, USER_HANDS_OFF];
const THIRD_PARTY_IDS: [&str; 1] = [THIRD_PARTY_DONE];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuthorRole {
    WaitingParty,
    User,
    ThirdParty,
}

impl AuthorRole {
    pub(super) const fn wire(self) -> &'static str {
        match self {
            Self::WaitingParty => "waiting_party",
            Self::User => "user",
            Self::ThirdParty => "third_party",
        }
    }

    pub(super) const fn branch_ids(self) -> &'static [&'static str] {
        match self {
            Self::WaitingParty => &WAITING_PARTY_IDS,
            Self::User => &USER_IDS,
            Self::ThirdParty => &THIRD_PARTY_IDS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Band {
    Accept,
    Gray,
    Reject,
}

pub(super) fn band(id: &str, probability: f64) -> Band {
    let question = Registry::get()
        .question(id)
        .expect("closure walk registry question");
    if probability >= question.accept {
        Band::Accept
    } else if probability >= question.escalate {
        Band::Gray
    } else {
        Band::Reject
    }
}

pub(super) struct WalkContext<'a> {
    pub user_name: &'a str,
    pub waiting_party: &'a str,
    pub ask: &'a str,
    pub due: &'a str,
}

pub(super) fn walk_state(
    context: &WalkContext<'_>,
    role: AuthorRole,
    paragraph: &str,
) -> serde_json::Value {
    serde_json::json!({
        "user": {"name": context.user_name},
        "waiting_party": context.waiting_party,
        "ask": context.ask,
        "due": context.due,
        "later": {"author_role": role.wire(), "paragraph_text": paragraph},
    })
}

pub(super) fn walk_questions(role: AuthorRole) -> Result<Questions, ProviderError> {
    let mut ids = vec![GATE];
    ids.extend_from_slice(role.branch_ids());
    Questions::from_registry(&ids)
}

#[derive(Clone, Debug)]
pub(super) struct ParagraphAnswers {
    pub ordinal: usize,
    pub probabilities: BTreeMap<&'static str, f64>,
}

pub(super) fn paragraph_answers(
    ordinal: usize,
    role: AuthorRole,
    answers: &Answers,
) -> Result<ParagraphAnswers, ProviderError> {
    let mut probabilities = BTreeMap::new();
    for id in std::iter::once(&GATE).chain(role.branch_ids()) {
        let Some(Answer::Noul { probability }) = answers.get(id) else {
            return Err(ProviderError::InvalidResponse);
        };
        probabilities.insert(*id, *probability);
    }
    Ok(ParagraphAnswers {
        ordinal,
        probabilities,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DateSource {
    WaitingParty,
    User,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Effect {
    /// Automatic: the waiting party confirmed or withdrew.
    Close {
        kind: ResolutionKind,
        ordinal: usize,
    },
    /// Review: the user's own reply or a third party's message.
    Suggest {
        kind: SuggestedUpdateKind,
        ordinal: usize,
        probability: f64,
    },
    AskedAgain {
        ordinal: usize,
    },
    Amended {
        ordinal: usize,
    },
    NewDate {
        source: DateSource,
        ordinal: usize,
        probability: f64,
    },
}

impl Effect {
    const fn is_terminal(&self) -> bool {
        matches!(self, Self::Close { .. } | Self::Suggest { .. })
    }

    fn same_outcome(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Close { kind: a, .. }, Self::Close { kind: b, .. }) => a == b,
            (Self::Suggest { kind: a, .. }, Self::Suggest { kind: b, .. }) => a == b,
            _ => false,
        }
    }

    const fn rank(&self) -> u8 {
        match self {
            Self::Close { .. } | Self::Suggest { .. } => 0,
            Self::AskedAgain { .. } => 1,
            Self::Amended { .. } => 2,
            Self::NewDate { .. } => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum StepVerdict {
    NoResponse,
    Escalate,
    Apply(Vec<Effect>),
}

/// One later message's verdict from its paragraphs' answers.
pub(super) fn step_verdict(role: AuthorRole, paragraphs: &[ParagraphAnswers]) -> StepVerdict {
    let gate = |paragraph: &ParagraphAnswers| band(GATE, paragraph.probabilities[GATE]);
    let responding: Vec<&ParagraphAnswers> = paragraphs
        .iter()
        .filter(|paragraph| gate(paragraph) == Band::Accept)
        .collect();
    if responding.is_empty() {
        return if paragraphs
            .iter()
            .any(|paragraph| gate(paragraph) == Band::Gray)
        {
            StepVerdict::Escalate
        } else {
            StepVerdict::NoResponse
        };
    }
    let mut terminal = Vec::new();
    let mut other: Vec<Effect> = Vec::new();
    for paragraph in responding {
        let Some(effects) = paragraph_effects(role, paragraph) else {
            return StepVerdict::Escalate;
        };
        for effect in effects {
            if effect.is_terminal() {
                terminal.push(effect);
            } else if !other.iter().any(|kept| kept.rank() == effect.rank()) {
                other.push(effect);
            }
        }
    }
    if let Some(first) = terminal.first() {
        if terminal.iter().any(|effect| !effect.same_outcome(first)) {
            return StepVerdict::Escalate;
        }
        return StepVerdict::Apply(vec![first.clone()]);
    }
    if other.is_empty() {
        StepVerdict::NoResponse
    } else {
        other.sort_by_key(Effect::rank);
        StepVerdict::Apply(other)
    }
}

/// `None` when an answer that matters sits in the gray band.
fn paragraph_effects(role: AuthorRole, paragraph: &ParagraphAnswers) -> Option<Vec<Effect>> {
    let ordinal = paragraph.ordinal;
    let probability = |id: &str| paragraph.probabilities[id];
    let yes = |id: &str| band(id, probability(id)) == Band::Accept;
    let checked: &[&str] = match role {
        AuthorRole::WaitingParty => &[
            WP_RECEIVED,
            WP_NO_LONGER_NEEDED,
            WP_ASKS_AGAIN,
            WP_CHANGES_TERMS,
            WP_CHANGES_DUE,
        ],
        AuthorRole::User => &USER_IDS,
        AuthorRole::ThirdParty => &THIRD_PARTY_IDS,
    };
    if checked
        .iter()
        .any(|id| band(id, probability(id)) == Band::Gray)
    {
        return None;
    }
    let mut effects = Vec::new();
    match role {
        AuthorRole::WaitingParty => {
            if yes(WP_RECEIVED) {
                effects.push(Effect::Close {
                    kind: ResolutionKind::Completed,
                    ordinal,
                });
            }
            if yes(WP_NO_LONGER_NEEDED) {
                effects.push(Effect::Close {
                    kind: ResolutionKind::Withdrawn,
                    ordinal,
                });
            }
            if yes(WP_CHANGES_TERMS) {
                match band(WP_STILL_OWED, probability(WP_STILL_OWED)) {
                    Band::Accept => effects.push(Effect::Amended { ordinal }),
                    Band::Reject => effects.push(Effect::Close {
                        kind: ResolutionKind::Withdrawn,
                        ordinal,
                    }),
                    Band::Gray => return None,
                }
            }
            if yes(WP_ASKS_AGAIN) {
                effects.push(Effect::AskedAgain { ordinal });
            }
            if yes(WP_CHANGES_DUE) {
                effects.push(Effect::NewDate {
                    source: DateSource::WaitingParty,
                    ordinal,
                    probability: probability(WP_CHANGES_DUE),
                });
            }
        }
        AuthorRole::User => {
            for (id, kind) in [
                (USER_DELIVERS, SuggestedUpdateKind::Closure),
                (USER_DECLINES, SuggestedUpdateKind::Declined),
                (USER_HANDS_OFF, SuggestedUpdateKind::Delegated),
            ] {
                if yes(id) {
                    effects.push(Effect::Suggest {
                        kind,
                        ordinal,
                        probability: probability(id),
                    });
                }
            }
            if yes(USER_PROMISES) {
                effects.push(Effect::NewDate {
                    source: DateSource::User,
                    ordinal,
                    probability: probability(USER_PROMISES),
                });
            }
        }
        AuthorRole::ThirdParty => {
            if yes(THIRD_PARTY_DONE) {
                effects.push(Effect::Suggest {
                    kind: SuggestedUpdateKind::Closure,
                    ordinal,
                    probability: probability(THIRD_PARTY_DONE),
                });
            }
        }
    }
    Some(effects)
}

/// Follow-up for a date change: `{ask, later: {paragraph_text}}`.
pub(super) fn date_state(ask: &str, paragraph: &str) -> serde_json::Value {
    serde_json::json!({"ask": ask, "later": {"paragraph_text": paragraph}})
}

/// `candidates` are `(handle, normalized value)` pairs.
pub(super) fn date_questions(candidates: &[(String, String)]) -> Result<Questions, ProviderError> {
    let registered = Registry::get()
        .question(NEW_DATE)
        .ok_or(ProviderError::InvalidQuestion)?;
    let none = registered
        .options
        .iter()
        .find(|(key, _)| key == NONE_OPTION)
        .map_or(
            "The paragraph sets no new date for the request.",
            |(_, text)| text.as_str(),
        );
    let mut options: Vec<(&str, &str)> = candidates
        .iter()
        .map(|(handle, value)| (handle.as_str(), value.as_str()))
        .collect();
    options.push((NONE_OPTION, none));
    Questions::new().choice(NEW_DATE, &registered.instructions, &options)
}

/// The chosen normalized date, or `None` (`none`, gray or low confidence).
pub(super) fn date_pick(answers: &Answers, candidates: &[(String, String)]) -> Option<String> {
    let Some(Answer::Choice {
        choice, confidence, ..
    }) = answers.get(NEW_DATE)
    else {
        return None;
    };
    if choice == NONE_OPTION || band(NEW_DATE, *confidence) != Band::Accept {
        return None;
    }
    candidates
        .iter()
        .find(|(handle, _)| handle == choice)
        .map(|(_, value)| value.clone())
}

/// One event an ask may serve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EventCandidate {
    pub handle: String,
    pub name: String,
    pub description: String,
    pub end: i64,
    pub message_handle: String,
    pub from_subject: bool,
}

pub(super) fn expiry_state(ask: &str, waiting_party: &str, due: &str) -> serde_json::Value {
    serde_json::json!({"ask": ask, "waiting_party": waiting_party, "due": due})
}

/// The event choice, with candidates in order or reversed; `none` is always last.
pub(super) fn expiry_event_questions(
    candidates: &[EventCandidate],
    reversed: bool,
) -> Result<Questions, ProviderError> {
    let registered = Registry::get()
        .question(EXPIRY_EVENT)
        .ok_or(ProviderError::InvalidQuestion)?;
    let none = registered
        .options
        .iter()
        .find(|(key, _)| key == NONE_OPTION)
        .map_or("The request serves no listed event.", |(_, text)| {
            text.as_str()
        });
    let mut options: Vec<(&str, &str)> = candidates
        .iter()
        .map(|candidate| (candidate.handle.as_str(), candidate.description.as_str()))
        .collect();
    if reversed {
        options.reverse();
    }
    options.push((NONE_OPTION, none));
    Questions::new().choice(EXPIRY_EVENT, &registered.instructions, &options)
}

/// Both orders must pick the same non-`none` event at accept confidence.
pub(super) fn expiry_pick<'a>(
    forward: &Answers,
    reversed: &Answers,
    candidates: &'a [EventCandidate],
) -> Option<&'a EventCandidate> {
    let pick = |answers: &Answers| match answers.get(EXPIRY_EVENT) {
        Some(Answer::Choice {
            choice, confidence, ..
        }) if choice != NONE_OPTION && band(EXPIRY_EVENT, *confidence) == Band::Accept => {
            Some(choice.clone())
        }
        _ => None,
    };
    let (forward_pick, reversed_pick) = (pick(forward)?, pick(reversed)?);
    (forward_pick == reversed_pick)
        .then(|| {
            candidates
                .iter()
                .find(|candidate| candidate.handle == forward_pick)
        })
        .flatten()
}

pub(super) fn ends_state(ask: &str, waiting_party: &str, event: &str) -> serde_json::Value {
    serde_json::json!({"ask": ask, "waiting_party": waiting_party, "event": event})
}

pub(super) fn ends_questions() -> Result<Questions, ProviderError> {
    Questions::from_registry(&[EXPIRY_ENDS])
}

/// Expired: the ask ends with the event and the event is over.
pub(super) fn expired(ends: &Answers, event_end: i64, now: i64) -> bool {
    matches!(
        ends.get(EXPIRY_ENDS),
        Some(Answer::Noul { probability })
            if band(EXPIRY_ENDS, *probability) == Band::Accept && event_end < now
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "typesafe/jev-1.13";

    fn accept(id: &str) -> f64 {
        Registry::get().question(id).unwrap().accept
    }

    fn gray(id: &str) -> f64 {
        let question = Registry::get().question(id).unwrap();
        question.accept.midpoint(question.escalate)
    }

    fn no(id: &str) -> f64 {
        Registry::get().question(id).unwrap().escalate / 2.0
    }

    fn paragraph(
        ordinal: usize,
        role: AuthorRole,
        yes: &[&'static str],
        grays: &[&'static str],
    ) -> ParagraphAnswers {
        let mut probabilities = BTreeMap::new();
        for id in std::iter::once(&GATE).chain(role.branch_ids()) {
            let probability = if yes.contains(id) {
                accept(id)
            } else if grays.contains(id) {
                gray(id)
            } else {
                no(id)
            };
            probabilities.insert(*id, probability);
        }
        ParagraphAnswers {
            ordinal,
            probabilities,
        }
    }

    #[test]
    fn no_responding_paragraph_is_no_response() {
        let paragraph = paragraph(0, AuthorRole::WaitingParty, &[], &[]);
        assert_eq!(
            step_verdict(AuthorRole::WaitingParty, &[paragraph]),
            StepVerdict::NoResponse
        );
    }

    #[test]
    fn a_gray_gate_with_no_accepted_gate_escalates() {
        let paragraph = paragraph(0, AuthorRole::User, &[], &[GATE]);
        assert_eq!(
            step_verdict(AuthorRole::User, &[paragraph]),
            StepVerdict::Escalate
        );
    }

    #[test]
    fn waiting_party_receipt_closes_as_completed() {
        let paragraph = paragraph(1, AuthorRole::WaitingParty, &[GATE, WP_RECEIVED], &[]);
        assert_eq!(
            step_verdict(AuthorRole::WaitingParty, &[paragraph]),
            StepVerdict::Apply(vec![Effect::Close {
                kind: ResolutionKind::Completed,
                ordinal: 1,
            }])
        );
    }

    #[test]
    fn waiting_party_withdrawal_closes_as_withdrawn() {
        let paragraph = paragraph(
            0,
            AuthorRole::WaitingParty,
            &[GATE, WP_NO_LONGER_NEEDED],
            &[],
        );
        assert_eq!(
            step_verdict(AuthorRole::WaitingParty, &[paragraph]),
            StepVerdict::Apply(vec![Effect::Close {
                kind: ResolutionKind::Withdrawn,
                ordinal: 0,
            }])
        );
    }

    #[test]
    fn a_correction_that_leaves_the_action_owed_amends() {
        let paragraph = paragraph(
            0,
            AuthorRole::WaitingParty,
            &[GATE, WP_CHANGES_TERMS, WP_STILL_OWED],
            &[],
        );
        assert_eq!(
            step_verdict(AuthorRole::WaitingParty, &[paragraph]),
            StepVerdict::Apply(vec![Effect::Amended { ordinal: 0 }])
        );
    }

    #[test]
    fn a_correction_that_ends_the_action_withdraws() {
        let paragraph = paragraph(0, AuthorRole::WaitingParty, &[GATE, WP_CHANGES_TERMS], &[]);
        assert_eq!(
            step_verdict(AuthorRole::WaitingParty, &[paragraph]),
            StepVerdict::Apply(vec![Effect::Close {
                kind: ResolutionKind::Withdrawn,
                ordinal: 0,
            }])
        );
    }

    #[test]
    fn a_gray_still_owed_after_a_change_escalates() {
        let paragraph = paragraph(
            0,
            AuthorRole::WaitingParty,
            &[GATE, WP_CHANGES_TERMS],
            &[WP_STILL_OWED],
        );
        assert_eq!(
            step_verdict(AuthorRole::WaitingParty, &[paragraph]),
            StepVerdict::Escalate
        );
    }

    #[test]
    fn chaser_and_new_date_combine() {
        let paragraph = paragraph(
            0,
            AuthorRole::WaitingParty,
            &[GATE, WP_ASKS_AGAIN, WP_CHANGES_DUE],
            &[],
        );
        let StepVerdict::Apply(effects) = step_verdict(AuthorRole::WaitingParty, &[paragraph])
        else {
            panic!()
        };
        assert!(effects.contains(&Effect::AskedAgain { ordinal: 0 }));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::NewDate {
                source: DateSource::WaitingParty,
                ..
            }
        )));
    }

    #[test]
    fn terminal_beats_non_terminal_and_conflicting_terminals_escalate() {
        let both = paragraph(
            0,
            AuthorRole::WaitingParty,
            &[GATE, WP_RECEIVED, WP_ASKS_AGAIN],
            &[],
        );
        assert_eq!(
            step_verdict(AuthorRole::WaitingParty, &[both]),
            StepVerdict::Apply(vec![Effect::Close {
                kind: ResolutionKind::Completed,
                ordinal: 0,
            }])
        );
        let received = paragraph(0, AuthorRole::WaitingParty, &[GATE, WP_RECEIVED], &[]);
        let withdrawn = paragraph(
            1,
            AuthorRole::WaitingParty,
            &[GATE, WP_NO_LONGER_NEEDED],
            &[],
        );
        assert_eq!(
            step_verdict(AuthorRole::WaitingParty, &[received, withdrawn]),
            StepVerdict::Escalate
        );
    }

    #[test]
    fn user_branches_become_suggestions() {
        for (id, kind) in [
            (USER_DELIVERS, SuggestedUpdateKind::Closure),
            (USER_DECLINES, SuggestedUpdateKind::Declined),
            (USER_HANDS_OFF, SuggestedUpdateKind::Delegated),
        ] {
            let paragraph = paragraph(0, AuthorRole::User, &[GATE, id], &[]);
            let StepVerdict::Apply(effects) = step_verdict(AuthorRole::User, &[paragraph]) else {
                panic!()
            };
            assert!(
                matches!(effects.as_slice(), [Effect::Suggest { kind: actual, ordinal: 0, .. }] if *actual == kind)
            );
        }
        let paragraph = paragraph(0, AuthorRole::User, &[GATE, USER_PROMISES], &[]);
        let StepVerdict::Apply(effects) = step_verdict(AuthorRole::User, &[paragraph]) else {
            panic!()
        };
        assert!(matches!(
            effects.as_slice(),
            [Effect::NewDate {
                source: DateSource::User,
                ..
            }]
        ));
    }

    #[test]
    fn third_party_done_suggests_handled_and_gray_branch_escalates() {
        let third_party = paragraph(0, AuthorRole::ThirdParty, &[GATE, THIRD_PARTY_DONE], &[]);
        let StepVerdict::Apply(effects) = step_verdict(AuthorRole::ThirdParty, &[third_party])
        else {
            panic!()
        };
        assert!(matches!(
            effects.as_slice(),
            [Effect::Suggest {
                kind: SuggestedUpdateKind::Closure,
                ..
            }]
        ));
        let user_paragraph = paragraph(0, AuthorRole::User, &[GATE], &[USER_DELIVERS]);
        assert_eq!(
            step_verdict(AuthorRole::User, &[user_paragraph]),
            StepVerdict::Escalate
        );
    }

    #[test]
    fn walk_state_has_the_harness_shape() {
        let state = walk_state(
            &WalkContext {
                user_name: "Alex",
                waiting_party: "Priya Venkataraman",
                ask: "Send the summary.",
                due: "Friday",
            },
            AuthorRole::User,
            "Attached.",
        );
        assert_eq!(
            state.to_string(),
            r#"{"user":{"name":"Alex"},"waiting_party":"Priya Venkataraman","ask":"Send the summary.","due":"Friday","later":{"author_role":"user","paragraph_text":"Attached."}}"#
        );
    }

    fn candidates() -> Vec<EventCandidate> {
        ["e0", "e1"]
            .iter()
            .map(|handle| EventCandidate {
                handle: (*handle).to_string(),
                name: format!("event {handle}"),
                description: format!("event {handle}, ending Oct 30, 2026 23:59"),
                end: 1_800_000_000,
                message_handle: "m0".into(),
                from_subject: false,
            })
            .collect()
    }

    fn choice_answers(questions: &Questions, choice: &str, confidence: f64) -> Answers {
        let mut probabilities = serde_json::Map::new();
        for key in ["e0", "e1", NONE_OPTION] {
            probabilities.insert(
                key.into(),
                serde_json::Value::from(if key == choice { confidence } else { 0.0 }),
            );
        }
        let body = serde_json::json!({
            "model": MODEL,
            "answers": {EXPIRY_EVENT: {
                "type": "choice",
                "choice": choice,
                "probabilities": probabilities,
                "confidence": confidence
            }},
            "usage": {"input_tokens": 1}
        });
        openloops_inference::decision::parse_answers(
            &serde_json::to_vec(&body).unwrap(),
            MODEL,
            questions,
        )
        .unwrap()
    }

    fn ends_answers(probability: f64) -> Answers {
        let body = serde_json::json!({
            "model": MODEL,
            "answers": {EXPIRY_ENDS: {"type": "noul", "noul": probability}},
            "usage": {"input_tokens": 1}
        });
        openloops_inference::decision::parse_answers(
            &serde_json::to_vec(&body).unwrap(),
            MODEL,
            &ends_questions().unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn expiry_needs_both_orders_to_agree() {
        let events = candidates();
        let forward_questions = expiry_event_questions(&events, false).unwrap();
        let reversed_questions = expiry_event_questions(&events, true).unwrap();
        let yes = accept(EXPIRY_EVENT);
        let picked = expiry_pick(
            &choice_answers(&forward_questions, "e0", yes),
            &choice_answers(&reversed_questions, "e0", yes),
            &events,
        );
        assert_eq!(
            picked.map(|candidate| candidate.handle.as_str()),
            Some("e0")
        );
        assert!(
            expiry_pick(
                &choice_answers(&forward_questions, "e0", yes),
                &choice_answers(&reversed_questions, "e1", yes),
                &events,
            )
            .is_none()
        );
        assert!(
            expiry_pick(
                &choice_answers(&forward_questions, NONE_OPTION, yes),
                &choice_answers(&reversed_questions, NONE_OPTION, yes),
                &events,
            )
            .is_none()
        );
        assert!(
            expiry_pick(
                &choice_answers(&forward_questions, "e0", gray(EXPIRY_EVENT)),
                &choice_answers(&reversed_questions, "e0", yes),
                &events,
            )
            .is_none()
        );
    }

    #[test]
    fn due_and_expires_table() {
        let end = 1_800_000_000;
        assert!(expired(&ends_answers(accept(EXPIRY_ENDS)), end, end + 1));
        assert!(!expired(&ends_answers(accept(EXPIRY_ENDS)), end, end));
        assert!(!expired(&ends_answers(no(EXPIRY_ENDS)), end, end + 1));
        assert!(!expired(&ends_answers(gray(EXPIRY_ENDS)), end, end + 1));
    }

    #[test]
    fn reversed_event_questions_keep_none_last() {
        let events = candidates();
        let body = openloops_inference::decision::request_body(
            MODEL,
            &serde_json::json!({}),
            &expiry_event_questions(&events, true).unwrap(),
            false,
        )
        .unwrap();
        let text = String::from_utf8(body).unwrap();
        let (e1, e0, none) = (
            text.find("\"e1\"").unwrap(),
            text.find("\"e0\"").unwrap(),
            text.find("\"none\"").unwrap(),
        );
        assert!(e1 < e0 && e0 < none);
    }

    #[test]
    fn follow_up_and_expiry_states_match_harness_shapes() {
        assert_eq!(
            date_state("Send the summary.", "Wednesday works better.").to_string(),
            r#"{"ask":"Send the summary.","later":{"paragraph_text":"Wednesday works better."}}"#
        );
        assert_eq!(
            expiry_state("Send the summary.", "Priya Venkataraman", "Friday").to_string(),
            r#"{"ask":"Send the summary.","waiting_party":"Priya Venkataraman","due":"Friday"}"#
        );
        assert_eq!(
            ends_state(
                "Send the summary.",
                "Priya Venkataraman",
                "the gala ending Friday"
            )
            .to_string(),
            r#"{"ask":"Send the summary.","waiting_party":"Priya Venkataraman","event":"the gala ending Friday"}"#
        );
    }
}
