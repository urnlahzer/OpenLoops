//! HMAC-SHA-256 purpose-separated digest framing.
//!
//! `digest_suite.input_encoding`: "17 fixed ASCII bytes openloops-hmac-v1,
//! unsigned 16-bit big-endian purpose-tag byte length, exact ASCII purpose
//! tag, 16-byte account binding, unsigned 32-bit big-endian nonzero
//! owning-ADR schema version, unsigned 16-bit big-endian component count,
//! then each owning-ADR-ordered component as unsigned 32-bit big-endian byte
//! length plus exact bytes; no alternate normalization encoding or field
//! order". This module owns exactly that generic framing engine and the
//! closed 23-entry purpose-tag catalog; it deliberately does not assemble
//! the domain-specific component lists themselves (e.g. what bytes make up
//! `content_digest_hmac`), because `purpose_catalog_status` reserves that to
//! each owning ADR. Sixteen of the twenty-three purposes remain
//! `unavailable_pending_owner_ADR` and this module refuses to compute a
//! digest for them.
//!
//! `openloops-application`'s IMPL-07 story explicitly did **not** close
//! [`PurposeTag::OperationLedgerOperationKey`], even though ADR-009's
//! `contracts/reminder/adapter-boundary.json` `operation_protocol.intent_hmac_input_order`
//! fully specifies that purpose's 18 ordered components. This crate's own
//! binding contract, `contracts/persistence/protected-state-boundary.json`
//! `digest_suite.purpose_catalog`, records `operation_ledger.operation_key_hmac`'s
//! `owner` as **"ADR-009 and ADR-011"** — two named owners, not one — and its
//! `purpose_catalog_status` closes a row only "until every named owner
//! closes them." ADR-011 (`docs/adr/ADR-011-automation-and-evaluation.md`)
//! does not address the operation-key layout at all, so the second owner has
//! not closed its half; the row must stay `unavailable_pending_owner_ADR`
//! here, and [`compute`]/[`frame`] must keep refusing it, regardless of how
//! completely ADR-009 alone specifies the framing. `openloops_application::ledger`
//! documents this as an explicit, tested honest gap: it implements the full
//! ADR-009 operation-ledger state machine over an opaque, externally supplied
//! key, and its own `operation_key_hmac` helper (the ADR-009 intent-to-key
//! framing) is tested only to confirm it fails closed with
//! [`DigestError::LayoutNotYetOwned`] while this purpose remains unavailable.
//! The two sibling `operation_ledger` purposes (`expected_remote_version_hmac`,
//! `remote_correlation_hmac`) are unrelated to the operation key and stay
//! `unavailable_pending_owner_ADR` for their own reason: no adapter gate has
//! proven a remote marker/version contract yet.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::ids::{AccountBindingAad, SchemaVersion};

type HmacSha256 = Hmac<Sha256>;

/// `digest_suite.input_encoding`: 17 fixed ASCII bytes.
const DOMAIN: &[u8; 17] = b"openloops-hmac-v1";
/// `digest_suite.output_bytes`.
pub const OUTPUT_BYTES: usize = 32;
/// `digest_suite.key_bytes`.
pub const KEY_BYTES: usize = 32;

/// A zeroize-on-drop, purpose-separated HMAC-SHA-256 content-digest key.
///
/// `secret_inventory[state_hmac_keyring]` / `digest_suite.purpose_separation`:
/// "one distinct random content-digest HMAC key per account ... never reuse
/// the AEAD rollback-anchor provider pairing token or session key". This
/// type is distinct at the type level from [`crate::envelope::EnvelopeKey`]
/// and [`crate::anchor::AnchorKey`] so a call site cannot pass the wrong key
/// to the wrong primitive without a compile error.
pub struct ContentDigestKey(zeroize::Zeroizing<[u8; KEY_BYTES]>);

impl ContentDigestKey {
    /// Wraps raw key bytes.
    #[must_use]
    pub fn new(bytes: [u8; KEY_BYTES]) -> Self {
        Self(zeroize::Zeroizing::new(bytes))
    }
}

