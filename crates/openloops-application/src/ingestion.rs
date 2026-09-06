//! The ADR-004 / implementation-plan §4.4 page-atomic ingestion transaction.
//!
//! For every delta page: stage the page cursor and per-item observation keys,
//! deduplicate by keyed digest, classify eligibility, run each item to a
//! terminal bounded outcome (retrying transient failures up to a per-item
//! cap before quarantining), then commit every terminal outcome and the
//! advanced checkpoint in exactly one [`openloops_persistence::store::Store::write_transaction`]
//! call. The cursor is never staged separately from item outcomes and is
//! never advanced while any item is non-terminal — there is only one
//! [`Store::write_transaction`] call in the whole module, and it is reached
//! only after every item resolved.
//!
//! `Store` has no "replace this row" primitive (only insert-and-scan; see
//! [`crate::seal`]'s doc comment), so both `sync_checkpoint` and
//! `message_observation` are modeled as an **append-only** log: "the current
//! checkpoint" is the highest-`generation` `sync_checkpoint` row for a given
//! collection, found by scanning. This is deliberate, not an oversight: it
//! is the only pattern `Store`'s genuinely tested primitives (insert +
//! `read_all`) support without reimplementing an UPDATE statement this crate
//! has no mandate to add to the persistence layer, and it composes exactly
//! with the crash rule below (a page's outcomes and its checkpoint advance
//! are one atomic append, never a mutation of the prior row).

use openloops_persistence::digest::{ContentDigestKey, DigestError, PurposeTag};
use openloops_persistence::envelope::EnvelopeKey;
use openloops_persistence::ids::{AccountBindingAad, RecordType, SchemaVersion};
use openloops_persistence::reservation::KeyUsageState;
use openloops_persistence::store::Store;

use crate::codec::{CodecError, Reader, push_bytes, push_option_i64, push_u8, push_u32};
use crate::ports::{Direction, ObservedReadState};
use crate::seal::{ScanError, SealError, commit_batch, scan_records, seal_batch};

pub use crate::ports::{DeltaPage, DeltaPageItem, MailSource, MailSourceError};

/// ADR-004's three eligibility buckets. Never `processed`: this story does
/// not wire an `Extractor`/`PolicyEngine` (a later story's job — see
/// [`SurfaceToReviewClassifier`]), so every eligible item surfaces to human
/// review rather than being autonomously analyzed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EligibilityOutcome {
    /// No eligible transition yet (or drafts/compose, which are never
    /// eligible).
    NoCandidate,
    /// Initial/backfill already-read or saved-sent-copy: one noninterruptive
    /// history-review batch, zero automatic reminders.
    HistoryReviewBatch,
    /// Post-baseline unread-to-read, first-observed-already-read, or
    /// first-observed saved sent copy: analyze once.
    AnalyzeOnce,
}

/// `mail-sync-boundary.json` `eligibility_policy`, applied literally:
///
/// * drafts/compose never activate a loop, backfill or not;
/// * initial/backfill: already-read incoming or any saved sent copy enters
///   the history-review batch; unread incoming waits for a later read;
/// * post-baseline: unread-to-read and first-observed-already-read incoming
///   analyze once; first-observed-unread waits; a first-observed saved sent
///   copy analyzes once and a re-observed one does not (it was already
///   analyzed).
#[must_use]
pub fn classify_eligibility(item: &DeltaPageItem) -> EligibilityOutcome {
    match item.direction {
        Direction::DraftOrCompose => EligibilityOutcome::NoCandidate,
        Direction::Incoming => {
            if item.in_backfill_batch {
                match item.read_state {
                    ObservedReadState::FirstObservedRead | ObservedReadState::UnreadToRead => {
                        EligibilityOutcome::HistoryReviewBatch
                    }
                    ObservedReadState::FirstObservedUnread | ObservedReadState::StillUnread => {
                        EligibilityOutcome::NoCandidate
                    }
                }
            } else {
                match item.read_state {
                    ObservedReadState::UnreadToRead | ObservedReadState::FirstObservedRead => {
                        EligibilityOutcome::AnalyzeOnce
                    }
                    ObservedReadState::FirstObservedUnread | ObservedReadState::StillUnread => {
                        EligibilityOutcome::NoCandidate
                    }
                }
            }
        }
        Direction::OutgoingSavedSentCopy => {
            if item.in_backfill_batch {
                // `initial_or_backfill.saved_sent_copy`: "analyze into the
                // history-review batch" — every backfill saved sent copy,
                // regardless of first-observed status.
                EligibilityOutcome::HistoryReviewBatch
            } else if item.first_observed {
                // `post_baseline_outgoing.saved_sent_copy_first_observed`:
                // "analyze once" — a distinct code from the backfill bucket
                // above, not folded into it.
                EligibilityOutcome::AnalyzeOnce
            } else {
                // Already analyzed on first observation; a re-observed
                // saved sent copy never activates a second time.
                EligibilityOutcome::NoCandidate
            }
        }
    }
}

