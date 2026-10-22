//! Owner-invoked export of the currently loaded review scan into the `Jev`
//! optimizer's JSONL training schema (`tools/jev-optimize/jev_optimize/data.py`'s
//! `DatasetRow`). Pure and synchronous: takes a snapshot of [`ReviewState`]
//! and a destination folder, and writes `triage.jsonl`, `closure.jsonl`,
//! `walk.jsonl`, and `rules.jsonl`.
//!
//! This deliberately writes mail text (subjects, paragraphs, participants)
//! to disk after applying the Jev name cipher to recognized participants --
//! see `docs/inbox-review.md` and `tools/jev-optimize/README.md` for the
//! privacy note. Other names can remain in prose. Nothing here ever logs or
//! returns row content; callers may only surface row counts.

use crate::claim_view::{LoopItem, Owner, RecoveredClaimKind, SuggestedUpdateKind, claim_kind_of};
use crate::link_state::LinkKind;
use crate::name_cipher::NameCipher;
use crate::review_model::{
    ReviewMessage, ReviewState, closure_candidates, same_thread_closure_candidates,
};
use openloops_inference::decision::Questions;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// A paragraph longer than this is skipped entirely -- never becomes a row.
const MAX_PARAGRAPH_CHARS: usize = 4_000;
/// The walk asks at most the first eight individual paragraphs per message.
const MAX_WALK_PARAGRAPHS: usize = 8;
/// Each output file stops growing past this many rows.
const MAX_ROWS_PER_FILE: usize = 20_000;

/// Row counts written by [`export`] -- the only detail that may ever reach
/// the UI or a log; row content never does.
#[derive(Debug)]
pub struct ExportSummary {
    pub triage_rows: usize,
    pub closure_rows: usize,
    pub walk_rows: usize,
    pub rules_rows: usize,
    pub files_written: usize,
}

#[derive(Debug)]
pub enum ExportError {
    RelativePath,
    PathInRepo,
    Io(std::io::Error),
}

impl From<std::io::Error> for ExportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Writes ciphered `triage.jsonl`, `closure.jsonl`, `walk.jsonl`, and
/// `rules.jsonl` under `folder`.
///
/// # Errors
/// Refuses a relative `folder` ([`ExportError::RelativePath`]) or one
/// inside this repository ([`ExportError::PathInRepo`]), and surfaces any
/// filesystem error while creating the folder or writing either file.
pub fn export(review: &ReviewState, folder: &Path) -> Result<ExportSummary, ExportError> {
    if !folder.is_absolute() {
        return Err(ExportError::RelativePath);
    }
    if is_inside_repo(folder) {
        return Err(ExportError::PathInRepo);
    }
    std::fs::create_dir_all(folder)?;
    let cipher = NameCipher::from_messages(&review.messages);
    let triage = cipher_rows(triage_rows(review), &cipher);
    let closure = cipher_rows(closure_rows(review), &cipher);
    let walk = cipher_rows(walk_rows(review), &cipher);
    let rules = cipher_rows(rules_rows(review), &cipher);
    write_jsonl(&folder.join("triage.jsonl"), &triage)?;
    write_jsonl(&folder.join("closure.jsonl"), &closure)?;
    write_jsonl(&folder.join("walk.jsonl"), &walk)?;
    write_jsonl(&folder.join("rules.jsonl"), &rules)?;
    Ok(ExportSummary {
        triage_rows: triage.len(),
        closure_rows: closure.len(),
        walk_rows: walk.len(),
        rules_rows: rules.len(),
        files_written: 4,
    })
}

fn cipher_rows(rows: Vec<Value>, cipher: &NameCipher) -> Vec<Value> {
    rows.into_iter()
        .map(|row| cipher.encode_request(&row, &Questions::new()).state)
        .collect()
}

fn rules_rows(review: &ReviewState) -> Vec<Value> {
    let Some(analysis) = &review.analysis else {
        return Vec::new();
    };
    let cards = review.card_contexts(&analysis.items);
    let actions: std::collections::BTreeMap<[u8; 32], &str> = cards
        .iter()
        .zip(&analysis.items)
        .filter_map(|(card, item)| {
            card.as_ref()
                .map(|card| (card.record.key, item.action.as_str()))
        })
        .collect();
    let mut rows = Vec::new();
    for link in &review.relations.links {
        let duplicate = match link.kind {
            LinkKind::SameLoop => Some(true),
            LinkKind::NotSameLoop => Some(false),
            LinkKind::SameThread | LinkKind::NotSameThread => None,
        };
        if let Some(label) = duplicate
            && let (Some(action_a), Some(action_b)) = (actions.get(&link.a), actions.get(&link.b))
        {
            rows.push(rule_duplicate_row(action_a, action_b, label));
        }
    }
    let threads = thread_export_groups(review);
    for link in &review.relations.links {
        let merged = match link.kind {
            LinkKind::SameThread => Some(true),
            LinkKind::NotSameThread => Some(false),
            LinkKind::SameLoop | LinkKind::NotSameLoop => None,
        };
        if let Some(label) = merged
            && let (Some(a), Some(b)) = (threads.get(&link.a), threads.get(&link.b))
        {
            rows.push(rule_thread_row(a, b, label));
        }
    }
    rows.truncate(MAX_ROWS_PER_FILE);
    rows
}

