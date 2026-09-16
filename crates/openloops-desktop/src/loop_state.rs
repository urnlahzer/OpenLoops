//! Bounded native-preview decision projection. Never stores readable source text.
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"OLDecisions\x02";
/// The previous format's magic, kept only so `open()` can tell "this is the
/// known pre-v2 fingerprint scheme being retired" apart from genuine
/// corruption -- see `open()`'s decode-failure handling.
const OLD_MAGIC: &[u8] = b"OLDecisions\x01";
/// Windows' generic-credential blob is hard-capped at 2560 bytes
/// (`CRED_MAX_CREDENTIAL_BLOB_SIZE`, `5 * CRED_MAX_STRING_LENGTH`) -- this is
/// an OS ceiling on `entry.set_secret()`'s payload, not a policy choice, and
/// `encode()` enforces it directly. With the fixed-size row (`MAGIC` +
/// `secret`(32) + count(1) + `CAP` * 42-byte rows), the true maximum that can
/// ever fit is `(2560 - MAGIC.len() - 33) / 42` ~= 59 records -- there is no
/// room to raise this materially without moving off a single Windows
/// credential blob (e.g. onto `openloops-persistence`'s SQLite store, or
/// splitting across multiple named credentials), which is out of scope here.
/// 55 leaves a small margin below that hard ceiling.
const CAP: usize = 55;
/// How long a terminal decision (`Done`/`Dismissed`/`Moot`) with no reminder
/// attached is kept before it ages out of the saved set. Generous rather than
/// unbounded, so the `CAP` rejection in `update()` stays rare under ordinary
/// use while still bounding the store's size.
const TERMINAL_EXPIRY_SECONDS: i64 = 180 * 86400;
/// Bound on each Graph id stored in `Reminder::Created`, chosen to fit a
/// single-byte length prefix (real Microsoft Graph To Do list/task ids are
/// consistently well under this). Given the same 2560-byte credential-blob
/// ceiling `CAP` is sized against, storing two such ids alongside every
/// record is not something a raised `CAP` could also absorb; a save that
/// would exceed the blob size still fails safely via `encode()`'s existing
/// check (no corruption, an "unavailable" error instead of the friendlier
/// "limit reached" one) rather than being separately guarded against here.
const MAX_REMINDER_ID_LEN: usize = 255;
const TARGET: &str = "OpenLoops/Decisions/v1";

#[derive(Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub enum Decision {
    #[default]
    Review,
    Mine,
    Done,
    Dismissed,
    Watching,
    Moot,
}
#[derive(Clone, Default, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub enum Reminder {
    #[default]
    None,
    Attempted,
    /// Carries the Graph task-list and task id `reminders::create()`
    /// resolved, so a later action (marking it complete once the card is
    /// Handled) can address the same task -- see `reminders::complete()`.
    /// Each id is bounded to `MAX_REMINDER_ID_LEN` bytes for storage; see
    /// that constant for why.
    Created { list_id: String, task_id: String },
    /// Set only by the reminder-sync check (`reminders::check_status`)
    /// finding the linked task already completed in Microsoft To Do --
    /// never by the user clicking Handled, which leaves `reminder` as
    /// `Created` even after `reminders::complete()` succeeds. This is the
    /// one thing that distinguishes "the user marked this handled" from
    /// "completing the task externally marked this handled" for
    /// `status_pill_hint`, since the decision alone (`Done` either way)
    /// can't say which.
    Completed { list_id: String, task_id: String },
}
#[derive(Clone)]
pub struct Record {
    pub key: [u8; 32],
    pub decision: Decision,
    pub reminder: Reminder,
    pub updated: i64,
}

pub struct Decisions {
    secret: Zeroizing<[u8; 32]>,
    pub records: Vec<Record>,
    entry: Option<keyring_core::Entry>,
    previous: Zeroizing<Vec<u8>>,
    pub error: Option<String>,
}

