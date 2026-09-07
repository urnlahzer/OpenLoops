//! Conversation review and explicit decisions. Readable mail stays in memory.
use crate::deadline_view::{DeadlineView, classify, label};
use crate::loop_state::{Decision, Decisions, Record, Reminder, marker, now};
use chrono::TimeZone;
use eframe::egui::{self, Color32, RichText};
use openloops_graph::live::{reminders::ReminderRequest, review::SourceReview};
use openloops_inference::ollama::expectations::{
    Anchor, EventPassed, Expectation, Expectations, Owner, ResolutionKind,
};
#[path = "review_scan.rs"]
mod scanning;
pub use scanning::{ReviewMessage, ScanProgress, ScanResult, probe, scan};

const SHOW_HANDLED_LABEL: &str = "Show resolved, handled, dismissed, and no-longer-relevant items";

struct ReminderDraft {
    key: [u8; 32],
    account: String,
    title: String,
    when: String,
    error: String,
}

/// Per-card decision/urgency facts, independent of the source mail or the
/// expectation item -- entirely owned/`Copy`, so a `Vec<Option<CardContext>>`
/// never borrows `self` and stays usable in `show_analysis`'s render loop
/// alongside `&mut self.draft`/`self.decisions` for whichever card owns an
/// open reminder draft.
#[derive(Clone, Copy)]
struct CardContext {
    record: Record,
    terminal: bool,
    /// True when the card ranks and hides with the terminal group: either a
    /// terminal decision, or the expectation is resolved by later evidence
    /// and the saved decision has not explicitly overridden that closure
    /// (`Decision::Mine` or `Decision::Watching`).
    closed: bool,
    deadline: Option<DeadlineView>,
}

fn is_past_due(view: &DeadlineView) -> bool {
    matches!(
        view,
        DeadlineView::PastDue { .. }
            | DeadlineView::DueRange { past: true, .. }
            | DeadlineView::DueDate { past: true, .. }
            | DeadlineView::DueBusinessDay { past: true, .. }
    )
}

fn card_rank(card: Option<&CardContext>) -> u8 {
    match card {
        Some(card) if card.closed => 2,
        Some(card) if card.deadline.as_ref().is_some_and(is_past_due) => 0,
        _ => 1,
    }
}

fn card_order(cards: &[Option<CardContext>]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..cards.len()).collect();
    order.sort_by_key(|&i| (card_rank(cards[i].as_ref()), i));
    order
}

#[derive(Default)]
pub struct ReviewState {
    pub messages: Vec<ReviewMessage>,
    pub notices: Vec<String>,
    pub analysis: Option<Expectations>,
    pub analysis_model: String,
    pub scan_summary: String,
    pub scan_errors: Vec<String>,
    pub scan_incomplete: bool,
    pub source_failures: usize,
    pub decisions: Decisions,
    pub action_status: String,
    pub pending_reminder: Option<([u8; 32], ReminderRequest)>,
    draft: Option<ReminderDraft>,
    /// Whether the open `draft`'s card has already been scrolled into view
    /// this time it opened. Reset to `false` whenever a new draft opens or
    /// the open draft closes (see [`ReviewState::show_draft`]), so the
    /// scroll-into-view happens exactly once per draft.
    draft_scrolled: bool,
    show_handled: bool,
}