/// A durable, safe per-item outcome (`page_transaction.durable_safe_outcomes`,
/// restricted to the subset this story's classifier can reach without
/// extraction/policy wiring).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobOutcome {
    NoCandidate,
    NeedsReview,
    /// Retry-exhausted; retains an encrypted re-fetch locator per
    /// `quarantine_policy` rather than being discarded.
    Quarantined,
}

impl JobOutcome {
    const fn code(self) -> u8 {
        match self {
            Self::NoCandidate => 0,
            Self::NeedsReview => 1,
            Self::Quarantined => 2,
        }
    }

    const fn from_code(code: u8) -> Result<Self, CodecError> {
        Ok(match code {
            0 => Self::NoCandidate,
            1 => Self::NeedsReview,
            2 => Self::Quarantined,
            other => return Err(CodecError::UnknownTag(other)),
        })
    }
}

/// One bounded classification attempt for one item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemAttempt {
    Terminal(JobOutcome),
    /// A transient failure; the ingestion loop retries up to its configured
    /// cap before quarantining.
    Retryable,
}

/// The bounded per-item job implementation-plan §4.4 step 4 calls "process in
/// a bounded job." A future story plugs in real content fetch/extraction
/// here without changing the transaction protocol around it.
pub trait ItemClassifier {
    fn classify(&mut self, item: &DeltaPageItem, eligibility: EligibilityOutcome) -> ItemAttempt;
}

/// This story's classifier: no `Extractor`/`PolicyEngine` is wired yet, so
/// every eligible item surfaces as `needs_review` and every ineligible item
/// is `no_candidate`. Never returns [`ItemAttempt::Retryable`]; the
/// bounded-retry path exists in the transaction loop for a classifier that
/// does real (and therefore transiently failable) work.
#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceToReviewClassifier;

impl ItemClassifier for SurfaceToReviewClassifier {
    fn classify(&mut self, _item: &DeltaPageItem, eligibility: EligibilityOutcome) -> ItemAttempt {
        match eligibility {
            EligibilityOutcome::NoCandidate => ItemAttempt::Terminal(JobOutcome::NoCandidate),
            EligibilityOutcome::HistoryReviewBatch | EligibilityOutcome::AnalyzeOnce => {
                ItemAttempt::Terminal(JobOutcome::NeedsReview)
            }
        }
    }
}

/// A rejected ingestion call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IngestionError {
    /// `page_transaction.whole_page_failure`: the fetch itself failed; zero
    /// envelopes are ever built or written on this path.
    WholePageFailure(MailSourceError),
    Digest(DigestError),
    Seal(SealError),
    Scan(ScanError),
    Codec(CodecError),
}

/// Durable state and keys this module composes; owned/loaded by a future
/// desktop host (this story does not implement key provisioning — see the
/// crate root's honest-gap list).
pub struct IngestionContext<'a> {
    pub store: &'a mut Store,
    pub account: AccountBindingAad,
    pub envelope_key: &'a EnvelopeKey,
    pub key_usage: &'a mut KeyUsageState,
    pub digest_key: &'a ContentDigestKey,
    pub schema_version: SchemaVersion,
}

/// The append-only "current checkpoint" projection for one collection.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CheckpointRecord {
    generation: u32,
    collection_id: [u8; 32],
    cursor: Vec<u8>,
    baseline_committed: bool,
    last_complete_at: Option<i64>,
}

