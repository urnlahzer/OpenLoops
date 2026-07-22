//! The ADR-009 Microsoft To Do adapter *shape*
//! (`contracts/reminder/adapter-boundary.json` `remote_marker_protocol`,
//! `ownership_matrix`, `direct_edit_and_conflict`, and `todo_boundary`):
//! marker derivation, bounded-enumeration reconciliation classification, the
//! field ownership/conflict engine, and the `ReminderAdapter` port wiring
//! over an injected [`port::TodoTransport`].
//!
//! `todo_boundary.prohibited` and the story brief are explicit that this is
//! a *shape*, not a shipped endpoint integration: no Microsoft To Do
//! request, response, or scope is contacted or requested anywhere in this
//! module or its submodules (`G-TODO` remains unrun; [`crate::is_available`]
//! stays `false`). Every submodule file lives flat in `src/` — `todo.rs`
//! plus `todo_marker.rs`/`todo_reconcile.rs`/`todo_fields.rs`/`todo_port.rs`
//! — and is mounted here with an explicit `#[path]` so the module tree reads
//! as `crate::todo::{marker, reconcile, fields, port}` while the files
//! themselves stay flat and easy to inventory.
//!
//! | Submodule | Contract section |
//! |---|---|
//! | [`marker`] | `remote_marker_protocol.derivation`/`persisted_verifier`/`request_rule` |
//! | [`reconcile`] | `remote_marker_protocol.enumeration_rule`/`outcomes`/`recreate_rule` |
//! | [`fields`] | `ownership_matrix`, `direct_edit_and_conflict`, `reminder_time_boundary` |
//! | [`port`] | the application-layer `ReminderAdapter` port, wired over [`port::TodoTransport`] |
//!
//! # Honest gap: `remote_correlation_hmac` cannot be computed yet
//!
//! `contracts/persistence/protected-state-boundary.json` `digest_suite.
//! purpose_catalog`'s `reminder_link.remote_correlation_hmac` row names
//! owner `"ADR-009"` — this story's own owning ADR fully specifies
//! `remote_marker_protocol.derivation` — but that row's `input_layout` still
//! reads `"unavailable_pending_owner_ADR"`, and
//! `openloops_persistence::digest::PurposeTag::ReminderLinkRemoteCorrelation
//! ::is_layout_closed()` reports `false` (only the seven ADR-006 rows are
//! closed today; see that crate's own `digest` module doc for the general
//! two-owner/one-owner discipline this mirrors from
//! `openloops_application::ledger`'s `operation_key_hmac`). This module does
//! not shortcut that refusal by inventing a layout on ADR-009's authority
//! alone: [`marker::remote_correlation_hmac`] calls straight into
//! `openloops_persistence::digest::compute` and unconditionally propagates
//! [`openloops_persistence::digest::DigestError::LayoutNotYetOwned`] today.
//! The *derivation* itself ([`marker::derive_marker`]) is unaffected — it is
//! a plain, unkeyed SHA-256 transform per the contract, never routed through
//! the purpose-separated HMAC API at all — so the full recoverable-marker
//! shape is implemented and tested; only the durable persisted-verifier half
//! stays honestly refused pending that catalog row's closure.

#[path = "todo_fields.rs"]
pub mod fields;
#[path = "todo_marker.rs"]
pub mod marker;
#[path = "todo_port.rs"]
pub mod port;
#[path = "todo_reconcile.rs"]
pub mod reconcile;