/// The closed 23-entry `digest_suite.purpose_catalog`.
///
/// Variant order and tag strings are copied verbatim from the contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PurposeTag {
    SyncCheckpointQueryFingerprint,
    MessageObservationSourceVersion,
    MessageObservationObservedVersion,
    EmailEvidenceRefMessageLocatorMailboxBinding,
    EmailEvidenceRefContentDigest,
    EmailEvidenceRefPrefixDigest,
    EmailEvidenceRefSuffixDigest,
    CalendarInvitationRefEventVersion,
    UserAuthoredArtifactRefTitleDigest,
    UserAuthoredArtifactRefBodyDigest,
    UserAuthoredArtifactRefDueDigest,
    UserAuthoredArtifactRefTitleVersion,
    UserAuthoredArtifactRefBodyVersion,
    UserAuthoredArtifactRefDueVersion,
    ReminderLinkTitleDigest,
    ReminderLinkBodyDigest,
    ReminderLinkDueDigest,
    ReminderLinkArtifactVersion,
    ReminderLinkObservedRemoteVersion,
    ReminderLinkRemoteCorrelation,
    OperationLedgerOperationKey,
    OperationLedgerExpectedRemoteVersion,
    OperationLedgerRemoteCorrelation,
}

impl PurposeTag {
    /// The exact ASCII purpose-tag text from `digest_suite.purpose_catalog[].tag`.
    #[must_use]
    pub const fn tag_str(self) -> &'static str {
        match self {
            Self::SyncCheckpointQueryFingerprint => {
                "openloops-hmac-v1/sync_checkpoint/query_fingerprint"
            }
            Self::MessageObservationSourceVersion => {
                "openloops-hmac-v1/message_observation/source_version"
            }
            Self::MessageObservationObservedVersion => {
                "openloops-hmac-v1/message_observation/observed_version"
            }
            Self::EmailEvidenceRefMessageLocatorMailboxBinding => {
                "openloops-hmac-v1/email_evidence_ref.message_locator/mailbox_binding"
            }
            Self::EmailEvidenceRefContentDigest => {
                "openloops-hmac-v1/email_evidence_ref/content_digest"
            }
            Self::EmailEvidenceRefPrefixDigest => {
                "openloops-hmac-v1/email_evidence_ref/prefix_digest"
            }
            Self::EmailEvidenceRefSuffixDigest => {
                "openloops-hmac-v1/email_evidence_ref/suffix_digest"
            }
            Self::CalendarInvitationRefEventVersion => {
                "openloops-hmac-v1/calendar_invitation_ref/event_version"
            }
            Self::UserAuthoredArtifactRefTitleDigest => {
                "openloops-hmac-v1/user_authored_artifact_ref/title_digest"
            }
            Self::UserAuthoredArtifactRefBodyDigest => {
                "openloops-hmac-v1/user_authored_artifact_ref/body_digest"
            }
            Self::UserAuthoredArtifactRefDueDigest => {
                "openloops-hmac-v1/user_authored_artifact_ref/due_digest"
            }
            Self::UserAuthoredArtifactRefTitleVersion => {
                "openloops-hmac-v1/user_authored_artifact_ref/title_version"
            }
            Self::UserAuthoredArtifactRefBodyVersion => {
                "openloops-hmac-v1/user_authored_artifact_ref/body_version"
            }
            Self::UserAuthoredArtifactRefDueVersion => {
                "openloops-hmac-v1/user_authored_artifact_ref/due_version"
            }
            Self::ReminderLinkTitleDigest => "openloops-hmac-v1/reminder_link/title_digest",
            Self::ReminderLinkBodyDigest => "openloops-hmac-v1/reminder_link/body_digest",
            Self::ReminderLinkDueDigest => "openloops-hmac-v1/reminder_link/due_digest",
            Self::ReminderLinkArtifactVersion => "openloops-hmac-v1/reminder_link/artifact_version",
            Self::ReminderLinkObservedRemoteVersion => {
                "openloops-hmac-v1/reminder_link/observed_remote_version"
            }
            Self::ReminderLinkRemoteCorrelation => {
                "openloops-hmac-v1/reminder_link/remote_correlation"
            }
            Self::OperationLedgerOperationKey => "openloops-hmac-v1/operation_ledger/operation_key",
            Self::OperationLedgerExpectedRemoteVersion => {
                "openloops-hmac-v1/operation_ledger/expected_remote_version"
            }
            Self::OperationLedgerRemoteCorrelation => {
                "openloops-hmac-v1/operation_ledger/remote_correlation"
            }
        }
    }

    /// Whether `digest_suite.purpose_catalog[].input_layout` is closed
    /// (an owning-ADR reference) rather than `unavailable_pending_owner_ADR`.
    ///
    /// Exactly the first seven catalog rows (ADR-006) are closed today.
    /// [`Self::OperationLedgerOperationKey`] is *not* included even though
    /// ADR-009 alone fully specifies its layout: the contract's `owner` for
    /// that row is "ADR-009 and ADR-011" and ADR-011 has not closed its half
    /// (see this module's doc comment).
    #[must_use]
    pub const fn is_layout_closed(self) -> bool {
        matches!(
            self,
            Self::SyncCheckpointQueryFingerprint
                | Self::MessageObservationSourceVersion
                | Self::MessageObservationObservedVersion
                | Self::EmailEvidenceRefMessageLocatorMailboxBinding
                | Self::EmailEvidenceRefContentDigest
                | Self::EmailEvidenceRefPrefixDigest
                | Self::EmailEvidenceRefSuffixDigest
        )
    }
}