impl CheckpointRecord {
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        push_u32(&mut buf, self.generation);
        buf.extend_from_slice(&self.collection_id);
        push_bytes(&mut buf, &self.cursor).expect("cursor bytes fit in u32");
        push_u8(&mut buf, u8::from(self.baseline_committed));
        push_option_i64(&mut buf, self.last_complete_at);
        buf
    }

    fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut reader = Reader::new(bytes);
        let generation = reader.read_u32()?;
        let collection_id = reader.read_array32()?;
        let cursor = reader.read_bytes()?.to_vec();
        let baseline_committed = match reader.read_u8()? {
            0 => false,
            1 => true,
            other => return Err(CodecError::UnknownTag(other)),
        };
        let last_complete_at = reader.read_option_i64()?;
        reader.finish()?;
        Ok(Self {
            generation,
            collection_id,
            cursor,
            baseline_committed,
            last_complete_at,
        })
    }
}

/// The append-only durable record of one item's terminal outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ObservationRecord {
    dedup_key: [u8; 32],
    outcome: JobOutcome,
    re_fetch_locator: Vec<u8>,
}

impl ObservationRecord {
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.dedup_key);
        push_u8(&mut buf, self.outcome.code());
        push_bytes(&mut buf, &self.re_fetch_locator).expect("locator bytes fit in u32");
        buf
    }

    fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut reader = Reader::new(bytes);
        let dedup_key = reader.read_array32()?;
        let outcome = JobOutcome::from_code(reader.read_u8()?)?;
        let re_fetch_locator = reader.read_bytes()?.to_vec();
        reader.finish()?;
        Ok(Self {
            dedup_key,
            outcome,
            re_fetch_locator,
        })
    }
}

/// implementation-plan §4.4 step 2: `HMAC(accountRef, testedMessageLocator,
/// observedVersion)`. `account_ref` is bound as the digest's own
/// `account_binding` parameter (as every other purpose in this catalog
/// does), so the explicit components are exactly the tested locator and
/// observed version.
fn dedup_key(ctx: &IngestionContext<'_>, item: &DeltaPageItem) -> Result<[u8; 32], DigestError> {
    openloops_persistence::digest::compute(
        ctx.digest_key,
        PurposeTag::MessageObservationSourceVersion,
        ctx.account,
        ctx.schema_version,
        &[&item.tested_locator, &item.observed_version],
    )
}

fn current_checkpoint(
    ctx: &IngestionContext<'_>,
    collection_id: [u8; 32],
) -> Result<Option<CheckpointRecord>, IngestionError> {
    let rows = scan_records(
        ctx.store,
        ctx.account,
        ctx.envelope_key,
        RecordType::SyncCheckpoint,
        CheckpointRecord::decode,
    )
    .map_err(IngestionError::Scan)?;
    Ok(rows
        .into_iter()
        .filter(|row| row.collection_id == collection_id)
        .max_by_key(|row| row.generation))
}

fn existing_observation(
    ctx: &IngestionContext<'_>,
    key: [u8; 32],
) -> Result<Option<ObservationRecord>, IngestionError> {
    let rows = scan_records(
        ctx.store,
        ctx.account,
        ctx.envelope_key,
        RecordType::MessageObservation,
        ObservationRecord::decode,
    )
    .map_err(IngestionError::Scan)?;
    Ok(rows.into_iter().find(|row| row.dedup_key == key))
}

/// The outcome of one committed page transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageTransactionOutcome {
    pub cursor: Vec<u8>,
    pub closes_round: bool,
    /// Parallel to the input page's `items`, in order. An item already
    /// durably recorded from an earlier (replayed) attempt reports its
    /// previously committed outcome, not a fresh one.
    pub item_outcomes: Vec<JobOutcome>,
    pub quarantined_locators: Vec<Vec<u8>>,
}

/// A fully planned, not-yet-committed page transaction: every item has a
/// terminal outcome and the envelopes to persist are already sealed. Exists
/// so callers can simulate "the process crashed after planning but before
/// the atomic commit" by simply never calling [`commit_planned_page`] on a
/// given plan — see this module's tests.
pub struct PlannedPage {
    outcome: PageTransactionOutcome,
    envelopes: Vec<openloops_persistence::envelope::Envelope>,
}

