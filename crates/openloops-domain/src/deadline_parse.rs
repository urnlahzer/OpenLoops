//! The deterministic OL-DUE-002 deadline text parser.
//!
//! Backs `docs/product-spec.md` 6.6 (OL-DUE-001..009, the operative-deadline
//! decision table already implemented in [`crate::deadline`], and the
//! operational-aging table) and ADR-008's `deadline_policy`. This module
//! turns an explicit or relative temporal expression into a
//! [`ParsedValue`] tagged with the exact same closed
//! [`crate::facets::DeadlinePrecision`] and
//! [`crate::facets::DeadlineInterpretation`] catalogs [`crate::deadline`]
//! already uses, so the two modules share one precision/interpretation
//! vocabulary rather than each inventing its own.
//!
//! Every function here is pure: the message timestamp and the
//! [`TimezoneContext`] are injected values, never the system clock or an OS
//! timezone database lookup (`OL-DUE-002`: "relative to the message
//! timestamp," never "relative to now"). This mirrors how [`crate::deadline`]
//! takes time only through its injected [`crate::deadline::Clock`] trait.
//!
//! # Dependency decision: no timezone-database crate is activated
//!
//! ADR-008's `deadline_policy` and the product spec both require DST/
//! timezone-aware resolution. `jiff` and `chrono-tz` (or any other IANA
//! tzdata-driven crate) are absent from this environment's offline package
//! cache (`%USERPROFILE%\.cargo\registry\cache\index.crates.io-*\`), and the
//! hard guardrail for this work item prohibits contacting crates.io or any
//! other network origin to fetch one. This module therefore hand-rolls the
//! minimal model the spec actually needs: proleptic-Gregorian civil-calendar
//! arithmetic (the well-known, public-domain `days_from_civil` algorithm;
//! see [`days_from_civil`]) plus a single-transition fixed-offset DST model
//! ([`TimezoneContext`]) whose offset/transition data is supplied by the
//! future settings layer, never looked up from the OS. This is disclosed
//! exactly as [`crate::super::canonical`] disclosed its bounded NFC table: a
//! real IANA tzdata crate would additionally know every historical and
//! future transition worldwide and validate a supplied instant against the
//! correct rule automatically; this model instead requires the caller to
//! supply the one transition (if any) relevant to the date being resolved,
//! and does not itself select which transition applies for a given
//! political timezone or year.
//!
//! This crate keeps its workspace-pinned zero-dependency layer
//! (`contracts/build-skeleton/skeleton.json` `workspace.domain.dependencies`
//! is `[]`): the closed [`TemporalKind`] catalog here deliberately mirrors,
//! rather than imports, `contracts/model/analysis-output.schema.json`
//! `$defs.temporal_hypothesis.kind`. `crates/openloops-inference/src/
//! validation.rs` (the only caller, which already depends on both
//! `openloops-contracts` and this crate) maps the schema-derived enum onto
//! this one at that integration boundary through an exhaustive match with no
//! wildcard arm, so the two catalogs' variant sets stay in lockstep at
//! compile time without this crate gaining a new dependency edge.

use crate::deadline::UnixSeconds;
use crate::facets::{DeadlineInterpretation, DeadlinePrecision};

// ---------------------------------------------------------------------------
// Civil-calendar arithmetic (proleptic Gregorian, pure, no lookup table).
// ---------------------------------------------------------------------------

/// A day count relative to the Unix epoch day (`1970-01-01` is day `0`),
/// proleptic Gregorian. Deliberately not a `(year, month, day)` struct: every
/// operation this module needs (today/tomorrow, weekday search, week-range,
/// business-day stepping, "in N days") is simplest and least error-prone as
/// integer day arithmetic, and [`days_from_civil`] is the only place an
/// explicit calendar date is ever converted into one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct CivilDay(pub i64);

impl CivilDay {
    /// Returns the day `count` days after this one (or before, if negative).
    #[must_use]
    pub const fn add_days(self, count: i64) -> Self {
        Self(self.0 + count)
    }
}

/// `true` for a proleptic-Gregorian leap year.
#[must_use]
const fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The number of days in `month` of `year`, or `0` for an out-of-catalog
/// month (`month` is validated by every caller before this is consulted).
#[must_use]
const fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Whether `(year, month, day)` is a real proleptic-Gregorian calendar date.
#[must_use]
const fn is_valid_civil_date(year: i32, month: u32, day: u32) -> bool {
    month >= 1 && month <= 12 && day >= 1 && day <= days_in_month(year, month)
}

/// Howard Hinnant's `days_from_civil` algorithm
/// (<https://howardhinnant.github.io/date_algorithms.html>): converts a
/// proleptic-Gregorian calendar date into a day count relative to the Unix
/// epoch. Public domain; exact for every date this module can produce
/// (`is_valid_civil_date` is checked by every caller before this runs on
/// externally supplied components).
#[must_use]
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y: i64 = if month <= 2 {
        i64::from(year) - 1
    } else {
        i64::from(year)
    };
    let era: i64 = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era: i64 = y - era * 400;
    let month_index: i64 = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year: i64 = (153 * month_index + 2) / 5 + i64::from(day) - 1;
    let day_of_era: i64 = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `0` (Sunday) through `6` (Saturday). `1970-01-01` (day `0`) is a Thursday.
#[must_use]
const fn weekday_index(day: CivilDay) -> u8 {
    let remainder = day.0.rem_euclid(7);
    // `remainder` is in `0..7` and the added `4` keeps the sum in `0..11`, so
    // this narrowing to `u8` never truncates and is always nonnegative.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let index = ((remainder + 4) % 7) as u8;
    index
}

