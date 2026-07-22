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
//! | [`deadline`] | `deadline_policy` |
//! | [`hypothesis`] | `hypothesis_projection` |
//! | [`command`] | `command_policy`, `transition_policy`, `correction_policy.duplicate` |
//! | [`transition`] | `record_contracts[transition]` |
//! | [`record`] | `record_contracts[loop]`, `record_contracts[deadline_evidence]` |
//! | [`display`] | `facet_catalogs.primary_label_precedence` |

pub mod command;
pub mod deadline;
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

#[cfg(test)]
mod tests {
    use super::CapabilityState;

    #[test]
    fn phase_zero_has_no_enabled_capability() {
        assert!(!CapabilityState::Disabled.is_enabled());
    }
}
