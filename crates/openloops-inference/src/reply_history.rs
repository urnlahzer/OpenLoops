//! Outlook/Exchange-style reply-history detection.
//!
//! This is a **semantic heuristic** layered above [`crate::walker`]'s
//! byte-deterministic HTML canonicalizer (and reused, at line granularity,
//! by the desktop crate's plain-text path). Unlike the walker, it makes
//! English-specific assumptions about reply-history markers (`From:`,
//! `Sent:`, `Date:`, `Subject:`, `-----Original Message-----`, and
//! Outlook's underscore-run separator). It is **English-only by design**:
//! localized headers such as German `Von:`/`Gesendet:`/`Betreff:` are
//! deliberately out of scope, not silently mishandled -- a message using
//! them simply is not detected as reply history and stays in the body.
//!
//! Detected history is bounded to at most [`REPLY_HISTORY_MAX_CHUNKS`]
//! quote-block chunks of at most [`REPLY_HISTORY_CHUNK_MAX_CHARS`]
//! characters each (see [`chunk_reply_history`]): a deep thread can
//! otherwise contribute one quote block per quoted paragraph, which is
//! unbounded and can alone exceed `message::MAX_BLOCKS_PER_MESSAGE`. Any
//! history paragraphs beyond that cap are dropped rather than emitted as
//! additional chunks. Dropped text is never emitted anywhere in this
//! module's output, so it never reaches the model prompt or the
//! evidence-anchor set downstream -- only the retained chunks do (both
//! remain fully available there: `validation.rs` can resolve anchors into
//! them, and `ollama.rs` sends them to the model like any other quote
//! block).

