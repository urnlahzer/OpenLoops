# Jev relative date candidates

Date: 2026-10-22. Status: approved by the owner.

## Problem

On the decision-model (Jev) path, code offers Jev the dates a paragraph
states, and Jev picks one or `none`. The deadline extraction question
(`extract.temporal`) and the closure walk's date question (`walk.new_date`)
both get their options from `extraction_temporal_candidates` in
`crates/openloops-desktop/src/review_scan.rs`. That function finds explicit
dates only ("September 9, 2026", "2026-10-30"). A paragraph that says "by
Friday", "tomorrow" or "end of the week" gets no candidate:

- deadline extraction records no deadline for the claim;
- the walk asks no date question, so a promise with a relative date does not
  become a deadline suggestion, and a waiting-party due change escalates.

In the 600-thread synthetic corpus, 108 of the 180 promise and due-change
replies contain a date phrase. None of them gets a candidate.

The chat-model path already handles these phrases. Its prompt tells the model
to normalize "by Friday" to `friday`, and `deadline_parse::reparse` resolves
the normalized value against the message timestamp.

## Design

1. A deterministic finder reads one paragraph and returns the relative
   phrases it contains, in text order. Each phrase maps to one canonical value
   in the grammar `deadline_parse::reparse(TemporalKind::Relative, ..)`
   accepts. The finder matches whole words, ignores case, and prefers the
   longest match where phrases overlap ("end of day tomorrow" is `tomorrow`,
   not `today` plus `tomorrow`).

   | Text (examples) | Canonical value |
   |---|---|
   | today, tonight, this morning, this afternoon, this evening, later today, end of (the) day, EOD, close of business, COB | `today` |
   | tomorrow (with or without morning/afternoon/evening, "first thing"), end of day tomorrow, tomorrow EOD | `tomorrow` |
   | a weekday name, also after "on", "by", "this", "next", or before/after "EOD" / "end of day" | the weekday name |
   | next week, early next week | `next week` |
   | this week, end of (the) week, later this week | `end of week` |
   | next business day, next working day | `next business day` |
   | in / within N days (N as digits or one to fourteen) | `in N days` |
   | in / within N business or working days | `in N business days` |

   Phrases outside the table (for example "end of the month", "soon", "as
   soon as I can") give no candidate.

2. Code resolves each value with `reparse(TemporalKind::Relative, value,
   &governed_temporal_context(message))`. The candidate's date is the local
   calendar day of `deadline_parse::policy_boundary` for the parsed value: the
   day itself for a date or business day, the last day of the week range for a
   week. This is the same day the aging policy uses. Values that do not parse,
   and soft or event-relative values, give no candidate.

3. `extraction_temporal_candidates` returns the explicit candidates first, as
   today, then the relative candidates in text order. It removes duplicate
   dates, keeps the first, and keeps at most `MAX_EXTRACTION_OPTIONS`.
   Handles stay `t0`, `t1`, ... in that order. Each value is `YYYY-MM-DD` and
   passes the existing `reparse(DeadlineTemporalKind::Date, ..)` check.

4. Option text (amended 2026-10-22, see "Trial"): an explicit candidate's
   option text stays its date value. A relative candidate's option text is
   the phrase as written in the paragraph ("by Friday", "tomorrow
   morning"); its value stays the resolved `YYYY-MM-DD` date, and a pick
   records that date. Because duplicates keep the first candidate, an
   explicit date wins over a relative phrase that resolves to the same day.
   The questions' instructions, the registry and its hash pin do not
   change.

## Trial

The `extract.temporal` and `walk.new_date` states carry no message date.
With the date value as option text, Jev cannot connect "by Friday" to
`2026-10-16`. A paid trial on the 17 held-out promise paragraphs that have
a relative candidate (`walk.new_date`, tuned registry, accept 0.7):

| Option text | Picked | None | Gray |
|---|---|---|---|
| date value (`2026-10-16`) | 0 | 13 | 4 |
| date, weekday and phrase | 0 | 3 | 14 |
| date value, send date added to the state | 4 | 7 | 6 |
| phrase as written (`by Friday`) | 15 | 0 | 2 |

All 15 picks were the phrase that sets the promised time. Jev reads the
text and code does the date arithmetic. The trial took its phrases from a
regex approximation of the finder; the `evaluate-walk` rerun uses the
finder's own matched text.

## Effects

- `extract.temporal` offers relative dates; a confident pick records the
  resolved date as the claim's `date` temporal value.
- After `feat/jev-closure-walk` rebases onto this change, `walk.new_date`
  offers them too, because `walk_unit_date_candidates` calls
  `extraction_temporal_candidates`. Its `date_questions` must use the
  candidate's option text, and its harness export must write that text
  next to the value.

## Known limits

- "next Tuesday" resolves to the nearest following Tuesday, as the domain
  parser's bare weekday does.
- A weekday that refers to the past ("I sent it Monday") still gives a future
  candidate; Jev decides whether the text sets that date.
- "end of week" resolves to the last day of the Monday-start week (Sunday),
  as the aging policy does.

## Verification

- Unit tests: each table row resolves to the expected date from a fixed
  message timestamp; overlap gives one candidate; phrases outside the table
  give none; explicit candidates come first; duplicates are removed; the cap
  holds.
- An extraction test: a paragraph "I will send the report by Friday." offers
  an `extract.temporal` option whose text is "by Friday"; picking it records
  Friday's date.
- The jev-optimize harness regenerates `date_candidates.jsonl` with the
  desktop dev test and reruns `evaluate-walk`.
