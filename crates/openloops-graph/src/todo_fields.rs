//! `ownership_matrix`, `direct_edit_and_conflict`, and
//! `reminder_time_boundary`: the service-projection/user-owned field
//! ownership and conflict engine.
//!
//! * [`initial_ownership`] and [`apply_direct_edit`] implement the exact
//!   nine-row `ownership_matrix` as pure functions over
//!   ([`ReminderOrigin`], [`FieldKind`]) rather than a lookup table a future
//!   edit could silently drift out of sync with the contract.
//! * [`service_update_decision`] answers, for one field's *current*
//!   ownership, whether a service-originated update may proceed outright,
//!   is flatly prohibited, or needs the due-field-specific keep-or-update
//!   choice (`reminder_time_boundary`/`ownership_matrix`'s
//!   `due`/`service_update` column).
//! * [`classify_owned_field_versions`] and [`service_owned_field_mask`] implement
//!   `direct_edit_and_conflict`: re-fetch-and-compare before any update,
//!   stale/changed anywhere stops at a visible conflict with zero PATCH, and
//!   only the exact service-owned fields ever enter the patch mask.

/// The three closed `ownership_matrix.origin` values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ReminderOrigin {
    EmailEvidence,
    CalendarInvitation,
    UserAuthoredMicrosoftArtifact,
}

/// The three fields `ownership_matrix` governs (`reminder_time` is
/// deliberately absent — see `reminder_time_boundary`: "no reminder-time
/// value ownership digest or override field is approved").
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum FieldKind {
    Title,
    Body,
    Due,
}

/// `catalogs.ownership_code`, in exact catalog order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnershipCode {
    ServiceProjection,
    UserOwned,
    UserAuthoritative,
    NotApplicable,
}

/// `ownership_matrix[].initial`: the field's ownership the moment a loop
/// (or manual artifact reference) is created from `origin`.
#[must_use]
pub const fn initial_ownership(origin: ReminderOrigin) -> OwnershipCode {
    match origin {
        ReminderOrigin::EmailEvidence | ReminderOrigin::CalendarInvitation => {
            OwnershipCode::ServiceProjection
        }
        ReminderOrigin::UserAuthoredMicrosoftArtifact => OwnershipCode::UserAuthoritative,
    }
}

/// The result of a user directly editing one field on the remote artifact
/// (`ownership_matrix[].direct_edit`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectEditOutcome {
    /// title/body on a detected (email/calendar) origin: becomes
    /// `user_owned`; every later service update is prohibited outright.
    BecomesUserOwned,
    /// due on a detected origin: creates `reminder_due_override`
    /// (`ownership_matrix`: `"direct_edit":"user_owned reminder_due_override"`);
    /// a later service update requires the explicit keep-override/
    /// update-reminder choice rather than a flat prohibition.
    CreatesDueOverride,
    /// `user_authored_microsoft_artifact` fields start (and remain)
    /// `user_authoritative`; a "direct edit" is a no-op transition since
    /// there is no more-owned state to move to and service updates are
    /// already unconditionally prohibited.
    AlreadyUserAuthoritative,
}

/// `ownership_matrix[].direct_edit`, applied to `origin`/`field`.
#[must_use]
pub const fn apply_direct_edit(origin: ReminderOrigin, field: FieldKind) -> DirectEditOutcome {
    match origin {
        ReminderOrigin::UserAuthoredMicrosoftArtifact => {
            DirectEditOutcome::AlreadyUserAuthoritative
        }
        ReminderOrigin::EmailEvidence | ReminderOrigin::CalendarInvitation => match field {
            FieldKind::Due => DirectEditOutcome::CreatesDueOverride,
            FieldKind::Title | FieldKind::Body => DirectEditOutcome::BecomesUserOwned,
        },
    }
}

/// Whether a service-originated update to `field`, currently in ownership
/// state `ownership`, may proceed (`ownership_matrix[].service_update`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceUpdateDecision {
    /// `"service_update":"prohibited after direct edit"`'s complement: no
    /// direct edit has happened yet (still `service_projection`).
    Permitted,
    /// title/body once `user_owned`, or any `user_authoritative` field:
    /// `"prohibited"`.
    Prohibited,
    /// due once `user_owned` (a `reminder_due_override` exists):
    /// `"requires keep-override or update-reminder choice"`.
    RequiresKeepOrUpdateChoice,
}

