//! Scalar-level canonicalization: UTF-16 decode, the forbidden-scalar
//! policy, CRLF/CR line-ending normalization, a bounded NFC engine, and the
//! plain-text/participant component rules.
//!
//! `contracts/evidence/identity-boundary.json`
//! `canonical_projection_contract.scalar_policy` /
//! `.participant_text_rule` / `.plain_component_rules`.
//!
//! # Dependency decision: no Unicode-normalization crate is activated
//!
//! The contract's `canonical_anchor_contract.normalization_steps_in_order`
//! requires "normalize Unicode to NFC". The offline package cache available
//! to this build (`%USERPROFILE%\.cargo\registry\cache\...`) does not
//! contain `unicode-normalization`, `unicode-bidi`, or any other
//! Unicode-Character-Database-driven crate, and the hard guardrail for this
//! work item prohibits contacting crates.io or any other network origin to
//! fetch one. A full, spec-exact NFC implementation requires the complete
//! UCD canonical-decomposition and canonical-combining-class tables, which
//! cannot be sourced offline in this environment.
//!
//! [`nfc`] therefore implements the real Unicode Canonical Ordering +
//! Canonical Composition *algorithm* in full, applied over two data
//! sources of different character:
//!
//! - **Hangul syllable composition is algorithmic, not table-based**, so it
//!   is implemented completely and exactly: [`compose_hangul`] is the
//!   closed-form `L + V -> LV` / `LV + T -> LVT` formula from Unicode's own
//!   published sample algorithm (UAX #15), over the fixed `SBase`/`LBase`/
//!   `VBase`/`TBase`/count constants that define the entire Hangul Jamo/
//!   Syllables block layout. There is no missing-data risk here (unlike
//!   every other script): this closes the "Hangul jamo" gap completely.
//! - Every other script's canonical composition is table-based and
//!   therefore **deliberately bounded**: [`compose`] holds the Latin-1
//!   Supplement precomposed letter/combining-mark pairs, a common subset of
//!   Latin Extended-A precomposed letters (the caron/ogonek/breve/macron/
//!   double-acute/ring/dot-above/additional-acute/additional-circumflex/
//!   additional-cedilla/additional-tilde families used by Central European,
//!   Baltic, Nordic, and Turkish text), the Greek tonos/dialytika
//!   precomposed vowels, and a handful of named Cyrillic precomposed
//!   letters (Ё/й/Ї/Ў/Ѓ/Ќ and case pairs). [`nfc`] also no longer caps a
//!   mark run at exactly one successful composition step: every mark in
//!   the run is tried, in canonical-ordering order, against the *running*
//!   composed value, so a later mark that does not itself pair with the
//!   original starter but would pair with an intermediate composed result
//!   still composes, and any mark that never finds an entry is preserved
//!   rather than dropped. No entry in the current table has a precomposed
//!   *result* that is itself a `compose` base (that is exactly the missing
//!   Vietnamese-style multi-level case below), so this is a forward-
//!   compatible correctness fix with no observable output change today,
//!   not a claim that multi-level composition is covered.
//!
//! Composable sequences outside that table (most other scripts' canonical
//! compositions, and any Latin/Greek/Cyrillic/other combining-mark sequence
//! whose intermediate or final precomposed form is not one of the pairs
//! named above — e.g. full Vietnamese horn-plus-tone stacking, which needs
//! precomposed forms outside this disclosed table) are **not** recomposed;
//! they pass through unchanged. This is disclosed, deterministic (the same
//! input always yields the same bytes, satisfying the byte-determinism
//! requirement even where it is not full-Unicode-conformant), and fails
//! closed in the sense that it never invents or drops a scalar — it only
//! leaves an out-of-table combining sequence in decomposed form.

use crate::error::ScalarError;