fn rule_duplicate_row(action_a: &str, action_b: &str, duplicate: bool) -> Value {
    serde_json::json!({
        "action_a": action_a,
        "action_b": action_b,
        "id": stable_id(&format!("rules|duplicate|{action_a}|{action_b}")),
        "label": {"rules.duplicate_action": duplicate},
        "label_source": "gold",
        "set": "rules",
        "source": "owner",
    })
}

struct ThreadExport<'a> {
    subject: String,
    first_paragraph: String,
    marker: &'a str,
}

fn thread_export_groups(
    review: &ReviewState,
) -> std::collections::BTreeMap<[u8; 32], ThreadExport<'_>> {
    let mut groups: std::collections::BTreeMap<(String, String), Vec<&ReviewMessage>> =
        std::collections::BTreeMap::new();
    for message in &review.messages {
        groups
            .entry((message.account.clone(), message.conversation.clone()))
            .or_default()
            .push(message);
    }
    groups
        .into_iter()
        .filter_map(|((account, conversation), messages)| {
            let earliest = messages
                .into_iter()
                .min_by_key(|message| message.input.timestamp)?;
            let key = review.relations.thread_key(&account, &conversation);
            Some((
                key,
                ThreadExport {
                    subject: earliest.input.message.subject.as_string(),
                    first_paragraph: earliest.input.message.body_blocks.first().map_or_else(
                        String::new,
                        openloops_inference::blocks::CanonicalBlock::as_string,
                    ),
                    marker: earliest.input.handle.as_str(),
                },
            ))
        })
        .collect()
}

fn rule_thread_row(a: &ThreadExport<'_>, b: &ThreadExport<'_>, merged: bool) -> Value {
    serde_json::json!({
        "subject_a": a.subject,
        "subject_b": b.subject,
        "first_paragraph_a": a.first_paragraph,
        "first_paragraph_b": b.first_paragraph,
        "id": stable_id(&format!("rules|thread|{}|{}", a.marker, b.marker)),
        "label": {"rules.thread_merge": merged},
        "label_source": "gold",
        "set": "rules",
        "source": "owner",
    })
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("openloops-desktop lives two directories under the workspace root")
        .to_path_buf()
}

/// Case-insensitive, component-wise "is `folder` at or under the
/// repository root", walking up to the nearest existing ancestor first (a
/// not-yet-created export folder cannot be canonicalized directly).
fn is_inside_repo(folder: &Path) -> bool {
    let Ok(root) = repo_root().canonicalize() else {
        return false;
    };
    let mut probe = folder.to_path_buf();
    loop {
        if let Ok(canonical) = probe.canonicalize() {
            return path_under(&canonical, &root);
        }
        match probe.parent() {
            Some(parent) if parent != probe => probe = parent.to_path_buf(),
            _ => return false,
        }
    }
}

fn path_under(path: &Path, root: &Path) -> bool {
    let mut path_components = path.components();
    for root_component in root.components() {
        let Some(component) = path_components.next() else {
            return false;
        };
        let lower = component.as_os_str().to_string_lossy().to_lowercase();
        let root_lower = root_component.as_os_str().to_string_lossy().to_lowercase();
        if lower != root_lower {
            return false;
        }
    }
    true
}

