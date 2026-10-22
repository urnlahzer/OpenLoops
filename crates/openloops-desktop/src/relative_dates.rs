#[derive(Clone, Debug)]
struct Token {
    word: String,
    start: usize,
    end: usize,
}

#[derive(Clone, Debug)]
struct Pattern {
    words: Vec<String>,
    value: String,
}

#[derive(Clone, Debug)]
struct PhraseMatch {
    start: usize,
    end: usize,
    value: String,
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character.is_alphanumeric() || character == '_' {
            start.get_or_insert(index);
        } else if let Some(token_start) = start.take() {
            tokens.push(Token {
                word: text[token_start..index].to_ascii_lowercase(),
                start: token_start,
                end: index,
            });
        }
    }
    if let Some(token_start) = start {
        tokens.push(Token {
            word: text[token_start..].to_ascii_lowercase(),
            start: token_start,
            end: text.len(),
        });
    }
    tokens
}

fn add_pattern(patterns: &mut Vec<Pattern>, phrase: &str, value: &str) {
    patterns.push(Pattern {
        words: phrase
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect(),
        value: value.to_string(),
    });
}

fn phrase_patterns() -> Vec<Pattern> {
    let mut patterns = Vec::new();
    for phrase in [
        "today",
        "tonight",
        "this morning",
        "this afternoon",
        "this evening",
        "later today",
        "end of day",
        "end of the day",
        "eod",
        "close of business",
        "cob",
    ] {
        add_pattern(&mut patterns, phrase, "today");
    }
    for phrase in [
        "tomorrow",
        "tomorrow morning",
        "tomorrow afternoon",
        "tomorrow evening",
        "first thing tomorrow",
        "end of day tomorrow",
        "end of the day tomorrow",
        "tomorrow eod",
    ] {
        add_pattern(&mut patterns, phrase, "tomorrow");
    }
    for phrase in ["next week", "early next week"] {
        add_pattern(&mut patterns, phrase, "next week");
    }
    for phrase in [
        "this week",
        "end of week",
        "end of the week",
        "later this week",
    ] {
        add_pattern(&mut patterns, phrase, "end of week");
    }
    for phrase in ["next business day", "next working day"] {
        add_pattern(&mut patterns, phrase, "next business day");
    }

    for weekday in [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ] {
        add_pattern(&mut patterns, weekday, weekday);
        for prefix in ["on", "by", "this", "next"] {
            add_pattern(&mut patterns, &format!("{prefix} {weekday}"), weekday);
        }
        for suffix in ["eod", "end of day", "end of the day"] {
            add_pattern(&mut patterns, &format!("{weekday} {suffix}"), weekday);
            add_pattern(&mut patterns, &format!("{suffix} {weekday}"), weekday);
            for prefix in ["on", "by", "this", "next"] {
                add_pattern(
                    &mut patterns,
                    &format!("{prefix} {weekday} {suffix}"),
                    weekday,
                );
                add_pattern(
                    &mut patterns,
                    &format!("{prefix} {suffix} {weekday}"),
                    weekday,
                );
            }
        }
    }
    patterns
}

fn number_value(word: &str) -> Option<u32> {
    let number = match word {
        "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "eleven" => 11,
        "twelve" => 12,
        "thirteen" => 13,
        "fourteen" => 14,
        _ if !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_digit()) => {
            word.parse().ok()?
        }
        _ => return None,
    };
    (number <= 3_650).then_some(number)
}

fn collect_matches(tokens: &[Token], patterns: &[Pattern]) -> Vec<PhraseMatch> {
    let mut matches = Vec::new();
    for start in 0..tokens.len() {
        for pattern in patterns {
            let end = start + pattern.words.len();
            if end <= tokens.len()
                && tokens[start..end]
                    .iter()
                    .zip(&pattern.words)
                    .all(|(token, word)| token.word == *word)
            {
                matches.push(PhraseMatch {
                    start,
                    end,
                    value: pattern.value.clone(),
                });
            }
        }

        let Some(count) = tokens
            .get(start + 1)
            .and_then(|token| number_value(&token.word))
        else {
            continue;
        };
        let Some(unit) = tokens.get(start + 2) else {
            continue;
        };
        let (end, canonical_unit) = if matches!(unit.word.as_str(), "business" | "working")
            && tokens
                .get(start + 3)
                .is_some_and(|token| token.word == "days")
        {
            (start + 4, "business days")
        } else if unit.word == "days" {
            (start + 3, "days")
        } else {
            continue;
        };
        if matches!(tokens[start].word.as_str(), "in" | "within") {
            matches.push(PhraseMatch {
                start,
                end,
                value: format!("in {count} {canonical_unit}"),
            });
        }
    }
    matches
}