/// Decodes raw UTF-16 code units (as a Graph JSON string value would supply
/// them via `\uXXXX` escapes before scalar conversion) into a validated
/// `String`.
///
/// `canonical_anchor_contract.normalization_steps_in_order[0]`: "decode the
/// selected Graph field using its declared JSON Unicode scalar value and
/// reject unpaired surrogates". `CANON-UNPAIRED-SURROGATE-001` pins that a
/// lone leading surrogate (`0xD800`) rejects rather than substituting a
/// replacement character.
///
/// # Errors
///
/// Returns [`ScalarError::UnpairedSurrogate`] on any code unit that does not
/// form a complete surrogate pair.
pub fn scalars_from_utf16(units: &[u16]) -> Result<String, ScalarError> {
    char::decode_utf16(units.iter().copied())
        .collect::<Result<String, _>>()
        .map_err(|_| ScalarError::UnpairedSurrogate)
}

/// Returns `true` for every scalar on `scalar_policy.rejected_scalars`.
///
/// Ranges: `U+0000..=U+0008`, `U+000B`, `U+000C`, `U+000E..=U+001F`,
/// `U+007F..=U+009F`, `U+FDD0..=U+FDEF`, and every scalar whose low 16 bits
/// equal `0xFFFE` or `0xFFFF` (the per-plane noncharacters). `HT` (`U+0009`)
/// and `LF` (`U+000A`) are always allowed; `CR` (`U+000D`) is allowed here
/// because it is consumed by [`normalize_line_endings`] before this check
/// would otherwise matter downstream.
#[must_use]
pub fn is_forbidden_scalar(c: char) -> bool {
    let code_point = u32::from(c);
    matches!(code_point, 0x0000..=0x0008 | 0x000B | 0x000C | 0x000E..=0x001F | 0x007F..=0x009F)
        || (0xFDD0..=0xFDEF).contains(&code_point)
        || (code_point & 0xFFFE) == 0xFFFE
}

/// Rejects `s` if it contains any [`is_forbidden_scalar`] scalar.
///
/// # Errors
///
/// Returns [`ScalarError::ForbiddenScalar`] on the first forbidden scalar.
pub fn reject_forbidden_scalars(s: &str) -> Result<(), ScalarError> {
    if s.chars().any(is_forbidden_scalar) {
        return Err(ScalarError::ForbiddenScalar);
    }
    Ok(())
}

