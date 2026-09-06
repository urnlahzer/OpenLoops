//! Opaque identifiers and closed catalogs shared by every other module.
//!
//! `contracts/persistence/protected-state-boundary.json` `envelope_suite`
//! fixes three independent 16-byte random identifier spaces (`key_id`,
//! `account_binding_aad`, `record_id`) and one closed 15-entry
//! `record_type_codes` catalog. None of these types expose a byte-inspection
//! or text-decoding API: `identifier_generation` requires "independent random
//! Windows-CSPRNG bytes with all-zero rejected; never a UUID containing time
//! host identity or Microsoft-derived input", so the only way to construct
//! one is [`RandomId::from_random_bytes`], which enforces the all-zero
//! rejection at the boundary.

/// A rejected [`RandomId::from_random_bytes`] input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RandomIdError {
    /// Every byte was zero, which `identifier_generation` forbids.
    AllZero,
}

/// A 16-byte random identifier drawn from the OS CSPRNG.
///
/// Backs `key_id`, `account_binding_aad`, and `record_id` uniformly; the
/// contract gives all three the same width, generation rule, and opacity, so
/// one newtype serves all three and call sites brand it with
/// [`KeyId`]/[`AccountBindingAad`]/[`RecordId`] type aliases below to avoid
/// accidental cross-purpose use at the type level.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct RandomId([u8; 16]);

impl RandomId {
    /// Wraps 16 already-generated random bytes.
    ///
    /// # Errors
    ///
    /// Returns [`RandomIdError::AllZero`] when every byte is zero.
    pub fn from_random_bytes(bytes: [u8; 16]) -> Result<Self, RandomIdError> {
        if bytes == [0u8; 16] {
            return Err(RandomIdError::AllZero);
        }
        Ok(Self(bytes))
    }

    /// Generates a fresh id from the OS CSPRNG via `getrandom`.
    ///
    /// # Errors
    ///
    /// Returns [`super::RngError`] when the OS CSPRNG call fails; per
    /// `envelope_suite.rng_failure` this must fail closed rather than retry
    /// with a weaker source, so the all-zero case (astronomically unlikely
    /// with a working CSPRNG) is treated as a fresh RNG failure and retried
    /// exactly once before failing closed.
    pub fn generate() -> Result<Self, crate::RngError> {
        for _ in 0..2 {
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).map_err(|_| crate::RngError::CsprngUnavailable)?;
            if let Ok(id) = Self::from_random_bytes(bytes) {
                return Ok(id);
            }
        }
        Err(crate::RngError::CsprngUnavailable)
    }

    /// Returns the raw 16 bytes for AAD/anchor framing only.
    ///
    /// This is intentionally not `Debug`/`Display`-friendly text: callers
    /// outside this crate's envelope/anchor framing should not need it.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl core::fmt::Debug for RandomId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("RandomId").field(&"<opaque-16>").finish()
    }
}

/// A random per-key identifier (`envelope_suite.identifier_layout.key_id_bytes`).
pub type KeyId = RandomId;
/// A random per-account binding value (`envelope_suite.identifier_layout.account_binding_aad_bytes`).
pub type AccountBindingAad = RandomId;
/// A random per-record identifier (`envelope_suite.identifier_layout.record_id_bytes`).
pub type RecordId = RandomId;

/// A nonzero, closed, u32 logical-schema version.
///
/// `logical_schema_boundary.field_contract.versions` requires "closed nonzero
/// integer catalogs; unknown future duplicate or zero versions reject".
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SchemaVersion(core::num::NonZeroU32);

impl SchemaVersion {
    /// Builds a schema version from a nonzero integer.
    #[must_use]
    pub const fn new(value: core::num::NonZeroU32) -> Self {
        Self(value)
    }

    /// Returns the raw integer value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// The one closed ciphertext version this crate implements.
///
/// `envelope_suite.ciphertext_version` and
/// `identifier_layout.ciphertext_version_encoding` fix this at exactly `1`.
pub const CIPHERTEXT_VERSION: u32 = 1;

/// The closed `record_type_codes` catalog (`envelope_suite.record_type_codes`).
///
/// Ordered and numbered exactly as the contract lists them; `#[repr(u16)]`
/// values equal `code` so `RecordType as u16` is the AAD/anchor encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(u16)]
pub enum RecordType {
    AccountBinding = 1,
    SyncCheckpoint = 2,
    MessageObservation = 3,
    EmailEvidenceRef = 4,
    CalendarInvitationRef = 5,
    UserAuthoredArtifactRef = 6,
    Loop = 7,
    DeadlineEvidence = 8,
    Transition = 9,
    ReminderLink = 10,
    OperationLedger = 11,
    JobHealth = 12,
    UserSetting = 13,
    ClientAssociation = 14,
    ProductCounter = 15,
}

/// A rejected [`RecordType::from_code`] input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownRecordTypeCode(pub u16);

impl RecordType {
    /// Every closed `record_type_codes` variant, in ascending code order.
    pub const ALL: [Self; 15] = [
        Self::AccountBinding,
        Self::SyncCheckpoint,
        Self::MessageObservation,
        Self::EmailEvidenceRef,
        Self::CalendarInvitationRef,
        Self::UserAuthoredArtifactRef,
        Self::Loop,
        Self::DeadlineEvidence,
        Self::Transition,
        Self::ReminderLink,
        Self::OperationLedger,
        Self::JobHealth,
        Self::UserSetting,
        Self::ClientAssociation,
        Self::ProductCounter,
    ];