impl ReviewState {
    pub fn loaded(sources: Vec<SourceReview>) -> Self {
        let mut state = Self::default();
        for source in sources {
            let mut count = 0;
            for item in source.messages {
                if state
                    .messages
                    .iter()
                    .any(|m| m.account == item.account && m.id == item.id)
                {
                    continue;
                }
                if let Ok(m) = scanning::prepare(&item, &source.label, state.messages.len()) {
                    state.messages.push(m);
                    count += 1;
                } else {
                    state.source_failures += 1;
                    state.notices.push(format!(
                        "{}: a message exceeded preparation limits or lacked required context.",
                        source.label
                    ));
                }
            }
            state.notices.push(format!(
                "{}: {count} messages{}",
                source.label,
                if source.partial {
                    "; coverage capped, more mail exists"
                } else {
                    ""
                }
            ));
            if source.partial {
                state.source_failures += 1;
            }
            state.source_failures += source.errors.len();
            state.notices.extend(
                source
                    .errors
                    .into_iter()
                    .map(|e| format!("{}: {e}", source.label)),
            );
        }
        let merged = scanning::merge_threads(&mut state.messages);
        if merged > 0 {
            state.notices.push(format!(
                "{merged} conversation identities were merged by subject and participants."
            ));
        }
        state.messages.sort_by_key(|m| m.input.timestamp);
        state
    }
    pub fn set_scan(&mut self, result: ScanResult, model: String) {
        self.scan_summary = format!(
            "Reviewed {} of {} loaded messages in their conversations. {} conversations could not be analyzed{}.",
            result.analyzed,
            result.total,
            result.failures.len(),
            if result.cancelled {
                "; stopped by you"
            } else {
                ""
            }
        );
        self.scan_incomplete = result.analyzed < result.total
            || self.source_failures > 0
            || result.analysis.rejected > 0
            || result.analysis.degraded > 0
            || result.closure_pass_failure.is_some();
        self.scan_errors = result.failures;
        for reason in &result.analysis.rejection_reasons {
            if !self.scan_errors.iter().any(|existing| existing == reason) {
                self.scan_errors.push((*reason).into());
            }
        }
        self.scan_errors.extend(result.conversation_notes);
        if let Some(failure) = result.closure_pass_failure {
            self.scan_errors.push(failure);
        }
        self.analysis = Some(result.analysis);
        self.analysis_model = model;
    }
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = &self.decisions.error {
            ui.colored_label(Color32::DARK_RED, error);
        }
        let saved = self.decisions.records.len();
        if self.analysis.is_none() && saved > 0 {
            ui.label(format!("{saved} saved decisions or reminder records. Scan to restore descriptions from Microsoft. No mail text is saved locally."));
        }
        ui.label(&self.scan_summary);
        if self.scan_incomplete {
            ui.colored_label(Color32::DARK_RED,"Partial coverage. These results cannot establish that all outstanding work has been found.");
        }
        ui.collapsing("Scan coverage and errors", |ui| {
            for notice in self.notices.iter().chain(&self.scan_errors) {
                ui.label(notice);
            }
        });
        self.show_analysis(ui);
        if !self.action_status.is_empty() {
            ui.label(&self.action_status);
        }
        ui.collapsing(
            format!("Scanned messages ({})", self.messages.len()),
            |ui| {
                for (index, m) in self.messages.iter().enumerate() {
                    // Two messages with the same source, date, and subject
                    // otherwise share this collapsible's default widget ID
                    // (derived from its label text alone); `push_id` scopes
                    // every widget in this row -- the row's own collapsible
                    // and its nested "Quoted history" one -- by the
                    // message's position instead.
                    ui.push_id(index, |ui| {
                        ui.collapsing(
                            format!(
                                "{} · {} · {}",
                                m.source,
                                m.date_label,
                                m.input.message.subject.as_string()
                            ),
                            |ui| {
                                if let Some(sender) = &m.input.message.sender {
                                    ui.label(sender.as_string());
                                }
                                for b in &m.input.message.body_blocks {
                                    ui.label(b.as_string());
                                }
                                ui.collapsing("Quoted history", |ui| {
                                    for b in &m.input.message.quote_blocks {
                                        ui.label(b.as_string());
                                    }
                                });
                                open_link(ui, &m.web_link);
                            },
                        );
                    });
                }
            },
        );
    }
    fn card_context(&self, item: &Expectation, now: i64, now_offset: i32) -> Option<CardContext> {
        let source = self
            .messages
            .iter()
            .find(|m| m.input.handle == item.evidence.message)?;
        let key = self
            .decisions
            .fingerprint(&source.account, &source.id, &item.action_phrase);
        let record = self.decisions.get(&key);
        let terminal = matches!(
            record.decision,
            Decision::Done | Decision::Dismissed | Decision::Moot
        );
        let closed = terminal
            || ((item.resolution.is_some() || item.event_passed.is_some())
                && !matches!(record.decision, Decision::Mine | Decision::Watching));
        let deadline = item.deadline.as_ref().map(|anchor| {
            let message = self
                .messages
                .iter()
                .find(|m| m.input.handle == anchor.message)
                .unwrap_or(source);
            let offset = chrono::Local
                .timestamp_opt(message.input.timestamp, 0)
                .single()
                .map_or(now_offset, |t| t.offset().local_minus_utc());
            classify(&anchor.quote, message.input.timestamp, now, offset)
        });
        Some(CardContext {
            record,
            terminal,
            closed,
            deadline,
        })
    }

    fn card_contexts(&self, items: &[Expectation]) -> Vec<Option<CardContext>> {
        let clock = chrono::Local::now();
        items
            .iter()
            .map(|item| {
                self.card_context(item, clock.timestamp(), clock.offset().local_minus_utc())
            })
            .collect()
    }

    fn show_analysis(&mut self, ui: &mut egui::Ui) {
        let Some(analysis) = &self.analysis else {
            return;
        };
        ui.separator();
        ui.heading("What may need your attention");
        ui.label("Review the action and evidence. A missing reply in this scan does not prove the work is unfinished.");
        ui.checkbox(&mut self.show_handled, SHOW_HANDLED_LABEL);
        let cards = self.card_contexts(&analysis.items);
        ui.label(expectations_summary(analysis, &cards, &self.analysis_model));
        if analysis.items.is_empty() {
            ui.label(if analysis.rejected>0 {"No usable expectations were returned. Evidence validation rejected suggestions; this is not a clean bill of health."} else {"No actionable expectations were identified in the successfully reviewed conversations."});
        }
        let order = card_order(&cards);
        // `cards` is entirely owned data (see `CardContext`'s doc comment)
        // and does not borrow `self`, so it stays usable through the whole
        // loop below alongside `&mut self.draft`/`self.decisions` for
        // whichever card owns an open reminder draft.
        let mut change = None;
        for &i in &order {
            let Some(card) = cards[i] else { continue };
            let key = card.record.key;
            let show_draft_here = self.draft.as_ref().is_some_and(|d| d.key == key);
            if card.closed && !self.show_handled && !show_draft_here {
                continue;
            }
            let mut card_change = None;
            let mut card_draft = None;
            {
                // `item`/`source` are scoped to this block, which ends
                // before `self.show_draft` below needs `&mut self.draft` for
                // the same card: they borrow `self.analysis`/`self.messages`,
                // which must not still be borrowed at that point.
                let item = &self.analysis.as_ref().expect("checked above").items[i];
                let Some(source) = self
                    .messages
                    .iter()
                    .find(|m| m.input.handle == item.evidence.message)
                else {
                    continue;
                };
                ui.push_id(i, |ui| {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        let (c, d) = render_card_body(ui, &self.messages, item, source, &card);
                        card_change = c;
                        card_draft = d;
                    });
                });
            }
            if let Some((decision, reminder)) = card_change {
                change = Some((key, decision, reminder));
            }
            if card_draft.is_some() {
                self.draft = card_draft;
                self.draft_scrolled = false;
            }
            if show_draft_here {
                self.show_draft(ui, key);
            }
        }
        if let Some((key, decision, reminder)) = change {
            let mut r = self.decisions.get(&key);
            r.decision = decision;
            r.reminder = reminder;
            r.updated = now();
            self.action_status = match self.decisions.update(r) {
                Ok(()) => "Decision saved on this Windows account.".into(),
                Err(e) => e,
            };
        }
    }
    /// Renders the open reminder draft's panel when it belongs to `key`
    /// (the card currently being rendered); a no-op otherwise. Scrolls the
    /// panel into view once, the first time it renders after opening (see
    /// `draft_scrolled`).
    fn show_draft(&mut self, ui: &mut egui::Ui, key: [u8; 32]) {
        if self.draft.as_ref().is_none_or(|d| d.key != key) {
            return;
        }
        let mut cancel = false;
        let mut create = false;
        let draft = self.draft.as_mut().expect("checked above");
        egui::Frame::group(ui.style()).show(ui,|ui|{
            ui.heading("Review your Microsoft To Do reminder");
            ui.label("Creates one task in your personal default Tasks list. Only the title, reminder time, and an opaque OpenLoops reference are sent to Microsoft.");
            ui.label("Task title (editable)");ui.text_edit_singleline(&mut draft.title);
            ui.label("Remind me at — this computer's local time (YYYY-MM-DD HH:MM)");ui.text_edit_singleline(&mut draft.when);
            ui.horizontal(|ui|{
                if ui.button("In one hour").clicked(){draft.when=default_reminder();}
                if ui.button("Tomorrow at 9 am").clicked(){draft.when=format!("{} 09:00",(chrono::Local::now()+chrono::Duration::days(1)).format("%Y-%m-%d"));}
            });
            ui.label("This time is your choice, separate from the email deadline. Microsoft To Do controls alert delivery, including when OpenLoops is closed.");
            if let Ok(at)=reminder_time(&draft.when) && let Some(time)=chrono::DateTime::from_timestamp(at,0){ui.label(format!("Scheduled instant: {}",time.with_timezone(&chrono::Local).format("%a %b %d, %Y at %H:%M %:z")));}
            ui.horizontal(|ui|{create=ui.button("Create this reminder in Microsoft To Do").clicked();cancel=ui.button("Cancel").clicked();});
            if !draft.error.is_empty(){ui.colored_label(Color32::DARK_RED,&draft.error);}
        });
        if !self.draft_scrolled {
            ui.scroll_to_cursor(Some(egui::Align::Center));
            self.draft_scrolled = true;
        }
        if create {
            match reminder_time(&draft.when) {
                Ok(at) if draft.title.trim().len()>=3 && draft.title.len()<=320 => {
                    match self.decisions.begin_reminder(draft.key){Ok(())=>{self.pending_reminder=Some((draft.key,ReminderRequest {account:draft.account.clone(),title:draft.title.trim().into(),at_utc:at,marker:marker(&draft.key)}));cancel=true;},Err(e)=>draft.error=e}
                }
                _=>draft.error="Enter a future local date/time and a title of 3–320 bytes. Ambiguous daylight-saving times need a different time.".into(),
            }
        }
        if cancel {
            self.draft = None;
            self.draft_scrolled = false;
        }
    }
}

