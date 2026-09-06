//! Opaque identifiers, bounded reference collections, and version counters.
//!
//! `contracts/domain/policy-state-boundary.json` `record_contracts` requires
//! every identifier field to be an opaque handle (never readable content) and
//! every reference collection to carry an exact upper bound
//! (`record_contracts[].collection_bounds`, `transition.source_ref_bound`).
//! This module gives those requirements a Rust type: opaque fixed-width byte
//! handles and small bounded containers that make an out-of-bound or
//! duplicate insertion a rejected `Result`, never a panic.

/// A random opaque 128-bit handle (`loop_id`, `account_ref`, `*_ref_id`,
/// `transition_id`, `user_action_id`, `deadline_evidence_id`, ...).
///
/// The bytes are never interpreted; ADR-008 forbids representing readable
/// content in any approved record, so this type intentionally exposes no
/// decoding, formatting-as-text, or byte-inspection API beyond equality and
/// ordering.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct OpaqueId(pub [u8; 16]);

impl OpaqueId {
    /// Builds an opaque id from its raw bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

/// A nonzero monotonically-assigned version counter.
///
/// `record_contracts[loop].version_rules` requires `schema`, `policy`, and
/// `loop` versions to be nonzero integers; `loop_version` increments exactly
/// once per committed transition. The same shape backs
/// `deadline_evidence`/`transition` version fields used throughout
/// `transition_policy`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Version(core::num::NonZeroU64);

impl Version {
    /// The first valid version.
    pub const FIRST: Self = Self(core::num::NonZeroU64::MIN);

    /// Builds a version from a nonzero integer.
    #[must_use]
    pub const fn new(value: core::num::NonZeroU64) -> Self {
        Self(value)
    }

    /// Returns the next version, or `None` on `u64` exhaustion.
    #[must_use]
    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }

    /// Returns the raw integer value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// A closed, bounded, duplicate-free code identifying a diagnostic or a
/// non-content model/schema label.
///
/// `loop.model_label_code` and diagnostic counters are "fixed non-content
/// codes" per `privacy_boundary.diagnostics`; this newtype carries a small
/// integer tag and never wraps a string.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ModelLabelCode(pub u16);

/// A validated timezone identifier.
///
/// `record_contracts[deadline_evidence].timezone_rule` requires "a validated
/// supported Windows or IANA identifier; normalized UTF-8; 1 through 128
/// bytes". This is the one contract-approved bounded identifier field that is
/// backed by text; the constructor enforces the byte bound so the type can
/// never carry unbounded or empty content. It is opaque to policy: this crate
/// does not validate that the text names a real timezone (that catalog is
/// owned by the settings/persistence layer), only that it is a bounded
/// nonempty identifier shape.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TimezoneId(String);

/// A rejected [`TimezoneId::new`] input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimezoneIdError {
    /// The identifier was empty.
    Empty,
    /// The identifier exceeded 128 UTF-8 bytes.
    TooLong,
}

impl TimezoneId {
    /// Validates and wraps a timezone identifier.
    ///
    /// # Errors
    ///
    /// Returns [`TimezoneIdError`] when the identifier is empty or exceeds
    /// 128 UTF-8 bytes, per `deadline_evidence.timezone_rule`.
    pub fn new(id: impl Into<String>) -> Result<Self, TimezoneIdError> {
        let id = id.into();
        if id.is_empty() {
            return Err(TimezoneIdError::Empty);
        }
        if id.len() > 128 {
            return Err(TimezoneIdError::TooLong);
        }
        Ok(Self(id))
    }

    /// Returns the validated identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Rejection reason for a bounded-collection insertion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundedInsertError {
    /// The collection is already at its contract-fixed capacity.
    CapacityExceeded,
    /// The value is already present in the collection.
    Duplicate,
}

/// A duplicate-free, order-preserving, capacity-bounded list of opaque ids.
///
/// Backs `source_ref_ids_by_role` (256), `reminder_ref_ids` (16),
/// `transition_ref_ids` (4096), and `transition.source_ref_ids` (32) from
/// `record_contracts`. Capacity is fixed at construction so every call site
/// states the exact contract bound it is enforcing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedIdList {
    capacity: usize,
    values: Vec<OpaqueId>,
}