fn hex_sha256(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    digest.iter().fold(String::new(), |mut hex, byte| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// `owner-<sha256 of the row's text fields>[:20]`, matching the Python
/// harness's own stable-id convention (see `data.py`'s `fetch_enron`).
fn stable_id(input: &str) -> String {
    format!("owner-{}", &hex_sha256(input)[..20])
}

fn write_jsonl(path: &Path, rows: &[Value]) -> Result<(), std::io::Error> {
    let file = std::fs::File::create(path)?;
    let mut writer = std::io::BufWriter::new(file);
    for row in rows {
        let text = serde_json::to_string(row).map_err(std::io::Error::other)?;
        writer.write_all(text.as_bytes())?;
        writer.write_all(b"\n")?;
    }
    writer.flush()
}

// --- Triage rows -----------------------------------------------------

fn triage_rows(review: &ReviewState) -> Vec<Value> {
    let Some(analysis) = &review.analysis else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for message in &review.messages {
        if rows.len() >= MAX_ROWS_PER_FILE {
            break;
        }
        let subject = message.input.message.subject.as_string();
        for (ordinal, block) in message.input.message.body_blocks.iter().enumerate() {
            if rows.len() >= MAX_ROWS_PER_FILE {
                break;
            }
            let text = block.as_string();
            if text.chars().count() > MAX_PARAGRAPH_CHARS {
                continue;
            }
            rows.push(triage_row(
                &subject,
                &text,
                message,
                ordinal,
                &analysis.items,
            ));
        }
    }
    rows
}

fn triage_row(
    subject: &str,
    paragraph_text: &str,
    message: &ReviewMessage,
    ordinal: usize,
    items: &[LoopItem],
) -> Value {
    let from_user = message.input.from_user;
    let label = triage_label(message, ordinal, items);

    let id_input = format!("triage|{subject}|{paragraph_text}|{from_user}");
    let mut row = Map::new();
    row.insert("from_user".into(), Value::Bool(from_user));
    row.insert("id".into(), Value::String(stable_id(&id_input)));
    row.insert("label".into(), Value::Object(label));
    row.insert(
        "paragraph_text".into(),
        Value::String(paragraph_text.into()),
    );
    row.insert("set".into(), Value::String("triage".into()));
    row.insert("source".into(), Value::String("owner-export".into()));
    row.insert("subject".into(), Value::String(subject.into()));
    Value::Object(row)
}

/// Silver triage labels derived from the accepted [`LoopItem`]s whose
/// evidence cites this exact `(message, ordinal)` paragraph. `boilerplate`
/// and `automated_notification` are never emitted -- the desktop's claim
/// model carries no signal for either.
fn triage_label(message: &ReviewMessage, ordinal: usize, items: &[LoopItem]) -> Map<String, Value> {
    let (mut asks_recipient, mut commits_sender, mut asks_question, mut names_time) =
        (false, false, false, false);
    for item in items {
        if item.evidence.message != message.input.handle || item.evidence.block != ordinal {
            continue;
        }
        let Some(kind) = claim_kind_of(item) else {
            continue;
        };
        match kind {
            RecoveredClaimKind::Request => asks_recipient = true,
            RecoveredClaimKind::Question => {
                asks_recipient = true;
                asks_question = true;
            }
            RecoveredClaimKind::Promise => commits_sender = true,
            RecoveredClaimKind::Delegation | RecoveredClaimKind::Attribution => {}
        }
        if item.deadline.is_some() {
            names_time = true;
        }
    }
    let mut label = Map::new();
    label.insert("triage.asks_question".into(), Value::Bool(asks_question));
    label.insert("triage.asks_recipient".into(), Value::Bool(asks_recipient));
    label.insert("triage.commits_sender".into(), Value::Bool(commits_sender));
    label.insert("triage.names_time".into(), Value::Bool(names_time));
    label
}

// --- Closure rows ------------------------------------------------------

fn is_offered_loop(item: &LoopItem) -> bool {
    item.resolution.is_none()
        && item.event_passed.is_none()
        && matches!(item.kind.as_str(), "request" | "promise")
        && item.owner == Owner::You
}

fn find_message<'a>(messages: &'a [ReviewMessage], handle: &str) -> Option<&'a ReviewMessage> {
    messages
        .iter()
        .find(|message| message.input.handle == handle)
}

/// Union of the same-thread and cross-thread closure candidate messages for
/// `item`'s obligation, deduplicated by handle. Reuses `review_scan`'s own
/// "keeps newest 8" enumeration rather than reimplementing it.
fn later_candidates<'a>(
    review: &'a ReviewState,
    item: &LoopItem,
    evidence_message: &ReviewMessage,
) -> Vec<&'a ReviewMessage> {
    let mut candidates = same_thread_closure_candidates(
        &review.messages,
        &evidence_message.account,
        &evidence_message.conversation,
        evidence_message.input.timestamp,
    );
    for extra in closure_candidates(
        item,
        &review.messages,
        &evidence_message.account,
        &evidence_message.conversation,
        evidence_message.input.timestamp,
    ) {
        if !candidates
            .iter()
            .any(|message| message.input.handle == extra.input.handle)
        {
            candidates.push(extra);
        }
    }
    candidates
}

fn closure_rows(review: &ReviewState) -> Vec<Value> {
    let Some(analysis) = &review.analysis else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for item in &analysis.items {
        if rows.len() >= MAX_ROWS_PER_FILE {
            break;
        }
        if !is_offered_loop(item) {
            continue;
        }
        let Some(evidence_message) = find_message(&review.messages, &item.evidence.message) else {
            continue;
        };
        for candidate in later_candidates(review, item, evidence_message) {
            append_closure_rows(review, item, evidence_message, candidate, &mut rows);
            if rows.len() >= MAX_ROWS_PER_FILE {
                break;
            }
        }
    }
    rows
}

fn append_closure_rows(
    review: &ReviewState,
    item: &LoopItem,
    evidence_message: &ReviewMessage,
    candidate: &ReviewMessage,
    rows: &mut Vec<Value>,
) {
    for (ordinal, block) in candidate.input.message.body_blocks.iter().enumerate() {
        if rows.len() >= MAX_ROWS_PER_FILE {
            return;
        }
        let text = block.as_string();
        if text.chars().count() > MAX_PARAGRAPH_CHARS {
            continue;
        }
        rows.push(closure_row(
            review,
            item,
            evidence_message,
            candidate,
            ordinal,
            &text,
        ));
    }
}

