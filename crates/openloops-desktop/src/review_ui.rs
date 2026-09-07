//! Conversation review and explicit decisions. Readable mail stays in memory.
use crate::deadline_view::{DeadlineView, classify, label};
use crate::loop_state::{Decision, Decisions, Record, Reminder, marker, now};
use chrono::TimeZone;
use eframe::egui::{self, Color32, RichText};
use openloops_graph::live::{reminders::ReminderRequest, review::SourceReview};
use openloops_inference::ollama::expectations::{
    Anchor, Expectation, Expectations, Owner, ResolutionKind,
};
#[path = "review_scan.rs"]
mod scanning;
pub use scanning::{ReviewMessage, ScanProgress, ScanResult, probe, scan};

const SHOW_HANDLED_LABEL: &str = "Show handled, dismissed, and no-longer-relevant items";

struct ReminderDraft {
    key: [u8; 32],
    account: String,
    title: String,
    when: String,
    error: String,
}

struct CardContext<'a> {
    source: &'a ReviewMessage,
    record: Record,
    terminal: bool,
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

fn card_rank(card: Option<&CardContext<'_>>) -> u8 {
    match card {
        Some(card) if card.terminal => 2,
        Some(card) if card.deadline.as_ref().is_some_and(is_past_due) => 0,
        _ => 1,
    }
}

fn card_order(cards: &[Option<CardContext<'_>>]) -> Vec<usize> {
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
            || result.analysis.degraded > 0;
        self.scan_errors = result.failures;
        for reason in &result.analysis.rejection_reasons {
            if !self.scan_errors.iter().any(|existing| existing == reason) {
                self.scan_errors.push((*reason).into());
            }
        }
        self.scan_errors.extend(result.conversation_notes);
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
        self.show_draft(ui);
        if !self.action_status.is_empty() {
            ui.label(&self.action_status);
        }
        ui.collapsing(
            format!("Scanned messages ({})", self.messages.len()),
            |ui| {
                for m in &self.messages {
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
                }
            },
        );
    }
    fn card_context(
        &self,
        item: &Expectation,
        now: i64,
        now_offset: i32,
    ) -> Option<CardContext<'_>> {
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
            source,
            record,
            terminal,
            deadline,
        })
    }

    fn card_contexts(&self, items: &[Expectation]) -> Vec<Option<CardContext<'_>>> {
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
        let degraded_note = if analysis.degraded > 0 {
            format!(" · {} kept with unverified evidence", analysis.degraded)
        } else {
            String::new()
        };
        ui.label(format!(
            "{} expectations · {} rejected for invalid evidence{} · {}",
            analysis.items.len(),
            analysis.rejected,
            degraded_note,
            self.analysis_model
        ));
        ui.label("Review the action and evidence. A missing reply in this scan does not prove the work is unfinished.");
        ui.checkbox(&mut self.show_handled, SHOW_HANDLED_LABEL);
        if analysis.items.is_empty() {
            ui.label(if analysis.rejected>0 {"No usable expectations were returned. Evidence validation rejected suggestions; this is not a clean bill of health."} else {"No actionable expectations were identified in the successfully reviewed conversations."});
        }
        let mut change = None;
        let mut draft = None;
        let cards = self.card_contexts(&analysis.items);
        let order = card_order(&cards);
        for &i in &order {
            let item = &analysis.items[i];
            let Some(CardContext {
                source,
                record,
                terminal,
                deadline: deadline_view,
            }) = &cards[i]
            else {
                continue;
            };
            let key = record.key;
            let terminal = *terminal;
            if terminal && !self.show_handled {
                continue;
            }
            ui.push_id(i,|ui|{
                egui::Frame::group(ui.style()).show(ui,|ui|{
                    ui.set_width(ui.available_width());
                    let status=if record.decision==Decision::Done {"Handled"} else if record.decision==Decision::Dismissed {"Dismissed / not mine"} else if record.decision==Decision::Moot {"No longer relevant"} else if item.resolution.is_some() {resolution_status_label(item.resolution_kind)} else if record.decision==Decision::Mine {"Tracking"} else if record.decision==Decision::Watching {"Watching team follow-up"} else {"Needs your review"};
                    ui.label(RichText::new(status).color(Color32::from_rgb(29,87,67)));
                    ui.label(RichText::new(&item.action).size(21.0).strong());
                    let owner=if record.decision==Decision::Mine {"You (confirmed)"} else {match item.owner {Owner::You=>"You (suggested)",Owner::Team=>"Team — no individual owner established",Owner::Unclear=>"Unclear — confirm responsibility"}};
                    ui.label(format!("Responsible: {owner}"));
                    ui.label(format!("Waiting: {}",item.waiting_party));
                    if let Some(deadline)=&item.deadline {ui.label(format!("Deadline stated in email: {}",deadline.quote));} else if item.unverified_deadline {ui.label("A deadline was stated, but its quotation could not be verified.");} else {ui.label("Deadline stated in email: Not specified");}
                    if let Some(view)=deadline_view {if !terminal && is_past_due(view) {ui.colored_label(Color32::DARK_RED,label(view));} else {ui.label(label(view));}}
                    if !item.uncertainty.is_empty() {ui.label(format!("Uncertainty: {}",item.uncertainty));}
                    ui.label(RichText::new(format!("{} · {} · {}",source.source,source.date_label,item.kind)).small());
                    ui.collapsing("Why this was suggested · evidence and replies",|ui|{
                        show_anchor(ui,"Original expectation",&item.evidence,&self.messages);
                        if let Some(deadline)=&item.deadline {show_anchor(ui,"Deadline evidence",deadline,&self.messages);}
                        if let Some(resolution)=&item.resolution {show_anchor(ui,resolution_anchor_label(item.resolution_kind),resolution,&self.messages);} else if item.unverified_resolution {ui.label("The analysis proposed a completion but it could not be validated; treat as open.");} else {ui.label("No matching completion was identified in the scanned conversation. Work may have happened elsewhere or outside this history window.");}
                        ui.collapsing("Full scanned conversation",|ui| {for m in self.messages.iter().filter(|m|m.account==source.account && m.conversation==source.conversation) {ui.label(format!("{} · {}",m.date_label,if m.input.from_user {"You"} else {"Other participant"}));for b in &m.input.message.body_blocks {ui.label(b.as_string());}}});
                    });
                    ui.horizontal_wrapped(|ui|{
                        if terminal {if ui.button("Reopen for review").clicked() {change=Some((key,Decision::Review,record.reminder));}}
                        else {
                            if record.decision!=Decision::Mine&&ui.button("Track — this is mine").clicked(){change=Some((key,Decision::Mine,record.reminder));}
                            if item.owner!=Owner::You && record.decision!=Decision::Watching && ui.button("Keep an eye on this").clicked(){change=Some((key,Decision::Watching,record.reminder));}
                            if ui.button("Handled").clicked(){change=Some((key,Decision::Done,record.reminder));}
                            if ui.button("Not mine / dismiss").clicked(){change=Some((key,Decision::Dismissed,record.reminder));}
                            if ui.button("No longer relevant").clicked(){change=Some((key,Decision::Moot,record.reminder));}
                            if record.reminder==Reminder::None && ui.add_enabled(matches!(record.decision,Decision::Mine|Decision::Watching),egui::Button::new("Set To Do reminder…")).clicked(){draft=Some(ReminderDraft {key,account:source.account.clone(),title:if record.decision==Decision::Watching {format!("Follow up: {}",item.action)} else {item.action.clone()},when:default_reminder(),error:String::new()});}
                        }
                    });
                    match record.reminder {
                        Reminder::None=>{},
                        Reminder::Created=>{ui.label("Reminder created in Microsoft To Do. Manage its alerts and completion there; marking this loop handled does not modify the task.");ui.hyperlink_to("Open Microsoft To Do","https://to-do.office.com/tasks/");},
                        Reminder::Attempted=>{
                            ui.colored_label(Color32::DARK_RED,"A reminder attempt has no confirmed outcome. Inspect Microsoft To Do before allowing another attempt.");
                            ui.hyperlink_to("Inspect Microsoft To Do","https://to-do.office.com/tasks/");
                            ui.label(format!("Reference: {}",marker(&key)));
                            if ui.button("I checked: the task exists").clicked(){change=Some((key,record.decision,Reminder::Created));}
                            if ui.button("I checked: no task was created").clicked(){change=Some((key,record.decision,Reminder::None));}
                        }
                    }
                });
            });
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
        if draft.is_some() {
            self.draft = draft;
        }
    }
    fn show_draft(&mut self, ui: &mut egui::Ui) {
        let Some(draft) = &mut self.draft else {
            return;
        };
        let mut cancel = false;
        let mut create = false;
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
        }
    }
}