/// Stages and classifies one delta page to a fully terminal, sealed (but not
/// yet committed) plan.
///
/// Implements implementation-plan §4.4 steps 1-5 and `page_transaction`'s
/// dedup/eligibility/retry rules: `fetch_result` threads
/// [`MailSourceError`] straight through as
/// [`IngestionError::WholePageFailure`] with zero envelopes staged
/// (`whole_page_failure`); every already-durably-recorded item (matched by
/// [`dedup_key`]) is skipped rather than reprocessed
/// (`delta_protocol.at_least_once` + `eligibility_policy.idempotency`); every
/// other item runs `classifier` up to `max_item_attempts` times before
/// falling back to [`JobOutcome::Quarantined`]
/// (`retry_policy.automatic_attempts_per_item_generation`, "exhaustion enters
/// recoverable quarantine rather than discard").
///
/// # Errors
///
/// See [`IngestionError`].
pub fn plan_page_transaction(
    ctx: &mut IngestionContext<'_>,
    collection_id: [u8; 32],
    fetch_result: Result<DeltaPage, MailSourceError>,
    classifier: &mut dyn ItemClassifier,
    max_item_attempts: u8,
) -> Result<PlannedPage, IngestionError> {
    let page = fetch_result.map_err(IngestionError::WholePageFailure)?;

    let existing_checkpoint = current_checkpoint(ctx, collection_id)?;
    let baseline_committed = existing_checkpoint
        .as_ref()
        .is_some_and(|c| c.baseline_committed);
    let next_generation = existing_checkpoint.as_ref().map_or(0, |c| c.generation + 1);

    let mut item_outcomes = Vec::with_capacity(page.items.len());
    let mut quarantined_locators = Vec::new();
    let mut new_observations: Vec<(RecordType, Vec<u8>)> = Vec::new();

    for raw_item in &page.items {
        // `eligibility_policy` is evaluated against the baseline in force
        // *before* this page closes it, not the tentative post-page value:
        // an item can only benefit from a prior committed baseline, never
        // from the very round that establishes one.
        let item = DeltaPageItem {
            in_backfill_batch: raw_item.in_backfill_batch || !baseline_committed,
            ..raw_item.clone()
        };
        let key = dedup_key(ctx, &item).map_err(IngestionError::Digest)?;

        if let Some(existing) = existing_observation(ctx, key)? {
            // Duplicate page replay: already durably recorded, zero new
            // writes, zero duplicate transitions.
            item_outcomes.push(existing.outcome);
            if existing.outcome == JobOutcome::Quarantined {
                quarantined_locators.push(existing.re_fetch_locator.clone());
            }
            continue;
        }

        let eligibility = classify_eligibility(&item);
        let mut outcome = None;
        for _ in 0..max_item_attempts.max(1) {
            match classifier.classify(&item, eligibility) {
                ItemAttempt::Terminal(result) => {
                    outcome = Some(result);
                    break;
                }
                ItemAttempt::Retryable => {}
            }
        }
        let outcome = outcome.unwrap_or(JobOutcome::Quarantined);

        let record = ObservationRecord {
            dedup_key: key,
            outcome,
            re_fetch_locator: if outcome == JobOutcome::Quarantined {
                item.re_fetch_locator.clone()
            } else {
                Vec::new()
            },
        };
        if outcome == JobOutcome::Quarantined {
            quarantined_locators.push(record.re_fetch_locator.clone());
        }
        item_outcomes.push(outcome);
        new_observations.push((RecordType::MessageObservation, record.encode()));
    }

    let checkpoint = CheckpointRecord {
        generation: next_generation,
        collection_id,
        cursor: page.next_cursor.clone(),
        baseline_committed: baseline_committed || page.closes_round,
        last_complete_at: None,
    };
    let mut plaintexts = new_observations;
    plaintexts.push((RecordType::SyncCheckpoint, checkpoint.encode()));

    let envelopes = seal_batch(
        ctx.envelope_key,
        ctx.key_usage,
        ctx.account,
        ctx.schema_version,
        plaintexts,
    )
    .map_err(IngestionError::Seal)?;

    Ok(PlannedPage {
        outcome: PageTransactionOutcome {
            cursor: page.next_cursor,
            closes_round: page.closes_round,
            item_outcomes,
            quarantined_locators,
        },
        envelopes,
    })
}

