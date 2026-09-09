//! Conversation review rendering. Toolkit-free state and pure decision/
//! status/urgency logic live in `review_model.rs`; this file draws `egui`
//! widgets and calls into it.
use crate::app_model::is_outlook_link;
use crate::deadline_view::label;
use crate::loop_state::{Decision, Record, Reminder, marker};
use crate::review_model::{
    CardContext, ReminderDraft, ReviewMessage, ReviewState, SHOW_HANDLED_LABEL, card_hidden,
    card_order, decision_after_setting_reminder, default_reminder, expectations_summary,
    is_past_due, reminder_button_enabled, reminder_time, resolution_anchor_label,
    status_base_label, status_label, status_shows_cross_thread,
};
use eframe::egui::{self, Color32, RichText};
use openloops_graph::live::reminders::ReminderRequest;
use openloops_inference::expectations::{Anchor, EventPassed, Expectation, Owner};

impl ReviewState {
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
            if card_hidden(card.closed, self.show_handled, show_draft_here) {
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
                // `cards[i]` is `Some(card)` (checked above), which
                // `card_context` only returns after this exact lookup
                // already succeeded once for `item.evidence.message` -- the
                // same `self.messages`, unchanged since -- so it cannot fail
                // here.
                let source = self
                    .messages
                    .iter()
                    .find(|m| m.input.handle == item.evidence.message)
                    .expect(
                        "card_context returned Some for this item, so its source message exists",
                    );
                ui.push_id(i, |ui| {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        let (c, d) = render_card_body(
                            ui,
                            &self.messages,
                            item,
                            source,
                            &card,
                            show_draft_here,
                        );
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
            self.apply_decision_change(key, decision, reminder);
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
        let mut cancel_clicked = false;
        let mut create_clicked = false;
        let draft = self.draft.as_mut().expect("checked above");
        let prior_decision = draft.prior_decision;
        let response = ui
            .push_id(("reminder-draft", key), |ui| {
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
                    ui.horizontal(|ui|{create_clicked=ui.button("Create this reminder in Microsoft To Do").clicked();cancel_clicked=ui.button("Cancel").clicked();});
                    if !draft.error.is_empty(){ui.colored_label(Color32::DARK_RED,&draft.error);}
                });
            })
            .response;
        // Scrolled once, the first render after opening -- via the id-scoped
        // group's own response, so the whole (possibly tall) panel is
        // brought into view rather than whatever egui's cursor happens to
        // sit at afterward.
        if !self.draft_scrolled {
            response.scroll_to_me(Some(egui::Align::Center));
            self.draft_scrolled = true;
        }
        let mut close_draft = false;
        if create_clicked {
            match reminder_time(&draft.when) {
                Ok(at) if draft.title.trim().len()>=3 && draft.title.len()<=320 => {
                    match self.decisions.begin_reminder(draft.key){Ok(())=>{self.pending_reminder=Some((draft.key,ReminderRequest {account:draft.account.clone(),title:draft.title.trim().into(),at_utc:at,marker:marker(&draft.key)}));close_draft=true;},Err(e)=>draft.error=e}
                }
                _=>draft.error="Enter a future local date/time and a title of 3–320 bytes. Ambiguous daylight-saving times need a different time.".into(),
            }
        }
        if cancel_clicked {
            // Cancel undoes the implied-tracking change from opening the
            // draft (see `decision_after_cancel`), but only when nothing
            // else moved the decision on in the meantime -- a successful
            // create (handled above) never reaches here, so it always keeps
            // `Mine`/`Watching`.
            self.revert_draft_decision(prior_decision, key);
            close_draft = true;
        }
        if close_draft {
            self.draft = None;
            self.draft_scrolled = false;
        }
    }
}

/// Renders one card's whole body -- status, action/owner/waiting/deadline
/// summary, evidence collapsing, action buttons, and reminder status --
/// inside the caller's `egui::Frame::group`. Returns the decision change and
/// any reminder draft the action buttons produced, so the caller (which owns
/// `self.decisions`/`self.draft`) can apply them; this function itself never
/// touches `self`, so it can run while the caller still holds an immutable
/// borrow of `self.analysis`/`self.messages` (see
/// [`ReviewState::show_analysis`]). `draft_open_here` is whether a reminder
/// draft is already open for this card's key -- passed through to
/// `card_action_buttons` so the "Set To Do reminder…" button greys itself
/// out (stays visible, disabled) rather than silently replacing an
/// already-open, possibly-edited draft.
fn render_card_body(
    ui: &mut egui::Ui,
    messages: &[ReviewMessage],
    item: &Expectation,
    source: &ReviewMessage,
    card: &CardContext,
    draft_open_here: bool,
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
    let (mut change, draft) =
        card_action_buttons(ui, item, source, record, terminal, closed, draft_open_here);
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
    if let Some(passed) = &item.event_passed {
        show_event_time_evidence(ui, "Event time evidence", passed, messages);
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
/// of open-card actions. `draft_open_here` greys out "Set To Do reminder…"
/// (via `egui::Ui::add_enabled`, not removing the button) while a draft is
/// already open for this card, so it never silently replaces one the user
/// may have already started editing.
fn card_action_buttons(
    ui: &mut egui::Ui,
    item: &Expectation,
    source: &ReviewMessage,
    record: &Record,
    terminal: bool,
    closed: bool,
    draft_open_here: bool,
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
            let reminder_enabled = reminder_button_enabled(record.reminder, draft_open_here);
            if ui
                .add_enabled(reminder_enabled, egui::Button::new("Set To Do reminder…"))
                .clicked()
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
                    prior_decision: record.decision,
                });
            }
        }
    });
    (change, draft)
}
fn open_link(ui: &mut egui::Ui, url: &str) {
    if is_outlook_link(url) {
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
/// Like [`show_anchor`] but for an [`EventPassed`], which names the
/// invitation, calendar-subject, or event-time-phrase message the closure
/// evidence came from -- not a quoted anchor, so there is no quote or
/// "Surrounding source text" to show. When `from_subject` is set (the time
/// came from a calendar-invite subject or the subject's own prose, rather
/// than a meeting invite's metadata or an event-time phrase found in a
/// message body), an extra line says so; otherwise the rendering is
/// unchanged, just the link to that message.
fn show_event_time_evidence(
    ui: &mut egui::Ui,
    label: &str,
    passed: &EventPassed,
    messages: &[ReviewMessage],
) {
    ui.label(RichText::new(label).strong());
    if passed.from_subject {
        ui.label(RichText::new("From the subject line of this message").small());
    }
    if let Some(m) = messages
        .iter()
        .find(|m| m.input.handle == passed.message_handle)
    {
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
}
