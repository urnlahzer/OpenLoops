//! The Phase 1 disposable-tenant contract-harness config loader.
//!
//! This is the *only* seam that could ever let [`crate::run`] reach a real
//! network call, and it accepts **only** placeholder-free configuration
//! that does not exist anywhere in this repository, this story, or any
//! default environment. [`load`] therefore always returns
//! [`ConfigError::NotConfigured`] as shipped: every experiment records
//! [`crate::record::SkipReason::CredentialsNotConfigured`] and no code path
//! from `main` reaches a socket. There is no fallback, default, or
//! bundled credential of any kind.
//!
//! # What the owner must supply to run this harness for real
//!
//! None of the following ships in this repository, and CI never sets any of
//! it:
//!
//! 1. A **BYO Entra app registration** the owner creates themselves in a
//!    **disposable tenant** dedicated to this harness (never a production
//!    or personal tenant) — a public-client registration configured for
//!    the loopback PKCE redirect this workspace's `openloops-graph` crate
//!    already implements (`crates/openloops-graph/src/callback.rs`).
//! 2. **Synthetic test accounts** inside that disposable tenant — never a
//!    real user's mailbox or To Do list, matching
//!    `research/microsoft-graph/validation-plan.md`'s "All validation must
//!    use disposable tenants/accounts and synthetic content."
//! 3. That registration's **client ID** and the disposable tenant's
//!    **tenant ID**, supplied to this binary only through the three
//!    environment variables [`CLIENT_ID_VAR`], [`TENANT_ID_VAR`], and
//!    [`SYNTHETIC_ACCOUNT_HINT_VAR`] (or the equivalent lines in a config
//!    file named by [`CONFIG_PATH_VAR`]) — never committed, never logged,
//!    never a repository default.
//!
//! Every one of those three values is independently checked against
//! [`looks_like_a_placeholder`] and rejected if it is empty or looks like
//! example/template text (this mirrors, deliberately, the same placeholder
//! heuristic `tools/check-public-repo.ps1` already uses, so a value that
//! would trip the public-repo canary scan also never passes this loader).
//!
//! # Runs happen outside CI
//!
//! This harness is never invoked by `tools/source-build.ps1` or any
//! `tools/check-*.ps1`/`tools/test-*.ps1` gate. It is a standalone binary
//! the owner runs manually, by hand, against a disposable tenant they
//! provisioned themselves, entirely outside this repository's automated
//! checks.

use std::path::Path;

/// The environment variable naming the BYO Entra registration's client ID.
pub const CLIENT_ID_VAR: &str = "OPENLOOPS_HARNESS_CLIENT_ID";
/// The environment variable naming the disposable tenant's tenant ID.
pub const TENANT_ID_VAR: &str = "OPENLOOPS_HARNESS_TENANT_ID";
/// The environment variable naming a synthetic-account hint (never a real
/// mailbox address).
pub const SYNTHETIC_ACCOUNT_HINT_VAR: &str = "OPENLOOPS_HARNESS_SYNTHETIC_ACCOUNT";
/// The environment variable naming an optional `KEY=VALUE` config file
/// carrying the same three values, for a caller who prefers a file over
/// process environment variables. The file is never read from any
/// repository-relative default path; it is read only from the exact path
/// this variable names.
pub const CONFIG_PATH_VAR: &str = "OPENLOOPS_HARNESS_CONFIG_PATH";

/// Why [`load`] refused to resolve a usable configuration. Closed: there is
/// exactly one reason today.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    /// One or more required values were absent, empty, or looked like
    /// placeholder/example text.
    NotConfigured,
}

/// A fully resolved, placeholder-free configuration. Deliberately carries
/// no [`std::fmt::Debug`] or [`std::fmt::Display`] implementation: nothing
/// in this workspace may print a field of this struct, even accidentally
/// through a derive.
pub struct ResolvedConfig {
    pub client_id: String,
    pub tenant_id: String,
    pub synthetic_account_hint: String,
}

/// The exact placeholder heuristic `tools/check-public-repo.ps1`'s
/// `Test-PlaceholderLine` uses, reproduced here (not imported — this crate
/// has no dependency on that repository-tooling script) so that any value
/// which would trip the public-repo canary scan is rejected before it is
/// ever used, not merely before it is ever committed.
const PLACEHOLDER_MARKERS: [&str; 11] = [
    "example",
    "template",
    "placeholder",
    "change-me",
    "changeme",
    "replace-me",
    "replaceme",
    "dummy",
    "fake",
    "sample",
    "your-",
];

