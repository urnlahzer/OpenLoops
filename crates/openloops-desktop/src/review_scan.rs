use crate::deadline_view::{DeadlineView, classify};
use chrono::TimeZone;
use openloops_graph::live::{ConnectionError, review::MailItem};
use openloops_inference::{
    blocks::CanonicalBlock,
    canonical::canonicalize_plain,
    message::CanonicalMessage,
    ollama::{
        OllamaCloud, ProviderError,
        expectations::{
            Anchor, ConversationMessage, EventPassed, Expectation, Expectations, Owner,
            ResolutionKind,
        },
    },
    reply_history::{
        REPLY_HISTORY_CHUNK_MAX_CHARS, chunk_reply_history, is_underscore_separator,
        starts_with_ascii_ci,
    },
    walker::canonicalize_html,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Clone)]
pub struct ReviewMessage {
    pub input: ConversationMessage,
    pub source: String,
    pub id: String,
    pub account: String,
    pub conversation: String,
    pub date_label: String,
    pub web_link: String,
    /// Lowercase addresses of sender, to, and cc, minus the account's own
    /// addresses, deduplicated and sorted. Used to link conversations that
    /// Exchange split into different `conversationId`s but that share
    /// participants -- see [`merge_threads`].
    pub other_addresses: Vec<String>,
    /// `(start, end, out_of_date)`, in UTC seconds, from either Graph
    /// meeting-message metadata or a calendar-invite subject line.
    pub event: Option<(i64, i64, bool)>,
}

pub struct EventRef {
    pub name: String,
    pub start: i64,
    pub end: i64,
    pub message_handle: String,
    /// True when this entry came from Graph meeting-message metadata rather
    /// than a calendar-invite subject line -- used only for the coverage
    /// diagnostic in [`close_passed_events`].
    pub from_meeting: bool,
}

/// Parses a calendar time embedded in a subject. Unknown or absent timezone
/// abbreviations use UTC because a UTC message timestamp does not imply a
/// reliable local offset without a timezone database.
pub fn subject_event_time(subject: &str, _message_timestamp: i64) -> Option<(i64, i64)> {
    let calendar = subject.rsplit_once(" @ ")?.1.trim();
    let (calendar, offset) = strip_organizer_and_timezone_suffix(calendar);
    let parts = join_spaced_am_pm(calendar.split_whitespace());
    if parts.len() < 4 {
        return None;
    }
    let date_text = parts[..4].join(" ");
    let date = chrono::NaiveDate::parse_from_str(&date_text, "%a %b %d, %Y").ok()?;
    let local_midnight = date.and_hms_opt(0, 0, 0)?.and_utc().timestamp();
    if parts.len() == 4 {
        return Some((
            local_midnight - i64::from(offset),
            local_midnight + 86_400 - i64::from(offset),
        ));
    }
    if parts.len() != 7 || parts[5] != "-" {
        return None;
    }
    let start = parse_subject_clock(&parts[4])?;
    let mut end = parse_subject_clock(&parts[6])?;
    if end <= start {
        end += 86_400;
    }
    Some((
        local_midnight + i64::from(start - offset),
        local_midnight + i64::from(end - offset),
    ))
}

/// Repeatedly peels a trailing `(...)` group off `calendar` (e.g. an
/// organizer name appended after the timezone abbreviation, "... (CDT) (Sam
/// Rivera)"), using the LAST peeled group [`timezone_offset`] recognizes as
/// the zone and stopping there; a group it does not recognize is discarded
/// as decoration and peeling continues. Bounded to a handful of groups so a
/// pathological subject cannot loop. Returns the calendar text with every
/// peeled group removed and the resolved offset (0/UTC when none is found).
fn strip_organizer_and_timezone_suffix(calendar: &str) -> (String, i32) {
    let mut calendar = calendar.trim_end().to_string();
    let mut offset = 0i32;
    for _ in 0..5 {
        if !calendar.ends_with(')') {
            break;
        }
        let Some(open) = calendar.rfind('(') else {
            break;
        };
        let content = calendar[open + 1..calendar.len() - 1].trim().to_string();
        calendar.truncate(open);
        calendar = calendar.trim_end().to_string();
        if let Some(found) = timezone_offset(&content) {
            offset = found;
            break;
        }
    }
    (calendar, offset)
}

/// Joins a bare "am"/"pm" token to its predecessor (e.g. "11:30", "am" ->
/// "11:30am"), so a subject with a space before the meridiem still parses
/// like the more common unspaced form.
fn join_spaced_am_pm<'a>(tokens: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut joined: Vec<String> = Vec::new();
    for token in tokens {
        let lower = token.to_ascii_lowercase();
        if (lower == "am" || lower == "pm") && !joined.is_empty() {
            joined.last_mut().expect("checked above").push_str(token);
        } else {
            joined.push(token.to_string());
        }
    }
    joined
}

fn timezone_offset(abbreviation: &str) -> Option<i32> {
    Some(match abbreviation {
        "PDT" | "MST" => -7 * 3600,
        "PST" => -8 * 3600,
        "MDT" | "CST" => -6 * 3600,
        "CDT" | "EST" => -5 * 3600,
        "EDT" => -4 * 3600,
        "UTC" | "GMT" => 0,
        "BST" | "CET" => 3600,
        "CEST" => 2 * 3600,
        _ => return None,
    })
}

fn parse_subject_clock(value: &str) -> Option<i32> {
    let lower = value.to_ascii_lowercase();
    let (clock, pm) = if let Some(clock) = lower.strip_suffix("am") {
        (clock, false)
    } else {
        (lower.strip_suffix("pm")?, true)
    };
    let mut parts = clock.split(':');
    let hour: i32 = parts.next()?.parse().ok()?;
    let minute: i32 = parts.next().map_or(Some(0), |v| v.parse().ok())?;
    if parts.next().is_some() || !(1..=12).contains(&hour) || !(0..60).contains(&minute) {
        return None;
    }
    Some(((hour % 12) + if pm { 12 } else { 0 }) * 3600 + minute * 60)
}

/// Builds the event index from Graph meeting-message metadata and
/// calendar-invite subject lines. An out-of-date entry (a superseded
/// meeting-message revision) is dropped outright, never learned. When
/// multiple messages name the same normalized event (a reschedule, or the
/// same invite echoed by more than one message), only the entry with the
/// LATEST start survives -- a stale "Aug 21" instance of "design workshop"
/// must not shadow a later "Sep 15" reschedule of the same meeting.
pub fn build_event_index(messages: &[ReviewMessage]) -> Vec<EventRef> {
    let mut best: BTreeMap<String, EventRef> = BTreeMap::new();
    for message in messages {
        let subject = message.input.message.subject.as_string();
        let (start, end, out_of_date, from_meeting) = match message.event {
            Some((start, end, out_of_date)) => (start, end, out_of_date, true),
            None => match subject_event_time(&subject, message.input.timestamp) {
                Some((start, end)) => (start, end, false, false),
                None => continue,
            },
        };
        if out_of_date {
            continue;
        }
        let name = normalize_subject(&subject);
        if name.is_empty() {
            continue;
        }
        let entry = best.entry(name.clone()).or_insert(EventRef {
            name,
            start: i64::MIN,
            end: 0,
            message_handle: String::new(),
            from_meeting: false,
        });
        if start > entry.start {
            entry.start = start;
            entry.end = end;
            entry.message_handle.clone_from(&message.input.handle);
            entry.from_meeting = from_meeting;
        }
    }
    best.into_values().collect()
}

/// Stop words dropped from both the phrase and the event name before
/// computing token overlap: too common to distinguish one event from
/// another.
const EVENT_MATCH_STOP_WORDS: &[&str] = &[
    "the", "our", "a", "an", "this", "that", "with", "for", "of", "on", "at", "and",
];

/// Generic event nouns dropped the same way: "workshop", "call", etc. name
/// the KIND of gathering, not which one, so they must not by themselves
/// count as an overlap between an unrelated phrase and an event name that
/// happen to share only this word.
const EVENT_MATCH_GENERIC_NOUNS: &[&str] = &[
    "call",
    "meeting",
    "event",
    "session",
    "review",
    "workshop",
    "hearing",
    "sync",
    "deposition",
    "closing",
];

/// Lowercased, alphanumeric-tokenized `text` with stop words and generic
/// event nouns removed.
fn meaningful_event_tokens(text: &str) -> BTreeSet<String> {
    let lower = text.to_lowercase();
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .filter(|token| {
            !EVENT_MATCH_STOP_WORDS.contains(token) && !EVENT_MATCH_GENERIC_NOUNS.contains(token)
        })
        .map(str::to_string)
        .collect()
}

/// Matches a phrase naming an event (e.g. an expectation's `event` anchor
/// quote) against the learned event index. Never substring matching: both
/// sides are tokenized and reduced to their meaningful (non-stop-word,
/// non-generic-noun) tokens first. A phrase with no meaningful tokens at all
/// (e.g. "the call") matches nothing. Otherwise at least one meaningful
/// token must be shared; when the event name itself has 2 or more
/// meaningful tokens, that alone is not enough -- either 2 tokens must be
/// shared, or every one of the phrase's meaningful tokens must appear in
/// the name (so a short, specific phrase like "our quarterly workshop" can
/// still identify a longer name it is a strict subset of).
pub fn match_event<'a>(
    phrase: &str,
    evidence_timestamp: i64,
    index: &'a [EventRef],
) -> Option<&'a EventRef> {
    let phrase_tokens = meaningful_event_tokens(phrase);
    if phrase_tokens.is_empty() {
        return None;
    }
    index
        .iter()
        .filter(|event| {
            event.start >= evidence_timestamp && event.start - evidence_timestamp <= 60 * 86_400
        })
        .filter(|event| {
            let name_tokens = meaningful_event_tokens(&event.name);
            let shared = phrase_tokens.intersection(&name_tokens).count();
            if shared == 0 {
                return false;
            }
            name_tokens.len() < 2 || shared >= 2 || phrase_tokens.is_subset(&name_tokens)
        })
        .min_by_key(|event| (event.start - evidence_timestamp, &event.message_handle))
}

fn mail_event_time(item: &MailItem) -> Option<(i64, i64, bool)> {
    let event = item.event.as_ref()?;
    let start = chrono::DateTime::parse_from_rfc3339(&event.start)
        .ok()?
        .timestamp();
    let end = chrono::DateTime::parse_from_rfc3339(&event.end)
        .ok()?
        .timestamp();
    Some((start, end, event.out_of_date))
}

/// The message's own local UTC offset at its timestamp, falling back to
/// `fallback` when that instant cannot be resolved to a local time (the
/// same rule `ReviewState::card_context` uses to age a deadline against the
/// message that stated it, shared here so `close_passed_events` ages event
/// times the same way).
pub fn local_offset_seconds(timestamp: i64, fallback: i32) -> i32 {
    chrono::Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map_or(fallback, |t| t.offset().local_minus_utc())
}
#[derive(Default)]
pub struct ScanProgress {
    pub processed: AtomicUsize,
    pub total: AtomicUsize,
    pub cancel: AtomicBool,
}
pub struct ScanResult {
    pub analysis: Expectations,
    pub failures: Vec<String>,
    pub analyzed: usize,
    pub total: usize,
    pub cancelled: bool,
    /// Diagnostics for display in the "Scan coverage and errors" panel.
    /// Most entries are per-conversation: one note per conversation that had
    /// a rejection, degraded item, or (for a two-party thread) returned no
    /// expectations at all. A few are aggregate, scan-wide notes pushed by
    /// the cross-thread closure pass (`scan_closures`) rather than tied to
    /// any single conversation -- how many expectations it closed, and
    /// whether its per-scan call cap bound. In-memory UI text only -- built
    /// from message subjects, so it must never be logged, saved, or emitted
    /// by [`probe`].
    pub conversation_notes: Vec<String>,
    /// Number of open requests resolved by the cross-thread closure pass
    /// (`scan_closures`), using evidence found in a different conversation
    /// than the request itself.
    pub cross_thread_closures: usize,
    pub event_closures: usize,
    /// Set when `scan_conversations` stopped early because a conversation's
    /// analysis failed with a transport-class provider error (the same
    /// `matches!` set that stops the main scan). When true, `scan_closures`
    /// skips the cross-thread closure pass entirely instead of repeating
    /// calls against a provider already known to be unreachable or
    /// unauthorized.
    pub primary_scan_transport_error: bool,
    /// Content-free diagnostic set when the cross-thread closure pass itself
    /// stopped early on a transport-class provider error, e.g.
    /// `"Cross-thread closure pass stopped: rate limited"`. Kept separate
    /// from `failures` (which counts unanalyzed conversations) since a
    /// closure-pass failure does not mean any conversation went unanalyzed.
    pub closure_pass_failure: Option<String>,
}

/// Extracts a lowercase email address from a participant label such as
/// `Alex <alex@example.invalid>` or a bare address: the token inside angle
/// brackets when present, otherwise the first whitespace-delimited token
/// containing `@`. Returns `None` when neither form is found — for example
/// the `"Not established"` placeholder `candidate()` uses when the model
/// supplied no waiting party.
pub fn waiting_party_address(label: &str) -> Option<String> {
    if let Some(start) = label.find('<')
        && let Some(rel_end) = label[start + 1..].find('>')
    {
        let inner = &label[start + 1..start + 1 + rel_end];
        if !inner.is_empty() {
            return Some(inner.to_lowercase());
        }
    }
    label
        .split_whitespace()
        .find(|token| token.contains('@'))
        .map(str::to_lowercase)
}

/// Candidate messages for the cross-thread closure pass: later messages
/// the signed-in user sent to `item`'s waiting party, in a conversation
/// other than `evidence_conversation`, for `account` -- the account the
/// caller (`scan_closures`) already resolved for the evidence message,
/// passed in directly rather than re-derived here from
/// `item.evidence.message`. Keeps the NEWEST 8 such messages (sorted
/// descending by timestamp, truncated, then re-sorted ascending) so the
/// closure prompt stays small while favoring the most recent, most
/// probative evidence over stale older messages.
pub fn closure_candidates<'a>(
    item: &Expectation,
    all: &'a [ReviewMessage],
    account: &str,
    evidence_conversation: &str,
    evidence_timestamp: i64,
) -> Vec<&'a ReviewMessage> {
    let Some(address) = waiting_party_address(&item.waiting_party) else {
        return vec![];
    };
    let mut candidates: Vec<&ReviewMessage> = all
        .iter()
        .filter(|m| {
            m.account == account
                && m.input.from_user
                && m.conversation != evidence_conversation
                && m.input.timestamp > evidence_timestamp
                && m.other_addresses.contains(&address)
        })
        .collect();
    candidates.sort_by_key(|m| std::cmp::Reverse(m.input.timestamp));
    candidates.truncate(8);
    candidates.sort_by_key(|m| m.input.timestamp);
    candidates
}

fn block(text: &str) -> Result<CanonicalBlock, ConnectionError> {
    let text = canonicalize_plain(text).map_err(|_| ConnectionError::ResourceUnavailable)?;
    if text.chars().count() > 8192 {
        return Err(ConnectionError::ResponseTooLarge);
    }
    CanonicalBlock::new(&text).map_err(|_| ConnectionError::ResponseTooLarge)
}
/// Plain-text analog of `reply_history`'s HTML-path detection, at line
/// granularity: `From:` detection is case-insensitive (via
/// [`starts_with_ascii_ci`]) and requires both a `Subject:` line and a
/// `Sent:`/`Date:` line within the following 5 lines; the underscore rule
/// is the shared [`is_underscore_separator`]; `-----Original Message-----`
/// is matched on the trimmed line, not merely contained within it. The
/// window sizes deliberately differ from the HTML path's (paragraph-block
/// granularity there vs. line granularity here), so this does not call
/// `reply_history::is_reply_history_start` directly.
///
/// Mirrors `reply_history::split_reply_history`'s "match at paragraph 0"
/// guard: if the very first line already looks like a reply-history
/// header, with nothing else preceding it, the whole input is returned as
/// `body` with an empty `quote`, rather than emptying the message into an
/// all-quote message. This is narrower than "body ended up empty": a
/// message consisting only of `>`-quoted lines (with no history marker at
/// all, or one that starts later than line 0) also ends up with an empty
/// `body`, and that must still go to `quote` as before -- `history_start`
/// tracks the line index where the reply-history marker itself was found
/// (not the unrelated `>`-prefix quoting rule), so the guard only fires
/// when that marker was the very first line.
fn plain_body(text: &str) -> (String, String) {
    let lines: Vec<&str> = text.lines().collect();
    let mut body = String::new();
    let mut quote = String::new();
    let mut history = false;
    let mut history_start: Option<usize> = None;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if !history
            && (trimmed.starts_with("On ") && trimmed.ends_with("wrote:")
                // Exact match on the trimmed line, not `contains`:
                // narrowing this deliberately avoids treating prose that
                // merely mentions the phrase mid-sentence as reply
                // history.
                || trimmed == "-----Original Message-----")
        {
            history = true;
            history_start = Some(idx);
        }
        let leading = line.trim_start();
        // Outlook plain-text reply: a From: line with both a Subject: line
        // and a Sent:/Date: line within the next 5 lines starts the
        // quoted original.
        if !history && starts_with_ascii_ci(leading, "From:") {
            let end = (idx + 6).min(lines.len());
            let window = &lines[idx + 1..end];
            let has_subject = window
                .iter()
                .any(|l| starts_with_ascii_ci(l.trim_start(), "Subject:"));
            let has_sent_or_date = window.iter().any(|l| {
                let t = l.trim_start();
                starts_with_ascii_ci(t, "Sent:") || starts_with_ascii_ci(t, "Date:")
            });
            if has_subject && has_sent_or_date {
                history = true;
                history_start = Some(idx);
            }
        }
        // Outlook's underscore separator, immediately followed (within 2
        // lines) by a From: line, also starts the quoted original.
        if !history && is_underscore_separator(trimmed) {
            let end = (idx + 3).min(lines.len());
            if lines[idx + 1..end]
                .iter()
                .any(|l| starts_with_ascii_ci(l.trim_start(), "From:"))
            {
                history = true;
                history_start = Some(idx);
            }
        }
        let target = if history || line.trim_start().starts_with('>') {
            &mut quote
        } else {
            &mut body
        };
        target.push_str(line);
        target.push('\n');
    }
    if history_start == Some(0) {
        return (text.to_string(), String::new());
    }
    (body, quote)
}