fn closure_row(
    review: &ReviewState,
    item: &LoopItem,
    evidence_message: &ReviewMessage,
    candidate: &ReviewMessage,
    ordinal: usize,
    paragraph_text: &str,
) -> Value {
    let from_user = candidate.input.from_user;
    let days_later = (candidate.input.timestamp - evidence_message.input.timestamp) / 86_400;
    let (label, label_source) = closure_label(review, item, candidate, ordinal);

    // The same two strings the closure pass sends to the decision model
    // (`title` = card title, `evidence_text` = the resolved evidence block),
    // so exported rows train the shape the app queries.
    let mut obligation = Map::new();
    obligation.insert(
        "evidence_text".into(),
        Value::String(item.evidence.context.clone()),
    );
    obligation.insert("title".into(), Value::String(item.action.clone()));

    let mut later = Map::new();
    later.insert("days_later".into(), Value::from(days_later));
    later.insert("from_user".into(), Value::Bool(from_user));
    later.insert(
        "paragraph_text".into(),
        Value::String(paragraph_text.into()),
    );

    let id_input = format!(
        "closure|{}|{}|{paragraph_text}|{from_user}|{days_later}",
        item.action, item.evidence.context
    );
    let mut row = Map::new();
    row.insert("id".into(), Value::String(stable_id(&id_input)));
    row.insert("label".into(), Value::Object(label));
    row.insert("label_source".into(), Value::String(label_source.into()));
    row.insert("later".into(), Value::Object(later));
    row.insert("obligation".into(), Value::Object(obligation));
    row.insert("set".into(), Value::String("closure".into()));
    row.insert("source".into(), Value::String("owner-export".into()));
    Value::Object(row)
}

/// Labels one `(loop, later paragraph)` pair. GOLD when the owner has
/// already accepted or rejected the suggestion that named this exact pair
/// ([`ReviewState::resolved_updates`]); SILVER from the chat model's still
/// pending suggestion when it names this pair; SILVER, all-`false`
/// otherwise.
fn closure_label(
    review: &ReviewState,
    item: &LoopItem,
    candidate: &ReviewMessage,
    ordinal: usize,
) -> (Map<String, Value>, &'static str) {
    let matches_pair = |source_message: &str, source_block: usize| {
        source_message == candidate.input.handle && source_block == ordinal
    };
    let gold = review.resolved_updates.iter().find(|update| {
        update.obligation_message == item.evidence.message
            && update.obligation_block == item.evidence.block
            && matches_pair(&update.source_message, update.source_block)
    });
    if let Some(resolved) = gold {
        let kind = resolved.accepted.then_some(resolved.kind);
        return (closure_label_map(kind), "gold");
    }
    if let Some(update) = &item.suggested_update
        && matches_pair(&update.source_message, update.source_block)
    {
        return (closure_label_map(Some(update.kind)), "silver");
    }
    (closure_label_map(None), "silver")
}

fn closure_label_map(kind: Option<SuggestedUpdateKind>) -> Map<String, Value> {
    let mut label = Map::new();
    label.insert(
        "closure.deadline_changed".into(),
        Value::Bool(kind == Some(SuggestedUpdateKind::DeadlineChange)),
    );
    label.insert(
        "closure.fulfilled".into(),
        Value::Bool(kind == Some(SuggestedUpdateKind::Closure)),
    );
    label.insert(
        "closure.modified".into(),
        Value::Bool(kind == Some(SuggestedUpdateKind::Modification)),
    );
    label.insert("closure.withdrawn".into(), Value::Bool(false));
    label
}

fn has_resolved_walk_update(review: &ReviewState, item: &LoopItem) -> bool {
    review.resolved_updates.iter().any(|update| {
        update.obligation_message == item.evidence.message
            && update.obligation_block == item.evidence.block
            && matches!(
                update.kind,
                SuggestedUpdateKind::Closure
                    | SuggestedUpdateKind::DeadlineChange
                    | SuggestedUpdateKind::Declined
                    | SuggestedUpdateKind::Delegated
            )
    })
}

fn walk_rows(review: &ReviewState) -> Vec<Value> {
    let Some(analysis) = &review.analysis else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for item in &analysis.items {
        if rows.len() >= MAX_ROWS_PER_FILE
            || (!is_offered_loop(item) && !has_resolved_walk_update(review, item))
        {
            continue;
        }
        let Some(evidence_message) = find_message(&review.messages, &item.evidence.message) else {
            continue;
        };
        for candidate in later_candidates(review, item, evidence_message) {
            let blocks: Vec<(usize, String)> = candidate
                .input
                .message
                .body_blocks
                .iter()
                .take(MAX_WALK_PARAGRAPHS)
                .enumerate()
                .map(|(ordinal, block)| (ordinal, block.as_string()))
                .collect();
            for (ordinal, text) in &blocks {
                if rows.len() >= MAX_ROWS_PER_FILE {
                    return rows;
                }
                if text.chars().count() > MAX_PARAGRAPH_CHARS {
                    continue;
                }
                rows.push(walk_row(
                    review,
                    item,
                    evidence_message,
                    candidate,
                    *ordinal,
                    text,
                    "paragraph",
                ));
            }
            if candidate.input.message.body_blocks.len() >= 2 {
                if rows.len() >= MAX_ROWS_PER_FILE {
                    return rows;
                }
                let text = candidate
                    .input
                    .message
                    .body_blocks
                    .iter()
                    .map(openloops_inference::blocks::CanonicalBlock::as_string)
                    .collect::<Vec<_>>()
                    .join("\n\n");
                rows.push(walk_row(
                    review,
                    item,
                    evidence_message,
                    candidate,
                    0,
                    &text,
                    "message",
                ));
            }
        }
    }
    rows
}