/// One named weekday, matching [`weekday_index`]'s `0..=6` numbering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Weekday {
    Sunday,
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
}

impl Weekday {
    #[must_use]
    const fn index(self) -> u8 {
        match self {
            Self::Sunday => 0,
            Self::Monday => 1,
            Self::Tuesday => 2,
            Self::Wednesday => 3,
            Self::Thursday => 4,
            Self::Friday => 5,
            Self::Saturday => 6,
        }
    }

    /// Parses a lowercase, full English weekday name; `None` for anything
    /// else (abbreviations and other locales are out of this module's
    /// disclosed scope).
    #[must_use]
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "sunday" => Self::Sunday,
            "monday" => Self::Monday,
            "tuesday" => Self::Tuesday,
            "wednesday" => Self::Wednesday,
            "thursday" => Self::Thursday,
            "friday" => Self::Friday,
            "saturday" => Self::Saturday,
            _ => return None,
        })
    }
}

/// `true` for a weekday that is not Saturday/Sunday, under the plain
/// Mon-Fri business-day model this module discloses (no holiday calendar;
/// `docs/product-spec.md` 6.6 leaves locale/holiday business-day policy to a
/// later work item).
#[must_use]
const fn is_business_day(day: CivilDay) -> bool {
    let index = weekday_index(day);
    index != 0 && index != 6
}

/// The next business day strictly after `from`.
#[must_use]
const fn next_business_day(from: CivilDay) -> CivilDay {
    let mut day = CivilDay(from.0 + 1);
    while !is_business_day(day) {
        day = CivilDay(day.0 + 1);
    }
    day
}

/// Steps `count` business days forward from `from` (`count` business days
/// are consumed one at a time; `from` itself is never counted).
#[must_use]
const fn add_business_days(from: CivilDay, count: u32) -> CivilDay {
    let mut day = from;
    let mut remaining = count;
    while remaining > 0 {
        day = next_business_day(day);
        remaining -= 1;
    }
    day
}

/// The next occurrence of `target` on or after `from`; when `strictly_after`
/// is set, `from` itself is never returned even if it already matches.
#[must_use]
fn next_weekday(from: CivilDay, target: Weekday, strictly_after: bool) -> CivilDay {
    let mut day = if strictly_after {
        CivilDay(from.0 + 1)
    } else {
        from
    };
    while weekday_index(day) != target.index() {
        day = CivilDay(day.0 + 1);
    }
    day
}

/// The most recent occurrence of `target` on or before `from`.
#[must_use]
fn previous_or_same_weekday(from: CivilDay, target: Weekday) -> CivilDay {
    let mut day = from;
    while weekday_index(day) != target.index() {
        day = CivilDay(day.0 - 1);
    }
    day
}

// ---------------------------------------------------------------------------
// Timezone/DST context and local-time resolution.
// ---------------------------------------------------------------------------

/// One instantaneous UTC offset change, e.g. a spring-forward or fall-back
/// transition. `offset_after_seconds > offset_before_seconds` models a
/// spring-forward (clocks move later); `<` models a fall-back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DstTransition {
    /// The UTC instant the offset changes.
    pub transition_instant: UnixSeconds,
    /// The UTC offset, in seconds, in force strictly before
    /// `transition_instant`.
    pub offset_before_seconds: i32,
    /// The UTC offset, in seconds, in force at and after
    /// `transition_instant`.
    pub offset_after_seconds: i32,
}

/// The injected offset/DST data a caller supplies for one timezone, in place
/// of an OS timezone-database lookup. `transition` is `None` for a
/// fixed-offset zone with no DST in force around the date being resolved;
/// `Some` models at most one transition, which must be the transition
/// nearest the date being resolved (see the module-level dependency-decision
/// note for why this crate does not select that transition itself).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimezoneContext {
    /// The UTC offset, in seconds, that applies with no configured
    /// transition, or before/after the configured transition when one is
    /// supplied but a wall-clock instant is not addressed by it.
    pub base_offset_seconds: i32,
    /// At most one configured DST transition.
    pub transition: Option<DstTransition>,
}

/// The UTC offset actually in force at `instant`, choosing between
/// `transition.offset_before_seconds`/`offset_after_seconds` when a
/// transition is configured.
#[must_use]
const fn offset_at(instant: UnixSeconds, timezone: &TimezoneContext) -> i32 {
    match timezone.transition {
        None => timezone.base_offset_seconds,
        Some(transition) => {
            if instant.0 >= transition.transition_instant.0 {
                transition.offset_after_seconds
            } else {
                transition.offset_before_seconds
            }
        }
    }
}

/// Why a wall-clock local time did not resolve to exactly one UTC instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DstAmbiguityReason {
    /// The wall-clock hour is repeated (a fall-back overlap): both
    /// `candidates` are genuine, distinct realizations of that local time.
    RepeatedWallClockHour,
    /// The wall-clock time never occurs (a spring-forward gap): neither
    /// `candidates` value is a real occurrence of that local time; both are
    /// offered only as the two natural readings (as if the transition had
    /// not yet happened, and as if it already had).
    NonexistentWallClockTime,
}

/// The outcome of resolving one local wall-clock time against a
/// [`TimezoneContext`]: exactly one UTC instant, or a typed ambiguity with
/// alternatives. Never a silent pick (`docs/product-spec.md` 6.6:
/// "Present alternatives ... no automatic reminder until resolved").
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalResolution {
    /// Exactly one UTC instant realizes this local time.
    Unambiguous(UnixSeconds),
    /// This local time resolves to more than one, or to zero, UTC instants.
    Ambiguous {
        /// The two candidate instants: the reading under the offset before
        /// the configured transition, then the reading under the offset
        /// after it — in that order regardless of `reason`.
        candidates: [UnixSeconds; 2],
        reason: DstAmbiguityReason,
    },
}

