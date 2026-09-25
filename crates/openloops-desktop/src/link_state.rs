//! Bounded owner-taught relationships. Stores fingerprints, never readable mail.
use crate::loop_state::{Decisions, LoopKeys, now, writer_lock};
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"OLRelations\x01";
const CAP: usize = 34;
const TARGET: &str = "OpenLoops/Relations/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    SameLoop = 0,
    NotSameLoop = 1,
    SameThread = 2,
    NotSameThread = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub kind: LinkKind,
    pub a: [u8; 32],
    pub b: [u8; 32],
    pub updated: i64,
}

pub struct Relations {
    keys: LoopKeys,
    pub links: Vec<Link>,
    entry: Option<keyring_core::Entry>,
    previous: Zeroizing<Vec<u8>>,
    pub error: Option<String>,
}

fn failure() -> String {
    "Saved links are unavailable or changed in another window. Close the other window and reopen OpenLoops; your previous links have been preserved.".into()
}

impl Relations {
    pub fn open_with(decisions: &Decisions) -> Self {
        Self::open(
            decisions.keys(),
            if cfg!(any(test, feature = "ui-screenshot")) {
                None
            } else {
                Some(TARGET)
            },
        )
    }

    fn open(keys: LoopKeys, target: Option<&str>) -> Self {
        let mut result = Self {
            keys,
            links: Vec::new(),
            entry: None,
            previous: Zeroizing::new(Vec::new()),
            error: None,
        };
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
                            "relations",
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
                            Ok(links) => result.links = links,
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
            let _ = target;
            result.error = Some(failure());
        }
        result
    }

    pub fn thread_key(&self, account: &str, conversation: &str) -> [u8; 32] {
        self.keys
            .domain_key(&["openloops-conversation-v1", account, conversation])
    }

    pub fn set(&mut self, kind: LinkKind, a: [u8; 32], b: [u8; 32]) -> Result<(), String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if a == b {
            return self.remove(a, b);
        }
        let (a, b) = canonical(a, b);
        let old = self.links.clone();
        if self.get(a, b).is_some() {
            let link = self
                .links
                .iter_mut()
                .find(|link| link.a == a && link.b == b)
                .expect("get found the canonical pair");
            *link = Link {
                kind,
                a,
                b,
                updated: now(),
            };
        } else {
            if self.links.len() >= CAP {
                return Err(
                    "The saved-link limit (34) is reached. Existing links are preserved.".into(),
                );
            }
            self.links.push(Link {
                kind,
                a,
                b,
                updated: now(),
            });
        }
        if self.save().is_err() {
            self.links = old;
            self.error = Some(failure());
            return Err(failure());
        }
        Ok(())
    }

    pub fn remove(&mut self, a: [u8; 32], b: [u8; 32]) -> Result<(), String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let (a, b) = canonical(a, b);
        let old = self.links.clone();
        self.links.retain(|link| link.a != a || link.b != b);
        if self.save().is_err() {
            self.links = old;
            self.error = Some(failure());
            return Err(failure());
        }
        Ok(())
    }

    pub fn get(&self, a: [u8; 32], b: [u8; 32]) -> Option<LinkKind> {
        let (a, b) = canonical(a, b);
        self.links
            .iter()
            .find(|link| link.a == a && link.b == b)
            .map(|link| link.kind)
    }

    pub fn pairs(&self, kind: LinkKind) -> Vec<([u8; 32], [u8; 32])> {
        self.links
            .iter()
            .filter(|link| link.kind == kind)
            .map(|link| (link.a, link.b))
            .collect()
    }

    pub fn linked_to(&self, key: &[u8; 32], kind: LinkKind) -> Vec<[u8; 32]> {
        self.links
            .iter()
            .filter(|link| link.kind == kind && (&link.a == key || &link.b == key))
            .map(|link| if &link.a == key { link.b } else { link.a })
            .collect()
    }

    fn save(&mut self) -> Result<(), ()> {
        let bytes = encode(&self.links)?;
        if let Some(entry) = &self.entry {
            let _writer = writer_lock("OpenLoops-relations-writer.lock")?;
            let current = match entry.get_secret() {
                Ok(bytes) => Zeroizing::new(bytes),
                Err(keyring_core::Error::NoEntry) => Zeroizing::new(Vec::new()),
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

fn canonical(a: [u8; 32], b: [u8; 32]) -> ([u8; 32], [u8; 32]) {
    if a < b { (a, b) } else { (b, a) }
}

fn encode(links: &[Link]) -> Result<Zeroizing<Vec<u8>>, ()> {
    if links.len() > CAP {
        return Err(());
    }
    let mut bytes = Zeroizing::new(MAGIC.to_vec());
    bytes.push(u8::try_from(links.len()).map_err(|_| ())?);
    for link in links {
        bytes.push(link.kind as u8);
        bytes.extend_from_slice(&link.a);
        bytes.extend_from_slice(&link.b);
        bytes.extend_from_slice(&link.updated.to_le_bytes());
    }
    if bytes.len() > 2560 {
        return Err(());
    }
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> Result<Vec<Link>, ()> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if bytes.len() > 2560 {
        return Err(());
    }
    let payload = bytes.strip_prefix(MAGIC).ok_or(())?;
    let count = usize::from(*payload.first().ok_or(())?);
    if count > CAP {
        return Err(());
    }
    let mut rest = &payload[1..];
    let mut links = Vec::with_capacity(count);
    for _ in 0..count {
        let (row, remaining) = rest.split_at_checked(73).ok_or(())?;
        let kind = match row[0] {
            0 => LinkKind::SameLoop,
            1 => LinkKind::NotSameLoop,
            2 => LinkKind::SameThread,
            3 => LinkKind::NotSameThread,
            _ => return Err(()),
        };
        let a = row[1..33].try_into().map_err(|_| ())?;
        let b = row[33..65].try_into().map_err(|_| ())?;
        if a >= b || links.iter().any(|link: &Link| link.a == a && link.b == b) {
            return Err(());
        }
        links.push(Link {
            kind,
            a,
            b,
            updated: i64::from_le_bytes(row[65..73].try_into().map_err(|_| ())?),
        });
        rest = remaining;
    }
    if !rest.is_empty() {
        return Err(());
    }
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_replay_without_readable_mail_and_thread_keys_are_account_bound() {
        let decisions = Decisions::default();
        let mut relations = Relations::open_with(&decisions);
        let a = decisions.fingerprint("account-a", "message-a", 0, "Synthetic ask", "ask");
        let b = decisions.fingerprint("account-a", "message-b", 0, "Synthetic reply", "reply");
        relations.set(LinkKind::SameLoop, a, b).unwrap();
        assert_ne!(
            relations.thread_key("account-a", "thread"),
            relations.thread_key("account-b", "thread")
        );
        let bytes = encode(&relations.links).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("Synthetic"));
        assert_eq!(decode(&bytes).unwrap(), relations.links);
        for length in 1..bytes.len() {
            assert!(decode(&bytes[..length]).is_err());
        }
    }

    #[test]
    fn every_link_kind_round_trips_with_stable_tags() {
        for (tag, kind) in [
            LinkKind::SameLoop,
            LinkKind::NotSameLoop,
            LinkKind::SameThread,
            LinkKind::NotSameThread,
        ]
        .into_iter()
        .enumerate()
        {
            let link = Link {
                kind,
                a: [1; 32],
                b: [2; 32],
                updated: 1_788_350_400,
            };
            let bytes = encode(&[link]).unwrap();
            assert_eq!(usize::from(bytes[MAGIC.len() + 1]), tag);
            assert_eq!(decode(&bytes).unwrap(), vec![link]);
        }
    }

    #[test]
    fn set_replaces_the_pair_and_caps_at_34() {
        let decisions = Decisions::default();
        let mut relations = Relations::open_with(&decisions);
        relations.set(LinkKind::SameLoop, [1; 32], [2; 32]).unwrap();
        relations
            .set(LinkKind::NotSameLoop, [2; 32], [1; 32])
            .unwrap();
        assert_eq!(relations.links.len(), 1);
        assert_eq!(relations.get([1; 32], [2; 32]), Some(LinkKind::NotSameLoop));
        for value in 3..=35 {
            relations
                .set(LinkKind::SameLoop, [0; 32], [value; 32])
                .unwrap();
        }
        assert_eq!(relations.links.len(), CAP);
        assert!(
            relations
                .set(LinkKind::SameLoop, [36; 32], [37; 32])
                .is_err()
        );
        assert_eq!(relations.links.len(), CAP);
    }

    #[cfg(windows)]
    #[test]
    fn native_links_survive_reopen() {
        let target = format!("OpenLoops/TestRelations/{}-{}", std::process::id(), now());
        let decisions = Decisions::default();
        let mut relations = Relations::open(decisions.keys(), Some(&target));
        relations.set(LinkKind::SameLoop, [1; 32], [2; 32]).unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "link_state::tests::read_links_in_new_process",
                "--ignored",
            ])
            .env("OPENLOOPS_TEST_RELATIONS", &target)
            .output()
            .unwrap();
        let result =
            child.status.success() && String::from_utf8_lossy(&child.stdout).contains("1 passed");
        let reopened = Relations::open(decisions.keys(), Some(&target));
        assert_eq!(reopened.get([1; 32], [2; 32]), Some(LinkKind::SameLoop));
        reopened
            .entry
            .as_ref()
            .unwrap()
            .delete_credential()
            .unwrap();
        assert!(result, "isolated fresh-process relation check failed");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "parent test supplies isolated native credential"]
    fn read_links_in_new_process() {
        let target = std::env::var("OPENLOOPS_TEST_RELATIONS").unwrap();
        assert!(target.starts_with("OpenLoops/TestRelations/"));
        let decisions = Decisions::default();
        let relations = Relations::open(decisions.keys(), Some(&target));
        assert_eq!(relations.get([1; 32], [2; 32]), Some(LinkKind::SameLoop));
    }
}