fn walk_row(
    review: &ReviewState,
    item: &LoopItem,
    evidence_message: &ReviewMessage,
    candidate: &ReviewMessage,
    ordinal: usize,
    text: &str,
    unit: &'static str,
) -> Value {
    let user_name = evidence_message
        .own_given_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .or(evidence_message
            .own_display_name
            .as_deref()
            .filter(|name| !name.trim().is_empty()))
        .unwrap_or("the user");
    let waiting_party = item
        .waiting_party
        .split_once('<')
        .map(|(name, _)| name.trim())
        .filter(|name| !name.is_empty())
        .unwrap_or(&item.waiting_party);
    let due = item
        .deadline
        .as_ref()
        .map_or("not stated", |deadline| deadline.quote.as_str());
    let group = stable_id(&format!(
        "walk|{}|{}",
        item.evidence.message, item.evidence.block
    ));
    let id = stable_id(&format!("walk|{group}|{unit}|{text}"));
    let (label, label_source) = walk_label(review, item, candidate, ordinal, unit);
    let mut later = Map::new();
    later.insert("author_role".into(), Value::String("user".into()));
    later.insert("text".into(), Value::String(text.into()));
    let mut row = Map::new();
    row.insert("ask".into(), Value::String(item.evidence.context.clone()));
    row.insert("due".into(), Value::String(due.into()));
    row.insert("group".into(), Value::String(group));
    row.insert("id".into(), Value::String(id));
    row.insert("label".into(), Value::Object(label));
    row.insert("label_source".into(), Value::String(label_source.into()));
    row.insert("later".into(), Value::Object(later));
    row.insert("set".into(), Value::String("walk".into()));
    row.insert("source".into(), Value::String("owner-export".into()));
    row.insert("unit".into(), Value::String(unit.into()));
    row.insert(
        "user".into(),
        Value::Object(Map::from_iter([(
            "name".into(),
            Value::String(user_name.into()),
        )])),
    );
    row.insert("waiting_party".into(), Value::String(waiting_party.into()));
    Value::Object(row)
}

fn walk_label(
    review: &ReviewState,
    item: &LoopItem,
    candidate: &ReviewMessage,
    ordinal: usize,
    unit: &'static str,
) -> (Map<String, Value>, &'static str) {
    let matches_pair = |source_message: &str, source_block: usize| {
        source_message == candidate.input.handle && source_block == ordinal
    };
    let gold = review.resolved_updates.iter().find(|update| {
        update.obligation_message == item.evidence.message
            && update.obligation_block == item.evidence.block
            && matches_pair(&update.source_message, update.source_block)
            && update.source_unit == unit
            && matches!(
                update.kind,
                SuggestedUpdateKind::Closure
                    | SuggestedUpdateKind::DeadlineChange
                    | SuggestedUpdateKind::Declined
                    | SuggestedUpdateKind::Delegated
            )
    });
    if let Some(resolved) = gold {
        return (
            walk_label_map(resolved.accepted.then_some(resolved.kind)),
            "gold",
        );
    }
    if let Some(update) = &item.suggested_update
        && matches_pair(&update.source_message, update.source_block)
        && update.source_unit == unit
        && matches!(
            update.kind,
            SuggestedUpdateKind::Closure
                | SuggestedUpdateKind::DeadlineChange
                | SuggestedUpdateKind::Declined
                | SuggestedUpdateKind::Delegated
        )
    {
        return (walk_label_map(Some(update.kind)), "silver");
    }
    (walk_label_map(None), "silver")
}

