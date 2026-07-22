/**
 * Pure render-model builders for the add-in review queue: queue ordering,
 * card view-model assembly, and typed correction/command intents.
 *
 * Nothing here executes anything — G-ADDIN's bridge is gated, so every
 * intent this module builds is inert data a future story hands to a bridge,
 * never a call this module makes itself. Every builder is a pure function
 * of its arguments: no `Date.now()`, no default `Intl`/locale lookups, and
 * no I/O — deadline formatting and "now" are always caller-injected, so
 * output is fully deterministic under test.
 */

import type {
  CardProjection,
  DeadlineProjection,
  PrimaryLabel,
  ProvenanceCode,
  ReminderState,
  SourceState,
} from "./projection.js";

/**
 * The review-queue lanes: exactly the `PrimaryLabel` values that ever
 * surface into the review queue, in the same precedence order as
 * `projection.ts`'s `PRIMARY_LABEL_PRECEDENCE` (`terminal_resolution` and
 * `active` never appear here — a terminal loop is closed, and an `active`
 * loop needs no review).
 */
export type ReviewLane =
  | "possible_closure"
  | "needs_review"
  | "overdue"
  | "approaching_deadline"
  | "needs_deadline"
  | "candidate";

/** [`ReviewLane`] values, in strict display order. */
export const REVIEW_LANE_ORDER: readonly ReviewLane[] = [
  "possible_closure",
  "needs_review",
  "overdue",
  "approaching_deadline",
  "needs_deadline",
  "candidate",
];

/** Maps one card's primary label to its review lane, or `null` if the card never enters the review queue. */
export function laneFor(label: PrimaryLabel): ReviewLane | null {
  switch (label) {
    case "possible_closure":
    case "needs_review":
    case "overdue":
    case "approaching_deadline":
    case "needs_deadline":
    case "candidate":
      return label;
    case "terminal_resolution":
    case "active":
      return null;
  }
}

/**
 * `OL-DUE-007`-consistent deadline ordering: ascending by
 * `epochMillis`, with a `null` (no approved deadline yet — the "No
 * deadline" case) sorting last, never in the middle and never assigned a
 * fabricated value.
 */
export function compareByDeadline(
  a: DeadlineProjection,
  b: DeadlineProjection,
): number {
  if (a.epochMillis === null && b.epochMillis === null) {
    return 0;
  }
  if (a.epochMillis === null) {
    return 1;
  }
  if (b.epochMillis === null) {
    return -1;
  }
  return a.epochMillis - b.epochMillis;
}

/** Injected deadline formatter: turns an approved epoch instant into display text. Never called for the no-deadline case. */
export type DeadlineFormatter = (epochMillis: number) => string;

/** The literal, non-fabricated label shown when no operative deadline exists yet. */
export const NO_DEADLINE_LABEL = "No deadline";

/** One card's rendered, locale-resolved view-model. */
export interface CardViewModel {
  readonly loopId: string;
  readonly primaryLabel: PrimaryLabel;
  readonly provenanceChips: readonly ProvenanceCode[];
  readonly reminderState: ReminderState;
  readonly sourceState: SourceState;
  readonly deadlineDisplay: string;
}

/** Assembles one pure, deterministic [`CardViewModel`] from a [`CardProjection`]. */
export function buildCardViewModel(
  card: CardProjection,
  formatDeadline: DeadlineFormatter,
): CardViewModel {
  const deadlineDisplay =
    card.operativeDeadline.epochMillis === null
      ? NO_DEADLINE_LABEL
      : formatDeadline(card.operativeDeadline.epochMillis);
  return {
    loopId: card.loopId,
    primaryLabel: card.primaryLabel,
    provenanceChips: card.secondaryChips.provenance,
    reminderState: card.secondaryChips.reminderState,
    sourceState: card.secondaryChips.sourceState,
    deadlineDisplay,
  };
}

/** One lane's worth of ordered, rendered cards. */
export interface ReviewQueueLane {
  readonly lane: ReviewLane;
  readonly cards: readonly CardViewModel[];
}

/**
 * Builds the complete, ordered review queue from `projections`: cards that
 * do not enter the review queue ([`laneFor`] returns `null`) are dropped;
 * every remaining card is grouped into its lane in [`REVIEW_LANE_ORDER`],
 * and each lane is deadline-sorted per [`compareByDeadline`] ("No deadline"
 * last).
 */
export function buildReviewQueue(
  projections: readonly CardProjection[],
  formatDeadline: DeadlineFormatter,
): readonly ReviewQueueLane[] {
  const byLane: Record<ReviewLane, CardProjection[]> = {
    possible_closure: [],
    needs_review: [],
    overdue: [],
    approaching_deadline: [],
    needs_deadline: [],
    candidate: [],
  };
  for (const projection of projections) {
    const lane = laneFor(projection.primaryLabel);
    if (lane === null) {
      continue;
    }
    byLane[lane].push(projection);
  }
  return REVIEW_LANE_ORDER.map((lane) => {
    const sorted = [...byLane[lane]].sort((a, b) =>
      compareByDeadline(a.operativeDeadline, b.operativeDeadline),
    );
    return {
      lane,
      cards: sorted.map((projection) =>
        buildCardViewModel(projection, formatDeadline),
      ),
    };
  });
}

/**
 * `OL-DUE-007`: "offer due date/time, no deadline, defer reminder, dismiss,
 * and not-mine actions", plus
 * `contracts/evidence/identity-boundary.json` `unavailable_projection.user_actions`.
 * Every variant is inert data only — building one never executes anything;
 * a future bridge story is solely responsible for acting on it.
 */
export type CorrectionIntent =
  | {
      readonly kind: "set_deadline";
      readonly loopId: string;
      readonly epochMillis: number;
    }
  | { readonly kind: "set_no_deadline"; readonly loopId: string }
  | { readonly kind: "defer_reminder"; readonly loopId: string }
  | { readonly kind: "dismiss"; readonly loopId: string }
  | { readonly kind: "not_mine"; readonly loopId: string }
  | { readonly kind: "select_replacement_evidence"; readonly loopId: string }
  | { readonly kind: "completed_outside_email"; readonly loopId: string }
  | { readonly kind: "reopen"; readonly loopId: string }
  | { readonly kind: "delete_loop"; readonly loopId: string };

/** Builds a [`CorrectionIntent`] setting an explicit deadline. Pure: `epochMillis` is caller-supplied, never derived from a clock here. */
export function setDeadlineIntent(
  loopId: string,
  epochMillis: number,
): CorrectionIntent {
  return { kind: "set_deadline", loopId, epochMillis };
}

/** Builds the "no deadline" confirmation [`CorrectionIntent`] (OL-DUE-007). */
export function setNoDeadlineIntent(loopId: string): CorrectionIntent {
  return { kind: "set_no_deadline", loopId };
}

/** Builds a plain, no-payload [`CorrectionIntent`] of `kind` for `loopId`. */
export function simpleIntent(
  kind:
    | "defer_reminder"
    | "dismiss"
    | "not_mine"
    | "select_replacement_evidence"
    | "completed_outside_email"
    | "reopen"
    | "delete_loop",
  loopId: string,
): CorrectionIntent {
  return { kind, loopId };
}
