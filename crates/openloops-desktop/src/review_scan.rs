use openloops_graph::live::{ConnectionError, review::MailItem};
use openloops_inference::{
    blocks::CanonicalBlock,
    canonical::canonicalize_plain,
    message::CanonicalMessage,
    ollama::{
        OllamaCloud, ProviderError,
        expectations::{
            Anchor, ConversationMessage, Expectation, Expectations, Owner, ResolutionKind,
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
    /// Per-conversation diagnostics: one note per conversation that had a
    /// rejection, degraded item, or (for a two-party thread) returned no
    /// expectations at all. In-memory UI text only -- built from message
    /// subjects, so it must never be logged, saved, or emitted by
    /// [`probe`].
    pub conversation_notes: Vec<String>,
    /// Number of open requests resolved by the cross-thread closure pass
    /// (`scan_closures`), using evidence found in a different conversation
    /// than the request itself.
    pub cross_thread_closures: usize,
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
/// other than `evidence_conversation`, for the same account as the
/// evidence message. Sorted chronologically ascending and capped at 8 so
/// the closure prompt stays small.
pub fn closure_candidates<'a>(
    item: &Expectation,
    all: &'a [ReviewMessage],
    evidence_conversation: &str,
    evidence_timestamp: i64,
) -> Vec<&'a ReviewMessage> {
    let Some(address) = waiting_party_address(&item.waiting_party) else {
        return vec![];
    };
    let Some(account) = all
        .iter()
        .find(|m| m.input.handle == item.evidence.message)
        .map(|m| m.account.as_str())
    else {
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
    candidates.sort_by_key(|m| m.input.timestamp);
    candidates.truncate(8);
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
///   group source, and no message with more than one `to` recipient or any
///   `cc`), so a card silently vanishing from that thread can be told apart
///   from a newsletter, group source, or other multi-party mail that never
///   had a single owed action to find.
///
/// Text only, built from in-memory subjects: never call this from `probe`,
/// and never persist or log its output.
fn conversation_note(
    index: usize,
    conversation: &[ConversationMessage],
    analysis: &Expectations,
) -> Option<String> {
    let len = conversation.len();
    let subject = conversation
        .first()
        .map(|m| m.message.subject.as_string())
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
    if analysis.items.is_empty()
        && analysis.rejected == 0
        && conversation.iter().any(|m| !m.from_user)
        && conversation.iter().any(|m| m.from_user)
        && conversation
            .iter()
            .all(|m| !m.team && m.message.to.len() <= 1 && m.message.cc.is_empty())
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
    };
    let mut conversations: BTreeMap<(&str, &str), Vec<ConversationMessage>> = BTreeMap::new();
    for m in messages {
        conversations
            .entry((&m.account, &m.conversation))
            .or_default()
            .push(m.input.clone());
    }
    for (index, mut conversation) in conversations.into_values().enumerate() {
        if progress.cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            break;
        }
        conversation.sort_by_key(|m| m.timestamp);
        match analyze(&conversation) {
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

/// After the primary per-conversation scan, attempts to close any
/// remaining open "you owe someone" requests using completion evidence the
/// signed-in user sent to the same waiting party in a DIFFERENT
/// conversation than the request — evidence `scan_conversations`'s
/// per-conversation analysis never sees. Mutates `result.analysis.items`
/// in place, sets `cross_thread: true` on every item it resolves, and
/// counts them in `result.cross_thread_closures`. A provider error stops
/// only this pass (recorded in `result.failures`), never the whole scan;
/// `progress.cancel` is checked between calls exactly like
/// `scan_conversations`.
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
    for item in &mut result.analysis.items {
        if progress.cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            break;
        }
        if item.resolution.is_some() || item.kind != "request" || item.owner != Owner::You {
            continue;
        }
        let Some(source) = messages
            .iter()
            .find(|m| m.input.handle == item.evidence.message)
        else {
            continue;
        };
        let candidates =
            closure_candidates(item, messages, &source.conversation, source.input.timestamp);
        if candidates.is_empty() {
            continue;
        }
        let inputs: Vec<ConversationMessage> = candidates.iter().map(|m| m.input.clone()).collect();
        match closure(item, source.input.timestamp, &inputs) {
            Ok(Some((anchor, kind))) => {
                item.resolution = Some(anchor);
                item.resolution_kind = Some(kind);
                item.cross_thread = true;
                result.cross_thread_closures += 1;
            }
            Ok(None) => {}
            Err(e) => {
                result
                    .failures
                    .push(format!("Cross-thread closure pass: {e}"));
                break;
            }
        }
    }
    if result.cross_thread_closures > 0 {
        result.conversation_notes.push(format!(
            "Closing evidence found in another conversation for {} expectation(s).",
            result.cross_thread_closures
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
/// a weekday or month name, and what follows that name must itself look
/// date-shaped -- a comma (e.g. "Mon Sep 7, 2026 ..."), a space directly
/// followed by a digit (e.g. "Sep 7 2026"), or a digit within the next 4
/// characters (e.g. "Dec25"). A bare place or product name that happens to
/// start with a weekday/month abbreviation ("Sun Valley Lodge", "May's
/// Diner") satisfies the prefix check but none of these follow-on shapes,
/// so it is correctly left alone.
fn looks_like_calendar_date(after: &str) -> bool {
    let Some(name_len) = strip_weekday_or_month_prefix(after) else {
        return false;
    };
    let rest = &after[name_len..];
    if rest.contains(',') {
        return true;
    }
    if rest
        .char_indices()
        .filter(|&(_, c)| c == ' ')
        .any(|(i, _)| {
            rest[i + 1..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
        })
    {
        return true;
    }
    rest.chars().take(4).any(|c| c.is_ascii_digit())
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

/// True when `subject`'s leading `word:` (or fullwidth `：`) prefix, before
/// any normalization, is one of [`CALENDAR_PREFIXES`]. Used by
/// [`merge_threads`] to keep a group carrying such a subject from ever
/// merging with another, since the calendar prefix means the normalized
/// subject alone does not identify the thread.
fn raw_subject_has_calendar_prefix(subject: &str) -> bool {
    let trimmed = subject.trim_start();
    let Some(sep_idx) = trimmed.find([':', '\u{FF1A}']) else {
        return false;
    };
    let prefix = trimmed[..sep_idx].trim().to_lowercase();
    CALENDAR_PREFIXES.contains(&prefix.as_str())
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
/// one that appears in every group belonging to `account` -- rather than a
/// genuine outside participant linking the two threads. Requires at least
/// 3 groups for the account: with only 2 groups, "every group" and "the
/// other group" are the same set, so the check would otherwise disqualify
/// the ordinary case of two threads linked by one real correspondent.
fn is_shared_mailbox_address(
    account: &str,
    address: &str,
    groups_per_account: &BTreeMap<String, usize>,
    address_group_counts: &BTreeMap<(String, String), usize>,
) -> bool {
    let total = groups_per_account.get(account).copied().unwrap_or(0);
    if total <= 2 {
        return false;
    }
    address_group_counts
        .get(&(account.to_string(), address.to_string()))
        .copied()
        .unwrap_or(0)
        == total
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

/// Merges conversation groups (keyed by account + Graph `conversationId`)
/// that are really the same thread, split by Exchange into different
/// `conversationId`s. All of the following must hold for a pair of groups
/// to merge:
/// - same account;
/// - a shared normalized subject that is non-empty and substantial (see
///   [`subject_strong_enough`]);
/// - neither group's raw subjects carried a calendar-response prefix (see
///   [`raw_subject_has_calendar_prefix`]) -- for those, the date/time that
///   normalization strips was the meeting's real identity, so the bare
///   subject cannot distinguish one instance from another;
/// - a shared `other_addresses` entry that is not a shared-mailbox or
///   distribution-list address common to every group of the account (see
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
    let mut groups_per_account: BTreeMap<String, usize> = BTreeMap::new();
    for (account, _) in groups.keys() {
        *groups_per_account.entry(account.clone()).or_insert(0) += 1;
    }
    let mut address_group_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for ((account, _), group) in &groups {
        for address in &group.addresses {
            *address_group_counts
                .entry((account.clone(), address.clone()))
                .or_insert(0) += 1;
        }
    }
    let keys: Vec<(String, String)> = groups.keys().cloned().collect();
    let group_count = keys.len();
    // Collected once into a Vec (index-aligned with `keys`) so the O(n^2)
    // pair loop below indexes a Vec instead of repeating a BTreeMap lookup
    // per pair.
    let group_values: Vec<ThreadGroup> = groups.into_values().collect();
    let mut parent: Vec<usize> = (0..group_count).collect();
    for a in 0..group_count {
        for b in (a + 1)..group_count {
            if keys[a].0 != keys[b].0 {
                continue;
            }
            let ga = &group_values[a];
            let gb = &group_values[b];
            if ga.has_team || gb.has_team || ga.has_calendar_prefix || gb.has_calendar_prefix {
                continue;
            }
            let subjects_match = ga
                .subjects
                .intersection(&gb.subjects)
                .any(|s| subject_strong_enough(s));
            if !subjects_match {
                continue;
            }
            let account = &keys[a].0;
            let addresses_match = ga.addresses.intersection(&gb.addresses).any(|addr| {
                !is_shared_mailbox_address(
                    account,
                    addr,
                    &groups_per_account,
                    &address_group_counts,
                )
            });
            if !addresses_match {
                continue;
            }
            if !groups_within(ga, gb, messages, MAX_GAP_SECONDS) {
                continue;
            }
            union_find_union(&mut parent, a, b);
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
    "Could we push it back by an hour instead?",
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
            resolution: None,
            resolution_kind: None,
            uncertainty: String::new(),
            unverified_deadline: false,
            unverified_resolution: false,
            cross_thread: false,
        };
        (all, item)
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

        let candidates = closure_candidates(&item, &all, evidence_conversation, evidence_timestamp);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, "v-1");
    }

    #[test]
    fn closure_candidates_sorted_ascending_and_capped_at_eight() {
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
            if day <= 9 {
                expected_ids.push(format!("c-{day}"));
            }
        }
        let candidates = closure_candidates(&item, &all, evidence_conversation, evidence_timestamp);
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
    }

    #[test]
    fn scan_closures_stops_when_cancelled_between_calls() {
        let (mut all, item1) = closure_test_messages();
        let valid1 = reply_to("sam@example.invalid", "v-1", "c2", "acct", "Fee");
        all.push(prepare(&valid1, "Sent", all.len()).unwrap());

        let evidence2 = request_from("sam@example.invalid", "req-2", "c6", "acct", "Fee 2");
        all.push(prepare(&evidence2, "Inbox", all.len()).unwrap());
        let evidence2_handle = all.last().unwrap().input.handle.clone();
        let valid2 = reply_to("sam@example.invalid", "v-2", "c7", "acct", "Fee 2");
        all.push(prepare(&valid2, "Sent", all.len()).unwrap());

        let mut item2 = item1.clone();
        item2.evidence.message = evidence2_handle;

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
    fn scan_closures_error_is_recorded_and_stops_only_this_pass() {
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
        };
        scan_closures(&all, &ScanProgress::default(), &mut result, |_, _, _| {
            Err(ProviderError::RateLimited)
        });
        assert_eq!(result.failures.len(), 1);
        assert!(result.failures[0].starts_with("Cross-thread closure pass: "));
        assert_eq!(result.cross_thread_closures, 0);
        assert!(!result.cancelled);
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
    fn prepared_handles_are_globally_unique_across_sources_like_review_state_loaded() {
        // Mirrors ReviewState::loaded's indexing: `index` is the running count
        // of already-prepared messages across ALL sources, not a per-source
        // counter, so show_anchor's handle lookup (which searches across every
        // loaded message regardless of source) never sees a collision.
        let mut messages: Vec<ReviewMessage> = vec![];
        for (source, bodies) in [
            ("Inbox", vec!["Please send the draft.", "Second message."]),
            ("Sent", vec!["Here is the draft."]),
            ("Group", vec!["Team update.", "Another update."]),
        ] {
            for (n, body) in bodies.into_iter().enumerate() {
                let item = synthetic(body, n, source);
                let m = prepare(&item, source, messages.len()).unwrap();
                messages.push(m);
            }
        }
        let handles: BTreeSet<&str> = messages.iter().map(|m| m.input.handle.as_str()).collect();
        assert_eq!(handles.len(), messages.len());
        for (i, m) in messages.iter().enumerate() {
            assert_eq!(m.input.handle, format!("m{i}"));
        }
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
        // outside participant ("sam@example.invalid").
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
        let merged = merge_threads(&mut messages);
        assert_eq!(merged, 1);
        assert_eq!(messages[0].conversation, messages[1].conversation);
        assert_ne!(messages[0].conversation, messages[2].conversation);
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
