//! The exact 78-byte envelope AAD encoding.
//!
//! `envelope_suite.aad_encoding`: "exact 78-byte concatenation with no length
//! prefixes optional fields or alternative encoding". Every field is fixed
//! width, so the encoding is injective by construction: two distinct
//! `(domain, algorithm, key_id, account_binding, record_type, record_id,
//! schema_version, ciphertext_version)` tuples can never collide onto the
//! same 78 bytes, and there is exactly one way to parse 78 bytes back into a
//! tuple. `aad_encoding_rows` fixes the field order and width used below.

use crate::ids::{
    AccountBindingAad, CIPHERTEXT_VERSION, KeyId, RecordId, RecordType, SchemaVersion,
};

/// `envelope_suite.aad_encoding_rows[0]`: 18 fixed ASCII bytes.
const DOMAIN: &[u8; 18] = b"openloops-state-v1";
/// `envelope_suite.algorithm_code` for AES-256-GCM.
const ALGORITHM_CODE: u16 = 1;
/// `envelope_suite.aad_total_bytes`.
pub const AAD_BYTES: usize = 78;

/// The exact typed AAD tuple bound to one envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnvelopeAad {
    pub key_id: KeyId,
    pub account_binding_aad: AccountBindingAad,
    pub record_type: RecordType,
    pub record_id: RecordId,
    pub schema_version: SchemaVersion,
}

/// A rejected [`EnvelopeAad::decode`] input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AadDecodeError {
    /// The input was not exactly [`AAD_BYTES`] long.
    WrongLength,
    /// The fixed domain tag did not match `openloops-state-v1`.
    WrongDomain,
    /// The algorithm code was not the closed AES-256-GCM code `1`.
    UnknownAlgorithm,
    /// `record_type` was outside the closed `record_type_codes` catalog.
    UnknownRecordType,
    /// `schema_version` was zero.
    ZeroSchemaVersion,
    /// `ciphertext_version` was not the closed value `1`.
    UnknownCiphertextVersion,
    /// `key_id`, `account_binding_aad`, or `record_id` was all-zero.
    AllZeroIdentifier,
}

impl EnvelopeAad {
    /// Encodes the exact 78-byte AAD per `aad_encoding_rows`.
    #[must_use]
    pub fn encode(&self) -> [u8; AAD_BYTES] {
        let mut out = [0u8; AAD_BYTES];
        let mut at = 0usize;
        out[at..at + 18].copy_from_slice(DOMAIN);
        at += 18;
        out[at..at + 2].copy_from_slice(&ALGORITHM_CODE.to_be_bytes());
        at += 2;
        out[at..at + 16].copy_from_slice(self.key_id.as_bytes());
        at += 16;
        out[at..at + 16].copy_from_slice(self.account_binding_aad.as_bytes());
        at += 16;
        out[at..at + 2].copy_from_slice(&self.record_type.code().to_be_bytes());
        at += 2;
        out[at..at + 16].copy_from_slice(self.record_id.as_bytes());
        at += 16;
        out[at..at + 4].copy_from_slice(&self.schema_version.get().to_be_bytes());
        at += 4;
        out[at..at + 4].copy_from_slice(&CIPHERTEXT_VERSION.to_be_bytes());
        at += 4;
        debug_assert_eq!(at, AAD_BYTES);
        out
    }

    /// Decodes and fully validates a 78-byte AAD.
    ///
    /// # Errors
    ///
    /// Returns the specific [`AadDecodeError`] variant for the first
    /// violated field; this is only used to reconstruct the typed AAD before
    /// re-encoding it to check the transmitted AAD bytes, never to accept
    /// unauthenticated AAD as ambient truth (`envelope_suite.authentication_failure`
    /// still governs whether the record is usable).
    pub fn decode(bytes: &[u8]) -> Result<Self, AadDecodeError> {
        if bytes.len() != AAD_BYTES {
            return Err(AadDecodeError::WrongLength);
        }
        let mut at = 0usize;
        if &bytes[at..at + 18] != DOMAIN {
            return Err(AadDecodeError::WrongDomain);
        }
        at += 18;
        let algorithm = u16::from_be_bytes([bytes[at], bytes[at + 1]]);
        if algorithm != ALGORITHM_CODE {
            return Err(AadDecodeError::UnknownAlgorithm);
        }
        at += 2;
        let key_id = read_id(bytes, at).ok_or(AadDecodeError::AllZeroIdentifier)?;
        at += 16;
        let account_binding_aad = read_id(bytes, at).ok_or(AadDecodeError::AllZeroIdentifier)?;
        at += 16;
        let record_type_code = u16::from_be_bytes([bytes[at], bytes[at + 1]]);
        let record_type = RecordType::from_code(record_type_code)
            .map_err(|_| AadDecodeError::UnknownRecordType)?;
        at += 2;
        let record_id = read_id(bytes, at).ok_or(AadDecodeError::AllZeroIdentifier)?;
        at += 16;
        let schema_version_raw =
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        let schema_version = core::num::NonZeroU32::new(schema_version_raw)
            .map(SchemaVersion::new)
            .ok_or(AadDecodeError::ZeroSchemaVersion)?;
        at += 4;
        let ciphertext_version =
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        if ciphertext_version != CIPHERTEXT_VERSION {
            return Err(AadDecodeError::UnknownCiphertextVersion);
        }
        Ok(Self {
            key_id,
            account_binding_aad,
            record_type,
            record_id,
            schema_version,
        })
    }
}