fn resolution_status_label(kind: Option<ResolutionKind>) -> &'static str {
    match kind {
        Some(ResolutionKind::Completed) | None => "Possible completion — confirm below",
        Some(ResolutionKind::Declined) => "Possible decline — confirm below",
        Some(ResolutionKind::Withdrawn) => "Possibly withdrawn by the requester — confirm below",
        Some(ResolutionKind::Superseded) => {
            "Possibly superseded by a later message — confirm below"
        }
        Some(ResolutionKind::Renegotiated) => {
            "Possibly renegotiated (new terms proposed) — confirm below"
        }
    }
}
fn resolution_anchor_label(kind: Option<ResolutionKind>) -> &'static str {
    match kind {
        Some(ResolutionKind::Completed) | None => "Later completion evidence",
        Some(ResolutionKind::Declined) => "Later decline evidence",
        Some(ResolutionKind::Withdrawn) => "Later withdrawal by the requester",
        Some(ResolutionKind::Superseded) => "Later superseding message",
        Some(ResolutionKind::Renegotiated) => "Later renegotiation (counter-proposal)",
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
        ui.collapsing("Surrounding source text", |ui| {
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
            resolution: None,
            resolution_kind: None,
            uncertainty: if index == 0 {
                String::new()
            } else {
                "The request was sent to the Group; no individual owner is named.".into()
            },
            unverified_deadline: false,
            unverified_resolution: false,
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
            resolution: None,
            resolution_kind: None,
            uncertainty: String::new(),
            unverified_deadline: false,
            unverified_resolution: false,
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
    }
    #[test]
    fn past_reminder_times_are_rejected() {
        assert!(reminder_time("2000-01-01 12:00").is_err());
        assert!(reminder_time("tomorrow").is_err());
    }
}