/// Whether `s`, compared ASCII case-insensitively, starts with `prefix`.
/// Never panics on a non-ASCII-boundary mismatch: `str::get` returns `None`
/// (so this returns `false`) instead of slicing at an invalid boundary.
#[must_use]
pub fn starts_with_ascii_ci(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// Outlook's separator rule: a line consisting solely of 10 or more `_`
/// characters, after trimming.
#[must_use]
pub fn is_underscore_separator(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.chars().count() >= 10 && trimmed.chars().all(|c| c == '_')
}

/// Whether `lines` contains both a line starting with `Subject:` and a
/// line starting with `Sent:` or `Date:` (all ASCII case-insensitive,
/// after trimming leading whitespace).
fn has_subject_and_sent_lines(lines: &[&str]) -> bool {
    let has_subject = lines
        .iter()
        .any(|l| starts_with_ascii_ci(l.trim_start(), "Subject:"));
    let has_sent_or_date = lines.iter().any(|l| {
        let t = l.trim_start();
        starts_with_ascii_ci(t, "Sent:") || starts_with_ascii_ci(t, "Date:")
    });
    has_subject && has_sent_or_date
}

/// Whether the paragraph at `paragraphs[index]` begins Outlook-style
/// reply-history header text. Matches either:
///
/// (b) the paragraph's first significant line, trimmed, is exactly
///     `-----Original Message-----` -- regardless of what follows in the
///     same paragraph;
/// (c) the paragraph's first significant line starts with `From:` (ASCII
///     case-insensitive) and, across that paragraph plus up to the next
///     three, some line starts with `Subject:` and some line starts with
///     `Sent:` or `Date:` (the header may land in one block, e.g. one
///     `<br>`-joined `<div>`, or be split across several, e.g. separate
///     `<p>`s).
///
/// "First significant line" skips a leading `-----Original Message-----`
/// line and/or a leading underscore-rule line (see
/// [`is_underscore_separator`]) within the *same* paragraph before
/// applying the `From:` test in (c): Outlook sometimes emits one or both
/// of those immediately before the real `From:` line inside a single
/// block.
///
/// There used to be a rule "(a): header entirely within this paragraph's
/// first 6 lines". It has been deleted as dead code: it was strictly
/// subsumed by (c), whose window always includes this paragraph's own
/// (uncapped) lines as its first element, so anything (a) could match,
/// (c) already matches too.
///
/// Never panics: an out-of-range `index` returns `false` rather than
/// indexing `paragraphs` directly.
#[must_use]
pub fn is_reply_history_start(paragraphs: &[&str], index: usize) -> bool {
    let Some(paragraph) = paragraphs.get(index).copied() else {
        return false;
    };
    let mut lines = paragraph.lines().filter(|l| !l.trim().is_empty());
    let Some(first_line) = lines.next() else {
        return false;
    };
    if first_line.trim() == "-----Original Message-----" {
        return true;
    }

    // Find the "first significant line" for the From: test: skip a
    // leading separator line and/or a leading underscore-rule line.
    let mut candidate = Some(first_line);
    while let Some(line) = candidate {
        let trimmed = line.trim();
        if trimmed == "-----Original Message-----" || is_underscore_separator(trimmed) {
            candidate = lines.next();
        } else {
            break;
        }
    }
    let Some(first_significant) = candidate else {
        return false;
    };
    if !starts_with_ascii_ci(first_significant.trim_start(), "From:") {
        return false;
    }

    let window_end = (index + 4).min(paragraphs.len());
    let window_lines: Vec<&str> = paragraphs[index..window_end]
        .iter()
        .flat_map(|block| block.lines())
        .collect();
    has_subject_and_sent_lines(&window_lines)
}

/// Post-pass implementing Outlook reply-history detection, called from
/// [`crate::walker::canonicalize_html`] (via `Walker::finish`) once per
/// walk. Outlook HTML replies place new text first, then the quoted
/// original, with no `<blockquote>` marking the boundary (unlike
/// top-posting clients, which the walker already handles via its own
/// `Category::Quote`).
///
/// Each `body_blocks` entry is split into blank-line-separated paragraphs
/// (the walker only starts a new `body_blocks` entry around a real
/// `<blockquote>`; sibling top-level blocks within one flush are joined by
/// the blank-line separator, so splitting on it recovers per-block
/// granularity). The first paragraph, anywhere across all entries, that
/// starts reply history per [`is_reply_history_start`] is found; if the
/// paragraph immediately before it in the *same* entry is an Outlook
/// underscore separator (see [`is_underscore_separator`]), that is
/// included too.
///
/// If no match is found, or the match (after the underscore look-back)
/// lands on the very first paragraph of the very first entry, both inputs
/// are returned unchanged -- a message that looks like reply history from
/// its first paragraph, with nothing else preceding it anywhere, is left
/// alone rather than emptied into an all-quote message.
///
/// Otherwise, the move is confined to the single `body_blocks` entry
/// containing the match: paragraphs from the match to the end of *that*
/// entry are removed and rejoined into a bounded number of chunks (see
/// [`chunk_reply_history`]), appended to the end of `quote_blocks`. Any
/// entries before or after the matching one are left exactly as they are
/// (an entry after the matching one can only exist following a real
/// `<blockquote>`).
///
/// `quote_blocks` therefore ends up grouped by *source*, not strictly by
/// document position: every blockquote-derived block is already present
/// in `quote_blocks` before this function ever runs (the walker pushes
/// them during the walk itself, well before this post-pass), and this
/// function only ever *appends* the reply-history chunk(s) to the end of
/// that vector. So blockquote-derived blocks always come first and
/// reply-history chunks always come last, even in a document where a
/// `<blockquote>` comes AFTER the `body_blocks` entry that contains the
/// reply-history match.
pub(crate) fn split_reply_history(
    body_blocks: Vec<String>,
    mut quote_blocks: Vec<String>,
) -> (Vec<String>, Vec<String>) {
    let groups: Vec<Vec<String>> = body_blocks
        .iter()
        .map(|block| block.split("\n\n").map(str::to_string).collect())
        .collect();

    let mut flat: Vec<&str> = Vec::new();
    let mut positions: Vec<(usize, usize)> = Vec::new();
    for (group_index, group) in groups.iter().enumerate() {
        for (within_index, paragraph) in group.iter().enumerate() {
            flat.push(paragraph.as_str());
            positions.push((group_index, within_index));
        }
    }

    let Some(mut flat_k) = (0..flat.len()).find(|&i| is_reply_history_start(&flat, i)) else {
        return (body_blocks, quote_blocks);
    };

    // Outlook's underscore-separator rule: if the immediately preceding
    // paragraph, still within the same body_blocks entry, is a lone
    // underscore run, move it too.
    if flat_k > 0
        && positions[flat_k - 1].0 == positions[flat_k].0
        && is_underscore_separator(flat[flat_k - 1])
    {
        flat_k -= 1;
    }

    if flat_k == 0 {
        return (body_blocks, quote_blocks);
    }

    let (match_group, match_within) = positions[flat_k];

    let mut new_body_blocks: Vec<String> = Vec::with_capacity(groups.len());
    let mut moved: Vec<String> = Vec::new();
    for (group_index, mut group) in groups.into_iter().enumerate() {
        if group_index == match_group {
            moved = group.split_off(match_within);
            if !group.is_empty() {
                new_body_blocks.push(group.join("\n\n"));
            }
        } else {
            new_body_blocks.push(group.join("\n\n"));
        }
    }

    quote_blocks.extend(chunk_reply_history(moved));
    (new_body_blocks, quote_blocks)
}

/// Maximum characters (`char` count, matching this codebase's other block
/// size bounds) in one reply-history quote chunk.
pub const REPLY_HISTORY_CHUNK_MAX_CHARS: usize = 4096;
/// Maximum number of reply-history quote chunks a single message may add.
pub const REPLY_HISTORY_MAX_CHUNKS: usize = 8;

/// Rejoins `paragraphs` (already in document order, nearest-to-the-reply
/// first) into at most [`REPLY_HISTORY_MAX_CHUNKS`] `"\n\n"`-joined chunks,
/// each at most [`REPLY_HISTORY_CHUNK_MAX_CHARS`] characters, splitting
/// only on paragraph boundaries. A single paragraph longer than the limit
/// still becomes its own, oversized, chunk rather than being cut
/// mid-text; if that single paragraph exceeds `message::
/// MAX_SCALARS_PER_BLOCK` (8192 scalars), `message.rs`'s `bounded_block`
/// then fails the whole message with `MessageError::TooManyScalarsInBlock`
/// when it builds the final `CanonicalMessage` -- this function itself
/// never truncates a paragraph to force it under either limit.
///
/// Once the chunk cap is reached, any remaining paragraphs are dropped:
/// see the module docs for why that is safe (dropped text is never
/// emitted anywhere downstream).
#[must_use]
pub fn chunk_reply_history(paragraphs: Vec<String>) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    for paragraph in paragraphs {
        let paragraph_len = paragraph.chars().count();
        let separator_len = if current.is_empty() { 0 } else { 2 };
        if !current.is_empty()
            && current_len + separator_len + paragraph_len > REPLY_HISTORY_CHUNK_MAX_CHARS
        {
            chunks.push(std::mem::take(&mut current));
            current_len = 0;
            if chunks.len() >= REPLY_HISTORY_MAX_CHUNKS {
                return chunks;
            }
        }
        if !current.is_empty() {
            current.push_str("\n\n");
            current_len += 2;
        }
        current.push_str(&paragraph);
        current_len += paragraph_len;
    }
    if !current.is_empty() && chunks.len() < REPLY_HISTORY_MAX_CHUNKS {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walker::canonicalize_html;

    // --- Outlook reply-history detection: HTML-path integration tests ---

    #[test]
    fn outlook_style_reply_header_is_moved_to_quote_blocks() {
        // Outlook HTML replies put new text first, then an <hr>, then a
        // From:/Sent:/To:/Subject: header block, then the original message
        // -- with no <blockquote> anywhere. The walker does not parse
        // attributes, so `id="appendonsend"`/`id="divRplyFwdMsg"` are inert
        // noise; detection must be purely text-based.
        let out = canonicalize_html(concat!(
            "<div>Let's move it one hour later.</div>",
            "<div id=\"appendonsend\"></div><hr>",
            "<div id=\"divRplyFwdMsg\">",
            "<b>From:</b> Alex &lt;alex@example.invalid&gt;<br>",
            "<b>Sent:</b> Monday, 1 September 2025 09:00<br>",
            "<b>To:</b> Me &lt;me@example.invalid&gt;<br>",
            "<b>Subject:</b> Meeting time</div>",
            "<div>Can we change the meeting time?</div>",
        ))
        .unwrap();
        assert_eq!(
            out.body_blocks,
            vec!["Let's move it one hour later.".to_string()]
        );
        // Both moved paragraphs are well under the 4096-char chunk limit,
        // so `chunk_reply_history` merges them into a single quote chunk.
        assert_eq!(
            out.quote_blocks,
            vec![
                "From: Alex <alex@example.invalid>\nSent: Monday, 1 September 2025 09:00\nTo: Me <me@example.invalid>\nSubject: Meeting time\n\nCan we change the meeting time?".to_string(),
            ]
        );
    }

    #[test]
    fn outlook_reply_header_split_across_p_blocks_is_moved() {
        // Rule (c): the From:/Sent:/To:/Subject: header lines land in
        // separate top-level blocks (e.g. separate <p>s) instead of one
        // <br>-joined block; detection must look ahead across blocks.
        let out = canonicalize_html(concat!(
            "<p>New text here.</p>",
            "<p><b>From:</b> Alex &lt;alex@example.invalid&gt;</p>",
            "<p><b>Sent:</b> Monday, 1 September 2025 09:00</p>",
            "<p><b>To:</b> Me &lt;me@example.invalid&gt;</p>",
            "<p><b>Subject:</b> Meeting time</p>",
            "<p>Can we change the meeting time?</p>",
        ))
        .unwrap();
        assert_eq!(out.body_blocks, vec!["New text here.".to_string()]);
        // All five moved paragraphs are well under the 4096-char chunk
        // limit, so `chunk_reply_history` merges them into one chunk.
        assert_eq!(
            out.quote_blocks,
            vec![
                concat!(
                    "From: Alex <alex@example.invalid>\n\n",
                    "Sent: Monday, 1 September 2025 09:00\n\n",
                    "To: Me <me@example.invalid>\n\n",
                    "Subject: Meeting time\n\n",
                    "Can we change the meeting time?",
                )
                .to_string()
            ]
        );
    }

    #[test]
    fn underscore_separator_before_reply_header_is_moved_on_html_path() {
        // Outlook's separator rule: a lone underscore-run block immediately
        // before the header block (a separate paragraph, not a leading
        // line inside it) is moved along with it.
        let out = canonicalize_html(concat!(
            "<div>New text.</div>",
            "<div>________________________________</div>",
            "<div><b>From:</b> Alex &lt;alex@example.invalid&gt;<br>",
            "<b>Sent:</b> Monday, 1 September 2025 09:00<br>",
            "<b>To:</b> Me &lt;me@example.invalid&gt;<br>",
            "<b>Subject:</b> Meeting time</div>",
            "<div>Original message text.</div>",
        ))
        .unwrap();
        assert_eq!(out.body_blocks, vec!["New text.".to_string()]);
        assert_eq!(
            out.quote_blocks,
            vec![
                concat!(
                    "________________________________\n\n",
                    "From: Alex <alex@example.invalid>\n",
                    "Sent: Monday, 1 September 2025 09:00\n",
                    "To: Me <me@example.invalid>\n",
                    "Subject: Meeting time\n\n",
                    "Original message text.",
                )
                .to_string()
            ]
        );
    }

    #[test]
    fn original_message_separator_moves_itself_and_everything_after() {
        // Rule (b): a standalone "-----Original Message-----" paragraph
        // starts reply history on its own, with no From:/Subject:
        // requirement.
        let out = canonicalize_html(concat!(
            "<div>New text.</div>",
            "<div>-----Original Message-----</div>",
            "<div><b>From:</b> Alex &lt;alex@example.invalid&gt;<br>",
            "<b>Sent:</b> Monday, 1 September 2025 09:00</div>",
            "<div>Original message text.</div>",
        ))
        .unwrap();
        assert_eq!(out.body_blocks, vec!["New text.".to_string()]);
        assert_eq!(
            out.quote_blocks,
            vec![
                concat!(
                    "-----Original Message-----\n\n",
                    "From: Alex <alex@example.invalid>\n",
                    "Sent: Monday, 1 September 2025 09:00\n\n",
                    "Original message text.",
                )
                .to_string()
            ]
        );
    }

    #[test]
    fn original_message_as_first_line_of_a_larger_block_is_moved() {
        // Rule (b) now fires when the paragraph's FIRST LINE (not the
        // whole paragraph) is exactly the separator: here it shares one
        // <div> with the rest of the header.
        let out = canonicalize_html(concat!(
            "<div>New text.</div>",
            "<div>-----Original Message-----<br>",
            "<b>From:</b> Alex<br>",
            "<b>Sent:</b> Monday<br>",
            "<b>Subject:</b> Meeting</div>",
            "<div>Original body.</div>",
        ))
        .unwrap();
        assert_eq!(out.body_blocks, vec!["New text.".to_string()]);
        assert_eq!(
            out.quote_blocks,
            vec![
                concat!(
                    "-----Original Message-----\n",
                    "From: Alex\n",
                    "Sent: Monday\n",
                    "Subject: Meeting\n\n",
                    "Original body.",
                )
                .to_string()
            ]
        );
    }

    #[test]
    fn leading_underscore_line_then_original_message_in_same_block_is_moved() {
        // The same case again with a leading underscore-rule line inside
        // the same block, ahead of the separator: "first significant
        // line" must skip both before the (here, rule (c)) From: test.
        let out = canonicalize_html(concat!(
            "<div>New text.</div>",
            "<div>________________________________<br>",
            "-----Original Message-----<br>",
            "<b>From:</b> Alex<br>",
            "<b>Sent:</b> Monday<br>",
            "<b>Subject:</b> Meeting</div>",
            "<div>Original body.</div>",
        ))
        .unwrap();
        assert_eq!(out.body_blocks, vec!["New text.".to_string()]);
        assert_eq!(
            out.quote_blocks,
            vec![
                concat!(
                    "________________________________\n",
                    "-----Original Message-----\n",
                    "From: Alex\n",
                    "Sent: Monday\n",
                    "Subject: Meeting\n\n",
                    "Original body.",
                )
                .to_string()
            ]
        );
    }

    #[test]
    fn header_first_message_with_nothing_before_it_is_not_split() {
        // If the very first paragraph of the very first body_blocks entry
        // already looks like reply history (even after the underscore
        // look-back), with nothing preceding it anywhere in the message,
        // the message is left alone rather than emptied into an all-quote
        // message.
        let out = canonicalize_html(concat!(
            "<div><b>From:</b> Alex &lt;alex@example.invalid&gt;<br>",
            "<b>Sent:</b> Monday, 1 September 2025 09:00<br>",
            "<b>Subject:</b> Meeting time</div>",
            "<div>FYI, can you handle this?</div>",
        ))
        .unwrap();
        assert_eq!(
            out.body_blocks,
            vec![
                concat!(
                    "From: Alex <alex@example.invalid>\n",
                    "Sent: Monday, 1 September 2025 09:00\n",
                    "Subject: Meeting time\n\n",
                    "FYI, can you handle this?",
                )
                .to_string()
            ]
        );
        assert!(out.quote_blocks.is_empty());
    }

    #[test]
    fn reply_history_move_is_confined_to_the_matching_body_entry() {
        // A body_blocks entry that comes AFTER a real <blockquote> (the
        // only way the walker ever produces more than one entry) must
        // stay in the body even though an earlier entry contains reply
        // history: the move is confined to the entry that matched.
        let out = canonicalize_html(concat!(
            "<div>New text.</div>",
            "<div><b>From:</b> Alex &lt;alex@example.invalid&gt;<br>",
            "<b>Sent:</b> Monday, 1 September 2025 09:00<br>",
            "<b>Subject:</b> Meeting time</div>",
            "<div>Original body.</div>",
            "<blockquote>Q</blockquote>",
            "<div>Trailing body after quote.</div>",
        ))
        .unwrap();
        assert_eq!(
            out.body_blocks,
            vec![
                "New text.".to_string(),
                "Trailing body after quote.".to_string(),
            ]
        );
        assert_eq!(
            out.quote_blocks,
            vec![
                "Q".to_string(),
                concat!(
                    "From: Alex <alex@example.invalid>\n",
                    "Sent: Monday, 1 September 2025 09:00\n",
                    "Subject: Meeting time\n\n",
                    "Original body.",
                )
                .to_string(),
            ]
        );
    }

    #[test]
    fn deep_reply_history_is_bounded_to_at_most_eight_quote_chunks() {
        // Scaling risk: without chunking, one quote block per quoted
        // paragraph is unbounded and a deep Outlook thread can alone blow
        // past `MAX_BLOCKS_PER_MESSAGE` (64 in `message.rs`), rejecting the
        // whole message. `chunk_reply_history` bounds the moved history to
        // at most 8 chunks regardless of how many paragraphs it started as.
        let mut html = String::from(concat!(
            "<div>New text.</div>",
            "<div id=\"appendonsend\"></div><hr>",
            "<div id=\"divRplyFwdMsg\">",
            "<b>From:</b> Alex &lt;alex@example.invalid&gt;<br>",
            "<b>Sent:</b> Monday, 1 September 2025 09:00<br>",
            "<b>To:</b> Me &lt;me@example.invalid&gt;<br>",
            "<b>Subject:</b> Meeting time</div>",
        ));
        for i in 0..100 {
            use core::fmt::Write as _;
            let _ = write!(
                html,
                "<div>Quoted paragraph number {i} of the original message.</div>"
            );
        }
        let out = canonicalize_html(&html).unwrap();
        assert_eq!(out.body_blocks, vec!["New text.".to_string()]);
        assert!(
            out.quote_blocks.len() <= 8,
            "expected at most 8 quote chunks, got {}",
            out.quote_blocks.len()
        );
        // `1 + body_blocks.len() + quote_blocks.len()` mirrors
        // `MessageError`'s `content_block_count` check (subject + body +
        // quote, `MAX_BLOCKS_PER_MESSAGE == 64`).
        assert!(1 + out.body_blocks.len() + out.quote_blocks.len() <= 64);
    }

    #[test]
    fn reply_history_beyond_the_chunk_cap_is_dropped_and_body_is_unchanged() {
        // The test above only fills 2 chunks, so it never exercises the
        // cap-and-drop path itself. This fixture's quoted paragraphs alone
        // total well over `8 * 4096` chars, forcing `chunk_reply_history`
        // to actually drop the remainder once the 8th chunk fills up.
        let header = concat!(
            "<div>New text.</div>",
            "<div id=\"appendonsend\"></div><hr>",
            "<div id=\"divRplyFwdMsg\">",
            "<b>From:</b> Alex &lt;alex@example.invalid&gt;<br>",
            "<b>Sent:</b> Monday, 1 September 2025 09:00<br>",
            "<b>To:</b> Me &lt;me@example.invalid&gt;<br>",
            "<b>Subject:</b> Meeting time</div>",
        );
        let mut html = String::from(header);
        let mut paragraph_texts: Vec<String> = Vec::new();
        for i in 0..700 {
            use core::fmt::Write as _;
            let text = format!("Quoted line {i:04} of the original message body text.");
            let _ = write!(html, "<div>{text}</div>");
            paragraph_texts.push(text);
        }
        let paragraphs_total_chars: usize = paragraph_texts
            .iter()
            .map(|p| p.chars().count())
            .sum::<usize>()
            + 2 * (paragraph_texts.len() - 1);
        assert!(
            paragraphs_total_chars > 8 * 4096,
            "fixture must exceed the 8-chunk cap on its own; got {paragraphs_total_chars} chars"
        );

        let out = canonicalize_html(&html).unwrap();
        assert_eq!(out.body_blocks, vec!["New text.".to_string()]);
        assert_eq!(
            out.quote_blocks.len(),
            8,
            "expected exactly 8 quote chunks once the cap is exercised"
        );
        for chunk in &out.quote_blocks {
            assert!(
                chunk.chars().count() <= 4096,
                "chunk exceeds 4096 chars: {}",
                chunk.chars().count()
            );
        }
        // Order preserved: the header block is the very first moved
        // paragraph, so the first chunk must start with it.
        assert!(out.quote_blocks[0].starts_with("From: Alex <alex@example.invalid>"));
    }

    #[test]
    fn from_paragraph_with_subject_but_no_sent_or_date_stays_in_body() {
        // Both Subject: AND Sent:/Date: are required in the window;
        // Subject: alone (no Sent:/Date: anywhere in the same paragraph or
        // the next three) must not trigger a move.
        let out = canonicalize_html(concat!(
            "<div>New text.</div>",
            "<div><b>From:</b> Alex &lt;alex@example.invalid&gt;<br>",
            "<b>Subject:</b> Meeting time</div>",
            "<div>Can we change the meeting time?</div>",
        ))
        .unwrap();
        assert_eq!(
            out.body_blocks,
            vec![
                concat!(
                    "New text.\n\n",
                    "From: Alex <alex@example.invalid>\n",
                    "Subject: Meeting time\n\n",
                    "Can we change the meeting time?",
                )
                .to_string()
            ]
        );
        assert!(out.quote_blocks.is_empty());
    }

    #[test]
    fn underscore_look_back_landing_on_index_zero_leaves_input_unchanged() {
        // The match itself is found at index 1 (the header paragraph), but
        // the underscore look-back pulls it back to index 0 (the very
        // first paragraph of the very first entry) -- per the same "no
        // split when the match lands on paragraph 0" rule as
        // `header_first_message_with_nothing_before_it_is_not_split`, this
        // must leave the input completely unchanged.
        let out = canonicalize_html(concat!(
            "<div>________________________________</div>",
            "<div><b>From:</b> Alex &lt;alex@example.invalid&gt;<br>",
            "<b>Sent:</b> Monday, 1 September 2025 09:00<br>",
            "<b>Subject:</b> Meeting time</div>",
            "<div>FYI, can you handle this?</div>",
        ))
        .unwrap();
        assert_eq!(
            out.body_blocks,
            vec![
                concat!(
                    "________________________________\n\n",
                    "From: Alex <alex@example.invalid>\n",
                    "Sent: Monday, 1 September 2025 09:00\n",
                    "Subject: Meeting time\n\n",
                    "FYI, can you handle this?",
                )
                .to_string()
            ]
        );
        assert!(out.quote_blocks.is_empty());
    }

    #[test]
    fn from_mid_sentence_or_without_header_context_is_not_reply_history() {
        // A "From:" that isn't the start of a header block must never be
        // moved: mid-sentence text, and a block that starts with "From:"
        // but has no accompanying Subject:/Sent:/Date: line nearby.
        let out =
            canonicalize_html("<div>Please see From: field below for details.</div>").unwrap();
        assert_eq!(
            out.body_blocks,
            vec!["Please see From: field below for details.".to_string()]
        );
        assert!(out.quote_blocks.is_empty());

        let out = canonicalize_html(
            "<div>Let's talk.</div><div>From: Alex, following up on our call.</div>",
        )
        .unwrap();
        assert_eq!(
            out.body_blocks,
            vec!["Let's talk.\n\nFrom: Alex, following up on our call.".to_string()]
        );
        assert!(out.quote_blocks.is_empty());
    }

    #[test]
    fn existing_blockquote_case_is_unaffected_by_reply_history_detection() {
        // Blockquote-derived quote blocks must stay exactly as they were
        // before this feature existed.
        let out = canonicalize_html("<p>A&amp;B<br>C</p><blockquote>Q</blockquote>").unwrap();
        assert_eq!(out.body_blocks, vec!["A&B\nC".to_string()]);
        assert_eq!(out.quote_blocks, vec!["Q".to_string()]);
    }

    // --- Pure unit tests on `split_reply_history` ---

    #[test]
    fn round_trip_when_nothing_is_dropped_reproduces_original_paragraph_sequence() {
        // When nothing is dropped (total content well under the 8-chunk
        // cap), rejoining the body's tail entry and the moved chunk(s)
        // must exactly reproduce the original paragraph sequence: the move
        // only regroups paragraphs into different Vec entries, it never
        // adds, drops, or otherwise alters their text or separators.
        let paragraphs = [
            "New text.",
            "From: Alex\nSent: Monday\nSubject: Meeting",
            "Original message body.",
            "Second original paragraph.",
        ];
        let original = paragraphs.join("\n\n");
        let (body_blocks, quote_blocks) = split_reply_history(vec![original.clone()], vec![]);
        let reconstructed = body_blocks
            .into_iter()
            .chain(quote_blocks)
            .collect::<Vec<_>>()
            .join("\n\n");
        assert_eq!(reconstructed, original);
    }

    #[test]
    fn multi_entry_round_trip_reassembles_the_matching_entrys_paragraph_sequence() {
        // Two body_blocks entries (the shape a real <blockquote> produces),
        // with the reply-history match in the FIRST entry and a
        // pre-existing (blockquote-derived) quote_blocks entry already in
        // place. The second body entry and the pre-existing quote entry
        // must be untouched; the first entry's kept prefix plus the newly
        // appended chunk(s) must reproduce that entry's original
        // paragraph sequence exactly.
        let first_entry_paragraphs = [
            "New text.",
            "From: Alex\nSent: Monday\nSubject: Meeting",
            "Original message body.",
        ];
        let first_entry = first_entry_paragraphs.join("\n\n");
        let second_entry = "Trailing body after quote.".to_string();
        let existing_quote = vec!["Q".to_string()];

        let (body_blocks, quote_blocks) = split_reply_history(
            vec![first_entry.clone(), second_entry.clone()],
            existing_quote.clone(),
        );

        assert_eq!(body_blocks.len(), 2);
        assert_eq!(body_blocks[1], second_entry);
        assert_eq!(
            &quote_blocks[..existing_quote.len()],
            existing_quote.as_slice()
        );

        let moved_chunks = &quote_blocks[existing_quote.len()..];
        let reconstructed_first_entry = std::iter::once(body_blocks[0].clone())
            .chain(moved_chunks.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n\n");
        assert_eq!(reconstructed_first_entry, first_entry);
    }

    #[test]
    fn is_reply_history_start_never_panics_on_an_out_of_range_index() {
        assert!(!is_reply_history_start(&[], 0));
        assert!(!is_reply_history_start(
            &["From: Alex\nSubject: X\nSent: Y"],
            5
        ));
    }
}