/// `ownership_matrix[].service_update`, keyed on the field's *current*
/// ownership rather than its origin (the origin only decides the *initial*
/// state via [`initial_ownership`]; from then on ownership alone drives this
/// decision, exactly as the contract's per-field rows do).
#[must_use]
pub const fn service_update_decision(
    field: FieldKind,
    ownership: OwnershipCode,
) -> ServiceUpdateDecision {
    match ownership {
        OwnershipCode::ServiceProjection => ServiceUpdateDecision::Permitted,
        OwnershipCode::UserOwned => match field {
            FieldKind::Due => ServiceUpdateDecision::RequiresKeepOrUpdateChoice,
            FieldKind::Title | FieldKind::Body => ServiceUpdateDecision::Prohibited,
        },
        OwnershipCode::UserAuthoritative | OwnershipCode::NotApplicable => {
            ServiceUpdateDecision::Prohibited
        }
    }
}

/// `record_contracts.field_contracts` `version_state_code` catalog, used
/// here for `direct_edit_and_conflict.enumeration`'s post-refetch compare of
/// one owned field's digest and version against the last-recorded value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionState {
    Current,
    Changed,
    Stale,
    Unavailable,
    Conflict,
}

/// `direct_edit_and_conflict`'s TOCTOU decision: re-fetch every owned
/// field's [`VersionState`] immediately before a would-be update and decide
/// whether the bounded, field-masked PATCH may proceed at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConflictDecision {
    /// Every re-fetched owned field is [`VersionState::Current`]: the
    /// caller may proceed with exactly its service-owned field mask.
    ProceedWithFieldMask,
    /// Any re-fetched owned field is not [`VersionState::Current`]:
    /// `direct_edit_and_conflict.stale_or_changed`: "stop at reminder
    /// conflict; no PATCH until adapter-specific conditional-write behavior
    /// is proven." Zero fields are patched.
    VisibleConflictNoPatch,
}

/// `direct_edit_and_conflict.stale_or_changed`: classifies a set of
/// just-re-fetched owned-field version states. Any single non-`Current`
/// entry — including one that changed in the window between an earlier
/// refetch and this exact call — forces
/// [`ConflictDecision::VisibleConflictNoPatch`] with zero PATCH, closing the
/// classic refetch-then-patch TOCTOU window: this function must be the last
/// thing called immediately before the PATCH is issued, over freshly
/// re-fetched states, never over a stale cached comparison.
#[must_use]
pub fn classify_owned_field_versions(owned_field_states: &[VersionState]) -> ConflictDecision {
    if owned_field_states
        .iter()
        .all(|state| matches!(state, VersionState::Current))
    {
        ConflictDecision::ProceedWithFieldMask
    } else {
        ConflictDecision::VisibleConflictNoPatch
    }
}