/// Splits `paragraph` into line-grouped pieces of at most
/// `REPLY_HISTORY_CHUNK_MAX_CHARS` characters each, if it exceeds that
/// limit; otherwise returns it unchanged as the only element. Plain-text
/// quoted history sometimes has no blank lines at all (one giant
/// hard-wrapped paragraph), which would otherwise make
/// `chunk_reply_history` -- which only ever splits on paragraph boundaries
/// -- powerless to bound it: with only one paragraph, there is nothing to
/// split on. Grouping consecutive lines into sub-paragraphs first restores
/// a splittable boundary every `REPLY_HISTORY_CHUNK_MAX_CHARS` characters
/// or so. A single line longer than the limit still becomes its own
/// oversized piece rather than being cut mid-line.
fn split_oversized_paragraph_by_lines(paragraph: &str) -> Vec<String> {
    if paragraph.chars().count() <= REPLY_HISTORY_CHUNK_MAX_CHARS {
        return vec![paragraph.to_string()];
    }
    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    for line in paragraph.lines() {
        let line_len = line.chars().count();
        let separator_len = usize::from(!current.is_empty());
        if !current.is_empty()
            && current_len + separator_len + line_len > REPLY_HISTORY_CHUNK_MAX_CHARS
        {
            pieces.push(std::mem::take(&mut current));
            current_len = 0;
        }
        if !current.is_empty() {
            current.push('\n');
            current_len += 1;
        }
        current.push_str(line);
        current_len += line_len;
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

pub fn prepare(
    item: &MailItem,
    source: &str,
    index: usize,
) -> Result<ReviewMessage, ConnectionError> {
    let (body_blocks, quote_blocks, link_labels) = if item.body_is_html {
        let walked =
            canonicalize_html(&item.body).map_err(|_| ConnectionError::ResourceUnavailable)?;
        (
            walked
                .body_blocks
                .iter()
                .map(|s| block(s))
                .collect::<Result<Vec<_>, _>>()?,
            walked
                .quote_blocks
                .iter()
                .map(|s| block(s))
                .collect::<Result<Vec<_>, _>>()?,
            walked
                .link_labels
                .iter()
                .map(|s| block(s))
                .collect::<Result<Vec<_>, _>>()?,
        )
    } else {
        let (body, quote) = plain_body(&item.body);
        let trimmed_body = body.trim();
        let body_blocks = if trimmed_body.is_empty() {
            vec![]
        } else {
            vec![block(trimmed_body)?]
        };
        // Bound the quote text the same way the HTML path bounds detected
        // reply history: split it into blank-line-separated paragraphs
        // (further splitting by line any paragraph that is itself over
        // the chunk limit -- see `split_oversized_paragraph_by_lines`) and
        // chunk them, instead of pushing one unbounded quote block.
        let quote_paragraphs: Vec<String> = quote
            .split("\n\n")
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .flat_map(split_oversized_paragraph_by_lines)
            .collect();
        let quote_blocks = chunk_reply_history(quote_paragraphs)
            .iter()
            .map(|s| block(s))
            .collect::<Result<Vec<_>, _>>()?;
        (body_blocks, quote_blocks, vec![])
    };
    if 1 + body_blocks.len() + quote_blocks.len() > 64 {
        return Err(ConnectionError::ResponseTooLarge);
    }
    let timestamp = chrono::DateTime::parse_from_rfc3339(&item.received)
        .map_err(|_| ConnectionError::ResourceUnavailable)?
        .timestamp();
    let from_user = item
        .own_addresses
        .iter()
        .any(|a| a.eq_ignore_ascii_case(&item.sender_address));
    let to_user = item.to.iter().any(|a| {
        item.own_addresses
            .iter()
            .any(|own| a.eq_ignore_ascii_case(own))
    });
    let other_addresses = other_addresses(item);
    Ok(ReviewMessage {
        input: ConversationMessage {
            handle: format!("m{index}"),
            timestamp,
            from_user,
            to_user,
            team: item.team,
            message: CanonicalMessage {
                subject: block(&item.subject)?,
                body_blocks,
                quote_blocks,
                sender: if item.sender.is_empty() {
                    None
                } else {
                    Some(block(&item.sender)?)
                },
                to: item
                    .to
                    .iter()
                    .map(|s| block(s))
                    .collect::<Result<Vec<_>, _>>()?,
                cc: item
                    .cc
                    .iter()
                    .map(|s| block(s))
                    .collect::<Result<Vec<_>, _>>()?,
                attachment_names: vec![],
                link_labels,
            },
        },
        source: source.into(),
        id: item.id.clone(),
        account: item.account.clone(),
        conversation: item.conversation.clone(),
        date_label: chrono::DateTime::from_timestamp(timestamp, 0)
            .ok_or(ConnectionError::ResourceUnavailable)?
            .with_timezone(&chrono::Local)
            .format("%b %d, %Y %H:%M %:z")
            .to_string(),
        web_link: item.web_link.clone(),
        other_addresses,
        event: mail_event_time(item),
    })
}

/// Lowercase addresses of sender, to, and cc, minus `item.own_addresses`,
/// deduplicated and sorted.
fn other_addresses(item: &MailItem) -> Vec<String> {
    let own: BTreeSet<String> = item
        .own_addresses
        .iter()
        .map(|a| a.to_lowercase())
        .collect();
    let mut addresses: BTreeSet<String> = BTreeSet::new();
    if !item.sender_address.is_empty() {
        addresses.insert(item.sender_address.to_lowercase());
    }
    for a in item.to.iter().chain(item.cc.iter()) {
        addresses.insert(a.to_lowercase());
    }
    addresses.retain(|a| !own.contains(a));
    addresses.into_iter().collect()
}

pub fn scan(
    key: String,
    model: &str,
    messages: &[ReviewMessage],
    progress: &ScanProgress,
) -> Result<ScanResult, ProviderError> {
    progress.total.store(messages.len(), Ordering::Relaxed);
    let provider = OllamaCloud::connect(key, model)?;
    let mut result = scan_conversations(messages, progress, |conversation| {
        provider.expectations(conversation)
    });
    close_passed_events(&mut result, messages, chrono::Utc::now().timestamp());
    scan_closures(
        messages,
        progress,
        &mut result,
        |item, evidence_timestamp, candidates| {
            provider.closure(item, evidence_timestamp, candidates)
        },
    );
    Ok(result)
}
/// Builds the one-line diagnostic note for a successfully analyzed
/// conversation (`index` is 0-based; the note is 1-based), or `None` when
/// the conversation needs no attention. `conversation` must already be
/// sorted chronologically (as `scan_conversations` sorts it before
/// analyzing), since the subject snippet is drawn from the first message.
///
/// Two cases produce a note:
/// - the conversation had a rejection, a degraded (unverified-evidence)
///   item, or any rejection reason at all -- reporting accepted/rejected/
///   degraded counts plus the conversation's own (deduplicated) reasons;
/// - the conversation returned zero items and zero rejections, but is a
///   genuinely two-party thread (at least one message from the signed-in
///   user and at least one not, every message personal mail rather than a
///   group source, no message with more than one `to` recipient or any
///   `cc`, and the union of every message's `other_addresses` across the
///   whole conversation has exactly one member), so a card silently
///   vanishing from that thread can be told apart from a newsletter, group
///   source, or other multi-party mail that never had a single owed action
///   to find. The address-union check catches what the per-message `to`/
///   `cc` checks alone cannot: a "two-party" conversation stitched together
///   from messages to or from different counterparties is not actually
///   two-party.
///
/// Text only, built from in-memory subjects: never call this from `probe`,
/// and never persist or log its output.
fn conversation_note(
    index: usize,
    conversation: &[&ReviewMessage],
    analysis: &Expectations,
) -> Option<String> {
    let len = conversation.len();
    let subject = conversation
        .first()
        .map(|m| m.input.message.subject.as_string())
        .unwrap_or_default();
    let snippet: String = subject.chars().take(60).collect();
    if analysis.rejected > 0 || analysis.degraded > 0 || !analysis.rejection_reasons.is_empty() {
        let mut seen: Vec<&'static str> = Vec::new();
        for reason in &analysis.rejection_reasons {
            if !seen.contains(reason) {
                seen.push(*reason);
            }
        }
        return Some(format!(
            "Conversation {} ({len} messages; subject: {snippet}): {} accepted, {} rejected, {} kept with unverified evidence. {}",
            index + 1,
            analysis.items.len(),
            analysis.rejected,
            analysis.degraded,
            seen.join(" / "),
        ));
    }
    let mut counterparties: BTreeSet<&str> = BTreeSet::new();
    for m in conversation {
        counterparties.extend(m.other_addresses.iter().map(String::as_str));
    }
    if analysis.items.is_empty()
        && counterparties.len() == 1
        && conversation.iter().any(|m| !m.input.from_user)
        && conversation.iter().any(|m| m.input.from_user)
        && conversation.iter().all(|m| {
            !m.input.team && m.input.message.to.len() <= 1 && m.input.message.cc.is_empty()
        })
    {
        return Some(format!(
            "Conversation {} ({len} messages; subject: {snippet}): no expectations returned.",
            index + 1
        ));
    }
    None
}

fn scan_conversations(
    messages: &[ReviewMessage],
    progress: &ScanProgress,
    mut analyze: impl FnMut(&[ConversationMessage]) -> Result<Expectations, ProviderError>,
) -> ScanResult {
    let mut result = ScanResult {
        analysis: Expectations {
            items: vec![],
            rejected: 0,
            rejection_reasons: vec![],
            degraded: 0,
        },
        failures: vec![],
        analyzed: 0,
        total: messages.len(),
        cancelled: false,
        conversation_notes: vec![],
        cross_thread_closures: 0,
        event_closures: 0,
        primary_scan_transport_error: false,
        closure_pass_failure: None,
    };
    // Grouped as borrows rather than owned clones: each `ReviewMessage`
    // already lives in `messages`, and the per-conversation `inputs`
    // (below) is the only owned copy this pass actually needs.
    let mut conversations: BTreeMap<(&str, &str), Vec<&ReviewMessage>> = BTreeMap::new();
    for m in messages {
        conversations
            .entry((&m.account, &m.conversation))
            .or_default()
            .push(m);
    }
    for (index, mut conversation) in conversations.into_values().enumerate() {
        if progress.cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            break;
        }
        conversation.sort_by_key(|m| m.input.timestamp);
        let inputs: Vec<ConversationMessage> =
            conversation.iter().map(|m| m.input.clone()).collect();
        match analyze(&inputs) {
            Ok(analysis) => {
                result.analyzed += conversation.len();
                if let Some(note) = conversation_note(index, &conversation, &analysis) {
                    result.conversation_notes.push(note);
                }
                result.analysis.items.extend(analysis.items);
                result.analysis.rejected += analysis.rejected;
                result.analysis.degraded += analysis.degraded;
                result
                    .analysis
                    .rejection_reasons
                    .extend(analysis.rejection_reasons);
            }
            Err(e) => {
                result.failures.push(format!(
                    "Conversation {} ({} messages): {e}",
                    index + 1,
                    conversation.len()
                ));
                if matches!(
                    e,
                    ProviderError::Unauthorized
                        | ProviderError::RateLimited
                        | ProviderError::Quota
                        | ProviderError::Network
                        | ProviderError::Timeout
                        | ProviderError::ServerError(_)
                ) {
                    result.primary_scan_transport_error = true;
                    break;
                }
            }
        }
        progress
            .processed
            .fetch_add(conversation.len(), Ordering::Relaxed);
    }
    result
}

/// The instant a [`DeadlineView`] says has already passed, or `None` when it
/// is not a past-due variant (or its boundary could not be resolved). Shared
/// by [`close_passed_events`]'s `event_time` path, which needs the same
/// boundary a deadline card would age against, not just the past/future bit.
fn past_due_boundary(view: &DeadlineView) -> Option<i64> {
    match *view {
        DeadlineView::PastDue { boundary, .. } => Some(boundary),
        DeadlineView::DueDate { day, past: true }
        | DeadlineView::DueBusinessDay { day, past: true } => day.checked_mul(86_400),
        DeadlineView::DueRange {
            end_day,
            past: true,
            ..
        } => end_day.checked_mul(86_400),
        _ => None,
    }
}

/// Closes any open, unresolved expectation whose event has already ended,
/// via two independent sources of evidence:
///
/// - the learned event index (Graph meeting metadata and calendar-invite
///   subjects), matched against the expectation's `event` anchor or an
///   `EventTied` deadline phrase ([`match_event`]);
/// - a model-supplied `event_time` phrase -- a verbatim date/time anchor
///   stating when the named event occurs, found anywhere in the
///   conversation -- classified directly with [`classify`]. This path
///   closes a loop even when the index has no entry at all, which is the
///   only way to close one whose event date is stated only in an email
///   body rather than a meeting invite or calendar subject.
///
/// Always pushes one content-free coverage note (even when every count is
/// zero, so the chain from "events learned" to "expectations closed" stays
/// visible), unless `result.cancelled` -- a cancelled scan's counts would be
/// partial and therefore misleading.
pub fn close_passed_events(result: &mut ScanResult, messages: &[ReviewMessage], now: i64) {
    let index = build_event_index(messages);
    let mut named = 0usize;
    let mut timed = 0usize;
    let mut matched = 0usize;
    for item in &mut result.analysis.items {
        if item.resolution.is_some() || item.event_passed.is_some() {
            continue;
        }
        let Some(source) = messages
            .iter()
            .find(|message| message.input.handle == item.evidence.message)
        else {
            continue;
        };
        let offset = local_offset_seconds(source.input.timestamp, 0);
        let phrase = if let Some(event) = &item.event {
            Some(event.quote.as_str())
        } else if let Some(deadline) = &item.deadline
            && matches!(
                classify(&deadline.quote, source.input.timestamp, now, offset),
                DeadlineView::EventTied
            )
        {
            Some(deadline.quote.as_str())
        } else {
            None
        };
        named += usize::from(phrase.is_some());
        let event_match =
            phrase.and_then(|phrase| match_event(phrase, source.input.timestamp, &index));
        matched += usize::from(event_match.is_some());
        if let Some(event) = event_match
            && event.end < now
        {
            item.event_passed = Some(EventPassed {
                name: event.name.clone(),
                end: event.end,
                message_handle: event.message_handle.clone(),
            });
            result.event_closures += 1;
            continue;
        }
        let Some(event_time) = &item.event_time else {
            continue;
        };
        timed += 1;
        let Some(time_message) = messages
            .iter()
            .find(|m| m.input.handle == event_time.message)
        else {
            continue;
        };
        let offset = local_offset_seconds(time_message.input.timestamp, 0);
        let view = classify(&event_time.quote, time_message.input.timestamp, now, offset);
        if let Some(end) = past_due_boundary(&view) {
            item.event_passed = Some(EventPassed {
                name: item
                    .event
                    .as_ref()
                    .map_or_else(|| "event".to_string(), |e| e.quote.clone()),
                end,
                message_handle: event_time.message.clone(),
            });
            result.event_closures += 1;
        }
    }
    if result.cancelled {
        return;
    }
    let from_meetings = index.iter().filter(|event| event.from_meeting).count();
    let from_subjects = index.len() - from_meetings;
    result.conversation_notes.push(format!(
        "Event index: {} events learned ({from_meetings} from meeting messages, {from_subjects} from subjects); {named} expectations named an event, {timed} carried an event time, {matched} matched the index, {} closed.",
        index.len(),
        result.event_closures,
    ));
}

/// Hard cap on how many `closure()` provider calls one scan makes, however
/// many open requests are eligible: a large mailbox could otherwise turn
/// into dozens of extra model calls in a single scan.
const MAX_CLOSURE_CALLS: usize = 40;

/// After the primary per-conversation scan, attempts to close any
/// remaining open "you owe someone" requests using completion evidence the
/// signed-in user sent to the same waiting party in a DIFFERENT
/// conversation than the request — evidence `scan_conversations`'s
/// per-conversation analysis never sees. Mutates `result.analysis.items`
/// in place, sets `cross_thread: true` on every item it resolves, and
/// counts them in `result.cross_thread_closures`.
///
/// Skipped entirely when `result.primary_scan_transport_error` is set: the
/// provider is already known to be unreachable or unauthorized, so per-item
/// closure calls would fail identically. Before running, the count of
/// eligible items (open, `request`-kind, `You`-owned) is added to
/// `progress.total`, and `progress.processed` is incremented once per
/// eligible item as it is processed, including one skipped for having no
/// candidates, a shared-mailbox/list waiting party (see
/// [`is_shared_mailbox_address`]), the [`MAX_CLOSURE_CALLS`] cap binding, or
/// a non-transport-class provider error -- `scan_closures` only ever
/// `continue`s past those, exactly like `scan_conversations` does for the
/// same error classes. Only a transport-class provider error (the same
/// `matches!` set `scan_conversations` uses) stops the pass early, recorded
/// content-free in `result.closure_pass_failure` rather than `failures`
/// (which counts unanalyzed conversations -- a closure-pass failure never
/// leaves a conversation unanalyzed). `progress.cancel` is checked between
/// items exactly like `scan_conversations`.
fn scan_closures(
    messages: &[ReviewMessage],
    progress: &ScanProgress,
    result: &mut ScanResult,
    mut closure: impl FnMut(
        &Expectation,
        i64,
        &[ConversationMessage],
    ) -> Result<Option<(Anchor, ResolutionKind)>, ProviderError>,
) {
    if result.primary_scan_transport_error {
        return;
    }
    let eligible = result
        .analysis
        .items
        .iter()
        .filter(|item| {
            item.resolution.is_none()
                && item.event_passed.is_none()
                && item.kind == "request"
                && item.owner == Owner::You
        })
        .count();
    progress.total.fetch_add(eligible, Ordering::Relaxed);
    let (groups_per_account, address_group_counts) = conversation_group_address_counts(messages);
    let mut calls_made = 0usize;
    let mut capped = false;
    for item in &mut result.analysis.items {
        if progress.cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            break;
        }
        if item.resolution.is_some()
            || item.event_passed.is_some()
            || item.kind != "request"
            || item.owner != Owner::You
        {
            continue;
        }
        progress.processed.fetch_add(1, Ordering::Relaxed);
        let Some(source) = messages
            .iter()
            .find(|m| m.input.handle == item.evidence.message)
        else {
            continue;
        };
        let Some(address) = waiting_party_address(&item.waiting_party) else {
            continue;
        };
        if is_shared_mailbox_address(
            &source.account,
            &address,
            &groups_per_account,
            &address_group_counts,
        ) {
            continue;
        }
        let candidates = closure_candidates(
            item,
            messages,
            &source.account,
            &source.conversation,
            source.input.timestamp,
        );
        if candidates.is_empty() {
            continue;
        }
        if calls_made >= MAX_CLOSURE_CALLS {
            capped = true;
            continue;
        }
        let inputs: Vec<ConversationMessage> = candidates.iter().map(|m| m.input.clone()).collect();
        calls_made += 1;
        match closure(item, source.input.timestamp, &inputs) {
            Ok(Some((anchor, kind))) => {
                item.resolution = Some(anchor);
                item.resolution_kind = Some(kind);
                item.cross_thread = true;
                result.cross_thread_closures += 1;
            }
            Ok(None) => {}
            Err(e) => {
                if matches!(
                    e,
                    ProviderError::Unauthorized
                        | ProviderError::RateLimited
                        | ProviderError::Quota
                        | ProviderError::Network
                        | ProviderError::Timeout
                        | ProviderError::ServerError(_)
                ) {
                    result.closure_pass_failure =
                        Some(format!("Cross-thread closure pass stopped: {e}"));
                    break;
                }
            }
        }
    }
    if result.cross_thread_closures > 0 {
        result.conversation_notes.push(format!(
            "Closing evidence found in another conversation for {} expectation(s).",
            result.cross_thread_closures
        ));
    }
    if capped {
        result.conversation_notes.push(format!(
            "Cross-thread closure checks were capped at {MAX_CLOSURE_CALLS} open requests this scan."
        ));
    }
}

