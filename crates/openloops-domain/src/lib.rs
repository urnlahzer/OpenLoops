#![forbid(unsafe_code)]
//! `openloops-domain`: the deterministic ADR-008 loop policy/state engine.
//!
//! Pure Rust, `std`-only, zero new dependencies. No I/O, no persistence, no
//! network, no reminder adapter, no model call. This crate turns
//! `contracts/domain/policy-state-boundary.json` into executable, tested
//! library code while every capability flag stays [`CapabilityState::Disabled`].
//!
//! | Module | Contract section |
//! |---|---|
//! | [`facets`] | `facet_catalogs` |
//! | [`ids`] | opaque identifiers and bounded reference collections underlying every `record_contracts` field |
//! | [`legality`] | `legality_rules` (the Cartesian facet-combination subset) |
//! | [`establishment`] | `establishment_policy` |
//! | [`deadline`] | `deadline_policy` (precedence and aging) |
//! | [`deadline_parse`] | `deadline_policy` (OL-DUE-002 deterministic text parsing) |
//! | [`hypothesis`] | `hypothesis_projection` |
//! | [`command`] | `command_policy`, `transition_policy`, `correction_policy.duplicate` |
//! | [`transition`] | `record_contracts[transition]` |
//! | [`record`] | `record_contracts[loop]`, `record_contracts[deadline_evidence]` |
//! | [`display`] | `facet_catalogs.primary_label_precedence` |

pub mod command;
pub mod deadline;
pub mod deadline_parse;
pub mod display;
pub mod establishment;
pub mod facets;
pub mod hypothesis;
pub mod ids;
pub mod legality;
pub mod record;
pub mod transition;

/// The only capability state permitted by the Phase 0 skeleton.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityState {
    Disabled,
}

impl CapabilityState {
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        false
    }
}

/// The ADR-011 feature-flag consumer seam: `docs/adr/ADR-011-automation-and-evaluation.md`
/// "Feature-flag topology" fixes `hybrid_enabled` and `automatic_enabled` as
/// two independent flags that "default to `false`" and that "[n]either flag
/// flips by code, configuration drift, a mode-change setting, or any other
/// implicit path." This crate does not own ADR-011's evaluation, gate, or
/// release-decision machinery (that stays entirely with ADR-011 and remains
/// unimplemented pending G-AUTO/G-AUTO-FULL); it owns only the closed,
/// always-false struct shape a future flag reader would consult, so that any
/// caller of this crate — including `openloops-desktop`'s composition
/// seam — has exactly one typed, exhaustive place to check "is any
/// automation capability enabled" rather than inventing an ad hoc boolean.
/// [`CapabilityFlags::default()`] is the only constructor; there is no
/// setter, no `From`, and no code path anywhere in this workspace that
/// produces a `CapabilityFlags` with either field `true`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CapabilityFlags {
    /// ADR-011 `hybrid_enabled`. Always `false` until an accountable owner
    /// records a release decision citing passed G-AUTO evidence; this crate
    /// has no path that sets it.
    pub hybrid_enabled: bool,
    /// ADR-011 `automatic_enabled`. Always `false` until an accountable
    /// owner records a release decision citing passed G-AUTO-FULL evidence
    /// plus the user's explicit opt-in; this crate has no path that sets it.
    pub automatic_enabled: bool,
}

impl CapabilityFlags {
    /// Whether any ADR-011 automation flag is enabled. A composition seam
    /// (e.g. `openloops-desktop::compose`) can assert `!flags.any_enabled()`
    /// as an invariant without needing to know the individual flag names.
    #[must_use]
    pub const fn any_enabled(self) -> bool {
        self.hybrid_enabled || self.automatic_enabled
    }
}

#[cfg(test)]
mod tests {
    use super::{CapabilityFlags, CapabilityState};

    #[test]
    fn phase_zero_has_no_enabled_capability() {
        assert!(!CapabilityState::Disabled.is_enabled());
    }

    #[test]
    fn capability_flags_default_to_all_disabled() {
        let flags = CapabilityFlags::default();
        assert!(!flags.hybrid_enabled);
        assert!(!flags.automatic_enabled);
        assert!(!flags.any_enabled());
    }
}
