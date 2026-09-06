#![forbid(unsafe_code)]
//! `openloops-eval`: development-tooling runner over the public synthetic
//! TRAIN/DEV corpus (implementation-plan §8.2's public half).
//!
//! This binary runs the real, deterministic pipeline —
//! `openloops_inference::message::canonicalize_message`, then
//! `openloops_inference::validation::validate` against a mock provider
//! response literally embedded in each fixture, then
//! `openloops_domain::establishment::decide` for every accepted claim that
//! declares an establishment case — over every fixture in
//! `fixtures/synthetic/`, and prints only sanitized per-category aggregate
//! counts.
//!
//! # This is not a gate
//!
//! [`NOT_A_GATE_DISCLAIMER`] is printed verbatim on every run. This corpus
//! is small, public, hand-authored, synthetic, and exists for development
//! iteration only. It is not, and does not stand in for, the sealed
//! ADR-011 release-judge corpus (owner-held, sealed outside the repository
//! and outside any maker/implementation-LLM context, versioned and
//! contamination-checked per `docs/adr/ADR-011-automation-and-evaluation.md`).
//! Nothing this binary prints passes a gate, completes an acceptance
//! criterion, or enables a capability.

mod corpus;
mod runner;
mod schema;

use std::path::PathBuf;

use runner::{CategoryTally, Report};
use schema::Fixture;

/// Printed verbatim on every run, first line of output.
pub const NOT_A_GATE_DISCLAIMER: &str = "NOT A GATE: this report is development tooling over a small public synthetic corpus, not the sealed ADR-011 release-judge corpus; it proves zero acceptance criterion, gate, or capability, and asserts nothing about production-scale accuracy.";

/// Reads and parses every fixture named in [`corpus::FIXTURE_IDS`] under
/// `root`, in that fixed manifest order, and checks every parsed `id`
/// matches its filename and is unique.
///
/// # Errors
///
/// Returns a sanitized, content-free error string (a fixture id and/or
/// filename only, never parsed content) on any read or parse failure.
fn load_corpus(root: &std::path::Path) -> Result<Vec<Fixture>, String> {
    let mut fixtures = Vec::with_capacity(corpus::FIXTURE_IDS.len());
    let mut seen_ids = std::collections::HashSet::new();
    for (expected_id, text) in corpus::read_fixture_texts(root)? {
        let fixture: Fixture = serde_json::from_str(&text)
            .map_err(|error| format!("fixture '{expected_id}' failed schema parse: {error}"))?;
        if fixture.id != expected_id {
            return Err(format!(
                "fixture '{expected_id}' declares a different id in its own body"
            ));
        }
        if !seen_ids.insert(fixture.id.clone()) {
            return Err(format!("fixture '{expected_id}' has a duplicate id"));
        }
        fixtures.push(fixture);
    }
    Ok(fixtures)
}

fn print_tally_row(label: &str, tally: &CategoryTally) {
    println!(
        "  {label:<26} fixtures={:<3} top_level_match={:<3} top_level_mismatch={:<3} canonicalize_failed={:<3} claim_count_mismatch={:<3} claim_validation_match={:<3} claim_validation_mismatch={:<3} establishment[open={} candidate={} pending_gate={} unchanged={} not_modeled={}]",
        tally.fixtures,
        tally.top_level_expected_match,
        tally.top_level_expected_mismatch,
        tally.canonicalize_failed,
        tally.claim_count_mismatch,
        tally.claim_validation_match,
        tally.claim_validation_mismatch,
        tally.establishment_open,
        tally.establishment_candidate,
        tally.establishment_pending_gate,
        tally.establishment_unchanged,
        tally.establishment_not_modeled,
    );
}

fn print_report(report: &Report) {
    println!("OpenLoops synthetic eval report (per-category counts; zero fixture content below):");
    for (category, tally) in &report.per_category {
        print_tally_row(category.label(), tally);
    }
    println!("  ---");
    print_tally_row("TOTAL", &report.total);
}

fn main() {
    println!("{NOT_A_GATE_DISCLAIMER}");

    let root = std::env::var_os("OPENLOOPS_EVAL_CORPUS_ROOT")
        .map_or_else(corpus::default_corpus_root, PathBuf::from);

    let fixtures = match load_corpus(&root) {
        Ok(fixtures) => fixtures,
        Err(message) => {
            eprintln!("BLOCKED: {message}");
            std::process::exit(1);
        }
    };
    if fixtures.is_empty() {
        eprintln!("BLOCKED: corpus directory contained zero fixtures");
        std::process::exit(1);
    }

    let report = runner::run_corpus(&fixtures);
    println!("corpus size: {} fixtures", fixtures.len());
    print_report(&report);
}
