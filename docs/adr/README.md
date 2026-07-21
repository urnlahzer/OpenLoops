# OpenLoops architecture decision records

Architecture decisions preserve the product-owner-approved boundary while
keeping unresolved implementation contracts visibly gated. An ADR may not turn
an unrun capability gate into an implementation assumption.

## Status meanings

- **Accepted:** the decision is approved and may constrain implementation.
- **Planned:** the ADR is required but incomplete; dependent capabilities remain
  disabled.
- **Superseded:** a later accepted ADR replaces it and records the relationship.

## Registry

| ADR | Topic | Status | Primary owner decisions | Blocking gates |
|---|---|---|---|---|
| [ADR-001](ADR-001-runtime-and-self-hosting-boundary.md) | Runtime and self-hosting boundary | Accepted | OWN-00, OWN-01, OWN-02, OWN-06 | G-ID, G-ADDIN, G-STATE, G-RELEASE |
| [ADR-002](ADR-002-rust-authentication.md) | Rust authentication | Accepted | OWN-00, OWN-02 | G-ID, G-PRIV |
| [ADR-003](ADR-003-incremental-authorization.md) | Incremental authorization | Accepted | OWN-02 | G-ID, G-MAIL, G-TODO, G-CAL, G-ADDIN, G-SELFMAIL, G-PRIV |
| [ADR-PRIV-001](ADR-PRIV-001-derived-metadata-and-source-boundary.md) | Derived metadata and source boundary | Accepted | OWN-06, OWN-07 | G-STATE, G-PRIV, G-SEC-AUDIT |
| [ADR-004](ADR-004-synchronization.md) | Synchronization | Accepted | OWN-05, OWN-09 | G-MAIL, G-CAL |
| [ADR-005](ADR-005-persistence-and-cryptography.md) | Persistence and cryptography | Accepted | OWN-06, OWN-07 | G-STATE, G-PRIV, G-SEC-AUDIT |
| [ADR-006](ADR-006-evidence-identity-and-anchoring.md) | Evidence identity and anchoring | Accepted | OWN-05, OWN-07 | G-MAIL, G-CAL, G-PRIV |
| [ADR-007](ADR-007-model-boundary.md) | Model boundary | Accepted | OWN-08 | G-MODEL, G-PRIV, G-SEC-AUDIT, G-RELEASE |
| [ADR-008](ADR-008-policy-and-state-model.md) | Policy and state model | Accepted | OWN-03 | G-AUTO, G-AUTO-FULL |
| [ADR-009](ADR-009-reminder-adapters.md) | Reminder adapters | Accepted | OWN-03, OWN-04, OWN-05, OWN-07 | G-TODO, G-CAL, G-AUTO, G-AUTO-FULL |
| [ADR-010](ADR-010-add-in-bridge.md) | Add-in bridge | Accepted | OWN-01 | G-ADDIN |
| [ADR-011](ADR-011-automation-and-evaluation.md) | Automation and evaluation | Accepted | OWN-03 | G-AUTO, G-AUTO-FULL |
| ADR-012 | Distribution and registration | Planned | OWN-00, OWN-01, OWN-02 | G-ID, G-RELEASE |
| ADR-013 | Self-email | Planned | OWN-10 | G-SELFMAIL |

The machine-readable registry is
`contracts/governance/capabilities.json`. `P0-TRACE-001` checks that its ADR,
gate, and owner-decision sets remain exact and that no planned capability is
advertised or enabled.
