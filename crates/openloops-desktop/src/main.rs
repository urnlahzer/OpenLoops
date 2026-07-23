#![forbid(unsafe_code)]

mod compose;

/// `--compose-probe`: a hidden diagnostic flag (undocumented in any
/// user-facing help text; there is no user-facing help text at all yet)
/// that additionally runs [`compose::compose_probe`] before printing the
/// same synthetic marker. Absent this flag, the binary's behavior is
/// byte-for-byte unchanged from before this story: it still prints exactly
/// `OPENLOOPS_SYNTHETIC_SMOKE_OK` and exits 0 under the same conditions as
/// before. This is a deliberate decision, not an oversight: the smoke
/// assertion `tools/source-build.ps1` runs must never depend on a flag the
/// bootstrap script does not pass, so ordinary invocation stays exactly the
/// old checked-in behavior, and `--compose-probe` exists only for a human
/// (or a future dedicated composition test) to opt into proving the object
/// graph links.
const COMPOSE_PROBE_FLAG: &str = "--compose-probe";

/// Runs [`compose::compose_probe`] and reads back (never mutates) a few
/// fields of the resulting [`compose::Composition`] as evidence the object
/// graph is real, not merely constructed and immediately discarded
/// unread. Returns `true` only when construction succeeded and every
/// invariant compose-time composition promises still holds.
fn run_compose_probe() -> bool {
    let probe_path = compose::default_probe_path();
    let outcome = compose::compose_probe(&probe_path);
    let composition_proved = match &outcome {
        Ok(composition) => {
            composition.scheduler.cadence_seconds()
                == openloops_application::scheduler::POLL_CADENCE_DEFAULT_SECONDS
                && !composition.capability_flags.any_enabled()
                && composition.probe_store_is_empty()
        }
        Err(_) => false,
    };
    // The probe file is intentionally left in place under the OS temp
    // directory rather than deleted here: this crate's only sanctioned
    // filesystem access is through `openloops-persistence`'s reviewed,
    // bounded primitives (`Store::open`, reused above), and this module
    // does not call a raw filesystem deletion itself. The probe database is
    // never under any product state root, holds zero application rows
    // (see `Composition::probe_store_is_empty`), is uniquely named per
    // process ID, and is reclaimed by the OS's own temp-directory
    // lifecycle.
    composition_proved
}

fn main() {
    let run_probe = std::env::args().any(|argument| argument == COMPOSE_PROBE_FLAG);

    let status = openloops_application::synthetic_status();
    let all_adapters_disabled = !openloops_graph::is_available()
        && !openloops_inference::is_available()
        && !openloops_persistence::is_available();
    let capability_flags = openloops_domain::CapabilityFlags::default();

    let composition_proved = if run_probe { run_compose_probe() } else { true };

    if openloops_contracts::has_no_claims(&status)
        && all_adapters_disabled
        && !capability_flags.any_enabled()
        && composition_proved
    {
        println!("OPENLOOPS_SYNTHETIC_SMOKE_OK");
    } else {
        std::process::exit(1);
    }
}