/// The card's status label before the cross-thread suffix (see
/// [`status_label`]) is appended. A terminal decision (`Done`/`Dismissed`/
/// `Moot`) always wins. Otherwise, an explicit `Mine`/`Watching` override
/// wins over resolution-by-evidence: the user has said this is still open
/// (or being watched), so the label must keep saying that even when
/// `item.resolution` is `Some` -- it must not flip back to resolved wording
/// just because closure evidence exists. Only when there is no override at
/// all does a resolution get to speak for itself.
fn status_base_label(decision: Decision, item: &Expectation) -> String {
    match decision {
        Decision::Done => "Handled".into(),
        Decision::Dismissed => "Dismissed / not mine".into(),
        Decision::Moot => "No longer relevant".into(),
        Decision::Mine => "Tracking".into(),
        Decision::Watching => "Watching team follow-up".into(),
        Decision::Review if item.resolution.is_some() => {
            resolution_status_label(item.resolution_kind).into()
        }
        Decision::Review if item.event_passed.is_some() => {
            event_passed_status_label(item.event_passed.as_ref().expect("checked above"))
        }
        Decision::Review => "Needs your review".into(),
    }
}

fn event_passed_status_label(event: &EventPassed) -> String {
    let date = chrono::DateTime::from_timestamp(event.end, 0).map_or_else(
        || "unknown date".to_string(),
        |time| {
            time.with_timezone(&chrono::Local)
                .format("%b %d, %Y %H:%M %:z")
                .to_string()
        },
    );
    format!("Closed: event passed ({}, ended {date})", event.name)
}
fn resolution_status_label(kind: Option<ResolutionKind>) -> &'static str {
    match kind {
        None => "Resolved",
        Some(ResolutionKind::Completed) => "Resolved: completed",
        Some(ResolutionKind::Declined) => "Resolved: declined",
        Some(ResolutionKind::Withdrawn) => "Resolved: withdrawn by the requester",
        Some(ResolutionKind::Superseded) => "Resolved: request replaced by the requester",
        Some(ResolutionKind::Agreed) => "Resolved: you agreed",
    }
}
/// Appends " (evidence in another conversation)" to `base` when
/// [`status_shows_cross_thread`] says the suffix applies.
fn status_label(base: &str, cross_thread: bool) -> String {
    if cross_thread {
        format!("{base} (evidence in another conversation)")
    } else {
        base.to_string()
    }
}

/// True when the " (evidence in another conversation)" suffix belongs on
/// this card's status: only when `item.cross_thread` is set AND the status
/// text itself came from the resolution branch of [`status_base_label`]
/// (`Decision::Review` with a resolution present) -- never when a user
/// decision (`Done`/`Dismissed`/`Moot`/`Mine`/`Watching`) is driving the
/// status text instead. A manually "Handled" item, for example, must not
/// read "Handled (evidence in another conversation)" just because some
/// earlier cross-thread resolution happens to sit on the same item.
fn status_shows_cross_thread(decision: Decision, resolved: bool, cross_thread: bool) -> bool {
    cross_thread && decision == Decision::Review && resolved
}

