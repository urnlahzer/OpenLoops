import assert from "node:assert/strict";
import test from "node:test";
import type { CardProjection } from "../src/projection.js";
import {
  NO_DEADLINE_LABEL,
  REVIEW_LANE_ORDER,
  buildCardViewModel,
  buildReviewQueue,
  compareByDeadline,
  laneFor,
  setDeadlineIntent,
  setNoDeadlineIntent,
  simpleIntent,
} from "../src/review.js";

function card(
  loopId: string,
  primaryLabel: CardProjection["primaryLabel"],
  epochMillis: number | null,
): CardProjection {
  return {
    loopId,
    primaryLabel,
    secondaryChips: {
      provenance: ["requested"],
      reminderState: "none",
      sourceState: "available",
      analysisState: "current",
    },
    operativeDeadline: {
      state: epochMillis === null ? "unresolved" : "scheduled",
      epochMillis,
    },
  };
}

// A fixed, injected formatter — never `Date`/`Intl` defaults — so output is
// fully deterministic under test.
const fixedFormatter = (epochMillis: number): string =>
  `formatted:${String(epochMillis)}`;

void test("laneFor maps every review-eligible label and excludes terminal/active", () => {
  assert.equal(laneFor("possible_closure"), "possible_closure");
  assert.equal(laneFor("needs_review"), "needs_review");
  assert.equal(laneFor("overdue"), "overdue");
  assert.equal(laneFor("approaching_deadline"), "approaching_deadline");
  assert.equal(laneFor("needs_deadline"), "needs_deadline");
  assert.equal(laneFor("candidate"), "candidate");
  assert.equal(laneFor("terminal_resolution"), null);
  assert.equal(laneFor("active"), null);
});

void test("REVIEW_LANE_ORDER matches precedence order", () => {
  assert.deepEqual(REVIEW_LANE_ORDER, [
    "possible_closure",
    "needs_review",
    "overdue",
    "approaching_deadline",
    "needs_deadline",
    "candidate",
  ]);
});

void test("compareByDeadline sorts ascending with no-deadline last, never fabricated", () => {
  const early = { state: "scheduled" as const, epochMillis: 100 };
  const late = { state: "scheduled" as const, epochMillis: 200 };
  const none = { state: "unresolved" as const, epochMillis: null };
  assert.ok(compareByDeadline(early, late) < 0);
  assert.ok(compareByDeadline(late, early) > 0);
  assert.ok(compareByDeadline(early, none) < 0);
  assert.ok(compareByDeadline(none, early) > 0);
  assert.equal(compareByDeadline(none, none), 0);
});

void test("buildReviewQueue groups into lanes and deadline-sorts within each lane, no-deadline last", () => {
  const projections: CardProjection[] = [
    card("overdue-late", "overdue", 500),
    card("overdue-none", "overdue", null),
    card("overdue-early", "overdue", 100),
    card("candidate-only", "candidate", 50),
    card("active-excluded", "active", 1),
    card("terminal-excluded", "terminal_resolution", 1),
  ];
  const queue = buildReviewQueue(projections, fixedFormatter);
  assert.equal(queue.length, REVIEW_LANE_ORDER.length);

  const overdueLane = queue.find((lane) => lane.lane === "overdue");
  assert.ok(overdueLane);
  assert.deepEqual(
    overdueLane.cards.map((c) => c.loopId),
    ["overdue-early", "overdue-late", "overdue-none"],
  );
  assert.equal(overdueLane.cards[2]?.deadlineDisplay, NO_DEADLINE_LABEL);
  assert.equal(overdueLane.cards[0]?.deadlineDisplay, "formatted:100");

  const candidateLane = queue.find((lane) => lane.lane === "candidate");
  assert.ok(candidateLane);
  assert.deepEqual(
    candidateLane.cards.map((c) => c.loopId),
    ["candidate-only"],
  );

  // active/terminal never enter any lane.
  for (const lane of queue) {
    assert.ok(
      !lane.cards.some(
        (c) =>
          c.loopId === "active-excluded" || c.loopId === "terminal-excluded",
      ),
    );
  }
});

void test("buildCardViewModel is deterministic: same input, same output, twice", () => {
  const projection = card("determinism-check", "needs_review", 12345);
  const first = buildCardViewModel(projection, fixedFormatter);
  const second = buildCardViewModel(projection, fixedFormatter);
  assert.deepEqual(first, second);
  assert.equal(first.deadlineDisplay, "formatted:12345");
});

void test("buildCardViewModel never fabricates a date for the no-deadline case", () => {
  const projection = card("no-deadline-check", "needs_deadline", null);
  const viewModel = buildCardViewModel(projection, fixedFormatter);
  assert.equal(viewModel.deadlineDisplay, NO_DEADLINE_LABEL);
});

void test("correction intents are pure typed data with no execution surface", () => {
  assert.deepEqual(setDeadlineIntent("loop-1", 999), {
    kind: "set_deadline",
    loopId: "loop-1",
    epochMillis: 999,
  });
  assert.deepEqual(setNoDeadlineIntent("loop-1"), {
    kind: "set_no_deadline",
    loopId: "loop-1",
  });
  assert.deepEqual(simpleIntent("dismiss", "loop-2"), {
    kind: "dismiss",
    loopId: "loop-2",
  });
  assert.deepEqual(simpleIntent("not_mine", "loop-2"), {
    kind: "not_mine",
    loopId: "loop-2",
  });
  assert.deepEqual(simpleIntent("reopen", "loop-3"), {
    kind: "reopen",
    loopId: "loop-3",
  });
  assert.deepEqual(simpleIntent("delete_loop", "loop-3"), {
    kind: "delete_loop",
    loopId: "loop-3",
  });
});
