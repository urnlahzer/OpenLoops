//! Minimal, bounded RFC 4648 §5 base64url (no padding), RFC 3986 query
//! percent-encoding, and percent-decoding.
//!
//! These are mechanical data transforms, not cryptography: the actual
//! randomness and hashing this crate needs come from
//! [`openloops_persistence::fill_random`]/[`openloops_persistence::sha256`].
//! `oauth2`, `url`, and `httparse` could not resolve their full transitive
//! dependency closures in this workspace's offline registry mirror (see
//! `crates/openloops-graph/Cargo.toml`'s header comment for the exact
//! missing crates and `lib.rs`'s module doc for the full deviation record),
//! so these narrow, exhaustively tested helpers substitute for the slice of
//! their behavior this story needs.

const BASE64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Encodes `input` as unpadded base64url (RFC 4648 §5).
#[must_use]
pub fn base64url_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut chunks = input.chunks_exact(3);
    for chunk in &mut chunks {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        push_group(&mut out, n, 4);
    }
    let remainder = chunks.remainder();
    match remainder.len() {
        0 => {}
        1 => {
            let n = u32::from(remainder[0]) << 16;
            push_group(&mut out, n, 2);
        }
        2 => {
            let n = (u32::from(remainder[0]) << 16) | (u32::from(remainder[1]) << 8);
            push_group(&mut out, n, 3);
        }
        _ => unreachable!("chunks_exact(3) remainder is always 0..=2 bytes"),
    }
    out
}

fn push_group(out: &mut String, n: u32, chars: usize) {
    let indices = [
        (n >> 18) & 0x3F,
        (n >> 12) & 0x3F,
        (n >> 6) & 0x3F,
        n & 0x3F,
    ];
    for &index in indices.iter().take(chars) {
        out.push(char::from(BASE64URL_ALPHABET[index as usize]));
    }
}

/// Percent-encodes `input` for use as one query value (RFC 3986 unreserved
/// set `A-Za-z0-9-._~` passes through unescaped; everything else becomes
/// `%XX` uppercase hex).
#[must_use]
pub fn percent_encode_query_value(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(*byte));
            }
            other => {
                out.push('%');
                out.push(hex_digit(other >> 4));
                out.push(hex_digit(other & 0x0F));
            }
        }
    }
    out
}

/// Percent-decodes one query value. Rejects (`None`) a truncated or invalid
/// `%XX` escape and any invalid-UTF-8 result. `+` is treated as a literal:
/// this callback query string is RFC 3986 percent-encoded, not
/// `application/x-www-form-urlencoded` space substitution.
#[must_use]
pub fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let high = *bytes.get(index + 1)?;
                let low = *bytes.get(index + 2)?;
                out.push((hex_value(high)? << 4) | hex_value(low)?);
                index += 3;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        _ => char::from(b'A' + (nibble - 10)),
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{base64url_encode, percent_decode, percent_encode_query_value};

    #[test]
    fn base64url_of_sha256_is_a_stable_deterministic_regression_anchor() {
        // This environment has no network access to independently fetch
        // RFC 7636 Appendix B's published text, so this test does not
        // assert a hand-transcribed external vector (a wrong transcription
        // would be exactly the kind of unverified "ad hoc" value this
        // story must avoid). Instead it pins this crate's own
        // base64url(sha256(verifier)) computation as a regression anchor;
        // `base64url_round_trips_every_remainder_length` below
        // independently checks the encoder against the standard RFC 4648
        // test vectors, and `sha256` itself is `openloops-persistence`'s
        // already-reviewed `sha2` crate, not code this story adds.
        let verifier = "dbjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let digest = openloops_persistence::sha256(verifier.as_bytes());
        assert_eq!(
            base64url_encode(&digest),
            "eINzvQ3Z8aARYw9pLv0ISwvsVZ3cecpv476AyyP_wEo"
        );
    }

    #[test]
    fn s256_matches_the_rfc_7636_appendix_b_vector() {
        // RFC 7636 Appendix B: the published interoperability vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let digest = openloops_persistence::sha256(verifier.as_bytes());
        assert_eq!(
            base64url_encode(&digest),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn base64url_has_no_padding_or_unsafe_characters() {
        let encoded = base64url_encode(&[0xFFu8; 32]);
        assert!(!encoded.contains('='));
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
        assert_eq!(encoded.len(), 43);
    }

    #[test]
    fn base64url_round_trips_every_remainder_length() {
        assert_eq!(base64url_encode(b""), "");
        assert_eq!(base64url_encode(b"f"), "Zg");
        assert_eq!(base64url_encode(b"fo"), "Zm8");
        assert_eq!(base64url_encode(b"foo"), "Zm9v");
        assert_eq!(base64url_encode(b"foob"), "Zm9vYg");
        assert_eq!(base64url_encode(b"fooba"), "Zm9vYmE");
        assert_eq!(base64url_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn percent_encode_covers_loopback_redirect_uri() {
        let encoded = percent_encode_query_value("http://127.0.0.1:54321/callback");
        assert_eq!(encoded, "http%3A%2F%2F127.0.0.1%3A54321%2Fcallback");
    }

    #[test]
    fn percent_decode_round_trips_encoded_values() {
        let original = "http://127.0.0.1:54321/callback?x=1";
        let encoded = percent_encode_query_value(original);
        assert_eq!(percent_decode(&encoded).as_deref(), Some(original));
    }

    #[test]
    fn percent_decode_rejects_truncated_and_invalid_escapes() {
        assert_eq!(percent_decode("abc%"), None);
        assert_eq!(percent_decode("abc%2"), None);
        assert_eq!(percent_decode("abc%zz"), None);
    }
}
