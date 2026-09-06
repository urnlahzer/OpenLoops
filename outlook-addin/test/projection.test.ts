import assert from "node:assert/strict";
import test from "node:test";
import {
  PRIMARY_LABEL_PRECEDENCE,
  primaryLabel,
  type DisplayFacets,
} from "../src/projection.js";

function base(): DisplayFacets {
  return {
    obligationState: "open",
    closureReviewState: "none",
    deadlineState: "scheduled",
    reviewFlags: [],
    analysisState: "current",
    sourceState: "available",
    reminderState: "none",
  };
}

void test("PRIMARY_LABEL_PRECEDENCE matches product-spec §5.1's exact eight-row table", () => {
  assert.deepEqual(PRIMARY_LABEL_PRECEDENCE, [
    "terminal_resolution",
    "possible_closure",
    "needs_review",
    "overdue",
    "approaching_deadline",
    "needs_deadline",
    "candidate",
    "active",
  ]);
});

void test("terminal outranks every other label", () => {
  const facets: DisplayFacets = {
    ...base(),
    obligationState: "terminal",
    closureReviewState: "possible",
    deadlineState: "overdue",
    reviewFlags: ["needs_review"],
  };
  assert.equal(primaryLabel(facets), "terminal_resolution");
});

void test("possible closure outranks needs-review and deadline labels", () => {
  const facets: DisplayFacets = {
    ...base(),
    closureReviewState: "possible",
    deadlineState: "overdue",
    reviewFlags: ["needs_review"],
  };
  assert.equal(primaryLabel(facets), "possible_closure");
});

void test("needs review outranks overdue and approaching, from any of its four triggers", () => {
  assert.equal(
    primaryLabel({
      ...base(),
      deadlineState: "overdue",
      reviewFlags: ["needs_review"],
    }),
    "needs_review",
  );
  assert.equal(
    primaryLabel({
      ...base(),
      deadlineState: "approaching",
      analysisState: "quarantined",
    }),
    "needs_review",
  );
  assert.equal(
    primaryLabel({ ...base(), sourceState: "changed" }),
    "needs_review",
  );
  assert.equal(
    primaryLabel({ ...base(), reminderState: "conflict" }),
    "needs_review",
  );
  assert.equal(
    primaryLabel({ ...base(), reminderState: "ambiguous_write" }),
    "needs_review",
  );
  assert.equal(
    primaryLabel({ ...base(), reminderState: "missing" }),
    "needs_review",
  );
  assert.equal(
    primaryLabel({ ...base(), reminderState: "changed" }),
    "needs_review",
  );
});

void test("benign states never trigger needs_review", () => {
  assert.equal(primaryLabel({ ...base(), analysisState: "stale" }), "active");
  assert.equal(
    primaryLabel({ ...base(), reminderState: "proposed" }),
    "active",
  );
  assert.equal(
    primaryLabel({ ...base(), reminderState: "completed_needs_evidence" }),
    "active",
  );
  assert.equal(
    primaryLabel({ ...base(), reminderState: "pending_write" }),
    "active",
  );
  assert.equal(primaryLabel({ ...base(), reminderState: "linked" }), "active");
});

void test("source states that require action all trigger needs_review", () => {
  assert.equal(
    primaryLabel({ ...base(), sourceState: "partially_available" }),
    "needs_review",
  );
  assert.equal(
    primaryLabel({ ...base(), sourceState: "unavailable" }),
    "needs_review",
  );
  assert.equal(
    primaryLabel({ ...base(), sourceState: "changed" }),
    "needs_review",
  );
});

void test("overdue outranks approaching and needs-deadline", () => {
  assert.equal(
    primaryLabel({ ...base(), deadlineState: "overdue" }),
    "overdue",
  );
});

void test("approaching outranks needs-deadline", () => {
  assert.equal(
    primaryLabel({ ...base(), deadlineState: "approaching" }),
    "approaching_deadline",
  );
});

void test("needs deadline outranks candidate and active", () => {
  assert.equal(
    primaryLabel({ ...base(), deadlineState: "unresolved" }),
    "needs_deadline",
  );
});

void test("candidate shows when nothing higher applies", () => {
  assert.equal(
    primaryLabel({
      ...base(),
      obligationState: "candidate",
      deadlineState: "scheduled",
    }),
    "candidate",
  );
});

void test("active is the final fallback", () => {
  assert.equal(primaryLabel(base()), "active");
});

void test("undated_confirmed never triggers needs_deadline", () => {
  assert.equal(
    primaryLabel({ ...base(), deadlineState: "undated_confirmed" }),
    "active",
  );
});
