use serde_json::Value;

use crate::live::ConnectionError;

const BODY_CHARS: usize = 131_072;

pub(super) fn header<'a>(payload: &'a Value, name: &str) -> Option<&'a str> {
    payload
        .get("headers")?
        .as_array()?
        .iter()
        .find(|entry| {
            entry
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case(name))
        })?
        .get("value")?
        .as_str()
}

fn attached(payload: &Value) -> bool {
    payload
        .get("filename")
        .and_then(Value::as_str)
        .is_some_and(|name| !name.is_empty())
        || header(payload, "Content-Disposition").is_some_and(|value| {
            value
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("attachment")
        })
}

fn collect_bodies(
    payload: &Value,
    html: &mut Option<String>,
    plain: &mut Option<String>,
) -> Result<(), ConnectionError> {
    if attached(payload) {
        return Ok(());
    }
    let mime = payload
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let target = if mime.eq_ignore_ascii_case("text/html") {
        Some(&mut *html)
    } else if mime.eq_ignore_ascii_case("text/plain") {
        Some(&mut *plain)
    } else {
        None
    };
    if let Some(target) = target
        && target.is_none()
        && let Some(data) = payload.pointer("/body/data").and_then(Value::as_str)
    {
        *target = decode_part(data, header(payload, "Content-Type"))?;
    }
    if let Some(parts) = payload.get("parts").and_then(Value::as_array) {
        for part in parts {
            collect_bodies(part, html, plain)?;
        }
    }
    Ok(())
}

pub(super) fn select_body(payload: &Value) -> Result<Option<(String, bool)>, ConnectionError> {
    let mut html = None;
    let mut plain = None;
    collect_bodies(payload, &mut html, &mut plain)?;
    Ok(html
        .map(|body| (body, true))
        .or_else(|| plain.map(|body| (body, false))))
}

fn charset(content_type: Option<&str>) -> &str {
    content_type
        .and_then(|value| {
            value.split(';').skip(1).find_map(|parameter| {
                let (name, value) = parameter.trim().split_once('=')?;
                name.eq_ignore_ascii_case("charset")
                    .then(|| value.trim().trim_matches(['\'', '"']))
            })
        })
        .unwrap_or("utf-8")
}

