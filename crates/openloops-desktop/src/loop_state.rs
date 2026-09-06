//! Bounded native-preview decision projection. Never stores readable source text.
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"OLDecisions\x01";
const CAP: usize = 50;
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
#[derive(Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub enum Reminder {
    #[default]
    None,
    Attempted,
    Created,
}
#[derive(Clone, Copy)]
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
    pub fn fingerprint(&self, account: &str, source: &str, quote: &str) -> [u8; 32] {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.secret.as_ref()).expect("fixed length HMAC key");
        for field in ["openloops-expectation-v1", account, source, quote] {
            mac.update(&(field.len() as u64).to_le_bytes());
            mac.update(field.as_bytes());
        }
        mac.finalize().into_bytes().into()
    }
    pub fn get(&self, key: &[u8; 32]) -> Record {
        self.records
            .iter()
            .find(|r| &r.key == key)
            .copied()
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
                || now() - r.updated < 30 * 86400
        });
        if let Some(existing) = self.records.iter_mut().find(|r| r.key == record.key) {
            *existing = record;
        } else {
            if self.records.len() >= CAP {
                self.records = old;
                return Err(
                    "The saved-decision limit (50) is reached. Existing decisions are preserved."
                        .into(),
                );
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
            Reminder::Created => 2,
        });
        bytes.extend_from_slice(&r.updated.to_le_bytes());
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
    if count > CAP || payload.len() != 33 + count * 42 {
        return Err(());
    }
    let mut records = vec![];
    for row in payload[33..].chunks_exact(42) {
        let key = row[..32].try_into().map_err(|_| ())?;
        if records.iter().any(|r: &Record| r.key == key) {
            return Err(());
        }
        records.push(Record {
            key,
            decision: match row[32] {
                0 => Decision::Review,
                1 => Decision::Mine,
                2 => Decision::Done,
                3 => Decision::Dismissed,
                4 => Decision::Watching,
                5 => Decision::Moot,
                _ => return Err(()),
            },
            reminder: match row[33] {
                0 => Reminder::None,
                1 => Reminder::Attempted,
                2 => Reminder::Created,
                _ => return Err(()),
            },
            updated: i64::from_le_bytes(row[34..42].try_into().map_err(|_| ())?),
        });
    }
    Ok((secret, records))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_records_expire_unless_a_reminder_is_present() {
        for decision in [Decision::Done, Decision::Dismissed, Decision::Moot] {
            for reminder in [Reminder::None, Reminder::Attempted, Reminder::Created] {
                let mut state = Decisions::default();
                state.records.push(Record {
                    key: [7; 32],
                    decision,
                    reminder,
                    updated: now() - 31 * 86400,
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
                    reminder != Reminder::None
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
                reminder: Reminder::Created,
                updated: 1_788_350_400,
            };
            let bytes = encode(&[0; 32], &[record]).unwrap();
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
    fn decisions_replay_without_readable_mail_and_are_account_bound() {
        let mut state = Decisions::default();
        let key = state.fingerprint("account-a", "source", "Please send the draft.");
        assert_ne!(
            key,
            state.fingerprint("account-b", "source", "Please send the draft.")
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
        let key = state.fingerprint("synthetic-account", "synthetic-source", "send the draft");
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
        let key = state.fingerprint("synthetic-account", "synthetic-source", "send the draft");
        assert_eq!(state.get(&key).decision, Decision::Watching);
        assert!(state.begin_reminder(key).is_err());
    }
}