/// Resolves one local wall-clock time (`day` at `seconds_since_midnight`
/// local) against `timezone`.
///
/// With no configured transition this is a single subtraction. With a
/// configured transition, both readings are computed and cross-checked
/// against `transition_instant`: a reading is self-consistent only if the
/// offset it assumes is the one actually in force at the UTC instant it
/// produces. Exactly one self-consistent reading is the unambiguous case;
/// both or neither self-consistent is the fall-back/spring-forward ambiguous
/// case respectively.
#[must_use]
pub fn resolve_local(
    day: CivilDay,
    seconds_since_midnight: u32,
    timezone: &TimezoneContext,
) -> LocalResolution {
    let naive = day.0 * 86_400 + i64::from(seconds_since_midnight);
    match timezone.transition {
        None => LocalResolution::Unambiguous(UnixSeconds(
            naive - i64::from(timezone.base_offset_seconds),
        )),
        Some(transition) => {
            let candidate_before = naive - i64::from(transition.offset_before_seconds);
            let candidate_after = naive - i64::from(transition.offset_after_seconds);
            let valid_before = candidate_before < transition.transition_instant.0;
            let valid_after = candidate_after >= transition.transition_instant.0;
            match (valid_before, valid_after) {
                (true, false) => LocalResolution::Unambiguous(UnixSeconds(candidate_before)),
                (false, true) => LocalResolution::Unambiguous(UnixSeconds(candidate_after)),
                (true, true) => LocalResolution::Ambiguous {
                    candidates: [UnixSeconds(candidate_before), UnixSeconds(candidate_after)],
                    reason: DstAmbiguityReason::RepeatedWallClockHour,
                },
                (false, false) => LocalResolution::Ambiguous {
                    candidates: [UnixSeconds(candidate_before), UnixSeconds(candidate_after)],
                    reason: DstAmbiguityReason::NonexistentWallClockTime,
                },
            }
        }
    }
}

/// `deadline_policy.default_eod`: `17:00` local, expressed as seconds since
/// local midnight.
pub const DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT: u32 = 17 * 3600;

/// The injected parsing context: the message's own timestamp/timezone
/// (`OL-DUE-002`) plus the configured EOD and week-start settings
/// (`deadline_policy.default_eod`; week-start is not separately pinned by
/// the current contract snapshot and defaults to Monday here, configurable
/// per caller). Never the system clock or an OS timezone lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseContext {
    /// The message's own timestamp; every relative expression resolves
    /// against this, never against the wall-clock "now" (`OL-DUE-002`).
    pub message_timestamp: UnixSeconds,
    pub timezone: TimezoneContext,
    /// `deadline_policy.default_eod`, configurable;
    /// [`DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT`] is the spec default.
    pub eod_seconds_since_midnight: u32,
    /// Which weekday a "week" starts on for `this week`/`next week`.
    pub week_start: Weekday,
}

/// The message's own local calendar day, resolved from `message_timestamp`
/// via whichever offset is actually in force at that instant (never
/// ambiguous: a message was actually sent/received at one real instant, so
/// only [`offset_at`]'s single deterministic choice applies, not
/// [`resolve_local`]'s two-reading cross-check).
#[must_use]
fn message_local_day(context: &ParseContext) -> CivilDay {
    let offset = offset_at(context.message_timestamp, &context.timezone);
    let local_seconds = context.message_timestamp.0 + i64::from(offset);
    CivilDay(local_seconds.div_euclid(86_400))
}

// ---------------------------------------------------------------------------
// The closed temporal-hypothesis kind catalog (see the dependency-decision
// note above for why this mirrors, rather than imports, the schema enum).
// ---------------------------------------------------------------------------

/// Mirrors `contracts/model/analysis-output.schema.json`
/// `$defs.temporal_hypothesis.kind` without importing it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemporalKind {
    Date,
    LocalDatetime,
    Relative,
    EventRelative,
    SoftWindow,
}

/// Every reason this module's deterministic reparse can fail to reproduce a
/// hypothesis. Typed and content-free: no variant carries the rejected text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseRejection {
    /// The text did not match any grammar this module recognizes for the
    /// declared `kind`.
    UnrecognizedText,
    /// The text named a calendar date or time-of-day component that is out
    /// of range (e.g. month 13, February 30, hour 25).
    InvalidCalendarComponent,
    /// A relative count (`"in N days"`) was not a parseable, in-bounds
    /// unsigned integer.
    CountOutOfRange,
}

/// The maximum `N` this module accepts in `"in N days"`/`"in N business
/// days"` (roughly ten years); larger values are rejected rather than
/// silently wrapping.
const MAXIMUM_RELATIVE_DAY_COUNT: u32 = 3_650;

/// One parsed temporal value, tagged with the exact catalogs
/// [`crate::deadline`] already uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParsedValue {
    /// An exact instant (`local_datetime`, or a relative expression that
    /// names a specific time such as `"eod"`).
    Instant(LocalResolution),
    /// A calendar date with no time-of-day (`date`, or a bare relative day
    /// name/offset such as `"tomorrow"`/`"in 3 days"`).
    Date(CivilDay),
    /// The end of a resolved business day.
    BusinessDay(CivilDay),
    /// A week/range, inclusive of both `start` and `end`.
    Week { start: CivilDay, end: CivilDay },
    /// Relative to an event with no correlation input; this module never
    /// resolves this to a boundary on its own (`docs/product-spec.md` 6.6:
    /// "before the meeting" without a unique event stays reviewable).
    EventRelative,
    /// Soft/inferred urgency (`ASAP`, "when you can") with no aging
    /// boundary.
    Soft,
}

