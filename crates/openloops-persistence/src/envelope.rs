//! AES-256-GCM record envelopes.
//!
//! `envelope_suite`: AES-256-GCM with a full 128-bit tag, a fresh 96-bit
//! CSPRNG nonce per attempt, and the exact 78-byte AAD from [`crate::aad`].
//! `authentication_failure` releases zero plaintext; this module never
//! constructs a plaintext buffer that a caller can observe before the tag
//! verifies, because [`aead::Aead::decrypt`]'s default implementation only
//! returns the buffer after `decrypt_in_place` succeeds and drops it
//! otherwise.

use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Payload},
};

use crate::aad::{AAD_BYTES, AadDecodeError, EnvelopeAad};
use crate::ids::{CIPHERTEXT_VERSION, KeyId};

/// `envelope_suite.maximum_plaintext_bytes` / `maximum_ciphertext_bytes`.
pub const MAXIMUM_PLAINTEXT_BYTES: usize = 1_048_576;
/// `envelope_suite.nonce_bytes`.
pub const NONCE_BYTES: usize = 12;
/// `envelope_suite.tag_bytes`.
pub const TAG_BYTES: usize = 16;
/// `envelope_suite.key_bytes`.
pub const KEY_BYTES: usize = 32;

/// A zeroize-on-drop AES-256-GCM data-encryption key bound to one [`KeyId`].
///
/// `secret_inventory[state_aead_keyring]`: the only place raw key bytes ever
/// live is inside this type, in the protected `state-root.dpapi` bundle
/// ([`crate::dpapi`]), and inside the `aes_gcm` cipher state during one
/// encrypt/decrypt call. There is no `Debug`/`Display` impl that could leak
/// bytes into a log line.
pub struct EnvelopeKey {
    id: KeyId,
    bytes: zeroize::Zeroizing<[u8; KEY_BYTES]>,
}

impl EnvelopeKey {
    /// Wraps raw key bytes under the given [`KeyId`].
    #[must_use]
    pub fn new(id: KeyId, bytes: [u8; KEY_BYTES]) -> Self {
        Self {
            id,
            bytes: zeroize::Zeroizing::new(bytes),
        }
    }

    /// Returns the [`KeyId`] this key is bound to.
    #[must_use]
    pub const fn id(&self) -> KeyId {
        self.id
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(self.bytes.as_slice()).expect("key is exactly 32 bytes")
    }
}

/// A sealed AES-256-GCM record: the exact `minimum_clear_envelope_fields`
/// plus the ciphertext.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Envelope {
    pub aad: EnvelopeAad,
    pub nonce: [u8; NONCE_BYTES],
    pub tag: [u8; TAG_BYTES],
    pub ciphertext: Vec<u8>,
}

/// A rejected seal or open operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvelopeError {
    /// The plaintext exceeded `maximum_plaintext_bytes`.
    PlaintextTooLarge,
    /// The stored ciphertext exceeded `maximum_ciphertext_bytes`.
    CiphertextTooLarge,
    /// The key used to open the envelope does not match `envelope.aad.key_id`.
    KeyIdMismatch,
    /// Tag verification failed; `authentication_failure` — no plaintext is released.
    AuthenticationFailed,
    /// The stored `aad`/`nonce`/`tag` shape was structurally invalid.
    MalformedEnvelope,
}

impl From<AadDecodeError> for EnvelopeError {
    fn from(_: AadDecodeError) -> Self {
        Self::MalformedEnvelope
    }
}

/// Encrypts `plaintext` under `key` using `nonce`, binding it to `aad`.
///
/// The caller supplies `nonce`: [`crate::reservation`] owns the "reserve
/// before generate, generate before encrypt, burn on failure" sequencing
/// required by `transaction_contract.encryption_reservation_order`, so this
/// function stays a pure, directly testable cryptographic primitive with no
/// reservation bookkeeping of its own.
///
/// # Errors
///
/// Returns [`EnvelopeError::PlaintextTooLarge`] over
/// `maximum_plaintext_bytes`, or [`EnvelopeError::KeyIdMismatch`] if
/// `aad.key_id != key.id()`.
pub fn seal(
    key: &EnvelopeKey,
    aad: EnvelopeAad,
    nonce: [u8; NONCE_BYTES],
    plaintext: &[u8],
) -> Result<Envelope, EnvelopeError> {
    if aad.key_id.as_bytes() != key.id().as_bytes() {
        return Err(EnvelopeError::KeyIdMismatch);
    }
    if plaintext.len() > MAXIMUM_PLAINTEXT_BYTES {
        return Err(EnvelopeError::PlaintextTooLarge);
    }
    let aad_bytes = aad.encode();
    let nonce_array = aes_gcm::aead::Nonce::<Aes256Gcm>::from(nonce);
    let sealed = key
        .cipher()
        .encrypt(
            &nonce_array,
            Payload {
                msg: plaintext,
                aad: &aad_bytes,
            },
        )
        .map_err(|_| EnvelopeError::PlaintextTooLarge)?;
    debug_assert!(sealed.len() >= TAG_BYTES);
    let split_at = sealed.len() - TAG_BYTES;
    let mut tag = [0u8; TAG_BYTES];
    tag.copy_from_slice(&sealed[split_at..]);
    Ok(Envelope {
        aad,
        nonce,
        tag,
        ciphertext: sealed[..split_at].to_vec(),
    })
}

