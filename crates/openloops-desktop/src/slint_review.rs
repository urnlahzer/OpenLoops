//! Review-screen projection and callback wiring for the Slint adapter.
use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering};

use crate::{
    app_model::{AppModel, Outcome, Service, Status},
    deadline_view::label as deadline_label,
    loop_state::{Decision, Reminder},
    review_model::{
        CardContext, Filter, ListGroup, ReminderDraft, ReviewState, SHOW_HANDLED_LABEL, ScanStrip,
        card_hidden, card_order, decision_after_setting_reminder, default_reminder,
        expectations_summary, list_group, open_badge_count, reminder_button_enabled,
        status_base_label, status_label, status_shows_cross_thread,
    },
    slint_ui::{AppWindow, MetaCell, ReviewPill, ReviewRow, ScanStripModel, refresh, start_timer},
};
use openloops_graph::live::{ConnectionConfig, review::load_recent};
use openloops_inference::expectations::{Expectation, Owner};
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

#[allow(clippy::too_many_lines)]
pub(crate) fn register_callbacks(
    window: &AppWindow,
    model: &Rc<RefCell<AppModel>>,
    timer: &Rc<Timer>,
) {
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
                model_ref.review.draft_scrolled = false;
                if record.decision == Decision::Review {
                    model_ref.review.action_status = "Opening a reminder draft tracks this as yours. Cancel restores the previous decision.".into();
                    model_ref.review.action_status_succeeded = true;
                }
            }
            drop(model_ref);
            refresh(&model, &weak);
        });
    }
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
        assert_eq!(rows.len(), 2);
        assert!(rows[0].first_in_group);
        assert_ne!(rows[0].group, ListGroup::Closed);
        assert_eq!(rows[0].group, rows[1].group);
        assert!(!rows[1].first_in_group);
        assert_eq!(rows[0].handle, 0);
        assert_eq!(rows[1].handle, 1);
        assert!(rows[0].selected);
        assert!(rows[0].waiting_source.contains("Waiting:"));
    }

    #[test]
    fn review_filter_is_applied_in_rust() {
        let review = crate::review_model::layout_fixture();
        let cards = review.card_contexts(&review.analysis.as_ref().unwrap().items);
        assert_eq!(
            visible_handles(&review, &cards, Filter::All, false),
            vec![0, 1]
        );
        assert_eq!(
            visible_handles(&review, &cards, Filter::Mine, false),
            vec![0]
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
        let cards = review.card_contexts(&review.analysis.as_ref().unwrap().items);
        let key = cards[0].as_ref().unwrap().record.key;
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
        assert!(finished.summary.contains("2 expectations"));
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
        assert!(finished.summary.contains("Reviewed 2 of 2 loaded messages"));
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
}
