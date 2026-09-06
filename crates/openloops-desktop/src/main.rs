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
    #[cfg(not(feature = "ollama-cloud"))]
    if std::env::args().any(|argument| argument == "--check-ollama") {
        eprintln!("Run tools/connect-ollama.ps1 to build the Ollama Cloud feature.");
        std::process::exit(1);
    }
    #[cfg(feature = "ollama-cloud")]
    if std::env::args().any(|argument| argument == "--check-ollama") {
        run_ollama_check();
        return;
    }
    #[cfg(not(feature = "live-connection"))]
    if std::env::args().any(|argument| argument == "--check-connection") {
        eprintln!("Build with --features live-connection, or run tools/connect-microsoft.ps1.");
        std::process::exit(1);
    }
    #[cfg(feature = "live-connection")]
    if std::env::args().any(|argument| argument == "--check-connection") {
        run_connection_check();
        return;
    }
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

#[cfg(feature = "ollama-cloud")]
fn run_ollama_check() {
    use openloops_inference::ollama::{OllamaCloud, available_models, suggested_model};
    use std::io::{self, Write};
    let key = std::env::var("OLLAMA_API_KEY").unwrap_or_default();
    let result = (|| {
        let models = available_models(&key)?;
        if models.is_empty() {
            return Err(openloops_inference::ollama::ProviderError::ModelUnavailable);
        }
        println!("Available Ollama Cloud models:");
        let default_index = suggested_model(&models);
        for (index, model) in models.iter().enumerate() {
            let suggested = if Some(index) == default_index {
                " (suggested)"
            } else {
                ""
            };
            println!("  {}. {model}{suggested}", index + 1);
        }
        let selected = loop {
            print!(
                "Choose a model number{}: ",
                if default_index.is_some() {
                    " (Enter selects DeepSeek)"
                } else {
                    ""
                }
            );
            if io::stdout().flush().is_err() {
                return Err(openloops_inference::ollama::ProviderError::InputUnavailable);
            }
            let mut input = String::new();
            if io::stdin().read_line(&mut input).unwrap_or(0) == 0 {
                return Err(openloops_inference::ollama::ProviderError::InputUnavailable);
            }
            let index = if input.trim().is_empty() {
                default_index
            } else {
                input
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .and_then(|number| number.checked_sub(1))
            };
            if let Some(model) = index.and_then(|index| models.get(index)) {
                break model;
            }
            println!("Enter one of the listed model numbers.");
        };
        println!("Checking {selected} on Ollama Cloud with a content-free generation request.");
        let provider = OllamaCloud::connect(key, selected)?;
        provider.check_generation()?;
        println!(
            "Ollama Cloud authentication, model access, and structured response validation succeeded."
        );
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(feature = "live-connection")]
fn run_connection_check() {
    use openloops_graph::live::{ConnectionConfig, SharedScope, check_connection};
    let client = std::env::var("OPENLOOPS_CLIENT_ID").unwrap_or_default();
    let shared = std::env::var("OPENLOOPS_SHARED_MAILBOX").ok();
    let groups = std::env::var("OPENLOOPS_GROUP_INBOXES").ok();
    let config = match ConnectionConfig::new(&client, shared.as_deref())
        .and_then(|config| config.with_groups(groups.as_deref()))
    {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    println!(
        "Opening Microsoft sign-in. This checks inbox access without storing mail or creating tasks."
    );
    match check_connection(&config) {
        Ok(report) => {
            println!("Microsoft sign-in and personal inbox access succeeded.");
            if !report.shared_inbox_results.is_empty() {
                println!(
                    "{}",
                    match report.shared_scope {
                        SharedScope::Granted =>
                            "Microsoft token response confirms shared-mail permission was granted.",
                        SharedScope::Missing =>
                            "Microsoft token response does not include shared-mail permission.",
                        SharedScope::NotReported =>
                            "Microsoft did not list granted permissions in its token response; checking shared inbox access directly.",
                    }
                );
            }
            let mut failed = false;
            for (index, result) in report.shared_inbox_results.iter().enumerate() {
                match result {
                    Ok(()) => println!("Shared inbox {}: access succeeded.", index + 1),
                    Err(error) => {
                        eprintln!("Shared inbox {}: {error}", index + 1);
                        failed = true;
                    }
                }
            }
            for (index, result) in report.group_inbox_results.iter().enumerate() {
                match result {
                    Ok(()) => println!("Group inbox {}: access succeeded.", index + 1),
                    Err(error) => {
                        eprintln!("Group inbox {}: {error}", index + 1);
                        failed = true;
                    }
                }
            }
            println!("Connection check complete. The session token has been discarded.");
            if failed {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