/// Finds relative date phrases as `(canonical value, matched text)` pairs.
/// The matched text keeps its case; whitespace runs become single spaces.
pub(super) fn find_relative_date_phrases(text: &str) -> Vec<(String, String)> {
    let tokens = tokenize(text);
    if tokens.is_empty() {
        return Vec::new();
    }
    let patterns = phrase_patterns();
    let mut matches = collect_matches(&tokens, &patterns);
    matches.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| right.end.cmp(&left.end))
    });

    let mut used = vec![false; tokens.len()];
    let mut phrases = Vec::new();
    for candidate in matches {
        if used[candidate.start..candidate.end]
            .iter()
            .any(|word| *word)
        {
            continue;
        }
        used[candidate.start..candidate.end].fill(true);
        let start = tokens[candidate.start].start;
        let end = tokens[candidate.end - 1].end;
        let phrase_text = text[start..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        phrases.push((candidate.value, phrase_text));
    }
    phrases
}

#[cfg(test)]
mod tests {
    use super::find_relative_date_phrases;

    #[test]
    fn finder_covers_the_relative_date_phrase_table() {
        let cases = [
            ("today", "today"),
            ("TONIGHT", "today"),
            ("this morning", "today"),
            ("this afternoon", "today"),
            ("this evening", "today"),
            ("later today", "today"),
            ("end of day", "today"),
            ("end of the day", "today"),
            ("EOD", "today"),
            ("close of business", "today"),
            ("COB", "today"),
            ("tomorrow", "tomorrow"),
            ("tomorrow morning", "tomorrow"),
            ("tomorrow afternoon", "tomorrow"),
            ("tomorrow evening", "tomorrow"),
            ("first thing tomorrow", "tomorrow"),
            ("end of day tomorrow", "tomorrow"),
            ("tomorrow EOD", "tomorrow"),
            ("Monday", "monday"),
            ("on Tuesday", "tuesday"),
            ("by Wednesday", "wednesday"),
            ("this Thursday", "thursday"),
            ("next Friday", "friday"),
            ("Saturday EOD", "saturday"),
            ("EOD Sunday", "sunday"),
            ("end of day Monday", "monday"),
            ("Tuesday end of the day", "tuesday"),
            ("next week", "next week"),
            ("early next week", "next week"),
            ("this week", "end of week"),
            ("end of week", "end of week"),
            ("end of the week", "end of week"),
            ("later this week", "end of week"),
            ("next business day", "next business day"),
            ("next working day", "next business day"),
            ("in 3 days", "in 3 days"),
            ("within 14 days", "in 14 days"),
            ("in four days", "in 4 days"),
            ("within fourteen business days", "in 14 business days"),
            ("in 2 working days", "in 2 business days"),
        ];

        for (text, canonical) in cases {
            assert_eq!(
                find_relative_date_phrases(text),
                vec![(canonical.to_string(), text.to_string())],
                "phrase: {text}"
            );
        }
    }

    #[test]
    fn finder_uses_longest_overlapping_phrase_and_whole_words() {
        assert_eq!(
            find_relative_date_phrases("by end of day tomorrow"),
            vec![("tomorrow".to_string(), "end of day tomorrow".to_string())]
        );
        assert!(find_relative_date_phrases("as soon as I can").is_empty());
        assert!(find_relative_date_phrases("soon").is_empty());
        assert!(find_relative_date_phrases("end of the month").is_empty());
        assert!(find_relative_date_phrases("Mayday").is_empty());
        assert!(find_relative_date_phrases("todayish").is_empty());
    }

    #[test]
    fn matched_text_spanning_a_line_break_uses_single_spaces() {
        assert_eq!(
            find_relative_date_phrases("by the end of\r\n  the day"),
            vec![("today".to_string(), "end of the day".to_string())]
        );
    }

    #[test]
    fn number_words_one_through_fourteen_map_to_digits() {
        let words = [
            "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
            "eleven", "twelve", "thirteen", "fourteen",
        ];
        for (index, word) in words.iter().enumerate() {
            let number = index + 1;
            assert_eq!(
                find_relative_date_phrases(&format!("within {word} working days")),
                vec![(
                    format!("in {number} business days"),
                    format!("within {word} working days")
                )]
            );
        }
    }

    #[test]
    fn finder_returns_matches_in_text_order_without_reusing_words() {
        assert_eq!(
            find_relative_date_phrases("Tomorrow, then Friday, and later today."),
            vec![
                ("tomorrow".to_string(), "Tomorrow".to_string()),
                ("friday".to_string(), "Friday".to_string()),
                ("today".to_string(), "later today".to_string()),
            ]
        );
    }
}
