//! Review-screen projection and callback wiring for the Slint adapter.
use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering};

use crate::{
    app_model::{self, AppModel, Outcome, Service, Status},
    deadline_view::label as deadline_label,
    loop_state::{Decision, Reminder, marker},
    review_model::{
        CardContext, Filter, ListGroup, ReminderDraft, ReviewState, SHOW_HANDLED_LABEL, ScanStrip,
        card_hidden, card_order, decision_after_setting_reminder, default_reminder,
        expectations_summary, list_group, open_badge_count, reminder_button_enabled, reminder_time,
        resolution_anchor_label, status_base_label, status_label, status_shows_cross_thread,
    },
    slint_ui::{
        AppWindow, CompletionCard, ConversationRow, EvidenceCard, MetaCell, ReminderStateView,
        ReviewPill, ReviewRow, ScanStripModel, refresh, start_timer,
    },
};
use openloops_graph::live::{ConnectionConfig, review::load_recent};
use openloops_inference::{
    blocks::CanonicalBlock,
    expectations::{Anchor, EventPassed, Expectation, Owner},
};
use slint::{ComponentHandle, ModelRc, Timer, VecModel};

// Review decision callback codes used by `ui/review.slint`.
const DECISION_REVIEW: i32 = 0;
const DECISION_MINE: i32 = 1;
const DECISION_DONE: i32 = 2;
const DECISION_DISMISSED: i32 = 3;
const DECISION_WATCHING: i32 = 4;
const DECISION_MOOT: i32 = 5;

#[allow(clippy::match_same_arms)]
fn decision_from_code(value: i32) -> Decision {
    match value {
        DECISION_MINE => Decision::Mine,
        DECISION_DONE => Decision::Done,
        DECISION_DISMISSED => Decision::Dismissed,
        DECISION_WATCHING => Decision::Watching,
        DECISION_MOOT => Decision::Moot,
        DECISION_REVIEW => Decision::Review,
        _ => Decision::Review,
    }
}

fn clamped_selection_index(current: usize, length: usize, delta: i32) -> usize {
    if delta < 0 {
        current.saturating_sub(1)
    } else {
        (current + 1).min(length.saturating_sub(1))
    }
}

fn no_usable_text(rejected: usize) -> &'static str {
    if rejected > 0 {
        "No usable expectations were returned. Evidence validation rejected suggestions; this is not a clean bill of health."
    } else {
        "No actionable expectations were identified in the successfully reviewed conversations."
    }
}

fn reminder_title(decision: Decision, action: &str) -> String {
    if decision == Decision::Watching {
        format!("Follow up: {action}")
    } else {
        action.to_owned()
    }
}

const REMINDER_VALIDATION_HINT: &str = "Enter a future local date/time and a title of 3–320 bytes. Ambiguous daylight-saving times need a different time.";
const TODO_URL: &str = "https://to-do.office.com/tasks/";

#[derive(Clone, Debug, PartialEq, Eq)]
struct DraftView {
    title: String,
    when: String,
    scheduled_line: String,
    valid: bool,
}