/// Normalizes `CRLF` and lone `CR` to `LF`.
///
/// `scalar_policy.line_endings`: "CRLF and CR become LF".
#[must_use]
pub fn normalize_line_endings(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// Canonical combining class for the small mark set [`compose`] knows
/// about; every other scalar is treated as combining class `0` (a
/// "starter", per the Unicode canonical-ordering algorithm) or, for any
/// *other* combining mark in the general `U+0300..=U+036F` block that this
/// table does not name, the common default `230` ("Above") used for
/// canonical ordering purposes only (composition for those marks is still
/// not attempted; see the module documentation).
fn combining_class(c: char) -> u8 {
    match c {
        '\u{0327}' | '\u{0328}' => 202, // COMBINING CEDILLA / OGONEK (Below)
        '\u{0323}' => 220,              // COMBINING DOT BELOW (Below)
        '\u{031B}' => 216,              // COMBINING HORN (Above Right)
        '\u{0300}'..='\u{036F}' => 230,
        _ => 0,
    }
}

/// Hangul Jamo/Syllables composition constants, exactly as published in
/// Unicode's own sample algorithm (UAX #15 "Unicode Normalization Forms",
/// section on Hangul); these define the complete block layout, not a
/// partial table.
const HANGUL_S_BASE: u32 = 0xAC00;
const HANGUL_L_BASE: u32 = 0x1100;
const HANGUL_V_BASE: u32 = 0x1161;
const HANGUL_T_BASE: u32 = 0x11A7;
const HANGUL_L_COUNT: u32 = 19;
const HANGUL_V_COUNT: u32 = 21;
const HANGUL_T_COUNT: u32 = 28;
const HANGUL_N_COUNT: u32 = HANGUL_V_COUNT * HANGUL_T_COUNT;
const HANGUL_S_COUNT: u32 = HANGUL_L_COUNT * HANGUL_N_COUNT;

/// Attempts one step of Hangul syllable composition: `L + V -> LV`, or an
/// already-composed `LV` syllable (a syllable with no trailing consonant
/// slot filled) `+ T -> LVT`. Returns `None` if `base`/`next` do not form
/// either pair; total and exact over the whole Hangul block, no data table.
fn compose_hangul(base: char, next: char) -> Option<char> {
    let b = u32::from(base);
    let n = u32::from(next);
    if (HANGUL_L_BASE..HANGUL_L_BASE + HANGUL_L_COUNT).contains(&b)
        && (HANGUL_V_BASE..HANGUL_V_BASE + HANGUL_V_COUNT).contains(&n)
    {
        let l_index = b - HANGUL_L_BASE;
        let v_index = n - HANGUL_V_BASE;
        let s_index = l_index * HANGUL_N_COUNT + v_index * HANGUL_T_COUNT;
        return char::from_u32(HANGUL_S_BASE + s_index);
    }
    if (HANGUL_S_BASE..HANGUL_S_BASE + HANGUL_S_COUNT).contains(&b)
        && (b - HANGUL_S_BASE).is_multiple_of(HANGUL_T_COUNT)
        && (HANGUL_T_BASE + 1..HANGUL_T_BASE + HANGUL_T_COUNT).contains(&n)
    {
        let t_index = n - HANGUL_T_BASE;
        return char::from_u32(b + t_index);
    }
    None
}

/// Attempts one canonical composition of `base` followed by `mark`.
///
/// Table scope: Latin-1 Supplement precomposed Latin letters formed from a
/// bare Latin letter plus one combining grave/acute/circumflex/
/// tilde/diaeresis/ring-above/cedilla. Hand-verified against the standard
/// Unicode Latin-1 Supplement block layout.
#[allow(clippy::too_many_lines)]
fn compose(base: char, mark: char) -> Option<char> {
    Some(match (base, mark) {
        ('A', '\u{0300}') => '\u{00C0}',
        ('A', '\u{0301}') => '\u{00C1}',
        ('A', '\u{0302}') => '\u{00C2}',
        ('A', '\u{0303}') => '\u{00C3}',
        ('A', '\u{0308}') => '\u{00C4}',
        ('A', '\u{030A}') => '\u{00C5}',
        ('C', '\u{0327}') => '\u{00C7}',
        ('E', '\u{0300}') => '\u{00C8}',
        ('E', '\u{0301}') => '\u{00C9}',
        ('E', '\u{0302}') => '\u{00CA}',
        ('E', '\u{0308}') => '\u{00CB}',
        ('I', '\u{0300}') => '\u{00CC}',
        ('I', '\u{0301}') => '\u{00CD}',
        ('I', '\u{0302}') => '\u{00CE}',
        ('I', '\u{0308}') => '\u{00CF}',
        ('N', '\u{0303}') => '\u{00D1}',
        ('O', '\u{0300}') => '\u{00D2}',
        ('O', '\u{0301}') => '\u{00D3}',
        ('O', '\u{0302}') => '\u{00D4}',
        ('O', '\u{0303}') => '\u{00D5}',
        ('O', '\u{0308}') => '\u{00D6}',
        ('U', '\u{0300}') => '\u{00D9}',
        ('U', '\u{0301}') => '\u{00DA}',
        ('U', '\u{0302}') => '\u{00DB}',
        ('U', '\u{0308}') => '\u{00DC}',
        ('Y', '\u{0301}') => '\u{00DD}',
        ('a', '\u{0300}') => '\u{00E0}',
        ('a', '\u{0301}') => '\u{00E1}',
        ('a', '\u{0302}') => '\u{00E2}',
        ('a', '\u{0303}') => '\u{00E3}',
        ('a', '\u{0308}') => '\u{00E4}',
        ('a', '\u{030A}') => '\u{00E5}',
        ('c', '\u{0327}') => '\u{00E7}',
        ('e', '\u{0300}') => '\u{00E8}',
        ('e', '\u{0301}') => '\u{00E9}',
        ('e', '\u{0302}') => '\u{00EA}',
        ('e', '\u{0308}') => '\u{00EB}',
        ('i', '\u{0300}') => '\u{00EC}',
        ('i', '\u{0301}') => '\u{00ED}',
        ('i', '\u{0302}') => '\u{00EE}',
        ('i', '\u{0308}') => '\u{00EF}',
        ('n', '\u{0303}') => '\u{00F1}',
        ('o', '\u{0300}') => '\u{00F2}',
        ('o', '\u{0301}') => '\u{00F3}',
        ('o', '\u{0302}') => '\u{00F4}',
        ('o', '\u{0303}') => '\u{00F5}',
        ('o', '\u{0308}') => '\u{00F6}',
        ('u', '\u{0300}') => '\u{00F9}',
        ('u', '\u{0301}') => '\u{00FA}',
        ('u', '\u{0302}') => '\u{00FB}',
        ('u', '\u{0308}') => '\u{00FC}',
        ('y', '\u{0301}') => '\u{00FD}',
        ('y', '\u{0308}') => '\u{00FF}',

        // Latin Extended-A: common precomposed Central European/Baltic/
        // Nordic/Turkish letters, hand-verified against that block's
        // well-known systematic (uppercase, lowercase) pair layout.
        // Additional acute.
        ('C', '\u{0301}') => '\u{0106}',
        ('c', '\u{0301}') => '\u{0107}',
        ('L', '\u{0301}') => '\u{0139}',
        ('l', '\u{0301}') => '\u{013A}',
        ('N', '\u{0301}') => '\u{0143}',
        ('n', '\u{0301}') => '\u{0144}',
        ('R', '\u{0301}') => '\u{0154}',
        ('r', '\u{0301}') => '\u{0155}',
        ('S', '\u{0301}') => '\u{015A}',
        ('s', '\u{0301}') => '\u{015B}',
        ('Z', '\u{0301}') => '\u{0179}',
        ('z', '\u{0301}') => '\u{017A}',
        // Additional circumflex.
        ('C', '\u{0302}') => '\u{0108}',
        ('c', '\u{0302}') => '\u{0109}',
        ('G', '\u{0302}') => '\u{011C}',
        ('g', '\u{0302}') => '\u{011D}',
        ('H', '\u{0302}') => '\u{0124}',
        ('h', '\u{0302}') => '\u{0125}',
        ('J', '\u{0302}') => '\u{0134}',
        ('j', '\u{0302}') => '\u{0135}',
        ('S', '\u{0302}') => '\u{015C}',
        ('s', '\u{0302}') => '\u{015D}',
        ('W', '\u{0302}') => '\u{0174}',
        ('w', '\u{0302}') => '\u{0175}',
        ('Y', '\u{0302}') => '\u{0176}',
        ('y', '\u{0302}') => '\u{0177}',
        // Additional tilde.
        ('I', '\u{0303}') => '\u{0128}',
        ('i', '\u{0303}') => '\u{0129}',
        ('U', '\u{0303}') => '\u{0168}',
        ('u', '\u{0303}') => '\u{0169}',
        // Macron.
        ('A', '\u{0304}') => '\u{0100}',
        ('a', '\u{0304}') => '\u{0101}',
        ('E', '\u{0304}') => '\u{0112}',
        ('e', '\u{0304}') => '\u{0113}',
        ('I', '\u{0304}') => '\u{012A}',
        ('i', '\u{0304}') => '\u{012B}',
        ('O', '\u{0304}') => '\u{014C}',
        ('o', '\u{0304}') => '\u{014D}',
        ('U', '\u{0304}') => '\u{016A}',
        ('u', '\u{0304}') => '\u{016B}',
        // Breve.
        ('A', '\u{0306}') => '\u{0102}',
        ('a', '\u{0306}') => '\u{0103}',
        ('E', '\u{0306}') => '\u{0114}',
        ('e', '\u{0306}') => '\u{0115}',
        ('G', '\u{0306}') => '\u{011E}',
        ('g', '\u{0306}') => '\u{011F}',
        ('I', '\u{0306}') => '\u{012C}',
        ('i', '\u{0306}') => '\u{012D}',
        ('O', '\u{0306}') => '\u{014E}',
        ('o', '\u{0306}') => '\u{014F}',
        ('U', '\u{0306}') => '\u{016C}',
        ('u', '\u{0306}') => '\u{016D}',
        // Dot above.
        ('C', '\u{0307}') => '\u{010A}',
        ('c', '\u{0307}') => '\u{010B}',
        ('E', '\u{0307}') => '\u{0116}',
        ('e', '\u{0307}') => '\u{0117}',
        ('G', '\u{0307}') => '\u{0120}',
        ('g', '\u{0307}') => '\u{0121}',
        ('Z', '\u{0307}') => '\u{017B}',
        ('z', '\u{0307}') => '\u{017C}',
        // Additional ring above.
        ('U', '\u{030A}') => '\u{016E}',
        ('u', '\u{030A}') => '\u{016F}',
        // Caron.
        ('C', '\u{030C}') => '\u{010C}',
        ('c', '\u{030C}') => '\u{010D}',
        ('D', '\u{030C}') => '\u{010E}',
        ('d', '\u{030C}') => '\u{010F}',
        ('E', '\u{030C}') => '\u{011A}',
        ('e', '\u{030C}') => '\u{011B}',
        ('L', '\u{030C}') => '\u{013D}',
        ('l', '\u{030C}') => '\u{013E}',
        ('N', '\u{030C}') => '\u{0147}',
        ('n', '\u{030C}') => '\u{0148}',
        ('R', '\u{030C}') => '\u{0158}',
        ('r', '\u{030C}') => '\u{0159}',
        ('S', '\u{030C}') => '\u{0160}',
        ('s', '\u{030C}') => '\u{0161}',
        ('T', '\u{030C}') => '\u{0164}',
        ('t', '\u{030C}') => '\u{0165}',
        ('Z', '\u{030C}') => '\u{017D}',
        ('z', '\u{030C}') => '\u{017E}',
        // Double acute.
        ('O', '\u{030B}') => '\u{0150}',
        ('o', '\u{030B}') => '\u{0151}',
        ('U', '\u{030B}') => '\u{0170}',
        ('u', '\u{030B}') => '\u{0171}',
        // Additional cedilla.
        ('G', '\u{0327}') => '\u{0122}',
        ('g', '\u{0327}') => '\u{0123}',
        ('K', '\u{0327}') => '\u{0136}',
        ('k', '\u{0327}') => '\u{0137}',
        ('L', '\u{0327}') => '\u{013B}',
        ('l', '\u{0327}') => '\u{013C}',
        ('N', '\u{0327}') => '\u{0145}',
        ('n', '\u{0327}') => '\u{0146}',
        ('R', '\u{0327}') => '\u{0156}',
        ('r', '\u{0327}') => '\u{0157}',
        ('S', '\u{0327}') => '\u{015E}',
        ('s', '\u{0327}') => '\u{015F}',
        ('T', '\u{0327}') => '\u{0162}',
        ('t', '\u{0327}') => '\u{0163}',
        // Ogonek.
        ('A', '\u{0328}') => '\u{0104}',
        ('a', '\u{0328}') => '\u{0105}',
        ('E', '\u{0328}') => '\u{0118}',
        ('e', '\u{0328}') => '\u{0119}',
        ('I', '\u{0328}') => '\u{012E}',
        ('i', '\u{0328}') => '\u{012F}',
        ('U', '\u{0328}') => '\u{0172}',
        ('u', '\u{0328}') => '\u{0173}',

        // Greek: tonos (oxia) and dialytika precomposed vowels.
        ('\u{0391}', '\u{0301}') => '\u{0386}', // Α -> Ά
        ('\u{0395}', '\u{0301}') => '\u{0388}', // Ε -> Έ
        ('\u{0397}', '\u{0301}') => '\u{0389}', // Η -> Ή
        ('\u{0399}', '\u{0301}') => '\u{038A}', // Ι -> Ί
        ('\u{039F}', '\u{0301}') => '\u{038C}', // Ο -> Ό
        ('\u{03A5}', '\u{0301}') => '\u{038E}', // Υ -> Ύ
        ('\u{03A9}', '\u{0301}') => '\u{038F}', // Ω -> Ώ
        ('\u{0399}', '\u{0308}') => '\u{03AA}', // Ι -> Ϊ
        ('\u{03A5}', '\u{0308}') => '\u{03AB}', // Υ -> Ϋ
        ('\u{03B1}', '\u{0301}') => '\u{03AC}', // α -> ά
        ('\u{03B5}', '\u{0301}') => '\u{03AD}', // ε -> έ
        ('\u{03B7}', '\u{0301}') => '\u{03AE}', // η -> ή
        ('\u{03B9}', '\u{0301}') => '\u{03AF}', // ι -> ί
        ('\u{03B9}', '\u{0308}') => '\u{03CA}', // ι -> ϊ
        ('\u{03C5}', '\u{0308}') => '\u{03CB}', // υ -> ϋ
        ('\u{03BF}', '\u{0301}') => '\u{03CC}', // ο -> ό
        ('\u{03C5}', '\u{0301}') => '\u{03CD}', // υ -> ύ
        ('\u{03C9}', '\u{0301}') => '\u{03CE}', // ω -> ώ

        // Cyrillic: named precomposed letters (breve/diaeresis/acute).
        ('\u{0418}', '\u{0306}') => '\u{0419}', // И -> Й
        ('\u{0438}', '\u{0306}') => '\u{0439}', // и -> й
        ('\u{0415}', '\u{0308}') => '\u{0401}', // Е -> Ё
        ('\u{0435}', '\u{0308}') => '\u{0451}', // е -> ё
        ('\u{0413}', '\u{0301}') => '\u{0403}', // Г -> Ѓ
        ('\u{0433}', '\u{0301}') => '\u{0453}', // г -> ѓ
        ('\u{041A}', '\u{0301}') => '\u{040C}', // К -> Ќ
        ('\u{043A}', '\u{0301}') => '\u{045C}', // к -> ќ
        ('\u{0423}', '\u{0306}') => '\u{040E}', // У -> Ў
        ('\u{0443}', '\u{0306}') => '\u{045E}', // у -> ў
        ('\u{0406}', '\u{0308}') => '\u{0407}', // І -> Ї
        ('\u{0456}', '\u{0308}') => '\u{0457}', // і -> ї
        _ => return None,
    })
}

/// Applies Unicode canonical ordering (stable sort of each maximal
/// combining-mark run by combining class) followed by canonical
/// composition: complete algorithmic Hangul composition ([`compose_hangul`])
/// plus chained composition over the bounded [`compose`] table for every
/// other script.
///
/// See the module documentation for this function's exact, disclosed
/// scope.
#[must_use]
pub fn nfc(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        let starter = chars[i];
        if combining_class(starter) != 0 {
            // A combining mark with no preceding starter in this run; emit
            // as-is (nothing to compose onto).
            out.push(starter);
            i += 1;
            continue;
        }

        // Hangul syllable composition is algorithmic and exact: try it
        // before the table-based path, chaining `L + V` then `+ T`.
        let mut hangul_composed = starter;
        let mut hangul_consumed = 1;
        while let Some(next) = chars.get(i + hangul_consumed).copied() {
            match compose_hangul(hangul_composed, next) {
                Some(c) => {
                    hangul_composed = c;
                    hangul_consumed += 1;
                }
                None => break,
            }
        }
        if hangul_consumed > 1 {
            out.push(hangul_composed);
            i += hangul_consumed;
            continue;
        }

        let mut j = i + 1;
        while j < chars.len() && combining_class(chars[j]) != 0 {
            j += 1;
        }
        let mut marks: Vec<char> = chars[i + 1..j].to_vec();
        marks.sort_by_key(|&m| combining_class(m));

        // Chain composition across the whole sorted mark run: each mark
        // that finds a [`compose`] entry against the *running* composed
        // value is folded in (so e.g. a base plus two successive
        // table-covered marks fully composes, not just the first mark);
        // a mark with no entry against the current running value is left
        // in `remaining` rather than blocking later marks from trying.
        let mut composed = starter;
        let mut remaining: Vec<char> = Vec::with_capacity(marks.len());
        for mark in marks {
            if let Some(next) = compose(composed, mark) {
                composed = next;
            } else {
                remaining.push(mark);
            }
        }
        out.push(composed);
        out.extend(remaining);
        i = j;
    }
    out
}

