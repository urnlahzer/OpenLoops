import assert from "node:assert/strict";
import test from "node:test";
import { syntheticStatus } from "../src/skeleton.js";

void test("the add-in skeleton claims no capability or gate", () => {
  const status = syntheticStatus();
  assert.equal(status.companion_state, "skeleton_disabled");
  assert.deepEqual(status.enabled_capabilities, []);
  assert.deepEqual(status.gates_passed, []);
});
