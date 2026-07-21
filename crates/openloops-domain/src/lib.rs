#![forbid(unsafe_code)]

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