fn decode_bytes(bytes: &[u8], charset: &str) -> String {
    if charset.eq_ignore_ascii_case("iso-8859-1") || charset.eq_ignore_ascii_case("latin1") {
        bytes.iter().map(|&byte| char::from(byte)).collect()
    } else if charset.eq_ignore_ascii_case("windows-1252") || charset.eq_ignore_ascii_case("cp1252")
    {
        bytes
            .iter()
            .map(|&byte| match byte {
                0x80 => '\u{20ac}',
                0x82 => '\u{201a}',
                0x83 => '\u{0192}',
                0x84 => '\u{201e}',
                0x85 => '\u{2026}',
                0x86 => '\u{2020}',
                0x87 => '\u{2021}',
                0x88 => '\u{02c6}',
                0x89 => '\u{2030}',
                0x8a => '\u{0160}',
                0x8b => '\u{2039}',
                0x8c => '\u{0152}',
                0x8e => '\u{017d}',
                0x91 => '\u{2018}',
                0x92 => '\u{2019}',
                0x93 => '\u{201c}',
                0x94 => '\u{201d}',
                0x95 => '\u{2022}',
                0x96 => '\u{2013}',
                0x97 => '\u{2014}',
                0x98 => '\u{02dc}',
                0x99 => '\u{2122}',
                0x9a => '\u{0161}',
                0x9b => '\u{203a}',
                0x9c => '\u{0153}',
                0x9e => '\u{017e}',
                0x9f => '\u{0178}',
                other => char::from(other),
            })
            .collect()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

pub(super) fn decode_part(
    data: &str,
    content_type: Option<&str>,
) -> Result<Option<String>, ConnectionError> {
    let Some(bytes) = crate::encoding::base64url_decode(data.trim_end_matches('=')) else {
        return Ok(None);
    };
    let decoded = decode_bytes(&bytes, charset(content_type));
    if decoded.chars().count() > BODY_CHARS {
        Err(ConnectionError::MessageTooLarge)
    } else {
        Ok(Some(decoded))
    }
}

fn decode_word_bytes(word: &str) -> Option<(&str, Vec<u8>)> {
    let inner = word.strip_prefix("=?")?.strip_suffix("?=")?;
    let mut pieces = inner.splitn(3, '?');
    let charset = pieces.next()?;
    let encoding = pieces.next()?;
    let value = pieces.next()?;
    if !charset.eq_ignore_ascii_case("utf-8")
        && !charset.eq_ignore_ascii_case("iso-8859-1")
        && !charset.eq_ignore_ascii_case("latin1")
    {
        return None;
    }
    let bytes = if encoding.eq_ignore_ascii_case("b") {
        let normalized = value
            .trim_end_matches('=')
            .replace('+', "-")
            .replace('/', "_");
        crate::encoding::base64url_decode(&normalized)?
    } else if encoding.eq_ignore_ascii_case("q") {
        let input = value.as_bytes();
        let mut output = Vec::with_capacity(input.len());
        let mut index = 0;
        while index < input.len() {
            match input[index] {
                b'_' => output.push(b' '),
                b'=' => {
                    let hex = std::str::from_utf8(input.get(index + 1..index + 3)?).ok()?;
                    output.push(u8::from_str_radix(hex, 16).ok()?);
                    index += 2;
                }
                byte => output.push(byte),
            }
            index += 1;
        }
        output
    } else {
        return None;
    };
    Some((charset, bytes))
}

pub(super) fn decode_rfc2047(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative_start) = value[cursor..].find("=?") {
        let start = cursor + relative_start;
        output.push_str(&value[cursor..start]);
        let Some(relative_end) = value[start + 2..].find("?=") else {
            output.push_str(&value[start..]);
            return output;
        };
        let end = start + 2 + relative_end + 2;
        let Some((first_charset, first_bytes)) = decode_word_bytes(&value[start..end]) else {
            output.push_str(&value[start..end]);
            cursor = end;
            continue;
        };

        let mut groups = vec![(first_charset, first_bytes)];
        let mut run_end = end;
        loop {
            let whitespace_end = value[run_end..]
                .char_indices()
                .take_while(|(_, character)| character.is_whitespace())
                .last()
                .map_or(run_end, |(index, character)| {
                    run_end + index + character.len_utf8()
                });
            if whitespace_end == run_end || !value[whitespace_end..].starts_with("=?") {
                break;
            }
            let Some(relative_end) = value[whitespace_end + 2..].find("?=") else {
                break;
            };
            let next_end = whitespace_end + 2 + relative_end + 2;
            let Some((next_charset, next_bytes)) =
                decode_word_bytes(&value[whitespace_end..next_end])
            else {
                break;
            };
            if let Some((charset, bytes)) = groups.last_mut()
                && charset.eq_ignore_ascii_case(next_charset)
            {
                bytes.extend(next_bytes);
            } else {
                groups.push((next_charset, next_bytes));
            }
            run_end = next_end;
        }
        for (charset, bytes) in groups {
            output.push_str(&decode_bytes(&bytes, charset));
        }
        cursor = run_end;
    }
    output.push_str(&value[cursor..]);
    output
}

pub(super) fn parse_address_list(value: &str) -> Vec<(Option<String>, String)> {
    let mut entries = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut angle_depth = 0u8;
    for (index, character) in value.char_indices() {
        match character {
            '"' => quoted = !quoted,
            '<' if !quoted => angle_depth = angle_depth.saturating_add(1),
            '>' if !quoted => angle_depth = angle_depth.saturating_sub(1),
            ',' if !quoted && angle_depth == 0 => {
                parse_address(&value[start..index], &mut entries);
                start = index + 1;
            }
            _ => {}
        }
    }
    parse_address(&value[start..], &mut entries);
    entries
}

fn parse_address(value: &str, entries: &mut Vec<(Option<String>, String)>) {
    let value = value.trim();
    let (name, address) = if let Some((name, rest)) = value.split_once('<') {
        (
            Some(name.trim().trim_matches('"')),
            rest.trim_end_matches('>').trim(),
        )
    } else {
        (None, value)
    };
    if address.is_empty() || address.chars().any(char::is_control) {
        return;
    }
    entries.push((
        name.filter(|name| !name.is_empty()).map(decode_rfc2047),
        address.to_ascii_lowercase(),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_html_and_skips_attachments() {
        let payload = serde_json::json!({"parts": [
            {"mimeType":"text/html","filename":"synthetic.html","body":{"data":"YmFk"}},
            {"mimeType":"multipart/alternative","parts":[
                {"mimeType":"text/plain","body":{"data":"cGxhaW4"}},
                {"mimeType":"text/html","headers":[{"name":"Content-Type","value":"text/html; charset=utf-8"}],"body":{"data":"PGI-aHRtbDwvYj4"}}
            ]}
        ]});
        assert_eq!(
            select_body(&payload),
            Ok(Some(("<b>html</b>".into(), true)))
        );

        let disposition = serde_json::json!({"parts": [
            {"mimeType":"text/html","headers":[{"name":"Content-Disposition","value":"attachment"}],"body":{"data":"YmFk"}},
            {"mimeType":"text/plain","body":{"data":"Z29vZA"}}
        ]});
        assert_eq!(select_body(&disposition), Ok(Some(("good".into(), false))));
    }

    #[test]
    fn decodes_charsets_headers_and_addresses() {
        assert_eq!(
            decode_part("Y2Fm6Q", Some("text/plain; charset=iso-8859-1"))
                .unwrap()
                .as_deref(),
            Some("caf\u{e9}")
        );
        assert_eq!(
            decode_part("cHJpY2UgOJk", Some("text/plain; charset=windows-1252"))
                .unwrap()
                .as_deref(),
            Some("price 8\u{2122}")
        );
        assert_eq!(
            decode_rfc2047("=?UTF-8?B?U3ludGhldGljIOKckw==?="),
            "Synthetic \u{2713}"
        );
        assert_eq!(
            parse_address_list(
                "\"Example, Sender\" <SENDER@example.invalid>, other@example.invalid"
            ),
            vec![
                (
                    Some("Example, Sender".into()),
                    "sender@example.invalid".into()
                ),
                (None, "other@example.invalid".into())
            ]
        );
    }

    #[test]
    fn accepts_padded_and_unpadded_base64url_parts() {
        assert_eq!(decode_part("Zm8=", None), Ok(Some("fo".into())));
        assert_eq!(decode_part("Zm9v", None), Ok(Some("foo".into())));
    }

    #[test]
    fn oversized_and_malformed_bodies_are_distinct() {
        let oversized = crate::encoding::base64url_encode("x".repeat(BODY_CHARS + 1).as_bytes());
        assert_eq!(
            decode_part(&oversized, None),
            Err(ConnectionError::MessageTooLarge)
        );
        let oversized_payload =
            serde_json::json!({"mimeType":"text/html","body":{"data":oversized}});
        assert_eq!(
            select_body(&oversized_payload),
            Err(ConnectionError::MessageTooLarge)
        );
        assert_eq!(decode_part("!", None), Ok(None));
        let malformed = serde_json::json!({"mimeType":"text/plain","body":{"data":"!"}});
        assert_eq!(select_body(&malformed), Ok(None));
    }

    #[test]
    fn rfc2047_joins_encoded_words_and_preserves_plain_whitespace() {
        assert_eq!(
            decode_rfc2047("=?UTF-8?Q?Synthetic_=E2=9C=93?="),
            "Synthetic ✓"
        );
        assert_eq!(decode_rfc2047("=?UTF-8?B?4pw=?= \t=?UTF-8?B?kw==?="), "✓");
        assert_eq!(decode_rfc2047("plain  text"), "plain  text");
    }
}