impl ParsedValue {
    /// The [`DeadlinePrecision`] this value preserves; never upgrades a
    /// `Date`/`Week`/`Soft`/`EventRelative` value into a fabricated
    /// `Instant` (`OL-DUE-004`).
    #[must_use]
    pub const fn precision(&self) -> DeadlinePrecision {
        match self {
            Self::Instant(_) => DeadlinePrecision::Instant,
            Self::Date(_) => DeadlinePrecision::Date,
            Self::BusinessDay(_) => DeadlinePrecision::BusinessDay,
            Self::Week { .. } => DeadlinePrecision::Week,
            Self::EventRelative => DeadlinePrecision::EventRelative,
            Self::Soft => DeadlinePrecision::Soft,
        }
    }

    /// The [`DeadlineInterpretation`] this value carries. `EventRelative` is
    /// always [`DeadlineInterpretation::NeedsUserInput`] because this module
    /// never resolves it without an externally supplied correlation; an
    /// `Instant` is `Resolved`/`Ambiguous` per its [`LocalResolution`]; every
    /// other precision has nothing left to disambiguate once parsed.
    #[must_use]
    pub const fn interpretation(&self) -> DeadlineInterpretation {
        match self {
            Self::Instant(LocalResolution::Unambiguous(_)) => DeadlineInterpretation::Resolved,
            Self::Instant(LocalResolution::Ambiguous { .. }) => DeadlineInterpretation::Ambiguous,
            Self::EventRelative => DeadlineInterpretation::NeedsUserInput,
            Self::Date(_) | Self::BusinessDay(_) | Self::Week { .. } | Self::Soft => {
                DeadlineInterpretation::Resolved
            }
        }
    }
}

/// The operational-aging policy boundary (`docs/product-spec.md` 6.6's
/// aging table) for a `Date`/`BusinessDay`/`Week` value, or `None` for a
/// value the aging table does not project a boundary from at all
/// (`Instant` already is one; `EventRelative` has none until correlated;
/// `Soft` never ages). This is deliberately a *separate* projection from
/// `value` itself: the evidence stays `Date`/`BusinessDay`/`Week` precision
/// forever, and only this derived boundary is an instant.
#[must_use]
pub fn policy_boundary(value: &ParsedValue, context: &ParseContext) -> Option<LocalResolution> {
    match value {
        ParsedValue::Date(day) | ParsedValue::BusinessDay(day) => Some(resolve_local(
            *day,
            context.eod_seconds_since_midnight,
            &context.timezone,
        )),
        ParsedValue::Week { end, .. } => Some(resolve_local(
            *end,
            context.eod_seconds_since_midnight,
            &context.timezone,
        )),
        ParsedValue::Instant(_) | ParsedValue::EventRelative | ParsedValue::Soft => None,
    }
}

fn instant_at(day: CivilDay, context: &ParseContext) -> ParsedValue {
    ParsedValue::Instant(resolve_local(
        day,
        context.eod_seconds_since_midnight,
        &context.timezone,
    ))
}

fn week_range(today: CivilDay, week_start: Weekday, next: bool) -> (CivilDay, CivilDay) {
    let this_week_start = previous_or_same_weekday(today, week_start);
    let start = if next {
        this_week_start.add_days(7)
    } else {
        this_week_start
    };
    (start, start.add_days(6))
}

fn parse_bounded_count(token: &str) -> Result<u32, ParseRejection> {
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseRejection::UnrecognizedText);
    }
    let count: u32 = token.parse().map_err(|_| ParseRejection::CountOutOfRange)?;
    if count > MAXIMUM_RELATIVE_DAY_COUNT {
        return Err(ParseRejection::CountOutOfRange);
    }
    Ok(count)
}

/// Parses the closed set of `soft_window` phrases; anything else rejects
/// rather than silently guessing at urgency (`OL-DUE-006`).
fn parse_soft(value: &str) -> Result<ParsedValue, ParseRejection> {
    match value.trim().to_ascii_lowercase().as_str() {
        "asap" | "when you can" | "when you get a chance" => Ok(ParsedValue::Soft),
        _ => Err(ParseRejection::UnrecognizedText),
    }
}

fn is_ascii_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

/// Parses a strict `"YYYY-MM-DD"` explicit date (exactly ten ASCII bytes,
/// dashes at positions 4 and 7, every other position a digit).
fn parse_date_components(value: &str) -> Result<(i32, u32, u32), ParseRejection> {
    let bytes = value.as_bytes();
    if value.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(ParseRejection::UnrecognizedText);
    }
    let (year_text, month_text, day_text) = (&value[0..4], &value[5..7], &value[8..10]);
    if !is_ascii_digits(year_text) || !is_ascii_digits(month_text) || !is_ascii_digits(day_text) {
        return Err(ParseRejection::UnrecognizedText);
    }
    let year: i32 = year_text
        .parse()
        .map_err(|_| ParseRejection::UnrecognizedText)?;
    let month: u32 = month_text
        .parse()
        .map_err(|_| ParseRejection::UnrecognizedText)?;
    let day: u32 = day_text
        .parse()
        .map_err(|_| ParseRejection::UnrecognizedText)?;
    if !is_valid_civil_date(year, month, day) {
        return Err(ParseRejection::InvalidCalendarComponent);
    }
    Ok((year, month, day))
}