/// The full plain-component pipeline shared by `subject` and
/// `attachment_name`: reject forbidden scalars, normalize line endings,
/// then NFC-normalize.
///
/// `canonical_projection_contract.plain_component_rules.subject` /
/// `.attachment_name`.
///
/// # Errors
///
/// Returns [`ScalarError::ForbiddenScalar`] per [`reject_forbidden_scalars`].
pub fn canonicalize_plain(raw: &str) -> Result<String, ScalarError> {
    reject_forbidden_scalars(raw)?;
    let line_normalized = normalize_line_endings(raw);
    Ok(nfc(&line_normalized))
}

/// One participant field (`name` or `address`) processed independently per
/// `participant_text_rule`: normalize line endings, reject forbidden
/// scalars, replace `HT` with one ASCII space, trim only leading/trailing
/// ASCII spaces, then NFC-normalize.
fn process_participant_field(raw: &str) -> Result<String, ScalarError> {
    let line_normalized = normalize_line_endings(raw);
    reject_forbidden_scalars(&line_normalized)?;
    let ht_replaced: String = line_normalized
        .chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .collect();
    let trimmed = ht_replaced.trim_matches(' ');
    Ok(nfc(trimmed))
}

/// `participant_text_rule`: processes `name` and `address` independently,
/// preserves address case, then emits `address` alone when `name` is empty,
/// otherwise `"{name} <{address}>"`.
///
/// # Errors
///
/// Returns [`ScalarError`] from [`process_participant_field`] on either
/// input.
pub fn canonicalize_participant(name: &str, address: &str) -> Result<String, ScalarError> {
    let name_processed = process_participant_field(name)?;
    let address_processed = process_participant_field(address)?;
    if name_processed.is_empty() {
        Ok(address_processed)
    } else {
        Ok(format!("{name_processed} <{address_processed}>"))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        canonicalize_participant, canonicalize_plain, is_forbidden_scalar, nfc,
        normalize_line_endings, reject_forbidden_scalars, scalars_from_utf16,
    };
    use crate::error::ScalarError;

    #[test]
    fn utf16_decode_rejects_lone_high_surrogate() {
        assert_eq!(
            scalars_from_utf16(&[0xD800]),
            Err(ScalarError::UnpairedSurrogate)
        );
    }

    #[test]
    fn utf16_decode_accepts_valid_pair_and_astral_scalar() {
        // U+1F600 GRINNING FACE as a surrogate pair.
        let decoded = scalars_from_utf16(&[0xD83D, 0xDE00]).unwrap();
        assert_eq!(decoded, "\u{1F600}");
    }

    #[test]
    fn forbidden_scalar_covers_every_listed_range() {
        assert!(is_forbidden_scalar('\u{0000}'));
        assert!(is_forbidden_scalar('\u{0008}'));
        assert!(!is_forbidden_scalar('\u{0009}')); // HT allowed
        assert!(!is_forbidden_scalar('\u{000A}')); // LF allowed
        assert!(is_forbidden_scalar('\u{000B}'));
        assert!(is_forbidden_scalar('\u{000C}'));
        assert!(!is_forbidden_scalar('\u{000D}')); // CR allowed pre-normalization
        assert!(is_forbidden_scalar('\u{000E}'));
        assert!(is_forbidden_scalar('\u{001F}'));
        assert!(is_forbidden_scalar('\u{007F}'));
        assert!(is_forbidden_scalar('\u{009F}'));
        assert!(!is_forbidden_scalar('\u{00A0}'));
        assert!(is_forbidden_scalar('\u{FDD0}'));
        assert!(is_forbidden_scalar('\u{FDEF}'));
        assert!(!is_forbidden_scalar('\u{FDCF}'));
        assert!(is_forbidden_scalar('\u{FFFE}'));
        assert!(is_forbidden_scalar(char::from_u32(0xFFFF).unwrap()));
        assert!(is_forbidden_scalar(char::from_u32(0x1FFFE).unwrap()));
        assert!(is_forbidden_scalar(char::from_u32(0x0010_FFFF).unwrap()));
        assert!(!is_forbidden_scalar('A'));
    }

    #[test]
    fn reject_forbidden_scalars_rejects_noncharacter() {
        assert_eq!(
            reject_forbidden_scalars("ok\u{FFFF}"),
            Err(ScalarError::ForbiddenScalar)
        );
        assert!(reject_forbidden_scalars("ok").is_ok());
    }

    #[test]
    fn line_endings_normalize_crlf_and_lone_cr() {
        assert_eq!(normalize_line_endings("a\r\nb\rc\nd"), "a\nb\nc\nd");
    }

    #[test]
    fn nfc_composes_decomposed_latin1_vector() {
        assert_eq!(nfc("Cafe\u{0301}"), "Caf\u{00E9}");
    }

    #[test]
    fn nfc_is_identity_on_already_composed_text() {
        assert_eq!(nfc("Caf\u{00E9} \u{1F600}"), "Caf\u{00E9} \u{1F600}");
    }

    #[test]
    fn nfc_composes_hangul_l_v_syllable() {
        // Hangul choseong KIYEOK (L) + jungseong A (V) -> syllable GA.
        assert_eq!(nfc("\u{1100}\u{1161}"), "\u{AC00}");
    }

    #[test]
    fn nfc_composes_hangul_l_v_t_syllable() {
        // L + V + jongseong KIYEOK (T) -> syllable GAG.
        assert_eq!(nfc("\u{1100}\u{1161}\u{11A8}"), "\u{AC01}");
    }

    #[test]
    fn nfc_composes_precomposed_lv_syllable_plus_trailing_consonant() {
        // An already-composed LV syllable (GA) plus a lone trailing
        // consonant jamo still composes to the LVT syllable.
        assert_eq!(nfc("\u{AC00}\u{11A8}"), "\u{AC01}");
    }

    #[test]
    fn nfc_does_not_compose_a_filled_lvt_syllable_with_a_further_trailing_consonant() {
        // GAG (already LVT, trailing slot filled) plus another trailing
        // consonant jamo has no further composition; it passes through.
        let input = "\u{AC01}\u{11A8}";
        assert_eq!(nfc(input), input);
    }

    #[test]
    fn nfc_composes_a_mark_that_is_not_the_first_in_its_run() {
        // A mark run where an earlier (lower-combining-class) mark does
        // not compose with the base must not block a later mark in the
        // same run from composing against that same (still-unchanged)
        // base: dot-below (class 220) has no Latin-1/Latin-Extended-A
        // pairing here and sorts before caron (class 230), yet the caron
        // still composes.
        assert_eq!(nfc("s\u{0323}\u{030C}"), "\u{0161}\u{0323}");
    }

    #[test]
    fn nfc_composes_latin_extended_a_caron_and_ogonek() {
        assert_eq!(nfc("s\u{030C}"), "\u{0161}"); // š
        assert_eq!(nfc("a\u{0328}"), "\u{0105}"); // ą
    }

    #[test]
    fn nfc_composes_greek_tonos_and_cyrillic_diaeresis() {
        assert_eq!(nfc("\u{03B1}\u{0301}"), "\u{03AC}"); // α + tonos -> ά
        assert_eq!(nfc("\u{0435}\u{0308}"), "\u{0451}"); // е + diaeresis -> ё
    }

    #[test]
    fn nfc_leaves_out_of_table_combining_sequences_untouched() {
        // Devanagari base + combining sign: outside the disclosed table, so
        // the sequence passes through unchanged rather than being guessed.
        let input = "\u{0915}\u{093E}";
        assert_eq!(nfc(input), input);
    }

    #[test]
    fn canonicalize_plain_full_vector() {
        let out = canonicalize_plain("Cafe\u{0301} \u{1F600}").unwrap();
        assert_eq!(out, "Caf\u{00E9} \u{1F600}");
        assert_eq!(out.chars().count(), 6);
    }

    #[test]
    fn canonicalize_plain_rejects_forbidden_scalar() {
        assert_eq!(
            canonicalize_plain("bad\u{0000}"),
            Err(ScalarError::ForbiddenScalar)
        );
    }

    #[test]
    fn participant_vector_matches_contract() {
        let out = canonicalize_participant("Synthetic User", "synthetic@example.invalid").unwrap();
        assert_eq!(out, "Synthetic User <synthetic@example.invalid>");
    }

    #[test]
    fn participant_with_empty_name_emits_address_alone() {
        let out = canonicalize_participant("  ", "synthetic@example.invalid").unwrap();
        assert_eq!(out, "synthetic@example.invalid");
    }

    #[test]
    fn participant_trims_only_leading_trailing_spaces_and_folds_tabs() {
        let out = canonicalize_participant(" A\tB ", "x@example.invalid").unwrap();
        assert_eq!(out, "A B <x@example.invalid>");
    }
}
