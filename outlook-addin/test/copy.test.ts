import assert from "node:assert/strict";
import test from "node:test";
import {
  BANNED_PHRASES,
  MissingPlaceholderError,
  containsBannedPhrase,
  copyKeys,
  renderCopy,
  templateFor,
} from "../src/copy.js";

void test("the closed template table has exactly the eleven declared keys", () => {
  assert.deepEqual(copyKeys(), [
    "evidence_unavailable",
    "evidence_changed",
    "evidence_ambiguous",
    "navigation_unavailable",
    "reconnect_required",
    "stale",
    "reminder_missing",
    "reminder_conflict",
    "reminder_ambiguous_write",
    "reminder_changed",
    "needs_deadline_prompt",
  ]);
});

void test("no real template contains a banned failure-attribution phrase", () => {
  for (const key of copyKeys()) {
    const template = templateFor(key);
    assert.equal(
      containsBannedPhrase(template.template),
      false,
      `template ${key} unexpectedly matched a banned phrase`,
    );
  }
});

void test("every placeholder named in a template is resolved by renderCopy", () => {
  for (const key of copyKeys()) {
    const template = templateFor(key);
    const values: Record<string, string> = {};
    for (const placeholder of template.placeholders) {
      values[placeholder] = `synthetic-${placeholder}`;
    }
    const text = renderCopy(key, values);
    assert.equal(
      text.includes("{"),
      false,
      `unresolved placeholder left in ${key}`,
    );
    for (const placeholder of template.placeholders) {
      assert.ok(text.includes(`synthetic-${placeholder}`));
    }
  }
});

void test("a missing placeholder is a typed rejection, never a silent blank", () => {
  assert.throws(
    () => renderCopy("evidence_unavailable", {}),
    (error: unknown) =>
      error instanceof MissingPlaceholderError &&
      error.key === "evidence_unavailable" &&
      error.placeholder === "loopLabel",
  );
});

void test("containsBannedPhrase is case-insensitive", () => {
  assert.equal(containsBannedPhrase("You Forgot to reply"), true);
  assert.equal(containsBannedPhrase("YOU FAILED to respond"), true);
  assert.equal(containsBannedPhrase("this text is entirely fine"), false);
});

// "Test the tests": would our own banned-phrase scan actually catch a
// hostile edit that reintroduces failure-attribution language into a real
// template? Construct three hostile mutations of real templates and confirm
// each one is rejected.
void test("three hostile template edits are all rejected by containsBannedPhrase", () => {
  const real = templateFor("evidence_unavailable").template;
  const hostileEdits = [
    `${real} You forgot to check this earlier.`,
    `${real} This is your fault for not following up sooner.`,
    `${real} You were unreliable in tracking this request.`,
  ];
  for (const hostile of hostileEdits) {
    assert.equal(
      containsBannedPhrase(hostile),
      true,
      `expected the hostile edit to be rejected: ${hostile}`,
    );
  }
});

void test("BANNED_PHRASES itself is nonempty and closed (no accidental empty entries)", () => {
  assert.ok(BANNED_PHRASES.length > 0);
  for (const phrase of BANNED_PHRASES) {
    assert.ok(phrase.trim().length > 0);
  }
});