const WEEKDAY_OR_MONTH_NAMES: &[&str] = &[
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "mon",
    "tue",
    "wed",
    "thu",
    "fri",
    "sat",
    "sun",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "jan",
    "feb",
    "mar",
    "apr",
    "jun",
    "jul",
    "aug",
    "sep",
    "oct",
    "nov",
    "dec",
];

/// True when `text` begins with a weekday or month name (abbreviated or
/// full), followed by a word boundary (end of string or a non-alphanumeric
/// character) rather than continuing into an unrelated word. Returns the
/// matched name's byte length so callers can inspect what follows it.
fn strip_weekday_or_month_prefix(text: &str) -> Option<usize> {
    WEEKDAY_OR_MONTH_NAMES.iter().find_map(|name| {
        text.strip_prefix(name)
            .filter(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric()))
            .map(|_| name.len())
    })
}

/// True when `after` (the lowercased text following " @ ") looks like a
/// calendar date/time rather than ordinary subject text: it must start with
/// a weekday or month name, and -- with the pattern's trigger character
/// (the digit or space checked below) bounded to the first 12 characters
/// that follow that name -- what comes next must itself look date-shaped: a
/// comma immediately preceded by a digit (e.g. "Mon Sep 7, 2026 ..."), a
/// space directly followed by a digit (e.g. "Sep 7 2026", or "Monday,
/// September 7, 2026 ..." where the trailing space before the day number
/// sits right at the 12-character bound), or a digit within the first 4 of
/// those characters (e.g. "Dec.25" -- the day number follows a punctuation
/// boundary rather than the name itself, since
/// [`strip_weekday_or_month_prefix`] requires a non-alphanumeric character
/// immediately after the name; a bare "Dec25" never reaches this function at
/// all, since the digit run right after the name fails that boundary
/// check). The two-character patterns' confirming character (the comma or
/// digit) is still read one past the 12-character bound when the trigger
/// sits right at it, since the pattern would otherwise straddle the
/// boundary and go unrecognized. Bounding the window, and requiring a digit
/// right before any comma, keeps a bare place or product name that happens
/// to start with a weekday/month abbreviation ("Sun Valley Lodge", "May's
/// Diner") -- including one whose own trailing punctuation is a comma
/// further along ("March House, London") -- from satisfying any of these
/// follow-on shapes.
fn looks_like_calendar_date(after: &str) -> bool {
    let Some(name_len) = strip_weekday_or_month_prefix(after) else {
        return false;
    };
    let rest = &after[name_len..];
    let chars: Vec<char> = rest.chars().collect();
    let bound = 12.min(chars.len());
    if (0..bound).any(|i| chars[i].is_ascii_digit() && chars.get(i + 1) == Some(&',')) {
        return true;
    }
    if (0..bound).any(|i| chars[i] == ' ' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
        return true;
    }
    chars.iter().take(4).any(char::is_ascii_digit)
}

/// True when `content` (the text between a trailing pair of parentheses, in
/// its original casing) looks like a timezone abbreviation -- 2 to 5
/// uppercase ASCII letters, e.g. `CDT`, `PDT`, `UTC`, `CEST` -- or contains
/// a digit. Used to decide whether a trailing parenthesized group is
/// calendar decoration (safe to strip) or meaningful subject text such as
/// "(draft)", "(final)", or an organizer's name (must be kept).
fn looks_like_timezone_or_has_digit(content: &str) -> bool {
    let is_timezone_abbreviation = (2..=5).contains(&content.chars().count())
        && content.chars().all(|c| c.is_ascii_uppercase());
    is_timezone_abbreviation || content.chars().any(|c| c.is_ascii_digit())
}

/// If `s` ends (after trimming trailing whitespace) with a parenthesized
/// group, removes that group from `s` and returns its original-cased
/// content wrapped back in parentheses when it should be kept (anything
/// that is not a timezone abbreviation or digit-bearing, per
/// [`looks_like_timezone_or_has_digit`]). A group that should be stripped
/// (a timezone marker, or a date fragment such as "(Q3)") is simply
/// dropped. Only the single trailing group is ever inspected -- a group
/// further to the left, if any, is left exactly where it is for the
/// subsequent " @ " date truncation to consume, which is what lets
/// "... @ Mon Sep 7, 2026 9am - 10am (CDT) (Sam Rivera)" end up as "...
/// (Sam Rivera)": the organizer's name is peeled off and kept first, then
/// the date truncation swallows the now-trailing "(CDT)" along with the
/// rest of the date text.
fn peel_trailing_paren_group(s: &mut String) -> Option<String> {
    let trimmed_end = s.trim_end();
    if !trimmed_end.ends_with(')') {
        return None;
    }
    let open_idx = trimmed_end.rfind('(')?;
    let content = trimmed_end[open_idx + 1..trimmed_end.len() - 1].to_string();
    let keep = if looks_like_timezone_or_has_digit(&content) {
        None
    } else {
        Some(format!("({content})"))
    };
    s.truncate(open_idx);
    keep
}

/// Strips a reply-count suffix such as "[2]" from a prefix like "re[2]" or
/// "aw[3]" (Outlook/Exchange's numbered reply/forward marker), returning
/// the bare prefix. Leaves `prefix` unchanged when it does not end in a
/// bracketed, all-digit count.
fn strip_reply_count_suffix(prefix: &str) -> &str {
    if let Some(bracket_idx) = prefix.find('[')
        && prefix.ends_with(']')
    {
        let count = &prefix[bracket_idx + 1..prefix.len() - 1];
        if !count.is_empty() && count.chars().all(|c| c.is_ascii_digit()) {
            return &prefix[..bracket_idx];
        }
    }
    prefix
}