fn parse_time_components(value: &str) -> Result<(u32, u32, u32), ParseRejection> {
    let parts: Vec<&str> = value.split(':').collect();
    let (hour_text, minute_text, second_text) = match parts.as_slice() {
        [hour, minute] => (*hour, *minute, "00"),
        [hour, minute, second] => (*hour, *minute, *second),
        _ => return Err(ParseRejection::UnrecognizedText),
    };
    if !is_ascii_digits(hour_text) || !is_ascii_digits(minute_text) || !is_ascii_digits(second_text)
    {
        return Err(ParseRejection::UnrecognizedText);
    }
    let hour: u32 = hour_text
        .parse()
        .map_err(|_| ParseRejection::UnrecognizedText)?;
    let minute: u32 = minute_text
        .parse()
        .map_err(|_| ParseRejection::UnrecognizedText)?;
    let second: u32 = second_text
        .parse()
        .map_err(|_| ParseRejection::UnrecognizedText)?;
    if hour > 23 || minute > 59 || second > 59 {
        return Err(ParseRejection::InvalidCalendarComponent);
    }
    Ok((hour, minute, second))
}

fn parse_explicit_date(value: &str) -> Result<ParsedValue, ParseRejection> {
    let (year, month, day) = parse_date_components(value)?;
    Ok(ParsedValue::Date(CivilDay(days_from_civil(
        year, month, day,
    ))))
}

fn parse_explicit_datetime(
    value: &str,
    context: &ParseContext,
) -> Result<ParsedValue, ParseRejection> {
    if value.len() < 12 {
        return Err(ParseRejection::UnrecognizedText);
    }
    let separator = value.as_bytes()[10];
    if separator != b'T' && separator != b' ' {
        return Err(ParseRejection::UnrecognizedText);
    }
    let (year, month, day) = parse_date_components(&value[0..10])?;
    let (hour, minute, second) = parse_time_components(&value[11..])?;
    let day = CivilDay(days_from_civil(year, month, day));
    let seconds_since_midnight = hour * 3600 + minute * 60 + second;
    Ok(instant_at(
        day,
        &ParseContext {
            eod_seconds_since_midnight: seconds_since_midnight,
            ..*context
        },
    ))
}

/// Parses the closed set of relative expressions this module recognizes:
/// `today`/`tomorrow`, a bare weekday name, `next week`/`this week`/`end of
/// week`, `next business day`, `in N days`/`in N business days`, each
/// optionally paired with `eod`, plus the bare soft phrases. Anything else
/// rejects (`OL-DUE-002`/`OL-DUE-009`: an interpretation the deterministic
/// parser cannot reproduce must route to review, never be accepted on the
/// model's word alone).
fn parse_relative(value: &str, context: &ParseContext) -> Result<ParsedValue, ParseRejection> {
    let lowered = value.trim().to_ascii_lowercase();
    let tokens: Vec<&str> = lowered.split_whitespace().collect();
    let today = message_local_day(context);
    match tokens.as_slice() {
        ["today"] => Ok(ParsedValue::Date(today)),
        ["today", "eod"] | ["eod", "today"] | ["eod"] => Ok(instant_at(today, context)),
        ["tomorrow"] => Ok(ParsedValue::Date(today.add_days(1))),
        ["tomorrow", "eod"] | ["eod", "tomorrow"] => Ok(instant_at(today.add_days(1), context)),
        ["next", "week"] => {
            let (start, end) = week_range(today, context.week_start, true);
            Ok(ParsedValue::Week { start, end })
        }
        ["this", "week"] | ["end", "of", "week"] => {
            let (start, end) = week_range(today, context.week_start, false);
            Ok(ParsedValue::Week { start, end })
        }
        ["next", "business", "day"] => Ok(ParsedValue::BusinessDay(next_business_day(today))),
        ["asap"] | ["when", "you", "can"] | ["when", "you", "get", "a", "chance"] => {
            Ok(ParsedValue::Soft)
        }
        [weekday, "eod"] | ["eod", weekday] if Weekday::from_name(weekday).is_some() => {
            let target = Weekday::from_name(weekday).expect("guarded by the match arm");
            Ok(instant_at(next_weekday(today, target, true), context))
        }
        [weekday] if Weekday::from_name(weekday).is_some() => {
            let target = Weekday::from_name(weekday).expect("guarded by the match arm");
            Ok(ParsedValue::Date(next_weekday(today, target, true)))
        }
        ["in", count, "business", "days"] => {
            let count = parse_bounded_count(count)?;
            Ok(ParsedValue::BusinessDay(add_business_days(today, count)))
        }
        ["in", count, "days"] => {
            let count = parse_bounded_count(count)?;
            Ok(ParsedValue::Date(today.add_days(i64::from(count))))
        }
        _ => Err(ParseRejection::UnrecognizedText),
    }
}

