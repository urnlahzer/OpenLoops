#![forbid(unsafe_code)]

use openloops_contracts::{SkeletonStatus, synthetic_status as generated_synthetic_status};
use openloops_domain::CapabilityState;

#[must_use]
pub fn synthetic_status() -> SkeletonStatus {
    debug_assert!(!CapabilityState::Disabled.is_enabled());
    generated_synthetic_status()
}

#[cfg(test)]
mod tests {
    #[test]
    fn synthetic_status_is_content_free_and_disabled() {
        let status = super::synthetic_status();
        assert!(openloops_contracts::has_no_claims(&status));
    }
}
