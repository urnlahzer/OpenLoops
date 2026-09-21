use crate::claim_view::{
    Anchor, ConversationMessage, EventPassed, LoopItem, LoopItems, Owner, SuggestedUpdate,
    SuggestedUpdateKind, action_phrase, card_action, claim_shape, resolve_owner, uncertainty_text,
    waiting_party_display,
};
use crate::deadline_view::{
    DeadlineKindHint, DeadlineView, EVENT_GENERIC_NOUNS, classify, classify_with_hint,
};
use crate::settings::Provider;
use chrono::{Datelike, TimeZone};
use openloops_contracts::{
    AmbiguityCode, Claim, ClaimType, EvidenceComponent, Nullable, TemporalKind,
};
use openloops_domain::deadline::UnixSeconds;
use openloops_domain::deadline_parse::{
    DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT, ParseContext, TemporalKind as DeadlineTemporalKind,
    TimezoneContext, Weekday, reparse,
};
use openloops_graph::live::{ConnectionError, review::MailItem};
use openloops_inference::{
    analysis::{
        AcceptedClaim, ClaimAnalysis, ReviewEvidence, analyze_claims, analyze_claims_omitting,
        rejection_label,
    },
    blocks::CanonicalBlock,
    canonical::canonicalize_plain,
    decision::{
        Answer, DECISION_DEADLINE, DecisionClient, OpenRouterDecisions, Questions,
        registry::Registry,
    },
    message::CanonicalMessage,
    ollama::OllamaCloud,
    openrouter::OpenRouter,
    provider::{MAX_PARALLEL_REQUESTS, ModelClient, ProviderError},
    reply_history::{
        REPLY_HISTORY_CHUNK_MAX_CHARS, chunk_reply_history, is_underscore_separator,
        starts_with_ascii_ci,
    },
    validation::{
        MessageContext, ParticipantHandle, ParticipantSlot, SuppliedContext, UserIdentity,
    },
    walker::canonicalize_html,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct ReviewMessage {
    pub input: ConversationMessage,
    pub source: String,
    pub id: String,
    pub account: String,
    pub conversation: String,
    pub date_label: String,
    pub web_link: String,
    /// Addresses belonging to the signed-in account, preserved for
    /// deterministic attribution checks after model output is projected.
    pub own_addresses: Vec<String>,
    /// Signed-in identity kept in memory only for governed attribution.
    pub own_display_name: Option<String>,
    pub own_given_name: Option<String>,
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
    pub conversation: String,
    pub message_handle: String,
    /// Which of the three ways [`build_event_index`] learns an event this
    /// entry came from -- used only for the coverage diagnostic in
    /// [`close_passed_events`].
    pub source: EventSource,
}

/// How [`build_event_index`] learned one [`EventRef`], in descending order
/// of confidence: Graph meeting-message metadata, a calendar-invite subject
/// line, an event noun and date found in the subject's own prose (never
/// structured as an invite), or an event phrase and date found in a
/// message body's prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSource {
    Meeting,
    Subject,
    SubjectProse,
    Prose,
}

/// Parses a calendar time embedded in a subject. Unknown or absent timezone
/// abbreviations use UTC because a UTC message timestamp does not imply a
/// reliable local offset without a timezone database.
///
/// An all-day (no stated time) subject ends at [`prose_event_time`]'s own
/// all-day policy -- local end-of-day
/// ([`DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT`], 17:00), not literal midnight --
/// so the two sources agree on what "all day" means rather than one ending
/// at 17:00 and the other at the next literal midnight.
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
            local_midnight + i64::from(DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT) - i64::from(offset),
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

/// One word-like run (alphanumerics plus the internal punctuation `.`, `-`,
/// `/`, and `:` that dates and times use, e.g. "Sep.", "9/3/2026",
/// "2026-09-03", "2:30") together with its byte offset into the original
/// text. All other punctuation (spaces, commas, parentheses) is a
/// separator and never appears in a token, which is what lets
/// `match_date_at` treat "Sep. 3, 2026" and "Sep. 3 2026" identically.
fn prose_tokens(text: &str) -> Vec<(String, usize)> {
    let is_word_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '/' | ':');
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (start, c) = chars[i];
        if !is_word_char(c) {
            i += 1;
            continue;
        }
        let mut end = start + c.len_utf8();
        let mut j = i + 1;
        while j < chars.len() && is_word_char(chars[j].1) {
            end = chars[j].0 + chars[j].1.len_utf8();
            j += 1;
        }
        tokens.push((text[start..end].to_string(), start));
        i = j;
    }
    tokens
}

/// The 1-based calendar month a written month name or its common
/// abbreviation refers to, case-insensitively and ignoring a trailing
/// period ("Sept 3", "Sep. 3", "September 3" are all September).
fn month_number(word: &str) -> Option<u32> {
    Some(
        match word.trim_end_matches('.').to_ascii_lowercase().as_str() {
            "january" | "jan" => 1,
            "february" | "feb" => 2,
            "march" | "mar" => 3,
            "april" | "apr" => 4,
            "may" => 5,
            "june" | "jun" => 6,
            "july" | "jul" => 7,
            "august" | "aug" => 8,
            "september" | "sep" | "sept" => 9,
            "october" | "oct" => 10,
            "november" | "nov" => 11,
            "december" | "dec" => 12,
            _ => return None,
        },
    )
}

/// A trailing sentence period glues onto the last token of a date at the
/// end of a sentence ("...on Sept 3." or "...on 2026-09-03."), since `.` is
/// itself a word character to `prose_tokens` (needed for "Sept."). Every
/// numeric field below strips it first so that period is never mistaken
/// for part of the digits.
fn strip_trailing_period(token: &str) -> &str {
    token.strip_suffix('.').unwrap_or(token)
}

fn parse_day_of_month(token: &str) -> Option<u32> {
    let token = strip_trailing_period(token);
    if token.is_empty() || token.len() > 2 || !token.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let day: u32 = token.parse().ok()?;
    (1..=31).contains(&day).then_some(day)
}

fn parse_four_digit_year(token: &str) -> Option<i32> {
    let token = strip_trailing_period(token);
    if token.len() != 4 || !token.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

/// `2026-09-03`, as one token (`-` is a word character to `prose_tokens`).
fn parse_iso_date(word: &str) -> Option<(u32, u32, Option<i32>)> {
    let word = strip_trailing_period(word);
    let parts: Vec<&str> = word.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return None;
    }
    let year: i32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let day: u32 = parts[2].parse().ok()?;
    ((1..=12).contains(&month) && (1..=31).contains(&day)).then_some((month, day, Some(year)))
}

/// `n/m/yyyy`, as one token (`/` is a word character to `prose_tokens`).
/// Read month-first (US convention, the owner's mail): "9/3/2026" is
/// September 3. Only when the first field exceeds 12 is the token read
/// day-first ("15/9/2026" is September 15). See [`prose_event_time`].
fn parse_numeric_date(word: &str) -> Option<(u32, u32, Option<i32>)> {
    let word = strip_trailing_period(word);
    let parts: Vec<&str> = word.split('/').collect();
    if parts.len() != 3 || parts[2].len() != 4 {
        return None;
    }
    let first: u32 = parts[0].parse().ok()?;
    let second: u32 = parts[1].parse().ok()?;
    let year: i32 = parts[2].parse().ok()?;
    // Month-first (US) by default, which is the owner's mail convention:
    // "9/3/2026" is September 3. Only a first field above 12 is read
    // day-first ("15/9/2026" is September 15).
    let (month, day) = if first > 12 && second <= 12 {
        (second, first)
    } else {
        (first, second)
    };
    ((1..=12).contains(&month) && (1..=31).contains(&day)).then_some((month, day, Some(year)))
}

/// One matched calendar date: `(month, day, year)`, where `year` is `None`
/// when the matched text did not state one (e.g. "September 3").
type MatchedDate = (u32, u32, Option<i32>);

/// Words that precede a bare number without making it a day of month --
/// "Room 12", "ext 12", "suite 12", "unit 12", "no 12", "#12" -- so a "day
/// month" match ([`match_date_at`]'s `day_then_month` case) starting at a
/// number immediately preceded by one of these is rejected outright: the
/// number far more likely names a room, extension, or other identifier
/// than a calendar day. Case-insensitive.
const DAY_FALSE_POSITIVE_PRECEDERS: &[&str] = &["room", "ext", "suite", "unit", "no", "#"];

/// Whether `tokens[i]` (a candidate day-of-month number) sits directly
/// after one of [`DAY_FALSE_POSITIVE_PRECEDERS`], and so must not be read
/// as a day-then-month date.
fn day_then_month_excluded(tokens: &[(String, usize)], i: usize) -> bool {
    i > 0 && DAY_FALSE_POSITIVE_PRECEDERS.contains(&tokens[i - 1].0.to_ascii_lowercase().as_str())
}

/// Tries every recognized date form starting exactly at `tokens[i]`,
/// returning the date it found and how many tokens it consumed.
fn match_date_at(tokens: &[(String, usize)], i: usize) -> Option<(MatchedDate, usize)> {
    let word = &tokens[i].0;
    if let Some(parsed) = parse_iso_date(word).or_else(|| parse_numeric_date(word)) {
        return Some((parsed, 1));
    }
    let month_then_day =
        month_number(word).zip(tokens.get(i + 1).and_then(|t| parse_day_of_month(&t.0)));
    let day_then_month = (!day_then_month_excluded(tokens, i))
        .then(|| parse_day_of_month(word).zip(tokens.get(i + 1).and_then(|t| month_number(&t.0))))
        .flatten()
        .map(|(day, month)| (month, day));
    let (month, day) = month_then_day.or(day_then_month)?;
    if let Some(year) = tokens.get(i + 2).and_then(|t| parse_four_digit_year(&t.0)) {
        return Some(((month, day, Some(year)), 3));
    }
    Some(((month, day, None), 2))
}

/// One optional token that may sit between a date and its trailing time
/// without blocking [`parse_prose_time`] from finding that time ("on
/// September 3 at 2 pm", "September 3, 2 pm", "September 3 @ 2pm",
/// "September 3 from 2pm", "September 3 starting 2pm"). `,` and `@` never
/// actually appear here as their own token -- [`prose_tokens`] treats both
/// as plain separators -- but are listed anyway so the intent reads
/// completely against the token stream that could exist.
const TIME_PREPOSITIONS: &[&str] = &["at", "@", ",", "from", "starting"];

/// True when `token` is, by itself, a dash acting as a separator: an ASCII
/// hyphen surrounded by whitespace on both sides tokenizes this way (`-` is
/// itself a `prose_tokens` word character, so it only ever stands alone
/// when nothing else is glued to it, e.g. "August 25 - 2pm" or
/// "2:00 - 5:30pm"); an en dash and an em dash are listed too, though
/// `prose_tokens` never actually returns either as its own token, since
/// neither is a word character -- listed anyway so the intent reads
/// completely against the token stream that could exist.
fn is_lone_dash_token(token: &str) -> bool {
    matches!(token, "-" | "\u{2013}" | "\u{2014}")
}

/// A clock time, or a clock time range, starting at `tokens[idx]`: `start`
/// and `end` in seconds since local midnight, and how many tokens were
/// consumed. A range is accepted as `H[:MM][am|pm]` optionally followed by
/// a dash (an ASCII hyphen -- fused, standalone, or glued onto either
/// side -- an en dash, an em dash, or the word "to") and a second
/// `H[:MM][am|pm]` -- see [`parse_prose_time_range`] and
/// [`resolve_range_meridiem`] for how a missing meridiem (and a range that
/// crosses midnight) is resolved. With no range, this is just a single
/// 12-hour time with the meridiem attached ("2pm", "2:30pm") or spaced
/// ("2:30 pm"), or a bare 24-hour time ("14:00"), and `end` is `start +
/// 1h`. Skips one leading [`TIME_PREPOSITIONS`] token, or a lone dash
/// token (see [`is_lone_dash_token`]) sitting between the date and the
/// time ("August 25 - 2pm"), first, so a time (or range) stated after one
/// still parses.
fn parse_prose_time(
    text: &str,
    tokens: &[(String, usize)],
    idx: usize,
) -> Option<(i32, i32, usize)> {
    let (start_idx, prep_consumed) = match tokens.get(idx) {
        Some(tok)
            if is_lone_dash_token(&tok.0)
                || TIME_PREPOSITIONS.contains(&tok.0.to_ascii_lowercase().as_str()) =>
        {
            (idx + 1, 1)
        }
        _ => (idx, 0),
    };
    if let Some((start, end, consumed)) = parse_prose_time_range(text, tokens, start_idx) {
        return Some((start, end, prep_consumed + consumed));
    }
    let (seconds, consumed) = parse_prose_time_literal(tokens, start_idx)?;
    Some((seconds, seconds + 3_600, prep_consumed + consumed))
}

/// The literal clock-time parse [`parse_prose_time`] falls back on for a
/// single (non-range) time, with no preposition handling of its own.
fn parse_prose_time_literal(tokens: &[(String, usize)], idx: usize) -> Option<(i32, usize)> {
    let token = &tokens.get(idx)?.0;
    if let Some(seconds) = parse_subject_clock(token) {
        return Some((seconds, 1));
    }
    if let Some(next) = tokens.get(idx + 1) {
        let lower = next.0.to_ascii_lowercase();
        if (lower == "am" || lower == "pm")
            && let Some(seconds) = parse_subject_clock(&format!("{token}{lower}"))
        {
            return Some((seconds, 2));
        }
    }
    let mut parts = token.split(':');
    let hour: i32 = parts.next()?.parse().ok()?;
    let minute: i32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(0..24).contains(&hour) || !(0..60).contains(&minute) {
        return None;
    }
    Some((hour * 3600 + minute * 60, 1))
}

/// One clock-time component within a possible range: hour/minute plus
/// whether "am"/"pm" was explicitly stated, and whether the source stated a
/// `:MM` at all ([`range_is_plausible`] uses this). `meridiem` is `None`
/// when the component stated neither, in which case `hour` is taken at
/// face value (0..=23) until [`resolve_range_meridiem`] disambiguates an
/// hour of 1..=12 (the only case a missing meridiem leaves ambiguous).
#[derive(Clone, Copy)]
struct RangeClock {
    hour: i32,
    minute: i32,
    meridiem: Option<bool>,
    has_colon: bool,
}

/// Parses one `H[:MM][am|pm]` range component, with no separator or range
/// handling of its own. A trailing sentence period ("11:30.") is stripped
/// first, the same way [`strip_trailing_period`] does for a date token.
fn parse_range_clock_str(s: &str) -> Option<RangeClock> {
    let s = s.strip_suffix('.').unwrap_or(s);
    let lower = s.to_ascii_lowercase();
    let (clock, meridiem) = if let Some(c) = lower.strip_suffix("am") {
        (c, Some(false))
    } else if let Some(c) = lower.strip_suffix("pm") {
        (c, Some(true))
    } else {
        (lower.as_str(), None)
    };
    let has_colon = clock.contains(':');
    let mut parts = clock.split(':');
    let hour: i32 = parts.next().filter(|p| !p.is_empty())?.parse().ok()?;
    let minute: i32 = parts.next().map_or(Some(0), |v| v.parse().ok())?;
    if parts.next().is_some() || !(0..60).contains(&minute) {
        return None;
    }
    let in_range = if meridiem.is_some() {
        (1..=12).contains(&hour)
    } else {
        (0..24).contains(&hour)
    };
    in_range.then_some(RangeClock {
        hour,
        minute,
        meridiem,
        has_colon,
    })
}

/// [`RangeClock`] resolved to seconds since local midnight, given whether it
/// should be read as pm. An hour already outside 1..=12 (only reachable
/// with no stated meridiem) is an unambiguous 24-hour value and `pm` is
/// ignored.
fn range_clock_seconds(clock: RangeClock, pm: bool) -> i32 {
    if !(1..=12).contains(&clock.hour) {
        return clock.hour * 3600 + clock.minute * 60;
    }
    ((clock.hour % 12) + if pm { 12 } else { 0 }) * 3600 + clock.minute * 60
}

/// Whether a bare range -- neither side stated a colon or a meridiem -- is
/// specific enough to trust as a time rather than an ordinary "N to M"
/// phrase ("5 to 7 people", "10 to 12 attendees"): a dash separator (a much
/// rarer, stronger signal than the common word "to") between two hours of
/// 1..=12 is trusted; a bare range joined by "to" never is, no matter the
/// hours. A range where either side stated a colon or a meridiem is always
/// trusted, regardless of the separator.
fn range_is_plausible(start: RangeClock, end: RangeClock, dash_separator: bool) -> bool {
    if start.has_colon || end.has_colon || start.meridiem.is_some() || end.meridiem.is_some() {
        return true;
    }
    dash_separator && start.hour <= 12 && end.hour <= 12
}

/// Resolves a range's two components to `(start, end)` seconds since local
/// midnight. When only one side states am/pm, the other inherits it (so
/// "2:00-5:30pm" is 14:00-17:30 and "9-11am" is 09:00-11:00). When neither
/// does, an hour of 1..=11 stays am and 12 stays noon, except the end is
/// read as pm when it is numerically smaller than the start ("11-1" is
/// 11am-1pm, not 11am-1am). Mirrors [`subject_event_time`]'s own policy for
/// a range that crosses midnight: when the resolved `end` does not come
/// after `start`, a day is added to it ("11pm-1am" is 23:00 to the next
/// day's 01:00).
fn resolve_range_meridiem(start: RangeClock, end: RangeClock) -> (i32, i32) {
    let (start_pm, end_pm) = match (start.meridiem, end.meridiem) {
        (Some(s), Some(e)) => (s, e),
        (None, Some(e)) => (e, e),
        (Some(s), None) => (s, s),
        (None, None) => (start.hour == 12, end.hour == 12 || end.hour < start.hour),
    };
    let start_secs = range_clock_seconds(start, start_pm);
    let mut end_secs = range_clock_seconds(end, end_pm);
    if end_secs <= start_secs {
        end_secs += 86_400;
    }
    (start_secs, end_secs)
}

/// True when `gap` -- the raw source text strictly between a range's two
/// time components -- is nothing but ASCII hyphens, en dashes, em dashes,
/// and whitespace. A plain ASCII hyphen only ever reaches here already
/// isolated: [`parse_prose_time_range`] strips one off either edge of the
/// two components first ([`strip_trailing_dash`] / [`strip_leading_dash`])
/// before computing the gap, since `-` is itself a `prose_tokens` word
/// character and would otherwise glue onto its neighbor's own token
/// ("2:00-5:30pm" fuses into one token entirely, handled instead by
/// [`parse_fused_range_token`]; "2:00- 5:30pm" and "2:00 -5:30pm" glue onto
/// only one side).
fn gap_is_dash_separator(gap: &str) -> bool {
    let trimmed = gap.trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|c| matches!(c, '-' | '\u{2013}' | '\u{2014}'))
}

/// A single fused token holding an ASCII-hyphen range ("2:00-5:30pm",
/// "9-11am"): since `-` is itself a `prose_tokens` word character, such a
/// range never splits into separate tokens the way an en/em dash or " to "
/// does. Splits on the first `-` and parses both halves as [`RangeClock`]s.
fn parse_fused_range_token(token: &str) -> Option<(RangeClock, RangeClock)> {
    let (left, right) = token.split_once('-')?;
    Some((parse_range_clock_str(left)?, parse_range_clock_str(right)?))
}

/// `token` with one trailing ASCII hyphen removed, when present and doing
/// so would not leave it empty -- separates a start clock like "2:00" from
/// a hyphen glued directly onto it ("2:00-", from "2:00- 5:30pm").
fn strip_trailing_dash(token: &str) -> &str {
    match token.strip_suffix('-') {
        Some(rest) if !rest.is_empty() => rest,
        _ => token,
    }
}

/// Like [`strip_trailing_dash`] but for a leading hyphen glued onto an end
/// clock ("-5:30pm", from "2:00 -5:30pm").
fn strip_leading_dash(token: &str) -> &str {
    match token.strip_prefix('-') {
        Some(rest) if !rest.is_empty() => rest,
        _ => token,
    }
}

/// Finishes a range once its two components and separator are known:
/// absorbs a meridiem spaced off from the end ("2:00-5:30 pm", where
/// `end_next_idx` names the token right after the end clock), rejects an
/// implausible bare range ([`range_is_plausible`]), then resolves the
/// meridiem and any midnight crossing ([`resolve_range_meridiem`]).
fn finalize_range(
    start: RangeClock,
    mut end: RangeClock,
    dash_separator: bool,
    tokens: &[(String, usize)],
    end_next_idx: usize,
    consumed: usize,
) -> Option<(i32, i32, usize)> {
    let mut consumed = consumed;
    if end.meridiem.is_none()
        && let Some(tok) = tokens.get(end_next_idx)
    {
        let lower = tok.0.to_ascii_lowercase();
        if lower == "am" || lower == "pm" {
            end.meridiem = Some(lower == "pm");
            consumed += 1;
        }
    }
    if !range_is_plausible(start, end, dash_separator) {
        return None;
    }
    let (start_secs, end_secs) = resolve_range_meridiem(start, end);
    Some((start_secs, end_secs, consumed))
}

/// A time range only (no preposition handling), starting exactly at
/// `tokens[idx]`: fused in one token via an ASCII hyphen ("2:00-5:30pm"),
/// across two tokens joined by a standalone "-" token, an en dash, an em
/// dash, or the word "to", or across two tokens where the ASCII hyphen is
/// instead glued onto the end of the start token ("2:00- 5:30pm") or the
/// start of the end token ("2:00 -5:30pm"). Returns `(start, end,
/// consumed)` in seconds since local midnight. `None` when no range is
/// found at `idx`, including when `idx` holds a single, unpaired time, or
/// when a bare range (see [`range_is_plausible`]) is not specific enough to
/// trust.
fn parse_prose_time_range(
    text: &str,
    tokens: &[(String, usize)],
    idx: usize,
) -> Option<(i32, i32, usize)> {
    let token = &tokens.get(idx)?.0;
    if let Some((start, end)) = parse_fused_range_token(token) {
        return finalize_range(start, end, true, tokens, idx + 1, 1);
    }
    let start_core = strip_trailing_dash(token);
    let start = parse_range_clock_str(start_core)?;
    let next = tokens.get(idx + 1)?;
    if next.0.eq_ignore_ascii_case("to") {
        let end = parse_range_clock_str(&tokens.get(idx + 2)?.0)?;
        return finalize_range(start, end, false, tokens, idx + 3, 3);
    }
    if is_lone_dash_token(&next.0) {
        let end = parse_range_clock_str(&tokens.get(idx + 2)?.0)?;
        return finalize_range(start, end, true, tokens, idx + 3, 3);
    }
    let end_core = strip_leading_dash(&next.0);
    let gap_start = tokens[idx].1 + start_core.len();
    let gap_end = next.1 + (next.0.len() - end_core.len());
    let gap = text.get(gap_start..gap_end)?;
    if !gap_is_dash_separator(gap) {
        return None;
    }
    let end = parse_range_clock_str(end_core)?;
    finalize_range(start, end, true, tokens, idx + 2, 2)
}

/// `message_timestamp`'s calendar year in the local time `offset` implies.
fn local_year(message_timestamp: i64, offset: i32) -> i32 {
    chrono::DateTime::from_timestamp(message_timestamp + i64::from(offset), 0)
        .map_or(1970, |t| t.year())
}

/// Finds the first explicit date in `text` -- "September 3", "Sept 3",
/// "Sep. 3, 2026", "3 September 2026", "9/3/2026", or "2026-09-03" --
/// optionally followed by a time ("2pm", "2:30 pm", "14:00", or any of
/// those after one leading preposition token -- see [`TIME_PREPOSITIONS`]),
/// and resolves it to `(start, end, byte_offset)` in UTC seconds, where
/// `byte_offset` is where the date text begins in `text`.
///
/// A date with no stated year defaults to `message_timestamp`'s own year in
/// the given `offset`, rolling forward one year when that default would
/// land the date more than 60 days before `message_timestamp` -- so
/// "September 3" said in November means next year's September 3rd, not one
/// already long past. A stated year is always taken as given, never rolled.
///
/// A numeric `n/m/yyyy` date is read month-first (US convention):
/// "9/3/2026" is September 3. A first field above 12 is read day-first
/// ("15/9/2026" is September 15): see [`parse_numeric_date`].
///
/// `end` is `start` plus one hour when a time was found; otherwise the
/// matched date's whole civil day, closed at its local end-of-day per the
/// same policy `deadline_view::classify` uses for a bare date deadline
/// (`DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT`) -- the same all-day policy
/// [`subject_event_time`] uses -- not literal midnight-to-midnight.
pub fn prose_event_time(
    text: &str,
    message_timestamp: i64,
    offset: i32,
) -> Option<(i64, i64, usize)> {
    prose_event_time_candidates(text, message_timestamp, offset)
        .into_iter()
        .next()
}

/// Every explicit date [`prose_event_time`] could find in `text`, resolved
/// the same way and in the same left-to-right order -- [`prose_event_time`]
/// itself is just this list's first element. Shared with the body-prose
/// event learner ([`nearest_body_event_pairing`]), which must weigh EVERY
/// date in a block against every candidate event phrase to pair each
/// phrase with the date nearest to it, rather than always the block's
/// first date.
fn prose_event_time_candidates(
    text: &str,
    message_timestamp: i64,
    offset: i32,
) -> Vec<(i64, i64, usize)> {
    let tokens = prose_tokens(text);
    let mut found = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let Some(((month, day, year), consumed)) = match_date_at(&tokens, i) else {
            i += 1;
            continue;
        };
        let year_given = year.is_some();
        let mut year = year.unwrap_or_else(|| local_year(message_timestamp, offset));
        let Some(mut date) = chrono::NaiveDate::from_ymd_opt(year, month, day) else {
            i += 1;
            continue;
        };
        let Some(mut local_midnight) = date.and_hms_opt(0, 0, 0).map(|d| d.and_utc().timestamp())
        else {
            i += 1;
            continue;
        };
        let mut day_start = local_midnight - i64::from(offset);
        if !year_given && day_start < message_timestamp - 60 * 86_400 {
            year += 1;
            let Some(rolled) = chrono::NaiveDate::from_ymd_opt(year, month, day) else {
                i += 1;
                continue;
            };
            date = rolled;
            let Some(rolled_midnight) = date.and_hms_opt(0, 0, 0).map(|d| d.and_utc().timestamp())
            else {
                i += 1;
                continue;
            };
            local_midnight = rolled_midnight;
            day_start = local_midnight - i64::from(offset);
        }
        let byte_offset = tokens[i].1;
        if let Some((start_seconds, end_seconds, _)) = parse_prose_time(text, &tokens, i + consumed)
        {
            let start = local_midnight + i64::from(start_seconds) - i64::from(offset);
            let end = local_midnight + i64::from(end_seconds) - i64::from(offset);
            found.push((start, end, byte_offset));
        } else {
            let day_end =
                local_midnight + i64::from(DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT) - i64::from(offset);
            found.push((day_start, day_end, byte_offset));
        }
        i += consumed;
    }
    found
}

/// Event nouns recognized in ordinary prose (as opposed to
/// [`EVENT_GENERIC_NOUNS`], which are stripped as too common to distinguish
/// one event from another): a capitalized multi-word phrase ending in one of
/// these, found near a date in a message body, is learned as that event's
/// name by [`body_event_name`], and a subject's own prose is only ever
/// learned as an event when the subject (minus its date) contains one of
/// these too, gating the subject-prose learner in [`learn_one_event`]
/// against turning any ordinary "<words> <date>" subject into a phantom
/// event. Deliberately excludes "deadline": a stated deadline is evidence
/// about an OBLIGATION, not proof that a gathering by that name exists.
const PROSE_EVENT_NOUNS: &[&str] = &[
    "ceremony",
    "call",
    "closing",
    "conference",
    "deposition",
    "hearing",
    "launch",
    "meeting",
    "retreat",
    "seminar",
    "session",
    "summit",
    "training",
    "trial",
    "webinar",
    "workshop",
];

/// True when at least one whitespace-separated word of `name` (already
/// lowercased, as [`normalize_subject`] produces) is a [`PROSE_EVENT_NOUNS`]
/// entry.
fn contains_prose_event_noun(name: &str) -> bool {
    name.split_whitespace()
        .any(|word| PROSE_EVENT_NOUNS.contains(&word))
}

/// How close (in bytes) a candidate event-name phrase must sit to the date
/// text in a message body for [`body_event_name`] to credit them as
/// belonging to the same event.
const PROSE_EVENT_NAME_WINDOW: usize = 120;

/// Builds the event index from every loaded message, learning each one from
/// the first source that applies, in descending order of confidence: Graph
/// meeting-message metadata, a calendar-invite subject line, an explicit
/// date found in the subject's own prose (a name never structured as an
/// invite, e.g. "Spring Estate Planning Workshop - Sept 3", gated by
/// [`contains_prose_event_noun`] so an ordinary "<words> <date>" subject
/// with no event-shaped noun in it never counts), or an explicit date found
/// in a message body near a phrase naming the event. An out-of-date entry
/// (a superseded meeting-message revision) is dropped outright, never
/// learned; a name of fewer than 2 words is also dropped -- too generic to
/// identify one event ("hi", "fyi", a bare "Hearing") -- [`learn_one_event`]
/// itself already enforces this per candidate so a short-name rejection
/// falls through to the next source or block rather than losing the whole
/// message, and this is a redundant final check. When multiple messages
/// name the same normalized event in the same conversation (a reschedule,
/// or the same invite echoed by more than one message), only the entry with
/// the LATEST start survives -- a stale "Aug 21" instance of "design
/// workshop" must not shadow a later "Sep 15" reschedule of the same
/// meeting. Equal names in different conversations remain separate so each
/// entry retains the scope it was learned from.
#[cfg(test)]
pub fn build_event_index(messages: &[ReviewMessage]) -> Vec<EventRef> {
    build_event_index_with_rules(messages, None)
}

fn build_event_index_with_rules(
    messages: &[ReviewMessage],
    rules: Option<&RuleDecisions>,
) -> Vec<EventRef> {
    let mut best: BTreeMap<(String, String), EventRef> = BTreeMap::new();
    for message in messages {
        let Some((name, start, end, source)) = learn_one_event(message, rules) else {
            continue;
        };
        if name.split_whitespace().count() < 2 {
            continue;
        }
        let entry = best
            .entry((message.conversation.clone(), name.clone()))
            .or_insert(EventRef {
                name,
                start: i64::MIN,
                end: 0,
                conversation: message.conversation.clone(),
                message_handle: String::new(),
                source,
            });
        if start > entry.start {
            entry.start = start;
            entry.end = end;
            entry.message_handle.clone_from(&message.input.handle);
            entry.source = source;
        }
    }
    best.into_values().collect()
}

/// Whether `name` is specific enough to identify one event: 2 or more
/// words. A candidate that fails this check is rejected and
/// [`learn_one_event`] falls through to its next candidate (a later body
/// block, or the block-level phrase search) instead of aborting the whole
/// message -- a message can carry more than one date/name candidate, and a
/// too-generic early one must not shadow a good later one.
fn is_specific_event_name(name: &str) -> bool {
    name.split_whitespace().count() >= 2
}

/// Sender domains recap/transcription services use for their meeting-summary
/// mail. Recognition hints only, deliberately generous: matching a sender
/// domain here is sufficient on its own for [`is_meeting_recap_artifact`]
/// because these domains are dedicated to producing after-the-fact meeting
/// summaries, never ordinary correspondence. A message this old excludes as
/// a recap artifact but that is ALSO written after its own event is already
/// caught independently by the temporal rule in [`close_from_index`] and
/// [`close_from_stated_time`]; this is belt-and-braces, not the only guard.
const RECAP_SENDER_DOMAINS: &[&str] = &[
    "fathom.video",
    "fathom.ai",
    "otter.ai",
    "fireflies.ai",
    "read.ai",
    "tactiq.io",
    "tldv.io",
    "grain.com",
    "grain.co",
    "avoma.com",
    "gong.io",
    "chorus.ai",
];

/// Sender display-name substrings (matched case-insensitively) that mark a
/// notetaker/recap bot riding on a general-purpose platform address (Zoom,
/// Google Meet) that cannot be identified by domain alone. Sufficient on
/// their own, same as [`RECAP_SENDER_DOMAINS`].
const RECAP_SENDER_NAME_HINTS: &[&str] = &["ai companion", "gemini", "copilot"];

/// Exact sender addresses (matched case-insensitively) belonging to a
/// general-purpose platform (Zoom, Teams, Meet) rather than a dedicated
/// recap vendor. These addresses ALSO send ordinary meeting invites and
/// notifications, so an address match here is never enough by itself --
/// [`is_meeting_recap_artifact`] additionally requires a
/// [`RECAP_SUBJECT_HINTS`] match before treating one of these as a recap.
const RECAP_SENDER_PLATFORM_ADDRESSES: &[&str] = &[
    "no-reply@zoom.us",
    "noreply@teams.microsoft.com",
    "meetings-noreply@google.com",
];

/// Subject substrings (matched case-insensitively) that mark a meeting-recap
/// or call-transcript email. Recognition hints only -- see
/// [`is_meeting_recap_artifact`] for how they combine with sender and body
/// evidence.
const RECAP_SUBJECT_HINTS: &[&str] = &[
    "meeting summary",
    "meeting notes",
    "meeting recap",
    "recap",
    "notes:",
    "summary:",
    "call summary",
    "call notes",
    "your meeting",
    "transcript",
    "highlights",
    "action items",
    "key takeaways",
];

/// Body section markers (matched case-insensitively, and only within the
/// first [`RECAP_BODY_SCAN_CHARS`] characters of the first body block) that
/// mark a structured meeting-recap email.
const RECAP_BODY_MARKERS: &[&str] = &[
    "action items",
    "key takeaways",
    "next steps",
    "meeting purpose",
    "summary",
    "attendees",
    "transcript",
];

/// How far into the first body block [`is_meeting_recap_artifact`] looks for
/// a [`RECAP_BODY_MARKERS`] entry -- these services put their section
/// headings up front, so scanning the whole (potentially long) body is
/// unnecessary and would only invite false positives from later prose.
const RECAP_BODY_SCAN_CHARS: usize = 600;

/// Whether `message` is a machine-generated meeting-recap or call-transcript
/// artifact from a transcription/notetaker service (Fathom, Otter, Fireflies,
/// Read.ai, tl;dv, Grain, Avoma, Gong, Chorus, or a notetaker bot riding on
/// Zoom/Teams/Meet), rather than an ordinary message written by a
/// participant. True when ANY of:
///
/// - the sender's address matches a dedicated recap vendor's domain
///   ([`RECAP_SENDER_DOMAINS`]) or display name ([`RECAP_SENDER_NAME_HINTS`]);
/// - the sender is a general-purpose platform's notetaker address
///   ([`RECAP_SENDER_PLATFORM_ADDRESSES`]) AND the subject also carries a
///   recap pattern ([`RECAP_SUBJECT_HINTS`]);
/// - the subject carries a recap pattern AND the first body block opens with
///   a recap section marker ([`RECAP_BODY_MARKERS`]).
///
/// A sender match alone is decisive; otherwise two independent signals are
/// required, so an ordinary message like "Send me your notes" (subject
/// contains "notes" but not the `"notes:"` pattern, and carries no body
/// marker) is never mistaken for one. These are recognition hints, not the
/// only defense against closing a request from a past-tense summary -- see
/// [`close_from_index`]'s and [`close_from_stated_time`]'s temporal guard.
#[cfg(test)]
pub(crate) fn is_meeting_recap_artifact(message: &ReviewMessage) -> bool {
    is_meeting_recap_artifact_fast(message)
}

fn recap_state(message: &ReviewMessage) -> serde_json::Value {
    serde_json::json!({
        "sender": message.input.message.sender.as_ref().map_or_else(String::new, CanonicalBlock::as_string),
        "subject": message.input.message.subject.as_string(),
        "first_paragraph": message.input.message.body_blocks.first().map_or_else(String::new, CanonicalBlock::as_string),
    })
}

fn recap_residue_candidate(message: &ReviewMessage) -> bool {
    if is_meeting_recap_artifact_fast(message) {
        return false;
    }
    let subject = message.input.message.subject.as_string().to_lowercase();
    let body = message
        .input
        .message
        .body_blocks
        .first()
        .map(|block| block.as_string().to_lowercase())
        .unwrap_or_default();
    RECAP_SUBJECT_HINTS
        .iter()
        .any(|hint| subject.contains(hint))
        || RECAP_BODY_MARKERS
            .iter()
            .any(|marker| body.contains(marker))
}

fn is_meeting_recap_artifact_with_rules(
    message: &ReviewMessage,
    rules: Option<&RuleDecisions>,
) -> bool {
    is_meeting_recap_artifact_fast(message)
        || (recap_residue_candidate(message)
            && rules.is_some_and(|cache| cache.noul(RULE_RECAP, &recap_state(message))))
}

fn is_meeting_recap_artifact_fast(message: &ReviewMessage) -> bool {
    let sender = message
        .input
        .message
        .sender
        .as_ref()
        .map(|block| block.as_string().to_lowercase())
        .unwrap_or_default();
    if RECAP_SENDER_DOMAINS
        .iter()
        .any(|domain| sender.contains(&format!("@{domain}")))
        || RECAP_SENDER_NAME_HINTS
            .iter()
            .any(|hint| sender.contains(hint))
    {
        return true;
    }

    let subject = message.input.message.subject.as_string().to_lowercase();
    let subject_has_recap_pattern = RECAP_SUBJECT_HINTS
        .iter()
        .any(|hint| subject.contains(hint));

    if subject_has_recap_pattern
        && RECAP_SENDER_PLATFORM_ADDRESSES
            .iter()
            .any(|address| sender.contains(address))
    {
        return true;
    }

    let body_prefix: String = message
        .input
        .message
        .body_blocks
        .first()
        .map(|block| {
            let text = block.as_string();
            let cut = text
                .char_indices()
                .nth(RECAP_BODY_SCAN_CHARS)
                .map_or(text.len(), |(index, _)| index);
            text[..cut].to_lowercase()
        })
        .unwrap_or_default();
    let body_has_marker = RECAP_BODY_MARKERS
        .iter()
        .any(|marker| body_prefix.contains(marker));

    subject_has_recap_pattern && body_has_marker
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the display projection API explicitly represents an optional meeting time"
)]
pub(crate) fn recap_meeting_time(message: &ReviewMessage) -> Option<(i64, bool)> {
    if let Some((start, _, false)) = message.event {
        return Some((start, false));
    }
    let subject = message.input.message.subject.as_string();
    if let Some((start, _)) = subject_event_time(&subject, message.input.timestamp) {
        return Some((start, false));
    }
    if let Some(body) = message.input.message.body_blocks.first() {
        let body = body.as_string();
        let offset = local_offset_seconds(message.input.timestamp, 0);
        if let Some((start, _, _)) = prose_event_time(&body, message.input.timestamp, offset) {
            return Some((start, false));
        }
    }
    Some((message.input.timestamp, true))
}

/// One message's contribution to the event index, per the priority order
/// documented on [`build_event_index`]. `None` when none of the sources
/// found a sufficiently specific ([`is_specific_event_name`]) candidate (or
/// the Graph/subject entry was out of date, or the message is a
/// [`is_meeting_recap_artifact`] describing an event that has already
/// happened).
fn learn_one_event(
    message: &ReviewMessage,
    rules: Option<&RuleDecisions>,
) -> Option<(String, i64, i64, EventSource)> {
    if is_meeting_recap_artifact_with_rules(message, rules) {
        return None;
    }
    let subject = message.input.message.subject.as_string();
    if let Some((start, end, out_of_date)) = message.event {
        if out_of_date {
            return None;
        }
        let name = strip_leading_possessive(&normalize_subject(&subject));
        return is_specific_event_name(&name).then_some((name, start, end, EventSource::Meeting));
    }
    if let Some((start, end)) = subject_event_time(&subject, message.input.timestamp) {
        let name = strip_leading_possessive(&normalize_subject(&subject));
        return is_specific_event_name(&name).then_some((name, start, end, EventSource::Subject));
    }
    let offset = local_offset_seconds(message.input.timestamp, 0);
    if let Some((start, end, byte_offset)) =
        subject_prose_event_time(&subject, message.input.timestamp)
    {
        let name_end = subject_prose_name_end(&subject, byte_offset);
        let prefix = &subject[..name_end];
        // A trailing ')' is only ever a stray, unmatched close -- one worth
        // trimming alongside "-@:," -- when this prefix has more ')' than
        // '(' overall. When they balance (or opens win), a trailing ')'
        // closes a real group ("(draft)") that `normalize_subject`'s own
        // trailing-paren handling must see intact to decide whether to keep
        // or drop it.
        let unmatched_closing_paren = prefix.matches(')').count() > prefix.matches('(').count();
        let name =
            strip_leading_possessive(&normalize_subject(prefix.trim_end_matches(|c: char| {
                c.is_whitespace()
                    || matches!(c, '-' | '@' | ':' | ',')
                    || (c == ')' && unmatched_closing_paren)
            })));
        if is_specific_event_name(&name) && contains_prose_event_noun(&name) {
            return Some((name, start, end, EventSource::SubjectProse));
        }
    }
    let subject_name = normalize_subject(&subject);
    for block in &message.input.message.body_blocks {
        let text = block.as_string();
        let dates = prose_event_time_candidates(&text, message.input.timestamp, offset);
        if dates.is_empty() {
            continue;
        }
        if let Some((name, start, end)) = body_event_name(&text, &dates, &subject_name)
            && is_specific_event_name(&name)
        {
            return Some((name, start, end, EventSource::Prose));
        }
    }
    None
}

/// Resolves subject-prose dates using the machine-local UTC offset at the
/// event instant. The first parse only supplies an instant at which to ask
/// for that offset; reparsing applies the event date's offset to the civil
/// date/time itself, which matters when the message and event straddle DST.
fn subject_prose_event_time(text: &str, message_timestamp: i64) -> Option<(i64, i64, usize)> {
    subject_prose_event_time_with(text, message_timestamp, |timestamp| {
        local_offset_seconds(timestamp, 0)
    })
}

fn subject_prose_event_time_with(
    text: &str,
    message_timestamp: i64,
    mut offset_at: impl FnMut(i64) -> i32,
) -> Option<(i64, i64, usize)> {
    let message_offset = offset_at(message_timestamp);
    let provisional = prose_event_time(text, message_timestamp, message_offset)?;
    let event_offset = offset_at(provisional.0);
    if event_offset == message_offset {
        Some(provisional)
    } else {
        prose_event_time(text, message_timestamp, event_offset)
    }
}

/// When `byte_offset` (a matched date's start, from [`prose_event_time`])
/// sits inside a parenthesized group opened earlier in `subject`, returns
/// that group's opening `(` byte index so the whole group -- including a
/// leading weekday and comma, e.g. "(Tuesday, August 25 - 2:00-5:30pm)" --
/// is dropped from the learned subject-prose event name, rather than just
/// the date text onward. Otherwise returns `byte_offset` unchanged, e.g. for
/// a date that follows a dash outside any parentheses ("Workshop - Sept
/// 3").
fn subject_prose_name_end(subject: &str, byte_offset: usize) -> usize {
    let mut depth = 0i32;
    let mut open_idx = None;
    for (idx, c) in subject[..byte_offset].char_indices() {
        match c {
            '(' => {
                if depth == 0 {
                    open_idx = Some(idx);
                }
                depth += 1;
            }
            ')' => {
                depth = (depth - 1).max(0);
                if depth == 0 {
                    open_idx = None;
                }
            }
            _ => {}
        }
    }
    if depth > 0 {
        open_idx.unwrap_or(byte_offset)
    } else {
        byte_offset
    }
}

/// Strips a leading possessive ("your ", "my ", or "our ") from a learned
/// subject-prose event name, e.g. "your spring planning workshop
/// checklist" becomes "spring planning workshop checklist". `name` is
/// already lowercased ([`normalize_subject`]'s output).
fn strip_leading_possessive(name: &str) -> String {
    for prefix in ["your ", "my ", "our "] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    name.to_string()
}

/// The event name and date for a body-block prose match: the message's own
/// normalized subject, paired with the block's first date, when that
/// subject is itself specific ([`is_specific_event_name`]) AND the block's
/// text contains it; otherwise (including when the subject is present but
/// too generic to trust on its own, e.g. a one-word subject like "Prep"
/// that happens to sit inside an unrelated word like "prepare") falls
/// through to the (phrase, date) pairing [`nearest_body_event_pairing`]
/// finds among every capitalized multi-word phrase ending in a
/// [`PROSE_EVENT_NOUN`] and every date in `dates`. Requiring specificity
/// FIRST matters: a short subject is exactly the case most likely to
/// appear as a coincidental substring of ordinary prose (as opposed to
/// actually naming the event), and matching on it there would silently
/// mask a real, specific phrase later in the same text. `None` when
/// nothing is found -- an unnamed date is not learned.
///
/// [`PROSE_EVENT_NOUN`]: PROSE_EVENT_NOUNS
fn body_event_name(
    text: &str,
    dates: &[(i64, i64, usize)],
    subject_name: &str,
) -> Option<(String, i64, i64)> {
    if is_specific_event_name(subject_name)
        && text.to_lowercase().contains(subject_name)
        && let Some(&(start, end, _)) = dates.first()
    {
        return Some((subject_name.to_string(), start, end));
    }
    nearest_body_event_pairing(text, dates)
}

/// True when `word` starts with an ASCII uppercase letter (a cheap proxy for
/// "looks like part of a proper name" in ordinary prose).
fn starts_uppercase(word: &str) -> bool {
    word.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// Every capitalized multi-word run (2 or more consecutive capitalized
/// words) ending in a [`PROSE_EVENT_NOUNS`] entry, each with its start/end
/// byte offsets, in left-to-right order. A run never crosses a sentence
/// boundary: it stops growing immediately after including a token that
/// ends in `.` (so "Notes. Onboarding Training" yields only "Onboarding
/// Training", never a run that swallows the unrelated preceding sentence).
fn capitalized_event_phrases(text: &str) -> Vec<(String, usize, usize)> {
    let tokens = prose_tokens(text);
    let mut phrases = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if !starts_uppercase(&tokens[i].0) {
            i += 1;
            continue;
        }
        let mut j = i;
        loop {
            let ends_sentence = tokens[j].0.ends_with('.');
            j += 1;
            if ends_sentence || j >= tokens.len() || !starts_uppercase(&tokens[j].0) {
                break;
            }
        }
        if j - i >= 2
            && PROSE_EVENT_NOUNS.contains(
                &tokens[j - 1]
                    .0
                    .trim_end_matches('.')
                    .to_lowercase()
                    .as_str(),
            )
        {
            let phrase_start = tokens[i].1;
            let phrase_end = tokens[j - 1].1 + tokens[j - 1].0.len();
            let phrase = tokens[i..j]
                .iter()
                .map(|t| t.0.as_str())
                .collect::<Vec<_>>()
                .join(" ")
                .trim_end_matches('.')
                .to_string();
            phrases.push((phrase, phrase_start, phrase_end));
        }
        i = j;
    }
    phrases
}

/// Among `phrases` (as returned by [`capitalized_event_phrases`]), the one
/// whose nearer edge sits closest to `date_byte_offset` and within
/// [`PROSE_EVENT_NAME_WINDOW`] bytes of it: the phrase lowercased, its
/// distance, and whether the date follows the phrase (`date_byte_offset` is
/// at or after the phrase's end) -- callers pairing one phrase against
/// MULTIPLE dates ([`nearest_body_event_pairing`]) use that bit to break a
/// distance tie in favor of the natural "<Event Name> on <date>" order.
fn nearest_phrase_for_date(
    phrases: &[(String, usize, usize)],
    date_byte_offset: usize,
) -> Option<(String, usize, bool)> {
    let mut best: Option<(String, usize, bool)> = None;
    for (phrase, phrase_start, phrase_end) in phrases {
        let (distance, date_follows) = if *phrase_start > date_byte_offset {
            (phrase_start - date_byte_offset, false)
        } else {
            (date_byte_offset.saturating_sub(*phrase_end), true)
        };
        if distance > PROSE_EVENT_NAME_WINDOW {
            continue;
        }
        let better = best
            .as_ref()
            .is_none_or(|(_, best_distance, _)| distance < *best_distance);
        if better {
            best = Some((phrase.to_lowercase(), distance, date_follows));
        }
    }
    best
}

/// Pairs a message body's dates against its capitalized event-noun phrases:
/// evaluates every phrase against every date in `dates` and returns the
/// (phrase, date) pairing with the smallest byte distance between them --
/// never just the block's first date -- so "please RSVP by September 1 for
/// the Spring Workshop on September 15" pairs "Spring Workshop" with
/// September 15, not the earlier, unrelated September 1. A tie is broken in
/// favor of a date that follows its phrase within
/// [`PROSE_EVENT_NAME_WINDOW`] bytes.
fn nearest_body_event_pairing(
    text: &str,
    dates: &[(i64, i64, usize)],
) -> Option<(String, i64, i64)> {
    let phrases = capitalized_event_phrases(text);
    let mut best: Option<(String, i64, i64, usize, bool)> = None;
    for &(start, end, date_offset) in dates {
        let Some((phrase, distance, date_follows)) = nearest_phrase_for_date(&phrases, date_offset)
        else {
            continue;
        };
        let better = match &best {
            None => true,
            Some((_, _, _, best_distance, best_follows)) => {
                distance < *best_distance
                    || (distance == *best_distance && date_follows && !*best_follows)
            }
        };
        if better {
            best = Some((phrase, start, end, distance, date_follows));
        }
    }
    best.map(|(phrase, start, end, ..)| (phrase, start, end))
}

/// Stop words dropped from both the phrase and the event name before
/// computing token overlap: too common to distinguish one event from
/// another, including prepositions and fillers that often precede an event
/// name ("before the design workshop", "ahead of my closing").
const EVENT_MATCH_STOP_WORDS: &[&str] = &[
    "the", "our", "a", "an", "this", "that", "with", "for", "of", "on", "at", "and", "before",
    "prior", "ahead", "after", "to", "by", "in", "during", "next", "upcoming", "my", "your",
    "coming",
];

/// Lowercased, alphanumeric-tokenized `text` with no filtering at all --
/// the fallback [`meaningful_event_tokens`] uses when stripping stop words
/// and generic nouns would otherwise leave nothing to match against.
fn raw_event_tokens(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

/// Lowercased, alphanumeric-tokenized `text` with stop words and generic
/// event nouns removed. When that leaves nothing (a name that is entirely
/// filler and its own kind-of-gathering word, e.g. "the call"), falls back
/// to the unfiltered tokens instead of an empty set, so such a name can
/// still be matched on its raw words rather than becoming permanently
/// unmatchable.
fn meaningful_event_tokens(text: &str) -> BTreeSet<String> {
    let raw = raw_event_tokens(text);
    let filtered: BTreeSet<String> = raw
        .iter()
        .filter(|token| {
            !EVENT_MATCH_STOP_WORDS.contains(&token.as_str())
                && !EVENT_GENERIC_NOUNS.contains(&token.as_str())
        })
        .cloned()
        .collect();
    if filtered.is_empty() { raw } else { filtered }
}

/// Matches a phrase naming an event (e.g. an expectation's `event` anchor
/// quote) against the learned event index. Never substring matching: both
/// sides are tokenized and reduced to their meaningful (non-stop-word,
/// non-generic-noun) tokens first. A phrase with no meaningful tokens at all
/// (e.g. "the call") matches nothing -- this requirement applies to the
/// PHRASE side only; an event NAME reducing to zero meaningful tokens keeps
/// its raw tokens instead (see [`meaningful_event_tokens`]), so an index
/// entry named only after its kind of gathering can still be matched.
/// Otherwise at least one meaningful token must be shared; when the event
/// name itself has 2 or more meaningful tokens, that alone is not enough --
/// either 2 tokens must be shared, or every one of the phrase's meaningful
/// tokens must appear in the name (so a short, specific phrase like "our
/// quarterly workshop" can still identify a longer name it is a strict
/// subset of).
#[cfg(test)]
pub fn match_event<'a>(
    phrase: &str,
    evidence_timestamp: i64,
    index: &'a [EventRef],
) -> Option<&'a EventRef> {
    match_event_with_rules(phrase, evidence_timestamp, index, None)
}

fn event_match_state(phrase: &str, event_name: &str) -> serde_json::Value {
    serde_json::json!({"phrase": phrase, "event_name": event_name})
}

fn match_event_with_rules<'a>(
    phrase: &str,
    evidence_timestamp: i64,
    index: &'a [EventRef],
    rules: Option<&RuleDecisions>,
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
            let fast =
                name_tokens.len() < 2 || shared >= 2 || phrase_tokens.is_subset(&name_tokens);
            fast || rules.is_some_and(|cache| {
                cache.noul(RULE_EVENT_MATCH, &event_match_state(phrase, &event.name))
            })
        })
        .min_by_key(|event| (event.start - evidence_timestamp, &event.message_handle))
}

/// Like [`match_event`] but requires proper-name-strength evidence: at
/// least 2 shared meaningful tokens against a name that itself carries 2 or
/// more meaningful tokens. Used only when the model named no event at all
/// (see [`text_matched_event`]), where an ordinary single-token overlap
/// would match on a generic phrase far too easily. Also never matches an
/// event learned from prose ([`EventSource::SubjectProse`] or
/// [`EventSource::Prose`]): text matching against those is a second,
/// unverified guess stacked on top of an already-inferred name, and would
/// otherwise let a subject like "Draft Agreement - September 3" (learned,
/// if at all, only because it names a real event) silently close an
/// unrelated action whose text happens to share those same words, e.g.
/// "Send the draft agreement to Alex". Restricted to
/// [`EventSource::Meeting`] and [`EventSource::Subject`] -- structured
/// calendar evidence, not an inference from ordinary prose.
#[cfg(test)]
fn match_event_by_text<'a>(
    phrase: &str,
    evidence_timestamp: i64,
    index: &'a [EventRef],
) -> Option<&'a EventRef> {
    match_event_by_text_with_rules(phrase, evidence_timestamp, index, None)
}

fn match_event_by_text_with_rules<'a>(
    phrase: &str,
    evidence_timestamp: i64,
    index: &'a [EventRef],
    rules: Option<&RuleDecisions>,
) -> Option<&'a EventRef> {
    let phrase_tokens = meaningful_event_tokens(phrase);
    if phrase_tokens.is_empty() {
        return None;
    }
    index
        .iter()
        .filter(|event| matches!(event.source, EventSource::Meeting | EventSource::Subject))
        .filter(|event| {
            event.start >= evidence_timestamp && event.start - evidence_timestamp <= 60 * 86_400
        })
        .filter(|event| {
            let name_tokens = meaningful_event_tokens(&event.name);
            let shared = phrase_tokens.intersection(&name_tokens).count();
            name_tokens.len() >= 2
                && (shared >= 2
                    || (shared > 0
                        && rules.is_some_and(|cache| {
                            cache.noul(RULE_EVENT_MATCH, &event_match_state(phrase, &event.name))
                        })))
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProgressSnapshot {
    pub conversation_index: usize,
    pub in_flight: usize,
    pub request_started_unix: i64,
}

#[derive(Default)]
struct ProgressState {
    conversation_index: usize,
    active_requests: BTreeMap<usize, i64>,
}

#[derive(Default)]
pub struct ScanProgress {
    pub processed: AtomicUsize,
    pub total: AtomicUsize,
    pub cancel: AtomicBool,
    /// Total conversations `scan_conversations` will analyze, or total
    /// conversations `scan_closures` will check -- whichever pass is
    /// currently running.
    pub conversation_total: AtomicUsize,
    /// The started count and active per-worker request clocks are changed
    /// under one lock, so the UI can read a self-consistent snapshot. The
    /// oldest published timestamp is the minimum of the requests still
    /// running rather than an approximation based on the first worker.
    state: Mutex<ProgressState>,
    /// Set once `scan_closures` starts (and left set for the rest of the
    /// scan): tells the desktop to label `conversation_index`/
    /// `conversation_total` as the closure pass ("Closure check {i} of
    /// {n}") rather than the primary per-conversation pass ("Conversation
    /// {i} of {n}").
    pub closure_phase: AtomicBool,
    /// Set only while decision-model paragraph triage is running.
    pub triage_phase: AtomicBool,
}

impl ScanProgress {
    pub fn snapshot(&self) -> ProgressSnapshot {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        ProgressSnapshot {
            conversation_index: state.conversation_index,
            in_flight: state.active_requests.len(),
            request_started_unix: state.active_requests.values().copied().min().unwrap_or(0),
        }
    }

    fn start_request(&self, worker: usize) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.conversation_index += 1;
        state
            .active_requests
            .insert(worker, chrono::Utc::now().timestamp());
    }

    fn finish_request(&self, worker: usize, processed: usize, sent: bool) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.active_requests.remove(&worker);
        if !sent {
            state.conversation_index = state.conversation_index.saturating_sub(1);
        }
        drop(state);
        if sent {
            self.processed.fetch_add(processed, Ordering::Relaxed);
        }
    }

    fn reset_pass(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.conversation_index = 0;
        debug_assert!(state.active_requests.is_empty());
    }
}
pub struct ScanResult {
    pub analysis: LoopItems,
    pub failures: Vec<String>,
    pub failed_conversations_detail: Vec<ConversationFailure>,
    pub analyzed: usize,
    pub total: usize,
    pub cancelled: bool,
    /// Diagnostics for display in the "Scan coverage and errors" panel.
    /// Most entries are per-conversation: one note per conversation that had
    /// a rejection, degraded item, or (for a two-party thread) returned no
    /// expectations at all. A few are aggregate, scan-wide notes pushed by
    /// the closure pass (`scan_closures`) rather than tied to
    /// any single conversation -- how many suggested updates it attached, and
    /// whether its per-scan provider-call cap bound. In-memory UI text
    /// only -- built
    /// from message subjects, so it must never be logged, saved, or emitted
    /// by [`probe`].
    pub conversation_notes: Vec<String>,
    /// Rejected and degraded counts attributable to each analyzed conversation.
    pub conversation_quality: BTreeMap<String, (usize, usize)>,
    /// Display notes attributable to each analyzed conversation.
    pub conversation_notes_by_id: BTreeMap<String, Vec<String>>,
    /// Validator rejection reasons attributable to each analyzed conversation.
    pub conversation_rejection_reasons: BTreeMap<String, Vec<&'static str>>,
    /// Number of open loops the closure pass (`scan_closures`) attached a
    /// pending [`SuggestedUpdate`] to.
    pub suggested_updates: usize,
    pub event_closures: usize,
    /// Set when `scan_conversations` stopped early because a conversation's
    /// analysis failed with a transport-class provider error (the same
    /// `matches!` set that stops the main scan). When true, `scan_closures`
    /// skips the closure pass entirely instead of repeating
    /// calls against a provider already known to be unreachable or
    /// unauthorized.
    pub primary_scan_transport_error: bool,
    /// Total number of conversations the scan grouped `analysis`'s source
    /// messages into (`conversations_by_size`'s job count), independent of
    /// how many messages each one carries.
    pub conversation_count: usize,
    /// Conversations `merge_conversation` folded in as
    /// `JobOutcome::Completed(Ok(_))`.
    pub analyzed_conversations: usize,
    /// Conversations that were dispatched and came back as an error (or
    /// panicked), including every Quota failure even though repeats
    /// collapse to one `failures` line.
    pub failed_conversations: usize,
    /// Conversations still queued when the pass stopped and so never
    /// dispatched at all -- the `run_jobs` indices with no outcome.
    pub not_started_conversations: usize,
    /// Content-free diagnostic set when the closure pass itself
    /// stopped early on a transport-class provider error, e.g.
    /// `"Closure pass stopped: rate limited"`. Kept separate
    /// from `failures` (which counts unanalyzed conversations) since a
    /// closure-pass failure does not mean any conversation went unanalyzed.
    pub closure_pass_failure: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationFailure {
    pub conversation: String,
    pub subject_short: String,
    pub reason: FailureReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailureReason {
    Timeout,
    RateLimited,
    Transport,
    Quota,
    Provider(String),
    Panicked,
    NotStarted,
}

impl ScanResult {
    /// Conversations `analysis` has no coverage for at all: dispatched and
    /// failed, plus never dispatched because the pass stopped first. The
    /// coverage panel and `set_scan`'s summary both report this instead of
    /// `failures.len()`, which undercounts once repeated Quota failures
    /// collapse to one line.
    pub fn unanalyzed_conversations(&self) -> usize {
        self.failed_conversations + self.not_started_conversations
    }
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

fn participant_display_name(label: &str) -> Option<&str> {
    let name = label.split_once('<')?.0.trim();
    (!name.is_empty()).then_some(name)
}

fn participant_given_name(label: &str) -> Option<&str> {
    participant_display_name(label)?.split_whitespace().next()
}

fn begins_with_non_user_vocative(text: &str, source: &ConversationMessage) -> bool {
    let Some((vocative, _)) = text.trim_start().split_once(',') else {
        return false;
    };
    let vocative = vocative.trim();
    if vocative.is_empty()
        || source
            .user_given_name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(vocative))
    {
        return false;
    }
    source
        .message
        .sender
        .iter()
        .chain(&source.message.to)
        .chain(&source.message.cc)
        .filter(|participant| !participant_is_user(participant, &source.own_addresses))
        .any(|participant| {
            let label = participant.as_string();
            participant_given_name(&label).is_some_and(|name| name.eq_ignore_ascii_case(vocative))
        })
}

fn resolve_owner_with_vocative_guard(
    claim_type: ClaimType,
    owner: Owner,
    text: &str,
    source: &ConversationMessage,
) -> (Owner, bool) {
    let guarded = matches!(claim_type, ClaimType::Request | ClaimType::Question)
        && owner == Owner::You
        && begins_with_non_user_vocative(text, source);
    if guarded {
        (Owner::Unclear, true)
    } else {
        (resolve_owner(owner, source), false)
    }
}

/// Corrects the known recap-service attribution failure after the model
/// response has been parsed, while the source conversation and account
/// addresses are still available. This is intentionally part of projection:
/// every later closure and coverage pass sees the corrected item.
fn correct_recap_attribution(
    analysis: &mut LoopItems,
    conversation: &[&ReviewMessage],
    rules: Option<&RuleDecisions>,
) {
    for item in &mut analysis.items {
        if item.waiting_party == "Not established" {
            continue;
        }
        let Some(source) = conversation
            .iter()
            .find(|message| message.input.handle == item.evidence.message)
        else {
            continue;
        };
        if !is_meeting_recap_artifact_with_rules(source, rules) {
            continue;
        }
        let Some(waiting_address) = waiting_party_address(&item.waiting_party) else {
            continue;
        };
        let waiting_on_owner = source
            .own_addresses
            .iter()
            .any(|address| address.eq_ignore_ascii_case(&waiting_address));
        let sender_label = source
            .input
            .message
            .sender
            .as_ref()
            .map(CanonicalBlock::as_string);
        let waiting_on_service = sender_label.as_deref().is_some_and(|sender| {
            waiting_party_address(sender)
                .is_some_and(|address| address.eq_ignore_ascii_case(&waiting_address))
        });
        if !waiting_on_owner && !waiting_on_service {
            continue;
        }
        if waiting_on_service
            && participant_display_name(&item.waiting_party).is_some_and(|waiting_name| {
                !waiting_name.eq_ignore_ascii_case("you")
                    && sender_label
                        .as_deref()
                        .and_then(participant_display_name)
                        .is_none_or(|sender_name| !waiting_name.eq_ignore_ascii_case(sender_name))
            })
        {
            continue;
        }
        item.waiting_party = "Not established".into();
        item.owner = Owner::You;
    }
}

fn tag_call_summaries(
    analysis: &mut LoopItems,
    conversation: &[&ReviewMessage],
    rules: Option<&RuleDecisions>,
) {
    for item in &mut analysis.items {
        let Some(source) = conversation
            .iter()
            .find(|message| message.input.handle == item.evidence.message)
        else {
            continue;
        };
        if is_meeting_recap_artifact_with_rules(source, rules) {
            item.from_call_summary = true;
            if let Some((meeting_time, approx)) = recap_meeting_time(source) {
                item.meeting_time = Some(meeting_time);
                item.meeting_time_approx = approx;
            }
        }
    }
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
    item: &LoopItem,
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

/// Later messages the signed-in user sent in the expectation's own
/// conversation. Keeps the newest 8, presented chronologically to the model.
pub fn same_thread_closure_candidates<'a>(
    all: &'a [ReviewMessage],
    account: &str,
    evidence_conversation: &str,
    evidence_timestamp: i64,
) -> Vec<&'a ReviewMessage> {
    let mut candidates: Vec<&ReviewMessage> = all
        .iter()
        .filter(|message| {
            message.account == account
                && message.conversation == evidence_conversation
                && message.input.from_user
                && message.input.timestamp > evidence_timestamp
        })
        .collect();
    candidates.sort_by_key(|message| std::cmp::Reverse(message.input.timestamp));
    candidates.truncate(8);
    candidates.sort_by_key(|message| message.input.timestamp);
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
/// Upper bound on body blocks one message contributes, leaving
/// room under the 64-block message cap for the subject and up to
/// `REPLY_HISTORY_MAX_CHUNKS` quote chunks. Paragraphs past the bound are
/// folded into the last block rather than dropped: body text is evidence
/// and is never discarded.
const MAX_BODY_BLOCKS: usize = 40;

/// Splits one or more current-body blocks into blank-line-separated paragraph
/// blocks. Applying this after the HTML walker as well as to plain text keeps
/// the governed pipeline's whole-block evidence paragraph-sized. The cap is
/// shared across all source blocks in one message.
fn paragraph_body_blocks<'a>(source_blocks: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut paragraphs = Vec::new();
    for source in source_blocks {
        let normalized = source.replace("\r\n", "\n");
        paragraphs.extend(
            normalized
                .split("\n\n")
                .map(str::trim)
                .filter(|paragraph| !paragraph.is_empty())
                .map(ToOwned::to_owned),
        );
    }
    if paragraphs.len() > MAX_BODY_BLOCKS {
        let tail = paragraphs.split_off(MAX_BODY_BLOCKS - 1);
        paragraphs.push(tail.join("\n\n"));
    }
    paragraphs
}

fn plain_body_blocks(body: &str) -> Vec<String> {
    paragraph_body_blocks([body])
}

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

/// Whether the signed-in user sent this message, and how they appear among
/// its recipients, from the account's own addresses.
fn user_relation(item: &MailItem) -> (bool, crate::claim_view::UserRecipient) {
    let is_own = |address: &String| {
        item.own_addresses
            .iter()
            .any(|own| address.eq_ignore_ascii_case(own))
    };
    let from_user = is_own(&item.sender_address);
    let recipient = if item.to.iter().any(is_own) {
        crate::claim_view::UserRecipient::To
    } else if item.cc.iter().any(is_own) {
        crate::claim_view::UserRecipient::Cc
    } else {
        crate::claim_view::UserRecipient::NotAddressed
    };
    (from_user, recipient)
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
            paragraph_body_blocks(walked.body_blocks.iter().map(String::as_str))
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
        let body_blocks = plain_body_blocks(&body)
            .iter()
            .map(|s| block(s))
            .collect::<Result<Vec<_>, _>>()?;
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
    let (from_user, recipient) = user_relation(item);
    let other_addresses = other_addresses(item);
    Ok(ReviewMessage {
        input: ConversationMessage {
            handle: format!("m{index}"),
            timestamp,
            from_user,
            recipient,
            own_addresses: item.own_addresses.clone(),
            user_display_name: item.own_display_name.clone(),
            user_given_name: item.own_given_name.clone(),
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
        own_addresses: item.own_addresses.clone(),
        own_display_name: item.own_display_name.clone(),
        own_given_name: item.own_given_name.clone(),
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

/// Builds the selected provider's client. Both adapters validate the key
/// and confirm the exact model before any message text is transmitted.
/// # Errors
/// Returns the adapter's fixed provider error; no upstream text escapes.
/// Connects the selected provider, telling the client how many requests
/// the caller may keep in flight: the Ollama Cloud plan's concurrent
/// slots, or the `OpenRouter` ceiling from the Connections tab.
fn connect(
    provider: Provider,
    key: String,
    model: &str,
    parallel: usize,
) -> Result<Box<dyn ModelClient>, ProviderError> {
    Ok(match provider {
        Provider::OllamaCloud => {
            Box::new(OllamaCloud::connect(key, model)?.with_max_parallel(parallel))
        }
        Provider::OpenRouter => {
            Box::new(OpenRouter::connect(key, model)?.with_max_parallel(parallel))
        }
    })
}

/// One participant slot's opaque governed handle (`"{m}-sender"`,
/// `"{m}-to-{i}"`, `"{m}-cc-{i}"`), the message handle it belongs to, and
/// which slot it names -- the per-conversation catalog
/// [`governed_pass`] offers as `SuppliedContext::participants`.
fn governed_participant_handles(
    conversation: &[ConversationMessage],
) -> Vec<(String, &str, ParticipantSlot, bool)> {
    let mut handles = Vec::new();
    for m in conversation {
        if m.message.sender.is_some() {
            handles.push((
                format!("{}-sender", m.handle),
                m.handle.as_str(),
                ParticipantSlot::Sender,
                m.from_user,
            ));
        }
        for i in 0..m.message.to.len() {
            handles.push((
                format!("{}-to-{i}", m.handle),
                m.handle.as_str(),
                ParticipantSlot::To(i),
                participant_is_user(&m.message.to[i], &m.own_addresses),
            ));
        }
        for i in 0..m.message.cc.len() {
            handles.push((
                format!("{}-cc-{i}", m.handle),
                m.handle.as_str(),
                ParticipantSlot::Cc(i),
                participant_is_user(&m.message.cc[i], &m.own_addresses),
            ));
        }
    }
    handles
}

fn participant_is_user(block: &CanonicalBlock, own_addresses: &[String]) -> bool {
    waiting_party_address(&block.as_string()).is_some_and(|address| {
        own_addresses
            .iter()
            .any(|own| own.eq_ignore_ascii_case(&address))
    })
}

/// The [`ParseContext`] one message's temporal hypotheses reparse against:
/// the same construction `deadline_view::classify` uses, so a governed
/// deadline uses the same deterministic aging rules as every review card.
fn governed_temporal_context(m: &ConversationMessage) -> ParseContext {
    ParseContext {
        message_timestamp: UnixSeconds(m.timestamp),
        timezone: TimezoneContext {
            base_offset_seconds: local_offset_seconds(m.timestamp, 0),
            transition: None,
        },
        eod_seconds_since_midnight: DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT,
        week_start: Weekday::Monday,
    }
}

/// Runs one conversation through the ADR-007 governed pipeline
/// ([`analyze_claims`]) and bridges its typed claims back onto the
/// [`LoopItem`] shape every downstream consumer (`close_passed_events`,
/// the review UI, decision fingerprinting) already understands. This is
/// [`scan`]'s only call site for its primary pass, which offers no loop
/// handles: closure, deadline-change and modification claims are the
/// closure pass's job (`scan_closures`), so the primary pass skips them.
/// # Errors
/// Returns the fixed provider or validation failure [`analyze_claims`] gave.
fn governed_pass(
    client: &dyn ModelClient,
    conversation: &[ConversationMessage],
    cancel: Option<&AtomicBool>,
) -> Result<LoopItems, ProviderError> {
    let analysis = governed_call(client, conversation, &[], cancel)?;
    Ok(map_claim_analysis(&analysis, conversation))
}

fn governed_pass_omitting(
    client: &dyn ModelClient,
    conversation: &[ConversationMessage],
    cancel: Option<&AtomicBool>,
    omitted_body_blocks: &BTreeMap<String, BTreeSet<usize>>,
) -> Result<LoopItems, ProviderError> {
    let analysis = governed_call_omitting(client, conversation, &[], cancel, omitted_body_blocks)?;
    Ok(map_claim_analysis(&analysis, conversation))
}

/// One governed request over `conversation`, offering `loop_handles` as the
/// `loop_candidate_handles` the model may name in `related_loop_handles`.
/// The primary pass offers none; the closure pass offers the open loops
/// reachable from the conversation.
/// # Errors
/// Returns the fixed provider or validation failure [`analyze_claims`] gave.
fn governed_call(
    client: &dyn ModelClient,
    conversation: &[ConversationMessage],
    loop_handles: &[&str],
    cancel: Option<&AtomicBool>,
) -> Result<ClaimAnalysis, ProviderError> {
    governed_call_omitting(client, conversation, loop_handles, cancel, &BTreeMap::new())
}

fn governed_call_omitting(
    client: &dyn ModelClient,
    conversation: &[ConversationMessage],
    loop_handles: &[&str],
    cancel: Option<&AtomicBool>,
    omitted_body_blocks: &BTreeMap<String, BTreeSet<usize>>,
) -> Result<ClaimAnalysis, ProviderError> {
    let messages: Vec<MessageContext> = conversation
        .iter()
        .map(|m| MessageContext {
            handle: m.handle.as_str(),
            message: &m.message,
            temporal_context: governed_temporal_context(m),
            from_user: m.from_user,
            to_user: m.to_user(),
            cc_user: m.cc_user(),
        })
        .collect();
    let owned_handles = governed_participant_handles(conversation);
    let participants: Vec<ParticipantHandle> = owned_handles
        .iter()
        .map(
            |(handle, message_handle, slot, is_user)| ParticipantHandle {
                handle: handle.as_str(),
                message_handle,
                slot: *slot,
                is_user: *is_user,
            },
        )
        .collect();
    let context = SuppliedContext {
        user: UserIdentity {
            handle: "user",
            display_name: conversation
                .first()
                .and_then(|message| message.user_display_name.as_deref()),
            given_name: conversation
                .first()
                .and_then(|message| message.user_given_name.as_deref()),
        },
        messages: &messages,
        participants: &participants,
        loop_candidate_handles: loop_handles,
    };
    if omitted_body_blocks.is_empty() {
        analyze_claims(client, &context, cancel)
    } else {
        analyze_claims_omitting(client, &context, cancel, omitted_body_blocks)
    }
}

/// One of the two fixed reasons [`map_accepted_claim`] skips a claim
/// instead of returning an [`LoopItem`] (see [`map_claim_analysis`] for
/// how each is counted and noted).
enum ClaimSkip {
    ClosureOrChange,
    NonBodyEvidence,
}

/// The fixed note recorded once, no matter how many claims triggered it,
/// when the primary pass returned a claim that would close or change an
/// existing loop. The primary pass offers no loop handles; the closure pass
/// handles those claim types.
const CLOSURE_OR_CHANGE_SKIPPED_NOTE: &str =
    "Closure or change claims in the main pass were left to the closure pass.";
/// The fixed note recorded once per claim skipped because its primary
/// evidence did not cite a body or subject block.
const NON_BODY_EVIDENCE_NOTE: &str = "Evidence cited a non-body component.";

/// Bridges one conversation's [`ClaimAnalysis`] onto the [`LoopItems`]
/// shape every downstream consumer already understands.
fn map_claim_analysis(analysis: &ClaimAnalysis, conversation: &[ConversationMessage]) -> LoopItems {
    let mut items = Vec::new();
    let mut rejection_reasons: Vec<&'static str> = Vec::new();
    let mut skipped = 0usize;
    let mut skipped_closure_or_change = false;
    for accepted in &analysis.accepted {
        match map_accepted_claim(accepted, conversation) {
            Ok(expectation) => push_unique_expectation(&mut items, expectation),
            Err(ClaimSkip::ClosureOrChange) => {
                skipped += 1;
                skipped_closure_or_change = true;
            }
            Err(ClaimSkip::NonBodyEvidence) => {
                skipped += 1;
                rejection_reasons.push(NON_BODY_EVIDENCE_NOTE);
            }
        }
    }
    if skipped_closure_or_change {
        rejection_reasons.push(CLOSURE_OR_CHANGE_SKIPPED_NOTE);
    }
    for reason in &analysis.rejected {
        rejection_reasons.push(rejection_label(*reason));
    }
    LoopItems {
        items,
        rejected: analysis.rejected.len() + skipped,
        rejection_reasons,
        degraded: 0,
    }
}

/// The mapping applied to every claim type that can attach to an
/// `LoopItem`: its `kind` string, its deterministic `action` prefix, and
/// the owner to start from before [`resolve_owner`] downgrades it.
/// `PossibleClosure`, `DeadlineChange` and `Modification` return `None`:
/// they attach to an existing loop rather than create one, which only the
/// closure pass (`scan_closures`) does.
/// Builds one [`LoopItem`] from an accepted claim, or reports which of
/// the two fixed reasons it was skipped for instead.
fn map_accepted_claim(
    accepted: &AcceptedClaim,
    conversation: &[ConversationMessage],
) -> Result<LoopItem, ClaimSkip> {
    let claim = &accepted.claim;
    let Some((kind, action_prefix, base_owner)) = claim_shape(claim.claim_type) else {
        return Err(ClaimSkip::ClosureOrChange);
    };
    // `evidence` is never empty: the schema requires `minItems: 1`.
    let primary = &accepted.evidence[0];
    if !matches!(
        primary.component,
        EvidenceComponent::BodyBlock | EvidenceComponent::Subject
    ) {
        return Err(ClaimSkip::NonBodyEvidence);
    }
    // `validate()` step 4 already proved every evidence source_handle
    // resolves against the messages this conversation supplied.
    let Some(source_message) = conversation
        .iter()
        .find(|m| m.handle == primary.source_handle)
    else {
        return Err(ClaimSkip::NonBodyEvidence);
    };
    let action = card_action(action_prefix, &primary.text);
    let action_phrase = action_phrase(&primary.text);
    // The model saying it cannot tell who asks or who owes outranks the
    // claim type's default owner: a card must not read "You (suggested)"
    // next to an uncertainty note that says the opposite.
    let base_owner = if claim.ambiguity_codes.contains(&AmbiguityCode::Identity) {
        Owner::Unclear
    } else {
        base_owner
    };
    let (owner, vocative_guard) = resolve_owner_with_vocative_guard(
        claim.claim_type,
        base_owner,
        &primary.text,
        source_message,
    );
    let mut ambiguity_codes = claim.ambiguity_codes.clone();
    if vocative_guard && !ambiguity_codes.contains(&AmbiguityCode::Identity) {
        ambiguity_codes.push(AmbiguityCode::Identity);
    }
    let waiting_party = waiting_party_display(&claim.waiting_party_handle, conversation);
    let (deadline, event) = temporal_anchors(claim, &accepted.evidence);
    let evidence = Anchor {
        message: primary.source_handle.clone(),
        block: usize::from(primary.block_ordinal),
        quote: primary.text.clone(),
        context: primary.text.clone(),
    };
    Ok(LoopItem {
        action,
        action_phrase,
        owner,
        waiting_party,
        kind: kind.to_string(),
        evidence,
        deadline,
        deadline_kind_hint: None,
        event,
        event_time: None,
        resolution: None,
        resolution_kind: None,
        uncertainty: uncertainty_text(claim.claim_type, &ambiguity_codes),
        unverified_deadline: false,
        unverified_resolution: false,
        cross_thread: false,
        event_passed: None,
        suggested_update: None,
        from_call_summary: false,
        meeting_time: None,
        meeting_time_approx: false,
    })
}

/// The `deadline`/`event` anchor pair for one claim's temporal hypothesis
/// (if any): a `date`/`local_datetime`/`relative`/`soft_window` value
/// anchors `deadline`; an `event_relative` one anchors `event` instead,
/// leaving `deadline` `None`. `quote` is the model-normalized temporal
/// value itself (never the surrounding evidence text), matching what
/// `deadline_view::classify` and `close_passed_events` already expect to
/// reparse.
fn temporal_anchors(
    claim: &Claim,
    evidence: &[ReviewEvidence],
) -> (Option<Anchor>, Option<Anchor>) {
    let Nullable::Value(temporal) = &claim.temporal else {
        return (None, None);
    };
    // Step 8 already proved this index addresses a real evidence entry.
    let Some(source) = evidence.get(usize::from(temporal.text_evidence_index)) else {
        return (None, None);
    };
    let anchor = Anchor {
        message: source.source_handle.clone(),
        block: usize::from(source.block_ordinal),
        quote: temporal.value.clone(),
        context: source.text.clone(),
    };
    match temporal.kind {
        TemporalKind::Date
        | TemporalKind::LocalDatetime
        | TemporalKind::Relative
        | TemporalKind::SoftWindow => (Some(anchor), None),
        TemporalKind::EventRelative => (None, Some(anchor)),
    }
}

/// Pushes `item` unless an item with the same action (case-insensitively)
/// and the same evidence message is already present.
fn push_unique_expectation(items: &mut Vec<LoopItem>, item: LoopItem) {
    let is_duplicate = items.iter().any(|existing| {
        existing.action.eq_ignore_ascii_case(&item.action)
            && existing.evidence.message == item.evidence.message
    });
    if !is_duplicate {
        items.push(item);
    }
}

const TRIAGE_IDS: &[&str] = &[
    "triage.asks_recipient",
    "triage.commits_sender",
    "triage.asks_question",
    "triage.names_time",
    "triage.boilerplate",
    "triage.automated_notification",
];

const RULE_RECAP: &str = "rules.recap";
const RULE_SCOPED_EVENT: &str = "rules.scoped_event";
const RULE_EVENT_MATCH: &str = "rules.event_match";
const RULE_DUPLICATE_ACTION: &str = "rules.duplicate_action";
const RULE_THREAD_MERGE: &str = "rules.thread_merge";
const RULE_DEADLINE_KIND: &str = "rules.deadline_kind";

#[derive(Clone, Copy)]
enum RuleCategory {
    Recap,
    Event,
    Duplicate,
    Thread,
    Deadline,
}

#[derive(Default)]
struct RuleDecisions {
    answers: BTreeMap<(String, Vec<u8>), Option<Answer>>,
    recap: usize,
    event: usize,
    duplicates: usize,
    threads: usize,
    deadlines: usize,
    skipped: usize,
}

impl RuleDecisions {
    fn key(id: &str, state: &serde_json::Value) -> (String, Vec<u8>) {
        (
            id.to_owned(),
            serde_json::to_vec(state).expect("rule state is serializable"),
        )
    }

    fn noul(&self, id: &str, state: &serde_json::Value) -> bool {
        let accept = Registry::get()
            .question(id)
            .expect("required rule question")
            .accept;
        matches!(
            self.answers.get(&Self::key(id, state)),
            Some(Some(Answer::Noul { probability })) if *probability >= accept
        )
    }

    fn choice(&self, id: &str, state: &serde_json::Value) -> Option<&str> {
        let accept = Registry::get()
            .question(id)
            .expect("required rule question")
            .accept;
        match self.answers.get(&Self::key(id, state)) {
            Some(Some(Answer::Choice {
                choice, confidence, ..
            })) if *confidence >= accept => Some(choice),
            _ => None,
        }
    }

    fn record_answer(&mut self, category: RuleCategory) {
        match category {
            RuleCategory::Recap => self.recap += 1,
            RuleCategory::Event => self.event += 1,
            RuleCategory::Duplicate => self.duplicates += 1,
            RuleCategory::Thread => self.threads += 1,
            RuleCategory::Deadline => self.deadlines += 1,
        }
    }

    fn note(&self) -> String {
        let answered = self.recap + self.event + self.duplicates + self.threads + self.deadlines;
        format!(
            "Decision model answered {answered} rule questions (recap {}, event {}, duplicates {}, threads {}, deadlines {}); {} skipped.",
            self.recap, self.event, self.duplicates, self.threads, self.deadlines, self.skipped
        )
    }
}

fn decide_rule_states(
    id: &str,
    category: RuleCategory,
    states: Vec<serde_json::Value>,
    cache: &mut RuleDecisions,
    client: &dyn DecisionClient,
    progress: &ScanProgress,
) -> Result<(), ProviderError> {
    let mut unique = BTreeMap::new();
    for state in states {
        let key = RuleDecisions::key(id, &state);
        if !cache.answers.contains_key(&key) {
            unique.entry(key).or_insert(state);
        }
    }
    if unique.is_empty() {
        return Ok(());
    }
    let jobs: Vec<_> = unique.into_iter().collect();
    let questions = Questions::from_registry(&[id])?;
    let pass = ParallelPass::new(client.max_parallel());
    let processed = vec![1; jobs.len()];
    let outcomes = run_jobs(&processed, &pass, progress, &|slot| {
        client
            .decide(
                &jobs[slot].1,
                &questions,
                Some(&progress.cancel),
                DECISION_DEADLINE,
            )
            .map(|answers| answers.get(id).cloned())
    });
    let mut completed = BTreeSet::new();
    for (slot, outcome) in outcomes {
        completed.insert(slot);
        let answer = match outcome {
            JobOutcome::Completed(Ok(answer)) => answer,
            JobOutcome::Completed(Err(_)) | JobOutcome::Panicked | JobOutcome::NotStarted => None,
        };
        if answer.is_some() {
            cache.record_answer(category);
        } else {
            cache.skipped += 1;
        }
        cache.answers.insert(jobs[slot].0.clone(), answer);
    }
    for (slot, (key, _)) in jobs.iter().enumerate() {
        if !completed.contains(&slot) {
            cache.skipped += 1;
            cache.answers.insert(key.clone(), None);
        }
    }
    Ok(())
}

fn recap_rule_states(messages: &[ReviewMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .filter(|message| recap_residue_candidate(message))
        .map(recap_state)
        .collect()
}

fn duplicate_rule_states(items: &[LoopItem]) -> Vec<serde_json::Value> {
    let mut states = Vec::new();
    for (index, a) in items.iter().enumerate() {
        for b in &items[index + 1..] {
            if a.evidence.message == b.evidence.message && !a.action.eq_ignore_ascii_case(&b.action)
            {
                states.push(serde_json::json!({"action_a": a.action, "action_b": b.action}));
            }
        }
    }
    states
}

fn apply_duplicate_rules(items: &mut Vec<LoopItem>, rules: &RuleDecisions) {
    let mut duplicates = BTreeSet::new();
    for (index, a) in items.iter().enumerate() {
        for (other, b) in items.iter().enumerate().skip(index + 1) {
            if a.evidence.message != b.evidence.message || a.action.eq_ignore_ascii_case(&b.action)
            {
                continue;
            }
            let state = serde_json::json!({"action_a": a.action, "action_b": b.action});
            if rules.noul(RULE_DUPLICATE_ACTION, &state) {
                duplicates.insert(other);
            }
        }
    }
    let mut index = 0usize;
    items.retain(|_| {
        let keep = !duplicates.contains(&index);
        index += 1;
        keep
    });
}

fn deadline_rule_states(items: &[LoopItem], messages: &[ReviewMessage]) -> Vec<serde_json::Value> {
    items
        .iter()
        .filter_map(|item| {
            let deadline = item.deadline.as_ref()?;
            let timestamp = messages
                .iter()
                .find(|message| message.input.handle == deadline.message)
                .map_or(0, |message| message.input.timestamp);
            matches!(
                classify(&deadline.quote, timestamp, timestamp, 0),
                DeadlineView::Unknown
            )
            .then_some(deadline)
        })
        .map(|deadline| serde_json::json!({"phrase": deadline.quote}))
        .collect()
}

fn apply_deadline_rules(items: &mut [LoopItem], messages: &[ReviewMessage], rules: &RuleDecisions) {
    for item in items {
        let Some(deadline) = &item.deadline else {
            continue;
        };
        let timestamp = messages
            .iter()
            .find(|message| message.input.handle == deadline.message)
            .map_or(0, |message| message.input.timestamp);
        if !matches!(
            classify(&deadline.quote, timestamp, timestamp, 0),
            DeadlineView::Unknown
        ) {
            continue;
        }
        let state = serde_json::json!({"phrase": deadline.quote});
        item.deadline_kind_hint = match rules.choice(RULE_DEADLINE_KIND, &state) {
            Some("event_tied") => Some(DeadlineKindHint::EventTied),
            Some("soft") => Some(DeadlineKindHint::Soft),
            Some("unknown") => Some(DeadlineKindHint::Unknown),
            _ => None,
        };
    }
}

fn event_rule_states(
    items: &[LoopItem],
    messages: &[ReviewMessage],
    index: &[EventRef],
    now: i64,
) -> Vec<serde_json::Value> {
    let mut states = Vec::new();
    for item in items {
        if item.resolution.is_some() || item.event_passed.is_some() {
            continue;
        }
        if !has_scoped_event_language(item) {
            states.push(scoped_event_state(item));
        }
        let Some(source) = messages
            .iter()
            .find(|message| message.input.handle == item.evidence.message)
        else {
            continue;
        };
        let offset = local_offset_seconds(source.input.timestamp, 0);
        let named = named_event_phrase(item, source.input.timestamp, now, offset);
        if let Some(phrase) = named {
            collect_event_match_states(&mut states, phrase, source.input.timestamp, index, false);
        } else {
            collect_event_match_states(
                &mut states,
                &item.action,
                source.input.timestamp,
                index,
                true,
            );
            collect_event_match_states(
                &mut states,
                &item.evidence.quote,
                source.input.timestamp,
                index,
                true,
            );
        }
    }
    states
}

fn collect_event_match_states(
    states: &mut Vec<serde_json::Value>,
    phrase: &str,
    evidence_timestamp: i64,
    index: &[EventRef],
    text_match: bool,
) {
    let phrase_tokens = meaningful_event_tokens(phrase);
    if phrase_tokens.is_empty() {
        return;
    }
    for event in index {
        if event.start < evidence_timestamp
            || event.start - evidence_timestamp > 60 * 86_400
            || (text_match && !matches!(event.source, EventSource::Meeting | EventSource::Subject))
        {
            continue;
        }
        let name_tokens = meaningful_event_tokens(&event.name);
        let shared = phrase_tokens.intersection(&name_tokens).count();
        let fast = if text_match {
            name_tokens.len() >= 2 && shared >= 2
        } else {
            shared > 0
                && (name_tokens.len() < 2 || shared >= 2 || phrase_tokens.is_subset(&name_tokens))
        };
        if shared > 0 && (!text_match || name_tokens.len() >= 2) && !fast {
            states.push(event_match_state(phrase, &event.name));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SignalBand {
    Signal,
    Gray,
    Negative,
}

#[derive(Clone, Copy, Debug)]
struct ParagraphTriage {
    signal: SignalBand,
    boilerplate: bool,
    notification: bool,
}

#[derive(Default)]
struct TriageResult {
    skipped: BTreeSet<(String, String)>,
    omitted_body_blocks: BTreeMap<String, BTreeSet<usize>>,
    boilerplate_dropped: usize,
    request_errors: usize,
    stop_class_error: bool,
    cancelled: bool,
}

fn triage_probability(answers: &openloops_inference::decision::Answers, id: &str) -> f64 {
    match answers.get(id) {
        Some(Answer::Noul { probability }) => *probability,
        _ => 0.0,
    }
}

fn classify_triage(answers: &openloops_inference::decision::Answers) -> ParagraphTriage {
    let registry = Registry::get();
    let signal_ids = &TRIAGE_IDS[..3];
    let signal = if signal_ids.iter().any(|id| {
        triage_probability(answers, id) >= registry.question(id).expect("triage question").accept
    }) {
        SignalBand::Signal
    } else if signal_ids.iter().any(|id| {
        let probability = triage_probability(answers, id);
        let question = registry.question(id).expect("triage question");
        probability >= question.escalate && probability < question.accept
    }) {
        SignalBand::Gray
    } else {
        SignalBand::Negative
    };
    let below_signal_escalation = signal_ids.iter().all(|id| {
        triage_probability(answers, id) < registry.question(id).expect("triage question").escalate
    });
    let boilerplate = triage_probability(answers, "triage.boilerplate")
        >= registry
            .question("triage.boilerplate")
            .expect("triage question")
            .accept
        && below_signal_escalation;
    let notification = triage_probability(answers, "triage.automated_notification")
        >= registry
            .question("triage.automated_notification")
            .expect("triage question")
            .accept;
    ParagraphTriage {
        signal,
        boilerplate,
        notification,
    }
}

fn triage_state(message: &ReviewMessage, ordinal: usize) -> serde_json::Value {
    serde_json::json!({
        "subject": message.input.message.subject.as_string(),
        "paragraph_text": message.input.message.body_blocks[ordinal].as_string(),
        "from_user": message.input.from_user,
    })
}

fn triage_jobs(messages: &[&ReviewMessage]) -> Vec<(usize, usize)> {
    messages
        .iter()
        .enumerate()
        .flat_map(|(message_index, message)| {
            (0..message.input.message.body_blocks.len().min(MAX_BODY_BLOCKS))
                .map(move |ordinal| (message_index, ordinal))
        })
        .collect()
}

fn collect_triage_results(
    messages: &[&ReviewMessage],
    jobs: &[(usize, usize)],
    outcomes: JobResults<ParagraphTriage>,
) -> TriageResult {
    let mut result = TriageResult::default();
    let mut classified = vec![Vec::new(); messages.len()];
    for (slot, outcome) in outcomes {
        let (message_index, ordinal) = jobs[slot];
        let paragraph = match outcome {
            JobOutcome::Completed(Ok(answer)) => answer,
            JobOutcome::Completed(Err(ProviderError::Cancelled)) => {
                result.cancelled = true;
                continue;
            }
            JobOutcome::Completed(Err(error)) => {
                result.request_errors += 1;
                result.stop_class_error |= is_stop_error(error);
                ParagraphTriage {
                    signal: SignalBand::Signal,
                    boilerplate: false,
                    notification: false,
                }
            }
            JobOutcome::Panicked => {
                result.request_errors += 1;
                ParagraphTriage {
                    signal: SignalBand::Signal,
                    boilerplate: false,
                    notification: false,
                }
            }
            JobOutcome::NotStarted => continue,
        };
        if paragraph.boilerplate {
            result
                .omitted_body_blocks
                .entry(messages[message_index].input.handle.clone())
                .or_default()
                .insert(ordinal);
            result.boilerplate_dropped += 1;
        }
        classified[message_index].push(paragraph);
    }
    if !result.stop_class_error && !result.cancelled {
        result.skipped = skipped_triage_conversations(messages, &classified);
    } else {
        result.omitted_body_blocks.clear();
        result.boilerplate_dropped = 0;
    }
    result
}

fn skipped_triage_conversations(
    messages: &[&ReviewMessage],
    classified: &[Vec<ParagraphTriage>],
) -> BTreeSet<(String, String)> {
    let mut conversations: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    for (index, message) in messages.iter().enumerate() {
        conversations
            .entry((&message.account, &message.conversation))
            .or_default()
            .push(index);
    }
    conversations
        .into_iter()
        .filter_map(|((account, conversation), message_indices)| {
            let has_signal = message_indices.iter().any(|index| {
                classified[*index]
                    .iter()
                    .any(|answer| answer.signal != SignalBand::Negative)
            });
            let notification_only = message_indices.iter().all(|index| {
                !classified[*index].is_empty()
                    && classified[*index].iter().all(|answer| answer.notification)
            });
            (!has_signal || notification_only)
                .then(|| (account.to_owned(), conversation.to_owned()))
        })
        .collect()
}

fn triage_pass(
    messages: &[ReviewMessage],
    progress: &ScanProgress,
    conversation_filter: Option<&BTreeSet<String>>,
    decision_client: &dyn DecisionClient,
) -> Result<TriageResult, ProviderError> {
    let selected: Vec<&ReviewMessage> = messages
        .iter()
        .filter(|message| {
            conversation_filter.is_none_or(|filter| filter.contains(&message.conversation))
        })
        .collect();
    let jobs = triage_jobs(&selected);
    let questions = Questions::from_registry(TRIAGE_IDS)?;
    progress.triage_phase.store(true, Ordering::Relaxed);
    progress.processed.store(0, Ordering::Relaxed);
    progress.total.store(jobs.len(), Ordering::Relaxed);
    progress
        .conversation_total
        .store(jobs.len(), Ordering::Relaxed);
    let pass = ParallelPass::new(decision_client.max_parallel());
    let outcomes = run_jobs(&vec![1; jobs.len()], &pass, progress, &|slot| {
        let (message_index, ordinal) = jobs[slot];
        decision_client
            .decide(
                &triage_state(selected[message_index], ordinal),
                &questions,
                Some(&progress.cancel),
                DECISION_DEADLINE,
            )
            .map(|answers| classify_triage(&answers))
    });
    progress.reset_pass();
    Ok(collect_triage_results(&selected, &jobs, outcomes))
}

fn prepare_rule_messages(
    messages: &[ReviewMessage],
    rules: &mut RuleDecisions,
    decision: &dyn DecisionClient,
    progress: &ScanProgress,
) -> Result<Vec<ReviewMessage>, ProviderError> {
    decide_rule_states(
        RULE_RECAP,
        RuleCategory::Recap,
        recap_rule_states(messages),
        rules,
        decision,
        progress,
    )?;
    decide_rule_states(
        RULE_THREAD_MERGE,
        RuleCategory::Thread,
        thread_rule_states(messages),
        rules,
        decision,
        progress,
    )?;
    let mut merged = messages.to_vec();
    merge_threads_with_rules(&mut merged, Some(rules));
    Ok(merged)
}

fn apply_rule_residue(
    result: &mut ScanResult,
    messages: &[ReviewMessage],
    rules: &mut RuleDecisions,
    decision: &dyn DecisionClient,
    progress: &ScanProgress,
) -> Result<(), ProviderError> {
    let all_messages: Vec<_> = messages.iter().collect();
    correct_recap_attribution(&mut result.analysis, &all_messages, Some(rules));
    tag_call_summaries(&mut result.analysis, &all_messages, Some(rules));

    decide_rule_states(
        RULE_DUPLICATE_ACTION,
        RuleCategory::Duplicate,
        duplicate_rule_states(&result.analysis.items),
        rules,
        decision,
        progress,
    )?;
    apply_duplicate_rules(&mut result.analysis.items, rules);

    decide_rule_states(
        RULE_DEADLINE_KIND,
        RuleCategory::Deadline,
        deadline_rule_states(&result.analysis.items, messages),
        rules,
        decision,
        progress,
    )?;
    apply_deadline_rules(&mut result.analysis.items, messages, rules);

    let now = chrono::Utc::now().timestamp();
    let event_index = build_event_index_with_rules(messages, Some(rules));
    let event_states = event_rule_states(&result.analysis.items, messages, &event_index, now);
    decide_rule_states(
        RULE_SCOPED_EVENT,
        RuleCategory::Event,
        event_states
            .iter()
            .filter(|state| state.get("request_text").is_some())
            .cloned()
            .collect(),
        rules,
        decision,
        progress,
    )?;
    decide_rule_states(
        RULE_EVENT_MATCH,
        RuleCategory::Event,
        event_states
            .into_iter()
            .filter(|state| state.get("phrase").is_some())
            .collect(),
        rules,
        decision,
        progress,
    )?;
    close_passed_events_with_rules(result, messages, now, Some(rules));
    result.conversation_notes.push(rules.note());
    Ok(())
}

/// Analyzes `messages`, running up to `parallel` model requests at once.
///
/// Conversations are independent of one another and so are the closure
/// pass's per-conversation requests, so both passes fan out across a worker pool
/// bounded by what the provider allows. Concurrency adapts downward when
/// the provider rate-limits and back up as requests succeed; a
/// rate-limited request is reported as a failed conversation and never
/// resent, because `network_policy.retries` forbids automatically
/// retrying a request that already carried content.
pub fn scan(
    options: ScanOptions,
    key: String,
    model: &str,
    parallel: usize,
    messages: &[ReviewMessage],
    progress: &ScanProgress,
    conversation_filter: Option<&BTreeSet<String>>,
) -> Result<ScanResult, ProviderError> {
    let selected_messages = messages
        .iter()
        .filter(|message| {
            conversation_filter.is_none_or(|filter| filter.contains(&message.conversation))
        })
        .count();
    let decision_key = (options.provider == Provider::OpenRouter && options.use_decision_model)
        .then(|| key.clone());
    let client = connect(options.provider, key, model, parallel)?;
    let client = client.as_ref();
    let pass = ParallelPass::new(client.max_parallel());
    let decision_client = maybe_decision_client(options, || {
        Ok(OpenRouterDecisions::connect(
            decision_key.expect("decision key exists when enabled"),
            &Registry::get().model,
        )?
        .with_max_parallel(parallel))
    })?;
    let mut rule_decisions = decision_client.as_ref().map(|_| RuleDecisions::default());
    let mut rule_messages = None;
    if let (Some(decision), Some(rules)) = (decision_client.as_ref(), rule_decisions.as_mut()) {
        rule_messages = Some(prepare_rule_messages(messages, rules, decision, progress)?);
    }
    let messages = rule_messages.as_deref().unwrap_or(messages);
    let triage = decision_client
        .as_ref()
        .map(|decision| triage_pass(messages, progress, conversation_filter, decision))
        .transpose()?;
    progress.triage_phase.store(false, Ordering::Relaxed);
    progress.processed.store(0, Ordering::Relaxed);
    progress.total.store(selected_messages, Ordering::Relaxed);
    let mut result = if let Some(triage) = &triage {
        let primary_messages: Vec<ReviewMessage> = messages
            .iter()
            .filter(|message| {
                conversation_filter.is_none_or(|filter| filter.contains(&message.conversation))
                    && !triage
                        .skipped
                        .contains(&(message.account.clone(), message.conversation.clone()))
            })
            .cloned()
            .collect();
        scan_conversations_filtered(&primary_messages, progress, &pass, None, &|conversation| {
            governed_pass_omitting(
                client,
                conversation,
                Some(&progress.cancel),
                &triage.omitted_body_blocks,
            )
        })
    } else {
        scan_conversations_filtered(
            messages,
            progress,
            &pass,
            conversation_filter,
            &|conversation| governed_pass(client, conversation, Some(&progress.cancel)),
        )
    };
    if let Some(triage) = &triage {
        apply_triage_coverage(
            &mut result,
            messages,
            conversation_filter,
            triage,
            selected_messages,
        );
    }
    if let (Some(decision), Some(rules)) = (decision_client.as_ref(), rule_decisions.as_mut()) {
        apply_rule_residue(&mut result, messages, rules, decision, progress)?;
    } else {
        close_passed_events(&mut result, messages, chrono::Utc::now().timestamp());
    }
    let decision_for_closure = if result.primary_scan_transport_error {
        None
    } else {
        decision_client
            .as_ref()
            .map(|value| value as &dyn DecisionClient)
    };
    scan_closures_selected(
        messages,
        progress,
        &mut result,
        &pass,
        client,
        decision_for_closure,
    );
    Ok(result)
}

fn apply_triage_coverage(
    result: &mut ScanResult,
    messages: &[ReviewMessage],
    conversation_filter: Option<&BTreeSet<String>>,
    triage: &TriageResult,
    selected_messages: usize,
) {
    let ordered = conversations_by_size(messages, conversation_filter);
    result.total = selected_messages;
    result.conversation_count = ordered.len();
    for (index, conversation) in ordered.iter().enumerate() {
        let Some(first) = conversation.first() else {
            continue;
        };
        let key = (first.account.clone(), first.conversation.clone());
        if !triage.skipped.contains(&key) {
            continue;
        }
        let note = format!(
            "Conversation {} ({} messages; subject: {}): no obligations found by triage.",
            index + 1,
            conversation.len(),
            subject_snippet(conversation),
        );
        result.analyzed += conversation.len();
        result.analyzed_conversations += 1;
        result.conversation_notes.push(note.clone());
        result
            .conversation_notes_by_id
            .entry(first.conversation.clone())
            .or_default()
            .push(note);
        result
            .conversation_quality
            .insert(first.conversation.clone(), (0, 0));
        result
            .conversation_rejection_reasons
            .insert(first.conversation.clone(), Vec::new());
    }
    let mut note = format!(
        "{} conversations skipped by triage, {} boilerplate paragraphs dropped, {} triage requests skipped (rate limit / errors).",
        triage.skipped.len(),
        triage.boilerplate_dropped,
        triage.request_errors,
    );
    if triage.stop_class_error {
        note.push_str(
            " Triage stopped after a provider error; every conversation continued to the primary pass.",
        );
    }
    if !triage.skipped.is_empty()
        && !result
            .conversation_notes
            .iter()
            .any(|existing| existing.contains("Rescan"))
    {
        note.push_str(" Rescan with the decision model off to analyze skipped conversations.");
    }
    result.conversation_notes.push(note);
    result.cancelled |= triage.cancelled;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanOptions {
    pub provider: Provider,
    pub use_decision_model: bool,
}

fn maybe_decision_client<T>(
    options: ScanOptions,
    connect: impl FnOnce() -> Result<T, ProviderError>,
) -> Result<Option<T>, ProviderError> {
    if options.provider == Provider::OpenRouter && options.use_decision_model {
        connect().map(Some)
    } else {
        Ok(None)
    }
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
/// The first 60 Unicode scalars of `conversation`'s first message's
/// subject (char-safe: a multibyte scalar is never split), used to make a
/// diagnostic line identifiable without including full message text.
/// Shared by [`conversation_note`] and `scan_conversations`'s failure
/// line so both name the same conversation the same way.
fn subject_snippet(conversation: &[&ReviewMessage]) -> String {
    conversation
        .first()
        .map(|m| m.input.message.subject.as_string())
        .unwrap_or_default()
        .chars()
        .take(60)
        .collect()
}

fn conversation_note(
    index: usize,
    conversation: &[&ReviewMessage],
    analysis: &LoopItems,
) -> Option<String> {
    let len = conversation.len();
    let snippet = subject_snippet(conversation);
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

/// Groups `messages` by `(account, conversation)`, then orders the groups
/// smallest-first (stable on the grouping's existing `(account,
/// conversation)` key order for ties), so quick conversations finish --
/// and contribute progress and results -- before a single slow one. Every
/// conversation is analyzed independently of every other, so this
/// ordering never changes what `scan_conversations` finds, only when.
/// Grouped as borrows rather than owned clones: each `ReviewMessage`
/// already lives in `messages`, and the per-conversation `inputs` built
/// during analysis is the only owned copy that pass actually needs.
fn conversations_by_size<'a>(
    messages: &'a [ReviewMessage],
    conversation_filter: Option<&BTreeSet<String>>,
) -> Vec<Vec<&'a ReviewMessage>> {
    let mut conversations: BTreeMap<(&str, &str), Vec<&ReviewMessage>> = BTreeMap::new();
    for m in messages {
        if conversation_filter.is_some_and(|filter| !filter.contains(&m.conversation)) {
            continue;
        }
        conversations
            .entry((&m.account, &m.conversation))
            .or_default()
            .push(m);
    }
    let mut ordered: Vec<Vec<&ReviewMessage>> = conversations.into_values().collect();
    ordered.sort_by_key(Vec::len);
    ordered
}

/// The provider error classes that mean the provider itself is currently
/// unreachable or unusable, rather than this one conversation being
/// rejected: both `scan_conversations` and `scan_closures` stop the whole
/// pass early on one of these instead of continuing past it.
///
/// `Quota` (HTTP 402) is deliberately excluded: live scans have repeatedly
/// shown it fired for one specific conversation while a dozen others on the
/// same account, key, and model succeeded around it, so it is evidence
/// about that one request, not about the provider's health. Treating it as
/// a stop condition cost every not-yet-dispatched conversation behind it
/// for no reason -- it is handled like any other per-conversation failure
/// instead (falls to `job_signal`'s `_ => JobSignal::Ok`).
///
/// `Timeout` is excluded for the same reason: the request deadline is per
/// request, and a slow model on a large conversation hits it while the
/// same model answers every smaller conversation around it. Treating it as
/// a stop condition also set `primary_scan_transport_error`, which skipped
/// the whole closure pass -- so one slow thread silently cost every
/// "already handled" detection in the scan.
fn is_transport_error(error: ProviderError) -> bool {
    matches!(
        error,
        ProviderError::Unauthorized
            | ProviderError::RateLimited
            | ProviderError::Network
            | ProviderError::ServerError(_)
    )
}

/// Consecutive completed requests a pass must see before the adaptive
/// limiter widens the allowed concurrency again.
const RAISE_AFTER_SUCCESSES: usize = 8;

/// How long a worker parked above the current allowed concurrency sleeps
/// before re-checking. Short enough to pick up a widening promptly, long
/// enough not to spin a core.
const PARK_INTERVAL: Duration = Duration::from_millis(25);

/// What one finished job tells the pool's limiter to do next.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum JobSignal {
    /// The provider answered. Whether that answer was usable says nothing
    /// about provider health, so a rejected analysis counts here too.
    Ok,
    /// Rate limited (HTTP 429), or `OpenRouter`'s in-flight credit
    /// reservation (an HTTP 402 that says to retry after in-flight requests
    /// settle): narrow the concurrency. The request is NOT resent --
    /// `network_policy.retries`
    /// forbids automatically retrying any request that carried content --
    /// so the conversation is reported as failed instead.
    Backoff,
    /// The provider is unreachable or unusable, or the caller cancelled:
    /// stop dispatching new jobs. Requests already in flight still finish.
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LimiterState {
    allowed: usize,
    consecutive_successes: usize,
    max: usize,
}

/// Shared state for one parallel pass: the job cursor every worker draws
/// from, the adaptive concurrency limit, and the stop flag.
///
/// The cursor hands jobs out lowest-index-first, so jobs START in the
/// sequential order; [`run_jobs`] sorts the answers back into that order,
/// which is what keeps `conversation_notes`, failure numbering, and
/// `analysis.items` identical to the sequential scan for a deterministic
/// provider.
struct ParallelPass {
    cursor: AtomicUsize,
    /// `allowed` and its success streak change as one linearizable
    /// transition. Workers above `allowed` park on `wake` for at most one
    /// [`PARK_INTERVAL`] before checking stop and cancellation again.
    limiter: Mutex<LimiterState>,
    wake: Condvar,
    stop: AtomicBool,
}

impl ParallelPass {
    fn new(max: usize) -> Self {
        let max = max.clamp(1, MAX_PARALLEL_REQUESTS);
        Self {
            cursor: AtomicUsize::new(0),
            limiter: Mutex::new(LimiterState {
                allowed: max,
                consecutive_successes: 0,
                max,
            }),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
        }
    }

    /// Readies the same pool for a second pass, keeping the concurrency it
    /// already learned from the provider.
    fn restart(&self) {
        self.cursor.store(0, Ordering::Relaxed);
        self.stop.store(false, Ordering::Relaxed);
    }

    fn limiter(&self) -> std::sync::MutexGuard<'_, LimiterState> {
        self.limiter.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(test)]
    fn limiter_state(&self) -> (usize, usize, usize) {
        let state = self.limiter();
        (state.allowed, state.consecutive_successes, state.max)
    }

    fn max(&self) -> usize {
        self.limiter().max
    }

    fn wake_workers(&self) {
        self.wake.notify_all();
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.wake_workers();
    }

    /// Blocks until this worker is inside the allowed concurrency.
    /// `false` when the pass stopped, the caller cancelled, or every job
    /// has already been handed out while this worker was parked -- a
    /// worker narrowed out of its slot must not wait for a widening that
    /// no longer has any work behind it. The stop and cancel checks here
    /// are also the ones made before taking any job.
    fn wait_for_slot(&self, worker: usize, jobs: usize, cancel: &AtomicBool) -> bool {
        let mut limiter = self.limiter();
        loop {
            if self.stop.load(Ordering::Acquire)
                || cancel.load(Ordering::Acquire)
                || self.cursor.load(Ordering::Acquire) >= jobs
            {
                return false;
            }
            if worker < limiter.allowed {
                return true;
            }
            limiter = self
                .wake
                .wait_timeout(limiter, PARK_INTERVAL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// The next job index, or `None` once the jobs run out or the pass
    /// stopped between the slot check and here.
    fn next_job(&self, jobs: usize, cancel: &AtomicBool) -> Option<usize> {
        if self.stop.load(Ordering::Acquire) || cancel.load(Ordering::Acquire) {
            return None;
        }
        let index = self.cursor.fetch_add(1, Ordering::AcqRel);
        (index < jobs).then_some(index)
    }

    fn on_success(&self) {
        let mut state = self.limiter();
        state.consecutive_successes += 1;
        if state.consecutive_successes < RAISE_AFTER_SUCCESSES {
            return;
        }
        state.consecutive_successes = 0;
        let previous = state.allowed;
        state.allowed = (state.allowed + (state.allowed / 4).max(1)).min(state.max);
        drop(state);
        if previous < self.max() {
            self.wake_workers();
        }
    }

    fn on_backoff(&self) {
        let mut state = self.limiter();
        state.consecutive_successes = 0;
        state.allowed = (state.allowed / 2).max(1);
        drop(state);
        self.wake_workers();
    }
}

/// How one finished job's answer steers the limiter. Rate limiting and
/// `OpenRouter`'s in-flight credit reservation narrow the pass; the
/// transport-class errors stop it; a cancelled request stops it too (the
/// caller's flag is normally already set, but a provider may answer
/// `Cancelled` on its own). A plain quota 402 is per-conversation.
fn job_signal<T>(outcome: &Result<T, ProviderError>) -> JobSignal {
    let Err(error) = outcome else {
        return JobSignal::Ok;
    };
    match *error {
        ProviderError::RateLimited | ProviderError::CreditsInFlight => JobSignal::Backoff,
        ProviderError::Cancelled => JobSignal::Stop,
        error if is_transport_error(error) => JobSignal::Stop,
        // Any other failure is about this one request, not the provider:
        // the round trip completed and the next one may well succeed.
        _ => JobSignal::Ok,
    }
}

/// The transport-class errors that stop a pass dispatching new jobs.
/// Rate limiting is excluded because it narrows rather than stops the
/// pass. Nothing is ever resent either way.
fn is_stop_error(error: ProviderError) -> bool {
    is_transport_error(error) && !matches!(error, ProviderError::RateLimited)
}

enum JobOutcome<T> {
    Completed(Result<T, ProviderError>),
    Panicked,
    NotStarted,
}

struct InFlightGuard<'a> {
    progress: &'a ScanProgress,
    pass: &'a ParallelPass,
    worker: usize,
    processed: usize,
    sent: bool,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.progress
            .finish_request(self.worker, self.processed, self.sent);
        self.pass.wake_workers();
    }
}

/// Answers for one pass's jobs, keyed by job index.
type JobResults<T> = Vec<(usize, JobOutcome<T>)>;

/// Runs jobs across the pass's maximum number of scoped worker threads and returns
/// every completed job's answer sorted by job index, so the caller merges
/// them in exactly the order the sequential loop produced them.
///
/// Jobs that were never dispatched -- because the pass stopped or the
/// caller cancelled -- are simply absent.
fn run_jobs<T: Send>(
    processed_per_job: &[usize],
    pass: &ParallelPass,
    progress: &ScanProgress,
    job: &(dyn Fn(usize) -> Result<T, ProviderError> + Sync),
) -> JobResults<T> {
    let done: Mutex<JobResults<T>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        // A worker beyond the job count would only wake, find the cursor
        // exhausted, and exit, so a three-conversation scan never spawns a
        // hundred threads just because the ceiling allows them.
        for worker in 0..pass.max().min(processed_per_job.len()) {
            let done = &done;
            scope.spawn(move || {
                run_worker(worker, processed_per_job, pass, progress, job, done);
            });
        }
    });
    let mut done = done.into_inner().unwrap_or_else(PoisonError::into_inner);
    done.sort_by_key(|(index, _)| *index);
    done
}

/// One worker's whole life: park until it is inside the allowed
/// concurrency, take the next job, run it, and report what its answer
/// means for the limiter.
fn run_worker<T: Send>(
    worker: usize,
    processed_per_job: &[usize],
    pass: &ParallelPass,
    progress: &ScanProgress,
    job: &(dyn Fn(usize) -> Result<T, ProviderError> + Sync),
    done: &Mutex<JobResults<T>>,
) {
    while pass.wait_for_slot(worker, processed_per_job.len(), &progress.cancel) {
        let Some(index) = pass.next_job(processed_per_job.len(), &progress.cancel) else {
            return;
        };
        if pass.stop.load(Ordering::Acquire) || progress.cancel.load(Ordering::Acquire) {
            done.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((index, JobOutcome::NotStarted));
            return;
        }
        progress.start_request(worker);
        let guard = InFlightGuard {
            progress,
            pass,
            worker,
            processed: processed_per_job[index],
            sent: false,
        };
        // These loads are deliberately adjacent to the invocation: work
        // claimed just before a stop is reported as not started, never sent.
        let mut guard = guard;
        let outcome =
            if pass.stop.load(Ordering::Acquire) || progress.cancel.load(Ordering::Acquire) {
                JobOutcome::NotStarted
            } else {
                guard.sent = true;
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(index))) {
                    Ok(outcome) => JobOutcome::Completed(outcome),
                    Err(_) => JobOutcome::Panicked,
                }
            };
        if let JobOutcome::Completed(completed) = &outcome {
            match job_signal(completed) {
                JobSignal::Ok => pass.on_success(),
                JobSignal::Backoff => pass.on_backoff(),
                JobSignal::Stop => pass.stop(),
            }
        }
        drop(guard);
        done.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((index, outcome));
    }
}

/// The one-line diagnostic for a conversation the model could not
/// analyze. A rate-limited request says so plainly and says it was not
/// resent, because `network_policy.retries` forbids automatically
/// retrying any request that already carried content: the conversation is
/// simply unanalyzed for this scan.
fn failure_line(index: usize, conversation: &[&ReviewMessage], error: ProviderError) -> String {
    let head = format!(
        "Conversation {} ({} messages; subject: {})",
        index + 1,
        conversation.len(),
        subject_snippet(conversation),
    );
    match error {
        ProviderError::RateLimited => {
            format!("{head}: The provider rate-limited this request; it was not resent.")
        }
        ProviderError::CreditsInFlight => {
            format!("{head}: {error} This request was not resent.")
        }
        ProviderError::Timeout => format!(
            "{head}: {error} Request size: {} messages.",
            conversation.len()
        ),
        // Append OpenRouter's own stated reason when one was captured, so a
        // 402 is diagnosable from the failure line itself -- see
        // `provider::record_quota_detail` for the boundary this crosses.
        ProviderError::Quota => match openloops_inference::provider::last_quota_detail() {
            Some(detail) => format!("{head}: {error} OpenRouter's reason: {detail}"),
            None => format!("{head}: {error}"),
        },
        error => format!("{head}: {error}"),
    }
}

fn failure_reason(error: ProviderError) -> FailureReason {
    match error {
        ProviderError::Timeout => FailureReason::Timeout,
        // OpenRouter's in-flight credit reservation is a too-many-at-once
        // condition, retried the same way a rate limit is.
        ProviderError::RateLimited | ProviderError::CreditsInFlight => FailureReason::RateLimited,
        ProviderError::Network => FailureReason::Transport,
        ProviderError::Quota => FailureReason::Quota,
        error => FailureReason::Provider(error.to_string()),
    }
}

fn failure_detail(conversation: &[&ReviewMessage], reason: FailureReason) -> ConversationFailure {
    ConversationFailure {
        conversation: conversation
            .first()
            .map_or_else(String::new, |message| message.conversation.clone()),
        subject_short: subject_snippet(conversation),
        reason,
    }
}

fn empty_result(total: usize) -> ScanResult {
    ScanResult {
        analysis: LoopItems {
            items: vec![],
            rejected: 0,
            rejection_reasons: vec![],
            degraded: 0,
        },
        failures: vec![],
        failed_conversations_detail: vec![],
        analyzed: 0,
        total,
        cancelled: false,
        conversation_notes: vec![],
        conversation_quality: BTreeMap::new(),
        conversation_notes_by_id: BTreeMap::new(),
        conversation_rejection_reasons: BTreeMap::new(),
        suggested_updates: 0,
        event_closures: 0,
        primary_scan_transport_error: false,
        conversation_count: 0,
        analyzed_conversations: 0,
        failed_conversations: 0,
        not_started_conversations: 0,
        closure_pass_failure: None,
    }
}

/// Folds one conversation's answer into `result`, exactly as the
/// sequential loop did: `conversation` is the analyzed conversation and
/// `index` its 0-based position in the smallest-first ordering, so
/// notes, failure numbering, and item order do not depend on which worker
/// happened to run it.
fn merge_conversation(
    result: &mut ScanResult,
    index: usize,
    conversation: &[&ReviewMessage],
    outcome: JobOutcome<LoopItems>,
    not_started_messages: &mut usize,
) {
    match outcome {
        JobOutcome::Completed(Ok(mut analysis)) => {
            let conversation_id = conversation
                .first()
                .map_or_else(String::new, |message| message.conversation.clone());
            result.analyzed += conversation.len();
            result.analyzed_conversations += 1;
            correct_recap_attribution(&mut analysis, conversation, None);
            tag_call_summaries(&mut analysis, conversation, None);
            if let Some(note) = conversation_note(index, conversation, &analysis) {
                result.conversation_notes.push(note.clone());
                result
                    .conversation_notes_by_id
                    .entry(conversation_id.clone())
                    .or_default()
                    .push(note);
            }
            result.conversation_quality.insert(
                conversation_id.clone(),
                (analysis.rejected, analysis.degraded),
            );
            result
                .conversation_rejection_reasons
                .insert(conversation_id, analysis.rejection_reasons.clone());
            result.analysis.items.extend(analysis.items);
            result.analysis.rejected += analysis.rejected;
            result.analysis.degraded += analysis.degraded;
            result
                .analysis
                .rejection_reasons
                .extend(analysis.rejection_reasons);
        }
        JobOutcome::Completed(Err(ProviderError::Cancelled)) => result.cancelled = true,
        JobOutcome::Completed(Err(error)) => {
            result
                .failures
                .push(failure_line(index, conversation, error));
            result
                .failed_conversations_detail
                .push(failure_detail(conversation, failure_reason(error)));
            result.failed_conversations += 1;
            if is_stop_error(error) {
                result.primary_scan_transport_error = true;
            }
        }
        JobOutcome::Panicked => {
            result.failures.push(format!(
                "Conversation {}: the analysis failed unexpectedly and was skipped.",
                index + 1
            ));
            result
                .failed_conversations_detail
                .push(failure_detail(conversation, FailureReason::Panicked));
            result.failed_conversations += 1;
        }
        // Queued but never claimed -- either the cursor never reached it
        // (the common case: the pass stopped and a worker simply had no
        // more work behind it) or a worker claimed it just as the pass
        // stopped (`run_worker`'s narrow post-claim check). Both are the
        // same fact for the user, so they are not reported as individual
        // failure lines here; `scan_conversations` folds every one of them
        // into a single aggregate line once the pass is done.
        JobOutcome::NotStarted => {
            result.failures.push(format!(
                "Conversation {} ({} messages; subject: {}): the conversation was not analyzed because the scan stopped after a provider error.",
                index + 1,
                conversation.len(),
                subject_snippet(conversation)
            ));
            result
                .failed_conversations_detail
                .push(failure_detail(conversation, FailureReason::NotStarted));
            result.not_started_conversations += 1;
            *not_started_messages += conversation.len();
        }
    }
}

/// Analyzes every conversation in `messages`, up to the pass's ceiling at
/// once. Conversations are independent, so running several concurrently
/// changes only when each answer arrives; the answers are merged in the
/// smallest-first job order [`conversations_by_size`] produced, so the
/// result is identical to analyzing them one at a time.
#[cfg(test)]
fn scan_conversations(
    messages: &[ReviewMessage],
    progress: &ScanProgress,
    pass: &ParallelPass,
    analyze: &(dyn Fn(&[ConversationMessage]) -> Result<LoopItems, ProviderError> + Sync),
) -> ScanResult {
    scan_conversations_filtered(messages, progress, pass, None, analyze)
}

fn scan_conversations_filtered(
    messages: &[ReviewMessage],
    progress: &ScanProgress,
    pass: &ParallelPass,
    conversation_filter: Option<&BTreeSet<String>>,
    analyze: &(dyn Fn(&[ConversationMessage]) -> Result<LoopItems, ProviderError> + Sync),
) -> ScanResult {
    let ordered: Vec<Vec<&ReviewMessage>> = conversations_by_size(messages, conversation_filter)
        .into_iter()
        .map(|mut conversation| {
            conversation.sort_by_key(|m| m.input.timestamp);
            conversation
        })
        .collect();
    let mut result = empty_result(ordered.iter().map(Vec::len).sum());
    result.conversation_count = ordered.len();
    progress
        .conversation_total
        .store(ordered.len(), Ordering::Relaxed);
    let processed_per_job: Vec<usize> = ordered.iter().map(Vec::len).collect();
    let mut outcomes = run_jobs(&processed_per_job, pass, progress, &|index| {
        let inputs: Vec<ConversationMessage> = ordered[index]
            .iter()
            .map(|message| {
                let mut input = message.input.clone();
                input
                    .user_display_name
                    .clone_from(&message.own_display_name);
                input.user_given_name.clone_from(&message.own_given_name);
                input
            })
            .collect();
        analyze(&inputs)
    });
    // A job the pass stopped before any worker ever reached it has no
    // entry in `outcomes` at all (`run_worker`/`next_job` never dispatch
    // it), which is exactly the bug this fixes: those conversations must
    // still be accounted for as not started, not silently dropped.
    if outcomes.len() < ordered.len() {
        let mut dispatched = vec![false; ordered.len()];
        for (index, _) in &outcomes {
            dispatched[*index] = true;
        }
        for (index, was_dispatched) in dispatched.into_iter().enumerate() {
            if !was_dispatched {
                outcomes.push((index, JobOutcome::NotStarted));
            }
        }
        outcomes.sort_by_key(|(index, _)| *index);
    }
    let mut not_started_messages = 0usize;
    for (index, outcome) in outcomes {
        merge_conversation(
            &mut result,
            index,
            &ordered[index],
            outcome,
            &mut not_started_messages,
        );
    }
    if progress.cancel.load(Ordering::Relaxed) {
        result.cancelled = true;
    }
    // A stopped-by-you scan reports its not-started conversations only in
    // the summary (see `set_scan`), not as a failure line here -- stopping
    // on request is not a failure. A provider-stopped scan gets one
    // aggregate line instead of one per conversation, so 95 unanalyzed
    // conversations behind a single quota error read as one line, not 95.
    let _ = not_started_messages;
    // Idle once this pass ends, same as `scan_closures`.
    progress.reset_pass();
    result
}

/// The instant a [`DeadlineView`] says has already passed, or `None` when it
/// is not a past-due variant (or its boundary could not be resolved). Shared
/// by [`close_passed_events`]'s `event_time` path, which needs the same
/// boundary a deadline card would age against, not just the past/future bit.
fn past_due_boundary(view: &DeadlineView) -> Option<i64> {
    match *view {
        DeadlineView::PastDue { boundary, .. }
        | DeadlineView::DueDate {
            boundary,
            past: true,
            ..
        }
        | DeadlineView::DueBusinessDay {
            boundary,
            past: true,
            ..
        }
        | DeadlineView::DueRange {
            boundary,
            past: true,
            ..
        } => Some(boundary),
        _ => None,
    }
}

/// The expectation's own explicit naming of an event: its `event` anchor
/// when present, otherwise its deadline quote when the deadline phrase
/// itself classifies as [`DeadlineView::EventTied`]. `None` means the item
/// never named an event at all -- per the review fix, that gates BOTH the
/// index match and the stated-`event_time` path below: a plain overdue
/// deadline must never be closed as "event passed" just because the model
/// separately filled in an `event_time` anchor.
fn named_event_phrase(
    item: &LoopItem,
    message_timestamp: i64,
    now: i64,
    offset: i32,
) -> Option<&str> {
    if let Some(event) = &item.event {
        return Some(event.quote.as_str());
    }
    let deadline = item.deadline.as_ref()?;
    matches!(
        classify_with_hint(
            &deadline.quote,
            message_timestamp,
            now,
            offset,
            item.deadline_kind_hint,
        ),
        DeadlineView::EventTied
    )
    .then_some(deadline.quote.as_str())
}

/// The text-based fallback used only when the model named no event at all
/// (`named_event_phrase` returned `None`): matches the item's `action`,
/// then its `evidence.quote`, against the index with
/// [`match_event_by_text`]'s proper-name-strength bar.
fn text_matched_event<'a>(
    item: &LoopItem,
    source: &ReviewMessage,
    index: &'a [EventRef],
    rules: Option<&RuleDecisions>,
) -> Option<&'a EventRef> {
    match_event_by_text_with_rules(&item.action, source.input.timestamp, index, rules).or_else(
        || {
            match_event_by_text_with_rules(
                &item.evidence.quote,
                source.input.timestamp,
                index,
                rules,
            )
        },
    )
}

/// Whether an otherwise unnamed request uses language that connects it to a
/// gathering in its own conversation. Event nouns are matched as whole
/// alphanumeric tokens; the timing phrases are intentionally narrow.
fn has_scoped_event_language(item: &LoopItem) -> bool {
    has_scoped_event_language_with_rules(item, None)
}

fn scoped_event_state(item: &LoopItem) -> serde_json::Value {
    serde_json::json!({"request_text": format!("{} {}", item.action, item.evidence.quote)})
}

fn has_scoped_event_language_with_rules(item: &LoopItem, rules: Option<&RuleDecisions>) -> bool {
    const EVENT_NOUN_FOLLOWERS: &[&str] = &[
        "notes",
        "minutes",
        "recording",
        "summary",
        "agenda",
        "invite",
        "link",
    ];
    let fast = [&item.action, &item.evidence.quote].iter().any(|text| {
        let lower = text.to_lowercase();
        let words: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect();
        words.iter().enumerate().any(|(index, word)| {
            ["arrive", "arriving", "attend", "attending", "bring"].contains(word)
                || (EVENT_GENERIC_NOUNS.contains(word)
                    && words
                        .get(index + 1)
                        .is_none_or(|next| !EVENT_NOUN_FOLLOWERS.contains(next)))
        })
    });
    fast || rules.is_some_and(|cache| cache.noul(RULE_SCOPED_EVENT, &scoped_event_state(item)))
}

/// Whether `item`'s own deadline phrase, classified against `message_timestamp`,
/// reads as [`DeadlineView::EventTied`] (e.g. "before the meeting") -- the
/// same test [`named_event_phrase`] uses to accept a deadline-only naming of
/// an event. Reused by [`scoped_event`] to let its own-message branch accept
/// a request that names no event and uses no [`has_scoped_event_language`]
/// wording, but whose deadline itself says it is tied to a gathering.
fn event_tied_deadline(item: &LoopItem, message_timestamp: i64, now: i64) -> bool {
    let Some(deadline) = item.deadline.as_ref() else {
        return false;
    };
    let offset = local_offset_seconds(message_timestamp, 0);
    matches!(
        classify_with_hint(
            &deadline.quote,
            message_timestamp,
            now,
            offset,
            item.deadline_kind_hint,
        ),
        DeadlineView::EventTied
    )
}

/// Finds deterministic event evidence scoped to the expectation's source:
/// first an event learned from the evidence message itself -- but only for a
/// request that [`has_scoped_event_language`], carries an
/// [`event_tied_deadline`], or satisfies [`rule_3_applies`]. The latter means
/// the model named both the event and its distinct time; when the time points
/// to the evidence message, it also lets that message's body-prose event
/// qualify.
/// Without one of those signals, merely appearing in an event-bearing message
/// is not enough (e.g. an unrelated action item in an invite). Otherwise this
/// finds the sole structured or subject-prose event in the same conversation
/// when the request uses event-shaped language. Ambiguity between two
/// conversation events leaves the item open.
fn scoped_event<'a>(
    item: &LoopItem,
    source: &ReviewMessage,
    messages: &[ReviewMessage],
    index: &'a [EventRef],
    now: i64,
    rules: Option<&RuleDecisions>,
) -> Option<&'a EventRef> {
    let normally_qualifying = |event: &&EventRef| {
        matches!(
            event.source,
            EventSource::Meeting | EventSource::Subject | EventSource::SubjectProse
        )
    };
    index
        .iter()
        .find(|event| {
            event.message_handle == item.evidence.message
                && (normally_qualifying(event)
                    || (event.source == EventSource::Prose
                        && item
                            .event_time
                            .as_ref()
                            .is_some_and(|anchor| anchor.message == item.evidence.message)
                        && rule_3_applies(item, messages, now, event.end)))
        })
        .filter(|event| {
            has_scoped_event_language_with_rules(item, rules)
                || event_tied_deadline(item, source.input.timestamp, now)
                || rule_3_applies(item, messages, now, event.end)
        })
        .or_else(|| {
            let mut events = index
                .iter()
                .filter(normally_qualifying)
                .filter(|event| event.conversation == source.conversation);
            has_scoped_event_language_with_rules(item, rules)
                .then(|| events.next().filter(|_| events.next().is_none()))?
        })
}

fn deadline_boundary(item: &LoopItem, messages: &[ReviewMessage], now: i64) -> Option<i64> {
    let deadline = item.deadline.as_ref()?;
    let message = messages
        .iter()
        .find(|message| message.input.handle == deadline.message)?;
    let offset = local_offset_seconds(message.input.timestamp, 0);
    match classify_with_hint(
        &deadline.quote,
        message.input.timestamp,
        now,
        offset,
        item.deadline_kind_hint,
    ) {
        DeadlineView::PastDue { boundary, .. }
        | DeadlineView::Due { boundary, .. }
        | DeadlineView::DueDate { boundary, .. }
        | DeadlineView::DueBusinessDay { boundary, .. }
        | DeadlineView::DueRange { boundary, .. } => Some(boundary),
        DeadlineView::EventTied | DeadlineView::Soft | DeadlineView::Unknown => None,
    }
}

fn local_day(timestamp: i64, offset_seconds: i32) -> i64 {
    timestamp
        .saturating_add(i64::from(offset_seconds))
        .div_euclid(86_400)
}

/// Whether the item's own deadline is another rendering of `event_time` or
/// resolves to the same instant/local day as `event_end`. In either case the
/// item is overdue work, not work made irrelevant by a passed event.
fn deadline_is_event_time(
    item: &LoopItem,
    messages: &[ReviewMessage],
    now: i64,
    event_end: i64,
) -> bool {
    let (Some(deadline), Some(event_time)) = (&item.deadline, &item.event_time) else {
        return false;
    };
    if deadline
        .quote
        .trim()
        .eq_ignore_ascii_case(event_time.quote.trim())
    {
        return true;
    }
    let Some(message) = messages
        .iter()
        .find(|message| message.input.handle == deadline.message)
    else {
        return false;
    };
    let offset = local_offset_seconds(message.input.timestamp, 0);
    match classify_with_hint(
        &deadline.quote,
        message.input.timestamp,
        now,
        offset,
        item.deadline_kind_hint,
    ) {
        DeadlineView::PastDue {
            boundary,
            offset_seconds,
        }
        | DeadlineView::Due {
            boundary,
            offset_seconds,
        } => {
            boundary == event_end
                || local_day(boundary, offset_seconds) == local_day(event_end, offset_seconds)
        }
        DeadlineView::DueDate { day, boundary, .. }
        | DeadlineView::DueBusinessDay { day, boundary, .. } => {
            boundary == event_end || day == local_day(event_end, offset)
        }
        DeadlineView::DueRange {
            end_day, boundary, ..
        } => boundary == event_end || end_day == local_day(event_end, offset),
        DeadlineView::EventTied | DeadlineView::Soft | DeadlineView::Unknown => false,
    }
}

/// Rule 3 requires both a named-event anchor and a distinct event-time anchor.
/// An own deadline at that same time/day remains overdue instead of closing.
fn rule_3_applies(item: &LoopItem, messages: &[ReviewMessage], now: i64, event_end: i64) -> bool {
    item.event.is_some()
        && item.event_time.is_some()
        && !deadline_is_event_time(item, messages, now, event_end)
}

/// Closes `item` against `event` when `event` has already ended -- but only
/// when `event` had ALREADY ended, relative to `event`, at
/// `source_timestamp` (the evidence message's own send time) as well as
/// `now`: a message written after its event (a summary, a recap, a
/// thank-you note) can never be closed by that event, since the event is the
/// source of the request, not its deadline. Returns whether it closed.
fn close_from_index(
    item: &mut LoopItem,
    event: &EventRef,
    messages: &[ReviewMessage],
    now: i64,
    source_timestamp: i64,
) -> bool {
    if event.end >= now
        || event.end <= source_timestamp
        || deadline_boundary(item, messages, now).is_some_and(|d| d > event.end)
        || (item.event.is_some()
            && item.event_time.is_some()
            && !rule_3_applies(item, messages, now, event.end))
    {
        return false;
    }
    item.event_passed = Some(EventPassed {
        name: event.name.clone(),
        end: event.end,
        message_handle: event.message_handle.clone(),
        from_subject: matches!(
            event.source,
            EventSource::Subject | EventSource::SubjectProse
        ),
    });
    true
}

/// Closes `item` from its own `event_time` anchor, classified directly with
/// [`classify`] against the message that stated it -- the only way to close
/// a loop whose event date is stated only in an email body rather than a
/// meeting invite or calendar subject, so it has no index entry at all.
/// The closure name always comes from the model's `event` anchor, never from
/// the bare time phrase.
/// Applies the same temporal guard as [`close_from_index`]: an event that had
/// already ended by `source_timestamp` (the evidence message's own send
/// time) can never close the item, even if it also reads as past relative to
/// `now`. Returns whether it closed.
fn close_from_stated_time(
    item: &mut LoopItem,
    messages: &[ReviewMessage],
    now: i64,
    source_timestamp: i64,
) -> bool {
    let Some(name) = item.event.as_ref().map(|event| event.quote.clone()) else {
        return false;
    };
    let Some(event_time) = &item.event_time else {
        return false;
    };
    let Some(time_message) = messages
        .iter()
        .find(|m| m.input.handle == event_time.message)
    else {
        return false;
    };
    let offset = local_offset_seconds(time_message.input.timestamp, 0);
    let view = classify(&event_time.quote, time_message.input.timestamp, now, offset);
    let Some(end) = past_due_boundary(&view) else {
        return false;
    };
    if end <= source_timestamp || !rule_3_applies(item, messages, now, end) {
        return false;
    }
    if deadline_boundary(item, messages, now).is_some_and(|deadline| deadline > end) {
        return false;
    }
    item.event_passed = Some(EventPassed {
        name,
        end,
        message_handle: event_time.message.clone(),
        // Governed `event_time` anchors in this path come from body prose.
        from_subject: false,
    });
    true
}

/// Records the normalized end time supplied by a matched source event when
/// the governed claim named that event but carried no separate time anchor.
/// The normalized value follows the same local-date-time grammar used by the
/// governed temporal contract and preserves the matched event's source handle.
fn set_event_time_from_source(item: &mut LoopItem, event: &EventRef, messages: &[ReviewMessage]) {
    if item.event.is_none() || item.event_time.is_some() {
        return;
    }
    let Some(message) = messages
        .iter()
        .find(|message| message.input.handle == event.message_handle)
    else {
        return;
    };
    let offset = local_offset_seconds(event.end, 0);
    let Some(offset) = chrono::FixedOffset::east_opt(offset) else {
        return;
    };
    let Some(end) = chrono::Utc.timestamp_opt(event.end, 0).single() else {
        return;
    };
    let event_anchor = item.event.as_ref().expect("checked above");
    item.event_time = Some(Anchor {
        message: message.input.handle.clone(),
        block: event_anchor.block,
        quote: end
            .with_timezone(&offset)
            .format("%Y-%m-%dT%H:%M")
            .to_string(),
        context: event.name.clone(),
    });
}

/// Closes any open, unresolved expectation whose event has already ended,
/// via four independent sources of evidence, tried in order for each item:
///
/// - the learned event index, matched against the expectation's own named
///   event ([`named_event_phrase`]) with [`match_event`] -- when the index
///   has a match, it alone decides this item, whether or not it closes;
/// - failing that, a model-supplied `event_time` phrase -- a verbatim
///   date/time anchor stating when the named `event` anchor occurs, found
///   anywhere in the conversation -- classified directly
///   ([`close_from_stated_time`]);
/// - when no named index event matched, an event learned from the evidence
///   message itself, or the sole qualifying event in its conversation when
///   the request uses event language ([`scoped_event`]); `event` plus
///   `event_time` anchors make the own-message branch event-bound
///   and can admit body prose when the time points to that same message;
/// - for an item that named no event at all, the index again, but matched
///   against the item's own `action`/`evidence.quote` text instead
///   ([`text_matched_event`]), at a much stronger bar so an unrelated
///   generic phrase never matches by accident.
///
/// Two rules apply across every source above: an event can only close a
/// request if the event had not yet ended when the evidence message was
/// sent ([`close_from_index`], [`close_from_stated_time`]) -- a summary,
/// recap, or thank-you note written after its own event can never be closed
/// by that event -- and [`scoped_event`]'s own-message branch only credits
/// an event learned from the evidence message itself to a request that
/// [`has_scoped_event_language`], carries an [`event_tied_deadline`], or has
/// distinct model-supplied `event` and `event_time` anchors. An `event_time`
/// that is also the item's own deadline never invokes rule 3.
///
/// Always pushes one content-free coverage note (even when every count is
/// zero, so the chain from "events learned" to "expectations closed" stays
/// visible), unless `result.cancelled` -- a cancelled scan's counts would be
/// partial and therefore misleading.
pub fn close_passed_events(result: &mut ScanResult, messages: &[ReviewMessage], now: i64) {
    close_passed_events_with_rules(result, messages, now, None);
}

fn close_passed_events_with_rules(
    result: &mut ScanResult,
    messages: &[ReviewMessage],
    now: i64,
    rules: Option<&RuleDecisions>,
) {
    let index = build_event_index_with_rules(messages, rules);
    let mut named = 0usize;
    let mut timed = 0usize;
    let mut matched = 0usize;
    let mut matched_by_text = 0usize;
    let mut scoped = 0usize;
    let mut closed_from_index = 0usize;
    let mut closed_from_stated_time = 0usize;
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
        timed += usize::from(item.event_time.is_some());

        let named_phrase =
            named_event_phrase(item, source.input.timestamp, now, offset).map(str::to_string);
        let named_phrase_present = named_phrase.is_some();
        if let Some(phrase) = named_phrase {
            named += 1;
            let event_match =
                match_event_with_rules(&phrase, source.input.timestamp, &index, rules);
            matched += usize::from(event_match.is_some());
            if let Some(event) = event_match {
                set_event_time_from_source(item, event, messages);
                if close_from_index(item, event, messages, now, source.input.timestamp) {
                    closed_from_index += 1;
                    result.event_closures += 1;
                }
                continue;
            }
            if close_from_stated_time(item, messages, now, source.input.timestamp) {
                closed_from_stated_time += 1;
                result.event_closures += 1;
                continue;
            }
        }

        if let Some(event) = scoped_event(item, source, messages, &index, now, rules) {
            scoped += 1;
            set_event_time_from_source(item, event, messages);
            if close_from_index(item, event, messages, now, source.input.timestamp) {
                closed_from_index += 1;
                result.event_closures += 1;
            }
            continue;
        }

        if !named_phrase_present
            && let Some(event) = text_matched_event(item, source, &index, rules)
        {
            matched_by_text += 1;
            if close_from_index(item, event, messages, now, source.input.timestamp) {
                closed_from_index += 1;
                result.event_closures += 1;
            }
        }
    }
    if result.cancelled {
        return;
    }
    let meetings = index
        .iter()
        .filter(|event| event.source == EventSource::Meeting)
        .count();
    let subjects = index
        .iter()
        .filter(|event| event.source == EventSource::Subject)
        .count();
    let subject_prose = index
        .iter()
        .filter(|event| event.source == EventSource::SubjectProse)
        .count();
    let prose = index
        .iter()
        .filter(|event| event.source == EventSource::Prose)
        .count();
    let event_word = if index.len() == 1 { "event" } else { "events" };
    result.conversation_notes.push(format!(
        "Event index: {} {event_word} learned ({meetings} meetings, {subjects} calendar subjects, {subject_prose} subject prose, {prose} body prose); {scoped} tied to their own message's event, {named} named an event, {timed} carried a time, {matched} matched by name, {matched_by_text} matched by request text, {closed_from_index} closed from the index, {closed_from_stated_time} closed from a stated time.",
        index.len(),
    ));
}

/// Hard cap on how many per-conversation closure-pass provider calls one
/// scan makes. A large mailbox could otherwise turn into dozens of extra
/// model calls in a single scan.
const MAX_CLOSURE_CONVERSATIONS: usize = 40;

/// The most open loops one closure-pass request may name.
const MAX_LOOP_HANDLES: usize = 8;

/// The message cap of one governed request (`analysis::projection`'s own
/// bound), which the closure pass shares between the conversation it checks
/// and the evidence messages of the loops it offers.
const MAX_REQUEST_MESSAGES: usize = 40;

/// Decision-model requests inspect at most the first eight body paragraphs
/// of one later message.
const MAX_CLOSURE_PARAGRAPHS: usize = 8;

/// One open, you-owed loop offered to the closure pass: its scan-local
/// opaque handle, the index of its item in `ScanResult::analysis.items`,
/// and the message its evidence anchors to.
///
/// The handle reads `loop-{n}-{message}-b{block}`. The message handle and
/// block ordinal are scan-local ids, never content. They tell the model
/// which supplied message states the obligation the handle stands for
/// (the closure request includes that message when it is not already part
/// of the conversation being checked).
struct OfferedLoop<'a> {
    handle: String,
    item: usize,
    source: &'a ReviewMessage,
}

/// The loops the closure pass may attach updates to: open (no resolution,
/// event not passed), a `request` or `promise` the signed-in user owes.
/// `attributed` items are excluded because they do not name the user as the
/// one who owes the action. Handles are minted in item order, so a scan's
/// handles are deterministic.
fn offered_loops<'a>(items: &[LoopItem], messages: &'a [ReviewMessage]) -> Vec<OfferedLoop<'a>> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.resolution.is_none()
                && item.event_passed.is_none()
                && matches!(item.kind.as_str(), "request" | "promise")
                && item.owner == Owner::You
        })
        .filter_map(|(index, item)| {
            let source = messages
                .iter()
                .find(|m| m.input.handle == item.evidence.message)?;
            Some((index, item, source))
        })
        .enumerate()
        .map(|(n, (index, item, source))| OfferedLoop {
            handle: format!(
                "loop-{}-{}-b{}",
                n + 1,
                source.input.handle,
                item.evidence.block
            ),
            item: index,
            source,
        })
        .collect()
}

/// `(account, conversation)` key of one conversation.
type ConversationKey<'a> = (&'a str, &'a str);

/// For every conversation with messages that could bear on an offered loop,
/// the indexes (into `loops`) of the loops plausibly reachable from it.
/// Reachability reuses the closure scoping rules: a later message the user
/// sent in the loop's own conversation, or a later message the user sent to
/// the loop's waiting party in another conversation of the same account. A
/// shared-mailbox or list waiting party (see [`is_shared_mailbox_address`])
/// suppresses only the cross-conversation route.
fn loops_by_conversation<'a>(
    loops: &[OfferedLoop<'a>],
    items: &[LoopItem],
    messages: &'a [ReviewMessage],
) -> BTreeMap<ConversationKey<'a>, Vec<usize>> {
    let (groups_per_account, address_group_counts) = conversation_group_address_counts(messages);
    let mut reach: BTreeMap<ConversationKey<'a>, Vec<usize>> = BTreeMap::new();
    for (index, offered) in loops.iter().enumerate() {
        let item = &items[offered.item];
        let source = offered.source;
        let mut targets: BTreeSet<&'a str> = BTreeSet::new();
        if !same_thread_closure_candidates(
            messages,
            &source.account,
            &source.conversation,
            source.input.timestamp,
        )
        .is_empty()
        {
            targets.insert(source.conversation.as_str());
        }
        if let Some(address) = waiting_party_address(&item.waiting_party)
            && !is_shared_mailbox_address(
                &source.account,
                &address,
                &groups_per_account,
                &address_group_counts,
            )
        {
            targets.extend(
                closure_candidates(
                    item,
                    messages,
                    &source.account,
                    &source.conversation,
                    source.input.timestamp,
                )
                .into_iter()
                .map(|m| m.conversation.as_str()),
            );
        }
        for conversation in targets {
            reach
                .entry((source.account.as_str(), conversation))
                .or_default()
                .push(index);
        }
    }
    reach
}

/// One closure-pass request: a conversation (chronological) and the
/// offered loops (indexes into the offered list, at most
/// [`MAX_LOOP_HANDLES`]) it may close or change.
struct ClosureJob<'a> {
    conversation: Vec<&'a ReviewMessage>,
    loops: Vec<usize>,
}

/// Turns the reach table into at most [`MAX_CLOSURE_CONVERSATIONS`] jobs,
/// busiest conversations first, and reports whether the cap dropped any.
fn closure_jobs<'a>(
    reach: BTreeMap<ConversationKey<'a>, Vec<usize>>,
    messages: &'a [ReviewMessage],
) -> (Vec<ClosureJob<'a>>, bool) {
    let mut ranked: Vec<(ConversationKey<'a>, Vec<usize>)> = reach.into_iter().collect();
    ranked.sort_by_key(|(_, loops)| std::cmp::Reverse(loops.len()));
    let capped = ranked.len() > MAX_CLOSURE_CONVERSATIONS;
    ranked.truncate(MAX_CLOSURE_CONVERSATIONS);
    let jobs = ranked
        .into_iter()
        .map(|((account, conversation), mut loops)| {
            loops.truncate(MAX_LOOP_HANDLES);
            let mut conversation: Vec<&ReviewMessage> = messages
                .iter()
                .filter(|m| m.account == account && m.conversation == conversation)
                .collect();
            conversation.sort_by_key(|m| m.input.timestamp);
            ClosureJob {
                conversation,
                loops,
            }
        })
        .collect();
    (jobs, capped)
}

/// Sends one closure-pass request: the job's conversation plus the evidence
/// message of every offered loop that lives elsewhere, with the job's loop
/// handles as `loop_candidate_handles`.
fn closure_request(
    client: &dyn ModelClient,
    job: &ClosureJob<'_>,
    offered: &[OfferedLoop<'_>],
    cancel: &AtomicBool,
) -> Result<ClaimAnalysis, ProviderError> {
    let loops: Vec<&OfferedLoop<'_>> = job.loops.iter().map(|&index| &offered[index]).collect();
    let mut extras: Vec<&ReviewMessage> = Vec::new();
    for offered_loop in &loops {
        let source = offered_loop.source;
        let present = job
            .conversation
            .iter()
            .chain(&extras)
            .any(|m| m.input.handle == source.input.handle);
        if !present {
            extras.push(source);
        }
    }
    let keep = MAX_REQUEST_MESSAGES.saturating_sub(extras.len());
    let skip = job.conversation.len().saturating_sub(keep);
    let mut inputs: Vec<ConversationMessage> = job.conversation[skip..]
        .iter()
        .chain(&extras)
        .map(|m| m.input.clone())
        .collect();
    inputs.sort_by_key(|m| m.timestamp);
    let handles: Vec<&str> = loops.iter().map(|l| l.handle.as_str()).collect();
    governed_call(client, &inputs, &handles, Some(cancel))
}

/// Builds the suggested update one accepted claim proposes for `target`, or
/// `None` when the claim is not a closure/deadline/modification claim, has
/// no usable evidence, or (for a deadline change) carries no new time.
/// The evidence is the claim's first body or subject block from a message
/// later than the one that stated the obligation; the loop is never
/// changed here.
fn suggested_update_for(
    accepted: &AcceptedClaim,
    target: &OfferedLoop<'_>,
    messages: &[ReviewMessage],
) -> Option<SuggestedUpdate> {
    let claim = &accepted.claim;
    let kind = match claim.claim_type {
        ClaimType::PossibleClosure => SuggestedUpdateKind::Closure,
        ClaimType::DeadlineChange => SuggestedUpdateKind::DeadlineChange,
        ClaimType::Modification => SuggestedUpdateKind::Modification,
        _ => return None,
    };
    let temporal_value = match (&claim.temporal, kind) {
        (Nullable::Value(temporal), SuggestedUpdateKind::DeadlineChange) => {
            Some(temporal.value.clone())
        }
        (_, SuggestedUpdateKind::DeadlineChange) => return None,
        _ => None,
    };
    let evidence = accepted.evidence.iter().find(|evidence| {
        matches!(
            evidence.component,
            EvidenceComponent::BodyBlock | EvidenceComponent::Subject
        ) && messages
            .iter()
            .find(|m| m.input.handle == evidence.source_handle)
            .is_some_and(|m| m.input.timestamp > target.source.input.timestamp)
    })?;
    Some(SuggestedUpdate {
        kind,
        evidence_text: evidence.text.clone(),
        source_message: evidence.source_handle.clone(),
        source_block: usize::from(evidence.block_ordinal),
        temporal_value,
        confidence_micros: claim.confidence_micros,
    })
}

/// Whether `candidate` should replace `current` as a loop's one suggested
/// update: higher confidence wins, and a closure wins a tie.
fn outranks(candidate: &SuggestedUpdate, current: &SuggestedUpdate) -> bool {
    candidate.confidence_micros > current.confidence_micros
        || (candidate.confidence_micros == current.confidence_micros
            && candidate.kind == SuggestedUpdateKind::Closure
            && current.kind != SuggestedUpdateKind::Closure)
}

/// Collects, per item index, the best suggested update among one job's
/// accepted claims. A claim counts only when it is a closure, deadline
/// change or modification whose `related_loop_handles` are non-empty and
/// all among the handles this job offered; every other claim type is
/// ignored, so the closure pass never creates a loop.
fn collect_updates(
    analysis: &ClaimAnalysis,
    job: &ClosureJob<'_>,
    offered: &[OfferedLoop<'_>],
    messages: &[ReviewMessage],
    best: &mut BTreeMap<usize, SuggestedUpdate>,
) {
    for accepted in &analysis.accepted {
        let related = &accepted.claim.related_loop_handles;
        if related.is_empty() {
            continue;
        }
        let targets: Vec<&OfferedLoop<'_>> = related
            .iter()
            .filter_map(|handle| {
                job.loops
                    .iter()
                    .map(|&index| &offered[index])
                    .find(|l| &l.handle == handle)
            })
            .collect();
        if targets.len() != related.len() {
            continue;
        }
        for target in targets {
            let Some(update) = suggested_update_for(accepted, target, messages) else {
                continue;
            };
            let replace = best
                .get(&target.item)
                .is_none_or(|current| outranks(&update, current));
            if replace {
                best.insert(target.item, update);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DecisionOutcome {
    Fulfilled,
    Withdrawn,
    DeadlineChanged,
    Modified,
}

impl DecisionOutcome {
    const ALL: [Self; 4] = [
        Self::Fulfilled,
        Self::Withdrawn,
        Self::DeadlineChanged,
        Self::Modified,
    ];

    const fn registry_id(self) -> &'static str {
        match self {
            Self::Fulfilled => "closure.fulfilled",
            Self::Withdrawn => "closure.withdrawn",
            Self::DeadlineChanged => "closure.deadline_changed",
            Self::Modified => "closure.modified",
        }
    }

    const fn suffix(self) -> &'static str {
        match self {
            Self::Fulfilled => "fulfilled",
            Self::Withdrawn => "withdrawn",
            Self::DeadlineChanged => "deadline_changed",
            Self::Modified => "modified",
        }
    }

    const fn kind(self) -> SuggestedUpdateKind {
        match self {
            Self::Fulfilled | Self::Withdrawn => SuggestedUpdateKind::Closure,
            Self::DeadlineChanged => SuggestedUpdateKind::DeadlineChange,
            Self::Modified => SuggestedUpdateKind::Modification,
        }
    }
}

struct DecisionPair<'a> {
    offered: usize,
    source_timestamp: i64,
    later: &'a ReviewMessage,
}

struct DecisionParagraphJob {
    pair: usize,
    ordinal: usize,
    text: String,
}

fn decision_pairs<'a>(
    reach: &BTreeMap<ConversationKey<'a>, Vec<usize>>,
    offered: &[OfferedLoop<'a>],
    messages: &'a [ReviewMessage],
) -> Vec<DecisionPair<'a>> {
    let mut pairs = Vec::new();
    for (&(account, conversation), loops) in reach {
        for &offered_index in loops {
            let source_timestamp = offered[offered_index].source.input.timestamp;
            pairs.extend(
                messages
                    .iter()
                    .filter(|message| {
                        message.account == account
                            && message.conversation == conversation
                            && message.input.timestamp > source_timestamp
                    })
                    .map(|later| DecisionPair {
                        offered: offered_index,
                        source_timestamp,
                        later,
                    }),
            );
        }
    }
    pairs.sort_by_key(|pair| {
        (
            pair.offered,
            pair.later.input.timestamp,
            pair.later.input.handle.as_str(),
        )
    });
    pairs
}

fn decision_paragraph_jobs(pairs: &[DecisionPair<'_>]) -> Vec<DecisionParagraphJob> {
    pairs
        .iter()
        .enumerate()
        .flat_map(|(pair, value)| {
            value
                .later
                .input
                .message
                .body_blocks
                .iter()
                .take(MAX_CLOSURE_PARAGRAPHS)
                .enumerate()
                .map(move |(ordinal, block)| DecisionParagraphJob {
                    pair,
                    ordinal,
                    text: block.as_string(),
                })
        })
        .collect()
}

fn closure_questions(ordinal: usize) -> Result<Questions, ProviderError> {
    let mut request_ids: Vec<String> = DecisionOutcome::ALL
        .iter()
        .map(|outcome| format!("p{ordinal}.{}", outcome.suffix()))
        .collect();
    request_ids.push(format!("p{ordinal}.outcome"));
    let mut mapped: Vec<(&str, &str)> = DecisionOutcome::ALL
        .iter()
        .zip(&request_ids)
        .map(|(outcome, request)| (outcome.registry_id(), request.as_str()))
        .collect();
    mapped.push(("closure.outcome", request_ids[4].as_str()));
    Questions::from_registry_with_ids(&mapped)
}

fn closure_state(item: &LoopItem, pair: &DecisionPair<'_>, paragraph: &str) -> serde_json::Value {
    let days_later = (pair.later.input.timestamp - pair.source_timestamp).div_euclid(86_400);
    serde_json::json!({
        "obligation": {
            "title": item.action,
            "evidence_text": item.evidence.context,
        },
        "later": {
            "paragraph_text": paragraph,
            "from_user": pair.later.input.from_user,
            "days_later": days_later,
        }
    })
}

struct DecisionProbabilities {
    values: [(DecisionOutcome, f64); 4],
    choice: DecisionChoice,
}

struct DecisionChoice {
    outcome: Option<DecisionOutcome>,
    probability: f64,
    confidence: f64,
}

enum DecisionJobResult {
    Answer(DecisionProbabilities),
    Failed,
}

fn decide_paragraph(
    client: &dyn DecisionClient,
    item: &LoopItem,
    pair: &DecisionPair<'_>,
    job: &DecisionParagraphJob,
    cancel: &AtomicBool,
) -> Result<DecisionProbabilities, ProviderError> {
    let questions = closure_questions(job.ordinal)?;
    let state = closure_state(item, pair, &job.text);
    let answers = client.decide(&state, &questions, Some(cancel), DECISION_DEADLINE)?;
    let mut values = [(DecisionOutcome::Fulfilled, 0.0); 4];
    for (slot, outcome) in DecisionOutcome::ALL.iter().copied().enumerate() {
        let id = format!("p{}.{}", job.ordinal, outcome.suffix());
        let Some(Answer::Noul { probability }) = answers.get(&id) else {
            return Err(ProviderError::InvalidResponse);
        };
        values[slot] = (outcome, *probability);
    }
    let id = format!("p{}.outcome", job.ordinal);
    let Some(Answer::Choice {
        choice,
        probabilities,
        confidence,
    }) = answers.get(&id)
    else {
        return Err(ProviderError::InvalidResponse);
    };
    let outcome = DecisionOutcome::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.suffix() == choice);
    if outcome.is_none() && choice != "none" {
        return Err(ProviderError::InvalidResponse);
    }
    let probability = *probabilities
        .get(choice)
        .ok_or(ProviderError::InvalidResponse)?;
    Ok(DecisionProbabilities {
        values,
        choice: DecisionChoice {
            outcome,
            probability,
            confidence: *confidence,
        },
    })
}

fn normalized_deadline(pair: &DecisionPair<'_>, text: &str) -> Result<Option<String>, ()> {
    let timestamp = pair.later.input.timestamp;
    let offset = local_offset_seconds(timestamp, 0);
    let Some(timezone) = chrono::FixedOffset::east_opt(offset) else {
        return Ok(None);
    };
    let context = governed_temporal_context(&pair.later.input);
    let values: Vec<String> = prose_event_time_candidates(text, timestamp, offset)
        .into_iter()
        .filter_map(|(start, _, _)| {
            let local = timezone.timestamp_opt(start, 0).single()?;
            let value = local.format("%Y-%m-%d").to_string();
            reparse(DeadlineTemporalKind::Date, &value, &context)
                .is_ok()
                .then_some(value)
        })
        .collect();
    match values.as_slice() {
        [] => Ok(None),
        [value] => Ok(Some(value.clone())),
        _ => Err(()),
    }
}

fn accepted_update(
    pair: &DecisionPair<'_>,
    jobs: &[DecisionParagraphJob],
    maxima: &BTreeMap<DecisionOutcome, (f64, usize)>,
) -> Result<Option<SuggestedUpdate>, ()> {
    let mut accepted = Vec::new();
    for (&outcome, &(probability, job_index)) in maxima {
        let threshold = Registry::get()
            .question(outcome.registry_id())
            .expect("closure registry question");
        if probability < threshold.accept {
            continue;
        }
        let temporal = if outcome == DecisionOutcome::DeadlineChanged {
            let Some(value) = normalized_deadline(pair, &jobs[job_index].text)? else {
                continue;
            };
            Some(value)
        } else {
            None
        };
        accepted.push((outcome, probability, job_index, temporal));
    }
    let Some((outcome, probability, job_index, temporal_value)) =
        accepted.into_iter().max_by(|left, right| {
            left.1.total_cmp(&right.1).then_with(|| {
                (left.0 == DecisionOutcome::Fulfilled).cmp(&(right.0 == DecisionOutcome::Fulfilled))
            })
        })
    else {
        return Ok(None);
    };
    let job = &jobs[job_index];
    let confidence_micros = format!("{:.0}", probability * 1_000_000.0)
        .parse()
        .expect("validated probability rounds into u32");
    Ok(Some(SuggestedUpdate {
        kind: outcome.kind(),
        evidence_text: job.text.clone(),
        source_message: pair.later.input.handle.clone(),
        source_block: job.ordinal,
        temporal_value,
        confidence_micros,
    }))
}

fn choice_update(
    pair: &DecisionPair<'_>,
    job: &DecisionParagraphJob,
    choice: &DecisionChoice,
) -> Result<Option<SuggestedUpdate>, ()> {
    let Some(outcome) = choice.outcome else {
        return Ok(None);
    };
    let temporal_value = if outcome == DecisionOutcome::DeadlineChanged {
        let Some(value) = normalized_deadline(pair, &job.text)? else {
            return Ok(None);
        };
        Some(value)
    } else {
        None
    };
    let confidence_micros = format!("{:.0}", choice.probability * 1_000_000.0)
        .parse()
        .expect("validated probability rounds into u32");
    Ok(Some(SuggestedUpdate {
        kind: outcome.kind(),
        evidence_text: job.text.clone(),
        source_message: pair.later.input.handle.clone(),
        source_block: job.ordinal,
        temporal_value,
        confidence_micros,
    }))
}

fn pair_has_escalation(
    pair: &DecisionPair<'_>,
    jobs: &[DecisionParagraphJob],
    maxima: &BTreeMap<DecisionOutcome, (f64, usize)>,
) -> bool {
    maxima.iter().any(|(&outcome, &(probability, job_index))| {
        let threshold = Registry::get()
            .question(outcome.registry_id())
            .expect("closure registry question");
        if probability < threshold.escalate || probability >= threshold.accept {
            return false;
        }
        outcome != DecisionOutcome::DeadlineChanged
            || matches!(
                normalized_deadline(pair, &jobs[job_index].text),
                Ok(Some(_)) | Err(())
            )
    })
}

fn recombine_decisions(
    pairs: &[DecisionPair<'_>],
    jobs: &[DecisionParagraphJob],
    outcomes: JobResults<DecisionJobResult>,
    offered: &[OfferedLoop<'_>],
) -> (
    BTreeMap<usize, SuggestedUpdate>,
    BTreeSet<usize>,
    usize,
    bool,
) {
    let mut maxima = vec![BTreeMap::new(); pairs.len()];
    let mut choices: Vec<Option<(DecisionChoice, usize)>> =
        (0..pairs.len()).map(|_| None).collect();
    let mut skipped = BTreeSet::new();
    let mut cancelled = false;
    for (slot, outcome) in outcomes {
        let pair_index = jobs[slot].pair;
        match outcome {
            JobOutcome::Completed(Ok(DecisionJobResult::Answer(answer))) => {
                for (kind, probability) in answer.values {
                    let entry = maxima[pair_index].entry(kind).or_insert((0.0, slot));
                    if probability > entry.0 {
                        *entry = (probability, slot);
                    }
                }
                let replace = choices[pair_index]
                    .as_ref()
                    .is_none_or(|(current, _)| answer.choice.confidence > current.confidence);
                if replace {
                    choices[pair_index] = Some((answer.choice, slot));
                }
            }
            JobOutcome::Completed(Err(ProviderError::Cancelled)) => cancelled = true,
            JobOutcome::NotStarted => {}
            JobOutcome::Completed(Ok(DecisionJobResult::Failed) | Err(_))
            | JobOutcome::Panicked => {
                skipped.insert(pair_index);
            }
        }
    }
    let mut best = BTreeMap::new();
    let mut escalated = BTreeSet::new();
    for (pair_index, pair) in pairs.iter().enumerate() {
        if skipped.contains(&pair_index) || maxima[pair_index].is_empty() {
            continue;
        }
        let choice_question = Registry::get()
            .question("closure.outcome")
            .expect("closure outcome registry question");
        let Some((choice, choice_job)) = &choices[pair_index] else {
            continue;
        };
        let choice_result = if choice.confidence >= choice_question.accept {
            choice_update(pair, &jobs[*choice_job], choice)
        } else if choice.confidence >= choice_question.escalate {
            accepted_update(pair, jobs, &maxima[pair_index])
        } else {
            Ok(None)
        };
        match choice_result {
            Err(()) => {
                escalated.insert(pair_index);
            }
            Ok(Some(update)) => {
                let item = offered[pair.offered].item;
                if best
                    .get(&item)
                    .is_none_or(|current| outranks(&update, current))
                {
                    best.insert(item, update);
                }
            }
            Ok(None)
                if choice.confidence >= choice_question.escalate
                    && choice.confidence < choice_question.accept
                    && pair_has_escalation(pair, jobs, &maxima[pair_index]) =>
            {
                escalated.insert(pair_index);
            }
            Ok(None) => {}
        }
    }
    (best, escalated, skipped.len(), cancelled)
}

fn apply_updates(result: &mut ScanResult, best: BTreeMap<usize, SuggestedUpdate>) -> usize {
    let mut attached = 0;
    for (item, update) in best {
        let replace = result.analysis.items[item]
            .suggested_update
            .as_ref()
            .is_none_or(|current| outranks(&update, current));
        if replace {
            if result.analysis.items[item].suggested_update.is_none() {
                result.suggested_updates += 1;
            }
            result.analysis.items[item].suggested_update = Some(update);
            attached += 1;
        }
    }
    attached
}

fn escalated_reach<'a>(
    pairs: &[DecisionPair<'a>],
    escalated: &BTreeSet<usize>,
) -> BTreeMap<ConversationKey<'a>, Vec<usize>> {
    let mut reach: BTreeMap<ConversationKey<'a>, Vec<usize>> = BTreeMap::new();
    for &pair_index in escalated {
        let pair = &pairs[pair_index];
        let loops = reach
            .entry((
                pair.later.account.as_str(),
                pair.later.conversation.as_str(),
            ))
            .or_default();
        if !loops.contains(&pair.offered) {
            loops.push(pair.offered);
        }
    }
    reach
}

fn scan_decision_closures(
    messages: &[ReviewMessage],
    progress: &ScanProgress,
    result: &mut ScanResult,
    chat_pass: &ParallelPass,
    chat_client: &dyn ModelClient,
    decision_client: &dyn DecisionClient,
) {
    let offered = offered_loops(&result.analysis.items, messages);
    let reach = loops_by_conversation(&offered, &result.analysis.items, messages);
    let pairs = decision_pairs(&reach, &offered, messages);
    let jobs = decision_paragraph_jobs(&pairs);
    progress.total.fetch_add(jobs.len(), Ordering::Relaxed);
    progress.closure_phase.store(true, Ordering::Relaxed);
    let decision_pass = ParallelPass::new(decision_client.max_parallel());
    let processed_per_job = vec![1; jobs.len()];
    progress
        .conversation_total
        .store(jobs.len(), Ordering::Relaxed);
    let outcomes = run_jobs(&processed_per_job, &decision_pass, progress, &|slot| {
        let job = &jobs[slot];
        let pair = &pairs[job.pair];
        match decide_paragraph(
            decision_client,
            &result.analysis.items[offered[pair.offered].item],
            pair,
            job,
            &progress.cancel,
        ) {
            Ok(answer) => Ok(DecisionJobResult::Answer(answer)),
            Err(ProviderError::Cancelled) => Err(ProviderError::Cancelled),
            Err(_) => Ok(DecisionJobResult::Failed),
        }
    });
    let (best, escalated, skipped, cancelled) =
        recombine_decisions(&pairs, &jobs, outcomes, &offered);
    result.cancelled |= cancelled;
    let decision_suggestions = apply_updates(result, best);
    let (chat_jobs, capped) = if cancelled || progress.cancel.load(Ordering::Relaxed) {
        (Vec::new(), false)
    } else {
        closure_jobs(escalated_reach(&pairs, &escalated), messages)
    };
    progress.total.fetch_add(chat_jobs.len(), Ordering::Relaxed);
    progress
        .conversation_total
        .store(jobs.len() + chat_jobs.len(), Ordering::Relaxed);
    chat_pass.restart();
    let chat_processed = vec![1; chat_jobs.len()];
    let chat_outcomes = run_jobs(&chat_processed, chat_pass, progress, &|slot| {
        closure_request(chat_client, &chat_jobs[slot], &offered, &progress.cancel)
    });
    merge_closure_outcomes(result, messages, &offered, &chat_jobs, chat_outcomes);
    result.conversation_notes.push(format!(
        "{decision_suggestions} suggested updates from the decision model, {} pairs escalated to the chat model, {skipped} skipped (rate limit / errors).",
        escalated.len()
    ));
    if capped {
        result.conversation_notes.push(format!(
            "Escalated update checks were capped at {MAX_CLOSURE_CONVERSATIONS} conversations this scan."
        ));
    }
}

fn scan_closures_selected(
    messages: &[ReviewMessage],
    progress: &ScanProgress,
    result: &mut ScanResult,
    pass: &ParallelPass,
    chat_client: &dyn ModelClient,
    decision_client: Option<&dyn DecisionClient>,
) {
    if result.primary_scan_transport_error {
        return;
    }
    if let Some(decision_client) = decision_client {
        scan_decision_closures(
            messages,
            progress,
            result,
            pass,
            chat_client,
            decision_client,
        );
        progress.reset_pass();
        if progress.cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
        }
    } else {
        scan_closures(messages, progress, result, pass, chat_client);
    }
}

/// After the primary per-conversation scan, asks the model once per
/// conversation that could bear on an open loop whether a later message
/// closes or changes it. The loops offered are the open `request`/`promise`
/// items the signed-in user owes (see [`offered_loops`]), reachable from
/// that conversation under [`loops_by_conversation`]'s scoping. Each
/// accepted claim becomes a pending [`SuggestedUpdate`] on its loop; no item
/// is otherwise changed.
///
/// Skipped entirely when `result.primary_scan_transport_error` is set: the
/// provider is already known to be unreachable or unauthorized. The number
/// of requests is added to `progress.total` and counted in
/// `progress.processed` as each finishes. At most
/// [`MAX_CLOSURE_CONVERSATIONS`] requests are made; when that binds, a
/// content-free note says so. Terminal transport-class provider errors stop
/// the pass, recorded in `result.closure_pass_failure` rather than
/// `failures`; a `Cancelled` answer sets `result.cancelled`. Rate-limited
/// requests are counted in a note and never resent.
fn scan_closures(
    messages: &[ReviewMessage],
    progress: &ScanProgress,
    result: &mut ScanResult,
    pass: &ParallelPass,
    client: &dyn ModelClient,
) {
    if result.primary_scan_transport_error {
        return;
    }
    let offered = offered_loops(&result.analysis.items, messages);
    let reach = loops_by_conversation(&offered, &result.analysis.items, messages);
    let (jobs, capped) = closure_jobs(reach, messages);
    progress.total.fetch_add(jobs.len(), Ordering::Relaxed);
    progress
        .conversation_total
        .store(jobs.len(), Ordering::Relaxed);
    // Left set for the rest of the scan: tells the desktop to label
    // `conversation_index`/`conversation_total` as this pass rather than
    // the primary per-conversation one.
    progress.closure_phase.store(true, Ordering::Relaxed);
    pass.restart();
    let processed_per_job = vec![1; jobs.len()];
    let outcomes = run_jobs(&processed_per_job, pass, progress, &|slot| {
        closure_request(client, &jobs[slot], &offered, &progress.cancel)
    });
    merge_closure_outcomes(result, messages, &offered, &jobs, outcomes);
    // Idle once this pass ends, same as `scan_conversations`.
    progress.reset_pass();
    if progress.cancel.load(Ordering::Relaxed) {
        result.cancelled = true;
    }
    if result.suggested_updates > 0 {
        result.conversation_notes.push(format!(
            "{} suggested update(s) from later messages, awaiting your review.",
            result.suggested_updates
        ));
    }
    if capped {
        result.conversation_notes.push(format!(
            "Update checks were capped at {MAX_CLOSURE_CONVERSATIONS} conversations this scan."
        ));
    }
}

/// Applies the closure pass's answers to `result`, in job order so the
/// outcome does not depend on which worker finished first.
fn merge_closure_outcomes(
    result: &mut ScanResult,
    messages: &[ReviewMessage],
    offered: &[OfferedLoop<'_>],
    jobs: &[ClosureJob<'_>],
    outcomes: JobResults<ClaimAnalysis>,
) {
    let mut rate_limited = 0usize;
    let mut best: BTreeMap<usize, SuggestedUpdate> = BTreeMap::new();
    for (slot, outcome) in outcomes {
        match outcome {
            JobOutcome::Completed(Ok(analysis)) => {
                collect_updates(&analysis, &jobs[slot], offered, messages, &mut best);
            }
            JobOutcome::Completed(Err(ProviderError::Cancelled)) => result.cancelled = true,
            JobOutcome::Completed(Err(
                ProviderError::RateLimited | ProviderError::CreditsInFlight,
            )) => rate_limited += 1,
            JobOutcome::Completed(Err(error)) if is_stop_error(error) => {
                if result.closure_pass_failure.is_none() {
                    result.closure_pass_failure = Some(format!("Closure pass stopped: {error}"));
                }
            }
            JobOutcome::Panicked => {
                if result.closure_pass_failure.is_none() {
                    result.closure_pass_failure = Some("Closure pass failed unexpectedly.".into());
                }
            }
            // A failure about this one conversation, or a job that never
            // started: its loops simply get no suggestion from it.
            JobOutcome::Completed(Err(_)) | JobOutcome::NotStarted => {}
        }
    }
    apply_updates(result, best);
    if rate_limited > 0 {
        result.conversation_notes.push(format!(
            "{rate_limited} update check(s) were rate-limited; they were not resent."
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
/// intersection check). This is a trade-off, not a free improvement: below
/// the 8-group floor, a genuine list address can still bridge two unrelated
/// threads and feed the cross-thread closure pass (`scan_closures`) with a
/// false completion. 8 was chosen as the smallest group count where "half of
/// them" (the `count * 2 >= total` clause) is a meaningful bar at all -- any
/// lower and "half" stops distinguishing a distribution list from a merely
/// frequent individual correspondent.
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
    should_merge_with_rules(
        account,
        a,
        b,
        &ThreadMergeContext {
            messages,
            groups_per_account,
            address_group_counts,
            max_gap_seconds,
            rules: None,
        },
    )
}

struct ThreadMergeContext<'a> {
    messages: &'a [ReviewMessage],
    groups_per_account: &'a BTreeMap<&'a str, usize>,
    address_group_counts: &'a BTreeMap<(&'a str, &'a str), usize>,
    max_gap_seconds: i64,
    rules: Option<&'a RuleDecisions>,
}

fn thread_merge_state(
    a: &ThreadGroup,
    b: &ThreadGroup,
    messages: &[ReviewMessage],
) -> serde_json::Value {
    let earliest = |group: &ThreadGroup| {
        group
            .indices
            .iter()
            .map(|index| &messages[*index])
            .min_by_key(|message| message.input.timestamp)
            .expect("thread group is non-empty")
    };
    let a = earliest(a);
    let b = earliest(b);
    serde_json::json!({
        "subject_a": a.input.message.subject.as_string(),
        "subject_b": b.input.message.subject.as_string(),
        "first_paragraph_a": a.input.message.body_blocks.first().map_or_else(String::new, CanonicalBlock::as_string),
        "first_paragraph_b": b.input.message.body_blocks.first().map_or_else(String::new, CanonicalBlock::as_string),
    })
}

fn should_merge_with_rules(
    account: &str,
    a: &ThreadGroup,
    b: &ThreadGroup,
    context: &ThreadMergeContext<'_>,
) -> bool {
    // Neither group's raw subjects carried a calendar-response prefix, and
    // neither is a group source -- both keep their own thread identity.
    if a.has_team || b.has_team || a.has_calendar_prefix || b.has_calendar_prefix {
        return false;
    }
    // A shared normalized subject that is non-empty and substantial.
    let any_subject_match = a.subjects.intersection(&b.subjects).next().is_some();
    let subjects_match = a
        .subjects
        .intersection(&b.subjects)
        .any(|s| subject_strong_enough(s));
    if !any_subject_match {
        return false;
    }
    // A shared `other_addresses` entry that is not a shared-mailbox or
    // distribution-list address.
    let any_address_match = a.addresses.intersection(&b.addresses).next().is_some();
    let addresses_match = a.addresses.intersection(&b.addresses).any(|addr| {
        !is_shared_mailbox_address(
            account,
            addr,
            context.groups_per_account,
            context.address_group_counts,
        )
    });
    if !any_address_match {
        return false;
    }
    // The nearest pair of messages across the two groups is within
    // `max_gap_seconds` of each other.
    if !groups_within(a, b, context.messages, context.max_gap_seconds) {
        return false;
    }
    (subjects_match && addresses_match)
        || context.rules.is_some_and(|cache| {
            cache.noul(
                RULE_THREAD_MERGE,
                &thread_merge_state(a, b, context.messages),
            )
        })
}

fn thread_rule_states(messages: &[ReviewMessage]) -> Vec<serde_json::Value> {
    const MAX_GAP_SECONDS: i64 = 259_200;
    let mut groups: BTreeMap<(String, String), ThreadGroup> = BTreeMap::new();
    for (index, message) in messages.iter().enumerate() {
        let group = groups
            .entry((message.account.clone(), message.conversation.clone()))
            .or_default();
        let raw_subject = message.input.message.subject.as_string();
        let subject = normalize_subject(&raw_subject);
        if !subject.is_empty() {
            group.subjects.insert(subject);
        }
        group.has_calendar_prefix |= raw_subject_has_calendar_prefix(&raw_subject);
        group.has_team |= message.input.team;
        group
            .addresses
            .extend(message.other_addresses.iter().cloned());
        group.indices.push(index);
    }
    let keys: Vec<_> = groups.keys().cloned().collect();
    let values: Vec<_> = groups.into_values().collect();
    let mut groups_per_account: BTreeMap<&str, usize> = BTreeMap::new();
    for (account, _) in &keys {
        *groups_per_account.entry(account).or_insert(0) += 1;
    }
    let mut address_group_counts = BTreeMap::new();
    for ((account, _), group) in keys.iter().zip(&values) {
        for address in &group.addresses {
            *address_group_counts
                .entry((account.as_str(), address.as_str()))
                .or_insert(0) += 1;
        }
    }
    let mut states = Vec::new();
    for a in 0..values.len() {
        for b in (a + 1)..values.len() {
            if keys[a].0 != keys[b].0
                || values[a].has_team
                || values[b].has_team
                || values[a].has_calendar_prefix
                || values[b].has_calendar_prefix
                || values[a]
                    .subjects
                    .intersection(&values[b].subjects)
                    .next()
                    .is_none()
                || values[a]
                    .addresses
                    .intersection(&values[b].addresses)
                    .next()
                    .is_none()
                || !groups_within(&values[a], &values[b], messages, MAX_GAP_SECONDS)
                || should_merge(
                    &keys[a].0,
                    &values[a],
                    &values[b],
                    messages,
                    &groups_per_account,
                    &address_group_counts,
                    MAX_GAP_SECONDS,
                )
            {
                continue;
            }
            states.push(thread_merge_state(&values[a], &values[b], messages));
        }
    }
    states
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
    merge_threads_with_rules(messages, None)
}

fn merge_threads_with_rules(
    messages: &mut [ReviewMessage],
    rules: Option<&RuleDecisions>,
) -> usize {
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
    let merge_context = ThreadMergeContext {
        messages,
        groups_per_account: &groups_per_account,
        address_group_counts: &address_group_counts,
        max_gap_seconds: MAX_GAP_SECONDS,
        rules,
    };
    for a in 0..group_count {
        for b in (a + 1)..group_count {
            if keys[a].0 != keys[b].0 {
                continue;
            }
            if should_merge_with_rules(
                &keys[a].0,
                &group_values[a],
                &group_values[b],
                &merge_context,
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
const COMPARE_IDS: &[&str] = &[
    "triage.asks_recipient",
    "triage.commits_sender",
    "triage.asks_question",
    "triage.names_time",
    "triage.boilerplate",
    "triage.automated_notification",
    "extract.claim_type",
];

#[derive(Clone)]
struct CompareRow {
    subject: String,
    paragraph_text: String,
    from_user: bool,
    labels: BTreeMap<String, CompareValue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CompareValue {
    Bool(bool),
    Choice(String),
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct CompareCount {
    n: usize,
    agreement: usize,
    gray: usize,
}

fn claim_type_from_labels(labels: &serde_json::Map<String, serde_json::Value>) -> &'static str {
    if labels
        .get("triage.asks_question")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        "question"
    } else if labels
        .get("triage.asks_recipient")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        "request"
    } else if labels
        .get("triage.commits_sender")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        "promise"
    } else {
        "none"
    }
}

fn exported_compare_rows(folder: &Path) -> Result<Vec<CompareRow>, String> {
    let file = std::fs::File::open(folder.join("triage.jsonl"))
        .map_err(|_| "Comparison data could not be opened".to_string())?;
    let mut rows = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let line = line.map_err(|_| "Comparison data could not be read".to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let value = openloops_contracts::parse_strict_json(line.as_bytes())
            .map_err(|_| "Comparison data is invalid".to_string())?;
        let labels = value
            .get("label")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| "Comparison data is invalid".to_string())?;
        let mut derived = BTreeMap::new();
        for id in &COMPARE_IDS[..6] {
            let label = labels
                .get(*id)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            derived.insert((*id).to_string(), CompareValue::Bool(label));
        }
        derived.insert(
            "extract.claim_type".into(),
            CompareValue::Choice(claim_type_from_labels(labels).into()),
        );
        rows.push(CompareRow {
            subject: value
                .get("subject")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "Comparison data is invalid".to_string())?
                .to_string(),
            paragraph_text: value
                .get("paragraph_text")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "Comparison data is invalid".to_string())?
                .to_string(),
            from_user: value
                .get("from_user")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| "Comparison data is invalid".to_string())?,
            labels: derived,
        });
    }
    Ok(rows)
}

fn chat_compare_labels(items: &LoopItems) -> BTreeMap<String, CompareValue> {
    let mut asks_recipient = false;
    let mut commits_sender = false;
    let mut asks_question = false;
    let mut names_time = false;
    let mut claim_type = "none";
    for item in &items.items {
        names_time |= item.deadline.is_some();
        match item.kind.as_str() {
            "question" => {
                asks_question = true;
                asks_recipient = true;
                claim_type = "question";
            }
            "request" if claim_type != "question" => {
                asks_recipient = true;
                claim_type = "request";
            }
            "promise" if !matches!(claim_type, "question" | "request") => {
                commits_sender = true;
                claim_type = "promise";
            }
            "attribution" if claim_type == "none" => claim_type = "attribution",
            "delegation" if claim_type == "none" => claim_type = "delegation",
            _ => {}
        }
    }
    BTreeMap::from([
        (
            "triage.asks_recipient".into(),
            CompareValue::Bool(asks_recipient),
        ),
        (
            "triage.commits_sender".into(),
            CompareValue::Bool(commits_sender),
        ),
        (
            "triage.asks_question".into(),
            CompareValue::Bool(asks_question),
        ),
        ("triage.names_time".into(), CompareValue::Bool(names_time)),
        ("triage.boilerplate".into(), CompareValue::Bool(false)),
        (
            "triage.automated_notification".into(),
            CompareValue::Bool(false),
        ),
        (
            "extract.claim_type".into(),
            CompareValue::Choice(claim_type.into()),
        ),
    ])
}

fn built_in_compare_rows(client: &dyn ModelClient) -> Result<Vec<CompareRow>, ProviderError> {
    let mut rows = Vec::new();
    for (body, from_user, to_user, team, _) in SEMANTIC_CASES {
        let item = synthetic(body, 0, "compare");
        let mut message = prepare(&item, "Synthetic", 0)
            .map_err(|_| ProviderError::InvalidAnalysis)?
            .input;
        message.from_user = from_user;
        message.recipient = if to_user {
            crate::claim_view::UserRecipient::To
        } else {
            crate::claim_view::UserRecipient::NotAddressed
        };
        message.team = team;
        if from_user {
            set_outgoing(&mut message);
        }
        let labels = chat_compare_labels(&governed_pass(
            client,
            std::slice::from_ref(&message),
            None,
        )?);
        rows.push(CompareRow {
            subject: message.message.subject.as_string(),
            paragraph_text: message.message.body_blocks[0].as_string(),
            from_user,
            labels,
        });
    }
    Ok(rows)
}

fn compare_prediction(id: &str, answer: &Answer) -> Result<(CompareValue, bool), ProviderError> {
    let registered = Registry::get()
        .question(id)
        .ok_or(ProviderError::InvalidQuestion)?;
    match answer {
        Answer::Noul { probability } => Ok((
            CompareValue::Bool(*probability >= 0.5),
            *probability >= registered.escalate && *probability < registered.accept,
        )),
        Answer::Choice {
            choice, confidence, ..
        } => Ok((
            CompareValue::Choice(choice.clone()),
            *confidence >= registered.escalate && *confidence < registered.accept,
        )),
        Answer::Score { .. } => Err(ProviderError::InvalidResponse),
    }
}

fn aggregate_comparison(
    counts: &mut BTreeMap<String, CompareCount>,
    labels: &BTreeMap<String, CompareValue>,
    predictions: &BTreeMap<String, (CompareValue, bool)>,
) {
    for id in COMPARE_IDS {
        let Some(label) = labels.get(*id) else {
            continue;
        };
        let Some((prediction, gray)) = predictions.get(*id) else {
            continue;
        };
        let count = counts.entry((*id).into()).or_default();
        count.n += 1;
        count.agreement += usize::from(label == prediction);
        count.gray += usize::from(*gray);
    }
}

/// Converts a paragraph count to `f64` without precision loss for any count
/// this comparison could realistically see (a probe corpus, never a count
/// near `u32::MAX`); a count that did overflow `u32` saturates rather than
/// panicking, since this is a diagnostic ratio, not a persisted value.
fn count_as_f64(count: usize) -> f64 {
    f64::from(u32::try_from(count).unwrap_or(u32::MAX))
}

fn format_comparison(
    counts: &BTreeMap<String, CompareCount>,
    chat_wall: Duration,
    chat_tokens: u64,
    jev_wall: Duration,
    jev_tokens: u64,
) -> Vec<String> {
    COMPARE_IDS
        .iter()
        .map(|id| {
            let count = counts.get(*id).copied().unwrap_or_default();
            let agreement = if count.n == 0 {
                0.0
            } else {
                count_as_f64(count.agreement) / count_as_f64(count.n)
            };
            let gray = if count.n == 0 {
                0.0
            } else {
                count_as_f64(count.gray) / count_as_f64(count.n)
            };
            format!(
                "{id}: n={} agreement={agreement:.3} gray_band={gray:.3} chat_wall_ms={} chat_input_tokens={chat_tokens} jev_wall_ms={} jev_input_tokens={jev_tokens}",
                count.n,
                chat_wall.as_millis(),
                jev_wall.as_millis(),
            )
        })
        .collect()
}

/// Compares paragraph-level chat labels with Jev triage and claim-type answers.
/// `extract.waiting_party` is intentionally not attempted in this phase.
pub fn compare_decisions(
    chat: &OpenRouter,
    decisions: &OpenRouterDecisions,
    data: Option<&Path>,
) -> Result<Vec<String>, String> {
    let chat_started = Instant::now();
    let rows = if let Some(folder) = data {
        exported_compare_rows(folder)?
    } else {
        built_in_compare_rows(chat).map_err(|error| error.to_string())?
    };
    let chat_wall = chat_started.elapsed();
    let chat_tokens = chat.input_tokens();
    let questions = Questions::from_registry(COMPARE_IDS).map_err(|error| error.to_string())?;
    let jev_started = Instant::now();
    let mut jev_tokens = 0;
    let mut counts = BTreeMap::new();
    for row in rows {
        let state = serde_json::json!({
            "subject": row.subject,
            "paragraph_text": row.paragraph_text,
            "from_user": row.from_user,
        });
        let answers = decisions
            .decide(&state, &questions, None, DECISION_DEADLINE)
            .map_err(|error| error.to_string())?;
        jev_tokens += answers.input_tokens;
        let predictions = COMPARE_IDS
            .iter()
            .map(|id| {
                let answer = answers.get(id).ok_or(ProviderError::InvalidResponse)?;
                Ok(((*id).to_string(), compare_prediction(id, answer)?))
            })
            .collect::<Result<BTreeMap<_, _>, ProviderError>>()
            .map_err(|error| error.to_string())?;
        aggregate_comparison(&mut counts, &row.labels, &predictions);
    }
    Ok(format_comparison(
        &counts,
        chat_wall,
        chat_tokens,
        jev_started.elapsed(),
        jev_tokens,
    ))
}

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
// decision, and got it one message later, must produce a governed closure
// suggestion for the offered loop.
const AGREEMENT_CASE: (&str, &str) = (
    "Can we move our meeting to a different time?",
    "Yes, happy to push it back an hour.",
);
// A correction that leaves the underlying action owed must produce a
// governed modification suggestion rather than a closure.
const AMENDMENT_CASE: (&str, &str) = (
    "My fee for the call is 359 USD, to be paid any time before our call.",
    "Apologies, the fee for the call is 350, not 359.",
);
// A plain completion must produce a governed closure suggestion.
const COMPLETED_RESOLUTION_CASE: (&str, &str) = (
    "Please send me the signed engagement letter.",
    "Attached is the signed engagement letter.",
);

const PROBE_LOOP_HANDLE: &str = "probe-loop";

/// Runs the fixed synthetic probe cases one at a time. The probe measures
/// whether a model understands the cases at all, so it stays sequential:
/// concurrency would only change how fast a diagnostic finishes.
pub fn probe(provider: Provider, key: String, model: &str) -> Result<usize, ProviderError> {
    let client = connect(provider, key, model, 1)?;
    let client = client.as_ref();
    let mut passed = 0;
    for (body, from_user, to_user, team, expected) in SEMANTIC_CASES {
        let item = synthetic(body, 0, "a");
        let mut m = prepare(&item, "Synthetic", 0)
            .map_err(|_| ProviderError::InvalidAnalysis)?
            .input;
        m.from_user = from_user;
        m.recipient = if to_user {
            crate::claim_view::UserRecipient::To
        } else {
            crate::claim_view::UserRecipient::NotAddressed
        };
        m.team = team;
        if from_user {
            set_outgoing(&mut m);
        }
        let result = governed_pass(client, &[m], None)?;
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
            && result
                .items
                .iter()
                .any(|item| item.owner != if team { Owner::Team } else { Owner::You })
        {
            return Err(ProviderError::InvalidAnalysis);
        }
        if passed < 2
            && result.items[0]
                .deadline
                .as_ref()
                .is_none_or(|d| !d.quote.to_ascii_lowercase().contains("friday"))
        {
            return Err(ProviderError::InvalidAnalysis);
        }
        if expected == 2 && result.items[0].action_phrase == result.items[1].action_phrase {
            return Err(ProviderError::InvalidAnalysis);
        }
        passed += 1;
    }
    let completion = governed_probe_update(
        client,
        &synthetic("Please send the draft budget.", 0, "b"),
        &synthetic(
            "I have sent the completed draft budget as requested.",
            1,
            "b",
        ),
        true,
    )?;
    print_claim_probe(passed + 1, &completion);
    if !has_governed_update(&completion, ClaimType::PossibleClosure)
        || !completion.rejected.is_empty()
    {
        return Err(ProviderError::InvalidAnalysis);
    }
    println!(
        "Semantic case {}: suggested closure identified.",
        passed + 1
    );
    let acknowledgement = governed_probe_update(
        client,
        &synthetic("Please send the draft budget.", 0, "b"),
        &synthetic("Thanks, I will take a look at this later.", 1, "b"),
        true,
    )?;
    print_claim_probe(passed + 2, &acknowledgement);
    if has_governed_update(&acknowledgement, ClaimType::PossibleClosure)
        || !acknowledgement.rejected.is_empty()
    {
        return Err(ProviderError::InvalidAnalysis);
    }
    println!(
        "Semantic case {}: acknowledgement did not close the request.",
        passed + 2
    );
    probe_agreement_case(client, passed + 3)?;
    probe_amendment_case(client, passed + 4)?;
    probe_completed_resolution_case(client, passed + 5)?;
    Ok(passed + 5)
}

fn governed_probe_update(
    client: &dyn ModelClient,
    request: &MailItem,
    reply: &MailItem,
    reply_from_user: bool,
) -> Result<ClaimAnalysis, ProviderError> {
    let mut request_message = prepare(request, "Synthetic", 0)
        .map_err(|_| ProviderError::InvalidAnalysis)?
        .input;
    request_message.recipient = crate::claim_view::UserRecipient::To;
    let mut reply_message = prepare(reply, "Synthetic", 1)
        .map_err(|_| ProviderError::InvalidAnalysis)?
        .input;
    reply_message.from_user = reply_from_user;
    reply_message.recipient = if reply_from_user {
        crate::claim_view::UserRecipient::NotAddressed
    } else {
        crate::claim_view::UserRecipient::To
    };
    if reply_from_user {
        set_outgoing(&mut reply_message);
    }
    governed_call(
        client,
        &[request_message, reply_message],
        &[PROBE_LOOP_HANDLE],
        None,
    )
}

fn print_claim_probe(case_number: usize, result: &ClaimAnalysis) {
    println!(
        "Semantic case {}: {} accepted, {} rejected, 0 degraded.",
        case_number,
        result.accepted.len(),
        result.rejected.len()
    );
    for reason in &result.rejected {
        println!("{}", rejection_label(*reason));
    }
}

fn has_governed_update(result: &ClaimAnalysis, kind: ClaimType) -> bool {
    result.accepted.iter().any(|accepted| {
        accepted.claim.claim_type == kind
            && accepted
                .claim
                .related_loop_handles
                .iter()
                .any(|handle| handle == PROBE_LOOP_HANDLE)
    })
}

/// Runs `AGREEMENT_CASE` through the governed update path.
fn probe_agreement_case(client: &dyn ModelClient, case_number: usize) -> Result<(), ProviderError> {
    let (request, reply) = AGREEMENT_CASE;
    let result = governed_probe_update(
        client,
        &synthetic(request, 0, "c"),
        &synthetic(reply, 2, "c"),
        true,
    )?;
    print_claim_probe(case_number, &result);
    if !has_governed_update(&result, ClaimType::PossibleClosure) || !result.rejected.is_empty() {
        return Err(ProviderError::InvalidAnalysis);
    }
    println!("Semantic case {case_number}: suggested closure identified.");
    Ok(())
}

/// Runs `AMENDMENT_CASE` through the governed update path.
fn probe_amendment_case(client: &dyn ModelClient, case_number: usize) -> Result<(), ProviderError> {
    let (request, correction) = AMENDMENT_CASE;
    let result = governed_probe_update(
        client,
        &synthetic(request, 0, "e"),
        &synthetic(correction, 2, "e"),
        false,
    )?;
    print_claim_probe(case_number, &result);
    if !has_governed_update(&result, ClaimType::Modification)
        || has_governed_update(&result, ClaimType::PossibleClosure)
        || !result.rejected.is_empty()
    {
        return Err(ProviderError::InvalidAnalysis);
    }
    println!("Semantic case {case_number}: suggested modification identified.");
    Ok(())
}

/// Runs `COMPLETED_RESOLUTION_CASE` through the governed update path.
fn probe_completed_resolution_case(
    client: &dyn ModelClient,
    case_number: usize,
) -> Result<(), ProviderError> {
    let (request, reply) = COMPLETED_RESOLUTION_CASE;
    let result = governed_probe_update(
        client,
        &synthetic(request, 0, "d"),
        &synthetic(reply, 2, "d"),
        true,
    )?;
    print_claim_probe(case_number, &result);
    if !has_governed_update(&result, ClaimType::PossibleClosure) || !result.rejected.is_empty() {
        return Err(ProviderError::InvalidAnalysis);
    }
    println!("Semantic case {case_number}: suggested closure identified.");
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
    use crate::claim_view::DELEGATION_UNCERTAINTY;
    use std::time::Instant;

    fn vocative_message() -> ConversationMessage {
        ConversationMessage {
            handle: "m0".into(),
            message: CanonicalMessage {
                subject: CanonicalBlock::new("Synthetic thread").unwrap(),
                body_blocks: vec![],
                quote_blocks: vec![],
                sender: Some(
                    CanonicalBlock::new("Taylor Sender <sender@example.invalid>").unwrap(),
                ),
                to: vec![
                    CanonicalBlock::new("Synthetic User <user@example.invalid>").unwrap(),
                    CanonicalBlock::new("Jordan Recipient <recipient@example.invalid>").unwrap(),
                ],
                cc: vec![CanonicalBlock::new("Casey Observer <observer@example.invalid>").unwrap()],
                attachment_names: vec![],
                link_labels: vec![],
            },
            timestamp: 0,
            from_user: false,
            recipient: crate::claim_view::UserRecipient::To,
            own_addresses: vec!["user@example.invalid".into()],
            user_display_name: Some("Synthetic User".into()),
            user_given_name: Some("Synthetic".into()),
            team: false,
        }
    }

    #[test]
    fn non_user_vocative_downgrades_request_owner_with_identity_ambiguity() {
        let (owner, identity) = resolve_owner_with_vocative_guard(
            ClaimType::Request,
            Owner::You,
            "Jordan, find a time.",
            &vocative_message(),
        );
        assert!(owner == Owner::Unclear);
        assert!(identity);
    }

    #[test]
    fn user_or_unknown_or_absent_vocative_leaves_owner_unchanged() {
        let message = vocative_message();
        for text in [
            "Synthetic, find a time.",
            "Morgan, find a time.",
            "Please find a time.",
        ] {
            let (owner, identity) =
                resolve_owner_with_vocative_guard(ClaimType::Request, Owner::You, text, &message);
            assert!(owner == Owner::You, "text: {text}");
            assert!(!identity, "text: {text}");
        }
    }

    #[test]
    fn prepare_marks_the_user_in_cc() {
        let mut item = synthetic("Synthetic note.", 0, "cc");
        item.to = vec!["recipient@example.invalid".into()];
        item.cc = vec!["user@example.invalid".into()];
        let prepared = prepare(&item, "Inbox", 0).unwrap();
        assert!(!prepared.input.to_user());
        assert!(prepared.input.cc_user());
    }

    /// Runs the primary pass with a single worker and an `FnMut` callback:
    /// the shape the pass had before it was parallelized. Most tests here
    /// assert per-call bookkeeping and read best with a callback that owns
    /// mutable state, and a one-worker pool is exactly the sequential loop.
    fn scan_conversations(
        messages: &[ReviewMessage],
        progress: &ScanProgress,
        analyze: impl FnMut(&[ConversationMessage]) -> Result<LoopItems, ProviderError> + Send,
    ) -> ScanResult {
        sequential_conversations(messages, progress, analyze)
    }

    fn sequential_conversations(
        messages: &[ReviewMessage],
        progress: &ScanProgress,
        analyze: impl FnMut(&[ConversationMessage]) -> Result<LoopItems, ProviderError> + Send,
    ) -> ScanResult {
        let analyze = Mutex::new(analyze);
        super::scan_conversations(messages, progress, &ParallelPass::new(1), &|conversation| {
            (analyze.lock().unwrap_or_else(PoisonError::into_inner))(conversation)
        })
    }

    #[test]
    fn conversation_filter_analyzes_only_selected_conversations() {
        let messages = [
            prepare(
                &synthetic("Synthetic selected body", 0, "selected"),
                "Inbox",
                0,
            )
            .unwrap(),
            prepare(
                &synthetic("Synthetic untouched body", 1, "untouched"),
                "Inbox",
                1,
            )
            .unwrap(),
        ];
        let calls = AtomicUsize::new(0);
        let filter = BTreeSet::from(["selected".to_string()]);
        let result = super::scan_conversations_filtered(
            &messages,
            &ScanProgress::default(),
            &ParallelPass::new(1),
            Some(&filter),
            &|_| {
                calls.fetch_add(1, Ordering::Relaxed);
                Ok(LoopItems {
                    items: vec![],
                    rejected: 0,
                    rejection_reasons: vec![],
                    degraded: 0,
                })
            },
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(result.conversation_count, 1);
        assert_eq!(result.total, 1);
    }

    #[test]
    fn full_scan_quality_totals_equal_the_sum_of_conversation_quality() {
        let messages = [
            prepare(
                &synthetic("First synthetic message", 0, "conversation-a"),
                "Inbox",
                0,
            )
            .unwrap(),
            prepare(
                &synthetic("Second synthetic message", 1, "conversation-b"),
                "Inbox",
                1,
            )
            .unwrap(),
        ];
        let call = AtomicUsize::new(0);
        let result = scan_conversations(&messages, &ScanProgress::default(), |_| {
            let (rejected, degraded) = if call.fetch_add(1, Ordering::Relaxed) == 0 {
                (2, 1)
            } else {
                (3, 4)
            };
            Ok(LoopItems {
                items: vec![],
                rejected,
                rejection_reasons: vec![],
                degraded,
            })
        });

        let (rejected, degraded) = result
            .conversation_quality
            .values()
            .copied()
            .fold((0, 0), |(rejected, degraded), quality| {
                (rejected + quality.0, degraded + quality.1)
            });
        assert_eq!(result.analysis.rejected, 5);
        assert_eq!(result.analysis.degraded, 5);
        assert_eq!((rejected, degraded), (5, 5));
    }

    #[test]
    fn structured_failure_contains_subject_snippet_but_never_body() {
        let body = "SYNTHETIC-BODY-MUST-NOT-ESCAPE";
        let message = prepare(&synthetic(body, 0, "failed"), "Inbox", 0).unwrap();
        let result = scan_conversations(&[message], &ScanProgress::default(), |_| {
            Err(ProviderError::Timeout)
        });
        assert_eq!(result.failed_conversations_detail.len(), 1);
        let detail = &result.failed_conversations_detail[0];
        assert_eq!(detail.subject_short, "Synthetic budget conversation");
        assert_eq!(detail.reason, FailureReason::Timeout);
        assert!(!format!("{detail:?}").contains(body));
        assert_eq!(
            result.failures.len(),
            result.failed_conversations_detail.len()
        );
    }

    fn timestamp(value: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(value)
            .unwrap()
            .timestamp()
    }

    /// A synthetic [`ModelClient`] that always answers with the fixed body
    /// it was constructed with, standing in for a real provider so
    /// [`governed_pass`] can be exercised against a hand-written governed
    /// answer instead of a network call.
    struct FixedAnalysisClient(String);

    impl ModelClient for FixedAnalysisClient {
        fn model(&self) -> &'static str {
            "fixed"
        }

        fn max_parallel(&self) -> usize {
            1
        }

        fn complete(
            &self,
            _system: &str,
            _user: &str,
            _cancel: Option<&AtomicBool>,
            _deadline: std::time::Duration,
        ) -> Result<zeroize::Zeroizing<String>, ProviderError> {
            Ok(zeroize::Zeroizing::new(self.0.clone()))
        }
    }

    /// The fixed sender text of the synthetic `m0` message in
    /// [`governed_pass_fixture`], reused by the test's own assertions.
    const FIXTURE_SENDER: &str = "Alex <alex@example.invalid>";

    /// Builds the two-message synthetic conversation and the hand-written
    /// `analysis-output-v1` document
    /// [`governed_pass_maps_claims_and_reports_skips_and_rejections`]
    /// exercises: a request with a `relative` deadline and a named waiting
    /// party, a question, a promise with a `local_datetime` deadline, a
    /// delegation whose ambiguity codes include the redundant `delegation`
    /// code, an attribution, a `possible_closure` with no offered loop
    /// handles (must be skipped), a claim citing a `sender` component (must
    /// be skipped), and a claim the validator rejects on range bounds.
    fn governed_pass_fixture() -> ([ConversationMessage; 2], String) {
        let m0_body = "Please send the report. Can you confirm receipt?";
        let m1_body = "I will send the deck by 2026-09-01T21:00. I asked Sam to \
            handle the appendix, but you are still on the hook for the deck. \
            Already sent the earlier draft.";
        let m0 = ConversationMessage {
            handle: "m0".into(),
            message: CanonicalMessage {
                subject: CanonicalBlock::new("Synthetic thread").unwrap(),
                body_blocks: vec![CanonicalBlock::new(m0_body).unwrap()],
                quote_blocks: vec![],
                sender: Some(CanonicalBlock::new(FIXTURE_SENDER).unwrap()),
                to: vec![CanonicalBlock::new("User <user@example.invalid>").unwrap()],
                cc: vec![],
                attachment_names: vec![],
                link_labels: vec![],
            },
            timestamp: timestamp("2026-08-28T12:00:00Z"),
            from_user: false,
            recipient: crate::claim_view::UserRecipient::To,
            own_addresses: vec!["user@example.invalid".into()],
            user_display_name: Some("Synthetic User".into()),
            user_given_name: Some("Synthetic".into()),
            team: false,
        };
        let m1 = ConversationMessage {
            handle: "m1".into(),
            message: CanonicalMessage {
                subject: CanonicalBlock::new("Synthetic thread").unwrap(),
                body_blocks: vec![CanonicalBlock::new(m1_body).unwrap()],
                quote_blocks: vec![],
                sender: Some(CanonicalBlock::new("User <user@example.invalid>").unwrap()),
                to: vec![CanonicalBlock::new(FIXTURE_SENDER).unwrap()],
                cc: vec![],
                attachment_names: vec![],
                link_labels: vec![],
            },
            timestamp: timestamp("2026-08-29T09:00:00Z"),
            from_user: true,
            recipient: crate::claim_view::UserRecipient::NotAddressed,
            own_addresses: vec!["user@example.invalid".into()],
            user_display_name: Some("Synthetic User".into()),
            user_given_name: Some("Synthetic".into()),
            team: false,
        };
        let m0_len = m0_body.chars().count();
        let m1_len = m1_body.chars().count();
        let sender_len = FIXTURE_SENDER.chars().count();
        let bad_range = m1_len + 500;
        let body = format!(
            r#"{{"schema_version":1,"claims":[
                {{"claim_type":"request","evidence":[{{"source_handle":"m0","component":"body_block","block_ordinal":0,"range_start":0,"range_end":{m0_len}}}],"waiting_party_handle":"m0-sender","related_loop_handles":[],"temporal":{{"text_evidence_index":0,"kind":"relative","value":"friday"}},"confidence_micros":900000,"ambiguity_codes":[]}},
                {{"claim_type":"question","evidence":[{{"source_handle":"m0","component":"body_block","block_ordinal":0,"range_start":0,"range_end":{m0_len}}}],"waiting_party_handle":null,"related_loop_handles":[],"temporal":null,"confidence_micros":900000,"ambiguity_codes":[]}},
                {{"claim_type":"promise","evidence":[{{"source_handle":"m1","component":"body_block","block_ordinal":0,"range_start":0,"range_end":{m1_len}}}],"waiting_party_handle":null,"related_loop_handles":[],"temporal":{{"text_evidence_index":0,"kind":"local_datetime","value":"2026-09-01T21:00"}},"confidence_micros":900000,"ambiguity_codes":[]}},
                {{"claim_type":"delegation","evidence":[{{"source_handle":"m1","component":"body_block","block_ordinal":0,"range_start":0,"range_end":{m1_len}}}],"waiting_party_handle":null,"related_loop_handles":[],"temporal":null,"confidence_micros":900000,"ambiguity_codes":["delegation","insufficient_context"]}},
                {{"claim_type":"attribution","evidence":[{{"source_handle":"m0","component":"body_block","block_ordinal":0,"range_start":0,"range_end":{m0_len}}}],"waiting_party_handle":null,"related_loop_handles":[],"temporal":null,"confidence_micros":900000,"ambiguity_codes":[]}},
                {{"claim_type":"possible_closure","evidence":[{{"source_handle":"m1","component":"body_block","block_ordinal":0,"range_start":0,"range_end":{m1_len}}}],"waiting_party_handle":null,"related_loop_handles":[],"temporal":null,"confidence_micros":900000,"ambiguity_codes":[]}},
                {{"claim_type":"request","evidence":[{{"source_handle":"m0","component":"sender","block_ordinal":0,"range_start":0,"range_end":{sender_len}}}],"waiting_party_handle":null,"related_loop_handles":[],"temporal":null,"confidence_micros":900000,"ambiguity_codes":[]}},
                {{"claim_type":"attribution","evidence":[{{"source_handle":"m1","component":"body_block","block_ordinal":0,"range_start":0,"range_end":{bad_range}}}],"waiting_party_handle":null,"related_loop_handles":[],"temporal":null,"confidence_micros":900000,"ambiguity_codes":[]}}
            ]}}"#
        );
        ([m0, m1], body)
    }

    /// Exercises [`governed_pass`] end to end against
    /// [`governed_pass_fixture`]'s hand-written governed answer, covering
    /// every mapping branch described there.
    #[test]
    fn governed_pass_maps_claims_and_reports_skips_and_rejections() {
        let (conversation, body) = governed_pass_fixture();
        let m0_timestamp = conversation[0].timestamp;
        let m1_timestamp = conversation[1].timestamp;
        let client = FixedAnalysisClient(body);
        let result = governed_pass(&client, &conversation, None).unwrap();

        assert_eq!(
            result.items.len(),
            5,
            "{:?}",
            result.items.iter().map(|i| &i.action).collect::<Vec<_>>()
        );
        assert_eq!(result.rejected, 3);
        assert!(
            result
                .rejection_reasons
                .contains(&CLOSURE_OR_CHANGE_SKIPPED_NOTE)
        );
        assert!(result.rejection_reasons.contains(&NON_BODY_EVIDENCE_NOTE));
        assert!(
            result
                .rejection_reasons
                .contains(&"Evidence range fell outside its block.")
        );

        let request_item = result
            .items
            .iter()
            .find(|i| i.action.starts_with("Requested: "))
            .expect("request item");
        assert!(request_item.owner == Owner::You);
        assert_eq!(request_item.waiting_party, FIXTURE_SENDER);
        let deadline = request_item.deadline.as_ref().expect("request deadline");
        assert_eq!(deadline.quote, "friday");
        assert_ne!(
            classify(&deadline.quote, m0_timestamp, m0_timestamp + 86_400, 0),
            DeadlineView::Unknown
        );

        let question_item = result
            .items
            .iter()
            .find(|i| i.action.starts_with("Answer: "))
            .expect("question item");
        assert!(question_item.owner == Owner::You);

        let promise_item = result
            .items
            .iter()
            .find(|i| i.action.starts_with("You promised: "))
            .expect("promise item");
        assert!(promise_item.owner == Owner::You);
        let promise_deadline = promise_item.deadline.as_ref().expect("promise deadline");
        assert_eq!(promise_deadline.quote, "2026-09-01T21:00");
        assert_ne!(
            classify(
                &promise_deadline.quote,
                m1_timestamp,
                m1_timestamp + 86_400,
                0
            ),
            DeadlineView::Unknown
        );

        let delegation_item = result
            .items
            .iter()
            .find(|i| i.action.starts_with("Delegated: "))
            .expect("delegation item");
        assert_eq!(
            delegation_item.uncertainty,
            format!("{DELEGATION_UNCERTAINTY}; the conversation may not show enough")
        );

        let attribution_item = result
            .items
            .iter()
            .find(|i| i.action.starts_with("Someone else owes: "))
            .expect("attribution item");
        assert_eq!(attribution_item.kind, "attributed");
    }

    #[test]
    fn governed_pass_titles_an_html_request_from_its_paragraph_not_the_greeting() {
        let mut item = synthetic(
            "<p>Hello team,</p><p>Thanks for reviewing.</p><p>Please send the synthetic report.</p>",
            0,
            "paragraph-request",
        );
        item.body_is_html = true;
        let prepared = prepare(&item, "Inbox", 0).unwrap();
        let request = "Please send the synthetic report.";
        let body = format!(
            r#"{{"schema_version":1,"claims":[{{"claim_type":"request","evidence":[{{"source_handle":"m0","component":"body_block","block_ordinal":2,"range_start":0,"range_end":{}}}],"waiting_party_handle":null,"related_loop_handles":[],"temporal":null,"confidence_micros":900000,"ambiguity_codes":[]}}]}}"#,
            request.chars().count()
        );
        let result = governed_pass(&FixedAnalysisClient(body), &[prepared.input], None).unwrap();

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].action, format!("Requested: {request}"));
        assert_eq!(result.items[0].evidence.block, 2);
        assert!(!result.items[0].action.contains("Hello team"));
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
                // All-day: ends at local end-of-day (17:00), consistent
                // with `prose_event_time`'s own all-day policy -- not
                // literal midnight-to-midnight.
                "Invitation: Design workshop @ Fri Aug 21, 2026",
                "2026-08-21T00:00:00Z",
                "2026-08-21T17:00:00Z",
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
    fn prose_event_time_parses_every_documented_date_form_to_the_same_day() {
        let message = timestamp("2026-08-01T00:00:00Z");
        let expected_start = timestamp("2026-09-03T00:00:00Z");
        let expected_end = expected_start + i64::from(DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT);
        let cases = [
            ("Let's meet September 3 to review.", "September 3"),
            ("Let's meet Sept 3 to review.", "Sept 3"),
            ("Let's meet Sep. 3, 2026 to review.", "Sep. 3"),
            ("Let's meet 3 September 2026 to review.", "3 September"),
            ("Let's meet 9/3/2026 to review.", "9/3/2026"),
            ("Let's meet 2026-09-03 to review.", "2026-09-03"),
        ];
        for (text, needle) in cases {
            let (start, end, byte_offset) =
                prose_event_time(text, message, 0).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(start, expected_start, "{text}");
            assert_eq!(end, expected_end, "{text}");
            assert_eq!(
                &text[byte_offset..byte_offset + needle.len()],
                needle,
                "{text}"
            );
        }
    }

    /// `n/m/yyyy` is month-first; only a first field above 12 flips to
    /// day-first.
    #[test]
    fn numeric_dates_are_month_first_unless_the_first_field_exceeds_twelve() {
        assert_eq!(parse_numeric_date("3/9/2026"), Some((3, 9, Some(2026))));
        assert_eq!(parse_numeric_date("9/3/2026"), Some((9, 3, Some(2026))));
        assert_eq!(
            parse_numeric_date("9/15/2026"),
            Some((9, 15, Some(2026))),
            "month/day/year: 9 <= 12, 15 > 12 -- unambiguously month=9, day=15"
        );
        assert_eq!(
            parse_numeric_date("15/9/2026"),
            Some((9, 15, Some(2026))),
            "day/month/year: 15 > 12, 9 <= 12 -- unambiguously day=15, month=9"
        );
    }

    /// Same policy exercised through `prose_event_time`, matching the
    /// review's exact probes.
    #[test]
    fn prose_event_time_reads_numeric_dates_month_first() {
        let message = timestamp("2026-08-01T00:00:00Z");
        let (start, _, _) = prose_event_time("Meet on 9/3/2026.", message, 0).unwrap();
        assert_eq!(start, timestamp("2026-09-03T00:00:00Z"));
        let (start, _, _) = prose_event_time("Meet on 9/15/2026.", message, 0).unwrap();
        assert_eq!(start, timestamp("2026-09-15T00:00:00Z"));
        let (start, _, _) = prose_event_time("Meet on 15/9/2026.", message, 0).unwrap();
        assert_eq!(start, timestamp("2026-09-15T00:00:00Z"));
    }

    #[test]
    fn prose_event_time_reads_an_optional_trailing_time() {
        let message = timestamp("2026-08-01T00:00:00Z");
        let day_start = timestamp("2026-09-03T00:00:00Z");
        for (text, seconds_since_midnight) in [
            ("Meet September 3 2pm sharp.", 14 * 3600),
            ("Meet September 3 2:30 pm sharp.", 14 * 3600 + 1800),
            ("Meet September 3 14:00 sharp.", 14 * 3600),
        ] {
            let (start, end, _) = prose_event_time(text, message, 0).unwrap();
            let expected_start = day_start + seconds_since_midnight;
            assert_eq!(start, expected_start, "{text}");
            assert_eq!(end, start + 3600, "{text}");
        }
    }

    /// A time range -- `H[:MM][am|pm]` followed by a dash (ASCII hyphen,
    /// en dash, em dash) or " to " and a second `H[:MM][am|pm]` -- resolves
    /// `end` to the range's own end instead of `start + 1h`, and a missing
    /// meridiem is filled in per the documented rules.
    #[test]
    fn prose_event_time_reads_a_time_range() {
        let message = timestamp("2026-08-01T00:00:00Z");
        let day_start = timestamp("2026-09-03T00:00:00Z");
        for (text, start_seconds, end_seconds) in [
            (
                "Meet September 3 2:00-5:30pm sharp.",
                14 * 3600,
                17 * 3600 + 1800,
            ),
            (
                "Meet September 3 2:00\u{2013}5:30pm sharp.",
                14 * 3600,
                17 * 3600 + 1800,
            ),
            (
                "Meet September 3 2:00\u{2014}5:30pm sharp.",
                14 * 3600,
                17 * 3600 + 1800,
            ),
            (
                "Meet September 3 2:00 to 5:30pm sharp.",
                14 * 3600,
                17 * 3600 + 1800,
            ),
            ("Meet September 3 9-11am sharp.", 9 * 3600, 11 * 3600),
            (
                "Meet September 3 11-1 sharp.",
                11 * 3600,
                13 * 3600,
                // Neither side states am/pm and the end (1) is smaller than
                // the start (11), so the end is read as pm: 11am-1pm.
            ),
        ] {
            let (start, end, _) =
                prose_event_time(text, message, 0).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(start, day_start + start_seconds, "{text}");
            assert_eq!(end, day_start + end_seconds, "{text}");
        }
    }

    /// A spaced ASCII hyphen ("2:00 - 5:30pm", a standalone "-" token) or
    /// one glued onto only one side ("2:00- 5:30pm", "2:00 -5:30pm") reads
    /// the same as the fused "2:00-5:30pm".
    #[test]
    fn prose_event_time_reads_a_spaced_or_one_sided_ascii_hyphen_range() {
        let message = timestamp("2026-08-01T00:00:00Z");
        let day_start = timestamp("2026-09-03T00:00:00Z");
        for text in [
            "Meet September 3 2:00 - 5:30pm sharp.",
            "Meet September 3 2:00- 5:30pm sharp.",
            "Meet September 3 2:00 -5:30pm sharp.",
        ] {
            let (start, end, _) =
                prose_event_time(text, message, 0).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(start, day_start + 14 * 3600, "{text}");
            assert_eq!(end, day_start + 17 * 3600 + 1800, "{text}");
        }
    }

    /// "9 - 11am" (a spaced dash with the meridiem only on the end) used to
    /// yield nothing at all; it now reads the same as "9-11am".
    #[test]
    fn prose_event_time_reads_a_spaced_ascii_hyphen_range_with_a_one_sided_meridiem() {
        let message = timestamp("2026-08-01T00:00:00Z");
        let day_start = timestamp("2026-09-03T00:00:00Z");
        let (start, end, _) =
            prose_event_time("Meet September 3 9 - 11am sharp.", message, 0).expect("9 - 11am");
        assert_eq!(start, day_start + 9 * 3600);
        assert_eq!(end, day_start + 11 * 3600);
    }

    /// A range that crosses midnight adds a day to `end`, mirroring
    /// `subject_event_time`'s own policy for the same case.
    #[test]
    fn prose_event_time_range_crossing_midnight_adds_a_day_to_the_end() {
        let message = timestamp("2026-08-01T00:00:00Z");
        let day_start = timestamp("2026-09-03T00:00:00Z");
        for (text, start_seconds, end_seconds) in [
            ("Meet September 3 11pm-1am sharp.", 23 * 3600, 86_400 + 3600),
            (
                "Meet September 3 3pm-1pm sharp.",
                15 * 3600,
                86_400 + 13 * 3600,
            ),
        ] {
            let (start, end, _) =
                prose_event_time(text, message, 0).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(start, day_start + start_seconds, "{text}");
            assert_eq!(end, day_start + end_seconds, "{text}");
        }
    }

    /// A meridiem spaced off from the range's end ("2:00-5:30 pm") is
    /// absorbed into it, and from there inherited by the start exactly as
    /// an attached one would be.
    #[test]
    fn prose_event_time_reads_a_spaced_trailing_meridiem() {
        let message = timestamp("2026-08-01T00:00:00Z");
        let day_start = timestamp("2026-09-03T00:00:00Z");
        let (start, end, _) =
            prose_event_time("Meet September 3 2:00\u{2013}5:30 pm sharp.", message, 0)
                .expect("2:00-5:30 pm");
        assert_eq!(start, day_start + 14 * 3600);
        assert_eq!(end, day_start + 17 * 3600 + 1800);
    }

    /// A lone "-" token between the date and the time -- an ASCII hyphen
    /// surrounded by whitespace, e.g. "September 3 - 2pm" -- is skipped the
    /// same way a preposition is, for both a single time and a range.
    #[test]
    fn prose_event_time_skips_a_lone_dash_between_the_date_and_the_time() {
        let message = timestamp("2026-08-01T00:00:00Z");
        let day_start = timestamp("2026-09-03T00:00:00Z");
        let (start, end, _) =
            prose_event_time("Meet September 3 - 2pm sharp.", message, 0).expect("- 2pm");
        assert_eq!(start, day_start + 14 * 3600);
        assert_eq!(end, start + 3600);

        let (start, end, _) = prose_event_time("Meet September 3 - 2:00-5:30pm sharp.", message, 0)
            .expect("- 2:00-5:30pm");
        assert_eq!(start, day_start + 14 * 3600);
        assert_eq!(end, day_start + 17 * 3600 + 1800);
    }

    /// A trailing sentence period on the range's end ("10:00 to 11:30.")
    /// does not block it from parsing, the same way one never blocks a
    /// date.
    #[test]
    fn prose_event_time_reads_a_range_that_ends_the_sentence() {
        let message = timestamp("2026-08-01T00:00:00Z");
        let day_start = timestamp("2026-09-03T00:00:00Z");
        let (start, end, _) = prose_event_time("Meet September 3 10:00 to 11:30.", message, 0)
            .expect("10:00 to 11:30.");
        assert_eq!(start, day_start + 10 * 3600);
        assert_eq!(end, day_start + 11 * 3600 + 1800);
    }

    /// A bare range (no colon, no meridiem on either side) joined by the
    /// word "to" is never trusted as a time -- "to" is far too common for
    /// two adjacent bare numbers to be good evidence ("5 to 7 people", "10
    /// to 12 attendees"). Each case still finds its date, and falls back to
    /// an all-day match rather than inventing a bogus time.
    #[test]
    fn prose_event_time_rejects_a_bare_to_joined_range_as_not_a_time() {
        let message = timestamp("2026-08-01T00:00:00Z");
        let day_start = timestamp("2026-09-03T00:00:00Z");
        let all_day_end = day_start + i64::from(DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT);
        for text in [
            "September 3, 5 to 7 people will attend.",
            "September 3, 10 to 12 attendees expected.",
        ] {
            let (start, end, _) =
                prose_event_time(text, message, 0).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(start, day_start, "{text}");
            assert_eq!(end, all_day_end, "{text}");
        }
    }

    /// A bare range is still trusted with a dash separator (a much rarer,
    /// stronger signal than "to") between two hours of 1..=12, or with a
    /// meridiem on either side regardless of separator.
    #[test]
    fn prose_event_time_accepts_a_plausible_bare_dash_range() {
        let message = timestamp("2026-08-01T00:00:00Z");
        let day_start = timestamp("2026-09-03T00:00:00Z");
        for (text, start_seconds, end_seconds) in [
            ("Meet September 3 11\u{2013}1 sharp.", 11 * 3600, 13 * 3600),
            ("Meet September 3 2-5pm sharp.", 14 * 3600, 17 * 3600),
        ] {
            let (start, end, _) =
                prose_event_time(text, message, 0).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(start, day_start + start_seconds, "{text}");
            assert_eq!(end, day_start + end_seconds, "{text}");
        }
    }

    /// Pins down the token count `parse_prose_time` reports consumed for
    /// each range form, including the extra token absorbed for a spaced
    /// trailing meridiem.
    #[test]
    fn parse_prose_time_reports_tokens_consumed_for_each_range_form() {
        let cases: &[(&str, usize)] = &[
            ("2:00-5:30pm", 1),
            ("2:00 to 5:30pm", 3),
            ("2:00 - 5:30pm", 3),
            ("2:00\u{2013}5:30pm", 2),
            ("2:00\u{2013}5:30 pm", 3),
        ];
        for (text, expected_consumed) in cases {
            let tokens = prose_tokens(text);
            let (_, _, consumed) =
                parse_prose_time(text, &tokens, 0).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(consumed, *expected_consumed, "{text}");
        }
    }

    /// A preposition ("at", "from", "starting") -- or a comma or "@", which
    /// `prose_tokens` treats as pure separators and so never even reach
    /// this far as their own token -- may sit between the date and its
    /// trailing time without blocking it from being read.
    #[test]
    fn prose_event_time_reads_a_time_after_one_leading_preposition() {
        let message = timestamp("2026-08-01T00:00:00Z");
        let day_start = timestamp("2026-09-03T00:00:00Z");
        for (text, seconds_since_midnight) in [
            ("Let's meet on September 3 at 2 pm sharp.", 14 * 3600),
            ("Let's meet on September 3 at 14:00 sharp.", 14 * 3600),
            ("Let's meet on Sept 3 at 2:30 pm sharp.", 14 * 3600 + 1800),
        ] {
            let (start, end, _) =
                prose_event_time(text, message, 0).unwrap_or_else(|| panic!("{text}"));
            let expected_start = day_start + seconds_since_midnight;
            assert_eq!(start, expected_start, "{text}");
            assert_eq!(end, start + 3600, "{text}");
        }
    }

    #[test]
    fn prose_event_time_rolls_the_default_year_forward_past_the_sixty_day_window() {
        // A message in December naming "September 3" with no year means
        // NEXT year's September 3rd -- this year's already passed by more
        // than 60 days.
        let message = timestamp("2026-12-01T00:00:00Z");
        let (start, _, _) = prose_event_time("See you September 3.", message, 0).unwrap();
        assert_eq!(start, timestamp("2027-09-03T00:00:00Z"));
    }

    #[test]
    fn prose_event_time_never_rolls_an_explicit_year() {
        // Even though "September 3, 2026" is more than 60 days before this
        // December message, a STATED year is trusted as-is, never rolled.
        let message = timestamp("2026-12-01T00:00:00Z");
        let (start, _, _) = prose_event_time("See you September 3, 2026.", message, 0).unwrap();
        assert_eq!(start, timestamp("2026-09-03T00:00:00Z"));
    }

    #[test]
    fn prose_event_time_finds_nothing_in_ordinary_prose() {
        assert!(
            prose_event_time(
                "Let's catch up soon, no rush.",
                timestamp("2026-08-01T00:00:00Z"),
                0
            )
            .is_none()
        );
    }

    /// Minor-fix regression: a bare number preceded by "Room" (or "ext",
    /// "suite", "unit", "no", "#") is never read as a day-then-month date
    /// -- "Room 12 August notes" must learn nothing, since "12" here names
    /// a room, not the 12th. An ordinary day-then-month date with no such
    /// preceder ("3 September 2026") still parses.
    #[test]
    fn day_then_month_excludes_room_and_similar_preceders() {
        let message = timestamp("2026-08-01T00:00:00Z");
        assert!(prose_event_time("Room 12 August notes", message, 0).is_none());
        let (start, _, _) = prose_event_time("Meet 3 September 2026 to review.", message, 0)
            .expect("day-then-month with no preceder still parses");
        assert_eq!(start, timestamp("2026-09-03T00:00:00Z"));
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
        assert_eq!(index[0].source, EventSource::Meeting);
    }

    #[test]
    fn event_index_keeps_the_latest_of_two_instances_of_the_same_named_event() {
        let mut aug = synthetic("Save the date.", 0, "a");
        aug.subject = "Invitation: Design workshop @ Fri Aug 21, 2026 11am - 12pm (UTC)".into();
        let aug_message = prepare(&aug, "Inbox", 0).unwrap();
        let mut sep = synthetic("Rescheduled, see below.", 1, "a");
        sep.subject =
            "Updated invitation: Design workshop @ Tue Sep 15, 2026 11am - 12pm (UTC)".into();
        let sep_message = prepare(&sep, "Inbox", 1).unwrap();
        let index = build_event_index(&[aug_message, sep_message]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].start, timestamp("2026-09-15T11:00:00Z"));
        assert_eq!(index[0].source, EventSource::Subject);
    }

    #[test]
    fn event_index_keeps_equal_names_from_different_conversations_scoped() {
        let mut mail = synthetic("Save the date.", 0, "a");
        mail.subject = "Invitation: Design workshop @ Fri Aug 21, 2026 11am - 12pm (UTC)".into();
        let first = prepare(&mail, "Inbox", 0).unwrap();
        let mut second = first.clone();
        second.conversation = "b".into();
        second.input.handle = "m1".into();

        let index = build_event_index(&[first, second]);
        assert_eq!(index.len(), 2);
        assert_eq!(
            index
                .iter()
                .map(|event| event.conversation.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
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

    /// The live finding this task fixes: the event is named by a proper
    /// name in the subject, with its date appended in prose ("- Sept 3")
    /// rather than structured as a calendar invite or Graph meeting
    /// message, so neither of the first two sources in
    /// `build_event_index`'s priority order finds anything. "Workshop" is
    /// an event noun, so the subject-prose learner's gate accepts it.
    #[test]
    fn event_index_learns_a_date_from_subject_prose_when_no_invite_exists() {
        let mut mail = synthetic("Let's finalize the agenda.", 0, "a");
        mail.subject = "Spring Estate Planning Workshop - Sept 3".into();
        // `start`/`end` are not asserted here: `build_event_index` resolves
        // prose against the message's own local offset
        // (`local_offset_seconds`, machine-dependent), and the exact
        // arithmetic is already pinned down offset-independently by
        // `prose_event_time`'s own tests above.
        let message = prepare(&mail, "Inbox", 0).unwrap();
        let index = build_event_index(&[message]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "spring estate planning workshop");
        assert_eq!(index[0].source, EventSource::SubjectProse);
    }

    #[test]
    fn subject_prose_event_time_looks_up_the_offset_at_the_event_instant() {
        let message = timestamp("2026-01-15T12:00:00Z");
        let provisional_event = timestamp("2026-07-01T22:00:00Z");
        let lookups = std::cell::RefCell::new(Vec::new());
        let (start, _, _) = subject_prose_event_time_with(
            "Product Launch - July 1, 2026 2pm",
            message,
            |instant| {
                lookups.borrow_mut().push(instant);
                if instant == message {
                    -8 * 3600
                } else {
                    -7 * 3600
                }
            },
        )
        .unwrap();

        assert_eq!(*lookups.borrow(), vec![message, provisional_event]);
        assert_eq!(start, timestamp("2026-07-01T21:00:00Z"));
    }

    /// The live finding this task fixes: the date and its time range sit
    /// inside a parenthesized group with a leading weekday and comma, and
    /// the subject itself opens with a possessive "Your ". The whole
    /// parenthesized group -- not just the date onward -- is dropped from
    /// the learned name, and the leading possessive is stripped too.
    #[test]
    fn event_index_strips_a_parenthesized_date_and_a_leading_possessive_from_the_name() {
        const SUBJECT: &str =
            "Your Spring Planning Workshop checklist (Tuesday, August 25 \u{2013} 2:00-5:30pm)";
        let mut mail = synthetic("Let's finalize the agenda.", 0, "a");
        mail.subject = SUBJECT.into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        let index = build_event_index(&[message]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "spring planning workshop checklist");
        assert_eq!(index[0].source, EventSource::SubjectProse);

        // The exact time, pinned down offset-independently the same way
        // `prose_event_time_reads_a_time_range` does -- `build_event_index`
        // itself resolves against the message's own machine-local offset.
        let (start, end, _) =
            prose_event_time(SUBJECT, timestamp("2026-08-01T00:00:00Z"), 0).unwrap();
        assert_eq!(start, timestamp("2026-08-25T14:00:00Z"));
        assert_eq!(end, timestamp("2026-08-25T17:30:00Z"));
    }

    /// The learned name strips the leading possessive and the whole date
    /// parenthetical, but the model's own `event` anchor -- which keeps the
    /// possessive and adds unrelated words -- still finds it: `match_event`
    /// drops stop words ("your") from both sides before comparing tokens.
    #[test]
    fn match_event_finds_a_subject_prose_event_despite_the_stripped_possessive() {
        let mut mail = synthetic("Let's finalize the agenda.", 0, "a");
        mail.subject =
            "Your Spring Planning Workshop checklist (Tuesday, August 25 \u{2013} 2:00-5:30pm)"
                .into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        let index = build_event_index(&[message]);
        let evidence = timestamp("2026-08-01T00:00:00Z");
        let matched = match_event(
            "your Spring Planning Workshop in San Francisco",
            evidence,
            &index,
        )
        .unwrap();
        assert_eq!(matched.name, "spring planning workshop checklist");
    }

    /// `strip_leading_possessive` applies to the Meeting and Subject name
    /// branches too, not just `SubjectProse`: a Graph meeting-metadata entry
    /// named "Your Team Sync" dedupes against a calendar-invite-subject
    /// entry for the same meeting named plainly "Team Sync", rather than
    /// standing as two separate learned events.
    #[test]
    fn learn_one_event_strips_a_leading_possessive_from_meeting_and_subject_names_too() {
        let mut meeting_mail = synthetic("", 0, "a");
        meeting_mail.subject = "Your Team Sync".into();
        meeting_mail.event = Some(openloops_graph::live::review::MailEvent {
            start: "2026-08-21T11:00:00Z".into(),
            end: "2026-08-21T12:00:00Z".into(),
            out_of_date: false,
        });
        let meeting_message = prepare(&meeting_mail, "Inbox", 0).unwrap();

        let mut subject_mail = synthetic("", 1, "a");
        subject_mail.subject = "Invitation: Team Sync @ Fri Aug 28, 2026 11am - 12pm (UTC)".into();
        let subject_message = prepare(&subject_mail, "Inbox", 1).unwrap();

        let index = build_event_index(&[meeting_message, subject_message]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "team sync");
        assert_eq!(index[0].source, EventSource::Subject);
        assert_eq!(index[0].start, timestamp("2026-08-28T11:00:00Z"));
    }

    /// A stray, unmatched trailing `)` immediately before the date (not
    /// part of any parenthesized group `subject_prose_name_end` finds still
    /// open) is trimmed from the learned name too, alongside the existing
    /// "-@:," set -- so a malformed subject never leaves a dangling
    /// unbalanced paren in the learned name.
    #[test]
    fn event_index_trims_a_stray_unmatched_closing_paren_from_the_name() {
        let mut mail = synthetic("Let's finalize the agenda.", 0, "a");
        mail.subject = "Design Workshop) Sept 3".into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        let index = build_event_index(&[message]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "design workshop");
    }

    /// Companion to the above: a trailing `)` that closes a REAL group
    /// ("(draft)", balanced against its own '(') must survive the trim --
    /// only a genuinely unmatched close is stray. `normalize_subject`'s own
    /// trailing-paren handling then decides to keep it (no digit, not a
    /// timezone abbreviation).
    #[test]
    fn event_index_keeps_a_matched_trailing_paren_group_in_the_name() {
        let mut mail = synthetic("Let's finalize the agenda.", 0, "a");
        mail.subject = "Planning Workshop (draft) September 3 2pm".into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        let index = build_event_index(&[message]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "planning workshop (draft)");
    }

    /// Critical review finding: the subject-prose learner previously
    /// turned ANY "<words> <date>" subject into an event, with no
    /// requirement that it actually name a gathering. "Draft Agreement"
    /// contains no [`PROSE_EVENT_NOUNS`] entry, so nothing is learned --
    /// and, with no body blocks either, the whole message contributes
    /// nothing to the index.
    #[test]
    fn subject_prose_without_an_event_noun_learns_nothing() {
        let mut mail = synthetic("", 0, "a");
        mail.subject = "Draft Agreement - September 3".into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        assert!(build_event_index(&[message]).is_empty());
    }

    /// Companion to the above at the closure level: since the "Draft
    /// Agreement" subject never enters the index, a real, unrelated
    /// obligation whose action text happens to share those same words
    /// ("Send the draft agreement to Alex") must not be closed as "event
    /// passed".
    #[test]
    fn close_passed_events_does_not_close_a_request_via_a_rejected_subject_prose_candidate() {
        let mut event_mail = synthetic("", 1, "event");
        event_mail.subject = "Draft Agreement - September 3".into();
        let event_message = prepare(&event_mail, "Inbox", 1).unwrap();
        let (mut messages, mut item) = closure_test_messages();
        messages[0].input.timestamp = timestamp("2026-08-01T00:00:00Z");
        item.action = "Send the draft agreement to Alex".into();
        messages.push(event_message);
        let mut result = ScanResult {
            analysis: LoopItems {
                items: vec![item],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            failed_conversations_detail: vec![],
            analyzed: 1,
            total: 1,
            cancelled: false,
            conversation_notes: vec![],
            conversation_quality: BTreeMap::new(),
            conversation_notes_by_id: BTreeMap::new(),
            conversation_rejection_reasons: BTreeMap::new(),
            suggested_updates: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            conversation_count: 1,
            analyzed_conversations: 1,
            failed_conversations: 0,
            not_started_conversations: 0,
            closure_pass_failure: None,
        };
        close_passed_events(&mut result, &messages, timestamp("2026-10-01T00:00:00Z"));
        assert!(result.analysis.items[0].event_passed.is_none());
        assert_eq!(result.event_closures, 0);
    }

    /// A proper-named event still closes once passed via its `event`
    /// anchor even though it was only ever learned from subject prose
    /// (never a structured invite): the noun gate lets a genuine event
    /// through, and `match_event` (unlike `match_event_by_text`) is not
    /// restricted to structured sources.
    #[test]
    fn close_passed_events_closes_a_subject_prose_named_event_via_the_event_anchor() {
        let mut event_mail = synthetic("", 1, "event");
        event_mail.subject = "Spring Estate Planning Workshop - Aug 21, 2026".into();
        let event_message = prepare(&event_mail, "Inbox", 1).unwrap();
        let (mut messages, mut item) = closure_test_messages();
        messages[0].input.timestamp = timestamp("2026-08-01T00:00:00Z");
        item.event = Some(Anchor {
            message: item.evidence.message.clone(),
            block: 0,
            quote: "the Spring Estate Planning Workshop".into(),
            context: String::new(),
        });
        messages.push(event_message);
        let mut result = ScanResult {
            analysis: LoopItems {
                items: vec![item],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            failed_conversations_detail: vec![],
            analyzed: 1,
            total: 1,
            cancelled: false,
            conversation_notes: vec![],
            conversation_quality: BTreeMap::new(),
            conversation_notes_by_id: BTreeMap::new(),
            conversation_rejection_reasons: BTreeMap::new(),
            suggested_updates: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            conversation_count: 1,
            analyzed_conversations: 1,
            failed_conversations: 0,
            not_started_conversations: 0,
            closure_pass_failure: None,
        };
        // Well clear of the machine's own local offset either way.
        close_passed_events(&mut result, &messages, timestamp("2026-08-25T00:00:00Z"));
        let passed = result.analysis.items[0].event_passed.as_ref().unwrap();
        assert_eq!(passed.name, "spring estate planning workshop");
        assert_eq!(result.event_closures, 1);
        assert!(
            passed.from_subject,
            "a SubjectProse-sourced closure came from the subject line"
        );
    }

    /// `match_event_by_text` must never match a prose-sourced event: text
    /// matching is restricted to structured (Meeting/Subject) evidence, so
    /// a subject-prose or body-prose entry can never be silently confirmed
    /// by an unrelated request's own wording.
    #[test]
    fn match_event_by_text_never_matches_a_prose_sourced_event() {
        let evidence = timestamp("2026-08-01T00:00:00Z");
        for source in [EventSource::SubjectProse, EventSource::Prose] {
            let index = vec![EventRef {
                name: "draft agreement".into(),
                start: timestamp("2026-09-03T00:00:00Z"),
                end: timestamp("2026-09-03T17:00:00Z"),
                conversation: "c0".into(),
                message_handle: "m0".into(),
                source,
            }];
            assert!(
                match_event_by_text("Send the draft agreement to Alex", evidence, &index).is_none(),
                "{source:?}"
            );
        }
    }

    /// The event's date is stated only in a body paragraph, next to a
    /// capitalized multi-word phrase ending in a recognized event noun --
    /// the subject itself names none of it, so the name must come from that
    /// nearby phrase rather than the (unrelated) subject.
    #[test]
    fn event_index_learns_a_date_from_body_prose_near_a_capitalized_event_phrase() {
        let mut mail = synthetic(
            "Quick update: the Johnson Hearing is now set for October 12, 2026 \
at the downtown courthouse. Let me know if that works.",
            0,
            "a",
        );
        mail.subject = "Re: scheduling".into();
        // `start` is not asserted for the same reason as the subject-prose
        // test above: it depends on the machine's own local offset.
        let message = prepare(&mail, "Inbox", 0).unwrap();
        let index = build_event_index(&[message]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "johnson hearing");
        assert_eq!(index[0].source, EventSource::Prose);
    }

    /// Critical review finding: a block with more than one date must pair
    /// the event phrase with the NEAREST date, not the first one in the
    /// block -- "please RSVP by September 1" is an unrelated, earlier
    /// date; "the Spring Workshop on September 15" is the phrase's real
    /// date.
    #[test]
    fn event_index_pairs_a_body_prose_event_phrase_with_the_nearest_date_not_the_first() {
        let mut mail = synthetic(
            "Hi team - please RSVP by September 1 for the Spring Workshop on September 15.",
            0,
            "a",
        );
        mail.subject = "Re: scheduling".into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        let index = build_event_index(&[message]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "spring workshop");
        let local_date = chrono::DateTime::from_timestamp(index[0].start, 0)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string();
        assert_eq!(local_date, "2026-09-15");
    }

    /// Important-fix regression: a rejected (too-short) candidate must fall
    /// through to the next candidate/block rather than aborting the whole
    /// message. The message's one-word subject ("Fee") appears verbatim in
    /// the first body block, which would otherwise be named "fee" (1 word,
    /// rejected); the fix must instead try the second block, which names a
    /// real, specific event.
    #[test]
    fn learn_one_event_falls_through_a_rejected_one_word_subject_match_to_a_later_block() {
        let mut mail = synthetic("placeholder", 0, "a");
        mail.subject = "Fee".into();
        let mut message = prepare(&mail, "Inbox", 0).unwrap();
        message.input.message.body_blocks = vec![
            CanonicalBlock::new("The Fee is due September 1.").unwrap(),
            CanonicalBlock::new("Update: the Johnson Hearing is now set for September 9.").unwrap(),
        ];
        let index = build_event_index(&[message]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "johnson hearing");
        assert_eq!(index[0].source, EventSource::Prose);
    }

    /// Minor-fix regression: a capitalized run must never cross a sentence
    /// boundary. Without the period-break fix, "Notes. Onboarding
    /// Training" would be read as one continuous capitalized run (and
    /// still match, since it also ends in "training"), silently absorbing
    /// the unrelated preceding sentence into the learned name.
    #[test]
    fn capitalized_event_phrase_breaks_at_a_token_ending_in_a_period() {
        let mut mail = synthetic("Notes. Onboarding Training on 2026-09-03", 0, "a");
        mail.subject = "Re: scheduling".into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        let index = build_event_index(&[message]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "onboarding training");
    }

    #[test]
    fn capitalized_event_phrase_keeps_a_sentence_final_event_noun() {
        let mut mail = synthetic(
            "Please prepare for the Johnson Hearing. It is on 2026-10-12.",
            0,
            "a",
        );
        mail.subject = "Re: prep".into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        let index = build_event_index(&[message]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "johnson hearing");
    }

    #[test]
    fn nearest_phrase_for_date_finds_the_closest_matching_run() {
        let text = "the Design Workshop is set for September 3, plans pending.";
        let phrases = capitalized_event_phrases(text);
        let date_offset = text.find("September").unwrap();
        let (phrase, _, date_follows) = nearest_phrase_for_date(&phrases, date_offset).unwrap();
        assert_eq!(phrase, "design workshop");
        assert!(date_follows);
        assert!(
            nearest_phrase_for_date(&capitalized_event_phrases("no capitals here"), 0).is_none()
        );
    }

    /// A body date near the message's own (multi-word) normalized subject
    /// is named after that subject rather than hunting for a capitalized
    /// phrase -- the strongest available evidence for what the date belongs
    /// to.
    #[test]
    fn event_index_prefers_the_message_subject_as_the_body_prose_name() {
        let mut mail = synthetic(
            "Reminder: the Spring Budget Summit is confirmed for November 5, 2026.",
            0,
            "a",
        );
        mail.subject = "Spring Budget Summit".into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        let index = build_event_index(&[message]);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "spring budget summit");
        assert_eq!(index[0].source, EventSource::Prose);
    }

    #[test]
    fn match_event_rejects_a_phrase_with_no_meaningful_tokens() {
        let index = vec![EventRef {
            name: "design workshop".into(),
            start: timestamp("2026-08-20T00:00:00Z"),
            end: timestamp("2026-08-20T01:00:00Z"),
            conversation: "c0".into(),
            message_handle: "m0".into(),
            source: EventSource::Subject,
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
            conversation: "c0".into(),
            message_handle: "m0".into(),
            source: EventSource::Subject,
        }];
        assert_eq!(
            match_event("before the design workshop", evidence, &workshop_only)
                .unwrap()
                .message_handle,
            "m0"
        );
        let standup_only = vec![EventRef {
            name: "weekly standup call".into(),
            start: timestamp("2026-08-20T00:00:00Z"),
            end: timestamp("2026-08-20T01:00:00Z"),
            conversation: "c0".into(),
            message_handle: "m1".into(),
            source: EventSource::Subject,
        }];
        assert!(match_event("before the design workshop", evidence, &standup_only).is_none());
    }

    #[test]
    fn match_event_drops_a_generic_event_noun_from_both_sides() {
        let evidence = timestamp("2026-08-01T00:00:00Z");
        let index = vec![EventRef {
            name: "hearing motion to compel".into(),
            start: timestamp("2026-08-20T00:00:00Z"),
            end: timestamp("2026-08-20T01:00:00Z"),
            conversation: "c0".into(),
            message_handle: "m0".into(),
            source: EventSource::Subject,
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
                conversation: "c0".into(),
                message_handle: "m0".into(),
                source: EventSource::Subject,
            },
            EventRef {
                name: "design workshop follow up".into(),
                start: timestamp("2026-08-10T00:00:00Z"),
                end: timestamp("2026-08-10T01:00:00Z"),
                conversation: "c0".into(),
                message_handle: "m1".into(),
                source: EventSource::Subject,
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
        let progress = ScanProgress::default();
        let result = scan_conversations(&[b, a, c], &progress, |messages| {
            lengths.push(messages.len());
            assert!(
                messages
                    .windows(2)
                    .all(|w| w[0].timestamp <= w[1].timestamp)
            );
            Ok(LoopItems {
                items: vec![],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            })
        });
        // Smallest conversation first: the 1-message "b" conversation
        // analyzes before the 2-message "a" one (see `conversations_by_size`).
        assert_eq!(lengths, [1, 2]);
        assert_eq!(result.analyzed, 3);
        assert_eq!(progress.conversation_total.load(Ordering::Relaxed), 2);
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
    fn failure_line_names_the_conversation_by_its_subject_snippet() {
        let m = prepare(&synthetic("Please send the draft.", 0, "a"), "Inbox", 0).unwrap();
        let result = scan_conversations(&[m], &ScanProgress::default(), |_| {
            Err(ProviderError::InvalidJson)
        });
        assert_eq!(result.failures.len(), 1);
        assert!(
            result.failures[0].contains("subject: Synthetic budget conversation"),
            "failure line: {}",
            result.failures[0]
        );
    }

    #[test]
    fn timeout_failure_line_appends_the_content_free_message_count() {
        let first = prepare(
            &synthetic("Please send the synthetic draft.", 0, "a"),
            "Inbox",
            0,
        )
        .unwrap();
        let second = prepare(
            &synthetic("The synthetic draft is ready.", 1, "a"),
            "Sent",
            1,
        )
        .unwrap();
        let line = failure_line(0, &[&first, &second], ProviderError::Timeout);
        assert!(line.contains(&ProviderError::Timeout.to_string()));
        assert!(line.ends_with("Request size: 2 messages."));
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
    /// One conversation per subject, sized so `conversations_by_size` keeps
    /// them in the order they are built (every conversation is a single
    /// message, and the grouping key is the conversation name).
    fn parallel_corpus(count: usize) -> Vec<ReviewMessage> {
        (0..count)
            .map(|index| {
                // synthetic() only spells single-digit days; the id and the
                // conversation name carry the index instead.
                let mut item =
                    synthetic("Please send the draft.", index % 9, &format!("c{index:02}"));
                item.id = format!("synthetic-{index}");
                prepare(&item, "Inbox", index).unwrap()
            })
            .collect()
    }

    /// Maps each corpus message's handle to its position, so a fake
    /// provider can answer deterministically per conversation without
    /// depending on how a handle is spelled.
    fn handle_positions(messages: &[ReviewMessage]) -> BTreeMap<String, usize> {
        messages
            .iter()
            .enumerate()
            .map(|(index, m)| (m.input.handle.clone(), index))
            .collect()
    }

    fn no_loop_items() -> LoopItems {
        LoopItems {
            items: vec![],
            rejected: 0,
            rejection_reasons: vec![],
            degraded: 0,
        }
    }

    /// One expectation naming `handle`, so a merged `analysis.items` list
    /// records exactly which conversations contributed and in what order.
    fn expectation_for(handle: &str) -> LoopItem {
        LoopItem {
            action: "Send the draft".into(),
            action_phrase: "send the draft".into(),
            owner: Owner::You,
            waiting_party: "Other <sam@example.invalid>".into(),
            kind: "request".into(),
            evidence: Anchor {
                message: handle.to_owned(),
                block: 0,
                quote: "Please send the draft.".into(),
                context: "Please send the draft.".into(),
            },
            deadline: None,
            deadline_kind_hint: None,
            event: None,
            event_time: None,
            resolution: None,
            resolution_kind: None,
            uncertainty: String::new(),
            unverified_deadline: false,
            unverified_resolution: false,
            cross_thread: false,
            event_passed: None,
            suggested_update: None,
            from_call_summary: false,
            meeting_time: None,
            meeting_time_approx: false,
        }
    }

    /// Tracks how many jobs a pass ever had running at the same time.
    #[derive(Default)]
    struct Concurrency {
        running: AtomicUsize,
        peak: AtomicUsize,
    }

    impl Concurrency {
        fn enter(&self) {
            let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(running, Ordering::SeqCst);
        }

        fn leave(&self) {
            self.running.fetch_sub(1, Ordering::SeqCst);
        }

        fn peak(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn conversations_run_in_parallel_up_to_the_clients_reported_ceiling() {
        // Eight conversations, each held for one delay, across four
        // workers: two sequential rounds rather than eight.
        const DELAY: Duration = Duration::from_millis(120);
        let messages = parallel_corpus(8);
        let seen = Concurrency::default();
        let progress = ScanProgress::default();
        let started = Instant::now();
        let result =
            super::scan_conversations(&messages, &progress, &ParallelPass::new(4), &|_| {
                seen.enter();
                std::thread::sleep(DELAY);
                seen.leave();
                Ok(no_loop_items())
            });
        let elapsed = started.elapsed();
        assert_eq!(result.analyzed, 8);
        assert_eq!(seen.peak(), 4, "four workers must run at once");
        assert!(
            elapsed < Duration::from_millis(1500) + DELAY * 5,
            "eight 120ms conversations across four workers took {elapsed:?}"
        );
        assert_eq!(progress.processed.load(Ordering::Relaxed), 8);
        assert_eq!(progress.snapshot(), ProgressSnapshot::default());
    }

    #[test]
    fn a_parallel_scan_produces_exactly_the_sequential_result() {
        let messages = parallel_corpus(12);
        // Deterministic and content-derived: the answer depends only on the
        // conversation, never on which worker ran it or when.
        let positions = handle_positions(&messages);
        let analyze = |conversation: &[ConversationMessage]| {
            let handle = conversation[0].handle.clone();
            let position = positions[&handle];
            if position == 3 {
                return Err(ProviderError::InvalidJson);
            }
            Ok(LoopItems {
                items: vec![expectation_for(&handle)],
                rejected: usize::from(position == 5),
                rejection_reasons: vec![],
                degraded: 0,
            })
        };
        let sequential = super::scan_conversations(
            &messages,
            &ScanProgress::default(),
            &ParallelPass::new(1),
            &analyze,
        );
        let parallel = super::scan_conversations(
            &messages,
            &ScanProgress::default(),
            &ParallelPass::new(8),
            &analyze,
        );
        let handles = |result: &ScanResult| -> Vec<String> {
            result
                .analysis
                .items
                .iter()
                .map(|item| item.evidence.message.clone())
                .collect()
        };
        assert_eq!(handles(&parallel), handles(&sequential));
        assert_eq!(parallel.failures, sequential.failures);
        assert_eq!(parallel.conversation_notes, sequential.conversation_notes);
        assert_eq!(parallel.analyzed, sequential.analyzed);
        assert_eq!(parallel.analysis.rejected, sequential.analysis.rejected);
        assert_eq!(parallel.analysis.items.len(), 11);
        assert_eq!(parallel.failures.len(), 1);
    }

    #[test]
    fn a_rate_limit_halves_concurrency_and_the_request_is_never_resent() {
        let messages = parallel_corpus(24);
        let positions = handle_positions(&messages);
        let calls: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());
        let seen = Concurrency::default();
        let pass = ParallelPass::new(8);
        let result = super::scan_conversations(&messages, &ScanProgress::default(), &pass, &|c| {
            *calls
                .lock()
                .unwrap()
                .entry(c[0].handle.clone())
                .or_default() += 1;
            seen.enter();
            std::thread::sleep(Duration::from_millis(20));
            seen.leave();
            if positions[&c[0].handle] % 12 == 1 {
                return Err(ProviderError::RateLimited);
            }
            Ok(no_loop_items())
        });
        let calls = calls.into_inner().unwrap();
        assert_eq!(calls.len(), 24);
        assert!(
            calls.values().all(|count| *count == 1),
            "a rate-limited request is never resent: {calls:?}"
        );
        // Two of the 24 conversations rate-limit, so two fail and
        // the allowed concurrency halves twice (8 -> 4 -> 2) unless eight
        // straight successes widened it back first; either way it narrowed.
        assert_eq!(result.failures.len(), 2);
        assert!(
            result
                .failures
                .iter()
                .all(|line| line
                    .ends_with("The provider rate-limited this request; it was not resent.")),
            "{:?}",
            result.failures
        );
        assert!(!result.primary_scan_transport_error);
        assert!(!result.cancelled);
        assert_eq!(result.analyzed, 22);
        let allowed = pass.limiter_state().0;
        assert!(
            (1..=8).contains(&allowed),
            "allowed concurrency stays inside 1..=max: {allowed}"
        );
    }

    #[test]
    fn plain_text_bodies_become_one_block_per_paragraph_and_never_drop_text() {
        let blocks =
            super::plain_body_blocks("Hi there,\r\n\r\nFriday at 11:30 works.\r\n\r\n\r\nThanks");
        assert_eq!(
            blocks,
            vec!["Hi there,", "Friday at 11:30 works.", "Thanks"]
        );
        assert!(super::plain_body_blocks("  \n\n  ").is_empty());
        let many: Vec<String> = (0..50).map(|i| format!("Paragraph {i}")).collect();
        let blocks = super::plain_body_blocks(&many.join("\n\n"));
        assert_eq!(blocks.len(), super::MAX_BODY_BLOCKS);
        assert!(blocks.last().unwrap().contains("Paragraph 49"));
        assert!(blocks.last().unwrap().contains("Paragraph 39"));
    }

    #[test]
    fn html_body_paragraphs_and_breaks_become_ordered_blocks() {
        let mut item = synthetic(
            "<p>Hello team,</p><p>Thanks for reviewing.</p><p>Please send the synthetic report.</p><div>First line.<br><br>Second line.</div>",
            0,
            "html-paragraphs",
        );
        item.body_is_html = true;
        let prepared = prepare(&item, "Inbox", 0).unwrap();
        let blocks: Vec<String> = prepared
            .input
            .message
            .body_blocks
            .iter()
            .map(CanonicalBlock::as_string)
            .collect();

        assert_eq!(
            blocks,
            [
                "Hello team,",
                "Thanks for reviewing.",
                "Please send the synthetic report.",
                "First line.",
                "Second line."
            ]
        );
    }

    #[test]
    fn html_quote_blocks_are_not_paragraph_split() {
        let html = "<p>Current note.</p><blockquote><p>Quoted first.</p><p>Quoted second.</p></blockquote>";
        let walked = canonicalize_html(html).unwrap();
        let mut item = synthetic(html, 0, "html-quotes");
        item.body_is_html = true;
        let prepared = prepare(&item, "Inbox", 0).unwrap();
        let quote_blocks: Vec<String> = prepared
            .input
            .message
            .quote_blocks
            .iter()
            .map(CanonicalBlock::as_string)
            .collect();

        assert_eq!(quote_blocks, walked.quote_blocks);
    }

    #[test]
    fn html_body_overflow_is_folded_into_the_last_message_block() {
        use std::fmt::Write as _;

        let mut html = String::new();
        for index in 0..50 {
            write!(html, "<p>Paragraph {index}</p>").unwrap();
        }
        let mut item = synthetic(&html, 0, "html-overflow");
        item.body_is_html = true;
        let prepared = prepare(&item, "Inbox", 0).unwrap();
        let blocks = &prepared.input.message.body_blocks;

        assert_eq!(blocks.len(), super::MAX_BODY_BLOCKS);
        let last = blocks.last().unwrap().as_string();
        assert!(last.contains("Paragraph 39"));
        assert!(last.contains("Paragraph 49"));

        let first_source = (0..25)
            .map(|index| format!("Source paragraph {index}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let second_source = (25..50)
            .map(|index| format!("Source paragraph {index}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let blocks = super::paragraph_body_blocks([first_source.as_str(), second_source.as_str()]);
        assert_eq!(blocks.len(), super::MAX_BODY_BLOCKS);
        assert!(blocks.last().unwrap().contains("Source paragraph 39"));
        assert!(blocks.last().unwrap().contains("Source paragraph 49"));
    }

    /// A slow answer on one large conversation is that conversation's
    /// failure, not a provider outage: the scan neither stops dispatching
    /// nor skips the closure pass because of it.
    #[test]
    fn a_timeout_fails_only_its_conversation_and_keeps_the_closure_pass() {
        assert!(!super::is_transport_error(ProviderError::Timeout));
        assert!(!super::is_stop_error(ProviderError::Timeout));
        assert_eq!(
            super::job_signal::<()>(&Err(ProviderError::Timeout)),
            JobSignal::Ok
        );
    }

    /// `OpenRouter`'s "would exceed your available credits given your current
    /// in-flight requests" 402 is a too-many-at-once condition, so it steers
    /// the limiter exactly like a 429: the scan narrows and continues, and
    /// the request is not resent. A plain quota 402 stays per-conversation.
    #[test]
    fn an_in_flight_credit_402_narrows_concurrency_like_a_rate_limit() {
        assert_eq!(
            super::job_signal::<()>(&Err(ProviderError::CreditsInFlight)),
            JobSignal::Backoff
        );
        assert_eq!(
            super::job_signal::<()>(&Err(ProviderError::Quota)),
            JobSignal::Ok
        );
        assert!(!super::is_transport_error(ProviderError::CreditsInFlight));

        let messages = parallel_corpus(24);
        let positions = handle_positions(&messages);
        let calls: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());
        let pass = ParallelPass::new(8);
        let result = super::scan_conversations(&messages, &ScanProgress::default(), &pass, &|c| {
            *calls
                .lock()
                .unwrap()
                .entry(c[0].handle.clone())
                .or_default() += 1;
            std::thread::sleep(Duration::from_millis(20));
            if positions[&c[0].handle] % 12 == 1 {
                return Err(ProviderError::CreditsInFlight);
            }
            Ok(no_loop_items())
        });
        let calls = calls.into_inner().unwrap();
        assert_eq!(calls.len(), 24, "every conversation is still attempted");
        assert!(
            calls.values().all(|count| *count == 1),
            "a deferred request is never resent: {calls:?}"
        );
        assert_eq!(result.failures.len(), 2);
        assert!(
            result.failures.iter().all(|line| {
                line.contains("requests already in flight")
                    && line.ends_with("This request was not resent.")
            }),
            "{:?}",
            result.failures
        );
        assert!(!result.primary_scan_transport_error);
        assert!(!result.cancelled);
        assert_eq!(result.analyzed, 22);
        let allowed = pass.limiter_state().0;
        assert!(
            (1..=8).contains(&allowed),
            "allowed concurrency stays inside 1..=max: {allowed}"
        );
    }

    #[test]
    fn halving_never_falls_below_one_worker_and_widening_never_passes_the_ceiling() {
        let pass = ParallelPass::new(8);
        for expected in [4, 2, 1, 1, 1] {
            pass.on_backoff();
            assert_eq!(pass.limiter_state().0, expected);
        }
        // Eight consecutive completed requests widen it, by at least one.
        for _ in 0..RAISE_AFTER_SUCCESSES {
            pass.on_success();
        }
        assert_eq!(pass.limiter_state().0, 2);
        for _ in 0..RAISE_AFTER_SUCCESSES * 40 {
            pass.on_success();
        }
        assert_eq!(pass.limiter_state().0, 8);
        // A rate limit resets the success streak, so the very next success
        // cannot widen what the backoff just narrowed.
        for _ in 0..RAISE_AFTER_SUCCESSES - 1 {
            pass.on_success();
        }
        pass.on_backoff();
        pass.on_success();
        assert_eq!(pass.limiter_state().0, 4);
    }

    #[test]
    fn concurrent_successes_widen_once_per_eight_and_backoff_resets_the_streak() {
        let pass = ParallelPass::new(8);
        for _ in 0..3 {
            pass.on_backoff();
        }
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..4 {
                        pass.on_success();
                    }
                });
            }
        });
        assert_eq!(pass.limiter_state(), (3, 0, 8));

        for _ in 0..RAISE_AFTER_SUCCESSES - 1 {
            pass.on_success();
        }
        pass.on_backoff();
        pass.on_success();
        assert_eq!(pass.limiter_state().1, 1);
    }

    #[test]
    fn cancelling_stops_dispatch_and_in_flight_requests_answer_cancelled() {
        let messages = parallel_corpus(20);
        let progress = ScanProgress::default();
        let started = AtomicUsize::new(0);
        let result =
            super::scan_conversations(&messages, &progress, &ParallelPass::new(4), &|_| {
                started.fetch_add(1, Ordering::SeqCst);
                progress.cancel.store(true, Ordering::SeqCst);
                // Every worker sees the flag the same way a real request
                // does: the request in flight is abandoned, not completed.
                Err(ProviderError::Cancelled)
            });
        let started = started.load(Ordering::SeqCst);
        assert!(
            started <= 4,
            "no job starts after the cancel flag is set: {started}"
        );
        assert!(result.cancelled);
        assert_eq!(
            result.failures.len(),
            result.failed_conversations_detail.len()
        );
        assert!(!result.primary_scan_transport_error);
        assert_eq!(progress.snapshot().in_flight, 0);
    }

    #[test]
    fn a_stop_flag_wakes_parked_workers_without_sending_more_requests() {
        let progress = ScanProgress::default();
        let pass = ParallelPass::new(4);
        pass.on_backoff();
        pass.on_backoff();
        let sent = AtomicUsize::new(0);
        let (first_started_tx, first_started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();

        std::thread::scope(|scope| {
            scope.spawn(|| {
                let release_rx = Mutex::new(release_rx);
                let processed_per_job = [1; 8];
                let _ = run_jobs(&processed_per_job, &pass, &progress, &|_| {
                    if sent.fetch_add(1, Ordering::SeqCst) == 0 {
                        first_started_tx.send(()).unwrap();
                        release_rx.lock().unwrap().recv().unwrap();
                    }
                    Ok::<_, ProviderError>(())
                });
                finished_tx.send(()).unwrap();
            });
            first_started_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            pass.stop.store(true, Ordering::SeqCst);
            pass.wake_workers();
            release_tx.send(()).unwrap();
            finished_rx
                .recv_timeout(Duration::from_millis(1500))
                .expect("the pool should observe stop within the 500 ms contract");
        });
        assert_eq!(sent.load(Ordering::SeqCst), 1);
        assert_eq!(progress.processed.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_transport_failure_stops_dispatch_while_in_flight_requests_finish() {
        let messages = parallel_corpus(20);
        let progress = ScanProgress::default();
        let started = AtomicUsize::new(0);
        let result =
            super::scan_conversations(&messages, &progress, &ParallelPass::new(4), &|_| {
                started.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
                Err(ProviderError::Network)
            });
        let started = started.load(Ordering::SeqCst);
        assert!(
            started <= 8,
            "dispatch stops once the provider is unreachable: {started}"
        );
        assert!(result.primary_scan_transport_error);
        assert_eq!(result.failures.len(), 20);
        assert_eq!(result.failed_conversations, started);
        assert_eq!(result.not_started_conversations, 20 - started);
        assert!(!result.cancelled);
        assert_eq!(progress.snapshot().in_flight, 0);
    }

    #[test]
    fn quota_is_a_per_conversation_failure_and_does_not_stop_dispatch() {
        let messages = parallel_corpus(8);
        let progress = ScanProgress::default();
        let calls = AtomicUsize::new(0);
        let result =
            super::scan_conversations(&messages, &progress, &ParallelPass::new(4), &|_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::Quota)
            });
        assert_eq!(
            calls.load(Ordering::SeqCst),
            8,
            "every conversation is still dispatched; a quota error is about that one request"
        );
        assert!(!result.primary_scan_transport_error);
        assert_eq!(result.failed_conversations, 8);
        assert_eq!(result.not_started_conversations, 0);
        assert_eq!(
            result
                .failures
                .iter()
                .filter(|line| line.contains("HTTP 402"))
                .count(),
            8,
            "each occurrence gets its own line, none collapsed: {:?}",
            result.failures
        );
        assert_eq!(progress.processed.load(Ordering::Relaxed), 8);
        assert_eq!(
            result.failures.len(),
            result.failed_conversations_detail.len(),
            "every failure line has a matching retry detail"
        );
    }

    /// Owner's live-scan symptom (2026-09-11, originally reproduced with a
    /// quota error -- now a genuine transport error, since quota no longer
    /// stops the pass; see `is_transport_error`): a stop-class error on
    /// conversation 2 of 6 leaves conversations 3-6 with no `JobOutcome` at
    /// all -- `run_worker`'s single worker sees `stop` before it ever calls
    /// `next_job` for them, so they are absent from `run_jobs`'s answers,
    /// not merely marked failed. Every one of the 6 must still be
    /// accounted for, and the 4 that were never dispatched must read as one
    /// line, not vanish and not get one line each.
    #[test]
    fn scan_stop_accounts_for_every_undispatched_conversation() {
        let messages = parallel_corpus(6);
        let positions = handle_positions(&messages);
        let progress = ScanProgress::default();
        let result = super::scan_conversations(&messages, &progress, &ParallelPass::new(1), &|c| {
            if positions[&c[0].handle] == 1 {
                return Err(ProviderError::Network);
            }
            Ok(no_loop_items())
        });
        assert_eq!(result.analyzed, 1, "only conversation 0 completed");
        assert_eq!(result.unanalyzed_conversations(), 5);
        assert_eq!(result.not_started_conversations, 4);
        assert!(result.primary_scan_transport_error);
        assert_eq!(
            result
                .failures
                .iter()
                .filter(|line| line.contains("Could not establish or complete a secure connection"))
                .count(),
            1,
            "exactly one Network line: {:?}",
            result.failures
        );
        assert_eq!(
            result
                .failures
                .iter()
                .filter(|line| line.contains("was not analyzed because the scan stopped"))
                .count(),
            4,
            "one line per not-started conversation: {:?}",
            result.failures
        );
        assert_eq!(
            result.failures.len(),
            result.failed_conversations_detail.len()
        );
    }

    /// A stop-class error answered by every worker in the same first wave is
    /// fully accounted for as failed, not not-started: each of those
    /// conversations was genuinely dispatched and attempted before the pass
    /// ever saw the stop signal, so this is pure `failed_conversations`
    /// accounting, with no aggregated not-started line -- nothing was left
    /// behind. Unlike a quota error (see
    /// `quota_is_a_per_conversation_failure_and_does_not_stop_dispatch`),
    /// each occurrence here gets its own line rather than collapsing,
    /// because that dedup was quota-specific.
    #[test]
    fn stop_error_failures_in_flight_are_each_counted_as_failed_not_not_started() {
        let messages = parallel_corpus(3);
        let progress = ScanProgress::default();
        let first_wave = std::sync::Barrier::new(3);
        let result =
            super::scan_conversations(&messages, &progress, &ParallelPass::new(3), &|_| {
                first_wave.wait();
                Err(ProviderError::Network)
            });
        assert_eq!(
            result
                .failures
                .iter()
                .filter(|line| line.contains("Could not establish or complete a secure connection"))
                .count(),
            3,
            "each in-flight conversation gets its own line: {:?}",
            result.failures
        );
        assert_eq!(result.unanalyzed_conversations(), 3);
        assert_eq!(result.not_started_conversations, 0);
    }

    #[test]
    fn a_panicking_job_is_skipped_and_releases_the_only_limiter_slot() {
        let messages = parallel_corpus(3);
        let positions = handle_positions(&messages);
        let progress = ScanProgress::default();
        let pass = ParallelPass::new(2);
        pass.on_backoff();
        let ran = AtomicUsize::new(0);
        let result = super::scan_conversations(&messages, &progress, &pass, &|conversation| {
            let index = positions[&conversation[0].handle];
            assert_ne!(index, 0, "synthetic provider panic");
            ran.fetch_add(1, Ordering::SeqCst);
            Ok(no_loop_items())
        });
        assert_eq!(ran.load(Ordering::SeqCst), 2);
        assert_eq!(progress.snapshot().in_flight, 0);
        assert_eq!(progress.processed.load(Ordering::Relaxed), 3);
        assert_eq!(result.analyzed, 2);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(
            result.failures[0],
            "Conversation 1: the analysis failed unexpectedly and was skipped."
        );
    }

    #[test]
    fn the_in_flight_counter_and_request_clock_track_the_running_workers() {
        let messages = parallel_corpus(4);
        let progress = ScanProgress::default();
        let observed = Mutex::new(Vec::new());
        super::scan_conversations(&messages, &progress, &ParallelPass::new(4), &|_| {
            std::thread::sleep(Duration::from_millis(60));
            let snapshot = progress.snapshot();
            observed
                .lock()
                .unwrap()
                .push((snapshot.in_flight, snapshot.request_started_unix));
            Ok(no_loop_items())
        });
        let observed = observed.into_inner().unwrap();
        assert_eq!(observed.len(), 4);
        assert!(
            observed.iter().any(|(in_flight, _)| *in_flight > 1),
            "several requests must be in flight at once: {observed:?}"
        );
        assert!(
            observed.iter().all(|(_, clock)| *clock != 0),
            "the request clock is armed while requests run: {observed:?}"
        );
        assert_eq!(progress.snapshot(), ProgressSnapshot::default());
    }

    #[test]
    fn cancellation_and_provider_failure_do_not_start_more_conversations() {
        let a = prepare(&synthetic("Please send the draft.", 0, "a"), "Inbox", 0).unwrap();
        let b = prepare(&synthetic("Please send the agenda.", 1, "b"), "Inbox", 1).unwrap();
        let progress = ScanProgress::default();
        let result = scan_conversations(&[a.clone(), b.clone()], &progress, |_| {
            progress.cancel.store(true, Ordering::Relaxed);
            Ok(LoopItems {
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
            Err(ProviderError::Network)
        });
        assert_eq!(calls, 1);
        assert_eq!(result.analyzed, 0);
        // The Network failure line, plus one aggregate line for the second
        // conversation the pass never dispatched.
        assert_eq!(result.failures.len(), 2);
        assert_eq!(result.not_started_conversations, 1);
        assert!(result.primary_scan_transport_error);
    }

    #[test]
    fn a_rate_limited_conversation_fails_visibly_without_stopping_the_scan() {
        // The provider is answering, just not this fast, so the remaining
        // conversations are still worth analyzing -- but the rate-limited
        // one is never resent, and says so.
        let a = prepare(&synthetic("Please send the draft.", 0, "a"), "Inbox", 0).unwrap();
        let b = prepare(&synthetic("Please send the agenda.", 1, "b"), "Inbox", 1).unwrap();
        let mut calls: Vec<String> = vec![];
        let result = scan_conversations(&[a, b], &ScanProgress::default(), |messages| {
            calls.push(messages[0].handle.clone());
            Err(ProviderError::RateLimited)
        });
        assert_eq!(calls.len(), 2, "every conversation is still attempted");
        assert_eq!(
            calls.iter().collect::<BTreeSet<_>>().len(),
            2,
            "no conversation is resent: {calls:?}"
        );
        assert_eq!(result.failures.len(), 2);
        assert!(
            result
                .failures
                .iter()
                .all(|line| line
                    .ends_with("The provider rate-limited this request; it was not resent.")),
            "failure lines: {:?}",
            result.failures
        );
        assert!(!result.primary_scan_transport_error);
        assert!(!result.cancelled);
    }

    #[test]
    fn a_cancelled_request_records_the_not_started_conversation() {
        let a = prepare(&synthetic("Please send the draft.", 0, "a"), "Inbox", 0).unwrap();
        let b = prepare(&synthetic("Please send the agenda.", 1, "b"), "Inbox", 1).unwrap();
        let mut calls = 0;
        let result = scan_conversations(&[a, b], &ScanProgress::default(), |_| {
            calls += 1;
            Err(ProviderError::Cancelled)
        });
        assert_eq!(calls, 1);
        assert!(result.cancelled);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(
            result.failures.len(),
            result.failed_conversations_detail.len()
        );
        assert!(!result.primary_scan_transport_error);
    }

    #[test]
    fn request_started_unix_is_set_during_a_conversation_and_cleared_after() {
        let a = prepare(&synthetic("Please send the draft.", 0, "a"), "Inbox", 0).unwrap();
        let progress = ScanProgress::default();
        let mut observed_during = 0;
        let result = scan_conversations(&[a], &progress, |_| {
            let snapshot = progress.snapshot();
            observed_during = snapshot.request_started_unix;
            assert_eq!(snapshot.conversation_index, 1);
            assert_eq!(progress.conversation_total.load(Ordering::Relaxed), 1);
            Ok(LoopItems {
                items: vec![],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            })
        });
        assert_ne!(observed_during, 0, "expected a request start timestamp");
        assert_eq!(progress.snapshot().request_started_unix, 0);
        assert_eq!(result.analyzed, 1);
        assert_eq!(
            progress.snapshot().conversation_index,
            0,
            "conversation_index resets once the pass ends"
        );
        assert!(!progress.closure_phase.load(Ordering::Relaxed));
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

    fn closure_test_messages() -> (Vec<ReviewMessage>, LoopItem) {
        let evidence = request_from("sam@example.invalid", "req-1", "c1", "acct", "Fee");
        let all = vec![prepare(&evidence, "Inbox", 0).unwrap()];
        let item = LoopItem {
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
            deadline_kind_hint: None,
            event: None,
            event_time: None,
            resolution: None,
            resolution_kind: None,
            uncertainty: String::new(),
            unverified_deadline: false,
            unverified_resolution: false,
            cross_thread: false,
            event_passed: None,
            suggested_update: None,
            from_call_summary: false,
            meeting_time: None,
            meeting_time_approx: false,
        };
        (all, item)
    }

    fn recap_attribution_fixture(waiting_party: &str, recap: bool) -> (ReviewMessage, LoopItem) {
        let mut mail = synthetic(
            "Meeting Purpose\nReview the project update.\nAction Items\nSend the project update.",
            0,
            "recap-attribution",
        );
        if recap {
            mail.subject = "Project sync - Meeting Summary".into();
            mail.sender = "Fathom <no-reply@fathom.video>".into();
            mail.sender_address = "no-reply@fathom.video".into();
        }
        let message = prepare(&mail, "Inbox", 0).unwrap();
        let (_, mut item) = closure_test_messages();
        item.action = "Send the project update".into();
        item.action_phrase = "send the project update".into();
        item.owner = Owner::Team;
        item.waiting_party = waiting_party.into();
        item.evidence.message.clone_from(&message.input.handle);
        item.evidence.quote = "Send the project update.".into();
        item.evidence.context = "Send the project update.".into();
        (message, item)
    }

    fn projected_recap_item(waiting_party: &str, recap: bool) -> LoopItem {
        let (message, item) = recap_attribution_fixture(waiting_party, recap);
        let result = scan_conversations(
            std::slice::from_ref(&message),
            &ScanProgress::default(),
            |_| {
                Ok(LoopItems {
                    items: vec![item.clone()],
                    rejected: 0,
                    rejection_reasons: vec![],
                    degraded: 0,
                })
            },
        );
        result.analysis.items.into_iter().next().unwrap()
    }

    /// `count` open requests, each with a later reply in a different
    /// conversation, so every one of them is eligible for the closure pass.
    fn parallel_closure_fixture(count: usize) -> (Vec<ReviewMessage>, Vec<LoopItem>) {
        let mut all = Vec::new();
        let mut items = Vec::new();
        for index in 0..count {
            let address = format!("sam{index}@example.invalid");
            let request = request_from(
                &address,
                &format!("req-{index}"),
                &format!("r{index}"),
                "acct",
                "Fee",
            );
            all.push(prepare(&request, "Inbox", all.len()).unwrap());
            let evidence = all.last().unwrap().input.handle.clone();
            let reply = reply_to(
                &address,
                &format!("rep-{index}"),
                &format!("v{index}"),
                "acct",
                "Fee",
            );
            all.push(prepare(&reply, "Sent", all.len()).unwrap());
            let (_, template) = closure_test_messages();
            items.push(LoopItem {
                waiting_party: format!("Other <{address}>"),
                evidence: Anchor {
                    message: evidence,
                    ..template.evidence
                },
                ..template
            });
        }
        (all, items)
    }

    fn closure_result(items: Vec<LoopItem>) -> ScanResult {
        let mut result = empty_result(items.len());
        result.analyzed = items.len();
        result.analysis.items = items;
        result
    }

    fn scoped_event_message() -> ReviewMessage {
        const SUBJECT: &str =
            "Spring Planning Workshop checklist (Tuesday, August 25 – 2:00–5:30pm)";
        let mut event_mail = synthetic(
            "Install the tool before arriving at the workshop. Claim the free month in the portal.",
            0,
            "workshop-thread",
        );
        event_mail.received = "2026-08-01T12:00:00Z".into();
        event_mail.subject = SUBJECT.into();
        prepare(&event_mail, "Inbox", 0).unwrap()
    }

    fn scoped_event_expectation(
        template: &LoopItem,
        message_handle: &str,
        action: &str,
        quote: &str,
    ) -> LoopItem {
        LoopItem {
            action: action.into(),
            action_phrase: action.into(),
            evidence: Anchor {
                message: message_handle.into(),
                block: 0,
                quote: quote.into(),
                context: String::new(),
            },
            ..template.clone()
        }
    }

    fn event_anchored_logistics_fixture() -> (ReviewMessage, LoopItem) {
        const SUBJECT: &str = "Claude Code Workshop SF | Anthropic x Tenex";
        const EVENT_TIME: &str = "Tuesday, September 2, 2026, 9:00 AM \u{2013} 12:00 PM PT";
        let body = format!(
            "{SUBJECT}\n{EVENT_TIME}\nVenue: Harbor Studio, 400 Example Avenue.\n\
             Please review the schedule, venue & logistics.\n\
             Send the slides to the organiser."
        );
        let mut mail = synthetic(&body, 0, "claude-code-workshop-thread");
        mail.received = "2026-08-20T16:00:00Z".into();
        mail.subject = SUBJECT.into();
        mail.sender = "Morgan Lee <morgan@training.example.invalid>".into();
        mail.sender_address = "morgan@training.example.invalid".into();
        let message = prepare(&mail, "Inbox", 0).unwrap();

        let (_, template) = closure_test_messages();
        let mut item = scoped_event_expectation(
            &template,
            &message.input.handle,
            "Review the schedule, venue & logistics",
            "Please review the schedule, venue & logistics.",
        );
        item.event = Some(Anchor {
            message: message.input.handle.clone(),
            block: 0,
            quote: SUBJECT.into(),
            context: body.clone(),
        });
        item.event_time = Some(Anchor {
            message: message.input.handle.clone(),
            block: 0,
            quote: EVENT_TIME.into(),
            context: body,
        });
        (message, item)
    }

    #[test]
    fn event_anchored_logistics_request_closes_after_the_event() {
        let (message, mut item) = event_anchored_logistics_fixture();
        item.event_time = None;
        let index = build_event_index(std::slice::from_ref(&message));
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].source, EventSource::Prose);
        let event_end = index[0].end;
        let mut result = closure_result(vec![item]);

        close_passed_events(&mut result, std::slice::from_ref(&message), event_end + 1);

        let passed = result.analysis.items[0].event_passed.as_ref().unwrap();
        assert_eq!(passed.name, "claude code workshop sf | anthropic x tenex");
        assert_eq!(passed.message_handle, message.input.handle);
        let event_time = result.analysis.items[0].event_time.as_ref().unwrap();
        assert_eq!(event_time.message, message.input.handle);
        let offset = chrono::FixedOffset::east_opt(local_offset_seconds(event_end, 0)).unwrap();
        let expected = chrono::Utc
            .timestamp_opt(event_end, 0)
            .single()
            .unwrap()
            .with_timezone(&offset)
            .format("%Y-%m-%dT%H:%M")
            .to_string();
        assert_eq!(event_time.quote, expected);
        assert_eq!(result.event_closures, 1);
    }

    #[test]
    fn event_anchored_request_stays_open_before_the_event() {
        let (message, mut item) = event_anchored_logistics_fixture();
        item.event_time = None;
        let event_start = build_event_index(std::slice::from_ref(&message))[0].start;
        let mut result = closure_result(vec![item]);

        close_passed_events(&mut result, std::slice::from_ref(&message), event_start - 1);

        assert!(result.analysis.items[0].event_passed.is_none());
        assert!(result.analysis.items[0].event_time.is_some());
        assert_eq!(result.event_closures, 0);
    }

    #[test]
    fn unanchored_generic_request_in_an_invite_still_stays_open() {
        let (message, mut item) = event_anchored_logistics_fixture();
        item.action = "Send the slides to the organiser".into();
        item.action_phrase = "send the slides to the organiser".into();
        item.evidence.quote = "Send the slides to the organiser.".into();
        item.event = None;
        item.event_time = None;
        let event_end = build_event_index(std::slice::from_ref(&message))[0].end;
        let mut result = closure_result(vec![item]);

        close_passed_events(&mut result, std::slice::from_ref(&message), event_end + 1);

        assert!(result.analysis.items[0].event_passed.is_none());
        assert_eq!(result.event_closures, 0);
    }

    #[test]
    fn scoped_subject_event_closes_the_event_shaped_request_and_future_stays_open() {
        let event_message = scoped_event_message();
        let event_end = build_event_index(std::slice::from_ref(&event_message))[0].end;
        let (_, template) = closure_test_messages();
        let handle = &event_message.input.handle;
        let install = scoped_event_expectation(
            &template,
            handle,
            "Install the tool before arriving at the workshop",
            "Install the tool before arriving at the workshop.",
        );
        let claim = scoped_event_expectation(
            &template,
            handle,
            "Claim the free month in the portal",
            "Claim the free month in the portal.",
        );

        let mut passed = closure_result(vec![install.clone(), claim.clone()]);
        close_passed_events(
            &mut passed,
            std::slice::from_ref(&event_message),
            event_end + 1,
        );
        // "Install ... before arriving" uses event-shaped language
        // (`has_scoped_event_language`), so the own-message branch of
        // `scoped_event` credits it and it closes. "Claim the free month in
        // the portal" merely appears in the same event-bearing message; it
        // is not itself worded as being about the event and carries no
        // `EventTied` deadline, so per the own-message-branch gating rule it
        // must stay open even though it shares the event's message.
        assert!(passed.analysis.items[0].event_passed.is_some());
        assert!(passed.analysis.items[1].event_passed.is_none());
        assert_eq!(passed.event_closures, 1);
        assert!(passed.conversation_notes[0].contains("1 tied to their own message's event"));

        let mut future = closure_result(vec![install, claim]);
        close_passed_events(&mut future, std::slice::from_ref(&event_message), event_end);
        assert!(
            future
                .analysis
                .items
                .iter()
                .all(|item| item.event_passed.is_none())
        );
    }

    #[test]
    fn scoped_subject_event_closure_keeps_an_invoice_with_a_later_deadline_open() {
        let mut mail = synthetic(
            "Install the tool before arriving. Claim the free month. Send me the invoice next Friday.",
            0,
            "workshop-thread",
        );
        mail.received = "2026-08-24T12:00:00Z".into();
        mail.subject =
            "Spring Planning Workshop checklist (Tuesday, August 25 - 2:00-5:30pm)".into();
        let event_message = prepare(&mail, "Inbox", 0).unwrap();
        let event_end = build_event_index(std::slice::from_ref(&event_message))[0].end;
        let (_, template) = closure_test_messages();
        let handle = &event_message.input.handle;
        let install = scoped_event_expectation(
            &template,
            handle,
            "Install the tool before arriving",
            "Install the tool before arriving.",
        );
        let claim = scoped_event_expectation(
            &template,
            handle,
            "Claim the free month",
            "Claim the free month.",
        );
        let mut invoice = scoped_event_expectation(
            &template,
            handle,
            "Send the invoice",
            "Send me the invoice next Friday.",
        );
        invoice.deadline = Some(Anchor {
            message: handle.clone(),
            block: 0,
            quote: "Friday".into(),
            context: "Send me the invoice next Friday.".into(),
        });
        let mut result = closure_result(vec![install, claim, invoice]);

        close_passed_events(
            &mut result,
            std::slice::from_ref(&event_message),
            timestamp("2026-09-01T00:00:00Z"),
        );

        assert!(result.analysis.items[0].event_passed.is_some());
        // "Claim the free month" is not itself event-shaped language and
        // carries no `EventTied` deadline, so the own-message-branch gating
        // rule keeps it open even though it shares the event's message.
        assert!(result.analysis.items[1].event_passed.is_none());
        assert!(result.analysis.items[2].event_passed.is_none());
        assert!(
            deadline_boundary(&result.analysis.items[2], &[event_message], event_end + 1)
                .is_some_and(|deadline| deadline > event_end)
        );
    }

    #[test]
    fn event_worded_request_in_the_same_conversation_uses_the_scoped_event() {
        let event_message = scoped_event_message();
        let event_end = build_event_index(std::slice::from_ref(&event_message))[0].end;
        let (_, template) = closure_test_messages();
        let mut scoped_mail = synthetic(
            "Please bring the outline before the workshop.",
            1,
            "workshop-thread",
        );
        scoped_mail.received = "2026-08-02T12:00:00Z".into();
        let scoped_message = prepare(&scoped_mail, "Inbox", 1).unwrap();
        let scoped_request = LoopItem {
            action: "Bring the outline before the workshop".into(),
            evidence: Anchor {
                message: scoped_message.input.handle.clone(),
                block: 0,
                quote: "Please bring the outline before the workshop.".into(),
                context: String::new(),
            },
            ..template.clone()
        };
        let mut scoped_result = closure_result(vec![scoped_request]);
        close_passed_events(
            &mut scoped_result,
            &[event_message.clone(), scoped_message],
            event_end + 1,
        );
        assert!(scoped_result.analysis.items[0].event_passed.is_some());
    }

    #[test]
    fn named_event_closure_precedes_conversation_fallback_with_multiple_events() {
        let mut workshop_mail = synthetic("Calendar invitation.", 0, "event-thread");
        workshop_mail.received = "2026-08-01T10:00:00Z".into();
        workshop_mail.subject =
            "Invitation: Planning Workshop @ Fri Aug 21, 2026 11am - 12pm (UTC)".into();
        let workshop = prepare(&workshop_mail, "Inbox", 0).unwrap();
        let mut launch_mail = synthetic("Calendar invitation.", 1, "event-thread");
        launch_mail.received = "2026-08-01T11:00:00Z".into();
        launch_mail.subject =
            "Invitation: Product Launch @ Fri Sep 25, 2026 11am - 12pm (UTC)".into();
        let launch = prepare(&launch_mail, "Inbox", 1).unwrap();
        let mut request_mail =
            synthetic("Bring the deck to the Product Launch.", 2, "event-thread");
        request_mail.received = "2026-08-01T12:00:00Z".into();
        let request = prepare(&request_mail, "Inbox", 2).unwrap();
        let (_, template) = closure_test_messages();
        let mut item = scoped_event_expectation(
            &template,
            &request.input.handle,
            "Bring the deck to the Product Launch",
            "Bring the deck to the Product Launch.",
        );
        item.event = Some(Anchor {
            message: request.input.handle.clone(),
            block: 0,
            quote: "the Product Launch".into(),
            context: "Bring the deck to the Product Launch.".into(),
        });
        let mut result = closure_result(vec![item]);

        close_passed_events(
            &mut result,
            &[workshop, launch, request],
            timestamp("2026-08-22T00:00:00Z"),
        );

        assert!(result.analysis.items[0].event_passed.is_none());
    }

    #[test]
    fn conversation_event_closure_fallback_skips_multiple_qualifying_events() {
        let mut workshop_mail = synthetic("Calendar invitation.", 0, "event-thread");
        workshop_mail.subject =
            "Invitation: Planning Workshop @ Fri Aug 21, 2026 11am - 12pm (UTC)".into();
        let workshop = prepare(&workshop_mail, "Inbox", 0).unwrap();
        let mut launch_mail = synthetic("Calendar invitation.", 1, "event-thread");
        launch_mail.subject =
            "Invitation: Product Launch @ Fri Aug 28, 2026 11am - 12pm (UTC)".into();
        let launch = prepare(&launch_mail, "Inbox", 1).unwrap();
        let request =
            prepare(&synthetic("Bring the deck.", 2, "event-thread"), "Inbox", 2).unwrap();
        let (_, template) = closure_test_messages();
        let item = scoped_event_expectation(
            &template,
            &request.input.handle,
            "Bring the deck",
            "Bring the deck.",
        );
        let mut result = closure_result(vec![item]);

        close_passed_events(
            &mut result,
            &[workshop, launch, request],
            timestamp("2026-09-01T00:00:00Z"),
        );

        assert!(result.analysis.items[0].event_passed.is_none());
    }

    #[test]
    fn conversation_event_closure_ignores_event_noun_followed_by_notes() {
        let event_message = scoped_event_message();
        let event_end = build_event_index(std::slice::from_ref(&event_message))[0].end;
        let mut request_mail = synthetic("Send the meeting notes by Friday.", 1, "workshop-thread");
        request_mail.received = "2026-08-02T12:00:00Z".into();
        let request = prepare(&request_mail, "Inbox", 1).unwrap();
        let (_, template) = closure_test_messages();
        let item = scoped_event_expectation(
            &template,
            &request.input.handle,
            "Send the meeting notes by Friday",
            "Send the meeting notes by Friday.",
        );
        let mut result = closure_result(vec![item]);

        close_passed_events(&mut result, &[event_message, request], event_end + 1);

        assert!(result.analysis.items[0].event_passed.is_none());
    }

    #[test]
    fn scoped_event_does_not_cross_conversations_or_close_unworded_later_requests() {
        let event_message = scoped_event_message();
        let event_end = build_event_index(std::slice::from_ref(&event_message))[0].end;
        let (_, template) = closure_test_messages();
        let mut unrelated_mail = synthetic("Please attend the workshop.", 1, "other-thread");
        unrelated_mail.received = "2026-08-02T12:00:00Z".into();
        let unrelated_message = prepare(&unrelated_mail, "Inbox", 1).unwrap();
        let mut unrelated = LoopItem {
            action: "Attend the workshop".into(),
            evidence: Anchor {
                message: unrelated_message.input.handle.clone(),
                block: 0,
                quote: "Please attend the workshop.".into(),
                context: String::new(),
            },
            ..template.clone()
        };
        let mut same_thread_mail =
            synthetic("Claim the free month in the portal.", 1, "workshop-thread");
        same_thread_mail.received = "2026-08-02T12:00:00Z".into();
        let same_thread_message = prepare(&same_thread_mail, "Inbox", 2).unwrap();
        let no_event_words = LoopItem {
            action: "Claim the free month in the portal".into(),
            evidence: Anchor {
                message: same_thread_message.input.handle.clone(),
                block: 0,
                quote: "Claim the free month in the portal.".into(),
                context: String::new(),
            },
            ..template
        };
        unrelated.event = None;
        let messages = vec![event_message, unrelated_message, same_thread_message];
        let mut out_of_scope = closure_result(vec![unrelated, no_event_words]);
        close_passed_events(&mut out_of_scope, &messages, event_end + 1);
        assert!(
            out_of_scope
                .analysis
                .items
                .iter()
                .all(|item| item.event_passed.is_none())
        );
    }

    #[test]
    fn close_passed_events_marks_matching_open_loops() {
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
            analysis: LoopItems {
                items: vec![item],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            failed_conversations_detail: vec![],
            analyzed: 1,
            total: 1,
            cancelled: false,
            conversation_notes: vec![],
            conversation_quality: BTreeMap::new(),
            conversation_notes_by_id: BTreeMap::new(),
            conversation_rejection_reasons: BTreeMap::new(),
            suggested_updates: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            conversation_count: 1,
            analyzed_conversations: 1,
            failed_conversations: 0,
            not_started_conversations: 0,
            closure_pass_failure: None,
        };
        close_passed_events(&mut result, &messages, timestamp("2026-08-22T00:00:00Z"));
        let passed = result.analysis.items[0].event_passed.as_ref().unwrap();
        assert_eq!(passed.name, "design workshop");
        assert_eq!(passed.end, timestamp("2026-08-21T12:00:00Z"));
        assert_eq!(passed.message_handle, "m1");
        assert_eq!(result.event_closures, 1);
        assert!(
            passed.from_subject,
            "a calendar-invite-subject closure came from the subject line"
        );
        assert_eq!(
            result.conversation_notes.last().unwrap(),
            "Event index: 1 event learned (0 meetings, 1 calendar subjects, \
0 subject prose, 0 body prose); \
0 tied to their own message's event, 1 named an event, 0 carried a time, 1 matched by name, 0 matched by request text, \
1 closed from the index, 0 closed from a stated time."
        );
    }

    /// Important-fix regression: when the item's named event has a
    /// matching index entry that has NOT ended yet, the index alone
    /// decides this item -- the stated-time path
    /// (`close_from_stated_time`) must never be consulted, even though the
    /// item's own `event_time` anchor would, evaluated in isolation,
    /// already read as passed.
    #[test]
    fn close_passed_events_a_future_index_match_suppresses_the_stated_time_path() {
        let mut event_mail = synthetic("Calendar invitation.", 1, "event");
        event_mail.subject =
            "Invitation: Design workshop @ Fri Sep 25, 2026 11am - 12pm (UTC)".into();
        let event_message = prepare(&event_mail, "Inbox", 1).unwrap();
        let (mut messages, mut item) = closure_test_messages();
        messages[0].input.timestamp = timestamp("2026-09-02T12:00:00Z"); // Wednesday
        item.event = Some(Anchor {
            message: item.evidence.message.clone(),
            block: 0,
            quote: "the design workshop".into(),
            context: String::new(),
        });
        item.event_time = Some(Anchor {
            message: item.evidence.message.clone(),
            block: 0,
            quote: "Friday".into(),
            context: String::new(),
        });
        messages.push(event_message);
        let mut result = ScanResult {
            analysis: LoopItems {
                items: vec![item],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            failed_conversations_detail: vec![],
            analyzed: 1,
            total: 1,
            cancelled: false,
            conversation_notes: vec![],
            conversation_quality: BTreeMap::new(),
            conversation_notes_by_id: BTreeMap::new(),
            conversation_rejection_reasons: BTreeMap::new(),
            suggested_updates: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            conversation_count: 1,
            analyzed_conversations: 1,
            failed_conversations: 0,
            not_started_conversations: 0,
            closure_pass_failure: None,
        };
        // After the stated "Friday" (Sep 4) but well before the indexed
        // workshop (Sep 25): a naive stated-time classification alone
        // would already read this item as passed.
        close_passed_events(&mut result, &messages, timestamp("2026-09-10T00:00:00Z"));
        assert!(result.analysis.items[0].event_passed.is_none());
        assert_eq!(result.event_closures, 0);
    }

    /// The coverage note's live finding: the model emitted no `event` or
    /// `event_time` anchor at all (0 named), so closing this item depends
    /// entirely on matching the index by the item's own request text
    /// (`action`, here) -- at the stronger, proper-name-strength bar
    /// ([`match_event_by_text`]), since "review" alone is a generic noun
    /// stripped from both sides.
    #[test]
    fn close_passed_events_matches_and_closes_via_request_text_when_no_event_was_named() {
        let mut event_mail = synthetic("Calendar invitation.", 1, "event");
        event_mail.subject =
            "Invitation: Acme Contract Review @ Fri Aug 21, 2026 11am - 12pm (UTC)".into();
        let event_message = prepare(&event_mail, "Inbox", 1).unwrap();
        let (mut messages, mut item) = closure_test_messages();
        messages[0].input.timestamp = timestamp("2026-08-01T00:00:00Z");
        item.action = "Prepare materials for the Acme Contract Review".into();
        messages.push(event_message);
        let mut result = ScanResult {
            analysis: LoopItems {
                items: vec![item],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            failed_conversations_detail: vec![],
            analyzed: 1,
            total: 1,
            cancelled: false,
            conversation_notes: vec![],
            conversation_quality: BTreeMap::new(),
            conversation_notes_by_id: BTreeMap::new(),
            conversation_rejection_reasons: BTreeMap::new(),
            suggested_updates: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            conversation_count: 1,
            analyzed_conversations: 1,
            failed_conversations: 0,
            not_started_conversations: 0,
            closure_pass_failure: None,
        };
        close_passed_events(&mut result, &messages, timestamp("2026-08-22T00:00:00Z"));
        let passed = result.analysis.items[0].event_passed.as_ref().unwrap();
        assert_eq!(passed.name, "acme contract review");
        assert_eq!(result.event_closures, 1);
        assert_eq!(
            result.conversation_notes.last().unwrap(),
            "Event index: 1 event learned (0 meetings, 1 calendar subjects, \
0 subject prose, 0 body prose); \
0 tied to their own message's event, 0 named an event, 0 carried a time, 0 matched by name, 1 matched by request text, \
1 closed from the index, 0 closed from a stated time."
        );
    }

    /// The text-matching fallback requires 2 shared meaningful tokens, not
    /// 1 -- a single shared word ("Acme") must never be enough to close an
    /// item whose model output named no event at all.
    #[test]
    fn close_passed_events_text_match_requires_two_shared_tokens_not_one() {
        let mut event_mail = synthetic("Calendar invitation.", 1, "event");
        event_mail.subject =
            "Invitation: Acme Contract Review @ Fri Aug 21, 2026 11am - 12pm (UTC)".into();
        let event_message = prepare(&event_mail, "Inbox", 1).unwrap();
        let (mut messages, mut item) = closure_test_messages();
        messages[0].input.timestamp = timestamp("2026-08-01T00:00:00Z");
        item.action = "Prepare the Acme budget".into();
        messages.push(event_message);
        let mut result = ScanResult {
            analysis: LoopItems {
                items: vec![item],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            failed_conversations_detail: vec![],
            analyzed: 1,
            total: 1,
            cancelled: false,
            conversation_notes: vec![],
            conversation_quality: BTreeMap::new(),
            conversation_notes_by_id: BTreeMap::new(),
            conversation_rejection_reasons: BTreeMap::new(),
            suggested_updates: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            conversation_count: 1,
            analyzed_conversations: 1,
            failed_conversations: 0,
            not_started_conversations: 0,
            closure_pass_failure: None,
        };
        close_passed_events(&mut result, &messages, timestamp("2026-08-22T00:00:00Z"));
        assert!(result.analysis.items[0].event_passed.is_none());
        assert_eq!(result.event_closures, 0);
    }

    #[test]
    fn close_passed_events_always_pushes_a_coverage_note_even_when_nothing_is_learned() {
        let (messages, item) = closure_test_messages();
        let mut result = ScanResult {
            analysis: LoopItems {
                items: vec![item],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            failed_conversations_detail: vec![],
            analyzed: 1,
            total: 1,
            cancelled: false,
            conversation_notes: vec![],
            conversation_quality: BTreeMap::new(),
            conversation_notes_by_id: BTreeMap::new(),
            conversation_rejection_reasons: BTreeMap::new(),
            suggested_updates: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            conversation_count: 1,
            analyzed_conversations: 1,
            failed_conversations: 0,
            not_started_conversations: 0,
            closure_pass_failure: None,
        };
        close_passed_events(&mut result, &messages, timestamp("2026-08-22T00:00:00Z"));
        assert_eq!(
            result.conversation_notes.last().unwrap(),
            "Event index: 0 events learned (0 meetings, 0 calendar subjects, \
0 subject prose, 0 body prose); \
0 tied to their own message's event, 0 named an event, 0 carried a time, 0 matched by name, 0 matched by request text, \
0 closed from the index, 0 closed from a stated time."
        );
    }

    #[test]
    fn close_passed_events_skips_the_note_when_the_scan_was_cancelled() {
        let (messages, item) = closure_test_messages();
        let mut result = ScanResult {
            analysis: LoopItems {
                items: vec![item],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            failed_conversations_detail: vec![],
            analyzed: 1,
            total: 1,
            cancelled: true,
            conversation_notes: vec![],
            conversation_quality: BTreeMap::new(),
            conversation_notes_by_id: BTreeMap::new(),
            conversation_rejection_reasons: BTreeMap::new(),
            suggested_updates: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            conversation_count: 1,
            analyzed_conversations: 1,
            failed_conversations: 0,
            not_started_conversations: 0,
            closure_pass_failure: None,
        };
        close_passed_events(&mut result, &messages, timestamp("2026-08-22T00:00:00Z"));
        assert!(result.conversation_notes.is_empty());
    }

    /// The workshop's date is stated only in an email body -- no invitation
    /// or calendar subject exists for it, so the event index has no entry
    /// at all -- so closing it depends entirely on the model-supplied
    /// `event_time` phrase, classified directly. Rather than recomputing
    /// the boundary with the same (formerly buggy) formula the production
    /// code used, this asserts the LOCAL calendar date the closure actually
    /// shows: the event's own day (that Friday), not a UTC-midnight
    /// approximation that can drift onto the wrong day once converted to a
    /// viewer's timezone.
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
        let mut result = ScanResult {
            analysis: LoopItems {
                items: vec![item],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            failed_conversations_detail: vec![],
            analyzed: 1,
            total: 1,
            cancelled: false,
            conversation_notes: vec![],
            conversation_quality: BTreeMap::new(),
            conversation_notes_by_id: BTreeMap::new(),
            conversation_rejection_reasons: BTreeMap::new(),
            suggested_updates: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            conversation_count: 1,
            analyzed_conversations: 1,
            failed_conversations: 0,
            not_started_conversations: 0,
            closure_pass_failure: None,
        };
        close_passed_events(&mut result, &messages, now);
        let passed = result.analysis.items[0].event_passed.as_ref().unwrap();
        assert_eq!(passed.name, "the workshop");
        assert_eq!(passed.message_handle, messages[0].input.handle);
        assert_eq!(result.event_closures, 1);
        assert!(
            !passed.from_subject,
            "a stated-event-time closure came from body prose, not the subject line"
        );
        let local_date = chrono::DateTime::from_timestamp(passed.end, 0)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string();
        assert_eq!(local_date, "2026-09-04");
    }

    /// Gating fix: an item that never named an event at all -- no `event`
    /// anchor, and a plain (not `EventTied`) deadline -- must never be
    /// closed as "event passed" just because the model separately filled in
    /// an `event_time` anchor. A plain overdue deadline is not evidence an
    /// EVENT has ended.
    #[test]
    fn close_passed_events_leaves_a_plain_deadline_open_despite_a_stated_event_time() {
        let (mut messages, mut item) = closure_test_messages();
        messages[0].input.timestamp = timestamp("2026-09-02T12:00:00Z"); // Wednesday
        item.deadline = Some(Anchor {
            message: item.evidence.message.clone(),
            block: 0,
            quote: "Friday".into(),
            context: String::new(),
        });
        item.event_time = Some(Anchor {
            message: item.evidence.message.clone(),
            block: 0,
            quote: "Friday".into(),
            context: String::new(),
        });
        let now = timestamp("2026-09-15T00:00:00Z");
        let mut result = ScanResult {
            analysis: LoopItems {
                items: vec![item],
                rejected: 0,
                rejection_reasons: vec![],
                degraded: 0,
            },
            failures: vec![],
            failed_conversations_detail: vec![],
            analyzed: 1,
            total: 1,
            cancelled: false,
            conversation_notes: vec![],
            conversation_quality: BTreeMap::new(),
            conversation_notes_by_id: BTreeMap::new(),
            conversation_rejection_reasons: BTreeMap::new(),
            suggested_updates: 0,
            event_closures: 0,
            primary_scan_transport_error: false,
            conversation_count: 1,
            analyzed_conversations: 1,
            failed_conversations: 0,
            not_started_conversations: 0,
            closure_pass_failure: None,
        };
        close_passed_events(&mut result, &messages, now);
        assert!(result.analysis.items[0].event_passed.is_none());
        assert_eq!(result.event_closures, 0);
    }

    #[test]
    fn event_time_equal_to_the_deadline_does_not_close_the_request() {
        let (mut messages, mut item) = closure_test_messages();
        messages[0].input.timestamp = timestamp("2026-09-02T12:00:00Z"); // Wednesday
        item.deadline = Some(Anchor {
            message: item.evidence.message.clone(),
            block: 0,
            quote: "Friday".into(),
            context: String::new(),
        });
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
        let mut result = closure_result(vec![item]);

        close_passed_events(&mut result, &messages, timestamp("2026-09-15T00:00:00Z"));

        assert!(result.analysis.items[0].event_passed.is_none());
        assert_eq!(result.event_closures, 0);
    }

    /// Regression for the real-world defect: a meeting-summary message sent
    /// AFTER its own meeting, naming that meeting in its subject and
    /// carrying an action item worded with event-shaped language ("bring"),
    /// must never have that action item closed by "the meeting ended" -- the
    /// meeting is the SOURCE of the request, not its deadline. The action
    /// item deliberately uses `has_scoped_event_language` wording so this
    /// test isolates the temporal guard (`close_from_index`'s
    /// `source_timestamp` check) from `scoped_event`'s separate
    /// own-message-language gate: without the temporal guard, this item
    /// would still close.
    #[test]
    fn summary_sent_after_the_meeting_never_closes_its_action_items() {
        let mut mail = synthetic(
            "Action Items\nBring the workshop handout to Thomas.",
            0,
            "team-sync-thread",
        );
        mail.subject = "Team Sync @ Fri Sep 04, 2026 11am - 12pm (UTC)".into();
        let mut summary_message = prepare(&mail, "Inbox", 0).unwrap();
        let event_end = build_event_index(std::slice::from_ref(&summary_message))[0].end;
        // Sent 90 minutes after the meeting ended.
        summary_message.input.timestamp = event_end + 90 * 60;

        let (_, template) = closure_test_messages();
        let item = scoped_event_expectation(
            &template,
            &summary_message.input.handle,
            "Bring the workshop handout to Thomas",
            "Bring the workshop handout to Thomas.",
        );
        let mut result = closure_result(vec![item]);

        close_passed_events(
            &mut result,
            std::slice::from_ref(&summary_message),
            event_end + 86_400,
        );

        assert!(result.analysis.items[0].event_passed.is_none());
        assert_eq!(result.event_closures, 0);
    }

    /// Rule 2 regression: `scoped_event`'s own-message branch must credit an
    /// event only to a request actually worded as being about it. An invite
    /// naming its own time, read before the event, still closes an
    /// attendance-shaped request afterward but leaves an unrelated one open,
    /// even though both requests share the invite's own message.
    #[test]
    fn own_message_event_closes_only_event_shaped_requests() {
        let event_message = scoped_event_message();
        let event_end = build_event_index(std::slice::from_ref(&event_message))[0].end;
        let (_, template) = closure_test_messages();
        let handle = &event_message.input.handle;
        let attend = scoped_event_expectation(
            &template,
            handle,
            "Confirm you can attend the workshop",
            "Confirm you can attend the workshop.",
        );
        let slides = scoped_event_expectation(
            &template,
            handle,
            "Send the slides to the organiser",
            "Send the slides to the organiser.",
        );
        let mut result = closure_result(vec![attend, slides]);

        close_passed_events(
            &mut result,
            std::slice::from_ref(&event_message),
            event_end + 1,
        );

        assert!(
            result.analysis.items[0].event_passed.is_some(),
            "an attendance-shaped request must close once its own event has passed"
        );
        assert!(
            result.analysis.items[1].event_passed.is_none(),
            "a request with no event-shaped language and no EventTied deadline must stay open"
        );
    }

    /// Rule 2's `EventTied`-deadline exception: a request with no
    /// event-shaped wording of its own can still be credited to its own
    /// message's event when its stated DEADLINE classifies as
    /// `DeadlineView::EventTied` (e.g. "before the meeting").
    #[test]
    fn event_tied_deadline_closes_with_its_own_message_event() {
        let event_message = scoped_event_message();
        let event_end = build_event_index(std::slice::from_ref(&event_message))[0].end;
        let (_, template) = closure_test_messages();
        let handle = &event_message.input.handle;
        let mut item = scoped_event_expectation(
            &template,
            handle,
            "Send the updated budget",
            "Send the updated budget.",
        );
        item.deadline = Some(Anchor {
            message: handle.clone(),
            block: 0,
            quote: "before the workshop".into(),
            context: "Send the updated budget, before the workshop.".into(),
        });
        let mut result = closure_result(vec![item]);

        close_passed_events(
            &mut result,
            std::slice::from_ref(&event_message),
            event_end + 1,
        );

        assert!(result.analysis.items[0].event_passed.is_some());
        assert_eq!(result.event_closures, 1);
    }

    #[test]
    fn fathom_sender_is_a_recap_artifact_regardless_of_subject_or_body() {
        let mut mail = synthetic(
            "Meeting Purpose\nCatch up on the neurosymbolic AI project.\n\
Key Takeaways\nGood progress overall.\n\
Action Items\nSend Thomas the resources on neurosymbolic AI and the Leavenitz link.",
            0,
            "fathom-thread",
        );
        mail.subject = "Liat and Erin catch up (Erin Fraser)".into();
        mail.sender = "Fathom <no-reply@fathom.video>".into();
        mail.sender_address = "no-reply@fathom.video".into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        assert!(is_meeting_recap_artifact(&message));
    }

    #[test]
    fn recap_waiting_on_own_address_defaults_to_suggested_you() {
        let item = projected_recap_item("USER@EXAMPLE.INVALID", true);

        assert_eq!(item.waiting_party, "Not established");
        assert!(item.owner == Owner::You);
        assert!(item.from_call_summary);
        assert!(item.meeting_time.is_some());
    }

    #[test]
    fn recap_waiting_on_service_sender_defaults_to_suggested_you() {
        let item = projected_recap_item("Fathom <NO-REPLY@FATHOM.VIDEO>", true);

        assert_eq!(item.waiting_party, "Not established");
        assert!(item.owner == Owner::You);
    }

    #[test]
    fn recap_waiting_on_named_counterparty_is_untouched() {
        let item = projected_recap_item("Other <dana@example.invalid>", true);

        assert_eq!(item.waiting_party, "Other <dana@example.invalid>");
        assert!(item.owner == Owner::Team);
    }

    #[test]
    fn non_recap_with_the_same_attribution_is_untouched() {
        let item = projected_recap_item("USER@EXAMPLE.INVALID", false);

        assert_eq!(item.waiting_party, "USER@EXAMPLE.INVALID");
        assert!(item.owner == Owner::Team);
        assert!(!item.from_call_summary);
        assert_eq!(item.meeting_time, None);
    }

    #[test]
    fn recap_with_no_waiting_party_is_untouched() {
        let item = projected_recap_item("Not established", true);

        assert_eq!(item.waiting_party, "Not established");
        assert!(item.owner == Owner::Team);
    }

    #[test]
    fn subject_and_body_signals_together_mark_a_recap_artifact_with_no_known_sender_domain() {
        let mut mail = synthetic(
            "Summary\nAttendees: Alex, Sam\nWe discussed the roadmap.",
            0,
            "otter-style-thread",
        );
        mail.subject = "Call Summary: Weekly Sync".into();
        mail.sender = "Notetaker <notifications@notetaking-relay.example.invalid>".into();
        mail.sender_address = "notifications@notetaking-relay.example.invalid".into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        assert!(is_meeting_recap_artifact(&message));
    }

    #[test]
    fn tactiq_domain_and_teams_copilot_name_mark_recap_artifacts() {
        let mut tactiq = synthetic("Synthetic recap", 0, "tactiq-thread");
        tactiq.sender = "Tactiq <notes@tactiq.io>".into();
        tactiq.sender_address = "notes@tactiq.io".into();
        assert!(is_meeting_recap_artifact(
            &prepare(&tactiq, "Inbox", 0).unwrap()
        ));

        let mut copilot = synthetic("Synthetic recap", 1, "teams-thread");
        copilot.sender = "Teams Copilot <notes@service.example.invalid>".into();
        copilot.sender_address = "notes@service.example.invalid".into();
        assert!(is_meeting_recap_artifact(
            &prepare(&copilot, "Inbox", 1).unwrap()
        ));
    }

    #[test]
    fn recap_meeting_time_uses_event_subject_body_then_message_timestamp() {
        let event_start = timestamp("2026-08-21T15:00:00Z");
        let subject_start = timestamp("2026-08-21T16:30:00Z");

        let mut mail = synthetic(
            "The meeting was September 3, 2026 at 2pm.",
            0,
            "meeting-time",
        );
        mail.subject =
            "Accepted: Design workshop @ Fri Aug 21, 2026 11:30am - 12:30pm (CDT)".into();
        let mut message = prepare(&mail, "Inbox", 0).unwrap();
        message.event = Some((event_start, event_start + 3600, false));
        assert_eq!(recap_meeting_time(&message), Some((event_start, false)));

        message.event = Some((event_start, event_start + 3600, true));
        assert_eq!(recap_meeting_time(&message), Some((subject_start, false)));

        let mut body_mail = synthetic(
            "The meeting was September 3, 2026 at 2pm.",
            0,
            "meeting-time-body",
        );
        body_mail.subject = "Meeting recap".into();
        let body_message = prepare(&body_mail, "Inbox", 0).unwrap();
        let body_start = prose_event_time(
            "The meeting was September 3, 2026 at 2pm.",
            body_message.input.timestamp,
            local_offset_seconds(body_message.input.timestamp, 0),
        )
        .unwrap()
        .0;
        assert_eq!(recap_meeting_time(&body_message), Some((body_start, false)));

        let fallback = prepare(
            &synthetic("No stated meeting time.", 1, "meeting-time-fallback"),
            "Inbox",
            1,
        )
        .unwrap();
        assert_eq!(
            recap_meeting_time(&fallback),
            Some((fallback.input.timestamp, true))
        );
    }

    #[test]
    fn a_platform_notetaker_address_needs_a_recap_subject_too() {
        let mut ordinary = synthetic("Here's the invite for next week.", 0, "zoom-thread");
        ordinary.subject = "Your Zoom meeting is ready".into();
        ordinary.sender = "Zoom <no-reply@zoom.us>".into();
        ordinary.sender_address = "no-reply@zoom.us".into();
        let ordinary_message = prepare(&ordinary, "Inbox", 0).unwrap();
        assert!(
            !is_meeting_recap_artifact(&ordinary_message),
            "a platform notetaker address alone, without a recap-shaped subject, is not enough"
        );

        let mut recap = synthetic("Full call notes attached.", 1, "zoom-thread");
        recap.subject = "Zoom Meeting Recap".into();
        recap.sender = "Zoom <no-reply@zoom.us>".into();
        recap.sender_address = "no-reply@zoom.us".into();
        let recap_message = prepare(&recap, "Inbox", 1).unwrap();
        assert!(is_meeting_recap_artifact(&recap_message));
    }

    #[test]
    fn send_me_your_notes_is_not_a_recap_artifact() {
        let mut mail = synthetic("Sure, here they are.", 0, "notes-thread");
        mail.subject = "Send me your notes".into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        assert!(!is_meeting_recap_artifact(&message));
    }

    #[test]
    fn an_ordinary_calendar_invite_is_not_a_recap_artifact() {
        let event_message = scoped_event_message();
        assert!(!is_meeting_recap_artifact(&event_message));
    }

    /// End-to-end regression for the Fathom-shaped defect: the recap message
    /// contributes no event to the index at all (belt-and-braces alongside
    /// the temporal rule), and its action item -- extracted as an open loop,
    /// which is the whole point of the product -- stays open.
    #[test]
    fn fathom_recap_contributes_no_event_and_its_action_item_stays_open() {
        let mut mail = synthetic(
            "Meeting Purpose\nCatch up on the neurosymbolic AI project.\n\
Key Takeaways\nGood progress overall.\n\
Action Items\nSend Thomas the resources on neurosymbolic AI and the Leavenitz link.",
            0,
            "fathom-thread",
        );
        mail.subject = "Liat and Erin catch up (Erin Fraser)".into();
        mail.sender = "Fathom <no-reply@fathom.video>".into();
        mail.sender_address = "no-reply@fathom.video".into();
        let mut message = prepare(&mail, "Inbox", 0).unwrap();
        // Graph-supplied meeting metadata, as a real Fathom-linked calendar
        // event might carry -- the recap check must skip it as an event
        // source regardless of where the timing would otherwise come from.
        message.event = Some((
            timestamp("2026-09-08T13:00:00Z"),
            timestamp("2026-09-08T14:00:00Z"),
            false,
        ));
        message.input.timestamp = timestamp("2026-09-08T15:30:00Z"); // sent after the meeting

        assert!(build_event_index(std::slice::from_ref(&message)).is_empty());

        let (_, template) = closure_test_messages();
        let item = scoped_event_expectation(
            &template,
            &message.input.handle,
            "Send Thomas the resources on neurosymbolic AI and the Leavenitz link",
            "Send Thomas the resources on neurosymbolic AI and the Leavenitz link.",
        );
        let mut result = closure_result(vec![item]);

        close_passed_events(
            &mut result,
            std::slice::from_ref(&message),
            timestamp("2026-09-09T15:30:00Z"),
        );

        assert!(result.analysis.items[0].event_passed.is_none());
        assert_eq!(result.event_closures, 0);
    }

    #[test]
    fn recap_artifact_never_closes_even_with_an_anchor() {
        const EVENT_TIME: &str = "Tuesday, September 2, 2026, 9:00 AM \u{2013} 12:00 PM PT";
        let body = format!(
            "Meeting Purpose\nClaude Code Workshop SF\n{EVENT_TIME}\n\
             Action Items\nReview the schedule, venue & logistics."
        );
        let mut mail = synthetic(&body, 0, "anchored-fathom-thread");
        mail.received = "2026-09-03T16:00:00Z".into();
        mail.subject = "Claude Code Workshop SF - Meeting Summary".into();
        mail.sender = "Fathom <no-reply@fathom.video>".into();
        mail.sender_address = "no-reply@fathom.video".into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        assert!(build_event_index(std::slice::from_ref(&message)).is_empty());

        let (_, template) = closure_test_messages();
        let mut item = scoped_event_expectation(
            &template,
            &message.input.handle,
            "Review the schedule, venue & logistics",
            "Review the schedule, venue & logistics.",
        );
        item.event = Some(Anchor {
            message: message.input.handle.clone(),
            block: 0,
            quote: "Claude Code Workshop SF".into(),
            context: body.clone(),
        });
        item.event_time = Some(Anchor {
            message: message.input.handle.clone(),
            block: 0,
            quote: EVENT_TIME.into(),
            context: body,
        });
        let mut result = closure_result(vec![item]);

        close_passed_events(
            &mut result,
            std::slice::from_ref(&message),
            timestamp("2026-09-10T00:00:00Z"),
        );

        assert!(result.analysis.items[0].event_passed.is_none());
        assert_eq!(result.event_closures, 0);
    }

    /// Negative case for the recap fix: an ordinary calendar invite with
    /// attendance-shaped language, sent before its event, must still close
    /// after the event -- the recap skip in `learn_one_event` must not
    /// suppress normal event evidence.
    #[test]
    fn an_ordinary_invite_with_attend_language_still_closes_after_its_event() {
        let event_message = scoped_event_message();
        let event_end = build_event_index(std::slice::from_ref(&event_message))[0].end;
        let (_, template) = closure_test_messages();
        let item = scoped_event_expectation(
            &template,
            &event_message.input.handle,
            "Install the tool before arriving at the workshop",
            "Install the tool before arriving at the workshop.",
        );
        let mut result = closure_result(vec![item]);

        close_passed_events(
            &mut result,
            std::slice::from_ref(&event_message),
            event_end + 1,
        );

        assert!(result.analysis.items[0].event_passed.is_some());
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

    /// A synthetic [`ModelClient`] whose answer is computed from the user
    /// payload of each request, standing in for a provider in closure-pass
    /// tests. `calls` counts requests and `payloads` keeps each user payload.
    type ScriptedAnswer<'a> =
        dyn Fn(usize, &str) -> Result<String, ProviderError> + Send + Sync + 'a;

    struct ScriptedClient<'a> {
        answer: Box<ScriptedAnswer<'a>>,
        calls: AtomicUsize,
        payloads: Mutex<Vec<String>>,
    }

    impl<'a> ScriptedClient<'a> {
        fn new(
            answer: impl Fn(usize, &str) -> Result<String, ProviderError> + Send + Sync + 'a,
        ) -> Self {
            Self {
                answer: Box::new(answer),
                calls: AtomicUsize::new(0),
                payloads: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }

        fn payload(&self, index: usize) -> String {
            self.payloads.lock().unwrap_or_else(PoisonError::into_inner)[index].clone()
        }
    }

    impl ModelClient for ScriptedClient<'_> {
        fn model(&self) -> &'static str {
            "scripted"
        }

        fn max_parallel(&self) -> usize {
            1
        }

        fn complete(
            &self,
            _system: &str,
            user: &str,
            _cancel: Option<&AtomicBool>,
            _deadline: std::time::Duration,
        ) -> Result<zeroize::Zeroizing<String>, ProviderError> {
            let index = self.calls.fetch_add(1, Ordering::Relaxed);
            self.payloads
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(user.to_string());
            (self.answer)(index, user).map(zeroize::Zeroizing::new)
        }
    }

    struct FixedDecisionValues {
        probabilities: Vec<f64>,
        choice: Option<(String, f64, f64)>,
    }

    type FixedDecisionAnswer<'a> = dyn Fn(usize, &serde_json::Value) -> Result<FixedDecisionValues, ProviderError>
        + Send
        + Sync
        + 'a;

    struct FixedDecisionClient<'a> {
        answer: Box<FixedDecisionAnswer<'a>>,
        calls: AtomicUsize,
        states: Mutex<Vec<serde_json::Value>>,
        parallel: usize,
    }

    impl<'a> FixedDecisionClient<'a> {
        fn new(
            answer: impl Fn(usize, &serde_json::Value) -> Result<[f64; 4], ProviderError>
            + Send
            + Sync
            + 'a,
        ) -> Self {
            Self {
                answer: Box::new(move |index, state| {
                    answer(index, state).map(|values| FixedDecisionValues {
                        probabilities: values.to_vec(),
                        choice: Some(("none".into(), 0.5, 0.5)),
                    })
                }),
                calls: AtomicUsize::new(0),
                states: Mutex::new(Vec::new()),
                parallel: 1,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }

        fn state(&self, index: usize) -> serde_json::Value {
            self.states.lock().unwrap_or_else(PoisonError::into_inner)[index].clone()
        }

        fn triage(
            answer: impl Fn(usize, &serde_json::Value) -> Result<[f64; 6], ProviderError>
            + Send
            + Sync
            + 'a,
        ) -> Self {
            Self {
                answer: Box::new(move |index, state| {
                    answer(index, state).map(|values| FixedDecisionValues {
                        probabilities: values.to_vec(),
                        choice: None,
                    })
                }),
                calls: AtomicUsize::new(0),
                states: Mutex::new(Vec::new()),
                parallel: 1,
            }
        }

        fn closure_choice(
            answer: impl Fn(
                usize,
                &serde_json::Value,
            ) -> Result<([f64; 4], &'static str, f64, f64), ProviderError>
            + Send
            + Sync
            + 'a,
        ) -> Self {
            Self {
                answer: Box::new(move |index, state| {
                    answer(index, state).map(
                        |(probabilities, choice, choice_probability, confidence)| {
                            FixedDecisionValues {
                                probabilities: probabilities.to_vec(),
                                choice: Some((choice.into(), choice_probability, confidence)),
                            }
                        },
                    )
                }),
                calls: AtomicUsize::new(0),
                states: Mutex::new(Vec::new()),
                parallel: 1,
            }
        }
    }

    impl DecisionClient for FixedDecisionClient<'_> {
        fn model(&self) -> &'static str {
            "fixed"
        }

        fn max_parallel(&self) -> usize {
            self.parallel
        }

        fn decide(
            &self,
            state: &serde_json::Value,
            questions: &Questions,
            _cancel: Option<&AtomicBool>,
            deadline: Duration,
        ) -> Result<openloops_inference::decision::Answers, ProviderError> {
            assert_eq!(deadline, DECISION_DEADLINE);
            let call = self.calls.fetch_add(1, Ordering::Relaxed);
            self.states
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(state.clone());
            let values = (self.answer)(call, state)?;
            let request =
                openloops_inference::decision::request_body(self.model(), state, questions, false)?;
            let request: serde_json::Value =
                serde_json::from_slice(&request).map_err(|_| ProviderError::InvalidQuestion)?;
            let ids = request["questions"]
                .as_object()
                .ok_or(ProviderError::InvalidQuestion)?
                .keys();
            let mut answers = serde_json::Map::new();
            for id in ids {
                if id.ends_with(".outcome") {
                    let (choice, probability, confidence) = values
                        .choice
                        .as_ref()
                        .ok_or(ProviderError::InvalidQuestion)?;
                    let mut choice_probabilities = serde_json::Map::new();
                    for option in [
                        "fulfilled",
                        "withdrawn",
                        "deadline_changed",
                        "modified",
                        "none",
                    ] {
                        choice_probabilities.insert(
                            option.into(),
                            serde_json::Value::from(if option == choice {
                                *probability
                            } else {
                                0.0
                            }),
                        );
                    }
                    answers.insert(
                        id.clone(),
                        serde_json::json!({
                            "type": "choice",
                            "choice": choice,
                            "probabilities": choice_probabilities,
                            "confidence": confidence
                        }),
                    );
                    continue;
                }
                let probability = if values.probabilities.len() == DecisionOutcome::ALL.len() {
                    DecisionOutcome::ALL
                        .iter()
                        .position(|outcome| id.ends_with(outcome.suffix()))
                        .map(|index| values.probabilities[index])
                } else {
                    TRIAGE_IDS
                        .iter()
                        .position(|candidate| id == candidate)
                        .map(|index| values.probabilities[index])
                }
                .ok_or(ProviderError::InvalidQuestion)?;
                answers.insert(
                    id.clone(),
                    serde_json::json!({"type":"noul", "noul":probability}),
                );
            }
            let response = serde_json::json!({
                "model": self.model(),
                "answers": answers,
                "usage": {"input_tokens": 1}
            });
            openloops_inference::decision::parse_answers(
                &serde_json::to_vec(&response).map_err(|_| ProviderError::InvalidResponse)?,
                self.model(),
                questions,
            )
        }
    }

    type RuleAnswer<'a> = dyn Fn(&str, &serde_json::Value) -> Result<serde_json::Value, ProviderError>
        + Send
        + Sync
        + 'a;

    struct FixedRuleDecisionClient<'a> {
        answer: Box<RuleAnswer<'a>>,
        calls: AtomicUsize,
        states: Mutex<Vec<serde_json::Value>>,
    }

    impl<'a> FixedRuleDecisionClient<'a> {
        fn new(
            answer: impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, ProviderError>
            + Send
            + Sync
            + 'a,
        ) -> Self {
            Self {
                answer: Box::new(answer),
                calls: AtomicUsize::new(0),
                states: Mutex::new(Vec::new()),
            }
        }

        fn noul(probability: f64) -> Self {
            Self::new(move |_, _| Ok(serde_json::json!({"type":"noul", "noul":probability})))
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }

        fn state(&self, index: usize) -> serde_json::Value {
            self.states.lock().unwrap_or_else(PoisonError::into_inner)[index].clone()
        }
    }

    impl DecisionClient for FixedRuleDecisionClient<'_> {
        fn model(&self) -> &'static str {
            "fixed-rule"
        }

        fn max_parallel(&self) -> usize {
            1
        }

        fn decide(
            &self,
            state: &serde_json::Value,
            questions: &Questions,
            _cancel: Option<&AtomicBool>,
            deadline: Duration,
        ) -> Result<openloops_inference::decision::Answers, ProviderError> {
            assert_eq!(deadline, DECISION_DEADLINE);
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.states
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(state.clone());
            let request =
                openloops_inference::decision::request_body(self.model(), state, questions, false)?;
            let request: serde_json::Value =
                serde_json::from_slice(&request).map_err(|_| ProviderError::InvalidQuestion)?;
            let question = request["questions"]
                .as_object()
                .and_then(|values| values.keys().next())
                .ok_or(ProviderError::InvalidQuestion)?;
            let answer = (self.answer)(question, state)?;
            let response = serde_json::json!({
                "model": self.model(),
                "answers": {(question): answer},
                "usage": {"input_tokens": 1}
            });
            openloops_inference::decision::parse_answers(
                &serde_json::to_vec(&response).map_err(|_| ProviderError::InvalidResponse)?,
                self.model(),
                questions,
            )
        }
    }

    fn answer_rules(
        id: &str,
        category: RuleCategory,
        states: Vec<serde_json::Value>,
        client: &dyn DecisionClient,
    ) -> RuleDecisions {
        let mut rules = RuleDecisions::default();
        decide_rule_states(
            id,
            category,
            states,
            &mut rules,
            client,
            &ScanProgress::default(),
        )
        .unwrap();
        rules
    }

    #[test]
    fn rule_fast_paths_create_no_decision_state() {
        let mut vendor = synthetic("Ordinary text.", 0, "vendor");
        vendor.sender = "Service <notes@fathom.video>".into();
        vendor.sender_address = "notes@fathom.video".into();
        let vendor = prepare(&vendor, "Inbox", 0).unwrap();
        assert!(is_meeting_recap_artifact(&vendor));
        assert!(recap_rule_states(&[vendor]).is_empty());

        let mut item = expectation_for("m0");
        item.action = "Attend the workshop".into();
        assert!(has_scoped_event_language(&item));
        assert!(duplicate_rule_states(&[item.clone(), item.clone()]).is_empty());

        let event = EventRef {
            name: "Quarterly planning workshop".into(),
            start: 100,
            end: 200,
            conversation: "event".into(),
            message_handle: "event-message".into(),
            source: EventSource::Meeting,
        };
        let mut states = Vec::new();
        collect_event_match_states(&mut states, "quarterly planning", 0, &[event], false);
        assert!(states.is_empty());

        item.deadline = Some(Anchor {
            message: "m0".into(),
            block: 0,
            quote: "Friday".into(),
            context: String::new(),
        });
        let message = prepare(&synthetic("Synthetic.", 0, "deadline"), "Inbox", 0).unwrap();
        item.deadline.as_mut().unwrap().message = message.input.handle.clone();
        assert!(deadline_rule_states(&[item], &[message]).is_empty());

        let mut threads = vec![
            prepare(&synthetic("First.", 0, "a"), "Inbox", 0).unwrap(),
            prepare(&synthetic("Second.", 1, "b"), "Inbox", 1).unwrap(),
        ];
        assert_eq!(merge_threads(&mut threads), 1);
        assert!(thread_rule_states(&threads).is_empty());
    }

    #[test]
    fn recap_residue_is_byte_exact_cached_and_accept_only() {
        let mut mail = synthetic("A short update.", 0, "recap-residue");
        mail.subject = "Project recap".into();
        let message = prepare(&mail, "Inbox", 0).unwrap();
        let states = recap_rule_states(std::slice::from_ref(&message));
        assert_eq!(states.len(), 1);

        let accepted = FixedRuleDecisionClient::noul(0.9);
        let mut rules = answer_rules(
            RULE_RECAP,
            RuleCategory::Recap,
            vec![states[0].clone(), states[0].clone()],
            &accepted,
        );
        decide_rule_states(
            RULE_RECAP,
            RuleCategory::Recap,
            states,
            &mut rules,
            &accepted,
            &ScanProgress::default(),
        )
        .unwrap();
        assert_eq!(accepted.calls(), 1);
        assert_eq!(
            serde_json::to_vec(&accepted.state(0)).unwrap(),
            br#"{"sender":"Alex <alex@example.invalid>","subject":"Project recap","first_paragraph":"A short update."}"#
        );
        assert!(is_meeting_recap_artifact_with_rules(&message, Some(&rules)));
        let mut analysis = LoopItems {
            items: vec![expectation_for(&message.input.handle)],
            rejected: 0,
            rejection_reasons: Vec::new(),
            degraded: 0,
        };
        analysis.items[0].waiting_party = "user@example.invalid".into();
        correct_recap_attribution(&mut analysis, &[&message], Some(&rules));
        tag_call_summaries(&mut analysis, &[&message], Some(&rules));
        assert_eq!(analysis.items[0].waiting_party, "Not established");
        assert!(analysis.items[0].owner == Owner::You);
        assert!(analysis.items[0].from_call_summary);

        for probability in [0.49, 0.1] {
            let client = FixedRuleDecisionClient::noul(probability);
            let fallback = answer_rules(
                RULE_RECAP,
                RuleCategory::Recap,
                vec![recap_state(&message)],
                &client,
            );
            assert!(!is_meeting_recap_artifact_with_rules(
                &message,
                Some(&fallback)
            ));
        }
        let error = FixedRuleDecisionClient::new(|_, _| Err(ProviderError::RateLimited));
        let fallback = answer_rules(
            RULE_RECAP,
            RuleCategory::Recap,
            vec![recap_state(&message)],
            &error,
        );
        assert!(!is_meeting_recap_artifact_with_rules(
            &message,
            Some(&fallback)
        ));
        assert_eq!((error.calls(), fallback.skipped), (1, 1));
    }

    #[test]
    fn event_rule_residues_accept_and_other_bands_keep_today_behavior() {
        let mut item = expectation_for("m0");
        item.action = "Coordinate the packet".into();
        item.evidence.quote = "Please prepare the packet.".into();
        let scoped_state = scoped_event_state(&item);
        assert!(!has_scoped_event_language(&item));
        let accepted = FixedRuleDecisionClient::noul(0.9);
        let rules = answer_rules(
            RULE_SCOPED_EVENT,
            RuleCategory::Event,
            vec![scoped_state.clone()],
            &accepted,
        );
        assert!(has_scoped_event_language_with_rules(&item, Some(&rules)));
        assert_eq!(
            serde_json::to_vec(&accepted.state(0)).unwrap(),
            br#"{"request_text":"Coordinate the packet Please prepare the packet."}"#
        );

        let event = EventRef {
            name: "Planning workshop schedule".into(),
            start: 100,
            end: 200,
            conversation: "event".into(),
            message_handle: "event-message".into(),
            source: EventSource::Meeting,
        };
        assert!(match_event("planning request", 0, std::slice::from_ref(&event)).is_none());
        let state = event_match_state("planning request", &event.name);
        let rules = answer_rules(
            RULE_EVENT_MATCH,
            RuleCategory::Event,
            vec![state],
            &FixedRuleDecisionClient::noul(0.9),
        );
        assert!(match_event_with_rules("planning request", 0, &[event], Some(&rules)).is_some());
        let text_event = EventRef {
            name: "Planning workshop schedule".into(),
            start: 100,
            end: 200,
            conversation: "event".into(),
            message_handle: "event-message".into(),
            source: EventSource::Meeting,
        };
        assert!(
            match_event_by_text("planning request", 0, std::slice::from_ref(&text_event)).is_none()
        );
        assert!(
            match_event_by_text_with_rules("planning request", 0, &[text_event], Some(&rules))
                .is_some()
        );

        for (id, gray) in [(RULE_SCOPED_EVENT, 0.8), (RULE_EVENT_MATCH, 0.69)] {
            for probability in [gray, 0.1] {
                let state = if id == RULE_SCOPED_EVENT {
                    scoped_state.clone()
                } else {
                    event_match_state("planning request", "Planning workshop schedule")
                };
                let rules = answer_rules(
                    id,
                    RuleCategory::Event,
                    vec![state.clone()],
                    &FixedRuleDecisionClient::noul(probability),
                );
                assert!(!rules.noul(id, &state));
            }
        }
    }

    #[test]
    fn duplicate_and_thread_residues_apply_only_on_accept() {
        let first = expectation_for("m0");
        let mut second = first.clone();
        second.action = "Provide the draft".into();
        let states = duplicate_rule_states(&[first.clone(), second.clone()]);
        assert_eq!(states.len(), 1);
        let client = FixedRuleDecisionClient::noul(0.9);
        let rules = answer_rules(
            RULE_DUPLICATE_ACTION,
            RuleCategory::Duplicate,
            states,
            &client,
        );
        let mut items = vec![first, second];
        apply_duplicate_rules(&mut items, &rules);
        assert_eq!(items.len(), 1);
        assert_eq!(
            serde_json::to_vec(&client.state(0)).unwrap(),
            br#"{"action_a":"Send the draft","action_b":"Provide the draft"}"#
        );

        let mut a = synthetic("First paragraph.", 0, "thread-a");
        a.subject = "Hi".into();
        let mut b = synthetic("Second paragraph.", 1, "thread-b");
        b.subject = "Hi".into();
        let mut messages = vec![
            prepare(&a, "Inbox", 0).unwrap(),
            prepare(&b, "Inbox", 1).unwrap(),
        ];
        assert_eq!(merge_threads(&mut messages.clone()), 0);
        let states = thread_rule_states(&messages);
        assert_eq!(states.len(), 1);
        let client = FixedRuleDecisionClient::noul(0.9);
        let rules = answer_rules(RULE_THREAD_MERGE, RuleCategory::Thread, states, &client);
        assert_eq!(merge_threads_with_rules(&mut messages, Some(&rules)), 1);
        assert_eq!(
            serde_json::to_vec(&client.state(0)).unwrap(),
            br#"{"subject_a":"Hi","subject_b":"Hi","first_paragraph_a":"First paragraph.","first_paragraph_b":"Second paragraph."}"#
        );
    }

    #[test]
    fn deadline_choice_is_stored_and_rendering_consults_it() {
        let message = prepare(&synthetic("Synthetic.", 0, "deadline"), "Inbox", 0).unwrap();
        let mut item = expectation_for(&message.input.handle);
        item.deadline = Some(Anchor {
            message: message.input.handle.clone(),
            block: 0,
            quote: "before kickoff".into(),
            context: String::new(),
        });
        let states =
            deadline_rule_states(std::slice::from_ref(&item), std::slice::from_ref(&message));
        let client = FixedRuleDecisionClient::new(|_, _| {
            Ok(serde_json::json!({
                "type":"choice",
                "choice":"event_tied",
                "probabilities":{"event_tied":0.9,"soft":0.05,"unknown":0.05},
                "confidence":0.9
            }))
        });
        let rules = answer_rules(RULE_DEADLINE_KIND, RuleCategory::Deadline, states, &client);
        apply_deadline_rules(
            std::slice::from_mut(&mut item),
            std::slice::from_ref(&message),
            &rules,
        );
        assert_eq!(item.deadline_kind_hint, Some(DeadlineKindHint::EventTied));
        assert_eq!(
            classify_with_hint(
                "before kickoff",
                message.input.timestamp,
                message.input.timestamp,
                0,
                item.deadline_kind_hint,
            ),
            DeadlineView::EventTied
        );
        assert_eq!(
            serde_json::to_vec(&client.state(0)).unwrap(),
            br#"{"phrase":"before kickoff"}"#
        );
        for confidence in [0.69, 0.1] {
            let client = FixedRuleDecisionClient::new(move |_, _| {
                Ok(serde_json::json!({
                    "type":"choice",
                    "choice":"event_tied",
                    "probabilities":{"event_tied":0.9,"soft":0.05,"unknown":0.05},
                    "confidence":confidence
                }))
            });
            let states =
                deadline_rule_states(std::slice::from_ref(&item), std::slice::from_ref(&message));
            let rules = answer_rules(RULE_DEADLINE_KIND, RuleCategory::Deadline, states, &client);
            let mut fallback = item.clone();
            fallback.deadline_kind_hint = None;
            apply_deadline_rules(
                std::slice::from_mut(&mut fallback),
                std::slice::from_ref(&message),
                &rules,
            );
            assert_eq!(fallback.deadline_kind_hint, None);
        }
    }

    #[test]
    fn rule_counter_note_is_content_free_and_exact() {
        let rules = RuleDecisions {
            recap: 1,
            event: 2,
            duplicates: 3,
            threads: 4,
            deadlines: 5,
            skipped: 6,
            ..RuleDecisions::default()
        };
        assert_eq!(
            rules.note(),
            "Decision model answered 15 rule questions (recap 1, event 2, duplicates 3, threads 4, deadlines 5); 6 skipped."
        );
    }

    #[test]
    fn every_rule_gray_reject_and_error_falls_back() {
        for id in [
            RULE_RECAP,
            RULE_SCOPED_EVENT,
            RULE_EVENT_MATCH,
            RULE_DUPLICATE_ACTION,
            RULE_THREAD_MERGE,
        ] {
            let registered = Registry::get().question(id).unwrap();
            let state = serde_json::json!({"synthetic": id});
            for probability in [registered.accept.midpoint(registered.escalate), 0.0] {
                let rules = answer_rules(
                    id,
                    RuleCategory::Event,
                    vec![state.clone()],
                    &FixedRuleDecisionClient::noul(probability),
                );
                assert!(!rules.noul(id, &state), "{id}");
            }
            let error = FixedRuleDecisionClient::new(|_, _| Err(ProviderError::RateLimited));
            let rules = answer_rules(id, RuleCategory::Event, vec![state.clone()], &error);
            assert!(!rules.noul(id, &state), "{id}");
            assert_eq!((error.calls(), rules.skipped), (1, 1));
        }

        let state = serde_json::json!({"phrase":"before kickoff"});
        for confidence in [0.69, 0.1] {
            let client = FixedRuleDecisionClient::new(move |_, _| {
                Ok(serde_json::json!({
                    "type":"choice",
                    "choice":"soft",
                    "probabilities":{"event_tied":0.05,"soft":0.9,"unknown":0.05},
                    "confidence":confidence
                }))
            });
            let rules = answer_rules(
                RULE_DEADLINE_KIND,
                RuleCategory::Deadline,
                vec![state.clone()],
                &client,
            );
            assert_eq!(rules.choice(RULE_DEADLINE_KIND, &state), None);
        }
        let error = FixedRuleDecisionClient::new(|_, _| Err(ProviderError::RateLimited));
        let rules = answer_rules(
            RULE_DEADLINE_KIND,
            RuleCategory::Deadline,
            vec![state.clone()],
            &error,
        );
        assert_eq!(rules.choice(RULE_DEADLINE_KIND, &state), None);
        assert_eq!((error.calls(), rules.skipped), (1, 1));
    }

    fn triage_message(body: &str) -> ReviewMessage {
        prepare(&synthetic(body, 0, "triage-thread"), "Inbox", 0).unwrap()
    }

    fn run_triage(messages: &[ReviewMessage], decision: &dyn DecisionClient) -> TriageResult {
        triage_pass(messages, &ScanProgress::default(), None, decision).unwrap()
    }

    #[test]
    fn triage_request_state_is_byte_exact_and_signal_keeps_the_conversation() {
        let message = triage_message("Please send the synthetic report.");
        let decision = FixedDecisionClient::triage(|_, _| Ok([0.8, 0.1, 0.1, 0.1, 0.1, 0.1]));

        let triage = run_triage(std::slice::from_ref(&message), &decision);

        assert_eq!(decision.calls(), 1);
        assert_eq!(
            serde_json::to_vec(&decision.state(0)).unwrap(),
            br#"{"subject":"Synthetic budget conversation","paragraph_text":"Please send the synthetic report.","from_user":false}"#
        );
        assert!(triage.skipped.is_empty());
    }

    #[test]
    fn triage_all_negative_skips_but_a_gray_band_keeps_the_conversation() {
        let message = triage_message("Synthetic status only.");
        let negative = FixedDecisionClient::triage(|_, _| Ok([0.1; 6]));
        let gray = FixedDecisionClient::triage(|_, _| Ok([0.47, 0.1, 0.1, 0.1, 0.1, 0.1]));

        assert_eq!(
            run_triage(std::slice::from_ref(&message), &negative)
                .skipped
                .len(),
            1
        );
        assert!(run_triage(&[message], &gray).skipped.is_empty());
    }

    #[test]
    fn triage_drops_boilerplate_without_compacting_projection_ordinals() {
        let message = triage_message("Keep this paragraph.\n\nSynthetic footer.\n\nKeep this too.");
        let decision = FixedDecisionClient::triage(|_, state| {
            if state["paragraph_text"] == "Synthetic footer." {
                Ok([0.1, 0.1, 0.1, 0.1, 0.9, 0.1])
            } else {
                Ok([0.8, 0.1, 0.1, 0.1, 0.1, 0.1])
            }
        });
        let triage = run_triage(std::slice::from_ref(&message), &decision);
        let chat = ScriptedClient::new(|_, _| Ok(EMPTY_CLAIMS.to_string()));
        governed_pass_omitting(
            &chat,
            std::slice::from_ref(&message.input),
            None,
            &triage.omitted_body_blocks,
        )
        .unwrap();
        let payload =
            openloops_contracts::parse_strict_json(&payload_json(&chat.payload(0))).unwrap();
        let ordinals: Vec<u64> = payload["messages"][0]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|block| block["component"] == "body_block")
            .map(|block| block["block_ordinal"].as_u64().unwrap())
            .collect();

        assert_eq!(ordinals, [0, 2]);
        assert_eq!(triage.boilerplate_dropped, 1);
    }

    #[test]
    fn triage_notification_only_conversation_skips_the_primary_pass() {
        let message = triage_message("Synthetic automated notification.");
        let decision = FixedDecisionClient::triage(|_, _| Ok([0.8, 0.1, 0.1, 0.1, 0.1, 0.9]));

        assert_eq!(run_triage(&[message], &decision).skipped.len(), 1);
    }

    #[test]
    fn triage_paragraph_errors_fail_open_and_rate_limits_are_never_resent() {
        let message = triage_message("Synthetic paragraph.");
        for error in [ProviderError::InvalidResponse, ProviderError::RateLimited] {
            let decision = FixedDecisionClient::triage(move |_, _| Err(error));
            let triage = run_triage(std::slice::from_ref(&message), &decision);
            assert_eq!(decision.calls(), 1);
            assert_eq!(triage.request_errors, 1);
            assert!(triage.skipped.is_empty());
        }
    }

    #[test]
    fn triage_stop_class_error_runs_every_conversation_in_the_primary_pass() {
        let mut messages = [
            triage_message("First synthetic paragraph."),
            triage_message("Second synthetic paragraph."),
        ];
        messages[1].conversation = "triage-thread-two".into();
        messages[1].input.handle = "m1".into();
        let decision = FixedDecisionClient::triage(|_, _| Err(ProviderError::Network));
        let triage = run_triage(&messages, &decision);
        let chat = ScriptedClient::new(|_, _| Ok(EMPTY_CLAIMS.to_string()));
        super::scan_conversations(
            &messages,
            &ScanProgress::default(),
            &ParallelPass::new(1),
            &|conversation| {
                governed_pass_omitting(&chat, conversation, None, &triage.omitted_body_blocks)
            },
        );

        assert!(triage.stop_class_error);
        assert!(triage.skipped.is_empty());
        assert!(triage.omitted_body_blocks.is_empty());
        assert_eq!(chat.calls(), 2);
    }

    #[test]
    fn triage_caps_each_message_at_forty_paragraph_requests() {
        let mut message = triage_message("Synthetic paragraph.");
        message.input.message.body_blocks = (0..45)
            .map(|index| CanonicalBlock::new(&format!("Synthetic paragraph {index}.")).unwrap())
            .collect();
        let decision = FixedDecisionClient::triage(|_, _| Ok([0.1; 6]));

        run_triage(&[message], &decision);

        assert_eq!(decision.calls(), MAX_BODY_BLOCKS);
    }

    #[test]
    fn triage_coverage_has_counters_reason_and_one_rescan_hint() {
        let message = triage_message("Synthetic footer.");
        let decision = FixedDecisionClient::triage(|_, _| Ok([0.1, 0.1, 0.1, 0.1, 0.9, 0.1]));
        let triage = run_triage(std::slice::from_ref(&message), &decision);
        let mut result = empty_result(0);
        apply_triage_coverage(&mut result, &[message], None, &triage, 1);

        assert!(
            result
                .conversation_notes
                .iter()
                .any(|note| { note.ends_with("no obligations found by triage.") })
        );
        let aggregate = result.conversation_notes.last().unwrap();
        assert!(aggregate.contains("1 conversations skipped by triage"));
        assert!(aggregate.contains("1 boilerplate paragraphs dropped"));
        assert!(aggregate.contains("0 triage requests skipped"));
        assert_eq!(aggregate.matches("Rescan").count(), 1);
    }

    const EMPTY_CLAIMS: &str = r#"{"schema_version":1,"claims":[]}"#;

    /// One `analysis-output-v1` claim citing the whole body block 0 of
    /// `message` (`length` scalars).
    fn closure_claim(
        claim_type: &str,
        message: &str,
        length: usize,
        loops: &[&str],
        temporal: Option<(&str, &str)>,
        confidence: u32,
    ) -> String {
        let loops = loops
            .iter()
            .map(|handle| format!("\"{handle}\""))
            .collect::<Vec<_>>()
            .join(",");
        let temporal = temporal.map_or_else(
            || "null".to_string(),
            |(kind, value)| {
                format!("{{\"text_evidence_index\":0,\"kind\":\"{kind}\",\"value\":\"{value}\"}}")
            },
        );
        format!(
            "{{\"claim_type\":\"{claim_type}\",\"evidence\":[{{\"source_handle\":\"{message}\",\"component\":\"body_block\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":{length}}}],\"waiting_party_handle\":null,\"related_loop_handles\":[{loops}],\"temporal\":{temporal},\"confidence_micros\":{confidence},\"ambiguity_codes\":[]}}"
        )
    }

    fn claims_document(claims: &[String]) -> String {
        format!("{{\"schema_version\":1,\"claims\":[{}]}}", claims.join(","))
    }

    /// The JSON document inside a framed request payload.
    fn payload_json(payload: &str) -> Vec<u8> {
        payload.split_once('\n').unwrap().1.as_bytes().to_vec()
    }

    /// The `(source_handle, scalar_length)` of the first supplied message
    /// whose first body block reads `text`.
    fn message_with_body(payload: &str, text: &str) -> Option<(String, usize)> {
        let value = openloops_contracts::parse_strict_json(&payload_json(payload)).ok()?;
        value["messages"].as_array()?.iter().find_map(|message| {
            let block = message["blocks"]
                .as_array()?
                .iter()
                .find(|b| b["component"] == "body_block" && b["text"] == text)?;
            Some((
                message["source_handle"].as_str()?.to_string(),
                usize::try_from(block["scalar_length"].as_u64()?).ok()?,
            ))
        })
    }

    /// The `related_loop_handles` a request offered.
    fn offered_handles(payload: &str) -> Vec<String> {
        let value = openloops_contracts::parse_strict_json(&payload_json(payload)).unwrap();
        value["related_loop_handles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|handle| handle.as_str().unwrap().to_string())
            .collect()
    }

    /// The message handles a request supplied, in order.
    fn supplied_messages(payload: &str) -> Vec<String> {
        let value = openloops_contracts::parse_strict_json(&payload_json(payload)).unwrap();
        value["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["source_handle"].as_str().unwrap().to_string())
            .collect()
    }

    /// Answers a closure-pass request with one `possible_closure` for the
    /// first offered loop, citing the reply body [`reply_to`] builds.
    fn close_first_loop(payload: &str) -> String {
        let handles = offered_handles(payload);
        let (message, length) = message_with_body(payload, "Sure, let's do it.").unwrap();
        claims_document(&[closure_claim(
            "possible_closure",
            &message,
            length,
            &[&handles[0]],
            None,
            900_000,
        )])
    }

    fn run_closures(
        all: &[ReviewMessage],
        items: Vec<LoopItem>,
        client: &dyn ModelClient,
        progress: &ScanProgress,
    ) -> ScanResult {
        let mut result = closure_result(items);
        super::scan_closures(all, progress, &mut result, &ParallelPass::new(1), client);
        result
    }

    fn run_decision_closures(
        all: &[ReviewMessage],
        items: Vec<LoopItem>,
        decision: &dyn DecisionClient,
        chat: &dyn ModelClient,
        progress: &ScanProgress,
    ) -> ScanResult {
        let mut result = closure_result(items);
        super::scan_closures_selected(
            all,
            progress,
            &mut result,
            &ParallelPass::new(1),
            chat,
            Some(decision),
        );
        result
    }

    /// The closed request `closure_test_messages` builds in conversation
    /// `c1`, plus a later user reply to the same person in conversation `c2`.
    fn cross_thread_fixture() -> (Vec<ReviewMessage>, LoopItem) {
        let (mut all, item) = closure_test_messages();
        let reply = reply_to("sam@example.invalid", "v-1", "c2", "acct", "Fee");
        all.push(prepare(&reply, "Sent", all.len()).unwrap());
        (all, item)
    }

    fn empty_chat() -> ScriptedClient<'static> {
        ScriptedClient::new(|_, _| Ok(EMPTY_CLAIMS.to_string()))
    }

    #[test]
    fn decision_fulfilled_attaches_the_paragraph_confidence_and_tuned_state_shape() {
        let (all, item) = cross_thread_fixture();
        let decision = FixedDecisionClient::new(|_, _| Ok([0.81, 0.1, 0.1, 0.1]));
        let chat = empty_chat();
        let result =
            run_decision_closures(&all, vec![item], &decision, &chat, &ScanProgress::default());

        assert_eq!(decision.calls(), 1);
        assert_eq!(chat.calls(), 0);
        assert_eq!(
            serde_json::to_vec(&decision.state(0)).unwrap(),
            br#"{"obligation":{"title":"Pay the 350 fee","evidence_text":"Can we meet?"},"later":{"paragraph_text":"Sure, let's do it.","from_user":true,"days_later":1}}"#
        );
        let update = result.analysis.items[0].suggested_update.as_ref().unwrap();
        assert_eq!(update.kind, SuggestedUpdateKind::Closure);
        assert_eq!(update.evidence_text, "Sure, let's do it.");
        assert_eq!(update.source_message, "m1");
        assert_eq!(update.source_block, 0);
        assert_eq!(update.confidence_micros, 810_000);
    }

    #[test]
    fn accepted_outcome_choice_precedes_the_nouls_and_uses_option_probability() {
        let (all, item) = cross_thread_fixture();
        let decision = FixedDecisionClient::closure_choice(|_, _| {
            Ok(([0.1, 0.99, 0.1, 0.1], "fulfilled", 0.42, 0.9))
        });
        let result = run_decision_closures(
            &all,
            vec![item],
            &decision,
            &empty_chat(),
            &ScanProgress::default(),
        );

        let update = result.analysis.items[0].suggested_update.as_ref().unwrap();
        assert_eq!(update.kind, SuggestedUpdateKind::Closure);
        assert_eq!(update.confidence_micros, 420_000);
    }

    #[test]
    fn accepted_none_choice_rejects_the_pair_regardless_of_nouls() {
        let (all, item) = cross_thread_fixture();
        let decision = FixedDecisionClient::closure_choice(|_, _| {
            Ok(([0.99, 0.1, 0.1, 0.1], "none", 0.8, 0.9))
        });
        let chat = empty_chat();
        let result =
            run_decision_closures(&all, vec![item], &decision, &chat, &ScanProgress::default());

        assert!(result.analysis.items[0].suggested_update.is_none());
        assert_eq!(chat.calls(), 0);
    }

    #[test]
    fn gray_outcome_choice_falls_back_to_noul_recombination() {
        let (all, item) = cross_thread_fixture();
        let decision = FixedDecisionClient::closure_choice(|_, _| {
            Ok(([0.81, 0.1, 0.1, 0.1], "none", 0.5, 0.5))
        });
        let result = run_decision_closures(
            &all,
            vec![item],
            &decision,
            &empty_chat(),
            &ScanProgress::default(),
        );

        let update = result.analysis.items[0].suggested_update.as_ref().unwrap();
        assert_eq!(update.kind, SuggestedUpdateKind::Closure);
        assert_eq!(update.confidence_micros, 810_000);
    }

    #[test]
    fn claim_type_from_labels_prioritizes_question_then_request_then_promise() {
        let labels = |asks_question: bool, asks_recipient: bool, commits_sender: bool| {
            serde_json::json!({
                "triage.asks_question": asks_question,
                "triage.asks_recipient": asks_recipient,
                "triage.commits_sender": commits_sender,
            })
            .as_object()
            .unwrap()
            .clone()
        };
        assert_eq!(
            claim_type_from_labels(&labels(true, true, true)),
            "question"
        );
        assert_eq!(
            claim_type_from_labels(&labels(false, true, true)),
            "request"
        );
        assert_eq!(
            claim_type_from_labels(&labels(false, false, true)),
            "promise"
        );
        assert_eq!(claim_type_from_labels(&labels(false, false, false)), "none");
    }

    #[test]
    fn aggregate_comparison_counts_agreement_and_gray_band_per_id() {
        let mut counts = BTreeMap::new();
        let mut labels = BTreeMap::new();
        labels.insert(
            "triage.asks_recipient".to_string(),
            CompareValue::Bool(true),
        );
        labels.insert(
            "extract.claim_type".to_string(),
            CompareValue::Choice("request".into()),
        );
        let mut agree = BTreeMap::new();
        agree.insert(
            "triage.asks_recipient".to_string(),
            (CompareValue::Bool(true), false),
        );
        agree.insert(
            "extract.claim_type".to_string(),
            (CompareValue::Choice("request".into()), true),
        );
        aggregate_comparison(&mut counts, &labels, &agree);

        let mut disagree = BTreeMap::new();
        disagree.insert(
            "triage.asks_recipient".to_string(),
            (CompareValue::Bool(false), false),
        );
        disagree.insert(
            "extract.claim_type".to_string(),
            (CompareValue::Choice("promise".into()), false),
        );
        aggregate_comparison(&mut counts, &labels, &disagree);

        let recipient = counts["triage.asks_recipient"];
        assert_eq!(recipient.n, 2);
        assert_eq!(recipient.agreement, 1);
        assert_eq!(recipient.gray, 0);

        let claim_type = counts["extract.claim_type"];
        assert_eq!(claim_type.n, 2);
        assert_eq!(claim_type.agreement, 1);
        assert_eq!(claim_type.gray, 1);

        // Ids absent from either map are skipped rather than defaulted in.
        assert!(!counts.contains_key("triage.boilerplate"));
    }

    #[test]
    fn format_comparison_is_content_free_and_reports_exact_ratios() {
        let mut counts = BTreeMap::new();
        counts.insert(
            "triage.asks_recipient".to_string(),
            CompareCount {
                n: 4,
                agreement: 3,
                gray: 1,
            },
        );
        let lines = format_comparison(
            &counts,
            Duration::from_millis(120),
            1_000,
            Duration::from_millis(80),
            250,
        );
        let line = lines
            .iter()
            .find(|line| line.starts_with("triage.asks_recipient:"))
            .expect("triage.asks_recipient line present");
        assert_eq!(
            line,
            "triage.asks_recipient: n=4 agreement=0.750 gray_band=0.250 chat_wall_ms=120 \
             chat_input_tokens=1000 jev_wall_ms=80 jev_input_tokens=250"
        );
        // Every configured comparison id gets a line, even with zero support.
        assert_eq!(lines.len(), COMPARE_IDS.len());
        let empty = lines
            .iter()
            .find(|line| line.starts_with("extract.claim_type:"))
            .expect("extract.claim_type line present");
        assert!(empty.contains("n=0 agreement=0.000 gray_band=0.000"));
        for line in &lines {
            for name in ["Synthetic", "@", "fulfilled", "withdrawn"] {
                assert!(!line.contains(name), "line leaked content: {line}");
            }
        }
    }

    #[test]
    fn compare_prediction_flags_the_registry_gray_band_for_nouls_and_choices() {
        let noul_gray = Answer::Noul { probability: 0.47 };
        let (value, gray) = compare_prediction("triage.asks_recipient", &noul_gray).unwrap();
        assert_eq!(value, CompareValue::Bool(false));
        assert!(
            gray,
            "0.47 sits inside triage.asks_recipient's 0.45..0.5 band"
        );

        let noul_accept = Answer::Noul { probability: 0.9 };
        let (_, gray) = compare_prediction("triage.asks_recipient", &noul_accept).unwrap();
        assert!(!gray);

        let choice_gray = Answer::Choice {
            choice: "request".into(),
            probabilities: BTreeMap::from([("request".to_string(), 0.5)]),
            confidence: 0.5,
        };
        let (value, gray) = compare_prediction("extract.claim_type", &choice_gray).unwrap();
        assert_eq!(value, CompareValue::Choice("request".into()));
        assert!(gray, "0.5 sits inside extract.claim_type's 0.3..0.7 band");

        let choice_accept = Answer::Choice {
            choice: "none".into(),
            probabilities: BTreeMap::from([("none".to_string(), 0.95)]),
            confidence: 0.95,
        };
        let (_, gray) = compare_prediction("extract.claim_type", &choice_accept).unwrap();
        assert!(!gray);
    }

    #[test]
    fn decision_withdrawn_wins_by_probability_and_fulfilled_wins_a_tie() {
        let (mut all, item) = closure_test_messages();
        let reply = reply_to("sam@example.invalid", "v-1", "c2", "acct", "Fee");
        let mut reply = prepare(&reply, "Sent", all.len()).unwrap();
        reply.input.message.body_blocks = vec![
            CanonicalBlock::new("Fulfilled paragraph.").unwrap(),
            CanonicalBlock::new("Withdrawn paragraph.").unwrap(),
        ];
        all.push(reply);
        for (withdrawn, expected) in [
            (0.82, "Withdrawn paragraph."),
            (0.8, "Fulfilled paragraph."),
        ] {
            let decision = FixedDecisionClient::new(move |_, state| {
                if state["later"]["paragraph_text"] == "Fulfilled paragraph." {
                    Ok([0.8, 0.1, 0.1, 0.1])
                } else {
                    Ok([0.1, withdrawn, 0.1, 0.1])
                }
            });
            let result = run_decision_closures(
                &all,
                vec![item.clone()],
                &decision,
                &empty_chat(),
                &ScanProgress::default(),
            );
            let update = result.analysis.items[0].suggested_update.as_ref().unwrap();
            assert_eq!(update.kind, SuggestedUpdateKind::Closure);
            assert_eq!(update.evidence_text, expected);
        }
    }

    #[test]
    fn gray_band_escalates_only_that_conversation_and_loop_to_chat() {
        let (all, item) = cross_thread_fixture();
        let decision = FixedDecisionClient::new(|_, _| Ok([0.66, 0.1, 0.1, 0.1]));
        let chat = ScriptedClient::new(|_, user| Ok(close_first_loop(user)));
        let result =
            run_decision_closures(&all, vec![item], &decision, &chat, &ScanProgress::default());

        assert_eq!(chat.calls(), 1);
        assert_eq!(offered_handles(&chat.payload(0)), ["loop-1-m0-b0"]);
        assert_eq!(result.suggested_updates, 1);
        assert!(result.conversation_notes.iter().any(|note| {
            note.contains("0 suggested updates from the decision model, 1 pairs escalated")
        }));
    }

    #[test]
    fn decision_deadline_requires_exactly_one_normalized_candidate() {
        let run = |text: &str| {
            let (mut all, item) = closure_test_messages();
            let reply = reply_to("sam@example.invalid", "v-1", "c2", "acct", "Fee");
            let mut reply = prepare(&reply, "Sent", all.len()).unwrap();
            reply.input.message.body_blocks = vec![CanonicalBlock::new(text).unwrap()];
            all.push(reply);
            let decision = FixedDecisionClient::new(|_, _| Ok([0.1, 0.1, 0.8, 0.1]));
            let chat = ScriptedClient::new(|_, _| Ok(EMPTY_CLAIMS.to_string()));
            let result =
                run_decision_closures(&all, vec![item], &decision, &chat, &ScanProgress::default());
            (result, chat.calls())
        };

        let (single, calls) = run("Please use September 20, 2026.");
        assert_eq!(calls, 0);
        let update = single.analysis.items[0].suggested_update.as_ref().unwrap();
        assert_eq!(update.kind, SuggestedUpdateKind::DeadlineChange);
        assert_eq!(update.temporal_value.as_deref(), Some("2026-09-20"));

        let (none, calls) = run("Please use the later date.");
        assert_eq!(calls, 0);
        assert!(none.analysis.items[0].suggested_update.is_none());

        let (multiple, calls) = run("Use September 20, 2026 or September 21, 2026.");
        assert_eq!(calls, 1);
        assert!(multiple.analysis.items[0].suggested_update.is_none());
    }

    #[test]
    fn decision_rate_limit_is_counted_once_and_never_resent() {
        let (all, item) = cross_thread_fixture();
        let decision = FixedDecisionClient::new(|_, _| Err(ProviderError::RateLimited));
        let result = run_decision_closures(
            &all,
            vec![item],
            &decision,
            &empty_chat(),
            &ScanProgress::default(),
        );

        assert_eq!(decision.calls(), 1);
        assert!(result.conversation_notes.iter().any(|note| {
            note.ends_with("0 pairs escalated to the chat model, 1 skipped (rate limit / errors).")
        }));
    }

    #[test]
    fn decision_cancel_before_dispatch_sends_nothing() {
        let (all, item) = cross_thread_fixture();
        let decision = FixedDecisionClient::new(|_, _| Ok([0.8, 0.1, 0.1, 0.1]));
        let progress = ScanProgress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        let result = run_decision_closures(&all, vec![item], &decision, &empty_chat(), &progress);

        assert_eq!(decision.calls(), 0);
        assert!(result.cancelled);

        let decision = FixedDecisionClient::new(|_, _| Err(ProviderError::Cancelled));
        let chat = empty_chat();
        let result = run_decision_closures(
            &all,
            vec![closure_test_messages().1],
            &decision,
            &chat,
            &ScanProgress::default(),
        );
        assert_eq!(decision.calls(), 1);
        assert_eq!(chat.calls(), 0);
        assert!(result.cancelled);
    }

    #[test]
    fn toggle_off_runs_the_unchanged_chat_pass_without_a_decision_call() {
        let (all, item) = cross_thread_fixture();
        let constructions = AtomicUsize::new(0);
        let connected = maybe_decision_client(
            ScanOptions {
                provider: Provider::OpenRouter,
                use_decision_model: false,
            },
            || {
                constructions.fetch_add(1, Ordering::Relaxed);
                Ok(FixedDecisionClient::new(|_, _| {
                    panic!("decision client must stay unused")
                }))
            },
        )
        .unwrap();
        let chat = ScriptedClient::new(|_, user| Ok(close_first_loop(user)));
        let mut result = closure_result(vec![item]);
        super::scan_closures_selected(
            &all,
            &ScanProgress::default(),
            &mut result,
            &ParallelPass::new(1),
            &chat,
            None,
        );

        assert!(connected.is_none());
        assert_eq!(constructions.load(Ordering::Relaxed), 0);
        assert_eq!(chat.calls(), 1);
        assert_eq!(result.suggested_updates, 1);
    }

    #[test]
    fn decision_requests_only_the_first_eight_body_paragraphs() {
        let (mut all, item) = closure_test_messages();
        let reply = reply_to("sam@example.invalid", "v-1", "c2", "acct", "Fee");
        let mut reply = prepare(&reply, "Sent", all.len()).unwrap();
        reply.input.message.body_blocks = (0..10)
            .map(|index| CanonicalBlock::new(&format!("Paragraph {index}")).unwrap())
            .collect();
        all.push(reply);
        let decision = FixedDecisionClient::new(|_, _| Ok([0.1, 0.1, 0.1, 0.1]));
        run_decision_closures(
            &all,
            vec![item],
            &decision,
            &empty_chat(),
            &ScanProgress::default(),
        );

        assert_eq!(decision.calls(), MAX_CLOSURE_PARAGRAPHS);
        let texts: Vec<String> = (0..decision.calls())
            .map(|index| {
                decision.state(index)["later"]["paragraph_text"]
                    .as_str()
                    .unwrap()
                    .into()
            })
            .collect();
        assert!(!texts.contains(&"Paragraph 8".to_string()));
    }

    #[test]
    fn closure_pass_attaches_a_suggested_update_for_an_offered_handle() {
        let (all, item) = cross_thread_fixture();
        let client = ScriptedClient::new(|_, user| Ok(close_first_loop(user)));
        let progress = ScanProgress::default();
        let result = run_closures(&all, vec![item], &client, &progress);

        assert_eq!(client.calls(), 1);
        let payload = client.payload(0);
        assert_eq!(offered_handles(&payload), vec!["loop-1-m0-b0".to_string()]);
        assert_eq!(
            supplied_messages(&payload),
            vec!["m0", "m1"],
            "the request that stands for the loop is supplied with the reply"
        );

        assert_eq!(result.suggested_updates, 1);
        let item = &result.analysis.items[0];
        assert!(item.resolution.is_none(), "the loop is never auto-closed");
        let update = item.suggested_update.as_ref().unwrap();
        assert_eq!(update.kind, SuggestedUpdateKind::Closure);
        assert_eq!(update.evidence_text, "Sure, let's do it.");
        assert_eq!(update.source_message, "m1");
        assert_eq!(update.source_block, 0);
        assert_eq!(update.temporal_value, None);
        assert_eq!(update.confidence_micros, 900_000);
        assert!(
            result
                .conversation_notes
                .iter()
                .any(|n| n.starts_with("1 suggested update(s)"))
        );
        assert_eq!(progress.total.load(Ordering::Relaxed), 1);
        assert_eq!(progress.processed.load(Ordering::Relaxed), 1);
        assert!(progress.closure_phase.load(Ordering::Relaxed));
    }

    #[test]
    fn a_same_thread_reply_produces_a_suggested_update_from_one_request() {
        let (mut all, item) = closure_test_messages();
        let reply = reply_to("sam@example.invalid", "v-1", "c1", "acct", "Fee");
        all.push(prepare(&reply, "Sent", all.len()).unwrap());
        let client = ScriptedClient::new(|_, user| Ok(close_first_loop(user)));
        let result = run_closures(&all, vec![item], &client, &ScanProgress::default());

        assert_eq!(client.calls(), 1);
        assert_eq!(result.suggested_updates, 1);
        assert_eq!(
            result.analysis.items[0]
                .suggested_update
                .as_ref()
                .unwrap()
                .source_message,
            "m1"
        );
    }

    #[test]
    fn an_unoffered_handle_is_rejected_and_attaches_nothing() {
        let (all, item) = cross_thread_fixture();
        let client = ScriptedClient::new(|_, user| {
            let (message, length) = message_with_body(user, "Sure, let's do it.").unwrap();
            Ok(claims_document(&[closure_claim(
                "possible_closure",
                &message,
                length,
                &["loop-9-m0-b0"],
                None,
                900_000,
            )]))
        });
        let result = run_closures(&all, vec![item], &client, &ScanProgress::default());

        assert_eq!(client.calls(), 1);
        assert_eq!(result.suggested_updates, 0);
        assert!(result.analysis.items[0].suggested_update.is_none());
    }

    #[test]
    fn other_claim_types_and_claims_without_a_loop_create_nothing() {
        let (all, item) = cross_thread_fixture();
        let client = ScriptedClient::new(|_, user| {
            let (message, length) = message_with_body(user, "Sure, let's do it.").unwrap();
            Ok(claims_document(&[
                closure_claim("request", &message, length, &[], None, 900_000),
                closure_claim("promise", &message, length, &[], None, 800_000),
                closure_claim("possible_closure", &message, length, &[], None, 700_000),
            ]))
        });
        let result = run_closures(&all, vec![item], &client, &ScanProgress::default());

        assert_eq!(result.suggested_updates, 0);
        assert_eq!(
            result.analysis.items.len(),
            1,
            "no new loops from this pass"
        );
        assert!(result.analysis.items[0].suggested_update.is_none());
    }

    #[test]
    fn deadline_change_and_modification_become_suggested_updates() {
        let (all, item) = cross_thread_fixture();
        let deadline = ScriptedClient::new(|_, user| {
            let (message, length) = message_with_body(user, "Sure, let's do it.").unwrap();
            Ok(claims_document(&[closure_claim(
                "deadline_change",
                &message,
                length,
                &["loop-1-m0-b0"],
                Some(("date", "2026-09-20")),
                600_000,
            )]))
        });
        let result = run_closures(
            &all,
            vec![item.clone()],
            &deadline,
            &ScanProgress::default(),
        );
        let update = result.analysis.items[0].suggested_update.as_ref().unwrap();
        assert_eq!(update.kind, SuggestedUpdateKind::DeadlineChange);
        assert_eq!(update.temporal_value.as_deref(), Some("2026-09-20"));

        let modification = ScriptedClient::new(|_, user| {
            let (message, length) = message_with_body(user, "Sure, let's do it.").unwrap();
            Ok(claims_document(&[closure_claim(
                "modification",
                &message,
                length,
                &["loop-1-m0-b0"],
                None,
                600_000,
            )]))
        });
        let result = run_closures(&all, vec![item], &modification, &ScanProgress::default());
        assert_eq!(
            result.analysis.items[0]
                .suggested_update
                .as_ref()
                .unwrap()
                .kind,
            SuggestedUpdateKind::Modification
        );
    }

    #[test]
    fn the_highest_confidence_claim_wins_and_a_closure_wins_ties() {
        let (all, item) = cross_thread_fixture();
        let pick = |closure_confidence: u32, modification_confidence: u32| {
            let client = ScriptedClient::new(move |_, user| {
                let (message, length) = message_with_body(user, "Sure, let's do it.").unwrap();
                Ok(claims_document(&[
                    closure_claim(
                        "modification",
                        &message,
                        length,
                        &["loop-1-m0-b0"],
                        None,
                        modification_confidence,
                    ),
                    closure_claim(
                        "possible_closure",
                        &message,
                        length,
                        &["loop-1-m0-b0"],
                        None,
                        closure_confidence,
                    ),
                ]))
            });
            let result = run_closures(&all, vec![item.clone()], &client, &ScanProgress::default());
            result.analysis.items[0]
                .suggested_update
                .as_ref()
                .unwrap()
                .kind
        };
        assert_eq!(pick(500_000, 800_000), SuggestedUpdateKind::Modification);
        assert_eq!(pick(800_000, 500_000), SuggestedUpdateKind::Closure);
        assert_eq!(pick(700_000, 700_000), SuggestedUpdateKind::Closure);
    }

    #[test]
    fn a_request_offers_at_most_eight_handles() {
        let mut all = Vec::new();
        let mut items = Vec::new();
        for n in 0..10 {
            // Distinct waiting parties keep any one address out of the
            // shared-mailbox heuristic; one reply is addressed to all ten.
            let address = format!("sam{n}@example.invalid");
            let request = request_from(
                &address,
                &format!("req-{n}"),
                &format!("r{n}"),
                "acct",
                "Fee",
            );
            all.push(prepare(&request, "Inbox", all.len()).unwrap());
            let (_, mut item) = closure_test_messages();
            item.waiting_party = format!("Other <{address}>");
            item.evidence
                .message
                .clone_from(&all.last().unwrap().input.handle);
            items.push(item);
        }
        let mut reply = reply_to("sam0@example.invalid", "rep", "v", "acct", "Fee");
        reply.to = (0..10).map(|n| format!("sam{n}@example.invalid")).collect();
        all.push(prepare(&reply, "Sent", all.len()).unwrap());
        let client = ScriptedClient::new(|_, _| Ok(EMPTY_CLAIMS.to_string()));
        run_closures(&all, items, &client, &ScanProgress::default());

        assert_eq!(client.calls(), 1);
        let handles = offered_handles(&client.payload(0));
        assert_eq!(handles.len(), MAX_LOOP_HANDLES);
        assert_eq!(handles[0], "loop-1-m0-b0");
    }

    #[test]
    fn the_conversation_budget_caps_requests_and_notes_it() {
        let (all, items) = parallel_closure_fixture(45);
        let client = ScriptedClient::new(|_, _| Ok(EMPTY_CLAIMS.to_string()));
        let result = run_closures(&all, items, &client, &ScanProgress::default());

        assert_eq!(client.calls(), MAX_CLOSURE_CONVERSATIONS);
        assert!(result.conversation_notes.contains(&format!(
            "Update checks were capped at {MAX_CLOSURE_CONVERSATIONS} conversations this scan."
        )));
    }

    #[test]
    fn a_cancelled_request_stops_the_pass_without_a_failure() {
        let (all, item) = cross_thread_fixture();
        let client = ScriptedClient::new(|_, _| Err(ProviderError::Cancelled));
        let result = run_closures(&all, vec![item], &client, &ScanProgress::default());

        assert_eq!(client.calls(), 1);
        assert!(result.cancelled);
        assert!(result.closure_pass_failure.is_none());
        assert!(result.analysis.items[0].suggested_update.is_none());
    }

    #[test]
    fn a_pass_cancelled_before_it_starts_sends_nothing() {
        let (all, items) = parallel_closure_fixture(3);
        let progress = ScanProgress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        let client = ScriptedClient::new(|_, _| Ok(EMPTY_CLAIMS.to_string()));
        let result = run_closures(&all, items, &client, &progress);

        assert_eq!(client.calls(), 0);
        assert!(result.cancelled);
    }

    #[test]
    fn a_transport_error_is_recorded_separately_and_stops_only_this_pass() {
        let (all, items) = parallel_closure_fixture(3);
        let client = ScriptedClient::new(|_, _| Err(ProviderError::Unauthorized));
        let result = run_closures(&all, items, &client, &ScanProgress::default());

        assert_eq!(client.calls(), 1, "the pass stops after a transport error");
        assert!(
            result
                .closure_pass_failure
                .as_deref()
                .is_some_and(|failure| failure.starts_with("Closure pass stopped"))
        );
        assert!(!result.cancelled);
        assert!(result.failures.is_empty());
        assert_eq!(result.suggested_updates, 0);
    }

    #[test]
    fn a_non_transport_error_skips_that_conversation_and_continues() {
        let (all, items) = parallel_closure_fixture(2);
        let client = ScriptedClient::new(|index, user| {
            if index == 0 {
                Err(ProviderError::Timeout)
            } else {
                Ok(close_first_loop(user))
            }
        });
        let result = run_closures(&all, items, &client, &ScanProgress::default());

        assert_eq!(client.calls(), 2);
        assert!(result.closure_pass_failure.is_none());
        assert_eq!(result.suggested_updates, 1);
    }

    #[test]
    fn a_rate_limited_request_is_noted_and_never_resent() {
        let (all, items) = parallel_closure_fixture(2);
        let client = ScriptedClient::new(|_, _| Err(ProviderError::RateLimited));
        let result = run_closures(&all, items, &client, &ScanProgress::default());

        assert_eq!(client.calls(), 2);
        assert!(result.closure_pass_failure.is_none());
        assert!(
            result.conversation_notes.contains(
                &"2 update check(s) were rate-limited; they were not resent.".to_string()
            )
        );
    }

    #[test]
    fn a_shared_mailbox_waiting_party_is_not_reached_across_conversations() {
        // list@example.invalid appears in at least three of the account's
        // conversation groups and in at least half of them, a distribution
        // list rather than an individual correspondent. Six other
        // conversations, the evidence conversation and the reply give the
        // account the eight groups `is_shared_mailbox_address` needs.
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
        let client = ScriptedClient::new(|_, _| Ok(EMPTY_CLAIMS.to_string()));
        let result = run_closures(&all, vec![item], &client, &ScanProgress::default());

        assert_eq!(client.calls(), 0);
        assert_eq!(result.suggested_updates, 0);
    }

    #[test]
    fn loops_that_are_not_open_and_owed_by_you_are_not_offered() {
        let (all, base_item) = cross_thread_fixture();
        let mut already_resolved = base_item.clone();
        already_resolved.resolution = Some(already_resolved.evidence.clone());
        let mut not_owed_by_the_user = base_item.clone();
        not_owed_by_the_user.kind = "attributed".into();
        let mut not_owned_by_you = base_item.clone();
        not_owned_by_you.owner = Owner::Team;
        let mut event_passed = base_item.clone();
        event_passed.event_passed = Some(EventPassed {
            name: "Synthetic event".into(),
            end: 0,
            message_handle: "m0".into(),
            from_subject: false,
        });
        let client = ScriptedClient::new(|_, _| Ok(EMPTY_CLAIMS.to_string()));
        run_closures(
            &all,
            vec![
                already_resolved,
                not_owed_by_the_user,
                not_owned_by_you,
                event_passed,
            ],
            &client,
            &ScanProgress::default(),
        );
        assert_eq!(client.calls(), 0, "none of these loops is offered");

        // The one eligible loop among them is the only handle offered, and
        // its handle counts eligible loops only.
        let (all, base_item) = cross_thread_fixture();
        let mut ineligible = base_item.clone();
        ineligible.kind = "attributed".into();
        let client = ScriptedClient::new(|_, _| Ok(EMPTY_CLAIMS.to_string()));
        run_closures(
            &all,
            vec![ineligible, base_item],
            &client,
            &ScanProgress::default(),
        );
        assert_eq!(client.calls(), 1);
        assert_eq!(
            offered_handles(&client.payload(0)),
            vec!["loop-1-m0-b0".to_string()]
        );
    }

    #[test]
    fn a_promise_the_user_made_is_offered_like_a_request() {
        let (all, mut item) = cross_thread_fixture();
        item.kind = "promise".into();
        let client = ScriptedClient::new(|_, user| Ok(close_first_loop(user)));
        let result = run_closures(&all, vec![item], &client, &ScanProgress::default());

        assert_eq!(client.calls(), 1);
        assert_eq!(result.suggested_updates, 1);
    }

    #[test]
    fn the_pass_is_skipped_when_the_primary_scan_broke_on_a_transport_error() {
        let (all, item) = cross_thread_fixture();
        let mut result = closure_result(vec![item]);
        result.primary_scan_transport_error = true;
        let client = ScriptedClient::new(|_, _| Ok(EMPTY_CLAIMS.to_string()));
        let progress = ScanProgress::default();
        super::scan_closures(&all, &progress, &mut result, &ParallelPass::new(1), &client);

        assert_eq!(client.calls(), 0);
        assert!(!progress.closure_phase.load(Ordering::Relaxed));
    }

    #[test]
    fn closure_phase_and_conversation_index_track_the_closure_pass() {
        let (all, item) = cross_thread_fixture();
        let progress = ScanProgress::default();
        let observed = Mutex::new((false, 0usize, 0usize));
        let client = ScriptedClient::new(|_, _| {
            *observed.lock().unwrap() = (
                progress.closure_phase.load(Ordering::Relaxed),
                progress.snapshot().conversation_index,
                progress.conversation_total.load(Ordering::Relaxed),
            );
            Ok(EMPTY_CLAIMS.to_string())
        });
        run_closures(&all, vec![item], &client, &progress);

        assert_eq!(*observed.lock().unwrap(), (true, 1, 1));
        assert!(progress.closure_phase.load(Ordering::Relaxed));
        assert_eq!(
            progress.snapshot().conversation_index,
            0,
            "conversation_index resets once the pass ends"
        );
    }

    #[test]
    fn recap_suggested_you_item_is_offered_to_the_closure_pass() {
        let (message, item) = recap_attribution_fixture("user@example.invalid", true);
        let mut result = scan_conversations(
            std::slice::from_ref(&message),
            &ScanProgress::default(),
            |_| {
                Ok(LoopItems {
                    items: vec![item.clone()],
                    rejected: 0,
                    rejection_reasons: vec![],
                    degraded: 0,
                })
            },
        );
        let mut reply = reply_to(
            "no-reply@fathom.video",
            "recap-reply",
            "recap-attribution",
            "synthetic-account",
            "Re: Project sync - Meeting Summary",
        );
        reply.body = "The project update was sent.".into();
        let reply = prepare(&reply, "Sent", 1).unwrap();
        let client = ScriptedClient::new(|_, user| {
            let handles = offered_handles(user);
            let (message, length) =
                message_with_body(user, "The project update was sent.").unwrap();
            Ok(claims_document(&[closure_claim(
                "possible_closure",
                &message,
                length,
                &[&handles[0]],
                None,
                900_000,
            )]))
        });
        super::scan_closures(
            &[message, reply],
            &ScanProgress::default(),
            &mut result,
            &ParallelPass::new(1),
            &client,
        );

        assert_eq!(client.calls(), 1);
        assert!(result.analysis.items[0].owner == Owner::You);
        assert!(result.analysis.items[0].suggested_update.is_some());
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
    fn address_in_two_of_eight_groups_still_bridges_merge() {
        // A correspondent that appears in only 2 of the account's 8 groups
        // fails `is_shared_mailbox_address`'s `count >= 3` clause (2 < 3) --
        // not the 8-group floor, which this test's padding deliberately
        // clears -- so it is never treated as a shared mailbox and must
        // still bridge the two groups it links, exactly the ordinary
        // two-thread case. Padded to 8 total groups (up from an earlier
        // version's 6) specifically so the account clears
        // `is_shared_mailbox_address`'s 8-group floor and this test actually
        // exercises the `count >= 3` sub-threshold, rather than passing
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
            Ok(LoopItems {
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
            Ok(LoopItems {
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
            Ok(LoopItems {
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
            Ok(LoopItems {
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
            Ok(LoopItems {
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
            Ok(LoopItems {
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
            Ok(LoopItems {
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
            Ok(LoopItems {
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
            Ok(LoopItems {
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
            Ok(LoopItems {
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
