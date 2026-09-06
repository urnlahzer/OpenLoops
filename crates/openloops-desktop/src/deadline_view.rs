//! Pure deadline aging relative to the source message and a fixed local offset.
use openloops_domain::deadline::UnixSeconds;
use openloops_domain::deadline_parse::{
    DEFAULT_EOD_SECONDS_SINCE_MIDNIGHT, LocalResolution, ParseContext, ParsedValue, TemporalKind,
    TimezoneContext, Weekday, policy_boundary, reparse,
};

#[derive(Debug, PartialEq, Eq)]
pub enum DeadlineView {
    PastDue {
        boundary: i64,
    },
    Due {
        boundary: i64,
    },
    DueRange {
        start_day: i64,
        end_day: i64,
        past: bool,
    },
    EventTied,
    Soft,
    Unknown,
}

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
    let parsed = [
        TemporalKind::LocalDatetime,
        TemporalKind::Date,
        TemporalKind::Relative,
        TemporalKind::SoftWindow,
    ]
    .into_iter()
    .find_map(|kind| reparse(kind, quote, &context).ok());
    match parsed {
        Some(ParsedValue::Instant(resolution)) => instant_view(resolution, now),
        Some(value @ (ParsedValue::Date(_) | ParsedValue::BusinessDay(_))) => instant_view(
            policy_boundary(&value, &context).expect("calendar date has an EOD boundary"),
            now,
        ),
        Some(value @ ParsedValue::Week { start, end }) => {
            let boundary = boundary_seconds(
                policy_boundary(&value, &context).expect("week has an EOD boundary"),
            );
            DeadlineView::DueRange {
                start_day: start.0,
                end_day: end.0,
                past: now > boundary,
            }
        }
        Some(ParsedValue::Soft) => DeadlineView::Soft,
        // EventRelative is not among the kinds tried above.
        Some(ParsedValue::EventRelative) => DeadlineView::EventTied,
        None => {
            let lower = quote.to_ascii_lowercase();
            if [
                "meeting",
                "call",
                "event",
                "session",
                "hearing",
                "closing",
                "deposition",
            ]
            .iter()
            .any(|word| lower.contains(word))
            {
                DeadlineView::EventTied
            } else {
                DeadlineView::Unknown
            }
        }
    }
}

fn boundary_seconds(resolution: LocalResolution) -> i64 {
    match resolution {
        LocalResolution::Unambiguous(time) => time.0,
        // A fixed offset with no transition cannot produce this arm. Choose the
        // earlier reading for exhaustiveness and future transition support.
        LocalResolution::Ambiguous { candidates, .. } => {
            candidates.iter().map(|time| time.0).min().unwrap()
        }
    }
}

fn instant_view(resolution: LocalResolution, now: i64) -> DeadlineView {
    let boundary = boundary_seconds(resolution);
    if now > boundary {
        DeadlineView::PastDue { boundary }
    } else {
        DeadlineView::Due { boundary }
    }
}

pub fn label(view: &DeadlineView) -> String {
    match view {
        DeadlineView::PastDue { boundary } => {
            format!("Past due since {}", format_instant(*boundary))
        }
        DeadlineView::Due { boundary } => format!("Due {}", format_instant(*boundary)),
        DeadlineView::DueRange { end_day, past, .. } => {
            if *past {
                format!("Past due: range ended {}", format_day(*end_day))
            } else {
                format!("Due by end of {}", format_day(*end_day))
            }
        }
        DeadlineView::EventTied => "Tied to an event; time not stated".to_string(),
        DeadlineView::Soft => "Soft urgency; no fixed deadline".to_string(),
        DeadlineView::Unknown => "Deadline phrase not understood".to_string(),
    }
}

fn format_instant(boundary: i64) -> String {
    chrono::DateTime::from_timestamp(boundary, 0).map_or_else(
        || boundary.to_string(),
        |time| {
            time.with_timezone(&chrono::Local)
                .format("%a %b %d, %Y %H:%M")
                .to_string()
        },
    )
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
    fn friday_ages_at_eod_and_is_due_at_the_boundary() {
        for now in [MESSAGE, FRIDAY_EOD] {
            assert_eq!(
                classify("Friday", MESSAGE, now, 0),
                DeadlineView::Due {
                    boundary: FRIDAY_EOD
                }
            );
        }
        assert_eq!(
            classify("Friday", MESSAGE, FRIDAY_EOD + 1, 0),
            DeadlineView::PastDue {
                boundary: FRIDAY_EOD
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
                DeadlineView::PastDue { boundary }
            );
            assert_eq!(
                classify(quote, MESSAGE, boundary + 25_200, -25_200),
                DeadlineView::Due {
                    boundary: boundary + 25_200
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
        assert_eq!(classify("ASAP", MESSAGE, MESSAGE, 0), DeadlineView::Soft);
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
                past: false
            }),
            "Due by end of Sun Sep 06, 2026"
        );
        assert_eq!(
            label(&DeadlineView::DueRange {
                start_day: 20_696,
                end_day: 20_702,
                past: true
            }),
            "Past due: range ended Sun Sep 06, 2026"
        );
        assert_eq!(
            label(&DeadlineView::Due { boundary: i64::MAX }),
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