/// `direct_edit_and_conflict.partial_update`: "patch only the exact
/// service-owned field mask; preserve every user-owned sibling field."
/// Filters `fields` down to exactly the ones currently `service_projection`
/// — a user-owned or user-authoritative sibling can never appear in the
/// returned mask, structurally.
#[must_use]
pub fn service_owned_field_mask(fields: &[(FieldKind, OwnershipCode)]) -> Vec<FieldKind> {
    fields
        .iter()
        .filter(|(_, ownership)| matches!(ownership, OwnershipCode::ServiceProjection))
        .map(|(field, _)| *field)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        ConflictDecision, DirectEditOutcome, FieldKind, OwnershipCode, ReminderOrigin,
        ServiceUpdateDecision, VersionState, apply_direct_edit, classify_owned_field_versions,
        initial_ownership, service_owned_field_mask, service_update_decision,
    };

    #[test]
    fn detected_origins_start_service_projected_and_manual_artifacts_start_authoritative() {
        assert_eq!(
            initial_ownership(ReminderOrigin::EmailEvidence),
            OwnershipCode::ServiceProjection
        );
        assert_eq!(
            initial_ownership(ReminderOrigin::CalendarInvitation),
            OwnershipCode::ServiceProjection
        );
        assert_eq!(
            initial_ownership(ReminderOrigin::UserAuthoredMicrosoftArtifact),
            OwnershipCode::UserAuthoritative
        );
    }

    #[test]
    fn direct_edit_on_title_or_body_becomes_user_owned_for_detected_origins() {
        for origin in [
            ReminderOrigin::EmailEvidence,
            ReminderOrigin::CalendarInvitation,
        ] {
            for field in [FieldKind::Title, FieldKind::Body] {
                assert_eq!(
                    apply_direct_edit(origin, field),
                    DirectEditOutcome::BecomesUserOwned
                );
            }
        }
    }

    #[test]
    fn direct_edit_on_due_creates_an_override_for_detected_origins() {
        for origin in [
            ReminderOrigin::EmailEvidence,
            ReminderOrigin::CalendarInvitation,
        ] {
            assert_eq!(
                apply_direct_edit(origin, FieldKind::Due),
                DirectEditOutcome::CreatesDueOverride
            );
        }
    }

    #[test]
    fn manual_artifact_fields_are_already_user_authoritative() {
        for field in [FieldKind::Title, FieldKind::Body, FieldKind::Due] {
            assert_eq!(
                apply_direct_edit(ReminderOrigin::UserAuthoredMicrosoftArtifact, field),
                DirectEditOutcome::AlreadyUserAuthoritative
            );
        }
    }

    #[test]
    fn service_update_permitted_only_while_still_service_projected() {
        for field in [FieldKind::Title, FieldKind::Body, FieldKind::Due] {
            assert_eq!(
                service_update_decision(field, OwnershipCode::ServiceProjection),
                ServiceUpdateDecision::Permitted
            );
        }
    }

    #[test]
    fn title_and_body_are_flatly_prohibited_once_user_owned() {
        assert_eq!(
            service_update_decision(FieldKind::Title, OwnershipCode::UserOwned),
            ServiceUpdateDecision::Prohibited
        );
        assert_eq!(
            service_update_decision(FieldKind::Body, OwnershipCode::UserOwned),
            ServiceUpdateDecision::Prohibited
        );
    }

    #[test]
    fn due_requires_the_keep_or_update_choice_once_user_owned() {
        assert_eq!(
            service_update_decision(FieldKind::Due, OwnershipCode::UserOwned),
            ServiceUpdateDecision::RequiresKeepOrUpdateChoice
        );
    }

    #[test]
    fn user_authoritative_and_not_applicable_are_always_prohibited() {
        for field in [FieldKind::Title, FieldKind::Body, FieldKind::Due] {
            assert_eq!(
                service_update_decision(field, OwnershipCode::UserAuthoritative),
                ServiceUpdateDecision::Prohibited
            );
            assert_eq!(
                service_update_decision(field, OwnershipCode::NotApplicable),
                ServiceUpdateDecision::Prohibited
            );
        }
    }

    #[test]
    fn every_field_current_proceeds_with_the_field_mask() {
        assert_eq!(
            classify_owned_field_versions(&[VersionState::Current, VersionState::Current]),
            ConflictDecision::ProceedWithFieldMask
        );
    }

    #[test]
    fn a_remote_edit_between_refetch_and_patch_is_a_conflict_with_no_write() {
        // The TOCTOU matrix: any single non-current state, anywhere in the
        // set, forces a visible conflict and zero PATCH.
        for state in [
            VersionState::Changed,
            VersionState::Stale,
            VersionState::Unavailable,
            VersionState::Conflict,
        ] {
            assert_eq!(
                classify_owned_field_versions(&[VersionState::Current, state]),
                ConflictDecision::VisibleConflictNoPatch
            );
        }
    }

    #[test]
    fn empty_owned_field_set_proceeds_vacuously() {
        assert_eq!(
            classify_owned_field_versions(&[]),
            ConflictDecision::ProceedWithFieldMask
        );
    }

    #[test]
    fn field_mask_includes_only_service_owned_fields() {
        let fields = [
            (FieldKind::Title, OwnershipCode::ServiceProjection),
            (FieldKind::Body, OwnershipCode::UserOwned),
            (FieldKind::Due, OwnershipCode::ServiceProjection),
        ];
        assert_eq!(
            service_owned_field_mask(&fields),
            vec![FieldKind::Title, FieldKind::Due]
        );
    }

    #[test]
    fn field_mask_is_empty_when_every_field_is_user_owned_or_authoritative() {
        let fields = [
            (FieldKind::Title, OwnershipCode::UserOwned),
            (FieldKind::Body, OwnershipCode::UserAuthoritative),
            (FieldKind::Due, OwnershipCode::NotApplicable),
        ];
        assert!(service_owned_field_mask(&fields).is_empty());
    }
}
