use crate::deadline_view::{DeadlineView, EVENT_GENERIC_NOUNS, classify};
use crate::settings::Provider;
use chrono::{Datelike, TimeZone};
use openloops_domain::deadline_parse::DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT;
use openloops_graph::live::{ConnectionError, review::MailItem};
use openloops_inference::{
    blocks::CanonicalBlock,
    canonical::canonicalize_plain,
    expectations::{
        Anchor, ConversationMessage, EventPassed, Expectation, Expectations, Owner, ResolutionKind,
        closure as closure_pass, expectations as expectations_pass,
    },
    message::CanonicalMessage,
    ollama::OllamaCloud,
    openrouter::OpenRouter,
    provider::{MAX_PARALLEL_REQUESTS, ModelClient, ProviderError},
    reply_history::{
        REPLY_HISTORY_CHUNK_MAX_CHARS, chunk_reply_history, is_underscore_separator,
        starts_with_ascii_ci,
    },
    walker::canonicalize_html,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

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
pub fn build_event_index(messages: &[ReviewMessage]) -> Vec<EventRef> {
    let mut best: BTreeMap<(String, String), EventRef> = BTreeMap::new();
    for message in messages {
        let Some((name, start, end, source)) = learn_one_event(message) else {
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

/// One message's contribution to the event index, per the priority order
/// documented on [`build_event_index`]. `None` when none of the sources
/// found a sufficiently specific ([`is_specific_event_name`]) candidate (or
/// the Graph/subject entry was out of date).
fn learn_one_event(message: &ReviewMessage) -> Option<(String, i64, i64, EventSource)> {
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
        prose_event_time(&subject, message.input.timestamp, offset)
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
fn match_event_by_text<'a>(
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
        .filter(|event| matches!(event.source, EventSource::Meeting | EventSource::Subject))
        .filter(|event| {
            event.start >= evidence_timestamp && event.start - evidence_timestamp <= 60 * 86_400
        })
        .filter(|event| {
            let name_tokens = meaningful_event_tokens(&event.name);
            name_tokens.len() >= 2 && phrase_tokens.intersection(&name_tokens).count() >= 2
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
    /// eligible items `scan_closures` will attempt -- whichever pass is
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

/// Analyzes `messages`, running up to `parallel` model requests at once.
///
/// Conversations are independent of one another and so are the closure
/// pass's eligible items, so both passes fan out across a worker pool
/// bounded by what the provider allows. Concurrency adapts downward when
/// the provider rate-limits and back up as requests succeed; a
/// rate-limited request is reported as a failed conversation and never
/// resent, because `network_policy.retries` forbids automatically
/// retrying a request that already carried content.
pub fn scan(
    provider: Provider,
    key: String,
    model: &str,
    parallel: usize,
    messages: &[ReviewMessage],
    progress: &ScanProgress,
) -> Result<ScanResult, ProviderError> {
    progress.total.store(messages.len(), Ordering::Relaxed);
    let client = connect(provider, key, model, parallel)?;
    let client = client.as_ref();
    let pass = ParallelPass::new(client.max_parallel());
    let mut result = scan_conversations(messages, progress, &pass, &|conversation| {
        expectations_pass(client, conversation, Some(&progress.cancel))
    });
    close_passed_events(&mut result, messages, chrono::Utc::now().timestamp());
    scan_closures(
        messages,
        progress,
        &mut result,
        &pass,
        &|item, evidence_timestamp, candidates| {
            closure_pass(
                client,
                item,
                evidence_timestamp,
                candidates,
                Some(&progress.cancel),
            )
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
    analysis: &Expectations,
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
fn conversations_by_size(messages: &[ReviewMessage]) -> Vec<Vec<&ReviewMessage>> {
    let mut conversations: BTreeMap<(&str, &str), Vec<&ReviewMessage>> = BTreeMap::new();
    for m in messages {
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
fn is_transport_error(error: ProviderError) -> bool {
    matches!(
        error,
        ProviderError::Unauthorized
            | ProviderError::RateLimited
            | ProviderError::Quota
            | ProviderError::Network
            | ProviderError::Timeout
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
    /// Rate limited: narrow the concurrency. The request is NOT resent --
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

/// How one finished job's answer steers the limiter. Rate limiting narrows
/// the pass; quota and the other transport-class errors stop it; a
/// cancelled request stops it too (the caller's flag is normally already
/// set, but a provider may answer `Cancelled` on its own).
fn job_signal<T>(outcome: &Result<T, ProviderError>) -> JobSignal {
    let Err(error) = outcome else {
        return JobSignal::Ok;
    };
    match *error {
        ProviderError::RateLimited => JobSignal::Backoff,
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
        error => format!("{head}: {error}"),
    }
}

fn empty_result(total: usize) -> ScanResult {
    ScanResult {
        analysis: Expectations {
            items: vec![],
            rejected: 0,
            rejection_reasons: vec![],
            degraded: 0,
        },
        failures: vec![],
        analyzed: 0,
        total,
        cancelled: false,
        conversation_notes: vec![],
        cross_thread_closures: 0,
        event_closures: 0,
        primary_scan_transport_error: false,
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
    outcome: JobOutcome<Expectations>,
    quota_reported: &mut bool,
) {
    match outcome {
        JobOutcome::Completed(Ok(analysis)) => {
            result.analyzed += conversation.len();
            if let Some(note) = conversation_note(index, conversation, &analysis) {
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
        JobOutcome::Completed(Err(ProviderError::Cancelled)) => result.cancelled = true,
        JobOutcome::Completed(Err(error)) => {
            if error != ProviderError::Quota || !*quota_reported {
                result
                    .failures
                    .push(failure_line(index, conversation, error));
            }
            *quota_reported |= error == ProviderError::Quota;
            if is_stop_error(error) {
                result.primary_scan_transport_error = true;
            }
        }
        JobOutcome::Panicked => result.failures.push(format!(
            "Conversation {}: the analysis failed unexpectedly and was skipped.",
            index + 1
        )),
        JobOutcome::NotStarted => result.failures.push(format!(
            "Conversation {} (subject: {}): not started because the scan stopped.",
            index + 1,
            subject_snippet(conversation)
        )),
    }
}

/// Analyzes every conversation in `messages`, up to the pass's ceiling at
/// once. Conversations are independent, so running several concurrently
/// changes only when each answer arrives; the answers are merged in the
/// smallest-first job order [`conversations_by_size`] produced, so the
/// result is identical to analyzing them one at a time.
fn scan_conversations(
    messages: &[ReviewMessage],
    progress: &ScanProgress,
    pass: &ParallelPass,
    analyze: &(dyn Fn(&[ConversationMessage]) -> Result<Expectations, ProviderError> + Sync),
) -> ScanResult {
    let mut result = empty_result(messages.len());
    let ordered: Vec<Vec<&ReviewMessage>> = conversations_by_size(messages)
        .into_iter()
        .map(|mut conversation| {
            conversation.sort_by_key(|m| m.input.timestamp);
            conversation
        })
        .collect();
    progress
        .conversation_total
        .store(ordered.len(), Ordering::Relaxed);
    let processed_per_job: Vec<usize> = ordered.iter().map(Vec::len).collect();
    let outcomes = run_jobs(&processed_per_job, pass, progress, &|index| {
        let inputs: Vec<ConversationMessage> =
            ordered[index].iter().map(|m| m.input.clone()).collect();
        analyze(&inputs)
    });
    let mut quota_reported = false;
    for (index, outcome) in outcomes {
        merge_conversation(
            &mut result,
            index,
            &ordered[index],
            outcome,
            &mut quota_reported,
        );
    }
    if progress.cancel.load(Ordering::Relaxed) {
        result.cancelled = true;
    }
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
    item: &Expectation,
    message_timestamp: i64,
    now: i64,
    offset: i32,
) -> Option<&str> {
    if let Some(event) = &item.event {
        return Some(event.quote.as_str());
    }
    let deadline = item.deadline.as_ref()?;
    matches!(
        classify(&deadline.quote, message_timestamp, now, offset),
        DeadlineView::EventTied
    )
    .then_some(deadline.quote.as_str())
}

/// The text-based fallback used only when the model named no event at all
/// (`named_event_phrase` returned `None`): matches the item's `action`,
/// then its `evidence.quote`, against the index with
/// [`match_event_by_text`]'s proper-name-strength bar.
fn text_matched_event<'a>(
    item: &Expectation,
    source: &ReviewMessage,
    index: &'a [EventRef],
) -> Option<&'a EventRef> {
    match_event_by_text(&item.action, source.input.timestamp, index)
        .or_else(|| match_event_by_text(&item.evidence.quote, source.input.timestamp, index))
}

/// Whether an otherwise unnamed request uses language that connects it to a
/// gathering in its own conversation. Event nouns are matched as whole
/// alphanumeric tokens; the timing phrases are intentionally narrow.
fn has_scoped_event_language(item: &Expectation) -> bool {
    [&item.action, &item.evidence.quote].iter().any(|text| {
        let lower = text.to_lowercase();
        let words: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect();
        words.iter().any(|word| {
            EVENT_GENERIC_NOUNS.contains(word)
                || ["arrive", "arriving", "attend", "attending", "bring"].contains(word)
        }) || words
            .windows(2)
            .any(|pair| matches!(pair, ["before", "the"] | ["prior", "to"] | ["ahead", "of"]))
    })
}

/// Finds deterministic event evidence scoped to the expectation's source:
/// first an event learned from the evidence message itself, otherwise an
/// event in the same conversation when the request uses event-shaped
/// language. Body-prose events are deliberately excluded from both rules.
fn scoped_event<'a>(
    item: &Expectation,
    source: &ReviewMessage,
    index: &'a [EventRef],
) -> Option<&'a EventRef> {
    let qualifying = |event: &&EventRef| {
        matches!(
            event.source,
            EventSource::Meeting | EventSource::Subject | EventSource::SubjectProse
        )
    };
    index
        .iter()
        .filter(qualifying)
        .find(|event| event.message_handle == item.evidence.message)
        .or_else(|| {
            has_scoped_event_language(item).then(|| {
                index
                    .iter()
                    .filter(qualifying)
                    .filter(|event| event.conversation == source.conversation)
                    .min_by_key(|event| (event.start, &event.message_handle))
            })?
        })
}

/// Closes `item` against `event` when `event` has already ended. Returns
/// whether it closed.
fn close_from_index(item: &mut Expectation, event: &EventRef, now: i64) -> bool {
    if event.end >= now {
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
/// `name` is the event's own name when known (from [`named_event_phrase`]),
/// used as-is since an item reaching this path always named an event.
/// Returns whether it closed.
fn close_from_stated_time(
    item: &mut Expectation,
    messages: &[ReviewMessage],
    now: i64,
    name: &str,
) -> bool {
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
    item.event_passed = Some(EventPassed {
        name: name.to_string(),
        end,
        message_handle: event_time.message.clone(),
        // `event_time` is an `Anchor`, which the model can only ground in a
        // message's `body_blocks` (never its subject -- see
        // `expectations::anchor`), so this path is always body prose.
        from_subject: false,
    });
    true
}

/// Closes any open, unresolved expectation whose event has already ended,
/// via four independent sources of evidence, tried in order for each item:
///
/// - a structured or subject-prose event learned from the evidence message
///   itself, or from its conversation when the request uses generic event
///   language ([`scoped_event`]);
/// - the learned event index, matched against the expectation's own named
///   event ([`named_event_phrase`]) with [`match_event`] -- when the index
///   has a match, it alone decides this item, whether or not it closes;
/// - failing that, a model-supplied `event_time` phrase -- a verbatim
///   date/time anchor stating when the NAMED event occurs, found anywhere
///   in the conversation -- classified directly ([`close_from_stated_time`]);
///   only ever consulted for an item that named an event in the first
///   place;
/// - for an item that named no event at all, the index again, but matched
///   against the item's own `action`/`evidence.quote` text instead
///   ([`text_matched_event`]), at a much stronger bar so an unrelated
///   generic phrase never matches by accident.
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

        if let Some(event) = scoped_event(item, source, &index) {
            scoped += 1;
            if close_from_index(item, event, now) {
                closed_from_index += 1;
                result.event_closures += 1;
            }
            continue;
        }

        if let Some(phrase) = named_event_phrase(item, source.input.timestamp, now, offset) {
            named += 1;
            let phrase = phrase.to_string();
            let event_match = match_event(&phrase, source.input.timestamp, &index);
            matched += usize::from(event_match.is_some());
            if let Some(event) = event_match {
                if close_from_index(item, event, now) {
                    closed_from_index += 1;
                    result.event_closures += 1;
                }
                continue;
            }
            if close_from_stated_time(item, messages, now, &phrase) {
                closed_from_stated_time += 1;
                result.event_closures += 1;
            }
            continue;
        }

        if let Some(event) = text_matched_event(item, source, &index) {
            matched_by_text += 1;
            if close_from_index(item, event, now) {
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

/// Hard cap on how many `closure()` provider calls one scan makes, however
/// many open requests are eligible: a large mailbox could otherwise turn
/// into dozens of extra model calls in a single scan.
const MAX_CLOSURE_CALLS: usize = 40;

/// The signature every `scan_closures` caller's `closure()` callback takes:
/// the open expectation, its evidence timestamp, and the candidate later
/// messages, answering with resolution evidence when found. `Sync` because
/// one callback serves every worker of the parallel closure pass.
type ClosureCall<'a> = dyn Fn(
        &Expectation,
        i64,
        &[ConversationMessage],
    ) -> Result<Option<(Anchor, ResolutionKind)>, ProviderError>
    + Sync
    + 'a;

/// State [`attempt_closure`] shares across every item in one
/// `scan_closures` pass: the account/address grouping
/// [`is_shared_mailbox_address`] needs (computed once up front, since it
/// depends on every message, not just the current item) and the
/// [`MAX_CLOSURE_CALLS`] budget shared by every item's provider call.
/// Bundled into one struct, rather than four separate parameters, so
/// `attempt_closure` stays under the pedantic argument-count lint without
/// an `allow`. The budget counters are atomic because the pass's workers
/// draw on the same budget concurrently.
struct ClosurePassState<'a> {
    groups_per_account: GroupsPerAccount<'a>,
    address_group_counts: AddressGroupCounts<'a>,
    calls_made: AtomicUsize,
    capped: AtomicBool,
}

impl ClosurePassState<'_> {
    /// Claims one of the [`MAX_CLOSURE_CALLS`] provider calls this scan
    /// allows, or records that the cap bound and refuses.
    fn claim_call(&self) -> bool {
        if self.calls_made.fetch_add(1, Ordering::Relaxed) < MAX_CLOSURE_CALLS {
            return true;
        }
        self.calls_made.fetch_sub(1, Ordering::Relaxed);
        self.capped.store(true, Ordering::Relaxed);
        false
    }
}

/// The per-item body of `scan_closures`'s pass, factored out to keep that
/// function under the line-count lint: looks up `item`'s evidence message,
/// checks same-thread candidates first, then looks up its waiting-party
/// address and checks cross-thread candidates. A shared-mailbox/list address
/// (see [`is_shared_mailbox_address`]) suppresses only that cross-thread
/// check. Each provider call uses the shared [`MAX_CLOSURE_CALLS`] budget.
/// Every skip answers `Ok(None)`, exactly like a call that found no
/// closing evidence; the caller cannot and need not tell them apart.
fn attempt_closure(
    item: &Expectation,
    messages: &[ReviewMessage],
    state: &ClosurePassState<'_>,
    closure: &ClosureCall<'_>,
) -> Result<Option<(Anchor, ResolutionKind, bool)>, ProviderError> {
    let Some(source) = messages
        .iter()
        .find(|m| m.input.handle == item.evidence.message)
    else {
        return Ok(None);
    };
    let same_thread = same_thread_closure_candidates(
        messages,
        &source.account,
        &source.conversation,
        source.input.timestamp,
    );
    if !same_thread.is_empty() {
        if !state.claim_call() {
            return Ok(None);
        }
        let inputs: Vec<ConversationMessage> = same_thread
            .iter()
            .map(|message| message.input.clone())
            .collect();
        if let Some(resolution) = closure(item, source.input.timestamp, &inputs)? {
            return Ok(Some((resolution.0, resolution.1, false)));
        }
    }
    let Some(address) = waiting_party_address(&item.waiting_party) else {
        return Ok(None);
    };
    if is_shared_mailbox_address(
        &source.account,
        &address,
        &state.groups_per_account,
        &state.address_group_counts,
    ) {
        return Ok(None);
    }
    let candidates = closure_candidates(
        item,
        messages,
        &source.account,
        &source.conversation,
        source.input.timestamp,
    );
    if candidates.is_empty() || !state.claim_call() {
        return Ok(None);
    }
    let inputs: Vec<ConversationMessage> = candidates.iter().map(|m| m.input.clone()).collect();
    closure(item, source.input.timestamp, &inputs)
        .map(|resolution| resolution.map(|(anchor, kind)| (anchor, kind, true)))
}

/// After the primary per-conversation scan, attempts to close any
/// remaining open "you owe someone" requests. Each item is checked first
/// against later replies the user sent in the same conversation, then
/// against messages sent to the waiting party in a different conversation.
/// Mutates `result.analysis.items` in place; only the latter resolutions set
/// `cross_thread: true` and increment `result.cross_thread_closures`.
///
/// Skipped entirely when `result.primary_scan_transport_error` is set: the
/// provider is already known to be unreachable or unauthorized, so per-item
/// closure calls would fail identically. Before running, the count of
/// eligible items (open, `request`-kind, `You`-owned) is added to
/// `progress.total`, and `progress.processed` is incremented once per
/// eligible item as it is processed, including one skipped for having no
/// candidates, a shared-mailbox/list waiting party (see
/// [`is_shared_mailbox_address`]), the [`MAX_CLOSURE_CALLS`] cap binding, or
/// a non-terminal provider error -- `scan_closures` only ever `continue`s
/// past those, exactly like `scan_conversations` does for the same error
/// classes. Quota and the terminal transport-class provider errors (the
/// same error classes `scan_conversations` uses) stop the pass early, recorded
/// content-free in `result.closure_pass_failure` rather than `failures`
/// (which counts unanalyzed conversations -- a closure-pass failure never
/// leaves a conversation unanalyzed). A `Cancelled` answer stops the pass
/// the same way but sets `result.cancelled` instead, exactly like
/// `scan_conversations`. `progress.cancel` is checked between items exactly
/// like `scan_conversations`.
fn scan_closures(
    messages: &[ReviewMessage],
    progress: &ScanProgress,
    result: &mut ScanResult,
    pass: &ParallelPass,
    closure: &ClosureCall<'_>,
) {
    if result.primary_scan_transport_error {
        return;
    }
    let eligible: Vec<usize> = result
        .analysis
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.resolution.is_none()
                && item.event_passed.is_none()
                && item.kind == "request"
                && item.owner == Owner::You
        })
        .map(|(index, _)| index)
        .collect();
    progress.total.fetch_add(eligible.len(), Ordering::Relaxed);
    progress
        .conversation_total
        .store(eligible.len(), Ordering::Relaxed);
    // Left set for the rest of the scan: tells the desktop to label
    // `conversation_index`/`conversation_total` as this pass rather than
    // the primary per-conversation one.
    progress.closure_phase.store(true, Ordering::Relaxed);
    let (groups_per_account, address_group_counts) = conversation_group_address_counts(messages);
    let state = ClosurePassState {
        groups_per_account,
        address_group_counts,
        calls_made: AtomicUsize::new(0),
        capped: AtomicBool::new(false),
    };
    pass.restart();
    let processed_per_job = vec![1; eligible.len()];
    let outcomes = {
        let items = &result.analysis.items;
        run_jobs(&processed_per_job, pass, progress, &|slot| {
            attempt_closure(&items[eligible[slot]], messages, &state, closure)
        })
    };
    let same_thread_closures = merge_closures(result, &eligible, outcomes);
    // Idle once this pass ends, same as `scan_conversations`.
    progress.reset_pass();
    if progress.cancel.load(Ordering::Relaxed) {
        result.cancelled = true;
    }
    if same_thread_closures > 0 {
        result.conversation_notes.push(format!(
            "{same_thread_closures} resolved from your later reply in the same thread."
        ));
    }
    if result.cross_thread_closures > 0 {
        result.conversation_notes.push(format!(
            "Closing evidence found in another conversation for {} expectation(s).",
            result.cross_thread_closures
        ));
    }
    if state.capped.load(Ordering::Relaxed) {
        result.conversation_notes.push(format!(
            "Cross-thread closure checks were capped at {MAX_CLOSURE_CALLS} open requests this scan."
        ));
    }
}

/// Applies the closure pass's answers to `result`, in item order, so the
/// outcome does not depend on which worker finished first. `eligible` maps
/// a job slot to the index of the item it examined.
fn merge_closures(
    result: &mut ScanResult,
    eligible: &[usize],
    outcomes: JobResults<Option<(Anchor, ResolutionKind, bool)>>,
) -> usize {
    let mut rate_limited = 0usize;
    let mut same_thread_closures = 0usize;
    for (slot, outcome) in outcomes {
        match outcome {
            JobOutcome::Completed(Ok(Some((anchor, kind, cross_thread)))) => {
                let item = &mut result.analysis.items[eligible[slot]];
                item.resolution = Some(anchor);
                item.resolution_kind = Some(kind);
                item.cross_thread = cross_thread;
                if cross_thread {
                    result.cross_thread_closures += 1;
                } else {
                    same_thread_closures += 1;
                }
            }
            JobOutcome::Completed(Err(ProviderError::Cancelled)) => result.cancelled = true,
            JobOutcome::Completed(Err(ProviderError::RateLimited)) => rate_limited += 1,
            JobOutcome::Completed(Err(error)) if is_stop_error(error) => {
                if result.closure_pass_failure.is_none() {
                    result.closure_pass_failure =
                        Some(format!("Cross-thread closure pass stopped: {error}"));
                }
            }
            // No closing evidence, a skipped item, or a failure about this
            // one item: the expectation simply stays open.
            JobOutcome::Panicked => {
                if result.closure_pass_failure.is_none() {
                    result.closure_pass_failure =
                        Some("Cross-thread closure pass failed unexpectedly.".into());
                }
            }
            JobOutcome::Completed(Ok(None) | Err(_)) | JobOutcome::NotStarted => {}
        }
    }
    if rate_limited > 0 {
        result.conversation_notes.push(format!(
            "{rate_limited} cross-thread closure check(s) were rate-limited; they were not resent."
        ));
    }
    same_thread_closures
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
        m.to_user = to_user;
        m.team = team;
        if from_user {
            set_outgoing(&mut m);
        }
        let result = expectations_pass(client, &[m], None)?;
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
                        openloops_inference::expectations::Owner::Team
                    } else {
                        openloops_inference::expectations::Owner::You
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
    let result = expectations_pass(client, &[a.clone(), b], None)?;
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
    let result = expectations_pass(client, &[a, ack], None)?;
    if result.items.len() != 1 || result.items[0].resolution.is_some() {
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

/// Runs `AGREEMENT_CASE` against `client`: a request that asked for the
/// user's agreement or decision, and got it, one message later must still
/// be recognized as closure evidence, with `resolution_kind` Agreed. The
/// reply may itself read as a new request, so 1 or 2 items are both
/// acceptable; what matters is that the item anchored on the original
/// request (`m0`) carries the expected resolution kind. `case_number` is
/// only for print numbering. Prints counts and the observed kind name (both
/// fixed strings) only; never returned content.
fn probe_agreement_case(client: &dyn ModelClient, case_number: usize) -> Result<(), ProviderError> {
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
    let result = expectations_pass(client, &[request_message, reply_message], None)?;
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

/// Runs `AMENDMENT_CASE` against `client`: a correction that leaves the
/// underlying action owed (only the amount changed) must NOT resolve the
/// request -- it must stay open, with the corrected amount reflected in
/// `action`, citing the original request as evidence. `case_number` is only
/// for print numbering. Prints counts only (fixed strings); the corrected
/// amount is asserted, never printed, since `action` is model-supplied free
/// text derived from message content.
fn probe_amendment_case(client: &dyn ModelClient, case_number: usize) -> Result<(), ProviderError> {
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
    let result = expectations_pass(client, &[request_message, correction_message], None)?;
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

/// Runs `COMPLETED_RESOLUTION_CASE` against `client`: a plain completion
/// must still resolve to exactly one item with `resolution_kind` Completed.
/// `case_number` is only for print numbering. Prints counts and the
/// observed kind name (both fixed strings) only; never returned content.
fn probe_completed_resolution_case(
    client: &dyn ModelClient,
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
    let result = expectations_pass(client, &[request_message, reply_message], None)?;
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
    use std::time::Instant;

    /// Runs the primary pass with a single worker and an `FnMut` callback:
    /// the shape the pass had before it was parallelized. Most tests here
    /// assert per-call bookkeeping and read best with a callback that owns
    /// mutable state, and a one-worker pool is exactly the sequential loop.
    fn scan_conversations(
        messages: &[ReviewMessage],
        progress: &ScanProgress,
        analyze: impl FnMut(&[ConversationMessage]) -> Result<Expectations, ProviderError> + Send,
    ) -> ScanResult {
        sequential_conversations(messages, progress, analyze)
    }

    fn sequential_conversations(
        messages: &[ReviewMessage],
        progress: &ScanProgress,
        analyze: impl FnMut(&[ConversationMessage]) -> Result<Expectations, ProviderError> + Send,
    ) -> ScanResult {
        let analyze = Mutex::new(analyze);
        super::scan_conversations(messages, progress, &ParallelPass::new(1), &|conversation| {
            (analyze.lock().unwrap_or_else(PoisonError::into_inner))(conversation)
        })
    }

    /// The closure-pass counterpart of [`scan_conversations`].
    fn scan_closures(
        messages: &[ReviewMessage],
        progress: &ScanProgress,
        result: &mut ScanResult,
        closure: impl FnMut(
            &Expectation,
            i64,
            &[ConversationMessage],
        ) -> Result<Option<(Anchor, ResolutionKind)>, ProviderError>
        + Send,
    ) {
        let closure = Mutex::new(closure);
        super::scan_closures(
            messages,
            progress,
            result,
            &ParallelPass::new(1),
            &|item, evidence_timestamp, candidates| {
                (closure.lock().unwrap_or_else(PoisonError::into_inner))(
                    item,
                    evidence_timestamp,
                    candidates,
                )
            },
        );
    }

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
            Ok(Expectations {
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

    fn no_expectations() -> Expectations {
        Expectations {
            items: vec![],
            rejected: 0,
            rejection_reasons: vec![],
            degraded: 0,
        }
    }

    /// One expectation naming `handle`, so a merged `analysis.items` list
    /// records exactly which conversations contributed and in what order.
    fn expectation_for(handle: &str) -> Expectation {
        Expectation {
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
            event: None,
            event_time: None,
            resolution: None,
            resolution_kind: None,
            uncertainty: String::new(),
            unverified_deadline: false,
            unverified_resolution: false,
            cross_thread: false,
            event_passed: None,
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
                Ok(no_expectations())
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
            Ok(Expectations {
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
            Ok(no_expectations())
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
        assert!(
            result.failures.is_empty(),
            "a stopped request is not a failure: {:?}",
            result.failures
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
        assert_eq!(result.failures.len(), started);
        assert!(!result.cancelled);
        assert_eq!(progress.snapshot().in_flight, 0);
    }

    #[test]
    fn quota_stops_dispatch_and_is_reported_once() {
        let messages = parallel_corpus(8);
        let progress = ScanProgress::default();
        let calls = AtomicUsize::new(0);
        let first_wave = std::sync::Barrier::new(4);
        let result =
            super::scan_conversations(&messages, &progress, &ParallelPass::new(4), &|_| {
                calls.fetch_add(1, Ordering::SeqCst);
                first_wave.wait();
                Err(ProviderError::Quota)
            });
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert!(result.primary_scan_transport_error);
        assert_eq!(result.failures.len(), 1);
        assert!(result.failures[0].contains("HTTP 402"));
        assert_eq!(progress.processed.load(Ordering::Relaxed), 4);
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
            Ok(no_expectations())
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
            Ok(no_expectations())
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
            Err(ProviderError::Network)
        });
        assert_eq!(calls, 1);
        assert_eq!(result.analyzed, 0);
        assert_eq!(result.failures.len(), 1);
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
    fn a_cancelled_request_stops_the_scan_without_a_failure_line() {
        let a = prepare(&synthetic("Please send the draft.", 0, "a"), "Inbox", 0).unwrap();
        let b = prepare(&synthetic("Please send the agenda.", 1, "b"), "Inbox", 1).unwrap();
        let mut calls = 0;
        let result = scan_conversations(&[a, b], &ScanProgress::default(), |_| {
            calls += 1;
            Err(ProviderError::Cancelled)
        });
        assert_eq!(calls, 1);
        assert!(result.cancelled);
        assert!(
            result.failures.is_empty(),
            "a stopped request is not a failure: {:?}",
            result.failures
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
            Ok(Expectations {
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
    fn closure_phase_and_conversation_index_track_the_closure_pass() {
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
        let progress = ScanProgress::default();
        let mut observed_phase_during = false;
        let mut observed_index_during = 0;
        scan_closures(&all, &progress, &mut result, |_, _, _| {
            observed_phase_during = progress.closure_phase.load(Ordering::Relaxed);
            observed_index_during = progress.snapshot().conversation_index;
            Ok(None)
        });
        assert!(observed_phase_during);
        assert_eq!(observed_index_during, 1);
        assert!(progress.closure_phase.load(Ordering::Relaxed));
        assert_eq!(
            progress.snapshot().conversation_index,
            0,
            "conversation_index resets once the pass ends"
        );
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

    /// `count` open requests, each with a later reply in a different
    /// conversation, so every one of them is eligible for the closure pass.
    fn parallel_closure_fixture(count: usize) -> (Vec<ReviewMessage>, Vec<Expectation>) {
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
            items.push(Expectation {
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

    fn closure_result(items: Vec<Expectation>) -> ScanResult {
        let mut result = empty_result(items.len());
        result.analyzed = items.len();
        result.analysis.items = items;
        result
    }

    #[test]
    fn the_closure_pass_runs_in_parallel_and_resolves_the_same_items_as_one_worker() {
        const DELAY: Duration = Duration::from_millis(80);
        let (all, items) = parallel_closure_fixture(8);
        // Deterministic per item: only the even-numbered waiting parties
        // have closing evidence.
        let closure = |item: &Expectation,
                       _: i64,
                       candidates: &[ConversationMessage]|
         -> Result<Option<(Anchor, ResolutionKind)>, ProviderError> {
            let closed = item
                .waiting_party
                .chars()
                .rfind(char::is_ascii_digit)
                .and_then(|c| c.to_digit(10))
                .is_some_and(|digit| digit % 2 == 0);
            Ok(closed.then(|| {
                (
                    Anchor {
                        message: candidates[0].handle.clone(),
                        block: 0,
                        quote: "Sure, let's do it.".into(),
                        context: "Sure, let's do it.".into(),
                    },
                    ResolutionKind::Completed,
                )
            }))
        };

        let mut sequential = closure_result(items.clone());
        super::scan_closures(
            &all,
            &ScanProgress::default(),
            &mut sequential,
            &ParallelPass::new(1),
            &closure,
        );

        let seen = Concurrency::default();
        let progress = ScanProgress::default();
        let mut parallel = closure_result(items);
        let started = Instant::now();
        super::scan_closures(
            &all,
            &progress,
            &mut parallel,
            &ParallelPass::new(4),
            &|item, evidence_timestamp, candidates| {
                seen.enter();
                std::thread::sleep(DELAY);
                seen.leave();
                closure(item, evidence_timestamp, candidates)
            },
        );
        let elapsed = started.elapsed();

        assert_eq!(sequential.cross_thread_closures, 4);
        assert_eq!(parallel.cross_thread_closures, 4);
        assert_eq!(seen.peak(), 4, "four closure checks must run at once");
        assert!(
            elapsed < Duration::from_millis(1500) + DELAY * 5,
            "eight 80ms closure checks across four workers took {elapsed:?}"
        );
        let resolved = |result: &ScanResult| -> Vec<bool> {
            result
                .analysis
                .items
                .iter()
                .map(|item| item.resolution.is_some())
                .collect()
        };
        assert_eq!(resolved(&parallel), resolved(&sequential));
        assert_eq!(parallel.conversation_notes, sequential.conversation_notes);
        assert!(
            parallel
                .analysis
                .items
                .iter()
                .filter(|item| item.resolution.is_some())
                .all(|item| item.cross_thread)
        );
        assert_eq!(progress.processed.load(Ordering::Relaxed), 8);
        assert_eq!(progress.snapshot().in_flight, 0);
    }

    #[test]
    fn rate_limited_closure_checks_are_reported_and_never_resent() {
        let (all, items) = parallel_closure_fixture(6);
        let calls = AtomicUsize::new(0);
        let mut result = closure_result(items);
        super::scan_closures(
            &all,
            &ScanProgress::default(),
            &mut result,
            &ParallelPass::new(4),
            &|_, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::RateLimited)
            },
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            6,
            "every eligible item is attempted exactly once"
        );
        assert_eq!(result.cross_thread_closures, 0);
        assert!(
            result.closure_pass_failure.is_none(),
            "rate limiting narrows the pass, it does not stop it: {:?}",
            result.closure_pass_failure
        );
        assert!(
            result.conversation_notes.iter().any(|note| note
                == "6 cross-thread closure check(s) were rate-limited; they were not resent."),
            "notes: {:?}",
            result.conversation_notes
        );
        assert!(!result.cancelled);
    }

    #[test]
    fn same_thread_agreement_closes_before_cross_thread_and_is_counted_separately() {
        let (mut all, item) = closure_test_messages();
        let mut reply = reply_to("sam@example.invalid", "rep-1", "c1", "acct", "Fee");
        reply.body = "Yes, works for me.".into();
        all.push(prepare(&reply, "Sent", 1).unwrap());
        let reply_handle = all[1].input.handle.clone();
        let mut result = closure_result(vec![item]);

        super::scan_closures(
            &all,
            &ScanProgress::default(),
            &mut result,
            &ParallelPass::new(1),
            &|_, _, candidates| {
                assert_eq!(candidates.len(), 1);
                assert_eq!(candidates[0].handle, reply_handle);
                Ok(Some((
                    Anchor {
                        message: candidates[0].handle.clone(),
                        block: 0,
                        quote: "Yes, works for me.".into(),
                        context: "Yes, works for me.".into(),
                    },
                    ResolutionKind::Agreed,
                )))
            },
        );

        let closed = &result.analysis.items[0];
        assert_eq!(closed.resolution_kind, Some(ResolutionKind::Agreed));
        assert!(!closed.cross_thread);
        assert_eq!(result.cross_thread_closures, 0);
        assert!(
            result
                .conversation_notes
                .iter()
                .any(|note| { note == "1 resolved from your later reply in the same thread." })
        );
    }

    #[test]
    fn same_thread_acknowledgement_that_returns_null_stays_open() {
        let (mut all, item) = closure_test_messages();
        let reply = reply_to("sam@example.invalid", "rep-1", "c1", "acct", "Fee");
        all.push(prepare(&reply, "Sent", 1).unwrap());
        let calls = AtomicUsize::new(0);
        let mut result = closure_result(vec![item]);

        super::scan_closures(
            &all,
            &ScanProgress::default(),
            &mut result,
            &ParallelPass::new(1),
            &|_, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            },
        );

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(result.analysis.items[0].resolution.is_none());
        assert!(result.conversation_notes.is_empty());
    }

    #[test]
    fn same_thread_candidates_are_checked_before_cross_thread_candidates() {
        let (mut all, item) = closure_test_messages();
        let same = reply_to("sam@example.invalid", "same", "c1", "acct", "Fee");
        all.push(prepare(&same, "Sent", 1).unwrap());
        let same_handle = all[1].input.handle.clone();
        let cross = reply_to("sam@example.invalid", "cross", "c2", "acct", "Fee");
        all.push(prepare(&cross, "Sent", 2).unwrap());
        let cross_handle = all[2].input.handle.clone();
        let seen = Mutex::new(Vec::new());
        let mut result = closure_result(vec![item]);

        super::scan_closures(
            &all,
            &ScanProgress::default(),
            &mut result,
            &ParallelPass::new(1),
            &|_, _, candidates| {
                let handle = candidates[0].handle.clone();
                seen.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(handle.clone());
                Ok((handle == cross_handle).then(|| {
                    (
                        Anchor {
                            message: handle,
                            block: 0,
                            quote: "Sure, let's do it.".into(),
                            context: "Sure, let's do it.".into(),
                        },
                        ResolutionKind::Completed,
                    )
                }))
            },
        );

        assert_eq!(
            *seen.lock().unwrap_or_else(PoisonError::into_inner),
            vec![same_handle, cross_handle]
        );
        assert!(result.analysis.items[0].cross_thread);
        assert_eq!(result.cross_thread_closures, 1);
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
        template: &Expectation,
        message_handle: &str,
        action: &str,
        quote: &str,
    ) -> Expectation {
        Expectation {
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

    #[test]
    fn scoped_subject_event_closes_both_requests_from_its_message_and_future_stays_open() {
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
        assert!(
            passed
                .analysis
                .items
                .iter()
                .all(|item| item.event_passed.is_some())
        );
        assert_eq!(passed.event_closures, 2);
        assert!(passed.conversation_notes[0].contains("2 tied to their own message's event"));

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
        let scoped_request = Expectation {
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
    fn scoped_event_does_not_cross_conversations_or_close_unworded_later_requests() {
        let event_message = scoped_event_message();
        let event_end = build_event_index(std::slice::from_ref(&event_message))[0].end;
        let (_, template) = closure_test_messages();
        let mut unrelated_mail = synthetic("Please attend the workshop.", 1, "other-thread");
        unrelated_mail.received = "2026-08-02T12:00:00Z".into();
        let unrelated_message = prepare(&unrelated_mail, "Inbox", 1).unwrap();
        let mut unrelated = Expectation {
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
        let no_event_words = Expectation {
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
        assert!(result.analysis.items[0].event_passed.is_none());
        assert_eq!(result.event_closures, 0);
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
        assert!(result.analysis.items[0].event_passed.is_none());
        assert_eq!(result.event_closures, 0);
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
        let mut observed_during = 0;
        let progress = ScanProgress::default();
        scan_closures(&all, &progress, &mut result, |_, _, _| {
            calls += 1;
            let snapshot = progress.snapshot();
            observed_during = snapshot.request_started_unix;
            assert_eq!(snapshot.conversation_index, 1);
            assert_eq!(progress.conversation_total.load(Ordering::Relaxed), 1);
            Ok(Some((resolved_anchor.clone(), ResolutionKind::Completed)))
        });
        assert_eq!(calls, 1);
        assert_ne!(observed_during, 0, "expected a request start timestamp");
        assert_eq!(progress.snapshot().request_started_unix, 0);
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
    fn scan_closures_treats_a_cancelled_call_as_stopped_not_a_pass_failure() {
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
        let progress = ScanProgress::default();
        let mut calls = 0;
        scan_closures(&all, &progress, &mut result, |_, _, _| {
            calls += 1;
            Err(ProviderError::Cancelled)
        });
        assert_eq!(calls, 1);
        assert!(result.cancelled);
        assert!(
            result.closure_pass_failure.is_none(),
            "a stopped request is not a closure-pass failure: {:?}",
            result.closure_pass_failure
        );
        assert!(result.analysis.items[0].resolution.is_none());
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
            Err(ProviderError::Network)
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
    fn scan_closures_caps_calls_at_the_configured_maximum_and_notes_it() {
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
