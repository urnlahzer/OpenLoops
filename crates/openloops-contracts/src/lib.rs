#![forbid(unsafe_code)]

typify::import_types!(schema = "../../contracts/ipc/skeleton-status.schema.json");

#[must_use]
/// Constructs the fixed Phase 0 status from the compile-time schema.
///
/// # Panics
///
/// Panics only if the schema-derived type rejects the fixed synthetic value,
/// which indicates a build-time contract-generator defect.
pub fn synthetic_status() -> SkeletonStatus {
    serde_json::from_value(serde_json::json!({
        "contract_version": 1,
        "companion_state": "skeleton_disabled",
        "enabled_capabilities": [],
        "gates_passed": []
    }))
    .expect("the compile-time schema accepts the fixed synthetic status")
}

#[must_use]
/// Returns whether the schema-derived status carries zero capability claims.
///
/// # Panics
///
/// Panics only if Typify produces a status type that Serde cannot serialize,
/// which indicates a build-time contract-generator defect.
pub fn has_no_claims(status: &SkeletonStatus) -> bool {
    let value = serde_json::to_value(status).expect("generated status is serializable");
    value["companion_state"] == "skeleton_disabled"
        && value["enabled_capabilities"]
            .as_array()
            .is_some_and(Vec::is_empty)
        && value["gates_passed"].as_array().is_some_and(Vec::is_empty)
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_contract_cannot_claim_capabilities_or_gates() {
        let status = super::synthetic_status();
        assert!(super::has_no_claims(&status));
    }
}