    /// Returns the closed numeric code (identical to the `#[repr(u16)]` value).
    #[must_use]
    pub const fn code(self) -> u16 {
        self as u16
    }

    /// Returns the exact `record_type_codes[].id` text, used only as a SQL
    /// table identifier (never stored as row content).
    #[must_use]
    pub const fn table_name(self) -> &'static str {
        match self {
            Self::AccountBinding => "account_binding",
            Self::SyncCheckpoint => "sync_checkpoint",
            Self::MessageObservation => "message_observation",
            Self::EmailEvidenceRef => "email_evidence_ref",
            Self::CalendarInvitationRef => "calendar_invitation_ref",
            Self::UserAuthoredArtifactRef => "user_authored_artifact_ref",
            Self::Loop => "loop",
            Self::DeadlineEvidence => "deadline_evidence",
            Self::Transition => "transition",
            Self::ReminderLink => "reminder_link",
            Self::OperationLedger => "operation_ledger",
            Self::JobHealth => "job_health",
            Self::UserSetting => "user_setting",
            Self::ClientAssociation => "client_association",
            Self::ProductCounter => "product_counter",
        }
    }

    /// Decodes a closed `record_type_codes` value.
    ///
    /// # Errors
    ///
    /// Returns [`UnknownRecordTypeCode`] for any value outside `1..=15`, per
    /// `logical_schema_boundary.field_contract.versions` ("unknown future
    /// duplicate or zero versions reject").
    pub const fn from_code(code: u16) -> Result<Self, UnknownRecordTypeCode> {
        Ok(match code {
            1 => Self::AccountBinding,
            2 => Self::SyncCheckpoint,
            3 => Self::MessageObservation,
            4 => Self::EmailEvidenceRef,
            5 => Self::CalendarInvitationRef,
            6 => Self::UserAuthoredArtifactRef,
            7 => Self::Loop,
            8 => Self::DeadlineEvidence,
            9 => Self::Transition,
            10 => Self::ReminderLink,
            11 => Self::OperationLedger,
            12 => Self::JobHealth,
            13 => Self::UserSetting,
            14 => Self::ClientAssociation,
            15 => Self::ProductCounter,
            other => return Err(UnknownRecordTypeCode(other)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{RandomId, RandomIdError, RecordType, SchemaVersion, UnknownRecordTypeCode};

    #[test]
    fn random_id_rejects_all_zero() {
        assert_eq!(
            RandomId::from_random_bytes([0u8; 16]),
            Err(RandomIdError::AllZero)
        );
    }

    #[test]
    fn random_id_accepts_nonzero() {
        let mut bytes = [0u8; 16];
        bytes[15] = 1;
        assert!(RandomId::from_random_bytes(bytes).is_ok());
    }

    #[test]
    fn generate_produces_distinct_nonzero_ids() {
        let a = RandomId::generate().expect("csprng available in tests");
        let b = RandomId::generate().expect("csprng available in tests");
        assert_ne!(a.as_bytes(), b.as_bytes());
        assert_ne!(*a.as_bytes(), [0u8; 16]);
    }

    #[test]
    fn record_type_round_trips_every_closed_code() {
        for code in 1u16..=15 {
            let record_type = RecordType::from_code(code).expect("code is in closed catalog");
            assert_eq!(record_type.code(), code);
        }
    }

    #[test]
    fn record_type_rejects_zero_and_out_of_range() {
        assert_eq!(RecordType::from_code(0), Err(UnknownRecordTypeCode(0)));
        assert_eq!(RecordType::from_code(16), Err(UnknownRecordTypeCode(16)));
    }

    #[test]
    fn record_type_all_has_fifteen_unique_table_names() {
        let names: std::collections::BTreeSet<&str> =
            RecordType::ALL.iter().map(|r| r.table_name()).collect();
        assert_eq!(names.len(), 15);
        assert_eq!(RecordType::ALL.len(), 15);
    }

    #[test]
    fn schema_version_round_trips() {
        let version = SchemaVersion::new(core::num::NonZeroU32::new(3).expect("nonzero"));
        assert_eq!(version.get(), 3);
    }
}