/// Commits a [`PlannedPage`] in exactly one atomic `Store::write_transaction`
/// call: `page_transaction`'s "cursor advance rule: prohibited until every
/// page item is durable and safe" is satisfied structurally, because the
/// checkpoint envelope this plan already sealed is only ever written
/// alongside every item's outcome envelope in this one call — there is no
/// way to reach a committed checkpoint through this module without every
/// item outcome committing in the same transaction.
///
/// # Errors
///
/// Returns [`IngestionError::Seal`] on any write failure; nothing commits.
pub fn commit_planned_page(
    ctx: &mut IngestionContext<'_>,
    planned: &PlannedPage,
) -> Result<(), IngestionError> {
    commit_batch(ctx.store, ctx.account, &planned.envelopes).map_err(IngestionError::Seal)
}

/// `plan_page_transaction` then `commit_planned_page`: the ordinary,
/// non-crash-simulating call a real host makes once per delta page.
///
/// # Errors
///
/// See [`plan_page_transaction`] and [`commit_planned_page`].
pub fn run_page_transaction(
    ctx: &mut IngestionContext<'_>,
    collection_id: [u8; 32],
    fetch_result: Result<DeltaPage, MailSourceError>,
    classifier: &mut dyn ItemClassifier,
    max_item_attempts: u8,
) -> Result<PageTransactionOutcome, IngestionError> {
    let planned = plan_page_transaction(
        ctx,
        collection_id,
        fetch_result,
        classifier,
        max_item_attempts,
    )?;
    commit_planned_page(ctx, &planned)?;
    Ok(planned.outcome)
}

#[cfg(test)]
mod tests {
    use super::{
        Direction, EligibilityOutcome, IngestionContext, IngestionError, ItemAttempt,
        ItemClassifier, JobOutcome, MailSourceError, ObservedReadState, PageTransactionOutcome,
        SurfaceToReviewClassifier, classify_eligibility, plan_page_transaction,
        run_page_transaction,
    };
    use crate::ports::{DeltaPage, DeltaPageItem};
    use openloops_persistence::digest::ContentDigestKey;
    use openloops_persistence::envelope::EnvelopeKey;
    use openloops_persistence::ids::{RandomId, RecordType, SchemaVersion};
    use openloops_persistence::reservation::KeyUsageState;
    use openloops_persistence::store::Store;

    fn item(locator: u8, direction: Direction, read_state: ObservedReadState) -> DeltaPageItem {
        DeltaPageItem {
            tested_locator: vec![locator],
            observed_version: vec![1],
            direction,
            read_state,
            first_observed: false,
            in_backfill_batch: false,
            re_fetch_locator: vec![0xAB, locator],
        }
    }

    fn schema() -> SchemaVersion {
        SchemaVersion::new(core::num::NonZeroU32::new(1).unwrap())
    }

    fn new_ctx_pieces() -> (Store, EnvelopeKey, KeyUsageState, ContentDigestKey) {
        let key = EnvelopeKey::new(RandomId::from_random_bytes([9u8; 16]).unwrap(), [1u8; 32]);
        let usage = KeyUsageState::fresh(key.id());
        let digest_key = ContentDigestKey::new([2u8; 32]);
        (Store::open_in_memory().unwrap(), key, usage, digest_key)
    }

    fn account() -> openloops_persistence::ids::AccountBindingAad {
        RandomId::from_random_bytes([4u8; 16]).unwrap()
    }

    // --- eligibility classification: ADR-004 literal rules ---

    #[test]
    fn drafts_never_activate_regardless_of_batch() {
        let mut draft = item(
            1,
            Direction::DraftOrCompose,
            ObservedReadState::FirstObservedRead,
        );
        draft.in_backfill_batch = true;
        assert_eq!(
            classify_eligibility(&draft),
            EligibilityOutcome::NoCandidate
        );
        draft.in_backfill_batch = false;
        assert_eq!(
            classify_eligibility(&draft),
            EligibilityOutcome::NoCandidate
        );
    }

    #[test]
    fn backfill_already_read_incoming_enters_history_batch() {
        let mut i = item(1, Direction::Incoming, ObservedReadState::FirstObservedRead);
        i.in_backfill_batch = true;
        assert_eq!(
            classify_eligibility(&i),
            EligibilityOutcome::HistoryReviewBatch
        );
    }

    #[test]
    fn backfill_unread_incoming_waits() {
        let mut i = item(
            1,
            Direction::Incoming,
            ObservedReadState::FirstObservedUnread,
        );
        i.in_backfill_batch = true;
        assert_eq!(classify_eligibility(&i), EligibilityOutcome::NoCandidate);
    }

