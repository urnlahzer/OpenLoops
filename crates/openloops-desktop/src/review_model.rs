//! Toolkit-free review state and pure decision/urgency/status logic.
use crate::deadline_view::{DeadlineView, classify};
use crate::loop_state::{Decision, Decisions, Record, Reminder, now};
use openloops_graph::live::{reminders::ReminderRequest, review::SourceReview};
use openloops_inference::expectations::{
    EventPassed, Expectation, Expectations, Owner, ResolutionKind,
};
use std::sync::atomic::Ordering;
#[path = "review_scan.rs"]
mod scanning;
pub(crate) use scanning::{ReviewMessage, ScanProgress, ScanResult, probe, scan};

pub(crate) const SHOW_HANDLED_LABEL: &str = "Show resolved, handled and dismissed";

pub(crate) struct ReminderDraft {
    pub(crate) key: [u8; 32],
    pub(crate) account: String,
    pub(crate) title: String,
    pub(crate) when: String,
    pub(crate) error: String,
    /// The decision in force on this card immediately before the draft
    /// opened -- i.e. before [`decision_after_setting_reminder`] applied its
    /// implied-tracking change. Cancelling the draft without creating the
    /// reminder consults this against the current decision (see
    /// [`decision_after_cancel`]) to undo that implied change, but only when
    /// nothing else has since moved the decision on.
    pub(crate) prior_decision: Decision,
}

/// Per-card decision and urgency facts, independent of the source mail or
/// expectation item.
#[derive(Clone, Copy)]
pub(crate) struct CardContext {
    pub(crate) record: Record,
    pub(crate) terminal: bool,
    /// True when the card ranks and hides with the terminal group: either a
    /// terminal decision, or the expectation is resolved by later evidence
    /// and the saved decision has not explicitly overridden that closure
    /// (`Decision::Mine` or `Decision::Watching`).
    pub(crate) closed: bool,
    pub(crate) deadline: Option<DeadlineView>,
}