fn reminder_quick_pick_in_hour(now: chrono::DateTime<chrono::Local>) -> String {
    (now + chrono::Duration::hours(1))
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn reminder_quick_pick_tomorrow(now: chrono::DateTime<chrono::Local>) -> String {
    (now + chrono::Duration::days(1))
        .format("%Y-%m-%d 09:00")
        .to_string()
}

fn scheduled_instant_line(value: &str) -> (String, bool) {
    let Ok(at) = reminder_time(value) else {
        return (REMINDER_VALIDATION_HINT.into(), false);
    };
    let Some(time) = chrono::DateTime::from_timestamp(at, 0) else {
        return (REMINDER_VALIDATION_HINT.into(), false);
    };
    (
        format!(
            "Scheduled instant: {}",
            time.with_timezone(&chrono::Local)
                .format("%a %b %d, %Y at %H:%M %:z")
        ),
        true,
    )
}

/// The reminder title rule, shared by [`draft_view`] (as-you-type validity)
/// and the "Create this reminder" click handler (the same rule enforced
/// before dispatch): at least 3 non-whitespace-trimmed bytes, and the raw
/// (untrimmed) title never over 320 bytes.
fn title_is_valid(title: &str) -> bool {
    title.trim().len() >= 3 && title.len() <= 320
}

fn draft_view(draft: Option<&ReminderDraft>) -> Option<DraftView> {
    let draft = draft?;
    let (scheduled_line, time_valid) = scheduled_instant_line(&draft.when);
    Some(DraftView {
        title: draft.title.clone(),
        when: draft.when.clone(),
        scheduled_line: if draft.error.is_empty() {
            scheduled_line
        } else {
            draft.error.clone()
        },
        valid: time_valid && title_is_valid(&draft.title),
    })
}

/// Updates only the draft-panel properties from `model`'s current draft,
/// rather than the full [`refresh`]/`sync` -- an edit to the title or the
/// reminder time changes nothing else on screen (the title/time text boxes
/// are already current through their own two-way binding), so re-deriving
/// every row, pill, evidence card and conversation row on each keystroke
/// would be wasted work. A no-op once the draft has closed.
fn refresh_draft(model: &Rc<RefCell<AppModel>>, weak: &slint::Weak<AppWindow>) {
    let Some(window) = weak.upgrade() else {
        return;
    };
    let Some(view) = draft_view(model.borrow().review.draft.as_ref()) else {
        return;
    };
    window.set_draft_title(view.title.into());
    window.set_draft_when(view.when.into());
    window.set_draft_scheduled_line(view.scheduled_line.into());
    window.set_draft_valid(view.valid);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReminderView {
    state: &'static str,
    text: &'static str,
    marker: String,
}

fn reminder_state_view(record: &crate::loop_state::Record) -> ReminderView {
    match record.reminder {
        Reminder::None => ReminderView {
            state: "none",
            text: "",
            marker: String::new(),
        },
        Reminder::Created => ReminderView {
            state: "created",
            text: "Reminder created in Microsoft To Do. Manage its alerts and completion there; marking this loop handled does not modify the task.",
            marker: String::new(),
        },
        Reminder::Attempted => ReminderView {
            state: "attempted",
            text: "A reminder attempt has no confirmed outcome. Inspect Microsoft To Do before allowing another attempt.",
            marker: format!("Reference: {}", marker(&record.key)),
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EvidenceView {
    label: String,
    sender: String,
    time: String,
    quote: String,
    context: String,
    subject_note: String,
    url: String,
}

pub(crate) fn sender_label(message: &crate::review_model::ReviewMessage) -> String {
    if message.input.from_user {
        "You".into()
    } else {
        message
            .input
            .message
            .sender
            .as_ref()
            .map_or_else(|| "Other participant".into(), CanonicalBlock::as_string)
    }
}

fn gated_outlook_url(url: &str) -> String {
    if app_model::is_outlook_link(url) {
        url.to_owned()
    } else {
        String::new()
    }
}

/// Wraps a non-empty evidence quote in typographic quotes (Companion §4.4);
/// an empty quote stays empty so `!evidence.quote.is-empty` in `review.slint`
/// still gates the quote block correctly.
fn typographic_quote(quote: &str) -> String {
    if quote.is_empty() {
        String::new()
    } else {
        format!("“{quote}”")
    }
}

fn evidence_card(
    label: &str,
    anchor: &Anchor,
    messages: &[crate::review_model::ReviewMessage],
) -> EvidenceView {
    let message = messages
        .iter()
        .find(|message| message.input.handle == anchor.message);
    EvidenceView {
        label: label.into(),
        sender: message.map_or_else(String::new, sender_label),
        time: message.map_or_else(String::new, |message| message.date_label.clone()),
        quote: typographic_quote(&anchor.quote),
        context: if anchor.context == anchor.quote {
            String::new()
        } else {
            anchor.context.clone()
        },
        subject_note: String::new(),
        url: message.map_or_else(String::new, |message| gated_outlook_url(&message.web_link)),
    }
}

fn event_time_evidence_card(
    label: &str,
    event: &EventPassed,
    messages: &[crate::review_model::ReviewMessage],
) -> EvidenceView {
    let message = messages
        .iter()
        .find(|message| message.input.handle == event.message_handle);
    EvidenceView {
        label: label.into(),
        sender: message.map_or_else(String::new, sender_label),
        time: message.map_or_else(String::new, |message| message.date_label.clone()),
        quote: String::new(),
        context: String::new(),
        subject_note: if event.from_subject {
            "From the subject line of this message".into()
        } else {
            String::new()
        },
        url: message.map_or_else(String::new, |message| gated_outlook_url(&message.web_link)),
    }
}

fn evidence_cards(
    item: &Expectation,
    messages: &[crate::review_model::ReviewMessage],
) -> Vec<EvidenceView> {
    let mut cards = vec![evidence_card(
        "Original expectation",
        &item.evidence,
        messages,
    )];
    if let Some(anchor) = &item.deadline {
        cards.push(evidence_card("Deadline evidence", anchor, messages));
    }
    if let Some(anchor) = &item.event {
        cards.push(evidence_card("Event evidence", anchor, messages));
    }
    if let Some(event) = &item.event_passed {
        cards.push(event_time_evidence_card(
            "Event time evidence",
            event,
            messages,
        ));
    }
    cards
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompletionView {
    state: &'static str,
    label: String,
    sender: String,
    time: String,
    quote: String,
    cross_thread: bool,
    url: String,
}

fn completion_card(
    item: &Expectation,
    messages: &[crate::review_model::ReviewMessage],
) -> CompletionView {
    if let Some(anchor) = &item.resolution {
        let evidence = evidence_card(
            resolution_anchor_label(item.resolution_kind, item.cross_thread),
            anchor,
            messages,
        );
        CompletionView {
            state: "evidence",
            label: evidence.label,
            sender: evidence.sender,
            time: evidence.time,
            quote: evidence.quote,
            cross_thread: item.cross_thread,
            url: evidence.url,
        }
    } else if item.unverified_resolution {
        CompletionView {
            state: "unvalidated",
            label:
                "The analysis proposed a completion but it could not be validated; treat as open."
                    .into(),
            sender: String::new(),
            time: String::new(),
            quote: String::new(),
            cross_thread: false,
            url: String::new(),
        }
    } else {
        CompletionView {
            state: "none",
            label: "No matching completion was identified in the scanned conversation. Work may have happened elsewhere or outside this history window.".into(),
            sender: String::new(), time: String::new(), quote: String::new(), cross_thread: false, url: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ConversationRowView {
    initials: String,
    sender: String,
    meta: String,
    body: String,
    sent: bool,
    quoted_history: String,
}

fn initials(name: &str) -> String {
    name.split(|character: char| character.is_whitespace() || character == '(' || character == '<')
        .filter(|part| !part.is_empty())
        .take(2)
        .filter_map(|part| part.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

fn conversation_rows(
    messages: &[crate::review_model::ReviewMessage],
    source: &crate::review_model::ReviewMessage,
) -> Vec<ConversationRowView> {
    messages
        .iter()
        .filter(|message| {
            message.account == source.account && message.conversation == source.conversation
        })
        .map(|message| {
            let sender = sender_label(message);
            ConversationRowView {
                initials: initials(&sender),
                sender,
                meta: format!("{} · {}", message.date_label, source_short(&message.source)),
                body: message
                    .input
                    .message
                    .body_blocks
                    .iter()
                    .map(CanonicalBlock::as_string)
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                sent: message.input.from_user,
                quoted_history: message
                    .input
                    .message
                    .quote_blocks
                    .iter()
                    .map(CanonicalBlock::as_string)
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            }
        })
        .collect()
}

struct ReviewUiState {
    selected: Option<[u8; 32]>,
    filter: Filter,
    coverage_open: bool,
}

impl Default for ReviewUiState {
    fn default() -> Self {
        Self {
            selected: None,
            filter: Filter::All,
            coverage_open: false,
        }
    }
}

thread_local! {
    static REVIEW_UI: RefCell<ReviewUiState> = RefCell::new(ReviewUiState::default());
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReviewRowView {
    handle: usize,
    group: ListGroup,
    first_in_group: bool,
    selected: bool,
    action: String,
    waiting_source: String,
    aging_status: String,
    date: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PillView {
    text: String,
    kind: &'static str,
}

#[allow(clippy::struct_excessive_bools)]
struct SelectedView {
    title: String,
    meta: Vec<(String, String)>,
    uncertainty: String,
    pills: Vec<PillView>,
    open: bool,
    terminal: bool,
    can_track: bool,
    can_watch: bool,
    can_remind: bool,
    draft: Option<DraftView>,
    reminder: ReminderView,
    evidence: Vec<EvidenceView>,
    completion: CompletionView,
    conversation_title: String,
    conversation: Vec<ConversationRowView>,
}

fn source_message<'a>(
    review: &'a ReviewState,
    item: &Expectation,
) -> Option<&'a crate::review_model::ReviewMessage> {
    review
        .messages
        .iter()
        .find(|message| message.input.handle == item.evidence.message)
}

fn source_short(source: &str) -> &str {
    source.rsplit_once(" / ").map_or(source, |(_, short)| short)
}

fn owner_label(item: &Expectation, decision: Decision) -> &'static str {
    if decision == Decision::Mine {
        "You (confirmed)"
    } else {
        match item.owner {
            Owner::You => "You (suggested)",
            Owner::Team => "Team — no individual owner established",
            Owner::Unclear => "Unclear — confirm responsibility",
        }
    }
}

fn status_pill(item: &Expectation, card: &CardContext) -> PillView {
    let (text, kind) = match card.record.decision {
        Decision::Done => ("Handled", "neutral"),
        Decision::Dismissed => ("Dismissed – not mine", "neutral"),
        Decision::Moot => ("No longer relevant", "neutral"),
        Decision::Mine => ("Tracking", "success"),
        Decision::Watching => ("Watching team follow-up", "watch"),
        Decision::Review if item.resolution.is_some() || item.event_passed.is_some() => {
            ("Resolved — later reply found", "success")
        }
        Decision::Review => ("Needs your review", "brand"),
    };
    PillView {
        text: text.into(),
        kind,
    }
}

fn pills_for(item: &Expectation, card: &CardContext) -> Vec<PillView> {
    let mut pills = vec![status_pill(item, card)];
    if let Some(deadline) = &card.deadline {
        pills.push(PillView {
            text: deadline_label(deadline),
            kind: if !card.closed && crate::review_model::is_past_due(deadline) {
                "danger"
            } else {
                "brand"
            },
        });
    }
    match card.record.reminder {
        Reminder::None => {}
        Reminder::Created => pills.push(PillView {
            text: "To Do reminder set".into(),
            kind: "brand",
        }),
        Reminder::Attempted => pills.push(PillView {
            text: "Reminder unconfirmed".into(),
            kind: "warning",
        }),
    }
    pills
}

fn visible_handles(
    review: &ReviewState,
    cards: &[Option<CardContext>],
    filter: Filter,
    show_handled: bool,
) -> Vec<usize> {
    let Some(analysis) = &review.analysis else {
        return vec![];
    };
    let ordered = card_order(cards);
    [
        ListGroup::PastDue,
        ListGroup::Due,
        ListGroup::NoFixedDeadline,
        ListGroup::Closed,
    ]
    .into_iter()
    .flat_map(|group| {
        ordered.iter().copied().filter({
            let cards = &cards;
            move |&index| {
                cards[index].as_ref().is_some_and(|card| {
                    filter.matches(analysis.items[index].owner)
                        && list_group(card) == group
                        && !card_hidden(
                            card.closed,
                            show_handled,
                            review
                                .draft
                                .as_ref()
                                .is_some_and(|draft| draft.key == card.record.key),
                        )
                })
            }
        })
    })
    .collect()
}

fn retained_selection(
    current: Option<[u8; 32]>,
    visible: &[usize],
    cards: &[Option<CardContext>],
) -> Option<[u8; 32]> {
    current
        .filter(|selected| {
            visible.iter().any(|&index| {
                cards[index]
                    .as_ref()
                    .is_some_and(|card| card.record.key == *selected)
            })
        })
        .or_else(|| {
            visible
                .first()
                .and_then(|&index| cards[index].as_ref().map(|card| card.record.key))
        })
}

fn review_rows(
    review: &ReviewState,
    filter: Filter,
    show_handled: bool,
    selected: Option<[u8; 32]>,
    cards: &[Option<CardContext>],
) -> Vec<ReviewRowView> {
    let Some(analysis) = &review.analysis else {
        return vec![];
    };
    let visible = visible_handles(review, cards, filter, show_handled);
    let mut prior = None;
    visible
        .into_iter()
        .filter_map(|index| {
            let item = &analysis.items[index];
            let card = cards[index].as_ref()?;
            let source = source_message(review, item)?;
            let group = list_group(card);
            let first_in_group = prior != Some(group);
            prior = Some(group);
            let aging = card
                .deadline
                .as_ref()
                .map_or_else(|| "No deadline stated".into(), deadline_label);
            let base = status_base_label(card.record.decision, item);
            let status = status_label(
                &base,
                status_shows_cross_thread(
                    card.record.decision,
                    item.resolution.is_some(),
                    item.cross_thread,
                ),
            );
            Some(ReviewRowView {
                handle: index,
                group,
                first_in_group,
                selected: selected == Some(card.record.key),
                action: item.action.clone(),
                waiting_source: format!(
                    "Waiting: {} · {}",
                    item.waiting_party,
                    source_short(&source.source)
                ),
                aging_status: format!("{aging} · {status}"),
                date: source.date_label.clone(),
            })
        })
        .collect()
}

fn selected_view(
    review: &ReviewState,
    selected: Option<[u8; 32]>,
    cards: &[Option<CardContext>],
) -> Option<SelectedView> {
    let analysis = review.analysis.as_ref()?;
    let selected = selected?;
    let index = cards.iter().position(|card| {
        card.as_ref()
            .is_some_and(|card| card.record.key == selected)
    })?;
    let item = analysis.items.get(index)?;
    let card = cards.get(index)?.as_ref()?;
    let source = source_message(review, item)?;
    let deadline = if let Some(anchor) = &item.deadline {
        format!("“{}”", anchor.quote)
    } else if item.unverified_deadline {
        "A deadline was stated, but its quotation could not be verified.".into()
    } else {
        "Not specified".into()
    };
    let draft_open = review
        .draft
        .as_ref()
        .is_some_and(|draft| draft.key == card.record.key);
    Some(SelectedView {
        title: item.action.clone(),
        meta: vec![
            (
                "Responsible".into(),
                owner_label(item, card.record.decision).into(),
            ),
            ("Waiting on this".into(), item.waiting_party.clone()),
            ("Deadline stated in email".into(), deadline),
            ("Source".into(), source.source.clone()),
        ],
        uncertainty: item.uncertainty.clone(),
        pills: pills_for(item, card),
        open: !card.closed,
        terminal: card.terminal,
        can_track: !card.closed && card.record.decision != Decision::Mine,
        can_watch: !card.closed
            && item.owner != Owner::You
            && card.record.decision != Decision::Watching,
        can_remind: !card.closed && reminder_button_enabled(card.record.reminder, draft_open),
        draft: draft_view(
            review
                .draft
                .as_ref()
                .filter(|draft| draft.key == card.record.key),
        ),
        reminder: reminder_state_view(&card.record),
        evidence: evidence_cards(item, &review.messages),
        completion: completion_card(item, &review.messages),
        conversation_title: format!(
            "Full scanned conversation · {} · {} messages",
            source.input.message.subject.as_string(),
            review
                .messages
                .iter()
                .filter(|message| message.account == source.account
                    && message.conversation == source.conversation)
                .count()
        ),
        conversation: conversation_rows(&review.messages, source),
    })
}

#[allow(clippy::too_many_lines)]
pub(crate) fn scan_strip_view(
    strip: &ScanStrip,
    model: &AppModel,
    cards: &[Option<CardContext>],
) -> (ScanStripModel, String) {
    match strip {
        ScanStrip::Scanning {
            phase,
            conversation_index,
            conversation_total,
            processed,
            total,
            elapsed_secs,
            percent,
            stopping,
        } => {
            let conversation = format!("Conversation {conversation_index} of {conversation_total}");
            let messages = format!("{processed} / {total} messages");
            (
                ScanStripModel {
                    state: "scanning".into(),
                    title: (*phase).into(),
                    conversation: conversation.clone().into(),
                    messages: messages.clone().into(),
                    elapsed: format!("{elapsed_secs}s on this request").into(),
                    provider: model.provider_disclosure().into(),
                    summary: if *stopping {
                        "Stopping; the current request is dropped within a second."
                    } else {
                        ""
                    }
                    .into(),
                    coverage_label: "".into(),
                    coverage_notes: "".into(),
                    progress: f32::from(*percent) / 100.0,
                    incomplete: false,
                },
                format!("Scanning · {conversation} · {messages}"),
            )
        }
        ScanStrip::Finished {
            incomplete,
            summary: scan_summary,
            coverage_notes,
        } => {
            let expectations = model
                .review
                .analysis
                .as_ref()
                .map_or_else(String::new, |analysis| {
                    expectations_summary(analysis, cards, &model.review.analysis_model)
                });
            let summary = if scan_summary.is_empty() {
                expectations
            } else if expectations.is_empty() {
                scan_summary.clone()
            } else {
                format!("{scan_summary} {expectations}")
            };
            let summary = if *incomplete {
                format!(
                    "{summary} Partial coverage. These results cannot establish that all outstanding work has been found."
                )
            } else {
                summary
            };
            let coverage_label = match model.review.source_failures {
                0 => "Coverage details".to_string(),
                1 => "Coverage: 1 source incomplete".to_string(),
                count => format!("Coverage: {count} sources incomplete"),
            };
            (
                ScanStripModel {
                    state: "finished".into(),
                    title: if *incomplete {
                        "Scan incomplete"
                    } else {
                        "Scan finished"
                    }
                    .into(),
                    conversation: "".into(),
                    messages: "".into(),
                    elapsed: "".into(),
                    provider: model.provider_disclosure().into(),
                    summary: summary.into(),
                    coverage_label: coverage_label.into(),
                    coverage_notes: coverage_notes.join("\n").into(),
                    progress: 1.0,
                    incomplete: *incomplete,
                },
                String::new(),
            )
        }
        ScanStrip::Idle => (
            ScanStripModel {
                state: "idle".into(),
                title: "".into(),
                conversation: "".into(),
                messages: "".into(),
                elapsed: "".into(),
                provider: model.provider_disclosure().into(),
                summary: "".into(),
                coverage_label: "".into(),
                coverage_notes: "".into(),
                progress: 0.0,
                incomplete: false,
            },
            String::new(),
        ),
    }
}

#[allow(clippy::too_many_lines)]
fn sync_review_inner(
    model: &AppModel,
    review_ui: &mut ReviewUiState,
    window: &AppWindow,
    cards: &[Option<CardContext>],
    busy: bool,
    review_scanning: bool,
    mut strip: ScanStripModel,
) {
    window.set_review_scanning(review_scanning);
    window.set_can_scan(model.ready_for_review() && !busy);
    window.set_can_rescan(
        !model.review.messages.is_empty()
            && !busy
            && !model.active_key().is_empty()
            && !model.selected_model().is_empty(),
    );
    window.set_review_filter_index(match review_ui.filter {
        Filter::All => 0,
        Filter::Mine => 1,
        Filter::Team => 2,
    });
    window.set_show_handled(model.review.show_handled);
    window.set_show_handled_label(SHOW_HANDLED_LABEL.into());
    if model.review.analysis.is_none()
        && !model.review_status.succeeded
        && !model.review_status.lines.is_empty()
    {
        strip.state = "warning".into();
        strip.title = "Scan incomplete".into();
        strip.summary = model.review_status.lines.join("\n").into();
        strip.incomplete = true;
    }
    window.set_scan_strip(strip);
    window.set_coverage_open(review_ui.coverage_open);
    window.set_review_has_analysis(model.review.analysis.is_some());
    let visible = visible_handles(
        &model.review,
        cards,
        review_ui.filter,
        model.review.show_handled,
    );
    review_ui.selected = retained_selection(review_ui.selected, &visible, cards);
    let row_views = review_rows(
        &model.review,
        review_ui.filter,
        model.review.show_handled,
        review_ui.selected,
        cards,
    );
    let rows = row_views
        .iter()
        .cloned()
        .map(|row| ReviewRow {
            handle: i32::try_from(row.handle).unwrap_or(i32::MAX),
            group_title: match row.group {
                ListGroup::PastDue => "Past due",
                ListGroup::Due => "Due",
                ListGroup::NoFixedDeadline => "No fixed deadline",
                ListGroup::Closed => "Resolved, handled or dismissed",
            }
            .into(),
            group_kind: match row.group {
                ListGroup::PastDue => "danger",
                ListGroup::Due => "brand",
                ListGroup::NoFixedDeadline => "neutral",
                ListGroup::Closed => "success",
            }
            .into(),
            group_count: i32::try_from(
                row_views
                    .iter()
                    .filter(|candidate| candidate.group == row.group)
                    .count(),
            )
            .unwrap_or(i32::MAX),
            first_in_group: row.first_in_group,
            selected: row.selected,
            action: row.action.into(),
            waiting_source: row.waiting_source.into(),
            aging_status: row.aging_status.into(),
            aging_kind: match row.group {
                ListGroup::PastDue => "danger",
                ListGroup::Due => "brand",
                ListGroup::Closed => "success",
                ListGroup::NoFixedDeadline => "neutral",
            }
            .into(),
            date: row.date.into(),
        })
        .collect::<Vec<_>>();
    window.set_review_rows(ModelRc::new(VecModel::from(rows)));
    window.set_review_open_count(i32::try_from(open_badge_count(cards)).unwrap_or(i32::MAX));
    let (no_usable, no_usable_text) =
        model
            .review
            .analysis
            .as_ref()
            .map_or((false, String::new()), |analysis| {
                if analysis.items.is_empty() {
                    (true, no_usable_text(analysis.rejected).into())
                } else {
                    (false, String::new())
                }
            });
    window.set_review_no_usable_items(no_usable);
    window.set_review_no_usable_text(no_usable_text.into());
    if let Some(selected) = selected_view(&model.review, review_ui.selected, cards) {
        window.set_review_has_selection(true);
        window.set_review_title(selected.title.into());
        window.set_review_meta(ModelRc::new(VecModel::from(
            selected
                .meta
                .into_iter()
                .map(|(label, value)| MetaCell {
                    label: label.into(),
                    value: value.into(),
                })
                .collect::<Vec<_>>(),
        )));
        window.set_review_pills(ModelRc::new(VecModel::from(
            selected
                .pills
                .into_iter()
                .map(|pill| ReviewPill {
                    text: pill.text.into(),
                    kind: pill.kind.into(),
                })
                .collect::<Vec<_>>(),
        )));
        window.set_review_uncertainty(selected.uncertainty.into());
        window.set_selected_open(selected.open);
        window.set_selected_terminal(selected.terminal);
        window.set_can_track(selected.can_track);
        window.set_can_watch(selected.can_watch);
        window.set_can_remind(selected.can_remind && !busy);
        if let Some(draft) = selected.draft {
            window.set_draft_open(true);
            window.set_draft_title(draft.title.into());
            window.set_draft_when(draft.when.into());
            window.set_draft_scheduled_line(draft.scheduled_line.into());
            window.set_draft_valid(draft.valid);
        } else {
            window.set_draft_open(false);
            window.set_draft_title("".into());
            window.set_draft_when("".into());
            window.set_draft_scheduled_line("".into());
            window.set_draft_valid(false);
        }
        window.set_reminder_state(ReminderStateView {
            state: selected.reminder.state.into(),
            text: selected.reminder.text.into(),
            marker: selected.reminder.marker.into(),
        });
        window.set_evidence_cards(ModelRc::new(VecModel::from(
            selected
                .evidence
                .into_iter()
                .map(|evidence| EvidenceCard {
                    label: evidence.label.into(),
                    sender: evidence.sender.into(),
                    time: evidence.time.into(),
                    quote: evidence.quote.into(),
                    context: evidence.context.into(),
                    subject_note: evidence.subject_note.into(),
                    url: evidence.url.into(),
                })
                .collect::<Vec<_>>(),
        )));
        window.set_completion_card(CompletionCard {
            state: selected.completion.state.into(),
            label: selected.completion.label.into(),
            sender: selected.completion.sender.into(),
            time: selected.completion.time.into(),
            quote: selected.completion.quote.into(),
            cross_thread: selected.completion.cross_thread,
            url: selected.completion.url.into(),
        });
        window.set_conversation_title(selected.conversation_title.into());
        window.set_conversation_rows(ModelRc::new(VecModel::from(
            selected
                .conversation
                .into_iter()
                .map(|message| ConversationRow {
                    initials: message.initials.into(),
                    sender: message.sender.into(),
                    meta: message.meta.into(),
                    body: message.body.into(),
                    sent: message.sent,
                    quoted_history: message.quoted_history.into(),
                })
                .collect::<Vec<_>>(),
        )));
    } else {
        window.set_review_has_selection(false);
        window.set_review_title("".into());
        window.set_review_meta(ModelRc::default());
        window.set_review_pills(ModelRc::default());
        window.set_review_uncertainty("".into());
        window.set_selected_open(false);
        window.set_selected_terminal(false);
        window.set_can_track(false);
        window.set_can_watch(false);
        window.set_can_remind(false);
        window.set_draft_open(false);
        window.set_draft_title("".into());
        window.set_draft_when("".into());
        window.set_draft_scheduled_line("".into());
        window.set_draft_valid(false);
        window.set_reminder_state(ReminderStateView::default());
        window.set_evidence_cards(ModelRc::default());
        window.set_completion_card(CompletionCard::default());
        window.set_conversation_title("".into());
        window.set_conversation_rows(ModelRc::default());
    }
    let decision_error = model.review.decisions.error.as_deref();
    window.set_review_action_status(decision_error.unwrap_or(&model.review.action_status).into());
    window.set_review_action_status_succeeded(
        decision_error.is_none() && model.review.action_status_succeeded,
    );
}

pub(crate) fn sync_review(
    model: &AppModel,
    window: &AppWindow,
    cards: &[Option<CardContext>],
    busy: bool,
    review_scanning: bool,
    strip: ScanStripModel,
) {
    REVIEW_UI.with(|state| {
        sync_review_inner(
            model,
            &mut state.borrow_mut(),
            window,
            cards,
            busy,
            review_scanning,
            strip,
        );
    });
}

/// Applies a "the task exists" / "no task was created" reconcile answer for
/// the record at `key`, matching `apply_decision_change`'s own reminder
/// state to it. Only replaces `apply_decision_change`'s own status text (and
/// marks it a success) when that change actually saved; a failure (e.g. the
/// saved-decision storage bound) must keep its error text and
/// `succeeded = false` rather than being papered over here. Factored out of
/// `on_reconcile_reminder` so both outcomes are directly testable without a
/// live `AppWindow`.
fn apply_reconcile(review: &mut ReviewState, key: [u8; 32], exists: bool) {
    let record = review.decisions.get(&key);
    let reminder = if exists {
        Reminder::Created
    } else {
        Reminder::None
    };
    review.apply_decision_change(key, record.decision, reminder);
    if review.action_status_succeeded {
        review.action_status = if exists {
            "Recorded: the To Do task exists. Manage it in Microsoft To Do."
        } else {
            "Recorded: no task was created. You can set a reminder again."
        }
        .into();
    }
}

fn dispatch_pending_reminder(model: &mut AppModel) -> bool {
    let Some((key, request)) = model.review.pending_reminder.take() else {
        return false;
    };
    match ConnectionConfig::new(model.client_id.trim(), None) {
        Ok(config) => {
            model.start(
                Service::Review,
                "Sign in to create the reviewed Microsoft To Do reminder",
                move || {
                    Outcome::Reminder(
                        key,
                        openloops_graph::live::reminders::create(&config, &request),
                    )
                },
                || {},
            );
            true
        }
        Err(error) => {
            let mut record = model.review.decisions.get(&key);
            record.reminder = Reminder::None;
            record.updated = crate::loop_state::now();
            let _ = model.review.decisions.update(record);
            model.review.action_status = error.to_string();
            model.review.action_status_succeeded = false;
            false
        }
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn register_callbacks(
    window: &AppWindow,
    model: &Rc<RefCell<AppModel>>,
    timer: &Rc<Timer>,
) {
    // Set once here rather than in every `sync`: the To Do URL never
    // changes, and `review.slint` never embeds the literal itself (it calls
    // back through `open-external(root.todo-url)`).
    window.set_todo_url(TODO_URL.into());
    let model = Rc::clone(model);
    let timer = Rc::clone(timer);
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        let timer = Rc::clone(&timer);
        window.on_scan_inboxes(move || {
            let config = {
                let model = model.borrow();
                ConnectionConfig::new(model.client_id.trim(), Some(&model.shared))
                    .and_then(|config| config.with_groups(Some(&model.groups)))
            };
            match config {
                Ok(config) => {
                    let mut model_ref = model.borrow_mut();
                    let decisions = std::mem::take(&mut model_ref.review.decisions);
                    model_ref.review = ReviewState::default();
                    model_ref.review.decisions = decisions;
                    model_ref.review_status = Status::default();
                    model_ref.start(
                        Service::Review,
                        "Complete Microsoft sign-in; then scanning recent messages",
                        move || Outcome::Mail(load_recent(&config)),
                        || {},
                    );
                    REVIEW_UI.with(|state| state.borrow_mut().selected = None);
                    start_timer(&timer);
                }
                Err(error) => {
                    model.borrow_mut().review_status = Status {
                        lines: vec![error.to_string()],
                        succeeded: false,
                    };
                }
            }
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_stop_scan(move || {
            let model_ref = model.borrow();
            if model_ref.pending_service == Service::Review
                && let Some(progress) = &model_ref.scan_progress
            {
                progress.cancel.store(true, Ordering::Relaxed);
            }
            drop(model_ref);
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        let timer = Rc::clone(&timer);
        window.on_rescan(move || {
            if model.borrow().pending.is_none() {
                model.borrow_mut().start_scan(|| {});
                start_timer(&timer);
                refresh(&model, &weak);
            }
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_clear_results(move || {
            let mut model_ref = model.borrow_mut();
            if let Some(progress) = &model_ref.scan_progress {
                progress.cancel.store(true, Ordering::Relaxed);
            }
            model_ref.pending = None;
            model_ref.scan_progress = None;
            let decisions = std::mem::take(&mut model_ref.review.decisions);
            model_ref.review = ReviewState::default();
            model_ref.review.decisions = decisions;
            model_ref.review.action_status =
                "Results and mail cleared from memory. Saved decisions and To Do tasks are preserved."
                    .into();
            model_ref.review.action_status_succeeded = true;
            model_ref.review_status = Status::default();
            REVIEW_UI.with(|state| {
                let mut state = state.borrow_mut();
                state.selected = None;
                state.coverage_open = false;
            });
            drop(model_ref);
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_review_filter_selected(move |index| {
            REVIEW_UI.with(|state| {
                state.borrow_mut().filter = match index {
                    1 => Filter::Mine,
                    2 => Filter::Team,
                    _ => Filter::All,
                };
            });
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_show_handled_toggled(move |show| {
            model.borrow_mut().review.show_handled = show;
            refresh(&model, &weak);
        });
    }
    window.on_coverage_toggled(|open| {
        REVIEW_UI.with(|state| state.borrow_mut().coverage_open = open);
    });
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_select_review_row(move |handle| {
            let selected = usize::try_from(handle).ok().and_then(|index| {
                let model_ref = model.borrow();
                let analysis = model_ref.review.analysis.as_ref()?;
                model_ref
                    .review
                    .card_contexts(&analysis.items)
                    .get(index)?
                    .as_ref()
                    .map(|card| card.record.key)
            });
            REVIEW_UI.with(|state| state.borrow_mut().selected = selected);
            model.borrow_mut().review.action_status.clear();
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_move_review_selection(move |delta| {
            REVIEW_UI.with(|state| {
                let mut state = state.borrow_mut();
                let model_ref = model.borrow();
                let cards = model_ref
                    .review
                    .analysis
                    .as_ref()
                    .map(|analysis| model_ref.review.card_contexts(&analysis.items))
                    .unwrap_or_default();
                let visible = visible_handles(
                    &model_ref.review,
                    &cards,
                    state.filter,
                    model_ref.review.show_handled,
                );
                if !visible.is_empty() {
                    let current = state
                        .selected
                        .and_then(|selected| {
                            visible.iter().position(|&index| {
                                cards[index]
                                    .as_ref()
                                    .is_some_and(|card| card.record.key == selected)
                            })
                        })
                        .unwrap_or(0);
                    let next = clamped_selection_index(current, visible.len(), delta);
                    state.selected = visible
                        .get(next)
                        .and_then(|&index| cards[index].as_ref().map(|card| card.record.key));
                }
            });
            model.borrow_mut().review.action_status.clear();
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_review_decision(move |value| {
            let selected = REVIEW_UI.with(|state| state.borrow().selected);
            let mut model_ref = model.borrow_mut();
            let Some(analysis) = &model_ref.review.analysis else {
                return;
            };
            let cards = model_ref.review.card_contexts(&analysis.items);
            let Some(index) = selected.and_then(|selected| {
                cards.iter().position(|card| {
                    card.as_ref()
                        .is_some_and(|card| card.record.key == selected)
                })
            }) else {
                return;
            };
            let Some(item) = analysis.items.get(index) else {
                return;
            };
            let Some(source) = source_message(&model_ref.review, item) else {
                return;
            };
            let key = model_ref.review.decisions.fingerprint(
                &source.account,
                &source.id,
                &item.action_phrase,
            );
            let reminder = model_ref.review.decisions.get(&key).reminder;
            let decision = decision_from_code(value);
            model_ref
                .review
                .apply_decision_change(key, decision, reminder);
            drop(model_ref);
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_open_reminder(move || {
            let selected = REVIEW_UI.with(|state| state.borrow().selected);
            let mut model_ref = model.borrow_mut();
            let Some(analysis) = &model_ref.review.analysis else { return };
            let cards = model_ref.review.card_contexts(&analysis.items);
            let Some(index) = selected.and_then(|selected| {
                cards.iter().position(|card| {
                    card.as_ref()
                        .is_some_and(|card| card.record.key == selected)
                })
            }) else { return };
            let Some(item) = analysis.items.get(index) else { return };
            let Some(source) = source_message(&model_ref.review, item) else { return };
            let account = source.account.clone();
            let source_id = source.id.clone();
            let action_phrase = item.action_phrase.clone();
            let key = model_ref.review.decisions.fingerprint(&account, &source_id, &action_phrase);
            let record = model_ref.review.decisions.get(&key);
            let draft_open_for_this_card = model_ref
                .review
                .draft
                .as_ref()
                .is_some_and(|draft| draft.key == key);
            if reminder_button_enabled(record.reminder, draft_open_for_this_card) {
                let next = decision_after_setting_reminder(record.decision);
                let title = reminder_title(record.decision, &item.action);
                model_ref.review.apply_decision_change(key, next, record.reminder);
                model_ref.review.draft = Some(ReminderDraft {
                    key,
                    account,
                    title,
                    when: default_reminder(),
                    error: String::new(),
                    prior_decision: record.decision,
                });
                if record.decision == Decision::Review {
                    model_ref.review.action_status = "Opening a reminder draft tracks this as yours. Cancel restores the previous decision.".into();
                    model_ref.review.action_status_succeeded = true;
                }
            }
            drop(model_ref);
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_draft_title_edited(move |value| {
            if let Some(draft) = &mut model.borrow_mut().review.draft {
                draft.title = value.to_string();
                draft.error.clear();
            }
            refresh_draft(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_draft_when_edited(move |value| {
            if let Some(draft) = &mut model.borrow_mut().review.draft {
                draft.when = value.to_string();
                draft.error.clear();
            }
            refresh_draft(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_draft_in_hour(move || {
            if let Some(draft) = &mut model.borrow_mut().review.draft {
                draft.when = reminder_quick_pick_in_hour(chrono::Local::now());
                draft.error.clear();
            }
            refresh_draft(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_draft_tomorrow(move || {
            if let Some(draft) = &mut model.borrow_mut().review.draft {
                draft.when = reminder_quick_pick_tomorrow(chrono::Local::now());
                draft.error.clear();
            }
            refresh_draft(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_cancel_reminder(move || {
            let draft = model.borrow_mut().review.draft.take();
            if let Some(draft) = draft {
                let mut model_ref = model.borrow_mut();
                model_ref
                    .review
                    .revert_draft_decision(draft.prior_decision, draft.key);
            }
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        let timer = Rc::clone(&timer);
        window.on_create_reminder(move || {
            let mut model_ref = model.borrow_mut();
            let Some(draft) = model_ref.review.draft.take() else {
                return;
            };
            match reminder_time(&draft.when) {
                Ok(at) if title_is_valid(&draft.title) => {
                    match model_ref.review.decisions.begin_reminder(draft.key) {
                        Ok(()) => {
                            model_ref.review.pending_reminder = Some((
                                draft.key,
                                openloops_graph::live::reminders::ReminderRequest {
                                    account: draft.account,
                                    title: draft.title.trim().into(),
                                    at_utc: at,
                                    marker: marker(&draft.key),
                                },
                            ));
                            if dispatch_pending_reminder(&mut model_ref) {
                                start_timer(&timer);
                            }
                        }
                        Err(error) => {
                            model_ref.review.draft = Some(ReminderDraft { error, ..draft });
                        }
                    }
                }
                _ => {
                    model_ref.review.draft = Some(ReminderDraft {
                        error: REMINDER_VALIDATION_HINT.into(),
                        ..draft
                    });
                }
            }
            drop(model_ref);
            refresh(&model, &weak);
        });
    }
    {
        let model = Rc::clone(&model);
        let weak = window.as_weak();
        window.on_reconcile_reminder(move |exists| {
            let selected = REVIEW_UI.with(|state| state.borrow().selected);
            let Some(key) = selected else { return };
            let mut model_ref = model.borrow_mut();
            apply_reconcile(&mut model_ref.review, key, exists);
            drop(model_ref);
            refresh(&model, &weak);
        });
    }
    window.on_open_external(|url| {
        if url.as_str() == TODO_URL || app_model::is_outlook_link(&url) {
            let _ = opener::open(url.as_str());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{deadline_view::DeadlineView, loop_state::Record, review_model::scan_strip};

    fn model() -> AppModel {
        AppModel::with_store(Ok(None))
    }

    #[test]
    fn review_rows_group_and_order_the_fixture_cards() {
        let review = crate::review_model::layout_fixture();
        let cards = review.card_contexts(&review.analysis.as_ref().unwrap().items);
        let selected = cards[0].as_ref().map(|card| card.record.key);
        let rows = review_rows(&review, Filter::All, false, selected, &cards);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].first_in_group);
        assert_ne!(rows[0].group, ListGroup::Closed);
        assert_eq!(rows[0].group, rows[1].group);
        assert_eq!(rows[0].group, rows[2].group);
        assert!(!rows[1].first_in_group);
        assert_eq!(rows[0].handle, 0);
        assert_eq!(rows[1].handle, 1);
        assert_eq!(rows[2].handle, 2);
        assert!(rows[0].selected);
        assert!(rows[0].waiting_source.contains("Waiting:"));
    }

    #[test]
    fn review_filter_is_applied_in_rust() {
        let review = crate::review_model::layout_fixture();
        let cards = review.card_contexts(&review.analysis.as_ref().unwrap().items);
        assert_eq!(
            visible_handles(&review, &cards, Filter::All, false),
            vec![0, 1, 2]
        );
        assert_eq!(
            visible_handles(&review, &cards, Filter::Mine, false),
            vec![0, 2]
        );
        assert_eq!(
            visible_handles(&review, &cards, Filter::Team, false),
            vec![1]
        );
    }

    #[test]
    fn review_selection_survives_resync_when_still_visible() {
        let mut review = crate::review_model::layout_fixture();
        let cards = review.card_contexts(&review.analysis.as_ref().unwrap().items);
        let selected = cards[1].as_ref().unwrap().record.key;
        review.analysis.as_mut().unwrap().items.swap(0, 1);
        let reordered = review.card_contexts(&review.analysis.as_ref().unwrap().items);
        let retained = retained_selection(Some(selected), &[0, 1], &reordered);
        assert_eq!(retained, Some(selected));
        assert_eq!(retained_selection(None, &[], &[]), None);
    }

    #[test]
    fn selected_view_covers_deadline_metadata_and_actions() {
        let mut review = crate::review_model::layout_fixture();
        let initial_cards = review.card_contexts(&review.analysis.as_ref().unwrap().items);
        let key = initial_cards[0].as_ref().unwrap().record.key;
        review.draft = None;
        review.analysis.as_mut().unwrap().items[0].resolution = None;
        review.analysis.as_mut().unwrap().items[0].cross_thread = false;
        let record = review
            .decisions
            .records
            .iter_mut()
            .find(|record| record.key == key)
            .unwrap();
        record.decision = Decision::Review;
        record.reminder = Reminder::None;
        let cards = review.card_contexts(&review.analysis.as_ref().unwrap().items);
        let selected = selected_view(&review, Some(key), &cards).unwrap();
        assert_eq!(
            selected.meta[0],
            ("Responsible".into(), "You (suggested)".into())
        );
        assert_eq!(
            selected.meta[2],
            ("Deadline stated in email".into(), "“Friday”".into())
        );
        assert_eq!(selected.meta[3].0, "Source");
        assert!(selected.open && !selected.terminal && selected.can_track);
        assert!(!selected.can_watch);
        assert!(selected.can_remind);

        let item = &mut review.analysis.as_mut().unwrap().items[0];
        item.deadline = None;
        item.unverified_deadline = true;
        let cards = review.card_contexts(&review.analysis.as_ref().unwrap().items);
        assert_eq!(
            selected_view(&review, Some(key), &cards).unwrap().meta[2].1,
            "A deadline was stated, but its quotation could not be verified."
        );
        review.analysis.as_mut().unwrap().items[0].unverified_deadline = false;
        let cards = review.card_contexts(&review.analysis.as_ref().unwrap().items);
        assert_eq!(
            selected_view(&review, Some(key), &cards).unwrap().meta[2].1,
            "Not specified"
        );

        review.apply_decision_change(key, Decision::Done, Reminder::None);
        let cards = review.card_contexts(&review.analysis.as_ref().unwrap().items);
        let selected = selected_view(&review, Some(key), &cards).unwrap();
        assert!(!selected.open && selected.terminal);
        assert!(!selected.can_track && !selected.can_watch && !selected.can_remind);
    }

    #[test]
    fn owner_and_source_labels_cover_all_branches() {
        let mut review = crate::review_model::layout_fixture();
        let item = &mut review.analysis.as_mut().unwrap().items[0];
        assert_eq!(owner_label(item, Decision::Mine), "You (confirmed)");
        assert_eq!(owner_label(item, Decision::Review), "You (suggested)");
        item.owner = Owner::Team;
        assert_eq!(
            owner_label(item, Decision::Review),
            "Team — no individual owner established"
        );
        item.owner = Owner::Unclear;
        assert_eq!(
            owner_label(item, Decision::Review),
            "Unclear — confirm responsibility"
        );
        assert_eq!(source_short("Personal mailbox / Inbox"), "Inbox");
        assert_eq!(source_short("Inbox"), "Inbox");
    }

    #[test]
    fn status_pills_cover_resolution_and_terminal_decisions() {
        let review = crate::review_model::layout_fixture();
        let mut item = review.analysis.as_ref().unwrap().items[0].clone();
        let base = review.card_contexts(&review.analysis.as_ref().unwrap().items)[0].unwrap();
        let base = CardContext {
            record: Record {
                decision: Decision::Review,
                reminder: Reminder::None,
                ..base.record
            },
            ..base
        };
        for (decision, text) in [
            (Decision::Done, "Handled"),
            (Decision::Dismissed, "Dismissed – not mine"),
            (Decision::Moot, "No longer relevant"),
        ] {
            let card = CardContext {
                record: Record {
                    decision,
                    ..base.record
                },
                terminal: true,
                closed: true,
                deadline: base.deadline,
            };
            assert_eq!(status_pill(&item, &card).text, text);
        }
        item.resolution = item.deadline.clone();
        let resolved = CardContext {
            closed: true,
            ..base
        };
        assert_eq!(
            status_pill(&item, &resolved),
            PillView {
                text: "Resolved — later reply found".into(),
                kind: "success"
            }
        );
    }

    #[test]
    fn deadline_pills_omit_missing_and_do_not_mark_closed_cards_danger() {
        let review = crate::review_model::layout_fixture();
        let item = &review.analysis.as_ref().unwrap().items[0];
        let base = review.card_contexts(&review.analysis.as_ref().unwrap().items)[0].unwrap();
        let base = CardContext {
            record: Record {
                decision: Decision::Review,
                reminder: Reminder::None,
                ..base.record
            },
            ..base
        };
        let no_deadline = CardContext {
            deadline: None,
            ..base
        };
        assert_eq!(pills_for(item, &no_deadline).len(), 1);
        let past = DeadlineView::PastDue {
            boundary: 0,
            offset_seconds: 0,
        };
        let open = CardContext {
            deadline: Some(past),
            ..base
        };
        assert_eq!(pills_for(item, &open)[1].kind, "danger");
        let closed = CardContext {
            closed: true,
            deadline: Some(past),
            ..base
        };
        assert_ne!(pills_for(item, &closed)[1].kind, "danger");
    }

    #[test]
    fn visible_handles_can_include_all_four_groups_when_handled_are_shown() {
        let mut review = crate::review_model::layout_fixture();
        let original = review.analysis.as_ref().unwrap().items[0].clone();
        review.analysis.as_mut().unwrap().items = vec![original.clone(); 4];
        let base = review.card_contexts(&review.analysis.as_ref().unwrap().items)[0].unwrap();
        let cards = vec![
            Some(CardContext {
                deadline: Some(DeadlineView::PastDue {
                    boundary: 0,
                    offset_seconds: 0,
                }),
                ..base
            }),
            Some(CardContext {
                deadline: Some(DeadlineView::Due {
                    boundary: i64::MAX,
                    offset_seconds: 0,
                }),
                ..base
            }),
            Some(CardContext {
                deadline: None,
                ..base
            }),
            Some(CardContext {
                record: Record {
                    decision: Decision::Done,
                    ..base.record
                },
                terminal: true,
                closed: true,
                deadline: None,
            }),
        ];
        let handles = visible_handles(&review, &cards, Filter::All, true);
        assert_eq!(handles, vec![0, 1, 2, 3]);
        let groups: Vec<_> = handles
            .into_iter()
            .map(|index| list_group(cards[index].as_ref().unwrap()))
            .collect();
        assert_eq!(
            groups,
            vec![
                ListGroup::PastDue,
                ListGroup::Due,
                ListGroup::NoFixedDeadline,
                ListGroup::Closed
            ]
        );
    }

    #[test]
    fn pure_callback_helpers_map_and_clamp() {
        assert_eq!(decision_from_code(DECISION_REVIEW), Decision::Review);
        assert_eq!(decision_from_code(DECISION_MINE), Decision::Mine);
        assert_eq!(decision_from_code(DECISION_DONE), Decision::Done);
        assert_eq!(decision_from_code(DECISION_DISMISSED), Decision::Dismissed);
        assert_eq!(decision_from_code(DECISION_WATCHING), Decision::Watching);
        assert_eq!(decision_from_code(DECISION_MOOT), Decision::Moot);
        assert_eq!(decision_from_code(99), Decision::Review);
        assert_eq!(clamped_selection_index(0, 3, -1), 0);
        assert_eq!(clamped_selection_index(1, 3, -1), 0);
        assert_eq!(clamped_selection_index(1, 3, 1), 2);
        assert_eq!(clamped_selection_index(2, 3, 1), 2);
        assert_eq!(
            reminder_title(Decision::Watching, "Reply"),
            "Follow up: Reply"
        );
        assert_eq!(reminder_title(Decision::Mine, "Reply"), "Reply");
    }

    #[test]
    fn no_usable_copy_distinguishes_rejected_results() {
        assert!(no_usable_text(0).starts_with("No actionable expectations"));
        assert!(no_usable_text(1).starts_with("No usable expectations"));
    }

    #[test]
    fn scan_strip_formats_progress_and_finished_summary() {
        let mut app = model();
        app.review = crate::review_model::layout_fixture();
        let cards = app
            .review
            .analysis
            .as_ref()
            .map(|analysis| app.review.card_contexts(&analysis.items))
            .unwrap();
        let scanning = ScanStrip::Scanning {
            phase: "Finding open loops",
            conversation_index: 3,
            conversation_total: 9,
            processed: 11,
            total: 42,
            elapsed_secs: 7,
            percent: 26,
            stopping: false,
        };
        let (view, chip) = scan_strip_view(&scanning, &app, &cards);
        assert_eq!(view.conversation.as_str(), "Conversation 3 of 9");
        assert_eq!(view.messages.as_str(), "11 / 42 messages");
        assert_eq!(view.elapsed.as_str(), "7s on this request");
        assert_eq!(chip, "Scanning · Conversation 3 of 9 · 11 / 42 messages");

        let (finished, chip) = scan_strip_view(&scan_strip(None, &app.review), &app, &cards);
        assert_eq!(finished.title.as_str(), "Scan finished");
        assert!(finished.summary.contains("3 expectations"));
        assert!(chip.is_empty());

        let (idle, chip) = scan_strip_view(&ScanStrip::Idle, &model(), &[]);
        assert_eq!(idle.state.as_str(), "idle");
        assert!(chip.is_empty());

        app.review.source_failures = 1;
        let (finished, _) = scan_strip_view(&scan_strip(None, &app.review), &app, &cards);
        assert_eq!(
            finished.coverage_label.as_str(),
            "Coverage: 1 source incomplete"
        );
        assert!(finished.summary.contains("Reviewed 4 of 4 loaded messages"));
    }

    #[test]
    fn pill_mapping_follows_decision_and_reminder_state() {
        let mut review = crate::review_model::layout_fixture();
        let item = review.analysis.as_ref().unwrap().items[0].clone();
        let source = source_message(&review, &item).unwrap();
        let key = review
            .decisions
            .fingerprint(&source.account, &source.id, &item.action_phrase);
        review.apply_decision_change(key, Decision::Mine, Reminder::Created);
        let cards = review.card_contexts(review.analysis.as_ref().map_or(&[], |a| &a.items));
        let pills = pills_for(&item, cards[0].as_ref().unwrap());
        assert_eq!(pills[0].text, "Tracking");
        assert_eq!(pills[0].kind, "success");
        assert!(pills.iter().any(|pill| pill.text == "To Do reminder set"));

        review.apply_decision_change(key, Decision::Watching, Reminder::Attempted);
        let cards = review.card_contexts(review.analysis.as_ref().map_or(&[], |a| &a.items));
        let pills = pills_for(&item, cards[0].as_ref().unwrap());
        assert_eq!(pills[0].text, "Watching team follow-up");
        assert!(pills.iter().any(|pill| pill.text == "Reminder unconfirmed"));
    }

    #[test]
    fn draft_prefill_quick_picks_and_scheduled_line_are_pure() {
        assert_eq!(reminder_title(Decision::Mine, "Send notes"), "Send notes");
        assert_eq!(
            reminder_title(Decision::Watching, "Send notes"),
            "Follow up: Send notes"
        );
        let now = chrono::Local::now();
        assert_eq!(
            reminder_quick_pick_in_hour(now),
            (now + chrono::Duration::hours(1))
                .format("%Y-%m-%d %H:%M")
                .to_string()
        );
        assert_eq!(
            reminder_quick_pick_tomorrow(now),
            (now + chrono::Duration::days(1))
                .format("%Y-%m-%d 09:00")
                .to_string()
        );
        let valid_value = reminder_quick_pick_in_hour(now);
        let (line, valid) = scheduled_instant_line(&valid_value);
        assert!(valid);
        assert!(line.starts_with("Scheduled instant: "));
        assert_eq!(
            scheduled_instant_line("not a date"),
            (REMINDER_VALIDATION_HINT.into(), false)
        );
        let draft = ReminderDraft {
            key: [1; 32],
            account: "synthetic".into(),
            title: "Send notes".into(),
            when: valid_value,
            error: String::new(),
            prior_decision: Decision::Mine,
        };
        assert!(draft_view(Some(&draft)).unwrap().valid);
    }

    #[test]
    fn reminder_state_view_covers_every_variant() {
        let mut record = Record {
            key: [0xab; 32],
            decision: Decision::Mine,
            reminder: Reminder::None,
            updated: 0,
        };
        assert_eq!(reminder_state_view(&record).state, "none");
        record.reminder = Reminder::Created;
        let created = reminder_state_view(&record);
        assert_eq!(created.state, "created");
        assert!(
            created
                .text
                .starts_with("Reminder created in Microsoft To Do.")
        );
        record.reminder = Reminder::Attempted;
        let attempted = reminder_state_view(&record);
        assert_eq!(attempted.state, "attempted");
        assert!(attempted.marker.starts_with("Reference: "));
    }

    #[test]
    fn evidence_cards_cover_request_deadline_event_and_subject_time() {
        let review = crate::review_model::layout_fixture();
        let mut item = review.analysis.as_ref().unwrap().items[0].clone();
        item.event = Some(Anchor {
            message: "m0".into(),
            block: 0,
            quote: "the planning meeting".into(),
            context: "Before the planning meeting".into(),
        });
        item.event_passed = Some(EventPassed {
            name: "planning meeting".into(),
            end: 1,
            message_handle: "m0".into(),
            from_subject: true,
        });
        let cards = evidence_cards(&item, &review.messages);
        assert_eq!(
            cards
                .iter()
                .map(|card| card.label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Original expectation",
                "Deadline evidence",
                "Event evidence",
                "Event time evidence"
            ]
        );
        assert_eq!(cards[2].context, "Before the planning meeting");
        assert_eq!(
            cards[3].subject_note,
            "From the subject line of this message"
        );
        assert!(
            cards
                .iter()
                .all(|card| card.url.starts_with("https://outlook.office.com/"))
        );
    }

    #[test]
    fn completion_card_covers_same_cross_unvalidated_and_none() {
        let review = crate::review_model::layout_fixture();
        let mut item = review.analysis.as_ref().unwrap().items[0].clone();
        item.cross_thread = false;
        let same = completion_card(&item, &review.messages);
        assert_eq!(same.state, "evidence");
        assert!(!same.cross_thread);
        item.cross_thread = true;
        assert!(completion_card(&item, &review.messages).cross_thread);
        item.resolution = None;
        item.unverified_resolution = true;
        let unvalidated = completion_card(&item, &review.messages);
        assert_eq!(unvalidated.state, "unvalidated");
        assert!(unvalidated.label.contains("could not be validated"));
        item.unverified_resolution = false;
        let none = completion_card(&item, &review.messages);
        assert_eq!(none.state, "none");
        assert!(none.label.starts_with("No matching completion"));
    }

    #[test]
    fn conversation_rows_mark_sent_mail_and_keep_quote_nested() {
        let review = crate::review_model::layout_fixture();
        let source = &review.messages[0];
        let rows = conversation_rows(&review.messages, source);
        assert_eq!(rows.len(), 2);
        assert!(!rows[0].sent);
        assert!(rows[1].sent);
        assert!(!rows[1].quoted_history.is_empty());
        assert!(!rows[1].body.contains("Original Message"));
    }

    #[test]
    fn outlook_link_projection_rejects_every_other_url() {
        assert_eq!(
            gated_outlook_url("https://outlook.office.com/mail/item"),
            "https://outlook.office.com/mail/item"
        );
        assert_eq!(
            gated_outlook_url("https://outlook.office365.com/mail/item"),
            "https://outlook.office365.com/mail/item"
        );
        assert!(gated_outlook_url("https://example.invalid/outlook.office.com/").is_empty());
        assert!(gated_outlook_url("http://outlook.office.com/mail/item").is_empty());
    }

    #[test]
    fn initials_split_on_whitespace_parens_and_angle_brackets_and_cap_at_two() {
        assert_eq!(initials("Jane Doe"), "JD");
        assert_eq!(initials("jane doe"), "JD");
        assert_eq!(initials("Alice Bob Carol"), "AB");
        assert_eq!(initials("Madonna"), "M");
        assert_eq!(initials("Team (Alice) <alice@example.invalid>"), "TA");
        assert_eq!(initials("  "), "");
    }

    #[test]
    fn draft_view_overrides_scheduled_line_with_a_pending_error_and_flags_a_short_title() {
        let valid_when = reminder_quick_pick_in_hour(chrono::Local::now());
        let with_error = ReminderDraft {
            key: [1; 32],
            account: "synthetic".into(),
            title: "Send notes".into(),
            when: valid_when.clone(),
            error: "The saved-decision limit (50) is reached. Existing decisions are preserved."
                .into(),
            prior_decision: Decision::Mine,
        };
        let view = draft_view(Some(&with_error)).unwrap();
        assert_eq!(
            view.scheduled_line,
            "The saved-decision limit (50) is reached. Existing decisions are preserved."
        );
        // A pending error still reflects the title/time validity underneath
        // it -- the override only replaces the displayed line, not `valid`.
        assert!(view.valid);

        let short_title = ReminderDraft {
            key: [1; 32],
            account: "synthetic".into(),
            title: "ab".into(),
            when: valid_when.clone(),
            error: String::new(),
            prior_decision: Decision::Mine,
        };
        assert!(!draft_view(Some(&short_title)).unwrap().valid);

        let long_title = ReminderDraft {
            key: [1; 32],
            account: "synthetic".into(),
            title: "x".repeat(321),
            when: valid_when,
            error: String::new(),
            prior_decision: Decision::Mine,
        };
        assert!(!draft_view(Some(&long_title)).unwrap().valid);

        assert!(draft_view(None).is_none());
    }

    #[test]
    fn reconcile_keeps_the_error_status_when_the_decision_update_is_rejected() {
        let mut review = crate::review_model::layout_fixture();
        let key = review
            .decisions
            .fingerprint("synthetic", "synthetic-0", "send the draft budget");

        apply_reconcile(&mut review, key, true);
        assert!(review.action_status_succeeded);
        assert_eq!(
            review.action_status,
            "Recorded: the To Do task exists. Manage it in Microsoft To Do."
        );
        assert_eq!(review.decisions.get(&key).reminder, Reminder::Created);
        let before_rejected_call = review.decisions.get(&key);

        review.decisions.error = Some(
            "The saved-decision limit (50) is reached. Existing decisions are preserved.".into(),
        );
        apply_reconcile(&mut review, key, false);
        assert!(!review.action_status_succeeded);
        assert_eq!(
            review.action_status,
            "The saved-decision limit (50) is reached. Existing decisions are preserved."
        );
        // The rejected update (Created -> None) must not have taken effect.
        assert_eq!(
            review.decisions.get(&key).reminder,
            before_rejected_call.reminder
        );
        assert_eq!(review.decisions.get(&key).reminder, Reminder::Created);
    }
}