/// Deterministically reparses one temporal hypothesis's `value` under its
/// declared `kind`, relative to `context`.
///
/// This is ADR-007 validation step 8's "deterministic date reparse hook":
/// the caller (`crates/openloops-inference/src/validation.rs`) treats any
/// [`Err`] here as proof the model's interpretation could not be
/// independently reproduced and routes the claim to review rather than
/// accepting the model's reading. `kind = EventRelative` always succeeds
/// with [`ParsedValue::EventRelative`] regardless of `value`'s text: this
/// module never resolves an event-relative reference from text alone
/// (correlating it against a real calendar event is out of this crate's
/// scope entirely), so a claim's boundary stays reviewable either way.
///
/// # Errors
///
/// Returns [`ParseRejection`] when `value` does not match the grammar this
/// module recognizes for the declared `kind`.
pub fn reparse(
    kind: TemporalKind,
    value: &str,
    context: &ParseContext,
) -> Result<ParsedValue, ParseRejection> {
    match kind {
        TemporalKind::EventRelative => Ok(ParsedValue::EventRelative),
        TemporalKind::SoftWindow => parse_soft(value),
        TemporalKind::Date => parse_explicit_date(value),
        TemporalKind::LocalDatetime => parse_explicit_datetime(value, context),
        TemporalKind::Relative => parse_relative(value, context),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CivilDay, DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT, DstAmbiguityReason, DstTransition,
        LocalResolution, ParseContext, ParseRejection, ParsedValue, TemporalKind, TimezoneContext,
        Weekday, days_from_civil, policy_boundary, reparse, resolve_local, weekday_index,
    };
    use crate::deadline::UnixSeconds;
    use crate::facets::{DeadlineInterpretation, DeadlinePrecision};

    fn fixed_offset(seconds: i32) -> TimezoneContext {
        TimezoneContext {
            base_offset_seconds: seconds,
            transition: None,
        }
    }

    /// A message sent 2026-08-03 (a Monday) at 09:00 UTC-5 (no DST).
    fn base_context() -> ParseContext {
        // 2026-08-03T09:00:00-05:00 == 2026-08-03T14:00:00Z.
        let day = CivilDay(days_from_civil(2026, 8, 3));
        let timestamp = UnixSeconds(day.0 * 86_400 + 14 * 3600);
        ParseContext {
            message_timestamp: timestamp,
            timezone: fixed_offset(-5 * 3600),
            eod_seconds_since_midnight: DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT,
            week_start: Weekday::Monday,
        }
    }

    #[test]
    fn days_from_civil_matches_known_epoch_and_millennium_values() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        // 2000-01-01 is a well-known 10_957 days after the Unix epoch.
        assert_eq!(days_from_civil(2000, 1, 1), 10_957);
    }

    #[test]
    fn weekday_index_matches_known_anchor_days() {
        assert_eq!(weekday_index(CivilDay(0)), 4); // 1970-01-01 Thursday
        // 2000-01-01 was a Saturday.
        assert_eq!(weekday_index(CivilDay(days_from_civil(2000, 1, 1))), 6);
    }

    #[test]
    fn explicit_date_preserves_date_precision_never_becomes_an_instant() {
        let context = base_context();
        let parsed = reparse(TemporalKind::Date, "2026-08-10", &context).unwrap();
        assert_eq!(parsed.precision(), DeadlinePrecision::Date);
        assert_eq!(parsed.interpretation(), DeadlineInterpretation::Resolved);
        assert_eq!(
            parsed,
            ParsedValue::Date(CivilDay(days_from_civil(2026, 8, 10)))
        );
    }

    #[test]
    fn invalid_calendar_date_rejects() {
        let context = base_context();
        assert_eq!(
            reparse(TemporalKind::Date, "2026-02-30", &context),
            Err(ParseRejection::InvalidCalendarComponent)
        );
        assert_eq!(
            reparse(TemporalKind::Date, "2026-13-01", &context),
            Err(ParseRejection::InvalidCalendarComponent)
        );
    }

    #[test]
    fn explicit_datetime_resolves_to_an_instant_using_the_supplied_offset() {
        let context = base_context();
        let parsed = reparse(TemporalKind::LocalDatetime, "2026-08-10T09:30", &context).unwrap();
        assert_eq!(parsed.precision(), DeadlinePrecision::Instant);
        let expected = days_from_civil(2026, 8, 10) * 86_400 + 9 * 3600 + 30 * 60 + 5 * 3600;
        assert_eq!(
            parsed,
            ParsedValue::Instant(LocalResolution::Unambiguous(UnixSeconds(expected)))
        );
    }

    #[test]
    fn kind_text_mismatch_routes_to_review_via_rejection() {
        let context = base_context();
        // Declared kind is `date` but the text is a relative/soft phrase:
        // the deterministic parser for `date` only tries the date grammar
        // and must reject rather than silently falling back.
        assert!(reparse(TemporalKind::Date, "asap", &context).is_err());
        assert!(reparse(TemporalKind::SoftWindow, "2026-08-10", &context).is_err());
    }

    #[test]
    fn today_and_tomorrow_are_relative_to_the_message_timestamp_not_now() {
        let context = base_context(); // message local day: 2026-08-03.
        assert_eq!(
            reparse(TemporalKind::Relative, "today", &context).unwrap(),
            ParsedValue::Date(CivilDay(days_from_civil(2026, 8, 3)))
        );
        assert_eq!(
            reparse(TemporalKind::Relative, "tomorrow", &context).unwrap(),
            ParsedValue::Date(CivilDay(days_from_civil(2026, 8, 4)))
        );
    }

    #[test]
    fn bare_weekday_name_means_the_next_strictly_future_occurrence() {
        let context = base_context(); // message is itself a Monday.
        // "monday" from a Monday message means *next* Monday, one week out,
        // never the same day.
        let parsed = reparse(TemporalKind::Relative, "Monday", &context).unwrap();
        assert_eq!(
            parsed,
            ParsedValue::Date(CivilDay(days_from_civil(2026, 8, 3) + 7))
        );
        let friday = reparse(TemporalKind::Relative, "friday", &context).unwrap();
        assert_eq!(
            friday,
            ParsedValue::Date(CivilDay(days_from_civil(2026, 8, 7)))
        );
    }

    #[test]
    fn this_week_and_next_week_stay_a_range_never_an_instant() {
        let context = base_context();
        let this_week = reparse(TemporalKind::Relative, "this week", &context).unwrap();
        assert_eq!(this_week.precision(), DeadlinePrecision::Week);
        assert_eq!(
            this_week,
            ParsedValue::Week {
                start: CivilDay(days_from_civil(2026, 8, 3)),
                end: CivilDay(days_from_civil(2026, 8, 9)),
            }
        );
        let next_week = reparse(TemporalKind::Relative, "next week", &context).unwrap();
        assert_eq!(
            next_week,
            ParsedValue::Week {
                start: CivilDay(days_from_civil(2026, 8, 10)),
                end: CivilDay(days_from_civil(2026, 8, 16)),
            }
        );
        let end_of_week = reparse(TemporalKind::Relative, "end of week", &context).unwrap();
        assert_eq!(end_of_week, this_week);
    }

    #[test]
    fn eod_defaults_to_seventeen_hundred_and_is_configurable() {
        let context = base_context();
        let parsed = reparse(TemporalKind::Relative, "eod", &context).unwrap();
        let expected = days_from_civil(2026, 8, 3) * 86_400 + 17 * 3600 + 5 * 3600;
        assert_eq!(
            parsed,
            ParsedValue::Instant(LocalResolution::Unambiguous(UnixSeconds(expected)))
        );
        let mut configured = context;
        configured.eod_seconds_since_midnight = 20 * 3600;
        let reconfigured = reparse(TemporalKind::Relative, "eod", &configured).unwrap();
        let expected_reconfigured = days_from_civil(2026, 8, 3) * 86_400 + 20 * 3600 + 5 * 3600;
        assert_eq!(
            reconfigured,
            ParsedValue::Instant(LocalResolution::Unambiguous(UnixSeconds(
                expected_reconfigured
            )))
        );
    }

    #[test]
    fn next_business_day_and_in_n_business_days_skip_weekends() {
        let context = base_context(); // Monday 2026-08-03.
        // Friday 2026-08-07 + "next business day" skips the weekend to Monday.
        let friday_context = ParseContext {
            message_timestamp: UnixSeconds(
                days_from_civil(2026, 8, 7) * 86_400 + 14 * 3600 + 5 * 3600,
            ),
            ..context
        };
        let parsed = reparse(TemporalKind::Relative, "next business day", &friday_context).unwrap();
        assert_eq!(
            parsed,
            ParsedValue::BusinessDay(CivilDay(days_from_civil(2026, 8, 10)))
        );
        let three_business_days = reparse(
            TemporalKind::Relative,
            "in 3 business days",
            &friday_context,
        )
        .unwrap();
        // Fri -> Mon, Tue, Wed.
        assert_eq!(
            three_business_days,
            ParsedValue::BusinessDay(CivilDay(days_from_civil(2026, 8, 12)))
        );
    }

    #[test]
    fn in_n_days_stays_date_precision_and_rejects_out_of_range_counts() {
        let context = base_context();
        let parsed = reparse(TemporalKind::Relative, "in 5 days", &context).unwrap();
        assert_eq!(parsed.precision(), DeadlinePrecision::Date);
        assert_eq!(
            parsed,
            ParsedValue::Date(CivilDay(days_from_civil(2026, 8, 3) + 5))
        );
        assert_eq!(
            reparse(TemporalKind::Relative, "in 999999 days", &context),
            Err(ParseRejection::CountOutOfRange)
        );
    }

    #[test]
    fn soft_phrases_never_age_and_event_relative_never_resolves_from_text_alone() {
        let context = base_context();
        for phrase in ["asap", "when you can", "when you get a chance", "ASAP"] {
            let parsed = reparse(TemporalKind::SoftWindow, phrase, &context).unwrap();
            assert_eq!(parsed, ParsedValue::Soft);
            assert_eq!(parsed.precision(), DeadlinePrecision::Soft);
        }
        // Any text at all under `event_relative` stays unresolved; only an
        // external correlation (out of this module's scope) can change that.
        for text in ["before the meeting", "garbage", ""] {
            let parsed = reparse(TemporalKind::EventRelative, text, &context).unwrap();
            assert_eq!(parsed, ParsedValue::EventRelative);
            assert_eq!(
                parsed.interpretation(),
                DeadlineInterpretation::NeedsUserInput
            );
        }
    }

    #[test]
    fn spring_forward_gap_and_fall_back_overlap_both_route_typed_ambiguity() {
        // US-style spring-forward: 2026-03-08 02:00 EST (-05:00) becomes
        // 03:00 EDT (-04:00); the transition instant is 2026-03-08T07:00Z.
        let spring_transition_day = CivilDay(days_from_civil(2026, 3, 8));
        let spring = TimezoneContext {
            base_offset_seconds: -5 * 3600,
            transition: Some(DstTransition {
                transition_instant: UnixSeconds(spring_transition_day.0 * 86_400 + 7 * 3600),
                offset_before_seconds: -5 * 3600,
                offset_after_seconds: -4 * 3600,
            }),
        };
        // 02:30 local never occurs (the gap).
        let gap = resolve_local(spring_transition_day, 2 * 3600 + 30 * 60, &spring);
        assert_eq!(
            gap,
            LocalResolution::Ambiguous {
                candidates: [
                    UnixSeconds(spring_transition_day.0 * 86_400 + 2 * 3600 + 30 * 60 + 5 * 3600),
                    UnixSeconds(spring_transition_day.0 * 86_400 + 2 * 3600 + 30 * 60 + 4 * 3600),
                ],
                reason: DstAmbiguityReason::NonexistentWallClockTime,
            }
        );
        // 01:30 local (before the gap) is unambiguous standard time.
        let before_gap = resolve_local(spring_transition_day, 3600 + 30 * 60, &spring);
        assert_eq!(
            before_gap,
            LocalResolution::Unambiguous(UnixSeconds(
                spring_transition_day.0 * 86_400 + 3600 + 30 * 60 + 5 * 3600
            ))
        );
        // 03:30 local (after the gap) is unambiguous daylight time.
        let after_gap = resolve_local(spring_transition_day, 3 * 3600 + 30 * 60, &spring);
        assert_eq!(
            after_gap,
            LocalResolution::Unambiguous(UnixSeconds(
                spring_transition_day.0 * 86_400 + 3 * 3600 + 30 * 60 + 4 * 3600
            ))
        );

        // US-style fall-back: 2026-11-01 02:00 EDT (-04:00) becomes 01:00
        // EST (-05:00); the transition instant is 2026-11-01T06:00Z.
        let fall_transition_day = CivilDay(days_from_civil(2026, 11, 1));
        let fall = TimezoneContext {
            base_offset_seconds: -4 * 3600,
            transition: Some(DstTransition {
                transition_instant: UnixSeconds(fall_transition_day.0 * 86_400 + 6 * 3600),
                offset_before_seconds: -4 * 3600,
                offset_after_seconds: -5 * 3600,
            }),
        };
        // 01:30 local occurs twice (the repeated hour).
        let repeated = resolve_local(fall_transition_day, 3600 + 30 * 60, &fall);
        assert_eq!(
            repeated,
            LocalResolution::Ambiguous {
                candidates: [
                    UnixSeconds(fall_transition_day.0 * 86_400 + 3600 + 30 * 60 + 4 * 3600),
                    UnixSeconds(fall_transition_day.0 * 86_400 + 3600 + 30 * 60 + 5 * 3600),
                ],
                reason: DstAmbiguityReason::RepeatedWallClockHour,
            }
        );
        // 00:30 local (before the repeat) is unambiguous daylight time.
        let before_repeat = resolve_local(fall_transition_day, 30 * 60, &fall);
        assert_eq!(
            before_repeat,
            LocalResolution::Unambiguous(UnixSeconds(
                fall_transition_day.0 * 86_400 + 30 * 60 + 4 * 3600
            ))
        );
        // 02:30 local (after the repeat) is unambiguous standard time.
        let after_repeat = resolve_local(fall_transition_day, 2 * 3600 + 30 * 60, &fall);
        assert_eq!(
            after_repeat,
            LocalResolution::Unambiguous(UnixSeconds(
                fall_transition_day.0 * 86_400 + 2 * 3600 + 30 * 60 + 5 * 3600
            ))
        );
    }

    #[test]
    fn policy_boundary_is_a_separate_instant_projection_from_the_preserved_evidence() {
        let context = base_context();
        let date_value = ParsedValue::Date(CivilDay(days_from_civil(2026, 8, 10)));
        assert_eq!(date_value.precision(), DeadlinePrecision::Date);
        let boundary = policy_boundary(&date_value, &context).unwrap();
        let expected = days_from_civil(2026, 8, 10) * 86_400 + 17 * 3600 + 5 * 3600;
        assert_eq!(
            boundary,
            LocalResolution::Unambiguous(UnixSeconds(expected))
        );
        // The evidence itself is untouched by computing its policy boundary.
        assert_eq!(date_value.precision(), DeadlinePrecision::Date);

        let week_value = ParsedValue::Week {
            start: CivilDay(days_from_civil(2026, 8, 3)),
            end: CivilDay(days_from_civil(2026, 8, 9)),
        };
        let week_boundary = policy_boundary(&week_value, &context).unwrap();
        let expected_week_end = days_from_civil(2026, 8, 9) * 86_400 + 17 * 3600 + 5 * 3600;
        assert_eq!(
            week_boundary,
            LocalResolution::Unambiguous(UnixSeconds(expected_week_end))
        );

        assert!(policy_boundary(&ParsedValue::Soft, &context).is_none());
        assert!(policy_boundary(&ParsedValue::EventRelative, &context).is_none());
    }

    #[test]
    fn determinism_property_loop_over_a_bounded_synthetic_input_space() {
        let bases = [
            ("today", TemporalKind::Relative),
            ("tomorrow", TemporalKind::Relative),
            ("monday", TemporalKind::Relative),
            ("friday eod", TemporalKind::Relative),
            ("next week", TemporalKind::Relative),
            ("this week", TemporalKind::Relative),
            ("next business day", TemporalKind::Relative),
            ("in 4 days", TemporalKind::Relative),
            ("in 2 business days", TemporalKind::Relative),
            ("asap", TemporalKind::SoftWindow),
            ("before the meeting", TemporalKind::EventRelative),
            ("2026-08-10", TemporalKind::Date),
            ("2026-08-10T09:30", TemporalKind::LocalDatetime),
            ("not a date", TemporalKind::Date),
        ];
        for message_offset_days in 0i64..10 {
            let mut context = base_context();
            context.message_timestamp =
                UnixSeconds(context.message_timestamp.0 + message_offset_days * 86_400);
            for (text, kind) in bases {
                let first = reparse(kind, text, &context);
                let second = reparse(kind, text, &context);
                assert_eq!(
                    first, second,
                    "reparse must be a pure function of its input"
                );
            }
        }
    }
}