/// A rejected digest framing, computation, or verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DigestError {
    /// `purpose.is_layout_closed()` was false; the owning ADR has not yet
    /// closed this purpose's component layout, so this crate refuses to
    /// compute or verify it rather than guess at a layout.
    LayoutNotYetOwned,
    /// More than 65535 components were supplied (`component count` is u16).
    TooManyComponents,
    /// One component exceeded `u32::MAX` bytes.
    ComponentTooLarge,
    /// Verification failed (constant-time comparison did not match).
    Mismatch,
}

/// Builds the exact framed byte string for one digest input.
///
/// # Errors
///
/// Returns [`DigestError::LayoutNotYetOwned`],
/// [`DigestError::TooManyComponents`], or [`DigestError::ComponentTooLarge`].
///
/// # Panics
///
/// Never in practice: every [`PurposeTag::tag_str`] value is a short
/// hard-coded constant far under `u16::MAX` bytes.
pub fn frame(
    purpose: PurposeTag,
    account_binding: AccountBindingAad,
    schema_version: SchemaVersion,
    components: &[&[u8]],
) -> Result<Vec<u8>, DigestError> {
    if !purpose.is_layout_closed() {
        return Err(DigestError::LayoutNotYetOwned);
    }
    let component_count: u16 =
        u16::try_from(components.len()).map_err(|_| DigestError::TooManyComponents)?;
    let tag_bytes = purpose.tag_str().as_bytes();
    let tag_len: u16 = u16::try_from(tag_bytes.len()).expect("purpose tags are short constants");

    let mut framed = Vec::with_capacity(
        DOMAIN.len()
            + 2
            + tag_bytes.len()
            + 16
            + 4
            + 2
            + components.iter().map(|c| 4 + c.len()).sum::<usize>(),
    );
    framed.extend_from_slice(DOMAIN);
    framed.extend_from_slice(&tag_len.to_be_bytes());
    framed.extend_from_slice(tag_bytes);
    framed.extend_from_slice(account_binding.as_bytes());
    framed.extend_from_slice(&schema_version.get().to_be_bytes());
    framed.extend_from_slice(&component_count.to_be_bytes());
    for component in components {
        let len: u32 =
            u32::try_from(component.len()).map_err(|_| DigestError::ComponentTooLarge)?;
        framed.extend_from_slice(&len.to_be_bytes());
        framed.extend_from_slice(component);
    }
    Ok(framed)
}

/// Computes the full 32-byte HMAC-SHA-256 output over [`frame`]'s framing.
///
/// # Errors
///
/// See [`frame`].
///
/// # Panics
///
/// Never in practice: `HmacSha256::new_from_slice` only rejects a key
/// length that does not fit its key size, and [`ContentDigestKey`] is always
/// exactly [`KEY_BYTES`] long.
pub fn compute(
    key: &ContentDigestKey,
    purpose: PurposeTag,
    account_binding: AccountBindingAad,
    schema_version: SchemaVersion,
    components: &[&[u8]],
) -> Result<[u8; OUTPUT_BYTES], DigestError> {
    let framed = frame(purpose, account_binding, schema_version, components)?;
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(key.0.as_slice())
        .expect("HMAC accepts any key length");
    Mac::update(&mut mac, &framed);
    let mut out = [0u8; OUTPUT_BYTES];
    out.copy_from_slice(&Mac::finalize(mac).into_bytes());
    Ok(out)
}

