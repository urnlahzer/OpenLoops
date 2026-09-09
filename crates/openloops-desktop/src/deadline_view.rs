//! Pure deadline aging relative to the source message and a fixed local offset.
use openloops_domain::deadline::UnixSeconds;
use openloops_domain::deadline_parse::{
    DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT, LocalResolution, ParseContext, ParsedValue, TemporalKind,
    TimezoneContext, Weekday, policy_boundary, reparse,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineView {
    PastDue {
        boundary: i64,
        offset_seconds: i32,
    },
    Due {
        boundary: i64,
        offset_seconds: i32,
    },
    DueDate {
        day: i64,
        /// The instant [`classify`] itself resolved as the deadline's
        /// boundary (the day's local end-of-day, converted to UTC) -- kept
        /// alongside `day` so a caller aging a stated event time (see
        /// `review_scan::past_due_boundary`) uses the SAME boundary the
        /// `past` flag above was decided against, rather than recomputing a
        /// UTC-midnight approximation that ignores both the end-of-day
        /// policy and the local offset.
        boundary: i64,
        past: bool,
    },
    DueBusinessDay {
        day: i64,
        boundary: i64,
        past: bool,
    },
    DueRange {
        start_day: i64,
        end_day: i64,
        boundary: i64,
        past: bool,
    },
    EventTied,
    Soft,
    Unknown,
}

/// Generic event nouns naming the KIND of gathering rather than which one --
/// shared with [`crate::review_scan`]'s event-name matching (one list, so
/// the words that make [`classify`] fall back to [`DeadlineView::EventTied`]
/// are exactly the words `match_event` strips as too common to distinguish
/// one event from another).
pub const EVENT_GENERIC_NOUNS: &[&str] = &[
    "call",
    "closing",
    "conference",
    "deposition",
    "event",
    "hearing",
    "meeting",
    "retreat",
    "review",
    "seminar",
    "session",
    "summit",
    "sync",
    "training",
    "trial",
    "webinar",
    "workshop",
];

pub fn classify(
    quote: &str,
    message_timestamp: i64,
    now: i64,
    local_offset_seconds: i32,
) -> DeadlineView {
    let context = ParseContext {
        message_timestamp: UnixSeconds(message_timestamp),
        timezone: TimezoneContext {
            base_offset_seconds: local_offset_seconds,
            transition: None,
        },
        eod_seconds_since_midnight: DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT,
        week_start: Weekday::Monday,
    };
    let normalized = normalize_quote(quote);
    // SoftWindow is redundant: see asap_is_soft_in_the_relative_grammar.
    let parsed = [
        TemporalKind::LocalDatetime,
        TemporalKind::Date,
        TemporalKind::Relative,
    ]
    .into_iter()
    .find_map(|kind| reparse(kind, &normalized, &context).ok());
    match parsed {
        Some(ParsedValue::Instant(resolution)) => {
            instant_view(resolution, now, local_offset_seconds)
        }
        Some(value @ (ParsedValue::Date(day) | ParsedValue::BusinessDay(day))) => {
            let Some(boundary) = policy_boundary(&value, &context).and_then(boundary_seconds)
            else {
                return DeadlineView::Unknown;
            };
            let past = now > boundary;
            match value {
                ParsedValue::Date(_) => DeadlineView::DueDate {
                    day: day.0,
                    boundary,
                    past,
                },
                _ => DeadlineView::DueBusinessDay {
                    day: day.0,
                    boundary,
                    past,
                },
            }
        }
        Some(value @ ParsedValue::Week { start, end }) => {
            let Some(boundary) = policy_boundary(&value, &context).and_then(boundary_seconds)
            else {
                return DeadlineView::Unknown;
            };
            DeadlineView::DueRange {
                start_day: start.0,
                end_day: end.0,
                boundary,
                past: now > boundary,
            }
        }
        Some(ParsedValue::Soft) => DeadlineView::Soft,
        // EventRelative is not among the kinds tried above.
        Some(ParsedValue::EventRelative) => DeadlineView::EventTied,
        None => {
            let lower = quote.to_ascii_lowercase();
            if lower.split_ascii_whitespace().any(|token| {
                EVENT_GENERIC_NOUNS
                    .contains(&token.trim_matches(|c: char| c.is_ascii_punctuation()))
            }) {
                DeadlineView::EventTied
            } else {
                DeadlineView::Unknown
            }
        }
    }
}

fn normalize_quote(quote: &str) -> String {
    let trim = |c: char| c.is_ascii_punctuation() || c.is_whitespace();
    let text = quote.trim_matches(trim);
    for prefix in ["no later than ", "due by ", "due ", "by ", "on "] {
        if text
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        {
            return text[prefix.len()..].trim_matches(trim).to_string();
        }
    }
    text.to_string()
}

fn boundary_seconds(resolution: LocalResolution) -> Option<i64> {
    match resolution {
        LocalResolution::Unambiguous(time) => Some(time.0),
        // Defensive: a fixed offset with transition: None cannot produce ambiguity.
        // If transitions are supported later, aging must never choose a candidate.
        LocalResolution::Ambiguous { .. } => None,
    }
}

fn instant_view(resolution: LocalResolution, now: i64, offset_seconds: i32) -> DeadlineView {
    let Some(boundary) = boundary_seconds(resolution) else {
        return DeadlineView::Unknown;
    };
    if now > boundary {
        DeadlineView::PastDue {
            boundary,
            offset_seconds,
        }
    } else {
        DeadlineView::Due {
            boundary,
            offset_seconds,
        }
    }
}

pub fn label(view: &DeadlineView) -> String {
    match view {
        DeadlineView::PastDue {
            boundary,
            offset_seconds,
        } => {
            format!(
                "Past due since {}",
                format_instant(*boundary, *offset_seconds)
            )
        }
        DeadlineView::Due {
            boundary,
            offset_seconds,
        } => format!("Due {}", format_instant(*boundary, *offset_seconds)),
        DeadlineView::DueDate {
            day, past: false, ..
        } => format!("Due by end of {}", format_day(*day)),
        DeadlineView::DueDate {
            day, past: true, ..
        } => {
            format!("Past due: {} has ended", format_day(*day))
        }
        DeadlineView::DueBusinessDay {
            day, past: false, ..
        } => {
            format!("Due by end of business {}", format_day(*day))
        }
        DeadlineView::DueBusinessDay {
            day, past: true, ..
        } => {
            format!("Past due: business day {} has ended", format_day(*day))
        }
        DeadlineView::DueRange {
            start_day,
            end_day,
            past,
            ..
        } => {
            if *past {
                format!(
                    "Past due: range {} to {} has ended",
                    format_day(*start_day),
                    format_day(*end_day)
                )
            } else {
                format!(
                    "Due by end of {} (range started {})",
                    format_day(*end_day),
                    format_day(*start_day)
                )
            }
        }
        DeadlineView::EventTied => "Tied to an event; time not stated".to_string(),
        DeadlineView::Soft => "Soft urgency; no fixed deadline".to_string(),
        DeadlineView::Unknown => "Deadline phrase not understood".to_string(),
    }
}

fn format_instant(boundary: i64, offset_seconds: i32) -> String {
    let offset = chrono::FixedOffset::east_opt(offset_seconds);
    match (chrono::DateTime::from_timestamp(boundary, 0), offset) {
        (Some(time), Some(offset)) => time
            .with_timezone(&offset)
            .format("%a %b %d, %Y %H:%M %:z")
            .to_string(),
        _ => boundary.to_string(),
    }
}

fn format_day(day: i64) -> String {
    day.checked_mul(86_400)
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map_or_else(
            || day.to_string(),
            |time| time.format("%a %b %d, %Y").to_string(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Wednesday 2026-09-02 12:00 UTC; Friday and Sunday at 17:00 UTC.
    const MESSAGE: i64 = 1_788_350_400;
    const FRIDAY_EOD: i64 = 1_788_541_200;
    const SUNDAY_EOD: i64 = 1_788_714_000;

    #[test]
    fn date_only_labels_never_invent_clock_times() {
        let view = classify("Friday", MESSAGE, MESSAGE, 0);
        assert!(!label(&view).contains(':'));
        assert!(matches!(view, DeadlineView::DueDate { .. }));
        let business = classify("in 2 business days", MESSAGE, MESSAGE, 0);
        assert!(matches!(business, DeadlineView::DueBusinessDay { .. }));
        assert!(!label(&business).contains(':'));
    }

    #[test]
    fn instant_labels_use_the_resolved_offset() {
        let view = classify("2026-09-01 15:00", MESSAGE, MESSAGE, -5 * 3600);
        assert!(label(&view).contains("15:00 -05:00"));
    }

    #[test]
    fn event_fallback_requires_whole_tokens() {
        for quote in [
            "practically",
            "recall",
            "prevent",
            "possession",
            "disclosing",
        ] {
            assert_eq!(
                classify(quote, MESSAGE, MESSAGE, 0),
                DeadlineView::Unknown,
                "{quote}"
            );
        }
        for quote in ["before the meeting", "before the (meeting)."] {
            assert_eq!(
                classify(quote, MESSAGE, MESSAGE, 0),
                DeadlineView::EventTied
            );
        }
    }

    #[test]
    fn normalized_quotes_age_like_their_baseline() {
        for input in [
            "by Friday",
            "Friday.",
            "due by Friday",
            "no later than Friday",
            "due Friday",
            "on Friday",
        ] {
            for now in [MESSAGE, FRIDAY_EOD + 1] {
                assert_eq!(
                    classify(input, MESSAGE, now, 0),
                    classify("Friday", MESSAGE, now, 0),
                    "{input}"
                );
            }
        }
        assert_eq!(
            classify("before Friday", MESSAGE, MESSAGE, 0),
            DeadlineView::Unknown
        );
    }

    #[test]
    fn friday_ages_at_eod_and_is_due_at_the_boundary() {
        for now in [MESSAGE, FRIDAY_EOD] {
            assert_eq!(
                classify("Friday", MESSAGE, now, 0),
                DeadlineView::DueDate {
                    day: FRIDAY_EOD / 86400,
                    boundary: FRIDAY_EOD,
                    past: false
                }
            );
        }
        assert_eq!(
            classify("Friday", MESSAGE, FRIDAY_EOD + 1, 0),
            DeadlineView::DueDate {
                day: FRIDAY_EOD / 86400,
                boundary: FRIDAY_EOD,
                past: true
            }
        );
    }

    #[test]
    fn explicit_local_datetime_accepts_space_and_t_and_respects_offset() {
        // 2026-09-01 15:00 UTC.
        let boundary = 1_788_274_800;
        for quote in ["2026-09-01 15:00", "2026-09-01T15:00"] {
            assert_eq!(
                classify(quote, MESSAGE, MESSAGE, 0),
                DeadlineView::PastDue {
                    boundary,
                    offset_seconds: 0
                }
            );
            assert_eq!(
                classify(quote, MESSAGE, boundary + 25_200, -25_200),
                DeadlineView::Due {
                    boundary: boundary + 25_200,
                    offset_seconds: -25_200
                }
            );
        }
    }

    #[test]
    fn parenthesized_datetime_still_parses_as_an_instant() {
        let boundary = 1_788_274_800; // 2026-09-01T15:00Z
        for quote in ["2026-09-01T15:00", "(2026-09-01T15:00)"] {
            assert_eq!(
                classify(quote, MESSAGE, MESSAGE, 0),
                DeadlineView::PastDue {
                    boundary,
                    offset_seconds: 0
                }
            );
        }
    }

    #[test]
    fn this_week_ages_after_sunday_eod() {
        for (now, past) in [
            (MESSAGE, false),
            (SUNDAY_EOD, false),
            (SUNDAY_EOD + 1, true),
        ] {
            assert_eq!(
                classify("this week", MESSAGE, now, 0),
                DeadlineView::DueRange {
                    start_day: 20_696,
                    end_day: 20_702,
                    boundary: SUNDAY_EOD,
                    past
                }
            );
        }
    }

    #[test]
    fn asap_is_soft_in_the_relative_grammar() {
        let context = ParseContext {
            message_timestamp: UnixSeconds(MESSAGE),
            timezone: TimezoneContext {
                base_offset_seconds: 0,
                transition: None,
            },
            eod_seconds_since_midnight: DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT,
            week_start: Weekday::Monday,
        };
        assert!(matches!(
            reparse(TemporalKind::Relative, "ASAP", &context),
            Ok(ParsedValue::Soft)
        ));
        for quote in ["ASAP", "when you can", "when you get a chance"] {
            assert_eq!(classify(quote, MESSAGE, MESSAGE, 0), DeadlineView::Soft);
        }
    }

    #[test]
    fn meeting_falls_back_to_event_tied() {
        assert_eq!(
            classify("before the meeting", MESSAGE, MESSAGE, 0),
            DeadlineView::EventTied
        );
    }

    #[test]
    fn whenever_is_unknown() {
        assert_eq!(
            classify("whenever", MESSAGE, MESSAGE, 0),
            DeadlineView::Unknown
        );
    }

    #[test]
    fn labels_preserve_civil_dates_and_handle_out_of_range_values() {
        assert_eq!(
            label(&DeadlineView::DueRange {
                start_day: 20_696,
                end_day: 20_702,
                boundary: SUNDAY_EOD,
                past: false
            }),
            "Due by end of Sun Sep 06, 2026 (range started Mon Aug 31, 2026)"
        );
        assert_eq!(
            label(&DeadlineView::DueRange {
                start_day: 20_696,
                end_day: 20_702,
                boundary: SUNDAY_EOD,
                past: true
            }),
            "Past due: range Mon Aug 31, 2026 to Sun Sep 06, 2026 has ended"
        );
        assert_eq!(
            label(&DeadlineView::Due {
                boundary: i64::MAX,
                offset_seconds: 0
            }),
            format!("Due {}", i64::MAX)
        );
        assert_eq!(format_day(i64::MAX), i64::MAX.to_string());
        assert_eq!(
            label(&DeadlineView::EventTied),
            "Tied to an event; time not stated"
        );
        assert_eq!(
            label(&DeadlineView::Soft),
            "Soft urgency; no fixed deadline"
        );
        assert_eq!(
            label(&DeadlineView::Unknown),
            "Deadline phrase not understood"
        );
    }
}
