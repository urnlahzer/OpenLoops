#![forbid(unsafe_code)]
//! `openloops-harness`: the Phase 1 disposable-tenant Microsoft Graph
//! contract-harness runner.
//!
//! Implements `research/microsoft-graph/validation-plan.md`'s experiment
//! matrix (identity 1-10, mail 1-10, To Do 1-8, reliability/privacy 1-9 —
//! 37 rows total) as typed experiment definitions
//! ([`record::Experiment`]/[`record::matrix`]) with sanitized-outcome
//! recording ([`record::SanitizedRecord`]: experiment id, pass/fail/skip, a
//! sanitized reason code, and the Graph API version string — never a
//! token, tenant/account/message identifier, or response payload, matching
//! AGENTS.md).
//!
//! # No network call is reachable without explicit, non-placeholder config
//!
//! [`config::load`] (see its own doc comment for exactly what an owner must
//! supply) accepts only configuration this repository never ships and this
//! story's CI never sets, so it always returns
//! `Err(config::ConfigError::NotConfigured)` today. [`run`] checks that
//! result and, for every one of the 37 experiments, unconditionally
//! records [`record::Outcome::Skip`] with
//! [`record::SkipReason::CredentialsNotConfigured`] — this is the *only*
//! outcome this story's runner can produce, by construction: there is no
//! experiment-body function anywhere in this crate that constructs a
//! [`openloops_graph::transport::Wire`] or calls
//! [`openloops_graph::transport::GraphTransport::send`]. The one place this
//! crate references [`openloops_graph::transport::GraphTransport`] at all
//! is inside the (this story, unreachable) branch of [`run`] where
//! `config::load` would have succeeded, and even there it is only
//! constructed, proving the typed transport this crate would reuse links
//! against this binary, never sent through.
//!
//! # `--list`
//!
//! Prints the full 37-row matrix (group, number, label) and exits without
//! resolving configuration, constructing anything, or touching the network.
//!
//! # Runs happen outside CI
//!
//! See [`config`]'s module doc comment. This binary is never invoked by
//! `tools/source-build.ps1` or any `tools/check-*.ps1`/`tools/test-*.ps1`
//! gate.

mod config;
mod record;

use std::time::Duration;

use config::ConfigError;
use record::{Experiment, Outcome, SanitizedRecord, SkipReason};

const LIST_FLAG: &str = "--list";

fn print_matrix() {
    println!("openloops-harness experiment matrix (37 rows; no experiment has run):");
    for experiment in record::matrix() {
        println!("  {:<16} {}", experiment.id(), experiment.label);
    }
}

/// Produces exactly one [`SanitizedRecord`] for `experiment`. Every code
/// path here that could conceivably reach a real request is gated by
/// `config`: the `Ok` arm below constructs (never sends through) a
/// [`openloops_graph::transport::GraphTransport`] purely to prove the
/// typed transport this crate would reuse links against this binary; no
/// experiment body exists in this story regardless of whether
/// configuration ever resolves, so both arms record the same closed skip
/// reason today.
fn run_experiment(
    experiment: &Experiment,
    config: &Result<config::ResolvedConfig, ConfigError>,
) -> SanitizedRecord {
    let outcome = match config {
        Err(ConfigError::NotConfigured) => Outcome::Skip(SkipReason::CredentialsNotConfigured),
        Ok(resolved) => {
            // Reachable only once an owner supplies real, non-placeholder
            // configuration outside this repository — never in this
            // story's CI. This reads every resolved field — never prints,
            // logs, or formats one — solely so the compiler proves each
            // field is genuinely plumbed through to this point rather than
            // dead weight; the bare `_` discard drops the borrowed tuple
            // immediately without ever inspecting a value.
            let _ = (
                &resolved.client_id,
                &resolved.tenant_id,
                &resolved.synthetic_account_hint,
            );
            let _: openloops_graph::transport::GraphTransport =
                openloops_graph::transport::GraphTransport::new(Duration::from_secs(30));
            Outcome::Skip(SkipReason::CredentialsNotConfigured)
        }
    };
    SanitizedRecord {
        experiment_id: experiment.id(),
        outcome,
        reason_code: SkipReason::CredentialsNotConfigured.code(),
        api_version: record::GRAPH_API_VERSION,
    }
}

/// Runs every matrix row and returns its sanitized records, in matrix
/// order.
#[must_use]
pub fn run() -> Vec<SanitizedRecord> {
    let config = config::load();
    record::matrix()
        .iter()
        .map(|experiment| run_experiment(experiment, &config))
        .collect()
}

fn print_records(records: &[SanitizedRecord]) {
    let pass = records
        .iter()
        .filter(|record| matches!(record.outcome, Outcome::Pass))
        .count();
    let fail = records
        .iter()
        .filter(|record| matches!(record.outcome, Outcome::Fail))
        .count();
    let skip = records
        .iter()
        .filter(|record| matches!(record.outcome, Outcome::Skip(_)))
        .count();
    println!(
        "openloops-harness run: {} experiments, {pass} pass, {fail} fail, {skip} skip (api_version {})",
        records.len(),
        record::GRAPH_API_VERSION,
    );
    for record in records {
        println!(
            "  {:<16} outcome={:<8} reason={} api_version={}",
            record.experiment_id,
            record.outcome.code(),
            record.reason_code,
            record.api_version
        );
    }
}

fn main() {
    let list_only = std::env::args().any(|argument| argument == LIST_FLAG);
    if list_only {
        print_matrix();
        return;
    }
    let records = run();
    print_records(&records);
}