/// Normalizes a mail subject so that the same underlying thread compares
/// equal regardless of reply/forward/meeting-response prefixes (including
/// numbered "re[2]:" forms and a fullwidth "：" separator), an appended
/// meeting date/time, or a trailing organizer name in parentheses. Case is
/// preserved through the pipeline (and lowercased only once, at the very
/// end) so the trailing-parenthesis timezone check can inspect the
/// original casing. Pure and side-effect free.
pub fn normalize_subject(subject: &str) -> String {
    let mut s = subject.to_string();
    loop {
        let trimmed = s.trim_start();
        let Some(sep_idx) = trimmed.find([':', '\u{FF1A}']) else {
            break;
        };
        let raw_prefix = trimmed[..sep_idx].trim();
        let prefix = strip_reply_count_suffix(&raw_prefix.to_lowercase()).to_string();
        if !is_thread_prefix(&prefix) {
            break;
        }
        let sep_len = trimmed[sep_idx..].chars().next().map_or(1, char::len_utf8);
        s = trimmed[sep_idx + sep_len..].to_string();
    }
    let keep_suffix = peel_trailing_paren_group(&mut s);
    if let Some(idx) = s.rfind(" @ ") {
        let after_lower = s[idx + 3..].to_lowercase();
        if looks_like_calendar_date(&after_lower) {
            s.truncate(idx);
        }
    }
    let mut result = s.to_lowercase();
    if let Some(suffix) = keep_suffix {
        result.push(' ');
        result.push_str(&suffix.to_lowercase());
    }
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Reply, forward, and calendar-response prefixes that Outlook and Google
/// prepend to a subject. Only these are stripped: an arbitrary "word:" lead-in
/// such as "Budget: Q3" is part of the subject and must not merge threads.
fn is_thread_prefix(prefix: &str) -> bool {
    matches!(
        prefix,
        "re" | "fw"
            | "fwd"
            | "aw"
            | "wg"
            | "sv"
            | "vs"
            | "tr"
            | "accepted"
            | "tentatively accepted"
            | "tentative"
            | "declined"
            | "invitation"
            | "updated invitation"
            | "canceled"
            | "cancelled"
            | "updated"
            | "new time proposed"
            | "meeting forward notification"
            | "automatic reply"
    )
}

/// The subset of [`is_thread_prefix`]'s calendar-response prefixes for
/// which the stripped "@ date" suffix -- not the bare subject text -- was
/// the meeting's real identity: two different instances of the same
/// recurring meeting ("Invitation: Weekly sync @ ...") normalize to the
/// same bare subject, but they are not the same thread.
const CALENDAR_PREFIXES: &[&str] = &[
    "invitation",
    "updated invitation",
    "accepted",
    "tentatively accepted",
    "tentative",
    "declined",
    "canceled",
    "cancelled",
    "new time proposed",
];

/// True when ANY prefix in the same chain [`normalize_subject`] strips --
/// reply/forward wrappers included, so "Re: Invitation: ..." and "Fwd:
/// Updated invitation: ..." are caught, not just a bare leading
/// "Invitation:" -- is one of [`CALENDAR_PREFIXES`]. Walks the chain the
/// same way `normalize_subject` does (numbered reply-count suffixes and the
/// fullwidth `：` separator included), stopping at the first segment that is
/// not a thread prefix at all, exactly where `normalize_subject` would stop
/// stripping. Used by [`merge_threads`] to keep a group carrying such a
/// subject from ever merging with another, since the calendar prefix means
/// the normalized subject alone does not identify the thread.
fn raw_subject_has_calendar_prefix(subject: &str) -> bool {
    let mut rest = subject;
    loop {
        let trimmed = rest.trim_start();
        let Some(sep_idx) = trimmed.find([':', '\u{FF1A}']) else {
            return false;
        };
        let raw_prefix = trimmed[..sep_idx].trim();
        let prefix = strip_reply_count_suffix(&raw_prefix.to_lowercase()).to_string();
        if !is_thread_prefix(&prefix) {
            return false;
        }
        if CALENDAR_PREFIXES.contains(&prefix.as_str()) {
            return true;
        }
        let sep_len = trimmed[sep_idx..].chars().next().map_or(1, char::len_utf8);
        rest = &trimmed[sep_idx + sep_len..];
    }
}

fn union_find_root(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

fn union_find_union(parent: &mut [usize], a: usize, b: usize) {
    let ra = union_find_root(parent, a);
    let rb = union_find_root(parent, b);
    if ra != rb {
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        parent[hi] = lo;
    }
}

#[derive(Default)]
struct ThreadGroup {
    subjects: BTreeSet<String>,
    addresses: BTreeSet<String>,
    indices: Vec<usize>,
    /// True when any message in this group has a raw subject carrying a
    /// calendar-response prefix (see [`raw_subject_has_calendar_prefix`]).
    has_calendar_prefix: bool,
    /// True when any message in this group came from a group source
    /// (`ConversationMessage::team`). Group sources keep their own thread
    /// identity and never merge.
    has_team: bool,
}

/// A normalized subject is only strong enough evidence to merge two groups
/// when it carries real content: at least two words, or at least 12
/// characters. This filters out short, generic subjects ("hi", "fyi") that
/// would otherwise bridge unrelated threads.
fn subject_strong_enough(subject: &str) -> bool {
    subject.split_whitespace().count() >= 2 || subject.chars().count() >= 12
}

/// True when an address that appears in the intersection of two groups'
/// `other_addresses` is a shared-mailbox or distribution-list address --
/// one that appears in at least 3 of `account`'s groups AND in at least
/// half of them -- rather than a genuine outside participant linking the
/// two threads. Requiring "every group" (as an earlier version of this
/// check did) never fires on a real mailbox: a distribution list rarely
/// appears on literally every conversation an account has, so the earlier
/// rule failed to recognize it as a shared address in practice. Requires at
/// least 8 conversation groups for the account before this check ever
/// applies: below that floor, a small mailbox does not yet have enough
/// conversation groups to tell a genuinely shared address apart from an
/// individual correspondent who simply appears often (review found the
/// earlier floor of 3 groups too easily satisfied by an ordinary small
/// mailbox, wrongly excluding a real correspondent's address from the
/// intersection check).
fn is_shared_mailbox_address(
    account: &str,
    address: &str,
    groups_per_account: &BTreeMap<&str, usize>,
    address_group_counts: &BTreeMap<(&str, &str), usize>,
) -> bool {
    let total = groups_per_account.get(account).copied().unwrap_or(0);
    if total < 8 {
        return false;
    }
    let count = address_group_counts
        .get(&(account, address))
        .copied()
        .unwrap_or(0);
    count >= 3 && count * 2 >= total
}

/// Number of distinct conversation groups per account.
type GroupsPerAccount<'a> = BTreeMap<&'a str, usize>;
/// Number of an account's conversation groups a given (account, address)
/// pair's address appears in.
type AddressGroupCounts<'a> = BTreeMap<(&'a str, &'a str), usize>;

/// Builds the two per-account count maps [`is_shared_mailbox_address`]
/// needs, straight from `messages`: how many distinct (account,
/// conversation) groups each account has, and how many of an account's
/// groups each address appears in (via any message's `other_addresses`).
/// This is the same shape of counts [`merge_threads`] builds inline from its
/// own `ThreadGroup`s for the identical purpose (spotting a distribution
/// list or shared mailbox rather than a genuine correspondent) -- factored
/// out here so the cross-thread closure pass (`scan_closures`, which has no
/// `ThreadGroup`s of its own) can reuse the same `is_shared_mailbox_address`
/// decision without re-deriving its own notion of "shared address".
fn conversation_group_address_counts(
    messages: &[ReviewMessage],
) -> (GroupsPerAccount<'_>, AddressGroupCounts<'_>) {
    let mut groups: BTreeMap<(&str, &str), BTreeSet<&str>> = BTreeMap::new();
    for m in messages {
        groups
            .entry((m.account.as_str(), m.conversation.as_str()))
            .or_default()
            .extend(m.other_addresses.iter().map(String::as_str));
    }
    let mut groups_per_account: BTreeMap<&str, usize> = BTreeMap::new();
    for (account, _) in groups.keys() {
        *groups_per_account.entry(*account).or_insert(0) += 1;
    }
    let mut address_group_counts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for ((account, _), addresses) in &groups {
        for address in addresses {
            *address_group_counts
                .entry((*account, *address))
                .or_insert(0) += 1;
        }
    }
    (groups_per_account, address_group_counts)
}

/// True when the nearest pair of messages across the two groups (by
/// `ReviewMessage::input.timestamp`) falls within `max_gap_seconds` of each
/// other.
fn groups_within(
    a: &ThreadGroup,
    b: &ThreadGroup,
    messages: &[ReviewMessage],
    max_gap_seconds: i64,
) -> bool {
    a.indices.iter().any(|&i| {
        b.indices.iter().any(|&j| {
            (messages[i].input.timestamp - messages[j].input.timestamp).abs() <= max_gap_seconds
        })
    })
}

/// The pair predicate [`merge_threads`] applies to every candidate pair of
/// groups already known to share an account: every bullet on its doc
/// comment except "same account", which the caller has already checked.
fn should_merge(
    account: &str,
    a: &ThreadGroup,
    b: &ThreadGroup,
    messages: &[ReviewMessage],
    groups_per_account: &BTreeMap<&str, usize>,
    address_group_counts: &BTreeMap<(&str, &str), usize>,
    max_gap_seconds: i64,
) -> bool {
    // Neither group's raw subjects carried a calendar-response prefix, and
    // neither is a group source -- both keep their own thread identity.
    if a.has_team || b.has_team || a.has_calendar_prefix || b.has_calendar_prefix {
        return false;
    }
    // A shared normalized subject that is non-empty and substantial.
    let subjects_match = a
        .subjects
        .intersection(&b.subjects)
        .any(|s| subject_strong_enough(s));
    if !subjects_match {
        return false;
    }
    // A shared `other_addresses` entry that is not a shared-mailbox or
    // distribution-list address.
    let addresses_match = a.addresses.intersection(&b.addresses).any(|addr| {
        !is_shared_mailbox_address(account, addr, groups_per_account, address_group_counts)
    });
    if !addresses_match {
        return false;
    }
    // The nearest pair of messages across the two groups is within
    // `max_gap_seconds` of each other.
    groups_within(a, b, messages, max_gap_seconds)
}

/// Merges conversation groups (keyed by account + Graph `conversationId`)
/// that are really the same thread, split by Exchange into different
/// `conversationId`s. All of the following must hold for a pair of groups
/// to merge (see [`should_merge`]):
/// - same account;
/// - a shared normalized subject that is non-empty and substantial (see
///   [`subject_strong_enough`]);
/// - neither group's raw subjects carried a calendar-response prefix (see
///   [`raw_subject_has_calendar_prefix`]) -- for those, the date/time that
///   normalization strips was the meeting's real identity, so the bare
///   subject cannot distinguish one instance from another;
/// - a shared `other_addresses` entry that is not a shared-mailbox or
///   distribution-list address common to the account (see
///   [`is_shared_mailbox_address`]);
/// - the nearest pair of messages across the two groups is within 3 days
///   (259,200 seconds) of each other;
/// - every message in both groups is personal mail, never a group source
///   (`team == false`) -- group sources keep their own thread identity.
///
/// Merging is transitive (chain-merges through a bridging group) via
/// union-find. Every message in a merged set is rewritten to carry the
/// lexicographically smallest conversation id in that set, so the result
/// is deterministic regardless of input order. Returns how many of the
/// original groups were absorbed into another (0 when nothing merged).
pub fn merge_threads(messages: &mut [ReviewMessage]) -> usize {
    const MAX_GAP_SECONDS: i64 = 259_200;
    let mut groups: BTreeMap<(String, String), ThreadGroup> = BTreeMap::new();
    for (i, m) in messages.iter().enumerate() {
        let key = (m.account.clone(), m.conversation.clone());
        let entry = groups.entry(key).or_default();
        let raw_subject = m.input.message.subject.as_string();
        let subject = normalize_subject(&raw_subject);
        if !subject.is_empty() {
            entry.subjects.insert(subject);
        }
        if raw_subject_has_calendar_prefix(&raw_subject) {
            entry.has_calendar_prefix = true;
        }
        if m.input.team {
            entry.has_team = true;
        }
        entry.addresses.extend(m.other_addresses.iter().cloned());
        entry.indices.push(i);
    }
    let keys: Vec<(String, String)> = groups.keys().cloned().collect();
    let group_count = keys.len();
    // Collected once into a Vec (index-aligned with `keys`) so the O(n^2)
    // pair loop below indexes a Vec instead of repeating a BTreeMap lookup
    // per pair.
    let group_values: Vec<ThreadGroup> = groups.into_values().collect();
    // Borrowed from `keys`/`group_values` (which outlive the pair loop)
    // rather than built from owned `String` keys, so the O(n^2) pair loop
    // below (and `is_shared_mailbox_address`, which it calls once per
    // shared address per pair) never allocates a `String` just to perform a
    // map lookup.
    let mut groups_per_account: BTreeMap<&str, usize> = BTreeMap::new();
    for (account, _) in &keys {
        *groups_per_account.entry(account.as_str()).or_insert(0) += 1;
    }
    let mut address_group_counts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for ((account, _), group) in keys.iter().zip(&group_values) {
        for address in &group.addresses {
            *address_group_counts
                .entry((account.as_str(), address.as_str()))
                .or_insert(0) += 1;
        }
    }
    let mut parent: Vec<usize> = (0..group_count).collect();
    for a in 0..group_count {
        for b in (a + 1)..group_count {
            if keys[a].0 != keys[b].0 {
                continue;
            }
            if should_merge(
                &keys[a].0,
                &group_values[a],
                &group_values[b],
                messages,
                &groups_per_account,
                &address_group_counts,
                MAX_GAP_SECONDS,
            ) {
                union_find_union(&mut parent, a, b);
            }
        }
    }
    let mut clusters: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..group_count {
        let root = union_find_root(&mut parent, i);
        clusters.entry(root).or_default().push(i);
    }
    let mut merged = 0usize;
    for members in clusters.into_values() {
        if members.len() <= 1 {
            continue;
        }
        merged += members.len() - 1;
        let winner = members
            .iter()
            .map(|&idx| keys[idx].1.clone())
            .min()
            .expect("non-empty cluster");
        for &idx in &members {
            for &msg_idx in &group_values[idx].indices {
                messages[msg_idx].conversation.clone_from(&winner);
            }
        }
    }
    merged
}

// Live semantic smoke suite: counts only, no returned content is logged or saved.
const SEMANTIC_CASES: [(&str, bool, bool, bool, usize); 7] = [
    (
        "Please send the draft budget by Friday.",
        false,
        true,
        false,
        1,
    ),
    (
        "I will send you the draft budget by Friday.",
        true,
        false,
        false,
        1,
    ),
    (
        "Meeting recap: Jordan hopes to change careers. They discussed a stipend and flexible work.",
        false,
        true,
        false,
        0,
    ),
    (
        "Morgan said they will send the budget. This update is for your information.",
        false,
        true,
        false,
        0,
    ),
    (
        "Team, could someone send the draft budget by Friday?",
        false,
        false,
        true,
        1,
    ),
    (
        "Please send the draft budget and schedule the planning meeting.",
        false,
        true,
        false,
        2,
    ),
    (
        "Thanks.\nOn Monday Alex wrote:\nPlease send the draft budget.",
        false,
        true,
        false,
        0,
    ),
];
// Live semantic probe: a request that asked for the user's agreement or
// decision, and got it, one message later must still be recognized as
// closure evidence, with resolution_kind Agreed rather than a plain
// completion. The reply is a paraphrase of the resolution_kind agreed
// example quoted in INSTRUCTIONS, not a literal copy, so this exercises
// genuine semantic recognition rather than an echo of the prompt's own
// example text.
const AGREEMENT_CASE: (&str, &str) = (
    "Can we move our meeting to a different time?",
    "Yes, happy to push it back an hour.",
);
// A correction that leaves the underlying action owed -- only the amount
// changed -- must NOT resolve the request: it must stay open, with the
// corrected amount reflected in `action`, citing the original request as
// evidence.
const AMENDMENT_CASE: (&str, &str) = (
    "My fee for the call is 359 USD, to be paid any time before our call.",
    "Apologies, the fee for the call is 350, not 359.",
);
// A plain completion must still resolve to resolution_kind Completed now
// that resolution_kind distinguishes several closure kinds.
const COMPLETED_RESOLUTION_CASE: (&str, &str) = (
    "Please send me the signed engagement letter.",
    "Attached is the signed engagement letter.",
);
fn resolution_kind_name(kind: Option<ResolutionKind>) -> &'static str {
    match kind {
        None => "none",
        Some(ResolutionKind::Completed) => "completed",
        Some(ResolutionKind::Declined) => "declined",
        Some(ResolutionKind::Withdrawn) => "withdrawn",
        Some(ResolutionKind::Superseded) => "superseded",
        Some(ResolutionKind::Agreed) => "agreed",
    }
}
pub fn probe(key: String, model: &str) -> Result<usize, ProviderError> {
    let provider = OllamaCloud::connect(key, model)?;
    let mut passed = 0;
    for (body, from_user, to_user, team, expected) in SEMANTIC_CASES {
        let item = synthetic(body, 0, "a");
        let mut m = prepare(&item, "Synthetic", 0)
            .map_err(|_| ProviderError::InvalidAnalysis)?
            .input;
        m.from_user = from_user;
        m.to_user = to_user;
        m.team = team;
        if from_user {
            set_outgoing(&mut m);
        }
        let result = provider.expectations(&[m])?;
        println!(
            "Semantic case {}: {} accepted, {} rejected, {} degraded; expected {}.",
            passed + 1,
            result.items.len(),
            result.rejected,
            result.degraded,
            expected
        );
        for reason in &result.rejection_reasons {
            println!("{reason}");
        }
        if result.items.len() != expected || result.rejected != 0 || result.degraded != 0 {
            return Err(ProviderError::InvalidAnalysis);
        }
        if expected > 0
            && result.items.iter().any(|item| {
                item.owner
                    != if team {
                        openloops_inference::ollama::expectations::Owner::Team
                    } else {
                        openloops_inference::ollama::expectations::Owner::You
                    }
            })
        {
            return Err(ProviderError::InvalidAnalysis);
        }
        if passed < 2
            && result.items[0]
                .deadline
                .as_ref()
                .is_none_or(|d| !d.quote.contains("Friday"))
        {
            return Err(ProviderError::InvalidAnalysis);
        }
        if expected == 2 && result.items[0].action_phrase == result.items[1].action_phrase {
            return Err(ProviderError::InvalidAnalysis);
        }
        passed += 1;
    }
    let first = synthetic("Please send the draft budget.", 0, "b");
    let second = synthetic(
        "I have sent the completed draft budget as requested.",
        1,
        "b",
    );
    let mut a = prepare(&first, "Synthetic", 0)
        .map_err(|_| ProviderError::InvalidAnalysis)?
        .input;
    a.to_user = true;
    let mut b = prepare(&second, "Synthetic", 1)
        .map_err(|_| ProviderError::InvalidAnalysis)?
        .input;
    b.from_user = true;
    set_outgoing(&mut b);
    let result = provider.expectations(&[a.clone(), b])?;
    if result.items.len() != 1 || result.items[0].resolution.is_none() {
        return Err(ProviderError::InvalidAnalysis);
    }
    println!(
        "Semantic case {}: later completion evidence identified.",
        passed + 1
    );
    let acknowledgement = synthetic("Thanks, I will take a look at this later.", 1, "b");
    let mut ack = prepare(&acknowledgement, "Synthetic", 1)
        .map_err(|_| ProviderError::InvalidAnalysis)?
        .input;
    ack.from_user = true;
    set_outgoing(&mut ack);
    let result = provider.expectations(&[a, ack])?;
    if result.items.len() != 1 || result.items[0].resolution.is_some() {
        return Err(ProviderError::InvalidAnalysis);
    }
    println!(
        "Semantic case {}: acknowledgement did not close the request.",
        passed + 2
    );
    probe_agreement_case(&provider, passed + 3)?;
    probe_amendment_case(&provider, passed + 4)?;
    probe_completed_resolution_case(&provider, passed + 5)?;
    Ok(passed + 5)
}

/// Runs `AGREEMENT_CASE` against `provider`: a request that asked for the
/// user's agreement or decision, and got it, one message later must still
/// be recognized as closure evidence, with `resolution_kind` Agreed. The
/// reply may itself read as a new request, so 1 or 2 items are both
/// acceptable; what matters is that the item anchored on the original
/// request (`m0`) carries the expected resolution kind. `case_number` is
/// only for print numbering. Prints counts and the observed kind name (both
/// fixed strings) only; never returned content.
fn probe_agreement_case(provider: &OllamaCloud, case_number: usize) -> Result<(), ProviderError> {
    let (request, reply) = AGREEMENT_CASE;
    let request_item = synthetic(request, 0, "c");
    let mut request_message = prepare(&request_item, "Synthetic", 0)
        .map_err(|_| ProviderError::InvalidAnalysis)?
        .input;
    request_message.to_user = true;
    let reply_item = synthetic(reply, 2, "c");
    let mut reply_message = prepare(&reply_item, "Synthetic", 1)
        .map_err(|_| ProviderError::InvalidAnalysis)?
        .input;
    reply_message.from_user = true;
    set_outgoing(&mut reply_message);
    let result = provider.expectations(&[request_message, reply_message])?;
    let anchored = result
        .items
        .iter()
        .find(|item| item.evidence.message == "m0");
    println!(
        "Semantic case {}: {} accepted, {} rejected, {} degraded.",
        case_number,
        result.items.len(),
        result.rejected,
        result.degraded
    );
    println!(
        "Semantic case {}: observed resolution kind {}.",
        case_number,
        resolution_kind_name(anchored.and_then(|item| item.resolution_kind))
    );
    for reason in &result.rejection_reasons {
        println!("{reason}");
    }
    if !(1..=2).contains(&result.items.len()) || result.rejected != 0 || result.degraded != 0 {
        return Err(ProviderError::InvalidAnalysis);
    }
    let Some(item) = anchored else {
        return Err(ProviderError::InvalidAnalysis);
    };
    if item.resolution.is_none() || !matches!(item.resolution_kind, Some(ResolutionKind::Agreed)) {
        return Err(ProviderError::InvalidAnalysis);
    }
    Ok(())
}

/// Runs `AMENDMENT_CASE` against `provider`: a correction that leaves the
/// underlying action owed (only the amount changed) must NOT resolve the
/// request -- it must stay open, with the corrected amount reflected in
/// `action`, citing the original request as evidence. `case_number` is only
/// for print numbering. Prints counts only (fixed strings); the corrected
/// amount is asserted, never printed, since `action` is model-supplied free
/// text derived from message content.
fn probe_amendment_case(provider: &OllamaCloud, case_number: usize) -> Result<(), ProviderError> {
    let (request, correction) = AMENDMENT_CASE;
    let request_item = synthetic(request, 0, "e");
    let mut request_message = prepare(&request_item, "Synthetic", 0)
        .map_err(|_| ProviderError::InvalidAnalysis)?
        .input;
    request_message.to_user = true;
    let correction_item = synthetic(correction, 2, "e");
    let mut correction_message = prepare(&correction_item, "Synthetic", 1)
        .map_err(|_| ProviderError::InvalidAnalysis)?
        .input;
    correction_message.to_user = true;
    let result = provider.expectations(&[request_message, correction_message])?;
    println!(
        "Semantic case {}: {} accepted, {} rejected, {} degraded.",
        case_number,
        result.items.len(),
        result.rejected,
        result.degraded
    );
    for reason in &result.rejection_reasons {
        println!("{reason}");
    }
    if result.items.len() != 1 || result.rejected != 0 || result.degraded != 0 {
        return Err(ProviderError::InvalidAnalysis);
    }
    let item = &result.items[0];
    if item.resolution.is_some() || !item.action.contains("350") {
        return Err(ProviderError::InvalidAnalysis);
    }
    Ok(())
}

/// Runs `COMPLETED_RESOLUTION_CASE` against `provider`: a plain completion
/// must still resolve to exactly one item with `resolution_kind` Completed.
/// `case_number` is only for print numbering. Prints counts and the
/// observed kind name (both fixed strings) only; never returned content.
fn probe_completed_resolution_case(
    provider: &OllamaCloud,
    case_number: usize,
) -> Result<(), ProviderError> {
    let (request, reply) = COMPLETED_RESOLUTION_CASE;
    let request_item = synthetic(request, 0, "d");
    let mut request_message = prepare(&request_item, "Synthetic", 0)
        .map_err(|_| ProviderError::InvalidAnalysis)?
        .input;
    request_message.to_user = true;
    let reply_item = synthetic(reply, 2, "d");
    let mut reply_message = prepare(&reply_item, "Synthetic", 1)
        .map_err(|_| ProviderError::InvalidAnalysis)?
        .input;
    reply_message.from_user = true;
    set_outgoing(&mut reply_message);
    let result = provider.expectations(&[request_message, reply_message])?;
    println!(
        "Semantic case {}: {} accepted, {} rejected, {} degraded.",
        case_number,
        result.items.len(),
        result.rejected,
        result.degraded
    );
    println!(
        "Semantic case {}: observed resolution kind {}.",
        case_number,
        resolution_kind_name(result.items.first().and_then(|item| item.resolution_kind))
    );
    for reason in &result.rejection_reasons {
        println!("{reason}");
    }
    if result.items.len() != 1 || result.rejected != 0 || result.degraded != 0 {
        return Err(ProviderError::InvalidAnalysis);
    }
    if !matches!(
        result.items[0].resolution_kind,
        Some(ResolutionKind::Completed)
    ) {
        return Err(ProviderError::InvalidAnalysis);
    }
    Ok(())
}

fn set_outgoing(m: &mut ConversationMessage) {
    m.message.sender =
        Some(CanonicalBlock::new("User <user@example.invalid>").expect("synthetic block"));
    m.message.to =
        vec![CanonicalBlock::new("Alex <alex@example.invalid>").expect("synthetic block")];
}
fn synthetic(body: &str, index: usize, conversation: &str) -> MailItem {
    MailItem {
        subject: "Synthetic budget conversation".into(),
        body: body.into(),
        sender: "Alex <alex@example.invalid>".into(),
        sender_address: "alex@example.invalid".into(),
        received: format!("2026-09-0{}T12:00:00Z", index + 1),
        id: format!("synthetic-{index}"),
        conversation: conversation.into(),
        account: "synthetic-account".into(),
        to: vec!["user@example.invalid".into()],
        own_addresses: vec!["user@example.invalid".into()],
        ..MailItem::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn timestamp(value: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(value)
            .unwrap()
            .timestamp()
    }

    #[test]
    fn subject_event_time_parses_timed_and_all_day_calendar_subjects() {
        let cases = [
            (
                "Accepted: Design workshop @ Fri Aug 21, 2026 11:30am - 12:30pm (CDT)",
                "2026-08-21T16:30:00Z",
                "2026-08-21T17:30:00Z",
            ),
            (
                "Invitation: Design workshop @ Fri Aug 21, 2026 11:30am - 12:30pm (PDT)",
                "2026-08-21T18:30:00Z",
                "2026-08-21T19:30:00Z",
            ),
            (
                "Updated invitation: Planning call @ Mon Sep 7, 2026 9am - 10am (PDT)",
                "2026-09-07T16:00:00Z",
                "2026-09-07T17:00:00Z",
            ),
            (
                "Invitation: Design workshop @ Fri Aug 21, 2026",
                "2026-08-21T00:00:00Z",
                "2026-08-22T00:00:00Z",
            ),
            (
                "Invitation: Design workshop @ Fri Aug 21, 2026 11am - 12pm (XYZ)",
                "2026-08-21T11:00:00Z",
                "2026-08-21T12:00:00Z",
            ),
        ];
        for (subject, start, end) in cases {
            assert_eq!(
                subject_event_time(subject, 0),
                Some((timestamp(start), timestamp(end))),
                "{subject}"
            );
        }
        assert_eq!(subject_event_time("Design workshop notes", 0), None);
    }

    #[test]
    fn ranges_crossing_midnight_add_a_day_to_the_end() {
        assert_eq!(
            subject_event_time(
                "Invitation: Late call @ Fri Aug 21, 2026 11pm - 1am (UTC)",
                0
            ),
            Some((
                timestamp("2026-08-21T23:00:00Z"),
                timestamp("2026-08-22T01:00:00Z")
            ))
        );
    }

    #[test]
    fn organizer_suffix_after_the_timezone_is_peeled_and_ignored() {
        assert_eq!(
            subject_event_time(
                "Invitation: Design workshop @ Fri Aug 21, 2026 11:30am - 12:30pm (CDT) (Sam Rivera)",
                0
            ),
            Some((
                timestamp("2026-08-21T16:30:00Z"),
                timestamp("2026-08-21T17:30:00Z")
            ))
        );
    }

    #[test]
    fn spaced_am_pm_tokens_still_parse() {
        assert_eq!(
            subject_event_time(
                "Invitation: Design workshop @ Fri Aug 21, 2026 11:30 am - 12:30 pm (CDT)",
                0
            ),
            Some((
                timestamp("2026-08-21T16:30:00Z"),
                timestamp("2026-08-21T17:30:00Z")
            ))
        );
    }

    #[test]
    fn event_index_prefers_graph_metadata_and_deduplicates_name_and_start() {
        let mut mail = synthetic("Prepare the handout.", 0, "a");
        mail.subject = "Invitation: Design workshop @ Fri Aug 21, 2026 11am - 12pm (UTC)".into();
        mail.event = Some(openloops_graph::live::review::MailEvent {
            start: "2026-08-21T13:00:00Z".into(),
            end: "2026-08-21T14:00:00Z".into(),
            out_of_date: false,
        });
        let first = prepare(&mail, "Inbox", 0).unwrap();
        let mut second = first.clone();
        second.input.handle = "m1".into();
        let index = build_event_index(&[first, second]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "design workshop");
        assert_eq!(index[0].start, timestamp("2026-08-21T13:00:00Z"));
        assert_eq!(index[0].message_handle, "m0");
        assert!(index[0].from_meeting);
    }

    #[test]
    fn event_index_keeps_the_latest_of_two_instances_of_the_same_named_event() {
        let mut aug = synthetic("Save the date.", 0, "a");
        aug.subject = "Invitation: Design workshop @ Fri Aug 21, 2026 11am - 12pm (UTC)".into();
        let aug_message = prepare(&aug, "Inbox", 0).unwrap();
        let mut sep = synthetic("Rescheduled, see below.", 1, "b");
        sep.subject =
            "Updated invitation: Design workshop @ Tue Sep 15, 2026 11am - 12pm (UTC)".into();
        let sep_message = prepare(&sep, "Inbox", 1).unwrap();
        let index = build_event_index(&[aug_message, sep_message]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].start, timestamp("2026-09-15T11:00:00Z"));
        assert!(!index[0].from_meeting);
    }

    #[test]
    fn event_index_drops_an_out_of_date_meeting_entry() {
        let mut mail = synthetic("Old invite.", 0, "a");
        mail.subject = "Design workshop".into();
        mail.event = Some(openloops_graph::live::review::MailEvent {
            start: "2026-08-21T11:00:00Z".into(),
            end: "2026-08-21T12:00:00Z".into(),
            out_of_date: true,
        });
        let message = prepare(&mail, "Inbox", 0).unwrap();
        assert!(build_event_index(&[message]).is_empty());
    }

    #[test]
    fn match_event_rejects_a_phrase_with_no_meaningful_tokens() {
        let index = vec![EventRef {
            name: "design workshop".into(),
            start: timestamp("2026-08-20T00:00:00Z"),
            end: timestamp("2026-08-20T01:00:00Z"),
            message_handle: "m0".into(),
            from_meeting: false,
        }];
        assert!(match_event("the call", timestamp("2026-08-01T00:00:00Z"), &index).is_none());
    }

    #[test]
    fn match_event_requires_a_real_token_overlap_not_a_substring() {
        let evidence = timestamp("2026-08-01T00:00:00Z");
        let workshop_only = vec![EventRef {
            name: "design workshop".into(),
            start: timestamp("2026-08-20T00:00:00Z"),
            end: timestamp("2026-08-20T01:00:00Z"),
            message_handle: "m0".into(),
            from_meeting: false,
        }];
        assert_eq!(
            match_event("the design workshop", evidence, &workshop_only)
                .unwrap()
                .message_handle,
            "m0"
        );
        let standup_only = vec![EventRef {
            name: "weekly standup call".into(),
            start: timestamp("2026-08-20T00:00:00Z"),
            end: timestamp("2026-08-20T01:00:00Z"),
            message_handle: "m1".into(),
            from_meeting: false,
        }];
        assert!(match_event("the design workshop", evidence, &standup_only).is_none());
    }

    #[test]
    fn match_event_drops_a_generic_event_noun_from_both_sides() {
        let evidence = timestamp("2026-08-01T00:00:00Z");
        let index = vec![EventRef {
            name: "hearing motion to compel".into(),
            start: timestamp("2026-08-20T00:00:00Z"),
            end: timestamp("2026-08-20T01:00:00Z"),
            message_handle: "m0".into(),
            from_meeting: false,
        }];
        assert_eq!(
            match_event("our hearing on the motion", evidence, &index)
                .unwrap()
                .message_handle,
            "m0"
        );
    }

    #[test]
    fn match_event_prefers_the_nearest_future_event_within_the_sixty_day_window() {
        let evidence = timestamp("2026-08-01T00:00:00Z");
        let index = vec![
            EventRef {
                name: "design workshop".into(),
                start: timestamp("2026-08-20T00:00:00Z"),
                end: timestamp("2026-08-20T01:00:00Z"),
                message_handle: "m0".into(),
                from_meeting: false,
            },
            EventRef {
                name: "design workshop follow up".into(),
                start: timestamp("2026-08-10T00:00:00Z"),
                end: timestamp("2026-08-10T01:00:00Z"),
                message_handle: "m1".into(),
                from_meeting: false,
            },
        ];
        assert_eq!(
            match_event("design workshop", evidence, &index)
                .unwrap()
                .message_handle,
            "m1"
        );
        assert!(
            match_event("design workshop", timestamp("2026-05-01T00:00:00Z"), &index).is_none()
        );
    }
    #[test]
    fn inbox_and_sent_are_analyzed_as_one_ordered_conversation() {
        let a = prepare(&synthetic("Please send the draft.", 0, "a"), "Inbox", 0).unwrap();
        let b = prepare(&synthetic("Here is the draft.", 1, "a"), "Sent", 1).unwrap();
        let c = prepare(&synthetic("Unrelated conversation.", 0, "b"), "Inbox", 2).unwrap();
        let mut lengths = vec![];
        let result = scan_conversations(&[b, a, c], &ScanProgress::default(), |messages| {
            lengths.push(messages.len());
            assert!(
                messages
                    .windows(2)
                    .all(|w| w[0].timestamp <= w[1].timestamp)
            );
            Ok(Expectations {
                items: vec![],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            })
        });
        assert_eq!(lengths, [2, 1]);
        assert_eq!(result.analyzed, 3);
    }
    #[test]
    fn failed_conversation_is_not_reported_as_clean_scan() {
        let m = prepare(&synthetic("Please send the draft.", 0, "a"), "Inbox", 0).unwrap();
        let result = scan_conversations(&[m], &ScanProgress::default(), |_| {
            Err(ProviderError::InvalidJson)
        });
        let mut state = super::super::ReviewState::default();
        state.set_scan(result, "synthetic".into());
        assert!(state.scan_incomplete);
    }
    #[test]
    fn quoted_plain_history_is_not_current_evidence() {
        let item = synthetic(
            "Thanks.\nOn Monday Alex wrote:\nPlease send the draft.",
            0,
            "a",
        );
        let m = prepare(&item, "Inbox", 0).unwrap();
        assert!(
            !m.input.message.body_blocks[0]
                .as_string()
                .contains("send the draft")
        );
        assert!(!m.input.message.quote_blocks.is_empty());
    }
    #[test]
    fn outlook_style_plain_text_reply_history_is_not_current_evidence() {
        // Plain-text Outlook replies put an underscore separator and a
        // From:/Sent:/To:/Subject: header before the quoted original, with
        // no "On ... wrote:" marker at all.
        let (body, _quote) = plain_body(concat!(
            "Sure.\n",
            "________________________________\n",
            "From: Alex\n",
            "Sent: Monday\n",
            "To: Me\n",
            "Subject: Meeting\n",
            "\n",
            "Can we change the meeting time?",
        ));
        assert_eq!(body.trim(), "Sure.");
    }
    #[test]
    fn plain_text_from_with_subject_nearby_starts_history_without_underscore_line() {
        // Exercises the From:+Subject: rule directly: no underscore
        // separator line precedes the header, so the underscore rule
        // cannot be what triggers history here.
        let (body, quote) = plain_body(concat!(
            "Sure.\n",
            "From: Alex\n",
            "Sent: Monday\n",
            "To: Me\n",
            "Subject: Meeting\n",
            "\n",
            "Can we change the meeting time?",
        ));
        assert_eq!(body.trim(), "Sure.");
        assert!(quote.contains("Can we change the meeting time?"));
    }
    #[test]
    fn plain_text_from_without_nearby_subject_stays_in_body() {
        // A "From:" line with no "Subject:" line within the next 5 lines
        // must not be treated as the start of reply history.
        let (body, quote) = plain_body(concat!(
            "Sure.\n",
            "From: Alex, checking in on this.\n",
            "Have a good day.",
        ));
        assert_eq!(
            body.trim(),
            "Sure.\nFrom: Alex, checking in on this.\nHave a good day."
        );
        assert!(quote.is_empty());
    }
    #[test]
    fn plain_text_from_with_subject_but_no_sent_or_date_stays_in_body() {
        // Both a Subject: line AND a Sent:/Date: line are now required
        // within the next 5 lines; Subject: alone is not enough.
        let (body, quote) = plain_body(concat!(
            "Sure.\n",
            "From: Alex\n",
            "Subject: Meeting\n",
            "\n",
            "Can we change the meeting time?",
        ));
        assert_eq!(
            body.trim(),
            "Sure.\nFrom: Alex\nSubject: Meeting\n\nCan we change the meeting time?"
        );
        assert!(quote.is_empty());
    }
    #[test]
    fn plain_text_header_first_message_with_nothing_before_it_is_not_split() {
        // Mirrors the HTML path's "match at paragraph 0" guard: if the
        // very first line already looks like a reply-history header, with
        // nothing preceding it, the whole message stays in the body
        // rather than being emptied into an all-quote message.
        let (body, quote) = plain_body(concat!(
            "From: Alex\n",
            "Sent: Monday\n",
            "Subject: Meeting\n",
            "\n",
            "FYI, can you handle this?",
        ));
        assert_eq!(
            body.trim(),
            "From: Alex\nSent: Monday\nSubject: Meeting\n\nFYI, can you handle this?"
        );
        assert!(quote.is_empty());
    }
    #[test]
    fn prepare_keeps_header_first_plain_text_message_as_nonempty_body() {
        let item = synthetic(
            "From: Alex\nSent: Monday\nSubject: Meeting\n\nFYI, can you handle this?",
            0,
            "a",
        );
        let m = prepare(&item, "Inbox", 0).unwrap();
        assert!(!m.input.message.body_blocks.is_empty());
    }
    #[test]
    fn plain_text_all_quoted_lines_with_no_history_marker_stay_in_quote() {
        // Regression: the "match at paragraph 0" guard must NOT fire just
        // because `body` ends up empty -- a message made entirely of
        // `>`-prefixed lines has no reply-history marker at all
        // (`history_start` stays `None`), so this is not the header-first
        // case and everything belongs in quote, not body.
        let (body, quote) = plain_body("> old text line one\n> old text line two");
        assert!(body.trim().is_empty());
        assert_eq!(quote.trim(), "> old text line one\n> old text line two");
    }
    #[test]
    fn plain_text_quoted_line_before_original_message_header_stays_in_quote() {
        // Regression: the reply-history marker ("-----Original
        // Message-----") is found at line index 1, not 0 (a `>`-quoted
        // line precedes it), so the guard must not fire even though body
        // ends up empty -- everything, including the leading `>` line,
        // belongs in quote.
        let (body, quote) = plain_body(concat!(
            "> old\n",
            "-----Original Message-----\n",
            "From: A\n",
            "Sent: B\n",
            "Subject: C\n",
            "body",
        ));
        assert!(body.trim().is_empty());
        assert_eq!(
            quote.trim(),
            "> old\n-----Original Message-----\nFrom: A\nSent: B\nSubject: C\nbody"
        );
    }
    #[test]
    fn plain_text_hard_wrapped_quote_with_no_blank_lines_chunks_by_line() {
        // A quoted original with no blank lines at all collapses to ONE
        // paragraph under the blank-line split, which would leave
        // `chunk_reply_history` powerless to bound it (nothing to split
        // on). `split_oversized_paragraph_by_lines` restores line-level
        // splitting boundaries so the 4096-char chunk cap still applies.
        let mut text = String::from("Sure.\n-----Original Message-----\n");
        for i in 0..300 {
            use std::fmt::Write as _;
            let _ = writeln!(
                text,
                "Line {i:03} of the original hard-wrapped message body text here."
            );
        }
        let item = synthetic(&text, 0, "a");
        let m = prepare(&item, "Inbox", 0).unwrap();
        assert!(
            m.input.message.quote_blocks.len() > 1,
            "expected more than one quote block, got {}",
            m.input.message.quote_blocks.len()
        );
        for block in &m.input.message.quote_blocks {
            assert!(block.as_string().chars().count() <= 4096);
        }
    }
    #[test]
    fn cancellation_and_provider_failure_do_not_start_more_conversations() {
        let a = prepare(&synthetic("Please send the draft.", 0, "a"), "Inbox", 0).unwrap();
        let b = prepare(&synthetic("Please send the agenda.", 1, "b"), "Inbox", 1).unwrap();
        let progress = ScanProgress::default();
        let result = scan_conversations(&[a.clone(), b.clone()], &progress, |_| {
            progress.cancel.store(true, Ordering::Relaxed);
            Ok(Expectations {
                items: vec![],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            })
        });
        assert_eq!(result.analyzed, 1);
        assert!(result.cancelled);
        let mut calls = 0;
        let result = scan_conversations(&[a, b], &ScanProgress::default(), |_| {
            calls += 1;
            Err(ProviderError::RateLimited)
        });
        assert_eq!(calls, 1);
        assert_eq!(result.analyzed, 0);
        assert_eq!(result.failures.len(), 1);
    }

    #[test]
    fn normalize_subject_table() {
        let cases = [
            (
                "Re: Alex and Sam discuss quarterly planning",
                "alex and sam discuss quarterly planning",
            ),
            (
                "Alex and Sam discuss quarterly planning",
                "alex and sam discuss quarterly planning",
            ),
            (
                // The trailing "(Sam Rivera)" organizer name is not a
                // timezone abbreviation and carries no digit, so it is
                // preserved; "(CDT)" is swallowed along with the rest of
                // the date text by the " @ " truncation.
                "Tentatively Accepted: Alex and Sam discuss quarterly planning @ Fri Aug 21, 2026 11:30am - 12:30pm (CDT) (Sam Rivera)",
                "alex and sam discuss quarterly planning (sam rivera)",
            ),
            ("RE: FW: Budget", "budget"),
            ("Budget: Q3 numbers", "budget: q3 numbers"),
            (
                "Updated invitation: Sync @ Mon Sep 7, 2026 9am - 10am (PDT)",
                "sync",
            ),
            ("Re:Budget", "budget"),
            ("re[2]: Budget", "budget"),
            ("aw[3]: Budget", "budget"),
            ("Re\u{FF1A}Budget", "budget"),
            ("Contract review (draft)", "contract review (draft)"),
            ("Contract review (final)", "contract review (final)"),
            ("Team sync (CDT)", "team sync"),
            ("Offsite @ Sun Valley Lodge", "offsite @ sun valley lodge"),
            ("Lunch @ May's Diner", "lunch @ may's diner"),
            (
                // The comma here sits well past the bounded window checked
                // by `looks_like_calendar_date`, and even within it is not
                // preceded by a digit, so it is not mistaken for a date.
                "Offsite @ Sun Valley Lodge, Idaho",
                "offsite @ sun valley lodge, idaho",
            ),
            (
                // The comma here falls within the bounded window but is
                // preceded by a letter, not a digit, so it must not be
                // mistaken for "Mon Sep 7, 2026 ...".
                "Kickoff @ March House, London",
                "kickoff @ march house, london",
            ),
            (
                // The day number ("7") sits right at the 12-character
                // bound, immediately after the space that triggers the
                // "space directly followed by a digit" rule -- exercising
                // the one-character lookahead past that bound.
                "Sync @ Monday, September 7, 2026 9am",
                "sync",
            ),
            (
                // "Dec25" never reaches the date-shaped checks at all:
                // `strip_weekday_or_month_prefix` requires a non-alphanumeric
                // boundary immediately after the month name, and "2" fails
                // that, so this stays ordinary subject text.
                "Reminder @ Dec25 party",
                "reminder @ dec25 party",
            ),
            ("", ""),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize_subject(input), expected, "input: {input:?}");
        }
        assert_eq!(
            normalize_subject("Meeting @ the office"),
            "meeting @ the office",
            "text after \" @ \" is not a date and must not be truncated"
        );
    }

    fn mail(
        id: &str,
        conversation: &str,
        account: &str,
        subject: &str,
        item: MailItem,
    ) -> MailItem {
        MailItem {
            id: id.into(),
            conversation: conversation.into(),
            account: account.into(),
            subject: subject.into(),
            own_addresses: vec!["user@example.invalid".into()],
            ..item
        }
    }

    fn request_from(
        address: &str,
        id: &str,
        conversation: &str,
        account: &str,
        subject: &str,
    ) -> MailItem {
        mail(
            id,
            conversation,
            account,
            subject,
            MailItem {
                body: "Can we meet?".into(),
                sender: format!("Other <{address}>"),
                sender_address: address.into(),
                received: "2026-09-01T12:00:00Z".into(),
                to: vec!["user@example.invalid".into()],
                ..MailItem::default()
            },
        )
    }

    fn reply_to(
        address: &str,
        id: &str,
        conversation: &str,
        account: &str,
        subject: &str,
    ) -> MailItem {
        mail(
            id,
            conversation,
            account,
            subject,
            MailItem {
                body: "Sure, let's do it.".into(),
                sender: "User <user@example.invalid>".into(),
                sender_address: "user@example.invalid".into(),
                received: "2026-09-02T12:00:00Z".into(),
                to: vec![address.into()],
                ..MailItem::default()
            },
        )
    }

    fn closure_test_messages() -> (Vec<ReviewMessage>, Expectation) {
        let evidence = request_from("sam@example.invalid", "req-1", "c1", "acct", "Fee");
        let all = vec![prepare(&evidence, "Inbox", 0).unwrap()];
        let item = Expectation {
            action: "Pay the 350 fee".into(),
            action_phrase: "pay the 350 fee".into(),
            owner: Owner::You,
            waiting_party: "Other <sam@example.invalid>".into(),
            kind: "request".into(),
            evidence: Anchor {
                message: all[0].input.handle.clone(),
                block: 0,
                quote: "Can we meet?".into(),
                context: "Can we meet?".into(),
            },
            deadline: None,
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
        (all, item)
    }

    #[test]
    fn close_passed_events_marks_matching_open_expectations() {
        let mut event_mail = synthetic("Calendar invitation.", 1, "event");
        event_mail.subject =
            "Invitation: Design workshop @ Fri Aug 21, 2026 11am - 12pm (UTC)".into();
        let event_message = prepare(&event_mail, "Inbox", 1).unwrap();
        let (mut messages, mut item) = closure_test_messages();
        messages[0].input.timestamp = timestamp("2026-08-01T00:00:00Z");
        item.event = Some(Anchor {
            message: item.evidence.message.clone(),
            block: 0,
            quote: "the design workshop".into(),
            context: String::new(),
        });
        messages.push(event_message);
        let mut result = ScanResult {
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
        };
        close_passed_events(&mut result, &messages, timestamp("2026-08-22T00:00:00Z"));
        let passed = result.analysis.items[0].event_passed.as_ref().unwrap();
        assert_eq!(passed.name, "design workshop");
        assert_eq!(passed.end, timestamp("2026-08-21T12:00:00Z"));
        assert_eq!(passed.message_handle, "m1");
        assert_eq!(result.event_closures, 1);
        assert_eq!(
            result.conversation_notes.last().unwrap(),
            "Event index: 1 events learned (0 from meeting messages, 1 from subjects); \
1 expectations named an event, 0 carried an event time, 1 matched the index, 1 closed."
        );
    }

    #[test]
    fn close_passed_events_always_pushes_a_coverage_note_even_when_nothing_is_learned() {
        let (messages, item) = closure_test_messages();
        let mut result = ScanResult {
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
        };
        close_passed_events(&mut result, &messages, timestamp("2026-08-22T00:00:00Z"));
        assert_eq!(
            result.conversation_notes.last().unwrap(),
            "Event index: 0 events learned (0 from meeting messages, 0 from subjects); \
0 expectations named an event, 0 carried an event time, 0 matched the index, 0 closed."
        );
    }

    #[test]
    fn close_passed_events_skips_the_note_when_the_scan_was_cancelled() {
        let (messages, item) = closure_test_messages();
        let mut result = ScanResult {
            analysis: Expectations {
                items: vec![item],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            analyzed: 1,
            total: 1,
            cancelled: true,
            conversation_notes: vec![],
            cross_thread_closures: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            closure_pass_failure: None,
        };
        close_passed_events(&mut result, &messages, timestamp("2026-08-22T00:00:00Z"));
        assert!(result.conversation_notes.is_empty());
    }

    /// The workshop's date is stated only in an email body -- no invitation
    /// or calendar subject exists for it, so the event index has no entry
    /// at all -- so closing it depends entirely on the model-supplied
    /// `event_time` phrase, classified directly. Mirrors
    /// `review_ui::tests::card_offset_comes_from_the_deadline_message_timestamp`'s
    /// pattern of computing the expected offset via the same real-local-time
    /// helper under test, so this stays deterministic across machine
    /// timezones.
    #[test]
    fn close_passed_events_closes_from_a_stated_event_time_with_no_index_entry() {
        let (mut messages, mut item) = closure_test_messages();
        messages[0].input.timestamp = timestamp("2026-09-02T12:00:00Z"); // Wednesday
        item.event = Some(Anchor {
            message: item.evidence.message.clone(),
            block: 0,
            quote: "the workshop".into(),
            context: String::new(),
        });
        item.event_time = Some(Anchor {
            message: item.evidence.message.clone(),
            block: 0,
            quote: "Friday".into(),
            context: String::new(),
        });
        let now = timestamp("2026-09-15T00:00:00Z"); // well past that Friday, any timezone
        let offset = local_offset_seconds(messages[0].input.timestamp, 0);
        let expected_end = match classify("Friday", messages[0].input.timestamp, now, offset) {
            DeadlineView::DueDate { day, past: true } => day * 86_400,
            other => panic!("expected a past DueDate, got {other:?}"),
        };
        let mut result = ScanResult {
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
        };
        close_passed_events(&mut result, &messages, now);
        let passed = result.analysis.items[0].event_passed.as_ref().unwrap();
        assert_eq!(passed.name, "the workshop");
        assert_eq!(passed.end, expected_end);
        assert_eq!(passed.message_handle, messages[0].input.handle);
        assert_eq!(result.event_closures, 1);
    }

    #[test]
    fn close_passed_events_names_a_stated_event_time_generically_with_no_event_anchor() {
        let (mut messages, mut item) = closure_test_messages();
        messages[0].input.timestamp = timestamp("2026-09-02T12:00:00Z"); // Wednesday
        item.event_time = Some(Anchor {
            message: item.evidence.message.clone(),
            block: 0,
            quote: "Friday".into(),
            context: String::new(),
        });
        let now = timestamp("2026-09-15T00:00:00Z");
        let mut result = ScanResult {
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
        };
        close_passed_events(&mut result, &messages, now);
        let passed = result.analysis.items[0].event_passed.as_ref().unwrap();
        assert_eq!(passed.name, "event");
        assert_eq!(result.event_closures, 1);
    }

    #[test]
    fn waiting_party_address_table() {
        let cases: &[(&str, Option<&str>)] = &[
            ("Alex <alex@example.invalid>", Some("alex@example.invalid")),
            ("Alex <ALEX@Example.Invalid>", Some("alex@example.invalid")),
            ("alex@example.invalid", Some("alex@example.invalid")),
            (
                "Alex Rivera alex@example.invalid",
                Some("alex@example.invalid"),
            ),
            ("Not established", None),
            ("Alex Rivera", None),
            ("<>", None),
            ("", None),
        ];
        for (input, expected) in cases {
            assert_eq!(
                waiting_party_address(input),
                expected.map(str::to_string),
                "input: {input:?}"
            );
        }
    }

    #[test]
    fn closure_candidates_filters_by_account_sender_conversation_timestamp_and_address() {
        let (mut all, item) = closure_test_messages();
        let evidence_conversation = "c1";
        let evidence_timestamp = all[0].input.timestamp;

        let valid = reply_to("sam@example.invalid", "v-1", "c2", "acct", "Fee");
        all.push(prepare(&valid, "Sent", all.len()).unwrap());

        let wrong_account = reply_to("sam@example.invalid", "w-1", "c2", "acct-two", "Fee");
        all.push(prepare(&wrong_account, "Sent", all.len()).unwrap());

        let same_conversation = reply_to("sam@example.invalid", "s-1", "c1", "acct", "Fee");
        all.push(prepare(&same_conversation, "Sent", all.len()).unwrap());

        let not_from_user = request_from("sam@example.invalid", "n-1", "c3", "acct", "Fee");
        all.push(prepare(&not_from_user, "Inbox", all.len()).unwrap());

        let mut earlier = reply_to("sam@example.invalid", "e-1", "c4", "acct", "Fee");
        earlier.received = "2026-08-01T12:00:00Z".into();
        all.push(prepare(&earlier, "Sent", all.len()).unwrap());

        let wrong_address = reply_to("dana@example.invalid", "d-1", "c5", "acct", "Fee");
        all.push(prepare(&wrong_address, "Sent", all.len()).unwrap());

        let candidates = closure_candidates(
            &item,
            &all,
            "acct",
            evidence_conversation,
            evidence_timestamp,
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, "v-1");
    }

    #[test]
    fn closure_candidates_keeps_newest_eight_in_ascending_order() {
        let (mut all, item) = closure_test_messages();
        let evidence_conversation = "c1";
        let evidence_timestamp = all[0].input.timestamp;
        let mut expected_ids = vec![];
        for day in 2..=11u32 {
            let mut m = reply_to(
                "sam@example.invalid",
                &format!("c-{day}"),
                &format!("c{day}"),
                "acct",
                "Fee",
            );
            m.received = format!("2026-09-{day:02}T12:00:00Z");
            all.push(prepare(&m, "Sent", all.len()).unwrap());
            // The newest 8 of days 2..=11 are days 4..=11 -- days 2 and 3
            // are the oldest two and must be dropped.
            if day >= 4 {
                expected_ids.push(format!("c-{day}"));
            }
        }
        let candidates = closure_candidates(
            &item,
            &all,
            "acct",
            evidence_conversation,
            evidence_timestamp,
        );
        assert_eq!(candidates.len(), 8);
        let ids: Vec<&str> = candidates.iter().map(|m| m.id.as_str()).collect();
        let expected: Vec<&str> = expected_ids.iter().map(String::as_str).collect();
        assert_eq!(ids, expected);
        assert!(
            candidates
                .windows(2)
                .all(|w| w[0].input.timestamp <= w[1].input.timestamp)
        );
    }

    #[test]
    fn scan_closures_resolves_open_request_and_counts_it() {
        let (mut all, item) = closure_test_messages();
        let valid = reply_to("sam@example.invalid", "v-1", "c2", "acct", "Fee");
        all.push(prepare(&valid, "Sent", all.len()).unwrap());

        let mut result = ScanResult {
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
        };
        let resolved_anchor = Anchor {
            message: all.last().unwrap().input.handle.clone(),
            block: 0,
            quote: "Sure, let's do it.".into(),
            context: "Sure, let's do it.".into(),
        };
        let mut calls = 0;
        let progress = ScanProgress::default();
        scan_closures(&all, &progress, &mut result, |_, _, _| {
            calls += 1;
            Ok(Some((resolved_anchor.clone(), ResolutionKind::Completed)))
        });
        assert_eq!(calls, 1);
        assert_eq!(result.cross_thread_closures, 1);
        assert!(result.analysis.items[0].resolution.is_some());
        assert!(result.analysis.items[0].cross_thread);
        assert_eq!(
            result.analysis.items[0].resolution_kind,
            Some(ResolutionKind::Completed)
        );
        assert!(result.conversation_notes.iter().any(|n| {
            n.contains("Closing evidence found in another conversation for 1 expectation")
        }));
        assert_eq!(progress.total.load(Ordering::Relaxed), 1);
        assert_eq!(progress.processed.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn scan_closures_progress_tracks_eligible_items_including_skipped_ones() {
        // One item resolves, one has no candidates (skipped without ever
        // reaching the provider): `progress.total` grows by the eligible
        // count (2) and `progress.processed` reaches that same total.
        let (mut all, item1) = closure_test_messages();
        let valid1 = reply_to("sam@example.invalid", "v-1", "c2", "acct", "Fee");
        all.push(prepare(&valid1, "Sent", all.len()).unwrap());

        let evidence2 = request_from("dana@example.invalid", "req-2", "c6", "acct", "Fee 2");
        all.push(prepare(&evidence2, "Inbox", all.len()).unwrap());
        let evidence2_handle = all.last().unwrap().input.handle.clone();

        let mut item2 = item1.clone();
        item2.evidence.message = evidence2_handle;
        item2.waiting_party = "Other <dana@example.invalid>".into();
        // No message replies to dana@example.invalid: item2 has no
        // candidates and is skipped without reaching the provider.

        let mut result = ScanResult {
            analysis: Expectations {
                items: vec![item1, item2],
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
        };
        let resolved_anchor = Anchor {
            message: all[1].input.handle.clone(),
            block: 0,
            quote: "Sure, let's do it.".into(),
            context: "Sure, let's do it.".into(),
        };
        let progress = ScanProgress::default();
        progress.total.store(2, Ordering::Relaxed);
        scan_closures(&all, &progress, &mut result, |_, _, _| {
            Ok(Some((resolved_anchor.clone(), ResolutionKind::Completed)))
        });
        assert_eq!(progress.total.load(Ordering::Relaxed), 4);
        assert_eq!(progress.processed.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn scan_closures_stops_when_cancelled_between_calls() {
        let (mut all, item1) = closure_test_messages();
        let valid1 = reply_to("sam@example.invalid", "v-1", "c2", "acct", "Fee");
        all.push(prepare(&valid1, "Sent", all.len()).unwrap());

        // A different waiting-party address than item1's: reusing
        // "sam@example.invalid" across four separate conversation groups
        // here would make it look like a shared mailbox/list (see
        // `is_shared_mailbox_address`) and get excluded before ever
        // reaching the provider, which is not what this test is about.
        let evidence2 = request_from("dana@example.invalid", "req-2", "c6", "acct", "Fee 2");
        all.push(prepare(&evidence2, "Inbox", all.len()).unwrap());
        let evidence2_handle = all.last().unwrap().input.handle.clone();
        let valid2 = reply_to("dana@example.invalid", "v-2", "c7", "acct", "Fee 2");
        all.push(prepare(&valid2, "Sent", all.len()).unwrap());

        let mut item2 = item1.clone();
        item2.evidence.message = evidence2_handle;
        item2.waiting_party = "Other <dana@example.invalid>".into();

        let mut result = ScanResult {
            analysis: Expectations {
                items: vec![item1, item2],
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
        };
        let progress = ScanProgress::default();
        let mut calls = 0;
        scan_closures(&all, &progress, &mut result, |_, _, _| {
            calls += 1;
            progress.cancel.store(true, Ordering::Relaxed);
            Ok(None)
        });
        assert_eq!(calls, 1);
        assert!(result.cancelled);
    }

    #[test]
    fn scan_closures_transport_error_is_recorded_separately_and_stops_only_this_pass() {
        let (mut all, item) = closure_test_messages();
        let valid = reply_to("sam@example.invalid", "v-1", "c2", "acct", "Fee");
        all.push(prepare(&valid, "Sent", all.len()).unwrap());
        let mut result = ScanResult {
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
        };
        scan_closures(&all, &ScanProgress::default(), &mut result, |_, _, _| {
            Err(ProviderError::RateLimited)
        });
        // A closure-pass failure is content-free diagnostic text, kept apart
        // from `failures` (which counts unanalyzed conversations -- no
        // conversation went unanalyzed here).
        assert!(result.failures.is_empty());
        assert!(
            result
                .closure_pass_failure
                .as_deref()
                .is_some_and(|f| f.starts_with("Cross-thread closure pass stopped: ")),
            "closure_pass_failure: {:?}",
            result.closure_pass_failure
        );
        assert_eq!(result.cross_thread_closures, 0);
        assert!(!result.cancelled);
    }

    #[test]
    fn scan_closures_continues_past_a_non_transport_error_to_the_next_item() {
        // An InputTooLarge on item 1 must not prevent item 2 from closing:
        // only the transport-class error set stops the pass.
        let (mut all, item1) = closure_test_messages();
        let valid1 = reply_to("sam@example.invalid", "v-1", "c2", "acct", "Fee");
        all.push(prepare(&valid1, "Sent", all.len()).unwrap());

        let evidence2 = request_from("dana@example.invalid", "req-2", "c6", "acct", "Fee 2");
        all.push(prepare(&evidence2, "Inbox", all.len()).unwrap());
        let evidence2_handle = all.last().unwrap().input.handle.clone();
        let valid2 = reply_to("dana@example.invalid", "v-2", "c7", "acct", "Fee 2");
        all.push(prepare(&valid2, "Sent", all.len()).unwrap());

        let mut item2 = item1.clone();
        item2.evidence.message = evidence2_handle;
        item2.waiting_party = "Other <dana@example.invalid>".into();

        let mut result = ScanResult {
            analysis: Expectations {
                items: vec![item1, item2],
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
        };
        let resolved_anchor = Anchor {
            message: all.last().unwrap().input.handle.clone(),
            block: 0,
            quote: "Sure, let's do it.".into(),
            context: "Sure, let's do it.".into(),
        };
        let mut calls = 0;
        scan_closures(&all, &ScanProgress::default(), &mut result, |_, _, _| {
            calls += 1;
            if calls == 1 {
                Err(ProviderError::InputTooLarge)
            } else {
                Ok(Some((resolved_anchor.clone(), ResolutionKind::Completed)))
            }
        });
        assert_eq!(calls, 2, "the second item must still be attempted");
        assert!(result.closure_pass_failure.is_none());
        assert!(result.failures.is_empty());
        assert!(result.analysis.items[0].resolution.is_none());
        assert!(result.analysis.items[1].resolution.is_some());
        assert_eq!(result.cross_thread_closures, 1);
    }

    #[test]
    fn scan_closures_caps_calls_at_forty_and_notes_it() {
        let (mut all, base_item) = closure_test_messages();
        let mut items = vec![];
        for n in 0..45u32 {
            let address = format!("party{n}@example.invalid");
            let evidence = request_from(
                &address,
                &format!("req-{n}"),
                &format!("c-req-{n}"),
                "acct",
                "Fee",
            );
            all.push(prepare(&evidence, "Inbox", all.len()).unwrap());
            let evidence_handle = all.last().unwrap().input.handle.clone();
            let reply = reply_to(
                &address,
                &format!("v-{n}"),
                &format!("c-reply-{n}"),
                "acct",
                "Fee",
            );
            all.push(prepare(&reply, "Sent", all.len()).unwrap());
            let mut item = base_item.clone();
            item.evidence.message = evidence_handle;
            item.waiting_party = format!("Other <{address}>");
            items.push(item);
        }
        let mut result = ScanResult {
            analysis: Expectations {
                items,
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            analyzed: 45,
            total: 45,
            cancelled: false,
            conversation_notes: vec![],
            cross_thread_closures: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            closure_pass_failure: None,
        };
        let mut calls = 0;
        scan_closures(&all, &ScanProgress::default(), &mut result, |_, _, _| {
            calls += 1;
            Ok(None)
        });
        assert_eq!(
            calls, MAX_CLOSURE_CALLS,
            "the cap must bind at MAX_CLOSURE_CALLS"
        );
        let expected_note = format!(
            "Cross-thread closure checks were capped at {MAX_CLOSURE_CALLS} open requests this scan."
        );
        assert!(result.conversation_notes.contains(&expected_note));
    }

    #[test]
    fn scan_closures_skips_a_shared_mailbox_or_list_waiting_party() {
        // list@example.invalid is the waiting party on the open item, but it
        // also appears in >=3 of the account's conversation groups and in
        // at least half of them -- a distribution list or shared mailbox,
        // not a genuine individual correspondent -- so the item must be
        // skipped without ever reaching the provider. Six (not three) other
        // conversations, plus the evidence conversation and the reply, give
        // the account 8 total conversation groups, clearing
        // `is_shared_mailbox_address`'s 8-group floor.
        let (mut all, mut item) = closure_test_messages();
        item.waiting_party = "List <list@example.invalid>".into();
        for n in 0..6u32 {
            let mail = request_from(
                "list@example.invalid",
                &format!("other-{n}"),
                &format!("other-c{n}"),
                "acct",
                "Other business",
            );
            all.push(prepare(&mail, "Inbox", all.len()).unwrap());
        }
        let reply = reply_to("list@example.invalid", "v-1", "c2", "acct", "Fee");
        all.push(prepare(&reply, "Sent", all.len()).unwrap());

        let mut result = ScanResult {
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
        };
        let mut calls = 0;
        scan_closures(&all, &ScanProgress::default(), &mut result, |_, _, _| {
            calls += 1;
            Ok(None)
        });
        assert_eq!(
            calls, 0,
            "a shared-mailbox waiting party must never reach the provider"
        );
        assert_eq!(result.cross_thread_closures, 0);
    }

    #[test]
    fn is_shared_mailbox_address_requires_at_least_eight_groups() {
        // Pure-function table test for the 8-group floor: an address that
        // would otherwise clear the per-address threshold (appears in every
        // group, so `count * 2 >= total` trivially holds) must still read as
        // NOT shared until the account has at least 8 conversation groups
        // total.
        for (total, count, expected) in [
            (1, 1, false),
            (2, 2, false),
            (3, 3, false),
            (7, 7, false),
            (8, 3, false), // count*2 (6) < total (8): below the per-address threshold
            (8, 4, true),  // count*2 (8) >= total (8), and count >= 3
            (8, 8, true),
            (9, 3, false), // count*2 (6) < total (9)
            (9, 5, true),  // count*2 (10) >= total (9), and count >= 3
        ] {
            let mut groups_per_account = BTreeMap::new();
            groups_per_account.insert("acct", total);
            let mut address_group_counts = BTreeMap::new();
            address_group_counts.insert(("acct", "list@example.invalid"), count);
            assert_eq!(
                is_shared_mailbox_address(
                    "acct",
                    "list@example.invalid",
                    &groups_per_account,
                    &address_group_counts,
                ),
                expected,
                "total={total} count={count}"
            );
        }
    }

    #[test]
    fn scan_closures_skips_the_pass_when_the_primary_scan_broke_on_a_transport_error() {
        let (mut all, item) = closure_test_messages();
        let valid = reply_to("sam@example.invalid", "v-1", "c2", "acct", "Fee");
        all.push(prepare(&valid, "Sent", all.len()).unwrap());
        let mut result = ScanResult {
            analysis: Expectations {
                items: vec![item],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec!["Conversation 1 (1 messages): rate limited".into()],
            analyzed: 0,
            total: 1,
            cancelled: false,
            conversation_notes: vec![],
            cross_thread_closures: 0,
            event_closures: 0,
            primary_scan_transport_error: true,
            closure_pass_failure: None,
        };
        let progress = ScanProgress::default();
        let mut calls = 0;
        scan_closures(&all, &progress, &mut result, |_, _, _| {
            calls += 1;
            Ok(None)
        });
        assert_eq!(calls, 0, "the closure pass must not run at all");
        assert_eq!(progress.total.load(Ordering::Relaxed), 0);
        assert_eq!(result.cross_thread_closures, 0);
    }

    #[test]
    fn scan_closures_skips_items_that_are_not_open_you_owned_requests() {
        let (mut all, base_item) = closure_test_messages();
        let valid = reply_to("sam@example.invalid", "v-1", "c2", "acct", "Fee");
        all.push(prepare(&valid, "Sent", all.len()).unwrap());

        let mut already_resolved = base_item.clone();
        already_resolved.resolution = Some(already_resolved.evidence.clone());
        let mut not_a_request = base_item.clone();
        not_a_request.kind = "promise".into();
        let mut not_owned_by_you = base_item.clone();
        not_owned_by_you.owner = Owner::Team;
        let mut unresolvable_address = base_item.clone();
        unresolvable_address.waiting_party = "Not established".into();

        let mut result = ScanResult {
            analysis: Expectations {
                items: vec![
                    already_resolved,
                    not_a_request,
                    not_owned_by_you,
                    unresolvable_address,
                ],
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
        };
        let mut calls = 0;
        scan_closures(&all, &ScanProgress::default(), &mut result, |_, _, _| {
            calls += 1;
            Ok(None)
        });
        assert_eq!(calls, 0, "none of these items should reach the provider");
        assert_eq!(result.cross_thread_closures, 0);
    }

    #[test]
    fn split_conversation_ids_with_shared_subject_and_participant_are_merged() {
        let request = request_from(
            "sam@example.invalid",
            "req-1",
            "c1",
            "acct",
            "Alex and Sam discuss quarterly planning",
        );
        let reply = reply_to(
            "sam@example.invalid",
            "reply-1",
            "c2",
            "acct",
            "Re: Alex and Sam discuss quarterly planning",
        );
        let mut messages = vec![
            prepare(&request, "Inbox", 0).unwrap(),
            prepare(&reply, "Sent", 1).unwrap(),
        ];
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 1);
        assert!(messages.iter().all(|m| m.conversation == "c1"));
    }

    #[test]
    fn same_subject_disjoint_participants_are_not_merged() {
        let request = request_from(
            "sam@example.invalid",
            "req-1",
            "c1",
            "acct",
            "Alex and Sam discuss quarterly planning",
        );
        let reply = reply_to(
            "dana@example.invalid",
            "reply-1",
            "c2",
            "acct",
            "Re: Alex and Sam discuss quarterly planning",
        );
        let mut messages = vec![
            prepare(&request, "Inbox", 0).unwrap(),
            prepare(&reply, "Sent", 1).unwrap(),
        ];
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 0);
        assert_eq!(messages[0].conversation, "c1");
        assert_eq!(messages[1].conversation, "c2");
    }

    #[test]
    fn same_subject_and_participants_different_account_are_not_merged() {
        let request = request_from(
            "sam@example.invalid",
            "req-1",
            "c1",
            "acct-one",
            "Alex and Sam discuss quarterly planning",
        );
        let reply = reply_to(
            "sam@example.invalid",
            "reply-1",
            "c2",
            "acct-two",
            "Re: Alex and Sam discuss quarterly planning",
        );
        let mut messages = vec![
            prepare(&request, "Inbox", 0).unwrap(),
            prepare(&reply, "Sent", 1).unwrap(),
        ];
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 0);
        assert_eq!(messages[0].conversation, "c1");
        assert_eq!(messages[1].conversation, "c2");
    }

    #[test]
    fn empty_normalized_subject_never_merges() {
        let request = request_from("sam@example.invalid", "req-1", "c1", "acct", "");
        let reply = reply_to("sam@example.invalid", "reply-1", "c2", "acct", "");
        let mut messages = vec![
            prepare(&request, "Inbox", 0).unwrap(),
            prepare(&reply, "Sent", 1).unwrap(),
        ];
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 0);
        assert_eq!(messages[0].conversation, "c1");
        assert_eq!(messages[1].conversation, "c2");
    }

    #[test]
    fn three_groups_chain_merge_through_a_bridging_message() {
        let a = request_from("p1@example.invalid", "a-1", "cA", "acct", "Widget Renewal");
        let mut b = mail(
            "b-1",
            "cB",
            "acct",
            "RE: Widget Renewal",
            MailItem {
                body: "Looping in both of you.".into(),
                sender: "User <user@example.invalid>".into(),
                sender_address: "user@example.invalid".into(),
                received: "2026-09-02T12:00:00Z".into(),
                to: vec!["p1@example.invalid".into(), "p2@example.invalid".into()],
                ..MailItem::default()
            },
        );
        b.own_addresses = vec!["user@example.invalid".into()];
        let c = request_from(
            "p2@example.invalid",
            "c-1",
            "cC",
            "acct",
            "Fwd: Widget Renewal",
        );
        let mut messages = vec![
            prepare(&a, "Inbox", 0).unwrap(),
            prepare(&b, "Sent", 1).unwrap(),
            prepare(&c, "Inbox", 2).unwrap(),
        ];
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 2);
        assert!(messages.iter().all(|m| m.conversation == "cA"));
    }

    #[test]
    fn calendar_invitation_subjects_never_merge_across_recurring_instances() {
        // Both instances normalize to the same bare subject ("weekly
        // sync") once the date/time is stripped, but the raw "Invitation:"
        // prefix means that bare subject was never the real thread
        // identity -- these are two different meeting occurrences.
        let first = mail(
            "cal-1",
            "c1",
            "acct",
            "Invitation: Weekly sync @ Mon Sep 7, 2026 9am - 10am (PDT)",
            MailItem {
                body: "You have been invited.".into(),
                sender: "Alex <alex@example.invalid>".into(),
                sender_address: "alex@example.invalid".into(),
                received: "2026-09-07T09:00:00Z".into(),
                to: vec!["user@example.invalid".into()],
                ..MailItem::default()
            },
        );
        let second = mail(
            "cal-2",
            "c2",
            "acct",
            "Invitation: Weekly sync @ Mon Sep 14, 2026 9am - 10am (PDT)",
            MailItem {
                body: "You have been invited.".into(),
                sender: "Alex <alex@example.invalid>".into(),
                sender_address: "alex@example.invalid".into(),
                received: "2026-09-14T09:00:00Z".into(),
                to: vec!["user@example.invalid".into()],
                ..MailItem::default()
            },
        );
        let mut messages = vec![
            prepare(&first, "Inbox", 0).unwrap(),
            prepare(&second, "Inbox", 1).unwrap(),
        ];
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 0);
        assert_eq!(messages[0].conversation, "c1");
        assert_eq!(messages[1].conversation, "c2");
    }

    #[test]
    fn raw_subject_has_calendar_prefix_walks_reply_and_forward_chain() {
        // Regression: the old implementation inspected only the very first
        // prefix ("re"/"fwd"), so a calendar prefix hiding one layer deeper
        // in the reply/forward chain went undetected.
        assert!(raw_subject_has_calendar_prefix(
            "Re: Invitation: Weekly sync @ Mon Sep 7, 2026 9am - 10am (PDT)"
        ));
        assert!(raw_subject_has_calendar_prefix(
            "Fwd: Updated invitation: Standup @ Tue Sep 8, 2026 9am (PDT)"
        ));
    }

    #[test]
    fn calendar_prefix_behind_a_reply_prefix_still_blocks_merge_across_instances() {
        // Same shape as `calendar_invitation_subjects_never_merge_across_recurring_instances`,
        // but with the calendar prefix hidden behind a leading "Re:" on the
        // first instance -- the exact case the old first-prefix-only check
        // missed. The two instances are only a day apart (well inside the
        // 3-day merge window), so only the calendar-prefix guard, not the
        // time gap, can be responsible for keeping them apart.
        let first = mail(
            "cal-1",
            "c1",
            "acct",
            "Re: Invitation: Weekly sync @ Mon Sep 7, 2026 9am - 10am (PDT)",
            MailItem {
                body: "You have been invited.".into(),
                sender: "Alex <alex@example.invalid>".into(),
                sender_address: "alex@example.invalid".into(),
                received: "2026-09-07T09:00:00Z".into(),
                to: vec!["user@example.invalid".into()],
                ..MailItem::default()
            },
        );
        let second = mail(
            "cal-2",
            "c2",
            "acct",
            "Invitation: Weekly sync @ Tue Sep 8, 2026 9am - 10am (PDT)",
            MailItem {
                body: "You have been invited.".into(),
                sender: "Alex <alex@example.invalid>".into(),
                sender_address: "alex@example.invalid".into(),
                received: "2026-09-08T09:00:00Z".into(),
                to: vec!["user@example.invalid".into()],
                ..MailItem::default()
            },
        );
        let mut messages = vec![
            prepare(&first, "Inbox", 0).unwrap(),
            prepare(&second, "Inbox", 1).unwrap(),
        ];
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 0);
        assert_eq!(messages[0].conversation, "c1");
        assert_eq!(messages[1].conversation, "c2");
    }

    #[test]
    fn contract_review_draft_and_final_are_not_merged() {
        let request = request_from(
            "sam@example.invalid",
            "req-1",
            "c1",
            "acct",
            "Contract review (draft)",
        );
        let reply = reply_to(
            "sam@example.invalid",
            "reply-1",
            "c2",
            "acct",
            "Contract review (final)",
        );
        let mut messages = vec![
            prepare(&request, "Inbox", 0).unwrap(),
            prepare(&reply, "Sent", 1).unwrap(),
        ];
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 0);
        assert_eq!(messages[0].conversation, "c1");
        assert_eq!(messages[1].conversation, "c2");
    }

    #[test]
    fn groups_more_than_three_days_apart_do_not_merge() {
        let request = request_from(
            "sam@example.invalid",
            "req-1",
            "c1",
            "acct",
            "Alex and Sam discuss quarterly planning",
        );
        let mut reply = reply_to(
            "sam@example.invalid",
            "reply-1",
            "c2",
            "acct",
            "Re: Alex and Sam discuss quarterly planning",
        );
        reply.received = "2026-09-11T12:00:00Z".into();
        let mut messages = vec![
            prepare(&request, "Inbox", 0).unwrap(),
            prepare(&reply, "Sent", 1).unwrap(),
        ];
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 0);
        assert_eq!(messages[0].conversation, "c1");
        assert_eq!(messages[1].conversation, "c2");
    }

    #[test]
    fn group_source_never_merges() {
        // Same setup as the positive merge case
        // (`split_conversation_ids_with_shared_subject_and_participant_are_merged`),
        // but with one message flagged as a group source: group sources
        // keep their own thread identity and must never merge.
        let request = request_from(
            "sam@example.invalid",
            "req-1",
            "c1",
            "acct",
            "Alex and Sam discuss quarterly planning",
        );
        let reply = reply_to(
            "sam@example.invalid",
            "reply-1",
            "c2",
            "acct",
            "Re: Alex and Sam discuss quarterly planning",
        );
        let mut messages = vec![
            prepare(&request, "Inbox", 0).unwrap(),
            prepare(&reply, "Sent", 1).unwrap(),
        ];
        messages[0].input.team = true;
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 0);
        assert_eq!(messages[0].conversation, "c1");
        assert_eq!(messages[1].conversation, "c2");
    }

    #[test]
    fn shared_mailbox_address_common_to_every_group_does_not_count_as_intersection() {
        // Three groups all carbon-copy a distribution list
        // ("list@example.invalid"). That address alone must not bridge
        // group C to groups A/B: only A and B additionally share a real
        // outside participant ("sam@example.invalid"). Five unrelated
        // filler groups (also carbon-copying the list, under a distinct
        // subject that never matches A/B/C's) pad the account to 8 total
        // conversation groups -- `is_shared_mailbox_address`'s floor -- so
        // "list" is still recognized as shared (8 of 8 groups) rather than
        // being let through just because the account is small.
        let a = mail(
            "a-1",
            "cA",
            "acct",
            "Alex and Sam discuss quarterly planning",
            MailItem {
                body: "Can we meet?".into(),
                sender: "Sam <sam@example.invalid>".into(),
                sender_address: "sam@example.invalid".into(),
                received: "2026-09-01T12:00:00Z".into(),
                to: vec!["user@example.invalid".into(), "list@example.invalid".into()],
                ..MailItem::default()
            },
        );
        let b = mail(
            "b-1",
            "cB",
            "acct",
            "RE: Alex and Sam discuss quarterly planning",
            MailItem {
                body: "Sure, let's do it.".into(),
                sender: "User <user@example.invalid>".into(),
                sender_address: "user@example.invalid".into(),
                received: "2026-09-02T12:00:00Z".into(),
                to: vec!["sam@example.invalid".into(), "list@example.invalid".into()],
                ..MailItem::default()
            },
        );
        let c = mail(
            "c-1",
            "cC",
            "acct",
            "FW: Alex and Sam discuss quarterly planning",
            MailItem {
                body: "FYI.".into(),
                sender: "Dana <dana@example.invalid>".into(),
                sender_address: "dana@example.invalid".into(),
                received: "2026-09-01T13:00:00Z".into(),
                to: vec!["user@example.invalid".into(), "list@example.invalid".into()],
                ..MailItem::default()
            },
        );
        let mut messages = vec![
            prepare(&a, "Inbox", 0).unwrap(),
            prepare(&b, "Sent", 1).unwrap(),
            prepare(&c, "Inbox", 2).unwrap(),
        ];
        for n in 0..5u32 {
            let filler = mail(
                &format!("filler-{n}"),
                &format!("cF{n}"),
                "acct",
                "Unrelated administrative note",
                MailItem {
                    body: "FYI only.".into(),
                    sender: "List <list@example.invalid>".into(),
                    sender_address: "list@example.invalid".into(),
                    received: "2026-09-03T12:00:00Z".into(),
                    to: vec!["user@example.invalid".into()],
                    ..MailItem::default()
                },
            );
            messages.push(prepare(&filler, "Inbox", messages.len()).unwrap());
        }
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 1);
        assert_eq!(messages[0].conversation, messages[1].conversation);
        assert_ne!(messages[0].conversation, messages[2].conversation);
    }

    #[test]
    fn address_in_four_of_six_groups_is_excluded_and_does_not_bridge() {
        // Regression: the old rule only excluded an address that appeared
        // in EVERY group of the account, which a real shared mailbox or
        // distribution list essentially never does. Here "list@example.invalid"
        // appears in 4 of 8 groups (>=3 and at least half of 8 -- padded
        // from the original 6 to clear `is_shared_mailbox_address`'s
        // 8-group floor), so it must still be excluded from the
        // intersection check -- none of these groups share any other
        // address, so nothing should merge.
        let subject = "Alex and Sam discuss quarterly planning";
        let list = "list@example.invalid";
        let with_list = |id: &str, conv: &str, unique: &str| {
            mail(
                id,
                conv,
                "acct",
                subject,
                MailItem {
                    body: "Can we meet?".into(),
                    sender: "User <user@example.invalid>".into(),
                    sender_address: "user@example.invalid".into(),
                    received: "2026-09-01T12:00:00Z".into(),
                    to: vec![list.into(), unique.into()],
                    ..MailItem::default()
                },
            )
        };
        let without_list = |id: &str, conv: &str, unique: &str| {
            mail(
                id,
                conv,
                "acct",
                subject,
                MailItem {
                    body: "Can we meet?".into(),
                    sender: "User <user@example.invalid>".into(),
                    sender_address: "user@example.invalid".into(),
                    received: "2026-09-01T12:00:00Z".into(),
                    to: vec![unique.into()],
                    ..MailItem::default()
                },
            )
        };
        let items = [
            with_list("g1", "c1", "u1@example.invalid"),
            with_list("g2", "c2", "u2@example.invalid"),
            with_list("g3", "c3", "u3@example.invalid"),
            with_list("g4", "c4", "u4@example.invalid"),
            without_list("g5", "c5", "v1@example.invalid"),
            without_list("g6", "c6", "v2@example.invalid"),
            without_list("g7", "c7", "v3@example.invalid"),
            without_list("g8", "c8", "v4@example.invalid"),
        ];
        let mut messages: Vec<ReviewMessage> = items
            .iter()
            .enumerate()
            .map(|(i, m)| prepare(m, "Inbox", i).unwrap())
            .collect();
        let merged = merge_threads(&mut messages);
        assert_eq!(
            merged, 0,
            "the shared list address must not bridge any pair"
        );
    }

    #[test]
    fn address_in_two_of_six_groups_still_bridges_merge() {
        // A correspondent that appears in only 2 of the account's groups
        // (below the "at least 3" per-address floor) is never treated as a
        // shared mailbox, so it must still bridge the two groups it links --
        // exactly the ordinary two-thread case. Padded to 8 total groups (up
        // from the original 6) so the account clears
        // `is_shared_mailbox_address`'s 8-group floor and this test actually
        // exercises the per-address sub-threshold, rather than passing
        // merely because the account is too small for the check to apply
        // at all.
        let subject = "Alex and Sam discuss quarterly planning";
        let request = request_from("sam@example.invalid", "req-1", "c1", "acct", subject);
        let reply = reply_to(
            "sam@example.invalid",
            "reply-1",
            "c2",
            "acct",
            &format!("Re: {subject}"),
        );
        let padding = [
            request_from("v1@example.invalid", "p-1", "c3", "acct", subject),
            request_from("v2@example.invalid", "p-2", "c4", "acct", subject),
            request_from("v3@example.invalid", "p-3", "c5", "acct", subject),
            request_from("v4@example.invalid", "p-4", "c6", "acct", subject),
            request_from("v5@example.invalid", "p-5", "c7", "acct", subject),
            request_from("v6@example.invalid", "p-6", "c8", "acct", subject),
        ];
        let mut messages = vec![
            prepare(&request, "Inbox", 0).unwrap(),
            prepare(&reply, "Sent", 1).unwrap(),
        ];
        for (i, m) in padding.iter().enumerate() {
            messages.push(prepare(m, "Inbox", 2 + i).unwrap());
        }
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 1);
        assert_eq!(messages[0].conversation, messages[1].conversation);
    }

    #[test]
    fn merge_winner_is_lexicographically_smallest_id_regardless_of_message_order() {
        // Chain-merges "cZ" -> "cA" -> "cM" through a bridging message that
        // addresses both outside participants, the same shape as
        // `three_groups_chain_merge_through_a_bridging_message`, but with
        // "cZ" (not the eventual winner, "cA") placed first in the input
        // to confirm winner selection does not depend on encounter order.
        let z = request_from(
            "p1@example.invalid",
            "z-1",
            "cZ",
            "acct",
            "Alex and Sam discuss quarterly planning",
        );
        let bridge = mail(
            "a-1",
            "cA",
            "acct",
            "RE: Alex and Sam discuss quarterly planning",
            MailItem {
                body: "Looping in both of you.".into(),
                sender: "User <user@example.invalid>".into(),
                sender_address: "user@example.invalid".into(),
                received: "2026-09-02T12:00:00Z".into(),
                to: vec!["p1@example.invalid".into(), "p2@example.invalid".into()],
                ..MailItem::default()
            },
        );
        let third = request_from(
            "p2@example.invalid",
            "m-1",
            "cM",
            "acct",
            "Fwd: Alex and Sam discuss quarterly planning",
        );
        let mut messages = vec![
            prepare(&z, "Inbox", 0).unwrap(),
            prepare(&bridge, "Sent", 1).unwrap(),
            prepare(&third, "Inbox", 2).unwrap(),
        ];
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 2);
        assert!(messages.iter().all(|m| m.conversation == "cA"));
    }

    #[test]
    fn merged_conversation_is_analyzed_as_one_conversation() {
        let mut messages = vec![
            prepare(&synthetic("Please send the draft.", 0, "c1"), "Inbox", 0).unwrap(),
            prepare(&synthetic("Here is the draft.", 1, "c2"), "Sent", 1).unwrap(),
        ];
        messages[0].other_addresses = vec!["alex@example.invalid".into()];
        messages[1].other_addresses = vec!["alex@example.invalid".into()];
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 1);
        let mut lengths = vec![];
        let result = scan_conversations(&messages, &ScanProgress::default(), |batch| {
            lengths.push(batch.len());
            Ok(Expectations {
                items: vec![],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            })
        });
        assert_eq!(lengths, [2]);
        assert_eq!(result.analyzed, 2);
    }

    #[test]
    fn rejected_conversation_note_includes_all_reasons() {
        let a = prepare(&synthetic("Please send the draft.", 0, "a"), "Inbox", 0).unwrap();
        let b = prepare(&synthetic("Following up.", 1, "a"), "Inbox", 1).unwrap();
        let result = scan_conversations(&[a, b], &ScanProgress::default(), |_| {
            Ok(Expectations {
                items: vec![],
                rejected: 1,
                rejection_reasons: vec!["Reason A.", "Reason B."],
                degraded: 0,
            })
        });
        assert_eq!(result.conversation_notes.len(), 1);
        let note = &result.conversation_notes[0];
        assert!(note.contains("1 rejected"), "note: {note}");
        assert!(note.contains("Reason A."), "note: {note}");
        assert!(note.contains("Reason B."), "note: {note}");
    }

    #[test]
    fn rejected_conversation_note_deduplicates_identical_reasons() {
        let a = prepare(&synthetic("Please send the draft.", 0, "a"), "Inbox", 0).unwrap();
        let b = prepare(&synthetic("Following up.", 1, "a"), "Inbox", 1).unwrap();
        let result = scan_conversations(&[a, b], &ScanProgress::default(), |_| {
            Ok(Expectations {
                items: vec![],
                rejected: 2,
                rejection_reasons: vec!["Reason A.", "Reason A.", "Reason B."],
                degraded: 0,
            })
        });
        assert_eq!(result.conversation_notes.len(), 1);
        let note = &result.conversation_notes[0];
        assert_eq!(note.matches("Reason A.").count(), 1, "note: {note}");
        assert!(note.contains("Reason B."), "note: {note}");
    }

    #[test]
    fn two_party_conversation_with_no_items_gets_a_no_expectations_note() {
        let request = request_from("alex@example.invalid", "req-1", "a", "acct", "Budget");
        let reply = reply_to("alex@example.invalid", "reply-1", "a", "acct", "Re: Budget");
        let a = prepare(&request, "Inbox", 0).unwrap();
        let b = prepare(&reply, "Sent", 1).unwrap();
        let result = scan_conversations(&[a, b], &ScanProgress::default(), |_| {
            Ok(Expectations {
                items: vec![],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            })
        });
        assert_eq!(result.conversation_notes.len(), 1);
        assert!(
            result.conversation_notes[0].contains("no expectations returned"),
            "note: {}",
            result.conversation_notes[0]
        );
    }

    #[test]
    fn conversation_with_more_than_one_counterparty_address_gets_no_note() {
        // Regression: per-message `to`/`cc` checks alone cannot catch a
        // conversation stitched together from messages addressed to
        // different counterparties -- each individual message still looks
        // two-party, but the conversation as a whole is not. Alex and Dana
        // each send one message (satisfying the from_user/!from_user and
        // single-recipient/no-cc checks per message), so only the added
        // union-of-addresses check keeps this from being mistaken for a
        // genuine two-party thread.
        let from_alex = request_from("alex@example.invalid", "req-1", "a", "acct", "Budget");
        let from_dana = request_from("dana@example.invalid", "req-2", "a", "acct", "Budget");
        let reply = reply_to("alex@example.invalid", "reply-1", "a", "acct", "Re: Budget");
        let a = prepare(&from_alex, "Inbox", 0).unwrap();
        let d = prepare(&from_dana, "Inbox", 1).unwrap();
        let b = prepare(&reply, "Sent", 2).unwrap();
        let result = scan_conversations(&[a, d, b], &ScanProgress::default(), |_| {
            Ok(Expectations {
                items: vec![],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            })
        });
        assert!(result.conversation_notes.is_empty());
    }

    #[test]
    fn group_source_with_no_items_gets_no_no_expectations_note() {
        let request = request_from("alex@example.invalid", "req-1", "a", "acct", "Budget");
        let reply = reply_to("alex@example.invalid", "reply-1", "a", "acct", "Re: Budget");
        let mut a = prepare(&request, "Inbox", 0).unwrap();
        a.input.team = true;
        let b = prepare(&reply, "Sent", 1).unwrap();
        let result = scan_conversations(&[a, b], &ScanProgress::default(), |_| {
            Ok(Expectations {
                items: vec![],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            })
        });
        assert!(result.conversation_notes.is_empty());
    }

    #[test]
    fn multi_recipient_conversation_with_no_items_gets_no_no_expectations_note() {
        let mut request = request_from("alex@example.invalid", "req-1", "a", "acct", "Budget");
        request.to = vec!["user@example.invalid".into(), "sam@example.invalid".into()];
        let reply = reply_to("alex@example.invalid", "reply-1", "a", "acct", "Re: Budget");
        let a = prepare(&request, "Inbox", 0).unwrap();
        let b = prepare(&reply, "Sent", 1).unwrap();
        let result = scan_conversations(&[a, b], &ScanProgress::default(), |_| {
            Ok(Expectations {
                items: vec![],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            })
        });
        assert!(result.conversation_notes.is_empty());
    }

    #[test]
    fn cc_present_conversation_with_no_items_gets_no_no_expectations_note() {
        let mut request = request_from("alex@example.invalid", "req-1", "a", "acct", "Budget");
        request.cc = vec!["sam@example.invalid".into()];
        let reply = reply_to("alex@example.invalid", "reply-1", "a", "acct", "Re: Budget");
        let a = prepare(&request, "Inbox", 0).unwrap();
        let b = prepare(&reply, "Sent", 1).unwrap();
        let result = scan_conversations(&[a, b], &ScanProgress::default(), |_| {
            Ok(Expectations {
                items: vec![],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            })
        });
        assert!(result.conversation_notes.is_empty());
    }

    #[test]
    fn single_party_conversation_with_no_items_gets_no_note() {
        let a = prepare(&synthetic("Please send the draft.", 0, "a"), "Inbox", 0).unwrap();
        let b = prepare(&synthetic("Following up.", 1, "a"), "Inbox", 1).unwrap();
        let result = scan_conversations(&[a, b], &ScanProgress::default(), |_| {
            Ok(Expectations {
                items: vec![],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            })
        });
        assert!(result.conversation_notes.is_empty());
    }

    #[test]
    fn subject_snippet_truncates_at_60_chars_without_splitting_a_multibyte_char() {
        // 'é' sits exactly as the 60th character: a byte-based truncation
        // (rather than a char-based one) would either panic slicing mid
        // encoding or silently corrupt it.
        let long_subject = format!("{}é{}", "a".repeat(59), "b".repeat(80));
        let request = mail(
            "req-1",
            "a",
            "acct",
            &long_subject,
            MailItem {
                body: "Can we meet?".into(),
                sender: "Other <other@example.invalid>".into(),
                sender_address: "other@example.invalid".into(),
                received: "2026-09-01T12:00:00Z".into(),
                to: vec!["user@example.invalid".into()],
                ..MailItem::default()
            },
        );
        let reply = mail(
            "reply-1",
            "a",
            "acct",
            &format!("Re: {long_subject}"),
            MailItem {
                body: "Sure.".into(),
                sender: "User <user@example.invalid>".into(),
                sender_address: "user@example.invalid".into(),
                received: "2026-09-02T12:00:00Z".into(),
                to: vec!["other@example.invalid".into()],
                ..MailItem::default()
            },
        );
        let a = prepare(&request, "Inbox", 0).unwrap();
        let b = prepare(&reply, "Sent", 1).unwrap();
        let result = scan_conversations(&[a, b], &ScanProgress::default(), |_| {
            Ok(Expectations {
                items: vec![],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            })
        });
        assert_eq!(result.conversation_notes.len(), 1);
        let expected_snippet: String = long_subject.chars().take(60).collect();
        assert_eq!(expected_snippet.chars().count(), 60);
        assert!(
            result.conversation_notes[0].contains(&expected_snippet),
            "note: {}",
            result.conversation_notes[0]
        );
    }
}