pub(crate) fn is_past_due(view: &DeadlineView) -> bool {
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

pub(crate) fn card_order(cards: &[Option<CardContext>]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..cards.len()).collect();
    order.sort_by_key(|&i| (card_rank(cards[i].as_ref()), i));
    order
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
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
    pub action_status_succeeded: bool,
    pub pending_reminder: Option<([u8; 32], ReminderRequest)>,
    /// An open reminder draft's implied-tracking change is reverted (see
    /// [`ReviewState::revert_draft_decision`]) whenever this field is
    /// cleared from *inside* `ReviewState` -- an explicit Cancel click or
    /// [`ReviewState::set_scan`] discarding a stale draft on rescan. The
    /// adapter's "Scan inboxes" and "Clear results and mail" actions instead
    /// replace the whole `ReviewState` with
    /// `ReviewState::default()`, dropping any open `draft` without going
    /// through that revert. This is left as is deliberately, not an
    /// oversight: unlike a rescan, which redraws the very same cards in
    /// place, so a stale implied `Mine` would sit right there on screen
    /// contradicting a decision the user never actually confirmed, a full
    /// "Scan inboxes"/"Clear results" reset clears the visible cards too --
    /// the stale decision only resurfaces on a later scan, where it reads
    /// like any other saved decision and the normal "Still open"/dismiss
    /// controls can correct it same as they would for one the user set on
    /// purpose. Treating "I was mid-draft when I started over" as "still
    /// tracking" is also the safer default of the two silent outcomes.
    pub(crate) draft: Option<ReminderDraft>,
    pub(crate) show_handled: bool,
    /// Set when the most recent scan/mail-load attempt failed outright
    /// (a worker disconnect, or a `ProviderError`/`ConnectionError` from
    /// `Outcome::Mail`/`Outcome::Scan`) rather than completing -- even
    /// partially -- with `set_scan`. Distinguishes that case from an
    /// ordinary "scan incomplete" result (which already carries its own
    /// warning-tinted `Finished` strip) so a rescan's outright failure
    /// still surfaces as the strip's `warning` state (spec §6) even though
    /// `analysis` still holds a previous, unrelated successful scan's
    /// results -- which stay listed, exactly as that section requires.
    pub(crate) scan_failed: bool,
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
        self.scan_failed = false;
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
            || result.closure_pass_failure.is_some()
            // Stopped by you can leave `analyzed == total` when only the
            // closure pass (which does not count toward `analyzed`/`total`)
            // was cut short, so this must be checked on its own rather than
            // assumed to already be covered by the `analyzed < total` case.
            || result.cancelled;
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
        // A rescan rebuilds `analysis`/`messages` from scratch; an open
        // draft refers to a card from the previous scan and would otherwise
        // be orphaned -- neither closeable (its card may no longer exist)
        // nor visibly tied to anything on screen. Revert the implied-tracking
        // change opening it made (see `decision_after_setting_reminder`),
        // same as an explicit Cancel click would (`revert_draft_decision`),
        // so a rescan can never silently leave a stray `Mine`/`Watching` the
        // user never actually confirmed sitting on a now-invisible draft.
        if let Some(draft) = self.draft.take() {
            self.revert_draft_decision(draft.prior_decision, draft.key);
        }
    }
    /// Reverts the implied-tracking change from opening a reminder draft
    /// (see [`decision_after_setting_reminder`]) for `key`, given `prior` --
    /// the decision in force just before that draft opened. Applies through
    /// `self.decisions.update` (recording `self.action_status`) only when
    /// [`decision_after_cancel`] says a revert applies; a no-op otherwise.
    /// Shared by an explicit Cancel click (`show_draft`) and `set_scan`
    /// discarding the draft out from under it on a rescan.
    pub(crate) fn revert_draft_decision(&mut self, prior: Decision, key: [u8; 32]) {
        if let Some(decision) = decision_after_cancel(prior, self.decisions.get(&key).decision) {
            let mut r = self.decisions.get(&key);
            r.decision = decision;
            r.updated = now();
            let saved = self.decisions.update(r);
            self.action_status_succeeded = saved.is_ok();
            self.action_status = match saved {
                Ok(()) => "Decision saved on this Windows account.".into(),
                Err(e) => e,
            };
        }
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
            let offset = scanning::local_offset_seconds(message.input.timestamp, now_offset);
            classify(&anchor.quote, message.input.timestamp, now, offset)
        });
        Some(CardContext {
            record,
            terminal,
            closed,
            deadline,
        })
    }

    pub(crate) fn card_contexts(&self, items: &[Expectation]) -> Vec<Option<CardContext>> {
        let clock = chrono::Local::now();
        items
            .iter()
            .map(|item| {
                self.card_context(item, clock.timestamp(), clock.offset().local_minus_utc())
            })
            .collect()
    }

    /// Applies a decision/reminder change requested for the card identified
    /// by `key`, updating `self.action_status` with the outcome.
    ///
    /// A terminal decision (reached from an action button while a reminder
    /// draft was open on this same card, e.g. "Handled") closes the card
    /// outright; an open draft on it no longer has anywhere sensible to
    /// render, so close the draft along with it rather than leaving it
    /// attached to a now-terminal card. Only when the update is `Ok`,
    /// though: if it was rejected (e.g. the saved-decision limit), the
    /// recorded decision never actually became terminal, so the draft must
    /// stay open and attached to its still-open card rather than vanishing
    /// out from under it.
    pub(crate) fn apply_decision_change(
        &mut self,
        key: [u8; 32],
        decision: Decision,
        reminder: Reminder,
    ) {
        let mut r = self.decisions.get(&key);
        r.decision = decision;
        r.reminder = reminder;
        r.updated = now();
        let saved = self.decisions.update(r);
        self.action_status_succeeded = saved.is_ok();
        self.action_status = match &saved {
            // X4: the second sentence is the Companion's own confirmation
            // (docs/design/OpenLoops Companion.dc.html, `setDecision`),
            // quoted verbatim onto the existing status sentence.
            Ok(()) => {
                "Decision saved on this Windows account. No mail text or names were stored.".into()
            }
            Err(e) => e.clone(),
        };
        if saved.is_ok()
            && matches!(
                decision,
                Decision::Done | Decision::Dismissed | Decision::Moot
            )
            && self.draft.as_ref().is_some_and(|d| d.key == key)
        {
            self.draft = None;
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
pub(crate) fn status_base_label(decision: Decision, item: &Expectation) -> String {
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
pub(crate) fn status_label(base: &str, cross_thread: bool) -> String {
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
pub(crate) fn status_shows_cross_thread(
    decision: Decision,
    resolved: bool,
    cross_thread: bool,
) -> bool {
    cross_thread && decision == Decision::Review && resolved
}

pub(crate) fn resolution_anchor_label(
    kind: Option<ResolutionKind>,
    cross_thread: bool,
) -> &'static str {
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
pub(crate) fn expectations_summary(
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
/// Whether "Set To Do reminder…" should be enabled: a card without a
/// reminder already attempted or created for it, and without a draft already
/// open for it. Setting a reminder no longer requires first tracking or
/// watching the card -- opening the draft implies tracking on its own (see
/// [`decision_after_setting_reminder`]). `card_action_buttons` only ever
/// calls this from its open-card branch, where `closed` is always `false`,
/// so `closed` is not (and must not be) a parameter here: `draft_open_here`
/// is the only thing that can additionally disable the button, guarding
/// against silently replacing a draft the user may have already edited.
pub(crate) fn reminder_button_enabled(reminder: Reminder, draft_open_here: bool) -> bool {
    reminder == Reminder::None && !draft_open_here
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
pub(crate) fn decision_after_setting_reminder(current: Decision) -> Decision {
    match current {
        Decision::Watching => Decision::Watching,
        _ => Decision::Mine,
    }
}
/// Decision to revert to when a reminder draft is cancelled without creating
/// the reminder, given the decision that was in force just before the draft
/// opened (`prior`) and the decision recorded now (`current`). Only reverses
/// exactly the implied change [`decision_after_setting_reminder`] makes from
/// `Decision::Review`: when `prior` was `Review` and `current` is still the
/// implied `Mine`, cancel restores `Review`. Any other combination leaves the
/// decision alone -- `prior` was already `Mine`/`Watching` (nothing was
/// implied, so there is nothing to undo), or `current` has since moved to
/// something else (an action button changed it, or a successful create was
/// made, while the draft was open) and undoing that would discard a decision
/// the user made deliberately. A successful create never reaches this
/// function at all (see [`ReviewState::show_draft`]), so it always keeps
/// `Mine`/`Watching` regardless.
fn decision_after_cancel(prior: Decision, current: Decision) -> Option<Decision> {
    if prior == Decision::Review && current == Decision::Mine {
        Some(Decision::Review)
    } else {
        None
    }
}
/// Whether a card should be hidden from the "What may need your attention"
/// list: `closed` and not revealed by the show-handled checkbox -- UNLESS a
/// reminder draft is currently open for this card's key, in which case the
/// card (and its draft) must stay visible so a rescan-independent decision
/// change (e.g. closure by later evidence) can never strand an open draft
/// behind a hidden card.
pub(crate) fn card_hidden(closed: bool, show_handled: bool, draft_open_here: bool) -> bool {
    closed && !show_handled && !draft_open_here
}
pub(crate) fn default_reminder() -> String {
    (chrono::Local::now() + chrono::Duration::hours(1))
        .format("%Y-%m-%d %H:%M")
        .to_string()
}
pub(crate) fn reminder_time(value: &str) -> Result<i64, ()> {
    use chrono::TimeZone;
    let naive = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M").map_err(|_| ())?;
    let time = chrono::Local
        .from_local_datetime(&naive)
        .single()
        .ok_or(())?
        .timestamp();
    if time <= now() { Err(()) } else { Ok(time) }
}

/// Which of the four "What may need your attention" list groups a card
/// belongs in (spec §4.3). `closed` always wins, regardless of the deadline
/// -- matches [`card_rank`]'s existing rank-2-for-closed behavior.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListGroup {
    PastDue,
    Due,
    NoFixedDeadline,
    Closed,
}

#[must_use]
pub(crate) fn list_group(card: &CardContext) -> ListGroup {
    if card.closed {
        return ListGroup::Closed;
    }
    match &card.deadline {
        None | Some(DeadlineView::EventTied | DeadlineView::Soft | DeadlineView::Unknown) => {
            ListGroup::NoFixedDeadline
        }
        Some(view) if is_past_due(view) => ListGroup::PastDue,
        Some(_) => ListGroup::Due,
    }
}

/// The review nav's ownership filter (spec §5). `All` always matches;
/// `Mine`/`Team` match only their own [`Owner`] -- `Owner::Unclear` matches
/// neither, so it appears only under `All`.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Mine,
    Team,
}

impl Filter {
    #[must_use]
    pub fn matches(self, owner: Owner) -> bool {
        match self {
            Self::All => true,
            Self::Mine => owner == Owner::You,
            Self::Team => owner == Owner::Team,
        }
    }
}

/// Count of loops where `decision ∈ {Review, Mine, Watching}` and not
/// auto-resolved. "Auto-resolved" here means `card.closed` (a `Review`-
/// decision card closed by resolution/event evidence with no explicit
/// `Mine`/`Watching` override -- `Mine`/`Watching` cards are never `closed`
/// by construction, see [`CardContext::closed`]'s doc comment).
///
/// Consumed by the nav rail badge.
#[must_use]
pub fn open_badge_count(cards: &[Option<CardContext>]) -> usize {
    cards
        .iter()
        .flatten()
        .filter(|card| {
            !card.closed
                && matches!(
                    card.record.decision,
                    Decision::Review | Decision::Mine | Decision::Watching
                )
        })
        .count()
}

/// The scan-progress strip's state (spec §4.2): actively scanning, a
/// finished scan's summary/coverage, or idle (no scan has run yet).
///
#[derive(Clone, Debug, PartialEq)]
pub enum ScanStrip {
    Scanning {
        phase: &'static str,
        conversation_index: usize,
        conversation_total: usize,
        processed: usize,
        total: usize,
        elapsed_secs: i64,
        percent: u8,
        stopping: bool,
    },
    Finished {
        incomplete: bool,
        summary: String,
        coverage_notes: Vec<String>,
    },
    Idle,
}

#[must_use]
pub fn scan_strip(progress: Option<&ScanProgress>, review: &ReviewState) -> ScanStrip {
    let Some(progress) = progress else {
        return if review.analysis.is_some() {
            ScanStrip::Finished {
                incomplete: review.scan_incomplete,
                summary: review.scan_summary.clone(),
                coverage_notes: review
                    .notices
                    .iter()
                    .chain(&review.scan_errors)
                    .cloned()
                    .collect(),
            }
        } else {
            ScanStrip::Idle
        };
    };
    let phase = if progress.closure_phase.load(Ordering::Relaxed) {
        "Cross-thread closure check"
    } else {
        "Finding open loops"
    };
    let processed = progress.processed.load(Ordering::Relaxed);
    let total = progress.total.load(Ordering::Relaxed);
    let conversation_total = progress.conversation_total.load(Ordering::Relaxed);
    let snapshot = progress.snapshot();
    let conversation_index = snapshot
        .conversation_index
        .saturating_sub(snapshot.in_flight);
    let elapsed_secs = if snapshot.request_started_unix == 0 {
        0
    } else {
        (chrono::Utc::now().timestamp() - snapshot.request_started_unix).max(0)
    };
    let percent = (processed * 100)
        .checked_div(total)
        .map_or(0, |value| u8::try_from(value.min(100)).unwrap_or(100));
    ScanStrip::Scanning {
        phase,
        conversation_index,
        conversation_total,
        processed,
        total,
        elapsed_secs,
        percent,
        stopping: progress.cancel.load(Ordering::Relaxed),
    }
}

/// A message fixture row for [`layout_fixture`]: `(subject, body, sender,
/// sender_address, source_label, conversation)`. `own_addresses` is always
/// `user@example.invalid`, so a message "from" that address renders with the
/// sent tint (see `conversation_rows`'s `sent` flag).
#[cfg(any(test, feature = "ui-screenshot"))]
type MessageFixture = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

#[cfg(any(test, feature = "ui-screenshot"))]
#[allow(clippy::too_many_lines)]
pub fn layout_fixture() -> ReviewState {
    use openloops_graph::live::review::MailItem;
    use openloops_inference::expectations::{Anchor, Expectation};
    let mut state = ReviewState::default();
    let body = "Please send the draft budget by Friday.";
    // m2 sits in its own conversation ("synthetic-thread-2") rather than
    // reusing m0/m1's "synthetic-thread": item 0's completion evidence
    // points at it, and the "evidence in another conversation" badge
    // (`cross_thread`) must stay truthful -- it would be a lie if the
    // completion lived in the very thread `conversation_rows` already shows
    // for that same card.
    let fixtures: [MessageFixture; 4] = [
        (
            "Quarterly planning",
            body,
            // T7 (brief §5): a long display name + address, both here and on
            // `waiting_party`/`evidence` below, so the preview at 1100 px
            // exercises the same sender-name wrap the "sender with a long
            // name" fixture requirement calls for.
            "Alexandra Priyanka Featherington-Vandermeer <alexandra.featherington-vandermeer@example-subdomain-name.invalid>",
            "alexandra.featherington-vandermeer@example-subdomain-name.invalid",
            "Personal mailbox / Inbox",
            "synthetic-thread",
        ),
        (
            "Quarterly planning",
            // T7: a ~4-line body ahead of the reply marker (the marker and
            // quoted text after it are untouched, so
            // `conversation_rows_mark_sent_mail_and_keep_quote_nested` still
            // sees the same quoted-history split).
            "I sent the draft budget this morning after folding in the finance team's revisions from yesterday's review call, including the updated headcount assumptions, the vendor renewal figures Priya flagged on Tuesday, and the revised travel contingency line that Legal asked us to break out separately this quarter.\n-----Original Message-----\nPlease send the draft budget by Friday.",
            "Synthetic User <user@example.invalid>",
            "user@example.invalid",
            "Group: planning@example.invalid",
            "synthetic-thread",
        ),
        (
            "Budget follow-up",
            "I sent the draft budget this morning.",
            "Synthetic User <user@example.invalid>",
            "user@example.invalid",
            "Group: planning@example.invalid",
            "synthetic-thread-2",
        ),
        (
            "Vendor invoice",
            "Please confirm the vendor invoice by end of day Friday.",
            "Priya <priya@example.invalid>",
            "priya@example.invalid",
            "Personal mailbox / Inbox",
            "synthetic-thread-3",
        ),
    ];
    for (index, (subject, body, sender, sender_address, label, conversation)) in
        fixtures.into_iter().enumerate()
    {
        let item = MailItem {
            subject: subject.into(),
            body: body.into(),
            sender: sender.into(),
            sender_address: sender_address.into(),
            own_addresses: vec!["user@example.invalid".into()],
            received: "2026-09-06T12:00:00Z".into(),
            id: format!("synthetic-{index}"),
            account: "synthetic".into(),
            conversation: conversation.into(),
            web_link: format!("https://outlook.office.com/mail/synthetic-{index}"),
            ..MailItem::default()
        };
        state
            .messages
            .push(scanning::prepare(&item, label, index).expect("synthetic fixture"));
    }
    let items = vec![
        // Card 1: tracked by you, a reminder already created, and completion
        // evidence found in another conversation (m2).
        Expectation {
            // T7 (brief §5): ~140 characters, long enough to wrap at 1100 px
            // in both the list row and the reading-pane title. `action_phrase`
            // (below) is untouched -- it feeds the decision-record
            // fingerprint several tests and `first_key` just below key on
            // verbatim, unlike this display-only field.
            action: "Send Alexandra the finalized draft budget, including the updated headcount assumptions and the vendor renewal figures Priya flagged, before Friday's sign-off meeting".into(),
            action_phrase: "send the draft budget".into(),
            owner: Owner::You,
            // T7: a long display name + address, matching m0's sender above.
            waiting_party: "Alexandra Priyanka Featherington-Vandermeer <alexandra.featherington-vandermeer@example-subdomain-name.invalid>".into(),
            kind: "request".into(),
            evidence: Anchor {
                message: "m0".into(),
                block: 0,
                // T7: a 3-line quote and a 3-line context, independent of
                // `deadline` below (its own quote must stay "Friday" verbatim
                // -- `selected_view_covers_deadline_metadata_and_actions`
                // asserts the meta grid's quoted deadline text).
                quote: "Could you send over the finalized draft budget before Thursday's sign-off meeting? I need to fold in the updated headcount numbers and the vendor renewal figures before we present it to the executive committee on Friday morning.".into(),
                context: "Addressed to you directly in the Inbox thread that also carries the Group's own budget follow-up message; the deadline is resolved from the Thursday meeting mentioned in the same paragraph, one day before the Friday executive review.".into(),
            },
            deadline: Some(Anchor {
                message: "m0".into(),
                block: 0,
                quote: "Friday".into(),
                context: body.into(),
            }),
            event: None,
            event_time: None,
            resolution: Some(Anchor {
                message: "m2".into(),
                block: 0,
                quote: "I sent the draft budget this morning.".into(),
                context: "I sent the draft budget this morning.".into(),
            }),
            resolution_kind: Some(ResolutionKind::Completed),
            // T7: 2 lines in the warning callout at 1100 px.
            uncertainty: "The deadline references Thursday's meeting only by day of week, and the message does not state whether the executive review the following day counts as the operative deadline instead.".into(),
            unverified_deadline: false,
            unverified_resolution: false,
            cross_thread: true,
            event_passed: None,
        },
        // Card 2: still needs a decision, no reminder -- this is the card
        // the preview's open draft attaches to. Its evidence message (m1)
        // shares m0's conversation, so the expanded "Full scanned
        // conversation" disclosure shows both messages: the sent tint on
        // m1 and its nested quoted-history block.
        Expectation {
            action: "Confirm who will send the team budget".into(),
            action_phrase: "send the draft budget".into(),
            owner: Owner::Team,
            waiting_party: "Alex <alex@example.invalid>".into(),
            kind: "request".into(),
            evidence: Anchor {
                message: "m1".into(),
                block: 0,
                quote: body.into(),
                context: body.into(),
            },
            deadline: Some(Anchor {
                message: "m1".into(),
                block: 0,
                quote: "Friday".into(),
                context: body.into(),
            }),
            event: None,
            event_time: None,
            resolution: None,
            resolution_kind: None,
            uncertainty: "The request was sent to the Group; no individual owner is named.".into(),
            unverified_deadline: false,
            unverified_resolution: false,
            cross_thread: false,
            event_passed: None,
        },
        // Card 3: a reminder attempt with no confirmed outcome, so the
        // preview also exercises the "attempted" marker callout and its two
        // reconcile buttons.
        Expectation {
            action: "Confirm the vendor invoice by Friday".into(),
            action_phrase: "confirm the vendor invoice".into(),
            owner: Owner::You,
            waiting_party: "Priya <priya@example.invalid>".into(),
            kind: "request".into(),
            evidence: Anchor {
                message: "m3".into(),
                block: 0,
                quote: "Please confirm the vendor invoice by end of day Friday.".into(),
                context: "Please confirm the vendor invoice by end of day Friday.".into(),
            },
            deadline: Some(Anchor {
                message: "m3".into(),
                block: 0,
                quote: "Friday".into(),
                context: "Please confirm the vendor invoice by end of day Friday.".into(),
            }),
            event: None,
            event_time: None,
            resolution: None,
            resolution_kind: None,
            uncertainty: String::new(),
            unverified_deadline: false,
            unverified_resolution: false,
            cross_thread: false,
            event_passed: None,
        },
    ];
    // T7 (brief §5): `source_failures` must be set before `set_scan` runs --
    // it reads `self.source_failures > 0` to derive `scan_incomplete`, and
    // `scan_strip` (§4) needs `incomplete: true` and a coverage count of 5 to
    // exercise the finished strip's warning line and "Coverage: 5 sources
    // incomplete" link at 1100 px.
    state.source_failures = 5;
    state.set_scan(
        ScanResult {
            analysis: Expectations {
                items,
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            analyzed: 4,
            total: 4,
            cancelled: false,
            conversation_notes: vec![],
            cross_thread_closures: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            closure_pass_failure: None,
        },
        // T7: a long model id so the finished strip's summary line (ending
        // "... · {model}") wraps at 1100 px.
        "anthropic/claude-sonnet-4.6-20260315:extended-thinking-zero-data-retention".into(),
    );
    let first_key =
        state
            .decisions
            .fingerprint("synthetic", "synthetic-0", "send the draft budget");
    let second_key =
        state
            .decisions
            .fingerprint("synthetic", "synthetic-1", "send the draft budget");
    let third_key =
        state
            .decisions
            .fingerprint("synthetic", "synthetic-3", "confirm the vendor invoice");
    state.decisions.records.push(Record {
        key: first_key,
        decision: Decision::Mine,
        reminder: Reminder::Created,
        updated: now(),
    });
    state.decisions.records.push(Record {
        key: third_key,
        decision: Decision::Watching,
        reminder: Reminder::Attempted,
        updated: now(),
    });
    state.draft = Some(ReminderDraft {
        key: second_key,
        account: "synthetic".into(),
        title: "Confirm who will send the team budget".into(),
        when: default_reminder(),
        error: String::new(),
        prior_decision: Decision::Review,
    });
    state.action_status = "Decision saved on this Windows account.".into();
    state.action_status_succeeded = true;
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use openloops_inference::expectations::Anchor;

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
            event_time: None,
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
            assert_eq!(
                is_past_due(&DeadlineView::DueDate {
                    day: 0,
                    boundary: 0,
                    past
                }),
                past
            );
            assert_eq!(
                is_past_due(&DeadlineView::DueBusinessDay {
                    day: 0,
                    boundary: 0,
                    past
                }),
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
    fn reminder_button_enabled_without_a_reminder_or_an_open_draft() {
        for (reminder, draft_open_here, expected) in [
            (Reminder::None, false, true),
            (Reminder::Attempted, false, false),
            (Reminder::Created, false, false),
            (Reminder::None, true, false),
            (Reminder::Attempted, true, false),
            (Reminder::Created, true, false),
        ] {
            assert_eq!(
                reminder_button_enabled(reminder, draft_open_here),
                expected,
                "reminder={reminder:?} draft_open_here={draft_open_here}"
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

    #[test]
    fn decision_after_cancel_reverts_only_the_implied_mine_from_review() {
        for (prior, current, expected) in [
            // The exact case `decision_after_setting_reminder` produces from
            // `Review`: cancel undoes it.
            (Decision::Review, Decision::Mine, Some(Decision::Review)),
            // Nothing implied changed (already `Mine`/`Watching` before the
            // draft opened): nothing to undo.
            (Decision::Mine, Decision::Mine, None),
            (Decision::Watching, Decision::Watching, None),
            // The decision has since moved on to something other than the
            // implied `Mine` (another action button, while the draft stayed
            // open): a stale `Review` prior must not clobber it.
            (Decision::Review, Decision::Watching, None),
            (Decision::Review, Decision::Done, None),
            (Decision::Review, Decision::Review, None),
            // Defensive: `prior` was never `Review`, so no revert fires even
            // if `current` happens to be `Mine`.
            (Decision::Watching, Decision::Mine, None),
            (Decision::Done, Decision::Mine, None),
        ] {
            assert_eq!(
                decision_after_cancel(prior, current),
                expected,
                "prior={prior:?} current={current:?}"
            );
        }
    }

    /// A rescan (`set_scan`) discards any open reminder draft outright (see
    /// the doc comment on `set_scan`), but must not silently strand the
    /// implied `Mine` that opening the draft applied to a `Review` card --
    /// same as an explicit Cancel click would revert it (see
    /// `decision_after_cancel`), via the shared `revert_draft_decision`.
    #[test]
    fn set_scan_reverts_an_open_drafts_implied_mine_back_to_review() {
        let (mut state, item) = aging_fixture();
        let source = state.messages[0].clone();
        let key = state
            .decisions
            .fingerprint(&source.account, &source.id, &item.action_phrase);
        // Simulate "Set To Do reminder..." having been clicked on this
        // `Review` card: `decision_after_setting_reminder` implied `Mine`.
        let mut record = state.decisions.get(&key);
        record.decision = Decision::Mine;
        state.decisions.records = vec![record];
        state.draft = Some(ReminderDraft {
            key,
            account: source.account.clone(),
            title: item.action.clone(),
            when: default_reminder(),
            error: String::new(),
            prior_decision: Decision::Review,
        });

        state.set_scan(
            ScanResult {
                analysis: Expectations {
                    items: vec![item],
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
                closure_pass_failure: None,
            },
            "model".into(),
        );

        assert!(state.draft.is_none());
        assert_eq!(state.decisions.get(&key).decision, Decision::Review);
    }

    #[test]
    fn card_hidden_stays_visible_while_its_draft_is_open() {
        for (closed, show_handled, draft_open_here, expected) in [
            (false, false, false, false),
            (false, true, false, false),
            (true, false, false, true),
            (true, true, false, false),
            (true, false, true, false),
            (true, true, true, false),
            (false, false, true, false),
            (false, true, true, false),
        ] {
            assert_eq!(
                card_hidden(closed, show_handled, draft_open_here),
                expected,
                "closed={closed} show_handled={show_handled} draft_open_here={draft_open_here}"
            );
        }
    }

    /// `closed` (together with whether a draft is open for the card) is what
    /// `card_hidden` decides on. Exercising `closed` here, with no draft open,
    /// covers "hidden unless the show-handled checkbox is on" without needing
    /// to render native widgets; `card_hidden`
    /// itself (including the open-draft override) is covered separately by
    /// its own table test above.
    #[test]
    fn resolved_item_without_a_decision_is_closed_and_hides_unless_shown() {
        let (state, mut item) = aging_fixture();
        item.resolution = Some(item.evidence.clone());
        item.resolution_kind = Some(ResolutionKind::Completed);
        let closed = state.card_context(&item, 0, 0).unwrap().closed;
        assert!(closed, "resolved item must be closed by default");
        for show_handled in [false, true] {
            let hidden = card_hidden(closed, show_handled, false);
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
            message_handle: "m0".into(),
            from_subject: false,
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

    fn empty_scan_result() -> ScanResult {
        ScanResult {
            analysis: Expectations {
                items: vec![],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            analyzed: 0,
            total: 0,
            cancelled: false,
            conversation_notes: vec![],
            cross_thread_closures: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            closure_pass_failure: None,
        }
    }

    #[test]
    fn set_scan_clears_an_open_draft() {
        // A rescan rebuilds `analysis`/`messages` from scratch; an open
        // draft refers to a card that may no longer exist, so it must not
        // survive the rescan orphaned.
        let mut state = ReviewState {
            draft: Some(ReminderDraft {
                key: [7; 32],
                account: "acct".into(),
                title: "Send the draft".into(),
                when: "2026-09-08 09:00".into(),
                error: String::new(),
                prior_decision: Decision::Review,
            }),
            ..ReviewState::default()
        };
        state.set_scan(empty_scan_result(), "model".into());
        assert!(state.draft.is_none());
    }

    #[test]
    fn set_scan_reports_a_cancelled_scan_as_incomplete_even_when_analyzed_equals_total() {
        // `analyzed == total` here (both 0, via `empty_scan_result`): a
        // cancellation that only cut short the closure pass (which does
        // not count toward `analyzed`/`total`) must still be visible.
        let mut state = ReviewState::default();
        let mut result = empty_scan_result();
        result.cancelled = true;
        state.set_scan(result, "model".into());
        assert!(state.scan_incomplete);
        assert!(state.scan_summary.contains("stopped by you"));
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
                closure_pass_failure: Some("Closure pass stopped: rate limited".into()),
            },
            "model".into(),
        );
        assert!(state.scan_incomplete);
        assert!(
            state
                .scan_errors
                .iter()
                .any(|e| e == "Closure pass stopped: rate limited")
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

    #[test]
    fn list_group_matches_the_spec_table_and_closed_always_wins() {
        for closed in [false, true] {
            let card = |deadline: Option<DeadlineView>| CardContext {
                record: Record {
                    key: [0; 32],
                    decision: Decision::Review,
                    reminder: Reminder::None,
                    updated: 0,
                },
                terminal: false,
                closed,
                deadline,
            };
            let expect = |deadline: Option<DeadlineView>, expected: ListGroup| {
                let group = list_group(&card(deadline));
                if closed {
                    assert_eq!(group, ListGroup::Closed);
                } else {
                    assert_eq!(group, expected);
                }
            };
            expect(None, ListGroup::NoFixedDeadline);
            expect(Some(DeadlineView::EventTied), ListGroup::NoFixedDeadline);
            expect(Some(DeadlineView::Soft), ListGroup::NoFixedDeadline);
            expect(Some(DeadlineView::Unknown), ListGroup::NoFixedDeadline);
            expect(
                Some(DeadlineView::PastDue {
                    boundary: 0,
                    offset_seconds: 0,
                }),
                ListGroup::PastDue,
            );
            expect(
                Some(DeadlineView::Due {
                    boundary: 0,
                    offset_seconds: 0,
                }),
                ListGroup::Due,
            );
            for past in [false, true] {
                expect(
                    Some(DeadlineView::DueDate {
                        day: 0,
                        boundary: 0,
                        past,
                    }),
                    if past {
                        ListGroup::PastDue
                    } else {
                        ListGroup::Due
                    },
                );
                expect(
                    Some(DeadlineView::DueBusinessDay {
                        day: 0,
                        boundary: 0,
                        past,
                    }),
                    if past {
                        ListGroup::PastDue
                    } else {
                        ListGroup::Due
                    },
                );
                expect(
                    Some(DeadlineView::DueRange {
                        start_day: 0,
                        end_day: 0,
                        boundary: 0,
                        past,
                    }),
                    if past {
                        ListGroup::PastDue
                    } else {
                        ListGroup::Due
                    },
                );
            }
        }
    }

    #[test]
    fn filter_matches_every_owner_combination() {
        // `Owner` (from `openloops_inference`) does not implement `Debug`,
        // so the failure message names each case by hand instead of `{:?}`.
        for (filter, owner, owner_name, expected) in [
            (Filter::All, Owner::You, "You", true),
            (Filter::All, Owner::Team, "Team", true),
            (Filter::All, Owner::Unclear, "Unclear", true),
            (Filter::Mine, Owner::You, "You", true),
            (Filter::Mine, Owner::Team, "Team", false),
            (Filter::Mine, Owner::Unclear, "Unclear", false),
            (Filter::Team, Owner::You, "You", false),
            (Filter::Team, Owner::Team, "Team", true),
            (Filter::Team, Owner::Unclear, "Unclear", false),
        ] {
            assert_eq!(
                filter.matches(owner),
                expected,
                "filter={filter:?} owner={owner_name}"
            );
        }
    }

    fn card_with(decision: Decision, closed: bool) -> CardContext {
        CardContext {
            record: Record {
                key: [0; 32],
                decision,
                reminder: Reminder::None,
                updated: 0,
            },
            terminal: matches!(
                decision,
                Decision::Done | Decision::Dismissed | Decision::Moot
            ),
            closed,
            deadline: None,
        }
    }

    #[test]
    fn open_badge_count_counts_untracked_and_tracked_but_not_closed_or_terminal() {
        assert_eq!(open_badge_count(&[]), 0);
        let cards = vec![
            None,
            Some(card_with(Decision::Review, true)),
            Some(card_with(Decision::Review, false)),
            Some(card_with(Decision::Mine, false)),
            Some(card_with(Decision::Watching, false)),
            Some(card_with(Decision::Done, true)),
            Some(card_with(Decision::Dismissed, true)),
            Some(card_with(Decision::Moot, true)),
        ];
        assert_eq!(open_badge_count(&cards), 3);
    }

    #[test]
    fn scan_strip_is_idle_with_no_progress_and_no_analysis() {
        let review = ReviewState::default();
        assert_eq!(scan_strip(None, &review), ScanStrip::Idle);
    }

    #[test]
    fn scan_strip_reports_finished_with_coverage_notes() {
        for cancelled in [false, true] {
            let mut review = ReviewState::default();
            review.notices.push("Inbox: 3 messages".into());
            let mut result = empty_scan_result();
            result.cancelled = cancelled;
            review.set_scan(result, "model".into());
            match scan_strip(None, &review) {
                ScanStrip::Finished {
                    incomplete,
                    coverage_notes,
                    ..
                } => {
                    assert_eq!(incomplete, cancelled);
                    assert!(coverage_notes.contains(&"Inbox: 3 messages".to_string()));
                }
                other => panic!("expected Finished, got {other:?}"),
            }
        }
    }

    #[test]
    fn scan_strip_reports_scanning_progress_and_phase() {
        let progress = ScanProgress::default();
        progress.processed.store(4, Ordering::Relaxed);
        progress.total.store(10, Ordering::Relaxed);
        progress.conversation_total.store(7, Ordering::Relaxed);
        let review = ReviewState::default();
        match scan_strip(Some(&progress), &review) {
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
                assert_eq!(phase, "Finding open loops");
                assert_eq!(conversation_index, 0);
                assert_eq!(conversation_total, 7);
                assert_eq!(processed, 4);
                assert_eq!(total, 10);
                assert_eq!(elapsed_secs, 0);
                assert_eq!(percent, 40);
                assert!(!stopping);
            }
            other => panic!("expected Scanning, got {other:?}"),
        }
        progress.closure_phase.store(true, Ordering::Relaxed);
        match scan_strip(Some(&progress), &review) {
            ScanStrip::Scanning { phase, .. } => {
                assert_eq!(phase, "Cross-thread closure check");
            }
            other => panic!("expected Scanning, got {other:?}"),
        }
    }

    #[test]
    fn scan_strip_percent_guards_against_divide_by_zero() {
        let progress = ScanProgress::default();
        progress.processed.store(0, Ordering::Relaxed);
        progress.total.store(0, Ordering::Relaxed);
        let review = ReviewState::default();
        match scan_strip(Some(&progress), &review) {
            ScanStrip::Scanning { percent, .. } => assert_eq!(percent, 0),
            other => panic!("expected Scanning, got {other:?}"),
        }
    }

    #[test]
    fn sender_label_falls_back_to_other_participant_without_a_sender_block() {
        let mail = openloops_graph::live::review::MailItem {
            id: "no-sender".into(),
            account: "synthetic".into(),
            received: "2026-09-06T12:00:00Z".into(),
            sender: String::new(),
            sender_address: String::new(),
            own_addresses: vec!["user@example.invalid".into()],
            body: "Body text.".into(),
            ..Default::default()
        };
        let message = scanning::prepare(&mail, "Inbox", 0).unwrap();
        assert!(!message.input.from_user);
        assert!(message.input.message.sender.is_none());
        assert_eq!(
            crate::slint_review::sender_label(&message),
            "Other participant"
        );
    }

    #[test]
    fn show_handled_label_matches_the_review_spec() {
        assert_eq!(SHOW_HANDLED_LABEL, "Show resolved, handled and dismissed");
    }
}