impl BoundedIdList {
    /// Creates an empty list with the given exact contract capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            values: Vec::new(),
        }
    }

    /// Appends `id` if capacity remains and `id` is not already present.
    ///
    /// # Errors
    ///
    /// Returns [`BoundedInsertError`] on capacity overflow or duplicate.
    pub fn push(&mut self, id: OpaqueId) -> Result<(), BoundedInsertError> {
        if self.values.contains(&id) {
            return Err(BoundedInsertError::Duplicate);
        }
        if self.values.len() >= self.capacity {
            return Err(BoundedInsertError::CapacityExceeded);
        }
        self.values.push(id);
        Ok(())
    }

    /// Returns the current number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns whether the list holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Returns the fixed contract capacity.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Iterates the entries in insertion order.
    pub fn iter(&self) -> core::slice::Iter<'_, OpaqueId> {
        self.values.iter()
    }
}

/// An opaque role tag for one `source_ref_ids_by_role` entry.
///
/// The role catalog itself (`task`/`ownership`/`waiting_party`/`deadline`/...)
/// is owned by ADR-006's evidence contract, not ADR-008; this crate bounds
/// only the `loop` record's shape and count, so a role is an opaque
/// comparable code rather than a fabricated enum this crate does not own.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SourceRoleCode(pub u8);

/// A duplicate-free, capacity-bounded list of `(role, id)` pairs.
///
/// Backs `loop.source_ref_ids_by_role` (256). `record_contracts[loop].ordering`
/// states "closed enum order for sets; semantic role then opaque reference
/// bytes for references"; duplicate detection compares the full `(role, id)`
/// pair, since the same evidence id may legitimately appear under more than
/// one role.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRoleRefList {
    capacity: usize,
    values: Vec<(SourceRoleCode, OpaqueId)>,
}

impl SourceRoleRefList {
    /// Creates an empty list with the given exact contract capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            values: Vec::new(),
        }
    }

    /// Appends `(role, id)` if capacity remains and the pair is not already
    /// present.
    ///
    /// # Errors
    ///
    /// Returns [`BoundedInsertError`] on capacity overflow or duplicate.
    pub fn push(&mut self, role: SourceRoleCode, id: OpaqueId) -> Result<(), BoundedInsertError> {
        if self.values.contains(&(role, id)) {
            return Err(BoundedInsertError::Duplicate);
        }
        if self.values.len() >= self.capacity {
            return Err(BoundedInsertError::CapacityExceeded);
        }
        self.values.push((role, id));
        Ok(())
    }

    /// Returns the current number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns whether the list holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Returns the fixed contract capacity.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Iterates the entries in insertion order.
    pub fn iter(&self) -> core::slice::Iter<'_, (SourceRoleCode, OpaqueId)> {
        self.values.iter()
    }
}

impl<'a> IntoIterator for &'a SourceRoleRefList {
    type Item = &'a (SourceRoleCode, OpaqueId);
    type IntoIter = core::slice::Iter<'a, (SourceRoleCode, OpaqueId)>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

impl<'a> IntoIterator for &'a BoundedIdList {
    type Item = &'a OpaqueId;
    type IntoIter = core::slice::Iter<'a, OpaqueId>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BoundedIdList, BoundedInsertError, OpaqueId, TimezoneId, TimezoneIdError, Version,
    };

    fn id(byte: u8) -> OpaqueId {
        OpaqueId::from_bytes([byte; 16])
    }

    #[test]
    fn version_increments_by_exactly_one() {
        let first = Version::FIRST;
        let second = first.next().expect("nonzero u64 has headroom");
        assert_eq!(first.get() + 1, second.get());
    }

    #[test]
    fn version_at_maximum_refuses_to_overflow() {
        let max = Version::new(core::num::NonZeroU64::new(u64::MAX).expect("u64::MAX is nonzero"));
        assert!(max.next().is_none());
    }

    #[test]
    fn bounded_id_list_rejects_capacity_overflow() {
        let mut list = BoundedIdList::with_capacity(1);
        assert_eq!(list.push(id(1)), Ok(()));
        assert_eq!(list.push(id(2)), Err(BoundedInsertError::CapacityExceeded));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn bounded_id_list_rejects_duplicates_without_consuming_capacity() {
        let mut list = BoundedIdList::with_capacity(4);
        assert_eq!(list.push(id(1)), Ok(()));
        assert_eq!(list.push(id(1)), Err(BoundedInsertError::Duplicate));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn timezone_id_rejects_empty_and_oversized() {
        assert_eq!(TimezoneId::new(""), Err(TimezoneIdError::Empty));
        let oversized = "x".repeat(129);
        assert_eq!(TimezoneId::new(oversized), Err(TimezoneIdError::TooLong));
        assert!(TimezoneId::new("America/New_York").is_ok());
    }
}