/// Verifies `expected` against the digest recomputed from `components`.
///
/// Uses [`Mac::verify_slice`], which compares the full output length in
/// constant time via `subtle`-style `ct_eq` and never returns early on a
/// byte-by-byte mismatch of secret-derived data (only the public,
/// fixed-width length is checked with an ordinary branch first).
///
/// # Errors
///
/// Returns [`DigestError::Mismatch`] on any verification failure, or the
/// [`frame`] errors.
///
/// # Panics
///
/// Never in practice; see [`compute`]'s `# Panics` section.
pub fn verify(
    key: &ContentDigestKey,
    purpose: PurposeTag,
    account_binding: AccountBindingAad,
    schema_version: SchemaVersion,
    components: &[&[u8]],
    expected: &[u8; OUTPUT_BYTES],
) -> Result<(), DigestError> {
    let framed = frame(purpose, account_binding, schema_version, components)?;
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(key.0.as_slice())
        .expect("HMAC accepts any key length");
    Mac::update(&mut mac, &framed);
    Mac::verify_slice(mac, expected).map_err(|_| DigestError::Mismatch)
}

#[cfg(test)]
mod tests {
    use super::{ContentDigestKey, DigestError, PurposeTag, compute, frame, verify};
    use crate::ids::{RandomId, SchemaVersion};

    fn account() -> crate::ids::AccountBindingAad {
        RandomId::from_random_bytes([5u8; 16]).unwrap()
    }

    fn schema() -> SchemaVersion {
        SchemaVersion::new(core::num::NonZeroU32::new(1).unwrap())
    }

    #[test]
    fn frame_rejects_purpose_without_closed_layout() {
        assert_eq!(
            frame(
                PurposeTag::ReminderLinkTitleDigest,
                account(),
                schema(),
                &[]
            ),
            Err(DigestError::LayoutNotYetOwned)
        );
    }

    #[test]
    fn purpose_catalog_has_exactly_seven_closed_layouts() {
        let closed = [
            PurposeTag::SyncCheckpointQueryFingerprint,
            PurposeTag::MessageObservationSourceVersion,
            PurposeTag::MessageObservationObservedVersion,
            PurposeTag::EmailEvidenceRefMessageLocatorMailboxBinding,
            PurposeTag::EmailEvidenceRefContentDigest,
            PurposeTag::EmailEvidenceRefPrefixDigest,
            PurposeTag::EmailEvidenceRefSuffixDigest,
        ];
        assert!(closed.iter().all(|p| p.is_layout_closed()));

        let unavailable = [
            PurposeTag::CalendarInvitationRefEventVersion,
            PurposeTag::UserAuthoredArtifactRefTitleDigest,
            PurposeTag::UserAuthoredArtifactRefBodyDigest,
            PurposeTag::UserAuthoredArtifactRefDueDigest,
            PurposeTag::UserAuthoredArtifactRefTitleVersion,
            PurposeTag::UserAuthoredArtifactRefBodyVersion,
            PurposeTag::UserAuthoredArtifactRefDueVersion,
            PurposeTag::ReminderLinkTitleDigest,
            PurposeTag::ReminderLinkBodyDigest,
            PurposeTag::ReminderLinkDueDigest,
            PurposeTag::ReminderLinkArtifactVersion,
            PurposeTag::ReminderLinkObservedRemoteVersion,
            PurposeTag::ReminderLinkRemoteCorrelation,
            PurposeTag::OperationLedgerOperationKey,
            PurposeTag::OperationLedgerExpectedRemoteVersion,
            PurposeTag::OperationLedgerRemoteCorrelation,
        ];
        assert_eq!(unavailable.len(), 16);
        assert!(unavailable.iter().all(|p| !p.is_layout_closed()));
    }

    #[test]
    fn operation_ledger_operation_key_refuses_to_frame_pending_the_second_owner() {
        // `contracts/persistence/protected-state-boundary.json`
        // `digest_suite.purpose_catalog`'s `operation_ledger.operation_key_hmac`
        // row names owner "ADR-009 and ADR-011"; ADR-011 has not closed its
        // half, so this purpose must stay refused even though ADR-009 alone
        // fully specifies `intent_hmac_input_order`.
        assert_eq!(
            frame(
                PurposeTag::OperationLedgerOperationKey,
                account(),
                schema(),
                &[]
            ),
            Err(DigestError::LayoutNotYetOwned)
        );
    }