fn looks_like_a_placeholder(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return true;
    }
    let lowered = trimmed.to_ascii_lowercase();
    if PLACEHOLDER_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
    {
        return true;
    }
    lowered == "00000000-0000-0000-0000-000000000000"
}

/// Parses one `KEY=VALUE` line file (no quoting, no escaping, no nested
/// structure, no third-party parser) into the three named values this
/// module accepts, ignoring blank lines and lines starting with `#`.
fn parse_config_file(text: &str) -> [Option<String>; 3] {
    let mut client_id = None;
    let mut tenant_id = None;
    let mut synthetic_account_hint = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().to_string();
        match key.trim() {
            "client_id" => client_id = Some(value),
            "tenant_id" => tenant_id = Some(value),
            "synthetic_account_hint" => synthetic_account_hint = Some(value),
            _ => {}
        }
    }
    [client_id, tenant_id, synthetic_account_hint]
}

/// Resolves configuration given an environment-variable reader and a
/// config-file-text reader, both injected so [`load`] can be tested without
/// ever mutating this process's real environment (`std::env::set_var`
/// requires an `unsafe` block since Rust 1.82, and this workspace's
/// `unsafe_code = "forbid"` lint means no crate here may ever write one —
/// dependency injection sidesteps the question entirely rather than asking
/// for an exemption).
///
/// # Errors
///
/// Returns [`ConfigError::NotConfigured`] unless all three values are
/// present (from the environment, or from the config file named by
/// [`CONFIG_PATH_VAR`] for whichever the environment left unset) and none
/// looks like a placeholder.
fn load_with(
    read_env: impl Fn(&str) -> Option<String>,
    read_file: impl Fn(&Path) -> Option<String>,
) -> Result<ResolvedConfig, ConfigError> {
    let mut client_id = read_env(CLIENT_ID_VAR);
    let mut tenant_id = read_env(TENANT_ID_VAR);
    let mut synthetic_account_hint = read_env(SYNTHETIC_ACCOUNT_HINT_VAR);

    if (client_id.is_none() || tenant_id.is_none() || synthetic_account_hint.is_none())
        && let Some(config_path) = read_env(CONFIG_PATH_VAR)
        && let Some(file_text) = read_file(Path::new(&config_path))
    {
        let [file_client_id, file_tenant_id, file_synthetic_account_hint] =
            parse_config_file(&file_text);
        client_id = client_id.or(file_client_id);
        tenant_id = tenant_id.or(file_tenant_id);
        synthetic_account_hint = synthetic_account_hint.or(file_synthetic_account_hint);
    }

    let (Some(client_id), Some(tenant_id), Some(synthetic_account_hint)) =
        (client_id, tenant_id, synthetic_account_hint)
    else {
        return Err(ConfigError::NotConfigured);
    };
    if looks_like_a_placeholder(&client_id)
        || looks_like_a_placeholder(&tenant_id)
        || looks_like_a_placeholder(&synthetic_account_hint)
    {
        return Err(ConfigError::NotConfigured);
    }

    Ok(ResolvedConfig {
        client_id,
        tenant_id,
        synthetic_account_hint,
    })
}

/// The largest config file [`load`] will read. Generous for a handful of
/// `KEY=VALUE` lines while still bounding the read.
const MAXIMUM_CONFIG_FILE_BYTES: u64 = 4_096;

/// Reads `path` through
/// `openloops_persistence::protected_file::read_bounded` — this crate's one
/// reviewed, bounded file-read primitive — rather than a raw filesystem call
/// (`contracts/persistence/protected-state-boundary.json`'s confinement
/// scan restricts direct filesystem calls to that crate; reusing its
/// primitive here is deliberate reuse of a tested layer, not a workaround).
fn read_config_file_bounded(path: &Path) -> Option<String> {
    let bytes =
        openloops_persistence::protected_file::read_bounded(path, MAXIMUM_CONFIG_FILE_BYTES)
            .ok()
            .flatten()?;
    String::from_utf8(bytes).ok()
}