fn failure() -> String {
    "Saved loop decisions are unavailable or changed in another window. Close the other window and reopen OpenLoops; your previous decisions have been preserved.".into()
}

impl Default for Decisions {
    fn default() -> Self {
        Self::open(if cfg!(any(test, feature = "ui-screenshot")) {
            None
        } else {
            Some(TARGET)
        })
    }
}

impl Decisions {
    fn open(target: Option<&str>) -> Self {
        let mut result = Self {
            secret: Zeroizing::new([0; 32]),
            records: vec![],
            entry: None,
            previous: Zeroizing::new(vec![]),
            error: None,
        };
        if getrandom::fill(&mut *result.secret).is_err() {
            result.error = Some(failure());
            return result;
        }
        let Some(target) = target else {
            return result;
        };
        #[cfg(windows)]
        {
            use keyring_core::api::CredentialStoreApi;
            let entry = windows_native_keyring_store::Store::new()
                .ok()
                .and_then(|store| {
                    store
                        .build(
                            "OpenLoops",
                            "decisions",
                            Some(&std::collections::HashMap::from([
                                ("target", target),
                                ("persistence", "Local"),
                            ])),
                        )
                        .ok()
                });
            if let Some(entry) = entry {
                let loaded = entry.get_secret();
                result.entry = Some(entry);
                match loaded {
                    Ok(bytes) => {
                        result.previous = Zeroizing::new(bytes);
                        match decode(&result.previous) {
                            Ok((secret, records)) => {
                                result.secret = Zeroizing::new(secret);
                                result.records = records;
                            }
                            // The fingerprint scheme changed (action_phrase ->
                            // evidence block+text), so a v1 store's keys would
                            // never match a v2 lookup anyway even if the bytes
                            // were readable: reset to empty rather than
                            // surfacing a persistent error banner for what is
                            // an intentional, one-time format retirement, not
                            // corruption. Any other decode failure (wrong
                            // length, bad tag, truncated) still means
                            // corruption and keeps the hard error.
                            Err(()) if result.previous.starts_with(OLD_MAGIC) => {}
                            Err(()) => result.error = Some(failure()),
                        }
                    }
                    Err(keyring_core::Error::NoEntry) => {
                        if result.save().is_err() {
                            result.error = Some(failure());
                        }
                    }
                    Err(_) => result.error = Some(failure()),
                }
            } else {
                result.error = Some(failure());
            }
        }
        #[cfg(not(windows))]
        {
            result.error = Some(failure());
        }
        result
    }
}