/// Authenticates and decrypts `envelope` under `key`.
///
/// `required_validation_order`: "on read authenticate the entire envelope
/// before releasing any plaintext" / "decrypt into a bounded temporary
/// buffer". This function releases the plaintext `Vec<u8>` only after the
/// GCM tag verifies; on any failure it returns
/// [`EnvelopeError::AuthenticationFailed`] with no plaintext, partial
/// plaintext, or diagnostic content.
///
/// # Errors
///
/// Returns [`EnvelopeError::CiphertextTooLarge`] over
/// `maximum_ciphertext_bytes`, [`EnvelopeError::KeyIdMismatch`] if
/// `envelope.aad.key_id != key.id()`, or
/// [`EnvelopeError::AuthenticationFailed`] on any tag mismatch — including a
/// mismatch caused solely by mutating one AAD field, since the AAD is
/// authenticated but never encrypted.
pub fn open(key: &EnvelopeKey, envelope: &Envelope) -> Result<Vec<u8>, EnvelopeError> {
    if envelope.aad.key_id.as_bytes() != key.id().as_bytes() {
        return Err(EnvelopeError::KeyIdMismatch);
    }
    if envelope.ciphertext.len() > MAXIMUM_PLAINTEXT_BYTES {
        return Err(EnvelopeError::CiphertextTooLarge);
    }
    let aad_bytes = envelope.aad.encode();
    let mut combined = Vec::with_capacity(envelope.ciphertext.len() + TAG_BYTES);
    combined.extend_from_slice(&envelope.ciphertext);
    combined.extend_from_slice(&envelope.tag);
    let nonce_array = aes_gcm::aead::Nonce::<Aes256Gcm>::from(envelope.nonce);
    key.cipher()
        .decrypt(
            &nonce_array,
            Payload {
                msg: &combined,
                aad: &aad_bytes,
            },
        )
        .map_err(|_| EnvelopeError::AuthenticationFailed)
}

/// The exact wire width of a serialized envelope's AAD component, for
/// callers that need to size a fixed record buffer.
#[must_use]
pub const fn aad_wire_bytes() -> usize {
    AAD_BYTES
}

/// The one closed ciphertext version this crate emits.
#[must_use]
pub const fn ciphertext_version() -> u32 {
    CIPHERTEXT_VERSION
}

#[cfg(test)]
mod tests {
    use super::{EnvelopeError, EnvelopeKey, NONCE_BYTES, open, seal};
    use crate::aad::EnvelopeAad;
    use crate::ids::{RandomId, RecordType, SchemaVersion};

    fn key() -> EnvelopeKey {
        EnvelopeKey::new(RandomId::from_random_bytes([7u8; 16]).unwrap(), [9u8; 32])
    }

    fn aad_for(key_id: crate::ids::KeyId) -> EnvelopeAad {
        EnvelopeAad {
            key_id,
            account_binding_aad: RandomId::from_random_bytes([2u8; 16]).unwrap(),
            record_type: RecordType::Loop,
            record_id: RandomId::from_random_bytes([3u8; 16]).unwrap(),
            schema_version: SchemaVersion::new(core::num::NonZeroU32::new(1).unwrap()),
        }
    }

    fn nonce(byte: u8) -> [u8; NONCE_BYTES] {
        [byte; NONCE_BYTES]
    }

    #[test]
    fn round_trips_plaintext() {
        let key = key();
        let aad = aad_for(key.id());
        let envelope = seal(&key, aad, nonce(1), b"synthetic plaintext").unwrap();
        let opened = open(&key, &envelope).unwrap();
        assert_eq!(opened, b"synthetic plaintext");
    }

