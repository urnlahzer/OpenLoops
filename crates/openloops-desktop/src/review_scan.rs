use openloops_graph::live::{ConnectionError, review::MailItem};
use openloops_inference::{
    blocks::CanonicalBlock,
    canonical::canonicalize_plain,
    message::CanonicalMessage,
    ollama::{
        OllamaCloud, ProviderError,
        expectations::{ConversationMessage, Expectations, ResolutionKind},
    },
    reply_history::{
        REPLY_HISTORY_CHUNK_MAX_CHARS, chunk_reply_history, is_underscore_separator,
        starts_with_ascii_ci,
    },
    walker::canonicalize_html,
};
use std::collections::BTreeMap;
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
    })
}

pub fn scan(
    key: String,
    model: &str,
    messages: &[ReviewMessage],
    progress: &ScanProgress,
) -> Result<ScanResult, ProviderError> {
    progress.total.store(messages.len(), Ordering::Relaxed);
    let provider = OllamaCloud::connect(key, model)?;
    Ok(scan_conversations(messages, progress, |conversation| {
        provider.expectations(conversation)
    }))
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
// Live semantic probe: a request that is renegotiated (a counter-proposal,
// not a plain completion) one message later must still be recognized as
// closure evidence, with resolution_kind reflecting the renegotiation or
// supersession rather than a plain completion. The reply is a paraphrase of
// the resolution_kind renegotiated example quoted in INSTRUCTIONS, not a
// literal copy, so this exercises genuine semantic recognition rather than
// an echo of the prompt's own example text.
const RENEGOTIATION_CASE: (&str, &str) = (
    "Can we move our meeting to a different time?",
    "Could we push it back by an hour instead?",
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
        Some(ResolutionKind::Renegotiated) => "renegotiated",
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
    probe_renegotiation_case(&provider, passed + 3)?;
    probe_completed_resolution_case(&provider, passed + 4)?;
    Ok(passed + 4)
}

/// Runs `RENEGOTIATION_CASE` against `provider`: a request renegotiated (a
/// counter-proposal, not a plain completion) one message later must still
/// be recognized as closure evidence, with `resolution_kind` reflecting the
/// renegotiation or supersession rather than a plain completion. The reply
/// may itself read as a new request, so 1 or 2 items are both acceptable;
/// what matters is that the item anchored on the original request (`m0`)
/// carries the expected resolution kind. `case_number` is only for print
/// numbering. Prints counts and the observed kind name (both fixed
/// strings) only; never returned content.
fn probe_renegotiation_case(
    provider: &OllamaCloud,
    case_number: usize,
) -> Result<(), ProviderError> {
    let (request, reply) = RENEGOTIATION_CASE;
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
    if item.resolution.is_none()
        || !matches!(
            item.resolution_kind,
            Some(ResolutionKind::Renegotiated | ResolutionKind::Superseded)
        )
    {
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
}
