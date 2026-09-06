//! The typed experiment matrix (`research/microsoft-graph/validation-plan.md`)
//! and the closed, sanitized outcome-record shape AGENTS.md requires: an
//! experiment id, pass/fail/skip, a sanitized reason code, and an API
//! version string — never a token, a tenant/account/message identifier, or
//! a response payload. No field on any type in this module can hold free
//! text; every field is either a `&'static str` drawn from a fixed
//! compile-time table or a closed enum.

/// `research/microsoft-graph/validation-plan.md`'s four experiment groups.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExperimentGroup {
    Identity,
    Mail,
    Todo,
    ReliabilityPrivacy,
}

impl ExperimentGroup {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Mail => "mail",
            Self::Todo => "todo",
            Self::ReliabilityPrivacy => "reliability_privacy",
        }
    }
}

/// One experiment: a group, a 1-based number within that group (matching
/// the validation plan's own numbered lists exactly), and a short,
/// already-sanitized label (no tenant, account, or message content — these
/// are the plan's own section headings/summaries, not runtime data).
#[derive(Clone, Copy, Debug)]
pub struct Experiment {
    pub group: ExperimentGroup,
    pub number: u8,
    pub label: &'static str,
}

impl Experiment {
    /// The stable, sanitized id this experiment is recorded under, e.g.
    /// `"identity-01"`.
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}-{:02}", self.group.code(), self.number)
    }
}

/// `research/microsoft-graph/validation-plan.md` "Identity and consent
/// experiments" 1-10, in exact list order.
const IDENTITY: [Experiment; 10] = [
    Experiment {
        group: ExperimentGroup::Identity,
        number: 1,
        label: "work/school browser-PKCE sign-in with the selected pure-Rust OAuth runtime",
    },
    Experiment {
        group: ExperimentGroup::Identity,
        number: 2,
        label: "personal-account browser-PKCE sign-in as a separately gated capability",
    },
    Experiment {
        group: ExperimentGroup::Identity,
        number: 3,
        label: "localhost and 127.0.0.1 redirects with the selected OAuth runtime and Entra registration",
    },
    Experiment {
        group: ExperimentGroup::Identity,
        number: 4,
        label: "missing, incorrect, replayed, duplicated, and expired OAuth state",
    },
    Experiment {
        group: ExperimentGroup::Identity,
        number: 5,
        label: "loopback port races and non-loopback/LAN reachability",
    },
    Experiment {
        group: ExperimentGroup::Identity,
        number: 6,
        label: "user consent enabled, disabled, admin-gated, and verified-publisher-only",
    },
    Experiment {
        group: ExperimentGroup::Identity,
        number: 7,
        label: "incremental consent denial and later grant",
    },
    Experiment {
        group: ExperimentGroup::Identity,
        number: 8,
        label: "consent/session revocation and Conditional Access changes",
    },
    Experiment {
        group: ExperimentGroup::Identity,
        number: 9,
        label: "guest-account and multiple-tenant account selection",
    },
    Experiment {
        group: ExperimentGroup::Identity,
        number: 10,
        label: "cache loss, secure-store lock, reconnect, and disconnect",
    },
];

/// "Mail experiments" 1-10, in exact list order.
const MAIL: [Experiment; 10] = [
    Experiment {
        group: ExperimentGroup::Mail,
        number: 1,
        label: "Mail.ReadBasic list, get, and per-folder delta for both account categories",
    },
    Experiment {
        group: ExperimentGroup::Mail,
        number: 2,
        label: "assert prohibited body/preview/attachment/extended-property data is unavailable under Mail.ReadBasic",
    },
    Experiment {
        group: ExperimentGroup::Mail,
        number: 3,
        label: "Mail.Read content access using synthetic messages",
    },
    Experiment {
        group: ExperimentGroup::Mail,
        number: 4,
        label: "Mail.ReadWrite mutation without Mail.Send",
    },
    Experiment {
        group: ExperimentGroup::Mail,
        number: 5,
        label: "Mail.Send behavior without read/write permissions",
    },
    Experiment {
        group: ExperimentGroup::Mail,
        number: 6,
        label: "immutable IDs across folder moves, copy, delete, and draft send",
    },
    Experiment {
        group: ExperimentGroup::Mail,
        number: 7,
        label: "Inbox, Sent Items, and user-selected folder delta behavior",
    },
    Experiment {
        group: ExperimentGroup::Mail,
        number: 8,
        label: "empty pages, paging, duplicate page application, cursor expiry, bounded resynchronization",
    },
    Experiment {
        group: ExperimentGroup::Mail,
        number: 9,
        label: "If-Match behavior for every intended mutation",
    },
    Experiment {
        group: ExperimentGroup::Mail,
        number: 10,
        label: "bounded initial-history filters and exact $select field sets",
    },
];