fn walk_label_map(kind: Option<SuggestedUpdateKind>) -> Map<String, Value> {
    let mut label = Map::new();
    label.insert(
        "walk.user_delivers".into(),
        Value::Bool(kind == Some(SuggestedUpdateKind::Closure)),
    );
    label.insert(
        "walk.user_promises".into(),
        Value::Bool(kind == Some(SuggestedUpdateKind::DeadlineChange)),
    );
    label.insert(
        "walk.user_declines".into(),
        Value::Bool(kind == Some(SuggestedUpdateKind::Declined)),
    );
    label.insert(
        "walk.user_hands_off".into(),
        Value::Bool(kind == Some(SuggestedUpdateKind::Delegated)),
    );
    label
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claim_view::SuggestedUpdate;
    use crate::review_model::layout_fixture;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FOLDER_SUFFIX: AtomicU64 = AtomicU64::new(0);

    fn export_to_temp(review: &ReviewState) -> (ExportSummary, PathBuf, PathBuf) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let suffix = NEXT_FOLDER_SUFFIX.fetch_add(1, Ordering::Relaxed);
        let folder = std::env::temp_dir().join(format!(
            "openloops-training-export-test-{}-{nanos}-{suffix}",
            std::process::id(),
        ));
        let summary = export(review, &folder).expect("export succeeds");
        (
            summary,
            folder.join("triage.jsonl"),
            folder.join("closure.jsonl"),
        )
    }

    fn read_rows(path: &Path) -> Vec<Value> {
        let text = std::fs::read_to_string(path).expect("read jsonl");
        text.lines()
            .map(|line| serde_json::from_str(line).expect("valid json line"))
            .collect()
    }

    #[test]
    fn rules_rows_carry_exactly_the_allowed_keys() {
        let mut review = layout_fixture();
        let cards = review.card_contexts(&review.analysis.as_ref().unwrap().items);
        let a = cards[0].as_ref().unwrap().record.key;
        let b = cards[1].as_ref().unwrap().record.key;
        review.relations.set(LinkKind::SameLoop, a, b).unwrap();
        let rows = rules_rows(&review);
        assert_eq!(rows.len(), 1);
        let object = rows[0].as_object().unwrap();
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "action_a",
                "action_b",
                "id",
                "label",
                "label_source",
                "set",
                "source"
            ]
        );
        assert_eq!(
            object["label"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            vec!["rules.duplicate_action"]
        );
    }

    #[test]
    fn a_same_loop_link_yields_gold_duplicate_rows() {
        let mut review = layout_fixture();
        let cards = review.card_contexts(&review.analysis.as_ref().unwrap().items);
        review
            .relations
            .set(
                LinkKind::SameLoop,
                cards[0].as_ref().unwrap().record.key,
                cards[1].as_ref().unwrap().record.key,
            )
            .unwrap();
        let rows = rules_rows(&review);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["label_source"], "gold");
        assert_eq!(rows[0]["source"], "owner");
        assert_eq!(rows[0]["set"], "rules");
        assert_eq!(rows[0]["label"]["rules.duplicate_action"], true);
    }

    #[test]
    fn triage_rows_carry_exactly_the_allowed_keys() {
        let review = layout_fixture();
        let (_summary, triage_path, _closure_path) = export_to_temp(&review);
        let rows = read_rows(&triage_path);
        assert!(!rows.is_empty());
        for row in &rows {
            let object = row.as_object().unwrap();
            let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                vec![
                    "from_user",
                    "id",
                    "label",
                    "paragraph_text",
                    "set",
                    "source",
                    "subject"
                ]
            );
            let label = object["label"].as_object().unwrap();
            for key in label.keys() {
                assert!(
                    matches!(
                        key.as_str(),
                        "triage.asks_question"
                            | "triage.asks_recipient"
                            | "triage.commits_sender"
                            | "triage.names_time"
                    ),
                    "unexpected triage label key {key}"
                );
            }
        }
        std::fs::remove_dir_all(triage_path.parent().unwrap()).ok();
    }

    #[test]
    fn closure_rows_always_carry_exactly_the_four_closure_keys() {
        let review = layout_fixture();
        let (_summary, triage_path, closure_path) = export_to_temp(&review);
        let rows = read_rows(&closure_path);
        for row in &rows {
            let object = row.as_object().unwrap();
            let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                vec![
                    "id",
                    "label",
                    "label_source",
                    "later",
                    "obligation",
                    "set",
                    "source"
                ]
            );
            let label = object["label"].as_object().unwrap();
            let mut label_keys: Vec<&str> = label.keys().map(String::as_str).collect();
            label_keys.sort_unstable();
            assert_eq!(
                label_keys,
                vec![
                    "closure.deadline_changed",
                    "closure.fulfilled",
                    "closure.modified",
                    "closure.withdrawn"
                ]
            );
        }
        std::fs::remove_dir_all(triage_path.parent().unwrap()).ok();
    }

    #[test]
    fn ids_are_stable_across_repeated_exports() {
        let review = layout_fixture();
        let (_summary, triage_path, closure_path) = export_to_temp(&review);
        let first_triage = read_rows(&triage_path);
        let first_closure = read_rows(&closure_path);
        let (_summary2, triage_path2, closure_path2) = export_to_temp(&review);
        let second_triage = read_rows(&triage_path2);
        let second_closure = read_rows(&closure_path2);
        let ids = |rows: &[Value]| -> Vec<String> {
            rows.iter()
                .map(|row| row["id"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(ids(&first_triage), ids(&second_triage));
        assert_eq!(ids(&first_closure), ids(&second_closure));
        std::fs::remove_dir_all(triage_path.parent().unwrap()).ok();
        std::fs::remove_dir_all(triage_path2.parent().unwrap()).ok();
    }

    /// Reshapes fixture item 0 into an open, owner-owed loop with a pending
    /// suggested closure naming a later, from-the-user message already in
    /// the same conversation -- the fixture's messages all share one
    /// timestamp, so the candidate's is bumped forward directly rather than
    /// relying on any baked ordering.
    fn open_loop_with_pending_update(review: &mut ReviewState) -> usize {
        let index = 0;
        let evidence_message = review.analysis.as_ref().unwrap().items[index]
            .evidence
            .message
            .clone();
        let evidence_timestamp = find_message(&review.messages, &evidence_message)
            .expect("fixture item 0 has a resolvable evidence message")
            .input
            .timestamp;
        let later_handle = review
            .messages
            .iter()
            .find(|message| message.input.from_user && message.input.handle != evidence_message)
            .expect("fixture has a user-sent message")
            .input
            .handle
            .clone();
        for message in &mut review.messages {
            if message.input.handle == later_handle {
                message.input.timestamp = evidence_timestamp + 2 * 86_400;
            }
        }
        let item = &mut review.analysis.as_mut().unwrap().items[index];
        item.resolution = None;
        item.event_passed = None;
        item.kind = "request".into();
        item.owner = Owner::You;
        item.suggested_update = Some(SuggestedUpdate {
            kind: SuggestedUpdateKind::Closure,
            handoff_text: None,
            source_unit: "paragraph",
            evidence_text: "Synthetic follow-up confirming completion.".into(),
            source_message: later_handle,
            source_block: 0,
            temporal_value: None,
            confidence_micros: 900_000,
        });
        index
    }

    #[test]
    fn a_rejected_suggested_update_yields_gold_all_false() {
        let mut review = layout_fixture();
        let index = open_loop_with_pending_update(&mut review);
        review.resolve_suggested_update(index, false);
        let (_summary, triage_path, closure_path) = export_to_temp(&review);
        let rows = read_rows(&closure_path);
        let gold_rows: Vec<&Value> = rows
            .iter()
            .filter(|row| row["label_source"] == "gold")
            .collect();
        assert_eq!(
            gold_rows.len(),
            1,
            "exactly one pair should carry the resolved gold label"
        );
        let label = gold_rows[0]["label"].as_object().unwrap();
        for value in label.values() {
            assert_eq!(value, &Value::Bool(false));
        }
        std::fs::remove_dir_all(triage_path.parent().unwrap()).ok();
    }

    #[test]
    fn an_accepted_closure_update_yields_gold_fulfilled() {
        let mut review = layout_fixture();
        let index = open_loop_with_pending_update(&mut review);
        let outcome = review.resolve_suggested_update(index, true);
        assert_eq!(outcome, crate::review_model::SuggestionOutcome::MarkHandled);
        let (_summary, triage_path, closure_path) = export_to_temp(&review);
        let rows = read_rows(&closure_path);
        let gold_rows: Vec<&Value> = rows
            .iter()
            .filter(|row| row["label_source"] == "gold")
            .collect();
        assert_eq!(
            gold_rows.len(),
            1,
            "exactly one pair should carry the resolved gold label"
        );
        let label = gold_rows[0]["label"].as_object().unwrap();
        assert_eq!(label["closure.fulfilled"], Value::Bool(true));
        assert_eq!(label["closure.withdrawn"], Value::Bool(false));
        assert_eq!(label["closure.deadline_changed"], Value::Bool(false));
        assert_eq!(label["closure.modified"], Value::Bool(false));
        std::fs::remove_dir_all(triage_path.parent().unwrap()).ok();
    }

    #[test]
    fn an_accepted_closure_update_yields_a_walk_shaped_gold_row() {
        let mut review = layout_fixture();
        let index = open_loop_with_pending_update(&mut review);
        let later_handle = review.analysis.as_ref().unwrap().items[index]
            .suggested_update
            .as_ref()
            .unwrap()
            .source_message
            .clone();
        let candidate = review
            .messages
            .iter_mut()
            .find(|message| message.input.handle == later_handle)
            .unwrap();
        candidate.input.message.body_blocks = vec![
            openloops_inference::blocks::CanonicalBlock::new("I sent the synthetic report.")
                .unwrap(),
            openloops_inference::blocks::CanonicalBlock::new("The receipt is attached.").unwrap(),
        ];
        review.resolve_suggested_update(index, true);
        let (summary, triage_path, closure_path) = export_to_temp(&review);
        let walk_path = closure_path.parent().unwrap().join("walk.jsonl");
        let rows = read_rows(&walk_path);
        let gold_rows: Vec<&Value> = rows
            .iter()
            .filter(|row| row["label_source"] == "gold")
            .collect();

        assert_eq!(summary.files_written, 4);
        assert_eq!(gold_rows.len(), 1);
        let row = gold_rows[0];
        assert_eq!(row["set"], "walk");
        assert_eq!(row["unit"], "paragraph");
        assert_eq!(row["label_source"], "gold");
        assert_eq!(row["source"], "owner-export");
        assert_eq!(row["later"]["author_role"], "user");
        assert_eq!(row["later"]["text"], "I sent the synthetic report.");
        assert_eq!(
            row["label"],
            serde_json::json!({
                "walk.user_delivers": true,
                "walk.user_promises": false,
                "walk.user_declines": false,
                "walk.user_hands_off": false,
            })
        );
        assert_eq!(
            row["group"],
            stable_id(&format!(
                "walk|{}|{}",
                review.analysis.as_ref().unwrap().items[index]
                    .evidence
                    .message,
                review.analysis.as_ref().unwrap().items[index]
                    .evidence
                    .block
            ))
        );
        assert_eq!(
            row.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            [
                "ask",
                "due",
                "group",
                "id",
                "label",
                "label_source",
                "later",
                "set",
                "source",
                "unit",
                "user",
                "waiting_party"
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(row["later"].as_object().unwrap().len(), 2);
        let group_rows: Vec<&Value> = rows
            .iter()
            .filter(|candidate| candidate["group"] == row["group"])
            .collect();
        assert_eq!(group_rows.len(), 3);
        assert!(group_rows.iter().any(|candidate| {
            candidate["unit"] == "message"
                && candidate["later"]["text"]
                    == "I sent the synthetic report.\n\nThe receipt is attached."
        }));
        std::fs::remove_dir_all(triage_path.parent().unwrap()).ok();
    }

    #[test]
    fn a_rejected_walk_suggestion_yields_gold_all_false() {
        let mut review = layout_fixture();
        let index = open_loop_with_pending_update(&mut review);
        review.resolve_suggested_update(index, false);
        let (_summary, triage_path, closure_path) = export_to_temp(&review);
        let walk_path = closure_path.parent().unwrap().join("walk.jsonl");
        let rows = read_rows(&walk_path);
        let gold_rows: Vec<&Value> = rows
            .iter()
            .filter(|row| row["label_source"] == "gold")
            .collect();
        assert_eq!(gold_rows.len(), 1);
        assert!(
            gold_rows[0]["label"]
                .as_object()
                .unwrap()
                .values()
                .all(|value| value == &Value::Bool(false))
        );
        std::fs::remove_dir_all(triage_path.parent().unwrap()).ok();
    }

    #[test]
    fn an_in_repo_path_is_refused() {
        let review = layout_fixture();
        let in_repo = repo_root().join("crates");
        let error = export(&review, &in_repo).expect_err("in-repo path refused");
        assert!(matches!(error, ExportError::PathInRepo));
    }

    #[test]
    fn a_relative_path_is_refused() {
        let review = layout_fixture();
        let error = export(&review, Path::new("relative/folder")).expect_err("relative refused");
        assert!(matches!(error, ExportError::RelativePath));
    }

    #[test]
    fn export_ciphers_known_people_in_every_training_file() {
        let mut review = layout_fixture();
        let message = &mut review.messages[0];
        message.input.message.subject =
            openloops_inference::blocks::CanonicalBlock::new("Rellian Voss asked for a document")
                .unwrap();
        message.input.message.sender = Some(
            openloops_inference::blocks::CanonicalBlock::new(
                "Rellian Voss <rellian.voss@example.invalid>",
            )
            .unwrap(),
        );
        message.input.message.body_blocks[0] = openloops_inference::blocks::CanonicalBlock::new(
            "Rellian Voss can be reached at rellian.voss@example.invalid.",
        )
        .unwrap();

        let (_summary, triage_path, closure_path) = export_to_temp(&review);
        let folder = triage_path.parent().unwrap().to_path_buf();
        let files = [triage_path, closure_path, folder.join("rules.jsonl")];
        for path in &files {
            let text = std::fs::read_to_string(path).expect("read exported data");
            assert!(!text.contains("Rellian"), "real given name in {path:?}");
            assert!(!text.contains("Voss"), "real family name in {path:?}");
            assert!(
                !text.contains("rellian.voss@example.invalid"),
                "real address in {path:?}"
            );
        }
        std::fs::remove_dir_all(folder).ok();
    }

    #[test]
    fn export_ciphers_known_people_in_walk_rows() {
        let mut review = layout_fixture();
        review.messages[0].input.message.sender = Some(
            openloops_inference::blocks::CanonicalBlock::new(
                "Rellian Voss <rellian.voss@example.invalid>",
            )
            .unwrap(),
        );
        let index = open_loop_with_pending_update(&mut review);
        let later_handle = review.analysis.as_ref().unwrap().items[index]
            .suggested_update
            .as_ref()
            .unwrap()
            .source_message
            .clone();
        let later = review
            .messages
            .iter_mut()
            .find(|message| message.input.handle == later_handle)
            .unwrap();
        later.input.message.body_blocks = vec![
            openloops_inference::blocks::CanonicalBlock::new(
                "I sent Rellian Voss the synthetic report at rellian.voss@example.invalid.",
            )
            .unwrap(),
        ];
        review.resolve_suggested_update(index, true);

        let (_summary, _triage_path, closure_path) = export_to_temp(&review);
        let folder = closure_path.parent().unwrap().to_path_buf();
        let walk_path = folder.join("walk.jsonl");
        let rows = read_rows(&walk_path);
        assert!(rows.iter().any(|row| {
            row["later"]["text"]
                .as_str()
                .is_some_and(|text| text.contains("the synthetic report"))
        }));
        let text = std::fs::read_to_string(&walk_path).expect("read exported walk rows");
        assert!(!text.contains("Rellian"), "real given name in walk.jsonl");
        assert!(!text.contains("Voss"), "real family name in walk.jsonl");
        assert!(
            !text.contains("rellian.voss@example.invalid"),
            "real address in walk.jsonl"
        );
        std::fs::remove_dir_all(folder).ok();
    }

    #[test]
    fn an_oversize_paragraph_is_skipped_without_failing_the_export() {
        let mut review = layout_fixture();
        let oversized = "x".repeat(MAX_PARAGRAPH_CHARS + 1);
        let block_count_before = review.messages[0].input.message.body_blocks.len();
        review.messages[0]
            .input
            .message
            .body_blocks
            .push(openloops_inference::blocks::CanonicalBlock::new(&oversized).unwrap());
        let (_summary, triage_path, _closure_path) = export_to_temp(&review);
        let rows = read_rows(&triage_path);
        assert!(
            rows.iter()
                .all(|row| row["paragraph_text"].as_str().unwrap().len() <= MAX_PARAGRAPH_CHARS)
        );
        assert_eq!(
            review.messages[0].input.message.body_blocks.len(),
            block_count_before + 1
        );
        std::fs::remove_dir_all(triage_path.parent().unwrap()).ok();
    }
}