    #[test]
    fn empty_plaintext_round_trips() {
        let key = key();
        let aad = aad_for(key.id());
        let envelope = seal(&key, aad, nonce(2), b"").unwrap();
        assert_eq!(open(&key, &envelope).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn wrong_key_never_decrypts() {
        let key = key();
        let aad = aad_for(key.id());
        let envelope = seal(&key, aad, nonce(3), b"secret").unwrap();
        let other_key = EnvelopeKey::new(key.id(), [1u8; 32]);
        assert_eq!(
            open(&other_key, &envelope),
            Err(EnvelopeError::AuthenticationFailed)
        );
    }

    #[test]
    fn key_id_mismatch_rejects_before_touching_ciphertext() {
        let key = key();
        let mismatched_aad = aad_for(RandomId::from_random_bytes([8u8; 16]).unwrap());
        assert_eq!(
            seal(&key, mismatched_aad, nonce(4), b"x"),
            Err(EnvelopeError::KeyIdMismatch)
        );
    }

    #[test]
    fn tampered_ciphertext_fails_closed() {
        let key = key();
        let aad = aad_for(key.id());
        let mut envelope = seal(&key, aad, nonce(5), b"synthetic").unwrap();
        envelope.ciphertext[0] ^= 0xFF;
        assert_eq!(
            open(&key, &envelope),
            Err(EnvelopeError::AuthenticationFailed)
        );
    }

    #[test]
    fn tampered_tag_fails_closed() {
        let key = key();
        let aad = aad_for(key.id());
        let mut envelope = seal(&key, aad, nonce(6), b"synthetic").unwrap();
        envelope.tag[0] ^= 0xFF;
        assert_eq!(
            open(&key, &envelope),
            Err(EnvelopeError::AuthenticationFailed)
        );
    }

    /// Every AAD field, mutated independently, must break authentication:
    /// the AAD is bound into the tag even though it is never encrypted.
    #[test]
    fn every_aad_field_mutation_breaks_authentication() {
        let key = key();
        let aad = aad_for(key.id());
        let envelope = seal(&key, aad, nonce(7), b"synthetic plaintext").unwrap();

        let mut record_id = envelope.clone();
        record_id.aad.record_id = RandomId::from_random_bytes([99u8; 16]).unwrap();
        assert_eq!(
            open(&key, &record_id),
            Err(EnvelopeError::AuthenticationFailed)
        );

        let mut account_binding = envelope.clone();
        account_binding.aad.account_binding_aad = RandomId::from_random_bytes([98u8; 16]).unwrap();
        assert_eq!(
            open(&key, &account_binding),
            Err(EnvelopeError::AuthenticationFailed)
        );

        let mut record_type = envelope.clone();
        record_type.aad.record_type = RecordType::AccountBinding;
        assert_eq!(
            open(&key, &record_type),
            Err(EnvelopeError::AuthenticationFailed)
        );

        let mut schema_version = envelope.clone();
        schema_version.aad.schema_version =
            SchemaVersion::new(core::num::NonZeroU32::new(2).unwrap());
        assert_eq!(
            open(&key, &schema_version),
            Err(EnvelopeError::AuthenticationFailed)
        );

        // key_id mutation is rejected before decryption is even attempted,
        // by the same KeyIdMismatch path exercised above; a mismatched but
        // still-valid-looking key_id inside the AAD (without changing which
        // key we call `open` with) still changes the authenticated bytes
        // and must fail the same way once a matching key is used.
        let other_key =
            EnvelopeKey::new(RandomId::from_random_bytes([97u8; 16]).unwrap(), [9u8; 32]);
        let mut key_id = envelope.clone();
        key_id.aad.key_id = other_key.id();
        assert_eq!(
            open(&other_key, &key_id),
            Err(EnvelopeError::AuthenticationFailed)
        );
    }

    #[test]
    fn nonce_mutation_breaks_authentication() {
        let key = key();
        let aad = aad_for(key.id());
        let mut envelope = seal(&key, aad, nonce(8), b"synthetic").unwrap();
        envelope.nonce[0] ^= 0xFF;
        assert_eq!(
            open(&key, &envelope),
            Err(EnvelopeError::AuthenticationFailed)
        );
    }

    #[test]
    fn distinct_nonces_produce_distinct_ciphertext_for_identical_plaintext() {
        let key = key();
        let aad = aad_for(key.id());
        let first = seal(&key, aad, nonce(10), b"same plaintext").unwrap();
        let second = seal(&key, aad, nonce(11), b"same plaintext").unwrap();
        assert_ne!(first.ciphertext, second.ciphertext);
        assert_ne!(first.tag, second.tag);
    }

    #[test]
    fn plaintext_over_maximum_is_rejected() {
        let key = key();
        let aad = aad_for(key.id());
        let oversized = vec![0u8; super::MAXIMUM_PLAINTEXT_BYTES + 1];
        assert_eq!(
            seal(&key, aad, nonce(12), &oversized),
            Err(EnvelopeError::PlaintextTooLarge)
        );
    }
}