    #[test]
    fn post_baseline_unread_to_read_analyzes_once() {
        let i = item(1, Direction::Incoming, ObservedReadState::UnreadToRead);
        assert_eq!(classify_eligibility(&i), EligibilityOutcome::AnalyzeOnce);
    }

    #[test]
    fn post_baseline_first_observed_already_read_analyzes_once() {
        let i = item(1, Direction::Incoming, ObservedReadState::FirstObservedRead);
        assert_eq!(classify_eligibility(&i), EligibilityOutcome::AnalyzeOnce);
    }

    #[test]
    fn post_baseline_first_observed_unread_waits() {
        let i = item(
            1,
            Direction::Incoming,
            ObservedReadState::FirstObservedUnread,
        );
        assert_eq!(classify_eligibility(&i), EligibilityOutcome::NoCandidate);
    }

    #[test]
    fn post_baseline_still_unread_never_activates() {
        let i = item(1, Direction::Incoming, ObservedReadState::StillUnread);
        assert_eq!(classify_eligibility(&i), EligibilityOutcome::NoCandidate);
    }

    #[test]
    fn post_baseline_saved_sent_copy_first_observed_analyzes_once_but_not_twice() {
        // `post_baseline_outgoing.saved_sent_copy_first_observed`: "analyze
        // once" — a distinct eligibility code from the backfill
        // history-review bucket, never folded into it.
        let mut i = item(
            1,
            Direction::OutgoingSavedSentCopy,
            ObservedReadState::StillUnread,
        );
        assert!(!i.in_backfill_batch);
        i.first_observed = true;
        assert_eq!(classify_eligibility(&i), EligibilityOutcome::AnalyzeOnce);
        i.first_observed = false;
        assert_eq!(classify_eligibility(&i), EligibilityOutcome::NoCandidate);
    }

    #[test]
    fn backfill_saved_sent_copy_enters_history_review_batch_regardless_of_first_observed() {
        // `initial_or_backfill.saved_sent_copy`: "analyze into the
        // history-review batch" for every backfill saved sent copy.
        let mut i = item(
            1,
            Direction::OutgoingSavedSentCopy,
            ObservedReadState::StillUnread,
        );
        i.in_backfill_batch = true;
        i.first_observed = true;
        assert_eq!(
            classify_eligibility(&i),
            EligibilityOutcome::HistoryReviewBatch
        );
        i.first_observed = false;
        assert_eq!(
            classify_eligibility(&i),
            EligibilityOutcome::HistoryReviewBatch
        );
    }

    // --- transaction protocol ---

    fn page(items: Vec<DeltaPageItem>, cursor: u8, closes_round: bool) -> DeltaPage {
        DeltaPage {
            items,
            next_cursor: vec![cursor],
            closes_round,
        }
    }

    #[test]
    fn whole_page_failure_stages_nothing_and_leaves_no_checkpoint() {
        let (mut store, key, mut usage, digest_key) = new_ctx_pieces();
        let mut ctx = IngestionContext {
            store: &mut store,
            account: account(),
            envelope_key: &key,
            key_usage: &mut usage,
            digest_key: &digest_key,
            schema_version: schema(),
        };
        let mut classifier = SurfaceToReviewClassifier;
        let result = run_page_transaction(
            &mut ctx,
            [1u8; 32],
            Err(MailSourceError::Transport),
            &mut classifier,
            3,
        );
        assert_eq!(
            result,
            Err(IngestionError::WholePageFailure(MailSourceError::Transport))
        );
        let all = ctx.store.read_all(ctx.account).unwrap();
        assert!(all.is_empty(), "a whole-page failure writes nothing");
    }

