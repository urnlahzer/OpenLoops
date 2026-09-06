/**
 * Typed card projection model for the Outlook add-in review UI.
 *
 * Mirrors `crates/openloops-domain/src/facets.rs` and
 * `crates/openloops-domain/src/display.rs` conceptually — the same closed
 * catalogs, in the same contract order, and the same primary-label
 * precedence algorithm (`docs/product-spec.md` §5.1's exact eight-row
 * table) — but is hand-written, not generated, per this story's brief.
 * Nothing here fetches, stores, or caches anything: every function is a
 * pure, render-only transform over an already-supplied projection (OL-UX-006:
 * no mailbox content of any kind in browser storage).
 */

/** `facet_catalogs.obligation_state_code`. */
export type ObligationState = "candidate" | "open" | "terminal";

/** `facet_catalogs.closure_review_state_code`. */
export type ClosureReviewState =
  "none" | "possible" | "kept_open" | "evidence_requested";

/** `facet_catalogs.deadline_state_code`. */
export type DeadlineState =
  "unresolved" | "undated_confirmed" | "scheduled" | "approaching" | "overdue";

/** `facet_catalogs.review_flag_codes`. */
export type ReviewFlagCode =
  | "needs_review"
  | "historical_backfill"
  | "identity_ambiguous"
  | "association_ambiguous"
  | "quote_ambiguous"
  | "deadline_ambiguous"
  | "delegation_ambiguous";

/** `facet_catalogs.provenance_codes`. */
export type ProvenanceCode =
  | "requested"
  | "acknowledged"
  | "promised"
  | "attributed"
  | "inferred"
  | "ambiguous";

/** `facet_catalogs.analysis_state_code`. */
export type AnalysisState =
  "current" | "queued" | "unavailable" | "quarantined" | "stale";

/** `facet_catalogs.source_state_code`. */
export type SourceState =
  "available" | "partially_available" | "unavailable" | "changed";

/** `facet_catalogs.reminder_state_code`. */
export type ReminderState =
  | "none"
  | "proposed"
  | "pending_write"
  | "linked"
  | "changed"
  | "completed_needs_evidence"
  | "missing"
  | "conflict"
  | "ambiguous_write";

/**
 * The single primary display label, product-spec §5.1's exact eight-row
 * precedence table (index 0 is highest precedence) — one-to-one with
 * `openloops_domain::facets::PrimaryLabel::ALL`.
 */
export type PrimaryLabel =
  | "terminal_resolution"
  | "possible_closure"
  | "needs_review"
  | "overdue"
  | "approaching_deadline"
  | "needs_deadline"
  | "candidate"
  | "active";

/** `PrimaryLabel::ALL`, in strict precedence order. */
export const PRIMARY_LABEL_PRECEDENCE: readonly PrimaryLabel[] = [
  "terminal_resolution",
  "possible_closure",
  "needs_review",
  "overdue",
  "approaching_deadline",
  "needs_deadline",
  "candidate",
  "active",
];

/** The facets [`primaryLabel`](#primaryLabel) needs to resolve one card's primary label. */
export interface DisplayFacets {
  readonly obligationState: ObligationState;
  readonly closureReviewState: ClosureReviewState;
  readonly deadlineState: DeadlineState;
  readonly reviewFlags: readonly ReviewFlagCode[];
  readonly analysisState: AnalysisState;
  readonly sourceState: SourceState;
  readonly reminderState: ReminderState;
}

function analysisRequiresAction(analysis: AnalysisState): boolean {
  return analysis === "quarantined";
}

function sourceRequiresAction(source: SourceState): boolean {
  return (
    source === "changed" ||
    source === "unavailable" ||
    source === "partially_available"
  );
}

function reminderRequiresAction(reminder: ReminderState): boolean {
  return (
    reminder === "conflict" ||
    reminder === "ambiguous_write" ||
    reminder === "missing" ||
    reminder === "changed"
  );
}

/**
 * Resolves the single primary display label for `facets`, exactly
 * reproducing `openloops_domain::display::primary_label`'s precedence order
 * (`docs/product-spec.md` §5.1).
 */
export function primaryLabel(facets: DisplayFacets): PrimaryLabel {
  if (facets.obligationState === "terminal") {
    return "terminal_resolution";
  }
  if (facets.closureReviewState === "possible") {
    return "possible_closure";
  }
  const needsReview =
    facets.reviewFlags.length > 0 ||
    analysisRequiresAction(facets.analysisState) ||
    sourceRequiresAction(facets.sourceState) ||
    reminderRequiresAction(facets.reminderState);
  if (needsReview) {
    return "needs_review";
  }
  if (facets.deadlineState === "overdue") {
    return "overdue";
  }
  if (facets.deadlineState === "approaching") {
    return "approaching_deadline";
  }
  if (facets.deadlineState === "unresolved") {
    return "needs_deadline";
  }
  if (facets.obligationState === "candidate") {
    return "candidate";
  }
  return "active";
}

/**
 * Secondary chips: product-spec §5.1: "The UI may show several secondary
 * chips ... Reminder, evidence, deadline, and analysis states remain
 * visible as secondary chips even when a higher-precedence primary label
 * applies." Independent of, and always alongside, the primary label.
 */
export interface SecondaryChips {
  readonly provenance: readonly ProvenanceCode[];
  readonly reminderState: ReminderState;
  readonly sourceState: SourceState;
  readonly analysisState: AnalysisState;
}

/**
 * The approved operative deadline for one loop. `epochMillis` is `null`
 * exactly when no operative deadline exists yet (`deadlineState` is
 * `"unresolved"` or `"undated_confirmed"`) — the UI must render "No
 * deadline" in that case rather than fabricate one (OL-DUE-007).
 */
export interface DeadlineProjection {
  readonly state: DeadlineState;
  readonly epochMillis: number | null;
}

/**
 * One rendered card's complete typed projection: everything
 * `review.ts`'s pure builders need, and nothing else. Generated-adjacent to
 * `openloops_domain::display`/`facets`, hand-written per this story's
 * brief.
 */
export interface CardProjection {
  /** An opaque loop handle only — never a raw Microsoft/Graph identifier. */
  readonly loopId: string;
  readonly primaryLabel: PrimaryLabel;
  readonly secondaryChips: SecondaryChips;
  readonly operativeDeadline: DeadlineProjection;
}