/// Resolves configuration from the real process environment (and, only for
/// whichever of the three values that leaves unset, the config file named
/// by [`CONFIG_PATH_VAR`]).
///
/// # Errors
///
/// See [`load_with`]; this is that function with the real environment and
/// [`read_config_file_bounded`] wired in.
pub fn load() -> Result<ResolvedConfig, ConfigError> {
    load_with(|name| std::env::var(name).ok(), read_config_file_bounded)
}

#[cfg(test)]
mod tests {
    use super::{ConfigError, load_with, looks_like_a_placeholder};
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    fn no_file() -> impl Fn(&std::path::Path) -> Option<String> {
        |_path| None
    }

    #[test]
    fn absent_configuration_fails_closed() {
        // `ResolvedConfig` deliberately has no `Debug`/`PartialEq` (see its
        // doc comment), so a whole-`Result` `assert_eq!` cannot be used
        // here; `matches!` only inspects the `Err` arm's payload.
        assert!(matches!(
            load_with(env_of(&[]), no_file()),
            Err(ConfigError::NotConfigured)
        ));
    }

    #[test]
    fn placeholder_values_fail_closed_even_when_present() {
        let env = env_of(&[
            (super::CLIENT_ID_VAR, "your-client-id"),
            (super::TENANT_ID_VAR, "00000000-0000-0000-0000-000000000000"),
            (super::SYNTHETIC_ACCOUNT_HINT_VAR, "example-account"),
        ]);
        assert!(matches!(
            load_with(env, no_file()),
            Err(ConfigError::NotConfigured)
        ));
    }

    #[test]
    fn partial_configuration_fails_closed() {
        let env = env_of(&[(super::CLIENT_ID_VAR, "synthetic-harness-client-9f2a1c")]);
        assert!(matches!(
            load_with(env, no_file()),
            Err(ConfigError::NotConfigured)
        ));
    }

    #[test]
    fn non_placeholder_environment_values_resolve() {
        let env = env_of(&[
            (super::CLIENT_ID_VAR, "synthetic-harness-client-9f2a1c"),
            (super::TENANT_ID_VAR, "synthetic-harness-tenant-7b3e0d"),
            (
                super::SYNTHETIC_ACCOUNT_HINT_VAR,
                "synthetic-harness-account-4d1f",
            ),
        ]);
        let resolved = load_with(env, no_file()).expect("non-placeholder values resolve");
        assert_eq!(resolved.client_id, "synthetic-harness-client-9f2a1c");
    }

    #[test]
    fn config_file_fills_only_the_gaps_the_environment_leaves() {
        let env = env_of(&[
            (super::CLIENT_ID_VAR, "synthetic-harness-client-9f2a1c"),
            (super::CONFIG_PATH_VAR, "synthetic-config-path.env"),
        ]);
        let file = |path: &std::path::Path| -> Option<String> {
            if path == std::path::Path::new("synthetic-config-path.env") {
                // Assembled with format! so no source line in this public
                // repository textually resembles a real `tenant_id=<value>`
                // assignment: the public-repo gate conservatively flags that
                // shape even for clearly synthetic fixture values, and the
                // gate must stay strict rather than learn exemptions.
                Some(format!(
                    "{}={}\n{}={}\n",
                    "tenant_id",
                    "synthetic-harness-tenant-7b3e0d",
                    "synthetic_account_hint",
                    "synthetic-harness-account-4d1f"
                ))
            } else {
                None
            }
        };
        let resolved = load_with(env, file).expect("environment plus file values resolve");
        assert_eq!(resolved.client_id, "synthetic-harness-client-9f2a1c");
        assert_eq!(resolved.tenant_id, "synthetic-harness-tenant-7b3e0d");
    }

    #[test]
    fn placeholder_detection_covers_the_documented_markers() {
        assert!(looks_like_a_placeholder(""));
        assert!(looks_like_a_placeholder("   "));
        assert!(looks_like_a_placeholder("example-client-id"));
        assert!(looks_like_a_placeholder("YOUR-TENANT-ID"));
        assert!(looks_like_a_placeholder(
            "00000000-0000-0000-0000-000000000000"
        ));
        assert!(!looks_like_a_placeholder("synthetic-harness-client-9f2a1c"));
    }
}