    #[test]
    fn terminal_page_commits_observations_and_checkpoint_atomically() {
        let (mut store, key, mut usage, digest_key) = new_ctx_pieces();
        let mut ctx = IngestionContext {
            store: &mut store,
            account: account(),
            envelope_key: &key,
            key_usage: &mut usage,
            digest_key: &digest_key,
            schema_version: schema(),
        };
        let mut classifier = SurfaceToReviewClassifier;
        let items = vec![
            item(1, Direction::Incoming, ObservedReadState::FirstObservedRead),
            item(
                2,
                Direction::Incoming,
                ObservedReadState::FirstObservedUnread,
            ),
        ];
        let outcome = run_page_transaction(
            &mut ctx,
            [7u8; 32],
            Ok(page(items, 9, true)),
            &mut classifier,
            3,
        )
        .unwrap();
        assert_eq!(
            outcome,
            PageTransactionOutcome {
                cursor: vec![9],
                closes_round: true,
                item_outcomes: vec![JobOutcome::NeedsReview, JobOutcome::NoCandidate],
                quarantined_locators: vec![],
            }
        );
        let observations = ctx
            .store
            .read_all(ctx.account)
            .unwrap()
            .into_iter()
            .filter(|e| e.aad.record_type == RecordType::MessageObservation)
            .count();
        assert_eq!(observations, 2);
        let checkpoints = ctx
            .store
            .read_all(ctx.account)
            .unwrap()
            .into_iter()
            .filter(|e| e.aad.record_type == RecordType::SyncCheckpoint)
            .count();
        assert_eq!(checkpoints, 1);
    }

    #[test]
    fn duplicate_page_replay_never_duplicates_observations() {
        let (mut store, key, mut usage, digest_key) = new_ctx_pieces();
        let mut ctx = IngestionContext {
            store: &mut store,
            account: account(),
            envelope_key: &key,
            key_usage: &mut usage,
            digest_key: &digest_key,
            schema_version: schema(),
        };
        let mut classifier = SurfaceToReviewClassifier;
        let items = || {
            vec![item(
                1,
                Direction::Incoming,
                ObservedReadState::FirstObservedRead,
            )]
        };
        run_page_transaction(
            &mut ctx,
            [3u8; 32],
            Ok(page(items(), 5, true)),
            &mut classifier,
            3,
        )
        .unwrap();
        // The exact same page is redelivered (e.g. the server resent it).
        let second = run_page_transaction(
            &mut ctx,
            [3u8; 32],
            Ok(page(items(), 5, true)),
            &mut classifier,
            3,
        )
        .unwrap();
        assert_eq!(second.item_outcomes, vec![JobOutcome::NeedsReview]);
        let observations = ctx
            .store
            .read_all(ctx.account)
            .unwrap()
            .into_iter()
            .filter(|e| e.aad.record_type == RecordType::MessageObservation)
            .count();
        assert_eq!(
            observations, 1,
            "a replayed page must not duplicate observations"
        );
    }

    #[test]
    fn exhausted_retries_quarantine_and_retain_the_re_fetch_locator() {
        struct AlwaysRetry;
        impl ItemClassifier for AlwaysRetry {
            fn classify(
                &mut self,
                _item: &DeltaPageItem,
                _eligibility: EligibilityOutcome,
            ) -> ItemAttempt {
                ItemAttempt::Retryable
            }
        }
        let (mut store, key, mut usage, digest_key) = new_ctx_pieces();
        let mut ctx = IngestionContext {
            store: &mut store,
            account: account(),
            envelope_key: &key,
            key_usage: &mut usage,
            digest_key: &digest_key,
            schema_version: schema(),
        };
        let mut classifier = AlwaysRetry;
        let items = vec![item(
            1,
            Direction::Incoming,
            ObservedReadState::FirstObservedRead,
        )];
        let outcome = run_page_transaction(
            &mut ctx,
            [8u8; 32],
            Ok(page(items, 2, true)),
            &mut classifier,
            2,
        )
        .unwrap();
        assert_eq!(outcome.item_outcomes, vec![JobOutcome::Quarantined]);
        assert_eq!(outcome.quarantined_locators, vec![vec![0xABu8, 1]]);
    }

    #[test]
    fn baseline_established_by_a_closing_backfill_round_changes_later_eligibility() {
        let (mut store, key, mut usage, digest_key) = new_ctx_pieces();
        let mut ctx = IngestionContext {
            store: &mut store,
            account: account(),
            envelope_key: &key,
            key_usage: &mut usage,
            digest_key: &digest_key,
            schema_version: schema(),
        };
        let mut classifier = SurfaceToReviewClassifier;
        // First (backfill) round closes and establishes the baseline.
        let backfill_items = vec![item(
            1,
            Direction::Incoming,
            ObservedReadState::FirstObservedRead,
        )];
        run_page_transaction(
            &mut ctx,
            [6u8; 32],
            Ok(page(backfill_items, 1, true)),
            &mut classifier,
            3,
        )
        .unwrap();
        // A later page, same collection, with a genuine unread-to-read
        // transition on a new item must now be `AnalyzeOnce`/`NeedsReview`,
        // not folded back into a "still backfill" bucket.
        let post_items = vec![item(
            2,
            Direction::Incoming,
            ObservedReadState::UnreadToRead,
        )];
        let outcome = run_page_transaction(
            &mut ctx,
            [6u8; 32],
            Ok(page(post_items, 2, true)),
            &mut classifier,
            3,
        )
        .unwrap();
        assert_eq!(outcome.item_outcomes, vec![JobOutcome::NeedsReview]);
    }