fn resolution_anchor_label(kind: Option<ResolutionKind>, cross_thread: bool) -> &'static str {
    if cross_thread {
        return "Later evidence in another conversation";
    }
    match kind {
        None => "Later resolution evidence",
        Some(ResolutionKind::Completed) => "Later completion evidence",
        Some(ResolutionKind::Declined) => "Later decline evidence",
        Some(ResolutionKind::Withdrawn) => "Later withdrawal by the requester",
        Some(ResolutionKind::Superseded) => "Later replacement by the requester",
        Some(ResolutionKind::Agreed) => "Your later agreement",
    }
}
/// Builds the "What may need your attention" summary line. `analysis.items`
/// includes resolved-by-evidence items, so the open count excludes them and
/// they get their own segment instead. `resolved` counts an item only when
/// BOTH `item.resolution.is_some()` AND the card is closed (using the same
/// `closed` predicate the card uses, see [`ReviewState::card_context`], by
/// way of the already-computed `cards`) -- requiring both keeps a manually
/// "Handled" item with no resolution evidence at all (terminal decisions
/// close the card too, but say nothing about resolution) from inflating
/// this count, and an item whose closure was explicitly overridden to
/// `Mine` or `Watching` still counts as open here, matching the card it
/// corresponds to. `cross_thread_closed` is the subset of `resolved` whose
/// resolution came from the cross-thread closure pass (`item.cross_thread`),
/// surfaced as its own segment alongside the "Scan coverage and errors"
/// panel note (`scanning::scan_closures`'s own conversation note).
fn expectations_summary(
    analysis: &Expectations,
    cards: &[Option<CardContext>],
    model: &str,
) -> String {
    let resolved_flags: Vec<bool> = analysis
        .items
        .iter()
        .zip(cards)
        .map(|(item, card)| item.resolution.is_some() && card.as_ref().is_some_and(|c| c.closed))
        .collect();
    let resolved = resolved_flags.iter().filter(|&&r| r).count();
    let cross_thread_closed = analysis
        .items
        .iter()
        .zip(&resolved_flags)
        .filter(|&(item, &r)| r && item.cross_thread)
        .count();
    let event_closed = analysis
        .items
        .iter()
        .zip(cards)
        .filter(|&(item, card)| {
            item.resolution.is_none()
                && item.event_passed.is_some()
                && card.as_ref().is_some_and(|card| card.closed)
        })
        .count();
    let open = analysis.items.len() - resolved - event_closed;
    let degraded_note = if analysis.degraded > 0 {
        format!(" · {} kept with unverified evidence", analysis.degraded)
    } else {
        String::new()
    };
    let cross_thread_note = if cross_thread_closed > 0 {
        format!(" · {cross_thread_closed} closed from evidence in other conversations")
    } else {
        String::new()
    };
    let event_note = if event_closed > 0 {
        format!(" · {event_closed} closed because the event passed")
    } else {
        String::new()
    };
    format!(
        "{open} expectations · {resolved} resolved by later evidence · {} rejected for invalid evidence{degraded_note}{cross_thread_note}{event_note} · {model}",
        analysis.rejected
    )
}
/// Renders one card's whole body -- status, action/owner/waiting/deadline
/// summary, evidence collapsing, action buttons, and reminder status --
/// inside the caller's `egui::Frame::group`. Returns the decision change and
/// any reminder draft the action buttons produced, so the caller (which owns
/// `self.decisions`/`self.draft`) can apply them; this function itself never
/// touches `self`, so it can run while the caller still holds an immutable
/// borrow of `self.analysis`/`self.messages` (see
/// [`ReviewState::show_analysis`]).
fn render_card_body(
    ui: &mut egui::Ui,
    messages: &[ReviewMessage],
    item: &Expectation,
    source: &ReviewMessage,
    card: &CardContext,
) -> (Option<(Decision, Reminder)>, Option<ReminderDraft>) {
    let record = &card.record;
    let (terminal, closed) = (card.terminal, card.closed);
    ui.set_width(ui.available_width());
    let status_base = status_base_label(record.decision, item);
    let status = status_label(
        &status_base,
        status_shows_cross_thread(
            record.decision,
            item.resolution.is_some(),
            item.cross_thread,
        ),
    );
    ui.label(RichText::new(status).color(Color32::from_rgb(29, 87, 67)));
    ui.label(RichText::new(&item.action).size(21.0).strong());
    let owner = if record.decision == Decision::Mine {
        "You (confirmed)"
    } else {
        match item.owner {
            Owner::You => "You (suggested)",
            Owner::Team => "Team — no individual owner established",
            Owner::Unclear => "Unclear — confirm responsibility",
        }
    };
    ui.label(format!("Responsible: {owner}"));
    ui.label(format!("Waiting: {}", item.waiting_party));
    if let Some(deadline) = &item.deadline {
        ui.label(format!("Deadline stated in email: {}", deadline.quote));
    } else if item.unverified_deadline {
        ui.label("A deadline was stated, but its quotation could not be verified.");
    } else {
        ui.label("Deadline stated in email: Not specified");
    }
    if let Some(view) = &card.deadline {
        if !closed && is_past_due(view) {
            ui.colored_label(Color32::DARK_RED, label(view));
        } else {
            ui.label(label(view));
        }
    }
    if !item.uncertainty.is_empty() {
        ui.label(format!("Uncertainty: {}", item.uncertainty));
    }
    ui.label(
        RichText::new(format!(
            "{} · {} · {}",
            source.source, source.date_label, item.kind
        ))
        .small(),
    );
    ui.collapsing("Why this was suggested · evidence and replies", |ui| {
        render_evidence_section(ui, messages, item, source);
    });
    let (mut change, draft) = card_action_buttons(ui, item, source, record, terminal, closed);
    if let Some(reminder_change) = render_reminder_status(ui, record) {
        change = Some(reminder_change);
    }
    (change, draft)
}
/// The "Why this was suggested · evidence and replies" collapsing section's
/// body: the original evidence, the deadline/event evidence (if any), the
/// resolution evidence or its absence, and the full scanned conversation.
fn render_evidence_section(
    ui: &mut egui::Ui,
    messages: &[ReviewMessage],
    item: &Expectation,
    source: &ReviewMessage,
) {
    show_anchor(ui, "Original expectation", &item.evidence, messages);
    if let Some(deadline) = &item.deadline {
        show_anchor(ui, "Deadline evidence", deadline, messages);
    }
    if let Some(event) = &item.event {
        show_anchor(ui, "Event evidence", event, messages);
    }
    if let Some(resolution) = &item.resolution {
        show_anchor(
            ui,
            resolution_anchor_label(item.resolution_kind, item.cross_thread),
            resolution,
            messages,
        );
    } else if item.unverified_resolution {
        ui.label(
            "The analysis proposed a completion but it could not be validated; treat as open.",
        );
    } else {
        ui.label("No matching completion was identified in the scanned conversation. Work may have happened elsewhere or outside this history window.");
    }
    ui.collapsing("Full scanned conversation", |ui| {
        for m in messages
            .iter()
            .filter(|m| m.account == source.account && m.conversation == source.conversation)
        {
            ui.label(format!(
                "{} · {}",
                m.date_label,
                if m.input.from_user {
                    "You"
                } else {
                    "Other participant"
                }
            ));
            for b in &m.input.message.body_blocks {
                ui.label(b.as_string());
            }
        }
    });
}
/// Renders the reminder-status line(s) beneath the action buttons -- none
/// for `Reminder::None`, a link to the created Microsoft To Do task, or the
/// unresolved-attempt warning with its two reconciliation buttons -- and
/// returns the decision change a reconciliation button requested, if any.
fn render_reminder_status(ui: &mut egui::Ui, record: &Record) -> Option<(Decision, Reminder)> {
    match record.reminder {
        Reminder::None => None,
        Reminder::Created => {
            ui.label("Reminder created in Microsoft To Do. Manage its alerts and completion there; marking this loop handled does not modify the task.");
            ui.hyperlink_to("Open Microsoft To Do", "https://to-do.office.com/tasks/");
            None
        }
        Reminder::Attempted => {
            ui.colored_label(Color32::DARK_RED, "A reminder attempt has no confirmed outcome. Inspect Microsoft To Do before allowing another attempt.");
            ui.hyperlink_to("Inspect Microsoft To Do", "https://to-do.office.com/tasks/");
            ui.label(format!("Reference: {}", marker(&record.key)));
            if ui.button("I checked: the task exists").clicked() {
                return Some((record.decision, Reminder::Created));
            }
            if ui.button("I checked: no task was created").clicked() {
                return Some((record.decision, Reminder::None));
            }
            None
        }
    }
}
/// Renders the card's action buttons and returns the requested decision
/// change (with its unchanged reminder state) and any reminder draft opened.
/// A terminal card only offers reopening; a card closed by resolution
/// evidence (but not explicitly overridden to `Mine`/`Watching`) only offers
/// reopening it as still-open tracking; every other card gets the full set
/// of open-card actions.
fn card_action_buttons(
    ui: &mut egui::Ui,
    item: &Expectation,
    source: &ReviewMessage,
    record: &Record,
    terminal: bool,
    closed: bool,
) -> (Option<(Decision, Reminder)>, Option<ReminderDraft>) {
    let mut change = None;
    let mut draft = None;
    ui.horizontal_wrapped(|ui| {
        if terminal {
            if ui.button("Reopen for review").clicked() {
                change = Some((Decision::Review, record.reminder));
            }
        } else if closed {
            if ui.button("Still open — track it").clicked() {
                change = Some((Decision::Mine, record.reminder));
            }
        } else {
            if record.decision != Decision::Mine && ui.button("Track — this is mine").clicked() {
                change = Some((Decision::Mine, record.reminder));
            }
            if item.owner != Owner::You
                && record.decision != Decision::Watching
                && ui.button("Keep an eye on this").clicked()
            {
                change = Some((Decision::Watching, record.reminder));
            }
            if ui.button("Handled").clicked() {
                change = Some((Decision::Done, record.reminder));
            }
            if ui.button("Not mine / dismiss").clicked() {
                change = Some((Decision::Dismissed, record.reminder));
            }
            if ui.button("No longer relevant").clicked() {
                change = Some((Decision::Moot, record.reminder));
            }
            if reminder_button_enabled(closed, record.reminder)
                && ui.button("Set To Do reminder…").clicked()
            {
                change = Some((
                    decision_after_setting_reminder(record.decision),
                    record.reminder,
                ));
                draft = Some(ReminderDraft {
                    key: record.key,
                    account: source.account.clone(),
                    title: if record.decision == Decision::Watching {
                        format!("Follow up: {}", item.action)
                    } else {
                        item.action.clone()
                    },
                    when: default_reminder(),
                    error: String::new(),
                });
            }
        }
    });
    (change, draft)
}
/// Whether "Set To Do reminder…" should be enabled: any open (non-closed)
/// card that does not already have a reminder attempted or created for it.
/// Setting a reminder no longer requires first tracking or watching the
/// card -- opening the draft implies tracking on its own (see
/// [`decision_after_setting_reminder`]).
fn reminder_button_enabled(closed: bool, reminder: Reminder) -> bool {
    !closed && reminder == Reminder::None
}
/// Decision implied by opening a reminder draft on a card whose current
/// decision is `current`. Setting a reminder implies personal tracking, so
/// `Decision::Review` becomes `Decision::Mine`; `Decision::Watching` is left
/// as is (an explicit team follow-up, not personal ownership); `Decision::
/// Mine` is unaffected (it is already `Mine`). The reminder button only ever
/// renders for a card whose decision is one of these three -- `closed`
/// (which rules out a terminal decision or an un-overridden resolution)
/// guards `card_action_buttons`'s open-card branch -- so no other input is
/// meaningful here.
fn decision_after_setting_reminder(current: Decision) -> Decision {
    match current {
        Decision::Watching => Decision::Watching,
        _ => Decision::Mine,
    }
}
fn open_link(ui: &mut egui::Ui, url: &str) {
    if url.starts_with("https://outlook.office.com/")
        || url.starts_with("https://outlook.office365.com/")
    {
        ui.hyperlink_to("Open message in Outlook", url);
    }
}
fn show_anchor(ui: &mut egui::Ui, label: &str, anchor: &Anchor, messages: &[ReviewMessage]) {
    ui.label(RichText::new(label).strong());
    ui.label(&anchor.quote);
    if let Some(m) = messages.iter().find(|m| m.input.handle == anchor.message) {
        ui.label(
            RichText::new(format!(
                "{} · {} · {}",
                m.date_label,
                m.source,
                m.input.message.subject.as_string()
            ))
            .small(),
        );
        open_link(ui, &m.web_link);
    }
    if anchor.context != anchor.quote {
        egui::CollapsingHeader::new("Surrounding source text")
            .id_salt(label)
            .show(ui, |ui| {
                ui.label(&anchor.context);
            });
    }
}
fn default_reminder() -> String {
    (chrono::Local::now() + chrono::Duration::hours(1))
        .format("%Y-%m-%d %H:%M")
        .to_string()
}
fn reminder_time(value: &str) -> Result<i64, ()> {
    use chrono::TimeZone;
    let naive = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M").map_err(|_| ())?;
    let time = chrono::Local
        .from_local_datetime(&naive)
        .single()
        .ok_or(())?
        .timestamp();
    if time <= now() { Err(()) } else { Ok(time) }
}