impl Decisions {
    pub fn begin_reminder(&mut self, key: [u8; 32]) -> Result<(), String> {
        let mut record = self.get(&key);
        if !matches!(record.decision, Decision::Mine | Decision::Watching)
            || record.reminder != Reminder::None
        {
            return Err("Confirm responsibility or choose Keep an eye on this before creating a reminder. An existing or uncertain reminder must be resolved first.".into());
        }
        record.reminder = Reminder::Attempted;
        record.updated = now();
        self.update(record)
    }
    /// `block` and `quote` must be the expectation's own evidence anchor
    /// (`Expectation.evidence.block`/`.quote`) -- a canonicalizer-normalized,
    /// exact-match-verified substring of the source message, never something
    /// else entirely. `action_phrase` must be the expectation's own
    /// `action_phrase` (`expectations.rs::action_phrase_from` already
    /// guarantees it is itself an exact substring of `quote`, so it is real
    /// anchored text too, not free-generated prose) -- it stays part of the
    /// key because two independent actions can share one evidence quote
    /// (e.g. "Please send the budget and schedule the meeting." backing both
    /// "send the budget" and "schedule the meeting" -- see
    /// `changed_summary_preserves_evidence_identity_but_two_actions_stay_
    /// distinct` in `expectations.rs`); dropping it would silently merge two
    /// different saved decisions. Both are normalized here
    /// (case/punctuation/whitespace-insensitive), so trivial reformatting
    /// between scans still resolves to the same fingerprint.
    ///
    /// What this does NOT fix: if a later scan anchors to a materially
    /// different (but still valid) substring for the same real action --
    /// e.g. "send the draft budget" vs. "send the budget" -- the key still
    /// changes, exactly as it did before this change. Closing that gap needs
    /// the evidence model itself to carry a stable sub-quote identity (a
    /// scalar range, not free text), which today's `Anchor` doesn't; it's
    /// tracked as future work, not solved by this fingerprint change alone.
    pub fn fingerprint(
        &self,
        account: &str,
        source: &str,
        block: usize,
        quote: &str,
        action_phrase: &str,
    ) -> [u8; 32] {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.secret.as_ref()).expect("fixed length HMAC key");
        let block = block.to_string();
        let quote = normalize_for_fingerprint(quote);
        let action_phrase = normalize_for_fingerprint(action_phrase);
        for field in [
            "openloops-expectation-v2",
            account,
            source,
            &block,
            &quote,
            &action_phrase,
        ] {
            mac.update(&(field.len() as u64).to_le_bytes());
            mac.update(field.as_bytes());
        }
        mac.finalize().into_bytes().into()
    }
    pub fn get(&self, key: &[u8; 32]) -> Record {
        self.records
            .iter()
            .find(|r| &r.key == key)
            .cloned()
            .unwrap_or(Record {
                key: *key,
                decision: Decision::Review,
                reminder: Reminder::None,
                updated: 0,
            })
    }
    pub fn update(&mut self, record: Record) -> Result<(), String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let old = self.records.clone();
        self.records.retain(|r| {
            r.reminder != Reminder::None
                || !matches!(
                    r.decision,
                    Decision::Done | Decision::Dismissed | Decision::Moot
                )
                || now() - r.updated < TERMINAL_EXPIRY_SECONDS
        });
        if let Some(existing) = self.records.iter_mut().find(|r| r.key == record.key) {
            *existing = record;
        } else {
            if self.records.len() >= CAP {
                self.records = old;
                return Err(format!(
                    "The saved-decision limit ({CAP}) is reached. Existing decisions are preserved."
                ));
            }
            self.records.push(record);
        }
        if self.save().is_err() {
            self.records = old;
            self.error = Some(failure());
            return Err(failure());
        }
        Ok(())
    }
    fn save(&mut self) -> Result<(), ()> {
        let bytes = encode(&self.secret, &self.records)?;
        if let Some(entry) = &self.entry {
            // One OS-protected, empty lock file serializes the credential CAS.
            // It contains no record, account identifier, key or mailbox content.
            let _writer = writer_lock()?;
            let current = match entry.get_secret() {
                Ok(b) => Zeroizing::new(b),
                Err(keyring_core::Error::NoEntry) => Zeroizing::new(vec![]),
                Err(_) => return Err(()),
            };
            if *current != *self.previous {
                return Err(());
            }
            entry.set_secret(&bytes).map_err(|_| ())?;
        } else if !cfg!(any(test, feature = "ui-screenshot")) {
            return Err(());
        }
        self.previous = bytes;
        Ok(())
    }
}

#[cfg(windows)]
fn writer_lock() -> Result<std::fs::File, ()> {
    exclusive_writer_file(&std::env::temp_dir().join("OpenLoops-decision-writer.lock"))
}

#[cfg(windows)]
fn exclusive_writer_file(path: &std::path::Path) -> Result<std::fs::File, ()> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
        .map_err(|_| ())
}

#[cfg(not(windows))]
fn writer_lock() -> Result<std::fs::File, ()> {
    Err(())
}