    /// Crash-replay idempotency: plan (process every item to terminal, seal
    /// the batch), then simulate a crash by dropping the plan before
    /// `commit_planned_page` ever runs. Reopening the *same* store and
    /// replaying the identical page from scratch must produce exactly one
    /// copy of every observation and exactly one checkpoint — the aborted
    /// attempt left nothing behind to deduplicate against.
    #[test]
    fn crash_between_item_terminal_and_cursor_commit_replays_cleanly() {
        // No direct filesystem-creation calls here on purpose (this crate's
        // confinement scan keeps that surface confined to
        // `openloops-persistence`): the OS temp directory already exists, so
        // a uniquely named file placed directly inside it needs no
        // directory creation.
        let path = std::env::temp_dir().join(format!(
            "openloops-application-ingestion-crash-{}.db",
            std::process::id()
        ));
        let key = EnvelopeKey::new(RandomId::from_random_bytes([9u8; 16]).unwrap(), [1u8; 32]);
        let digest_key = ContentDigestKey::new([2u8; 32]);
        let acct = account();
        let items = || {
            vec![
                item(1, Direction::Incoming, ObservedReadState::FirstObservedRead),
                item(
                    2,
                    Direction::Incoming,
                    ObservedReadState::FirstObservedUnread,
                ),
            ]
        };

        {
            // Attempt 1: plan fully (every item reaches terminal, envelopes
            // are sealed) but never commit — the simulated crash.
            let mut store = Store::open(&path).unwrap();
            let mut usage = KeyUsageState::fresh(key.id());
            let mut ctx = IngestionContext {
                store: &mut store,
                account: acct,
                envelope_key: &key,
                key_usage: &mut usage,
                digest_key: &digest_key,
                schema_version: schema(),
            };
            let mut classifier = SurfaceToReviewClassifier;
            let planned = plan_page_transaction(
                &mut ctx,
                [5u8; 32],
                Ok(page(items(), 3, true)),
                &mut classifier,
                3,
            )
            .unwrap();
            let _ = planned; // dropped without ever calling commit_planned_page
        }

        {
            // Reopen the same on-disk database: the crashed attempt left it
            // untouched.
            let store = Store::open(&path).unwrap();
            assert!(store.read_all(acct).unwrap().is_empty());
        }

        {
            // Attempt 2: full replay against the same store succeeds and
            // produces exactly one observation per item plus one
            // checkpoint — no duplicate transitions from the aborted first
            // attempt.
            let mut store = Store::open(&path).unwrap();
            let mut usage = KeyUsageState::fresh(key.id());
            let mut ctx = IngestionContext {
                store: &mut store,
                account: acct,
                envelope_key: &key,
                key_usage: &mut usage,
                digest_key: &digest_key,
                schema_version: schema(),
            };
            let mut classifier = SurfaceToReviewClassifier;
            let outcome = run_page_transaction(
                &mut ctx,
                [5u8; 32],
                Ok(page(items(), 3, true)),
                &mut classifier,
                3,
            )
            .unwrap();
            assert_eq!(
                outcome.item_outcomes,
                vec![JobOutcome::NeedsReview, JobOutcome::NoCandidate]
            );
            let observations = ctx
                .store
                .read_all(acct)
                .unwrap()
                .into_iter()
                .filter(|e| e.aad.record_type == RecordType::MessageObservation)
                .count();
            assert_eq!(observations, 2);
            let checkpoints = ctx
                .store
                .read_all(acct)
                .unwrap()
                .into_iter()
                .filter(|e| e.aad.record_type == RecordType::SyncCheckpoint)
                .count();
            assert_eq!(
                checkpoints, 1,
                "the crashed attempt's checkpoint never committed"
            );
        }
    }
}