#[cfg(feature = "ui-screenshot")]
pub fn layout_fixture() -> ReviewState {
    use openloops_graph::live::review::MailItem;
    use openloops_inference::ollama::expectations::Expectation;
    let mut state = ReviewState::default();
    let body = "Please send the draft budget by Friday.";
    for index in 0..2 {
        let item = MailItem {
            subject: "Quarterly planning".into(),
            body: body.into(),
            sender: "Alex <alex@example.invalid>".into(),
            sender_address: "alex@example.invalid".into(),
            received: "2026-09-06T12:00:00Z".into(),
            id: format!("synthetic-{index}"),
            account: "synthetic".into(),
            conversation: format!("synthetic-{index}"),
            ..MailItem::default()
        };
        state.messages.push(
            scanning::prepare(
                &item,
                if index == 0 {
                    "Personal mailbox / Inbox"
                } else {
                    "Group: planning@example.invalid"
                },
                index,
            )
            .expect("synthetic fixture"),
        );
    }
    let items = (0..2)
        .map(|index| Expectation {
            action: if index == 0 {
                "Send the draft budget to Alex".into()
            } else {
                "Confirm who will send the team budget".into()
            },
            action_phrase: "send the draft budget".into(),
            owner: if index == 0 { Owner::You } else { Owner::Team },
            waiting_party: "Alex <alex@example.invalid>".into(),
            kind: "request".into(),
            evidence: Anchor {
                message: format!("m{index}"),
                block: 0,
                quote: body.into(),
                context: body.into(),
            },
            deadline: Some(Anchor {
                message: format!("m{index}"),
                block: 0,
                quote: "Friday".into(),
                context: body.into(),
            }),
            event: None,
            resolution: None,
            resolution_kind: None,
            uncertainty: if index == 0 {
                String::new()
            } else {
                "The request was sent to the Group; no individual owner is named.".into()
            },
            unverified_deadline: false,
            unverified_resolution: false,
            cross_thread: false,
            event_passed: None,
        })
        .collect();
    state.set_scan(
        ScanResult {
            analysis: Expectations {
                items,
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            analyzed: 2,
            total: 2,
            cancelled: false,
            conversation_notes: vec![],
            cross_thread_closures: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            closure_pass_failure: None,
        },
        "Synthetic layout check".into(),
    );
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    fn aging_fixture() -> (ReviewState, Expectation) {
        let mut state = ReviewState::default();
        for (index, received) in ["2026-09-02T12:00:00Z", "2026-09-09T12:00:00Z"]
            .into_iter()
            .enumerate()
        {
            let mail = openloops_graph::live::review::MailItem {
                id: format!("synthetic-{index}"),
                account: "synthetic".into(),
                received: received.into(),
                body: "Send the draft by Friday.".into(),
                ..Default::default()
            };
            state
                .messages
                .push(scanning::prepare(&mail, "Synthetic", index).unwrap());
        }
        let anchor = Anchor {
            message: "m0".into(),
            block: 0,
            quote: "Friday".into(),
            context: String::new(),
        };
        let item = Expectation {
            action: "Send the draft".into(),
            action_phrase: "send the draft".into(),
            owner: Owner::You,
            waiting_party: String::new(),
            kind: "request".into(),
            evidence: anchor.clone(),
            deadline: Some(anchor),
            event: None,
            resolution: None,
            resolution_kind: None,
            uncertainty: String::new(),
            unverified_deadline: false,
            unverified_resolution: false,
            cross_thread: false,
            event_passed: None,
        };
        (state, item)
    }

    #[test]
    fn card_offset_comes_from_the_deadline_message_timestamp() {
        use chrono::TimeZone;
        let (mut state, mut item) = aging_fixture();
        state.messages[0].input.timestamp = 1_767_268_800;
        for handle in ["m1", "missing"] {
            let anchor = item.deadline.as_mut().unwrap();
            anchor.message = handle.into();
            anchor.quote = "2026-09-01 15:00".into();
            let message = &state.messages[usize::from(handle == "m1")];
            let offset = chrono::Local
                .timestamp_opt(message.input.timestamp, 0)
                .single()
                .unwrap()
                .offset()
                .local_minus_utc();
            assert_eq!(
                state
                    .card_context(&item, 1_788_609_600, 1234)
                    .unwrap()
                    .deadline,
                Some(classify(
                    "2026-09-01 15:00",
                    message.input.timestamp,
                    1_788_609_600,
                    offset
                ))
            );
        }
    }

    #[test]
    fn date_and_business_day_urgency_tracks_past_flag() {
        for past in [false, true] {
            assert_eq!(is_past_due(&DeadlineView::DueDate { day: 0, past }), past);
            assert_eq!(
                is_past_due(&DeadlineView::DueBusinessDay { day: 0, past }),
                past
            );
        }
    }

    #[test]
    fn deadline_uses_its_own_message_and_falls_back_to_evidence() {
        let (state, mut item) = aging_fixture();
        // Mon 2026-09-07T12:00:00Z — >=55h past the Friday EOD boundary
        // anchored to message0 for any offset in [-12h,+14h] (see the
        // margin comment in urgency_order_is_stable_and_moot_is_terminal_until_reopened),
        // and still well before message1's own deadline so the `m1` case
        // below stays "not past due" with wide margin too.
        let now = 1_788_350_400 + 5 * 86_400;
        assert!(is_past_due(
            state
                .card_context(&item, now, 0)
                .unwrap()
                .deadline
                .as_ref()
                .unwrap()
        ));
        item.deadline.as_mut().unwrap().message = "m1".into();
        assert!(!is_past_due(
            state
                .card_context(&item, now, 0)
                .unwrap()
                .deadline
                .as_ref()
                .unwrap()
        ));
        item.deadline.as_mut().unwrap().message = "missing".into();
        assert!(is_past_due(
            state
                .card_context(&item, now, 0)
                .unwrap()
                .deadline
                .as_ref()
                .unwrap()
        ));
        item.evidence.message = "missing".into();
        assert!(state.card_context(&item, now, 0).is_none());
    }

    #[test]
    fn urgency_order_is_stable_and_moot_is_terminal_until_reopened() {
        // `card_context` derives each card's UTC offset from the real OS
        // timezone (see 9ee9b69), so every `now` below that tests a
        // past/due boundary must classify the same way for ANY real-world
        // offset, not just this machine's — CI may run in a different
        // zone. `aging_fixture`'s message0 is pinned at midweek noon UTC
        // (`MESSAGE0`) specifically because noon UTC ± the full plausible
        // offset range keeps the domain parser's local-day resolution
        // within one calendar day either way, so "Friday"/"this week"
        // always resolve to the same civil days regardless of offset.
        // Each `now` constant below keeps at least 24h of margin from the
        // relevant boundary across the full `[-12h, +14h]` UTC offset
        // range (hand-verified): CLEARLY_DUE is >=39h before the Friday
        // EOD boundary, CLEARLY_PAST_DUE is >=55h after it, and
        // CLEARLY_PAST_WEEK is >=31h after the end-of-week boundary.
        // Exact boundary-crossing behavior (the instant a deadline flips)
        // is covered separately in deadline_view.rs's tests, which inject
        // a fixed offset explicitly instead of depending on the OS zone.
        const MESSAGE0: i64 = 1_788_350_400; // Wed 2026-09-02T12:00:00Z
        const CLEARLY_DUE: i64 = MESSAGE0;
        const CLEARLY_PAST_DUE: i64 = MESSAGE0 + 5 * 86_400; // Mon 2026-09-07T12:00:00Z
        const CLEARLY_PAST_WEEK: i64 = MESSAGE0 + 6 * 86_400; // Tue 2026-09-08T12:00:00Z

        let (mut state, item) = aging_fixture();
        let source = &state.messages[0];
        let key = state
            .decisions
            .fingerprint(&source.account, &source.id, &item.action_phrase);
        for decision in [Decision::Done, Decision::Dismissed, Decision::Moot] {
            let mut record = state.decisions.get(&key);
            record.decision = decision;
            state.decisions.records = vec![record];
            let card = state.card_context(&item, CLEARLY_PAST_DUE, 0).unwrap();
            assert!(card.terminal);
            assert_eq!(card_rank(Some(&card)), 2);
        }
        state.decisions.records[0].decision = Decision::Review;
        let overdue = state.card_context(&item, CLEARLY_PAST_DUE, 0);
        assert!(!overdue.as_ref().unwrap().terminal);
        let due = state.card_context(&item, CLEARLY_DUE, 0);
        let mut range_item = item.clone();
        range_item.deadline.as_mut().unwrap().quote = "this week".into();
        let range = state.card_context(&range_item, CLEARLY_PAST_WEEK, 0);
        assert_eq!(card_rank(range.as_ref()), 0);
        let (mut terminal_state, terminal_item) = aging_fixture();
        let mut record = terminal_state
            .card_context(&terminal_item, CLEARLY_PAST_DUE, 0)
            .unwrap()
            .record;
        record.decision = Decision::Moot;
        terminal_state.decisions.records = vec![record];
        let terminal = terminal_state.card_context(&terminal_item, CLEARLY_PAST_DUE, 0);
        let cards = vec![
            terminal,
            due,
            overdue,
            None,
            range,
            terminal_state.card_context(&terminal_item, CLEARLY_DUE, 0),
        ];
        assert_eq!(card_order(&cards), vec![2, 4, 1, 3, 0, 5]);

        // A card resolved by later evidence, with no decision recorded (the
        // default is `Decision::Review`), ranks with the terminal group --
        // rank 2 -- even though `CLEARLY_DUE` is not yet past due: closure
        // by evidence overrides deadline-based ranking, same as a terminal
        // decision does.
        let mut resolved_item = item.clone();
        resolved_item.resolution = Some(resolved_item.evidence.clone());
        resolved_item.resolution_kind = Some(ResolutionKind::Completed);
        let (resolved_state, _) = aging_fixture();
        let resolved = resolved_state.card_context(&resolved_item, CLEARLY_DUE, 0);
        assert!(resolved.as_ref().unwrap().closed);
        assert!(!resolved.as_ref().unwrap().terminal);
        assert_eq!(card_rank(resolved.as_ref()), 2);

        // The same resolved item, once the saved decision explicitly
        // overrides closure with `Decision::Mine`, ranks and hides like any
        // other open card again.
        let (mut overridden_state, _) = aging_fixture();
        let override_key = overridden_state
            .card_context(&resolved_item, CLEARLY_DUE, 0)
            .unwrap()
            .record
            .key;
        let mut override_record = overridden_state.decisions.get(&override_key);
        override_record.decision = Decision::Mine;
        overridden_state.decisions.records = vec![override_record];
        let overridden = overridden_state.card_context(&resolved_item, CLEARLY_DUE, 0);
        assert!(!overridden.as_ref().unwrap().closed);
        assert_eq!(card_rank(overridden.as_ref()), 1);
    }
    #[test]
    fn past_reminder_times_are_rejected() {
        assert!(reminder_time("2000-01-01 12:00").is_err());
        assert!(reminder_time("tomorrow").is_err());
    }

    #[test]
    fn reminder_button_enabled_on_any_open_card_without_a_reminder() {
        for (closed, reminder, expected) in [
            (false, Reminder::None, true),
            (false, Reminder::Attempted, false),
            (false, Reminder::Created, false),
            (true, Reminder::None, false),
            (true, Reminder::Attempted, false),
            (true, Reminder::Created, false),
        ] {
            assert_eq!(
                reminder_button_enabled(closed, reminder),
                expected,
                "closed={closed} reminder={reminder:?}"
            );
        }
    }

    #[test]
    fn decision_after_setting_reminder_implies_tracking_except_when_watching() {
        for (current, expected) in [
            (Decision::Review, Decision::Mine),
            (Decision::Mine, Decision::Mine),
            (Decision::Watching, Decision::Watching),
        ] {
            assert_eq!(
                decision_after_setting_reminder(current),
                expected,
                "current={current:?}"
            );
        }
    }

    /// `closed` is the single source of truth for `show_analysis`'s hide
    /// condition (`if closed && !self.show_handled { continue; }`): the
    /// card is skipped exactly when `closed && !show_handled`. Exercising
    /// `closed` here covers "hidden unless the show-handled checkbox is
    /// on" without needing to render the actual egui widgets.
    #[test]
    fn resolved_item_without_a_decision_is_closed_and_hides_unless_shown() {
        let (state, mut item) = aging_fixture();
        item.resolution = Some(item.evidence.clone());
        item.resolution_kind = Some(ResolutionKind::Completed);
        let closed = state.card_context(&item, 0, 0).unwrap().closed;
        assert!(closed, "resolved item must be closed by default");
        for show_handled in [false, true] {
            let hidden = closed && !show_handled;
            assert_eq!(hidden, !show_handled, "show_handled={show_handled}");
        }
    }

    #[test]
    fn event_passed_closure_hides_sorts_labels_and_can_be_overridden() {
        let (mut state, mut item) = aging_fixture();
        let end = chrono::DateTime::parse_from_rfc3339("2026-08-21T19:30:00Z")
            .unwrap()
            .timestamp();
        item.event_passed = Some(EventPassed {
            name: "design workshop".into(),
            end,
        });
        let card = state.card_context(&item, 0, 0).unwrap();
        assert!(card.closed);
        assert_eq!(card_rank(Some(&card)), 2);
        assert_eq!(
            status_base_label(Decision::Review, &item),
            event_passed_status_label(item.event_passed.as_ref().unwrap())
        );

        let key = card.record.key;
        let mut record = state.decisions.get(&key);
        record.decision = Decision::Mine;
        state.decisions.records = vec![record];
        let overridden = state.card_context(&item, 0, 0).unwrap();
        assert!(!overridden.closed);
        assert_eq!(status_base_label(Decision::Mine, &item), "Tracking");

        let analysis = Expectations {
            items: vec![item],
            rejected: 0,
            rejection_reasons: vec![],
            degraded: 0,
        };
        let (plain_state, _) = aging_fixture();
        let cards = plain_state.card_contexts(&analysis.items);
        let summary = expectations_summary(&analysis, &cards, "model");
        assert!(
            summary.contains("· 1 closed because the event passed"),
            "summary: {summary}"
        );
    }

    #[test]
    fn resolved_item_with_decision_mine_or_watching_stays_open() {
        let (mut state, mut item) = aging_fixture();
        item.resolution = Some(item.evidence.clone());
        item.resolution_kind = Some(ResolutionKind::Agreed);
        for decision in [Decision::Mine, Decision::Watching] {
            let key = state.card_context(&item, 0, 0).unwrap().record.key;
            let mut record = state.decisions.get(&key);
            record.decision = decision;
            state.decisions.records = vec![record];
            let card = state.card_context(&item, 0, 0).unwrap();
            assert!(!card.closed, "{decision:?} should keep the card open");
            assert!(!card.terminal);
        }
    }

    #[test]
    fn status_base_label_prefers_mine_or_watching_override_over_resolution() {
        let (_, mut item) = aging_fixture();
        item.resolution = Some(item.evidence.clone());
        item.resolution_kind = Some(ResolutionKind::Completed);
        assert_eq!(status_base_label(Decision::Mine, &item), "Tracking");
        assert_eq!(
            status_base_label(Decision::Watching, &item),
            "Watching team follow-up"
        );
        // Without an override, the resolution still gets to speak for
        // itself.
        assert_eq!(
            status_base_label(Decision::Review, &item),
            "Resolved: completed"
        );
        // Terminal decisions win outright, same as before.
        assert_eq!(status_base_label(Decision::Done, &item), "Handled");
    }

    #[test]
    fn resolution_status_label_matches_each_kind() {
        assert_eq!(
            resolution_status_label(Some(ResolutionKind::Completed)),
            "Resolved: completed"
        );
        assert_eq!(
            resolution_status_label(Some(ResolutionKind::Declined)),
            "Resolved: declined"
        );
        assert_eq!(
            resolution_status_label(Some(ResolutionKind::Withdrawn)),
            "Resolved: withdrawn by the requester"
        );
        assert_eq!(
            resolution_status_label(Some(ResolutionKind::Superseded)),
            "Resolved: request replaced by the requester"
        );
        assert_eq!(
            resolution_status_label(Some(ResolutionKind::Agreed)),
            "Resolved: you agreed"
        );
        for label in [
            resolution_status_label(Some(ResolutionKind::Completed)),
            resolution_status_label(Some(ResolutionKind::Declined)),
            resolution_status_label(Some(ResolutionKind::Withdrawn)),
            resolution_status_label(Some(ResolutionKind::Superseded)),
            resolution_status_label(Some(ResolutionKind::Agreed)),
        ] {
            assert!(!label.contains("confirm below"));
        }
    }

    #[test]
    fn resolution_anchor_label_reports_cross_thread_regardless_of_kind() {
        for kind in [
            Some(ResolutionKind::Completed),
            Some(ResolutionKind::Declined),
            Some(ResolutionKind::Withdrawn),
            Some(ResolutionKind::Superseded),
            Some(ResolutionKind::Agreed),
            None,
        ] {
            assert_eq!(
                resolution_anchor_label(kind, true),
                "Later evidence in another conversation"
            );
        }
    }

    #[test]
    fn status_label_appends_cross_thread_suffix_only_when_set() {
        assert_eq!(status_label("Tracking", false), "Tracking");
        assert_eq!(
            status_label("Tracking", true),
            "Tracking (evidence in another conversation)"
        );
    }

    #[test]
    fn resolution_anchor_label_matches_each_kind() {
        assert_eq!(
            resolution_anchor_label(Some(ResolutionKind::Completed), false),
            "Later completion evidence"
        );
        assert_eq!(
            resolution_anchor_label(Some(ResolutionKind::Declined), false),
            "Later decline evidence"
        );
        assert_eq!(
            resolution_anchor_label(Some(ResolutionKind::Withdrawn), false),
            "Later withdrawal by the requester"
        );
        assert_eq!(
            resolution_anchor_label(Some(ResolutionKind::Superseded), false),
            "Later replacement by the requester"
        );
        assert_eq!(
            resolution_anchor_label(Some(ResolutionKind::Agreed), false),
            "Your later agreement"
        );
    }

    #[test]
    fn resolution_status_label_none_is_neutral() {
        assert_eq!(resolution_status_label(None), "Resolved");
    }

    #[test]
    fn resolution_anchor_label_none_is_neutral() {
        assert_eq!(
            resolution_anchor_label(None, false),
            "Later resolution evidence"
        );
    }

    #[test]
    fn expectations_summary_counts_an_overridden_resolution_as_open() {
        let (mut state, mut item) = aging_fixture();
        item.resolution = Some(item.evidence.clone());
        item.resolution_kind = Some(ResolutionKind::Completed);
        let analysis = Expectations {
            items: vec![item.clone()],
            rejected: 0,
            rejection_reasons: vec![],
            degraded: 0,
        };
        let cards = state.card_contexts(&analysis.items);
        assert!(cards[0].as_ref().unwrap().closed);
        let summary = expectations_summary(&analysis, &cards, "model");
        assert!(
            summary.starts_with("0 expectations · 1 resolved"),
            "summary: {summary}"
        );

        let key = state.card_context(&item, 0, 0).unwrap().record.key;
        let mut record = state.decisions.get(&key);
        record.decision = Decision::Mine;
        state.decisions.records = vec![record];
        let cards = state.card_contexts(&analysis.items);
        assert!(!cards[0].as_ref().unwrap().closed);
        let summary = expectations_summary(&analysis, &cards, "model");
        assert!(
            summary.starts_with("1 expectations · 0 resolved"),
            "summary: {summary}"
        );
    }

    #[test]
    fn expectations_summary_excludes_manually_handled_items_without_resolution() {
        // A manually "Handled" item with no resolution evidence at all must
        // not count as "resolved by later evidence": a terminal decision
        // closes the card, but says nothing about resolution.
        let (mut state, item) = aging_fixture();
        assert!(item.resolution.is_none());
        let analysis = Expectations {
            items: vec![item.clone()],
            rejected: 0,
            rejection_reasons: vec![],
            degraded: 0,
        };
        let key = state.card_context(&item, 0, 0).unwrap().record.key;
        let mut record = state.decisions.get(&key);
        record.decision = Decision::Done;
        state.decisions.records = vec![record];
        let cards = state.card_contexts(&analysis.items);
        assert!(cards[0].as_ref().unwrap().closed);
        assert!(cards[0].as_ref().unwrap().terminal);
        let summary = expectations_summary(&analysis, &cards, "model");
        assert!(
            summary.contains("0 resolved by later evidence"),
            "summary: {summary}"
        );
    }

    #[test]
    fn expectations_summary_reports_cross_thread_closed_count() {
        let (state, mut item) = aging_fixture();
        item.resolution = Some(item.evidence.clone());
        item.resolution_kind = Some(ResolutionKind::Completed);
        item.cross_thread = true;
        let analysis = Expectations {
            items: vec![item],
            rejected: 0,
            rejection_reasons: vec![],
            degraded: 0,
        };
        let cards = state.card_contexts(&analysis.items);
        assert!(cards[0].as_ref().unwrap().closed);
        let summary = expectations_summary(&analysis, &cards, "model");
        assert!(
            summary.contains("1 closed from evidence in other conversations"),
            "summary: {summary}"
        );
    }

    #[test]
    fn status_shows_cross_thread_only_from_the_resolution_branch() {
        assert!(status_shows_cross_thread(Decision::Review, true, true));
        assert!(!status_shows_cross_thread(Decision::Review, false, true));
        assert!(!status_shows_cross_thread(Decision::Review, true, false));
        assert!(!status_shows_cross_thread(Decision::Mine, true, true));
        assert!(!status_shows_cross_thread(Decision::Watching, true, true));
        assert!(!status_shows_cross_thread(Decision::Done, true, true));
        assert!(!status_shows_cross_thread(Decision::Dismissed, true, true));
        assert!(!status_shows_cross_thread(Decision::Moot, true, true));
    }

    #[test]
    fn set_scan_reports_closure_pass_failure_as_incomplete() {
        let mut state = ReviewState::default();
        state.set_scan(
            ScanResult {
                analysis: Expectations {
                    items: vec![],
                    rejected: 0,
                    rejection_reasons: vec![],
                    degraded: 0,
                },
                failures: vec![],
                analyzed: 1,
                total: 1,
                cancelled: false,
                conversation_notes: vec![],
                cross_thread_closures: 0,
                event_closures: 0,
                primary_scan_transport_error: false,
                closure_pass_failure: Some(
                    "Cross-thread closure pass stopped: rate limited".into(),
                ),
            },
            "model".into(),
        );
        assert!(state.scan_incomplete);
        assert!(
            state
                .scan_errors
                .iter()
                .any(|e| e == "Cross-thread closure pass stopped: rate limited")
        );
    }

    #[test]
    fn review_state_loaded_assigns_globally_unique_handles_across_sources() {
        // Mirrors the concern the old hand-simulated test covered, but now
        // drives `ReviewState::loaded` directly with two `SourceReview`s:
        // `index` is the running count of already-loaded messages across
        // ALL sources, not a per-source counter, so show_anchor's handle
        // lookup (which searches every loaded message regardless of source)
        // never sees a collision.
        use std::collections::BTreeSet;
        let mail = |id: &str, conversation: &str, day: u32, body: &str| {
            openloops_graph::live::review::MailItem {
                id: id.into(),
                account: "synthetic".into(),
                conversation: conversation.into(),
                received: format!("2026-09-0{day}T12:00:00Z"),
                body: body.into(),
                ..Default::default()
            }
        };
        let sources = vec![
            SourceReview {
                label: "Inbox".into(),
                messages: vec![
                    mail("i-1", "c1", 1, "Please send the draft."),
                    mail("i-2", "c2", 2, "Second message."),
                ],
                errors: vec![],
                partial: false,
            },
            SourceReview {
                label: "Sent".into(),
                messages: vec![mail("s-1", "c3", 3, "Here is the draft.")],
                errors: vec![],
                partial: false,
            },
        ];
        let state = ReviewState::loaded(sources);
        assert_eq!(state.messages.len(), 3);
        let handles: BTreeSet<&str> = state
            .messages
            .iter()
            .map(|m| m.input.handle.as_str())
            .collect();
        assert_eq!(handles.len(), state.messages.len());
        for (i, m) in state.messages.iter().enumerate() {
            assert_eq!(m.input.handle, format!("m{i}"));
        }
    }
}