/// "Microsoft To Do experiments" 1-8, in exact list order. The plan
/// requires running each test separately for work/school and personal
/// accounts; that per-account-category split is a future runner's
/// responsibility, not a distinct matrix row here (see this crate's
/// `README.md` for the honest gap).
const TODO: [Experiment; 8] = [
    Experiment {
        group: ExperimentGroup::Todo,
        number: 1,
        label: "with Tasks.Read, list lists and tasks",
    },
    Experiment {
        group: ExperimentGroup::Todo,
        number: 2,
        label: "with Tasks.Read, call list delta and task delta and record sanitized status/permission outcomes",
    },
    Experiment {
        group: ExperimentGroup::Todo,
        number: 3,
        label: "with Tasks.ReadWrite, repeat reads/deltas and test create/update/delete",
    },
    Experiment {
        group: ExperimentGroup::Todo,
        number: 4,
        label: "test built-in Tasks and Flagged Email lists",
    },
    Experiment {
        group: ExperimentGroup::Todo,
        number: 5,
        label: "test task movement or delete/add behavior between lists",
    },
    Experiment {
        group: ExperimentGroup::Todo,
        number: 6,
        label: "test shared list/task behavior only if it becomes product scope",
    },
    Experiment {
        group: ExperimentGroup::Todo,
        number: 7,
        label: "test stale/current If-Match behavior",
    },
    Experiment {
        group: ExperimentGroup::Todo,
        number: 8,
        label: "simulate ambiguous POST completion and verify idempotency handling",
    },
];

/// "Reliability and privacy experiments" 1-9, in exact list order.
const RELIABILITY_PRIVACY: [Experiment; 9] = [
    Experiment {
        group: ExperimentGroup::ReliabilityPrivacy,
        number: 1,
        label: "crash before and after every cursor/idempotency checkpoint boundary",
    },
    Experiment {
        group: ExperimentGroup::ReliabilityPrivacy,
        number: 2,
        label: "inject 429/503/504/timeout/malformed-retry/partial-batch-throttling responses",
    },
    Experiment {
        group: ExperimentGroup::ReliabilityPrivacy,
        number: 3,
        label: "validate bounded exponential backoff, jitter, stale-state reporting, recovery from invalid delta state",
    },
    Experiment {
        group: ExperimentGroup::ReliabilityPrivacy,
        number: 4,
        label: "place unique synthetic canaries in subject/body/participant/task-title/task-body/token/delta-URL/authorization-header positions",
    },
    Experiment {
        group: ExperimentGroup::ReliabilityPrivacy,
        number: 5,
        label: "exercise success/denial/error/crash/update/uninstall/support-bundle paths; scan for raw canaries",
    },
    Experiment {
        group: ExperimentGroup::ReliabilityPrivacy,
        number: 6,
        label: "attempt cache access as another OS user and after backup/restore or machine transfer",
    },
    Experiment {
        group: ExperimentGroup::ReliabilityPrivacy,
        number: 7,
        label: "run without a usable secure store and confirm session-only/fail-closed behavior with no cache artifact",
    },
    Experiment {
        group: ExperimentGroup::ReliabilityPrivacy,
        number: 8,
        label: "tamper with installers/updates/dependencies/lockfiles; unsigned or inconsistent artifacts must be rejected",
    },
    Experiment {
        group: ExperimentGroup::ReliabilityPrivacy,
        number: 9,
        label: "hostile synthetic message content (HTML, scripts, remote images, prompt injection, shell-like instructions, fake administrator requests) must not execute or override product policy",
    },
];

/// The complete 37-row matrix, group by group, number order within each
/// group — exactly `research/microsoft-graph/validation-plan.md`'s
/// identity (10) + mail (10) + To Do (8) + reliability/privacy (9) lists.
#[must_use]
pub fn matrix() -> Vec<Experiment> {
    let mut all = Vec::with_capacity(37);
    all.extend_from_slice(&IDENTITY);
    all.extend_from_slice(&MAIL);
    all.extend_from_slice(&TODO);
    all.extend_from_slice(&RELIABILITY_PRIVACY);
    all
}

/// A rejected/incomplete experiment attempt's sanitized reason. Closed: the
/// only reason this story's runner can ever produce, because
/// [`crate::config::load`] has exactly one failure mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkipReason {
    CredentialsNotConfigured,
}

impl SkipReason {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::CredentialsNotConfigured => "credentials_not_configured",
        }
    }
}

/// One experiment's closed outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Pass,
    Fail,
    Skip(SkipReason),
}

impl Outcome {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Skip(reason) => reason.code(),
        }
    }
}

/// The exact API version string every recorded outcome carries
/// (`research/microsoft-graph/validation-plan.md`'s own recording rule:
/// "Record only sanitized test outcomes and API/service versions").
pub const GRAPH_API_VERSION: &str = "v1.0";

/// One experiment's complete sanitized outcome record: experiment id,
/// pass/fail/skip, sanitized reason code, API version string. AGENTS.md:
/// "NEVER tokens/IDs/payloads" — no field here can hold one; every field is
/// a closed enum code or a fixed `&'static str`.
#[derive(Clone, Debug)]
pub struct SanitizedRecord {
    pub experiment_id: String,
    pub outcome: Outcome,
    pub reason_code: &'static str,
    pub api_version: &'static str,
}

#[cfg(test)]
mod tests {
    use super::{ExperimentGroup, matrix};

    #[test]
    fn matrix_has_exactly_the_validation_plan_row_count() {
        let rows = matrix();
        assert_eq!(rows.len(), 37);
        assert_eq!(
            rows.iter()
                .filter(|row| row.group == ExperimentGroup::Identity)
                .count(),
            10
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.group == ExperimentGroup::Mail)
                .count(),
            10
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.group == ExperimentGroup::Todo)
                .count(),
            8
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.group == ExperimentGroup::ReliabilityPrivacy)
                .count(),
            9
        );
    }

    #[test]
    fn every_experiment_id_is_unique() {
        let rows = matrix();
        let mut ids: Vec<String> = rows.iter().map(super::Experiment::id).collect();
        let before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), before);
    }
}