/// Lowercases, strips ASCII punctuation, and collapses whitespace runs, so
/// two fingerprint calls for the same evidence text differing only in case,
/// punctuation, or incidental spacing still land on the same key.
fn normalize_for_fingerprint(quote: &str) -> String {
    quote
        .chars()
        .map(|c| if c.is_ascii_punctuation() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}
pub fn marker(key: &[u8; 32]) -> String {
    use std::fmt::Write;
    key.iter().fold(String::with_capacity(64), |mut text, b| {
        let _ = write!(text, "{b:02x}");
        text
    })
}

/// Appends one Graph id as a single-byte length prefix followed by its
/// bytes. Fails rather than silently truncating an id over
/// `MAX_REMINDER_ID_LEN` -- callers must reject or shorten it upstream
/// instead (see `reminders::valid_graph_id`, which already bounds ids this
/// tightly is not required there, so this is the actual enforcement point).
fn push_id(bytes: &mut Vec<u8>, id: &str) -> Result<(), ()> {
    let id = id.as_bytes();
    if id.len() > MAX_REMINDER_ID_LEN {
        return Err(());
    }
    bytes.push(u8::try_from(id.len()).map_err(|_| ())?);
    bytes.extend_from_slice(id);
    Ok(())
}
/// Reads one `push_id`-encoded id, returning it and the remaining bytes.
fn read_id(rest: &[u8]) -> Result<(String, &[u8]), ()> {
    let len = usize::from(*rest.first().ok_or(())?);
    let rest = &rest[1..];
    let (id, rest) = rest.split_at_checked(len).ok_or(())?;
    Ok((String::from_utf8(id.to_vec()).map_err(|_| ())?, rest))
}

fn encode(secret: &[u8; 32], records: &[Record]) -> Result<Zeroizing<Vec<u8>>, ()> {
    if records.len() > CAP {
        return Err(());
    }
    let mut bytes = Zeroizing::new(MAGIC.to_vec());
    bytes.extend_from_slice(secret);
    bytes.push(u8::try_from(records.len()).map_err(|_| ())?);
    for r in records {
        bytes.extend_from_slice(&r.key);
        bytes.push(match r.decision {
            Decision::Review => 0,
            Decision::Mine => 1,
            Decision::Done => 2,
            Decision::Dismissed => 3,
            Decision::Watching => 4,
            Decision::Moot => 5,
        });
        bytes.push(match r.reminder {
            Reminder::None => 0,
            Reminder::Attempted => 1,
            Reminder::Created { .. } => 2,
            Reminder::Completed { .. } => 3,
        });
        bytes.extend_from_slice(&r.updated.to_le_bytes());
        match &r.reminder {
            Reminder::Created { list_id, task_id } | Reminder::Completed { list_id, task_id } => {
                push_id(&mut bytes, list_id)?;
                push_id(&mut bytes, task_id)?;
            }
            Reminder::None | Reminder::Attempted => {}
        }
    }
    if bytes.len() > 2560 {
        return Err(());
    }
    Ok(bytes)
}
fn decode(bytes: &[u8]) -> Result<([u8; 32], Vec<Record>), ()> {
    let payload = bytes.strip_prefix(MAGIC).ok_or(())?;
    let secret = payload.get(..32).ok_or(())?.try_into().map_err(|_| ())?;
    let count = usize::from(*payload.get(32).ok_or(())?);
    if count > CAP {
        return Err(());
    }
    let mut rest = payload.get(33..).ok_or(())?;
    let mut records = vec![];
    for _ in 0..count {
        let (row, remaining) = rest.split_at_checked(42).ok_or(())?;
        let key: [u8; 32] = row[..32].try_into().map_err(|_| ())?;
        if records.iter().any(|r: &Record| r.key == key) {
            return Err(());
        }
        let decision = match row[32] {
            0 => Decision::Review,
            1 => Decision::Mine,
            2 => Decision::Done,
            3 => Decision::Dismissed,
            4 => Decision::Watching,
            5 => Decision::Moot,
            _ => return Err(()),
        };
        let reminder_tag = row[33];
        let updated = i64::from_le_bytes(row[34..42].try_into().map_err(|_| ())?);
        rest = remaining;
        let reminder = match reminder_tag {
            0 => Reminder::None,
            1 => Reminder::Attempted,
            2 | 3 => {
                let (list_id, remaining) = read_id(rest)?;
                let (task_id, remaining) = read_id(remaining)?;
                rest = remaining;
                if reminder_tag == 2 {
                    Reminder::Created { list_id, task_id }
                } else {
                    Reminder::Completed { list_id, task_id }
                }
            }
            _ => return Err(()),
        };
        records.push(Record {
            key,
            decision,
            reminder,
            updated,
        });
    }
    if !rest.is_empty() {
        return Err(());
    }
    Ok((secret, records))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_records_expire_unless_a_reminder_is_present() {
        for decision in [Decision::Done, Decision::Dismissed, Decision::Moot] {
            for reminder in [
                Reminder::None,
                Reminder::Attempted,
                Reminder::Created {
                    list_id: "list".into(),
                    task_id: "task".into(),
                },
            ] {
                let has_reminder = !matches!(reminder, Reminder::None);
                let mut state = Decisions::default();
                state.records.push(Record {
                    key: [7; 32],
                    decision,
                    reminder,
                    updated: now() - (TERMINAL_EXPIRY_SECONDS + 86400),
                });
                state
                    .update(Record {
                        key: [8; 32],
                        decision: Decision::Review,
                        reminder: Reminder::None,
                        updated: now(),
                    })
                    .unwrap();
                assert_eq!(
                    state.records.iter().any(|r| r.key == [7; 32]),
                    has_reminder
                );
            }
        }
    }

    #[test]
    fn every_decision_round_trips_with_stable_tags() {
        for (tag, decision) in [
            Decision::Review,
            Decision::Mine,
            Decision::Done,
            Decision::Dismissed,
            Decision::Watching,
            Decision::Moot,
        ]
        .into_iter()
        .enumerate()
        {
            let record = Record {
                key: [7; 32],
                decision,
                reminder: Reminder::Created {
                    list_id: "list".into(),
                    task_id: "task".into(),
                },
                updated: 1_788_350_400,
            };
            let bytes = encode(&[0; 32], &[record.clone()]).unwrap();
            assert_eq!(usize::from(bytes[MAGIC.len() + 33 + 32]), tag);
            let (_, records) = decode(&bytes).unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].key, record.key);
            assert_eq!(records[0].decision, record.decision);
            assert_eq!(records[0].reminder, record.reminder);
            assert_eq!(records[0].updated, record.updated);
        }
    }
    #[test]
    fn fingerprint_is_insensitive_to_case_punctuation_and_incidental_whitespace() {
        // The concrete, guaranteed improvement this fix makes: two scans that
        // land on the exact same evidence text but differ in trivial
        // formatting (case, punctuation, double spaces) still reattach.
        // Genuine rewording -- the model choosing a materially different
        // valid substring of the same sentence as action_phrase -- still
        // changes the key; that residual gap needs a richer evidence model
        // (a scalar range, not free text) to close fully, tracked as future
        // work rather than solved here.
        let state = Decisions::default();
        let clean = state.fingerprint(
            "acct",
            "msg-1",
            0,
            "Please send the draft budget by Friday.",
            "send the draft budget",
        );
        let reformatted = state.fingerprint(
            "acct",
            "msg-1",
            0,
            "please   send the draft budget by friday",
            "Send The Draft Budget!",
        );
        assert_eq!(
            clean, reformatted,
            "case/punctuation/whitespace differences alone must not change the key"
        );
    }
    #[test]
    fn fingerprint_still_distinguishes_two_actions_sharing_one_quote() {
        // Regression guard: dropping action_phrase entirely would merge two
        // genuinely different expectations that cite the same evidence quote
        // (see expectations.rs's
        // changed_summary_preserves_evidence_identity_but_two_actions_stay_distinct).
        let state = Decisions::default();
        let shared_quote = "Please send the budget and schedule the meeting.";
        let first_action = state.fingerprint("acct", "msg-1", 0, shared_quote, "send the budget");
        let second_action =
            state.fingerprint("acct", "msg-1", 0, shared_quote, "schedule the meeting");
        assert_ne!(
            first_action, second_action,
            "two distinct actions sharing one evidence quote must not collide"
        );
    }
    #[test]
    fn decisions_replay_without_readable_mail_and_are_account_bound() {
        let mut state = Decisions::default();
        let key = state.fingerprint("account-a", "source", 0, "Please send the draft.", "phrase");
        assert_ne!(
            key,
            state.fingerprint("account-b", "source", 0, "Please send the draft.", "phrase")
        );
        state
            .update(Record {
                key,
                decision: Decision::Dismissed,
                reminder: Reminder::None,
                updated: now(),
            })
            .unwrap();
        let bytes = encode(&state.secret, &state.records).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("draft"));
        let (_, records) = decode(&bytes).unwrap();
        assert_eq!(records[0].decision, Decision::Dismissed);
        for n in 0..bytes.len() {
            assert!(decode(&bytes[..n]).is_err());
        }
    }
    #[test]
    fn ambiguous_attempt_survives_retention_and_codec_rejects_bad_codes() {
        let mut state = Decisions::default();
        let key = [7; 32];
        state
            .update(Record {
                key,
                decision: Decision::Done,
                reminder: Reminder::Attempted,
                updated: 1,
            })
            .unwrap();
        state
            .update(Record {
                key: [8; 32],
                decision: Decision::Review,
                reminder: Reminder::None,
                updated: now(),
            })
            .unwrap();
        assert_eq!(state.records.len(), 2);
        let mut bytes = encode(&state.secret, &state.records).unwrap();
        bytes[MAGIC.len() + 33 + 33] = 9;
        assert!(decode(&bytes).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn native_decisions_survive_reopen_and_block_duplicate_reminders() {
        let target = format!("OpenLoops/TestDecisions/{}-{}", std::process::id(), now());
        let mut state = Decisions::open(Some(&target));
        assert!(state.error.is_none());
        let key = state.fingerprint("synthetic-account", "synthetic-source", 0, "send the draft", "phrase");
        state
            .update(Record {
                key,
                decision: Decision::Watching,
                reminder: Reminder::None,
                updated: now(),
            })
            .unwrap();
        state.begin_reminder(key).unwrap();
        assert!(state.begin_reminder(key).is_err());
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "loop_state::tests::read_decisions_in_new_process",
                "--ignored",
            ])
            .env("OPENLOOPS_TEST_DECISIONS", &target)
            .output()
            .unwrap();
        let result =
            child.status.success() && String::from_utf8_lossy(&child.stdout).contains("1 passed");
        let reopened = Decisions::open(Some(&target));
        assert_eq!(reopened.get(&key).reminder, Reminder::Attempted);
        reopened
            .entry
            .as_ref()
            .unwrap()
            .delete_credential()
            .unwrap();
        assert!(result, "isolated fresh-process decision check failed");
    }

    #[cfg(windows)]
    #[test]
    fn only_one_native_writer_can_hold_the_credential_write_lock() {
        let path =
            std::env::temp_dir().join(format!("OpenLoops-test-writer-{}.lock", std::process::id()));
        let first = exclusive_writer_file(&path).unwrap();
        assert!(exclusive_writer_file(&path).is_err());
        drop(first);
        assert!(exclusive_writer_file(&path).is_ok());
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "parent test supplies isolated native credential"]
    fn read_decisions_in_new_process() {
        let target = std::env::var("OPENLOOPS_TEST_DECISIONS").unwrap();
        assert!(target.starts_with("OpenLoops/TestDecisions/"));
        let mut state = Decisions::open(Some(&target));
        let key = state.fingerprint("synthetic-account", "synthetic-source", 0, "send the draft", "phrase");
        assert_eq!(state.get(&key).decision, Decision::Watching);
        assert!(state.begin_reminder(key).is_err());
    }
}