    #[test]
    fn frame_matches_exact_byte_layout_for_two_components() {
        let account = RandomId::from_random_bytes([0x11u8; 16]).unwrap();
        let schema = SchemaVersion::new(core::num::NonZeroU32::new(7).unwrap());
        let framed = frame(
            PurposeTag::MessageObservationSourceVersion,
            account,
            schema,
            &[b"abc", b"de"],
        )
        .unwrap();

        let mut expected = Vec::new();
        expected.extend_from_slice(b"openloops-hmac-v1");
        let tag = PurposeTag::MessageObservationSourceVersion
            .tag_str()
            .as_bytes();
        expected.extend_from_slice(&u16::try_from(tag.len()).unwrap().to_be_bytes());
        expected.extend_from_slice(tag);
        expected.extend_from_slice(&[0x11u8; 16]);
        expected.extend_from_slice(&7u32.to_be_bytes());
        expected.extend_from_slice(&2u16.to_be_bytes());
        expected.extend_from_slice(&3u32.to_be_bytes());
        expected.extend_from_slice(b"abc");
        expected.extend_from_slice(&2u32.to_be_bytes());
        expected.extend_from_slice(b"de");
        assert_eq!(framed, expected);
    }

    #[test]
    fn compute_and_verify_round_trip() {
        let key = ContentDigestKey::new([3u8; 32]);
        let digest = compute(
            &key,
            PurposeTag::EmailEvidenceRefContentDigest,
            account(),
            schema(),
            &[b"one", b"two", b"three"],
        )
        .unwrap();
        assert!(
            verify(
                &key,
                PurposeTag::EmailEvidenceRefContentDigest,
                account(),
                schema(),
                &[b"one", b"two", b"three"],
                &digest,
            )
            .is_ok()
        );
    }

    #[test]
    fn verify_rejects_any_component_change() {
        let key = ContentDigestKey::new([3u8; 32]);
        let digest = compute(
            &key,
            PurposeTag::EmailEvidenceRefPrefixDigest,
            account(),
            schema(),
            &[b"one", b"two"],
        )
        .unwrap();
        assert_eq!(
            verify(
                &key,
                PurposeTag::EmailEvidenceRefPrefixDigest,
                account(),
                schema(),
                &[b"one", b"TWO"],
                &digest,
            ),
            Err(DigestError::Mismatch)
        );
    }

    #[test]
    fn verify_rejects_wrong_purpose_tag() {
        let key = ContentDigestKey::new([3u8; 32]);
        let digest = compute(
            &key,
            PurposeTag::EmailEvidenceRefPrefixDigest,
            account(),
            schema(),
            &[b"one"],
        )
        .unwrap();
        assert_eq!(
            verify(
                &key,
                PurposeTag::EmailEvidenceRefSuffixDigest,
                account(),
                schema(),
                &[b"one"],
                &digest,
            ),
            Err(DigestError::Mismatch)
        );
    }

    #[test]
    fn verify_rejects_wrong_key() {
        let key = ContentDigestKey::new([3u8; 32]);
        let other_key = ContentDigestKey::new([4u8; 32]);
        let digest = compute(
            &key,
            PurposeTag::SyncCheckpointQueryFingerprint,
            account(),
            schema(),
            &[b"one"],
        )
        .unwrap();
        assert_eq!(
            verify(
                &other_key,
                PurposeTag::SyncCheckpointQueryFingerprint,
                account(),
                schema(),
                &[b"one"],
                &digest,
            ),
            Err(DigestError::Mismatch)
        );
    }

    /// Pinned known-answer vector: HMAC-SHA-256 over the exact framed bytes
    /// for a fixed key/account/schema/component input, computed once with an
    /// independent Python `hmac`/`hashlib` oracle and pinned here so any
    /// future change to the framing or the HMAC wiring is caught.
    #[test]
    fn known_answer_vector_is_stable() {
        let key = ContentDigestKey::new([0u8; 32]);
        let mut account_bytes = [0u8; 16];
        account_bytes[15] = 1;
        let account = RandomId::from_random_bytes(account_bytes).unwrap();
        let schema = SchemaVersion::new(core::num::NonZeroU32::new(1).unwrap());
        let digest = compute(
            &key,
            PurposeTag::SyncCheckpointQueryFingerprint,
            account,
            schema,
            &[b"vector"],
        )
        .unwrap();
        assert_eq!(
            hex(&digest),
            "448802596c86ae0c3282b97dedc4f5afaa0396405010e1a31c166e5a6c5e6be8"
        );
    }

    fn hex(bytes: &[u8]) -> String {
        use core::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
    }
}