fn read_id(bytes: &[u8], at: usize) -> Option<crate::ids::RandomId> {
    let mut raw = [0u8; 16];
    raw.copy_from_slice(&bytes[at..at + 16]);
    crate::ids::RandomId::from_random_bytes(raw).ok()
}

#[cfg(test)]
mod tests {
    use super::{AAD_BYTES, AadDecodeError, EnvelopeAad};
    use crate::ids::{RandomId, RecordType, SchemaVersion};

    fn sample() -> EnvelopeAad {
        EnvelopeAad {
            key_id: RandomId::from_random_bytes([1u8; 16]).unwrap(),
            account_binding_aad: RandomId::from_random_bytes([2u8; 16]).unwrap(),
            record_type: RecordType::Loop,
            record_id: RandomId::from_random_bytes([3u8; 16]).unwrap(),
            schema_version: SchemaVersion::new(core::num::NonZeroU32::new(1).unwrap()),
        }
    }

    #[test]
    fn encodes_to_exactly_78_bytes() {
        assert_eq!(sample().encode().len(), AAD_BYTES);
    }

    #[test]
    fn round_trips_through_decode() {
        let aad = sample();
        let decoded = EnvelopeAad::decode(&aad.encode()).expect("valid encoding decodes");
        assert_eq!(decoded, aad);
    }

    #[test]
    fn rejects_wrong_length() {
        let mut bytes = sample().encode().to_vec();
        bytes.pop();
        assert_eq!(
            EnvelopeAad::decode(&bytes),
            Err(AadDecodeError::WrongLength)
        );
    }

    #[test]
    fn rejects_wrong_domain() {
        let mut bytes = sample().encode();
        bytes[0] ^= 0xFF;
        assert_eq!(
            EnvelopeAad::decode(&bytes),
            Err(AadDecodeError::WrongDomain)
        );
    }

    #[test]
    fn rejects_unknown_algorithm() {
        let mut bytes = sample().encode();
        bytes[18] = 0xFF;
        bytes[19] = 0xFF;
        assert_eq!(
            EnvelopeAad::decode(&bytes),
            Err(AadDecodeError::UnknownAlgorithm)
        );
    }

    #[test]
    fn rejects_all_zero_key_id() {
        let mut bytes = sample().encode();
        bytes[20..36].fill(0);
        assert_eq!(
            EnvelopeAad::decode(&bytes),
            Err(AadDecodeError::AllZeroIdentifier)
        );
    }

    #[test]
    fn rejects_all_zero_account_binding() {
        let mut bytes = sample().encode();
        bytes[36..52].fill(0);
        assert_eq!(
            EnvelopeAad::decode(&bytes),
            Err(AadDecodeError::AllZeroIdentifier)
        );
    }

    #[test]
    fn rejects_unknown_record_type() {
        let mut bytes = sample().encode();
        bytes[52] = 0;
        bytes[53] = 0;
        assert_eq!(
            EnvelopeAad::decode(&bytes),
            Err(AadDecodeError::UnknownRecordType)
        );
    }

    #[test]
    fn rejects_all_zero_record_id() {
        let mut bytes = sample().encode();
        bytes[54..70].fill(0);
        assert_eq!(
            EnvelopeAad::decode(&bytes),
            Err(AadDecodeError::AllZeroIdentifier)
        );
    }

    #[test]
    fn rejects_zero_schema_version() {
        let mut bytes = sample().encode();
        bytes[70..74].fill(0);
        assert_eq!(
            EnvelopeAad::decode(&bytes),
            Err(AadDecodeError::ZeroSchemaVersion)
        );
    }

    #[test]
    fn rejects_unknown_ciphertext_version() {
        let mut bytes = sample().encode();
        bytes[74..78].copy_from_slice(&2u32.to_be_bytes());
        assert_eq!(
            EnvelopeAad::decode(&bytes),
            Err(AadDecodeError::UnknownCiphertextVersion)
        );
    }

    /// Two distinct logical identities can never encode to the same 78
    /// bytes: every field is fixed-width with no shared delimiter, so a
    /// change to any one field always changes a disjoint byte range.
    #[test]
    fn distinct_identities_never_collide() {
        let base = sample();
        let mut other_record_id = base;
        other_record_id.record_id = RandomId::from_random_bytes([9u8; 16]).unwrap();
        assert_ne!(base.encode(), other_record_id.encode());

        let mut other_account = base;
        other_account.account_binding_aad = RandomId::from_random_bytes([9u8; 16]).unwrap();
        assert_ne!(base.encode(), other_account.encode());

        let mut other_key = base;
        other_key.key_id = RandomId::from_random_bytes([9u8; 16]).unwrap();
        assert_ne!(base.encode(), other_key.encode());

        let mut other_type = base;
        other_type.record_type = RecordType::AccountBinding;
        assert_ne!(base.encode(), other_type.encode());

        let mut other_version = base;
        other_version.schema_version = SchemaVersion::new(core::num::NonZeroU32::new(2).unwrap());
        assert_ne!(base.encode(), other_version.encode());
    }
}
