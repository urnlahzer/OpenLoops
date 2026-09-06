//! Loads `fixtures/synthetic/*.json` by a fixed, hand-maintained filename
//! manifest, reading each file through
//! `openloops_persistence::protected_file::read_bounded` rather than a raw
//! filesystem call.
//!
//! This is a deliberate reuse, not a workaround: `openloops-persistence` is
//! this workspace's one reviewed, bounded file-read primitive
//! (`contracts/persistence/protected-state-boundary.json`'s confinement
//! scan restricts direct filesystem calls to that crate), and a fixed
//! manifest here means adding a fixture is a reviewable one-line diff in
//! this file rather than a silent directory-listing change nothing
//! compiles against.

use std::path::{Path, PathBuf};

use openloops_persistence::protected_file::read_bounded;

/// Generous enough for any hand-authored fixture in this corpus (each is a
/// few hundred bytes to a few kilobytes) while still bounding the read.
const MAXIMUM_FIXTURE_BYTES: u64 = 65_536;

/// Every fixture id in the corpus, in the order this report groups its
/// category rows (see `schema::Category::ALL`); the filename is always
/// `{id}.json`. Keep this list and `fixtures/synthetic/`'s actual contents
/// in lockstep — `load_corpus` fails closed (loudly) if a name here has no
/// file, and the loader's own duplicate-id check catches an id reused by
/// mistake.
pub const FIXTURE_IDS: &[&str] = &[
    // direct_request
    "direct-request-01",
    "direct-request-02",
    "direct-request-03",
    // promise
    "promise-01",
    "promise-02",
    "promise-03",
    // acknowledgment
    "acknowledgment-01",
    "acknowledgment-02",
    // soft_implied
    "soft-implied-01",
    "soft-implied-02",
    // quote_duplicate_trap
    "quote-duplicate-trap-01",
    "quote-duplicate-trap-02",
    "quote-duplicate-trap-03",
    // forwarded_reassignment
    "forwarded-reassignment-01",
    "forwarded-reassignment-02",
    // multi_action_split
    "multi-action-split-01",
    "multi-action-split-02",
    // deadline_relative
    "deadline-relative-01",
    "deadline-relative-02",
    // deadline_ambiguous
    "deadline-ambiguous-01",
    "deadline-ambiguous-02",
    // deadline_event_relative
    "deadline-event-relative-01",
    "deadline-event-relative-02",
    // closure_candidate
    "closure-candidate-01",
    "closure-candidate-02",
    // delegation
    "delegation-01",
    "delegation-02",
    // hostile_prompt_injection
    "hostile-prompt-injection-01",
    "hostile-prompt-injection-02",
    "hostile-prompt-injection-03",
    // invalid_model_output
    "invalid-model-output-01",
    "invalid-model-output-02",
    "invalid-model-output-03",
];

/// The default corpus root, resolved at compile time relative to this
/// crate's own manifest so the binary finds the corpus regardless of the
/// caller's current directory.
#[must_use]
pub fn default_corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic")
}

/// Reads every fixture named in [`FIXTURE_IDS`] under `root` as raw UTF-8
/// text, in [`FIXTURE_IDS`] order.
///
/// # Errors
///
/// Returns a sanitized, content-free error string (the fixture id and
/// filename only, never file content) if a named fixture is missing,
/// oversized, unreadable, or not valid UTF-8.
pub fn read_fixture_texts(root: &Path) -> Result<Vec<(&'static str, String)>, String> {
    let mut texts = Vec::with_capacity(FIXTURE_IDS.len());
    for &id in FIXTURE_IDS {
        let path = root.join(format!("{id}.json"));
        let bytes = read_bounded(&path, MAXIMUM_FIXTURE_BYTES)
            .map_err(|_| format!("fixture '{id}' could not be read"))?
            .ok_or_else(|| format!("fixture '{id}' has no file at its manifest path"))?;
        let text =
            String::from_utf8(bytes).map_err(|_| format!("fixture '{id}' is not valid UTF-8"))?;
        texts.push((id, text));
    }
    Ok(texts)
}
