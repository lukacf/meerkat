import assert from "node:assert/strict";
import test from "node:test";

import { CONSOLE_COMMAND_NAMES, consoleCommandMethod } from "./headless";
import {
  WORKGRAPH_CONFLICT_CODE,
  WORKGRAPH_UNOBSERVED_REVISION_MESSAGE,
  workGraphClaimOwnerId,
  workGraphConflictRefreshRequest,
} from "./workgraph-actions";

test("claim owner id prefers the authenticated operator subject and falls back to the ops-lead id", () => {
  assert.equal(workGraphClaimOwnerId("luka@example.com", "console-ops-lead"), "luka@example.com");
  assert.equal(workGraphClaimOwnerId("  padded@example.com  ", "console-ops-lead"), "padded@example.com");
  assert.equal(workGraphClaimOwnerId("", "console-ops-lead"), "console-ops-lead");
  assert.equal(workGraphClaimOwnerId("   ", "console-ops-lead"), "console-ops-lead");
  assert.equal(workGraphClaimOwnerId(null, "console-ops-lead"), "console-ops-lead");
  assert.equal(workGraphClaimOwnerId(undefined, "console-ops-lead"), "console-ops-lead");
});

test("conflict code pins the wire contract for the retryable workgraph CAS class", () => {
  assert.equal(WORKGRAPH_CONFLICT_CODE, -32042);
});

test("an action whose card showed no revision says nothing was changed, and its entity plans the conflict path's re-read", () => {
  assert.match(WORKGRAPH_UNOBSERVED_REVISION_MESSAGE, /nothing was changed/);
  assert.deepEqual(
    workGraphConflictRefreshRequest({ id: "item-1" }),
    { command: CONSOLE_COMMAND_NAMES.workgraphGet, params: { id: "item-1" } },
  );
  assert.deepEqual(
    workGraphConflictRefreshRequest({ binding_id: "b-1" }),
    { command: CONSOLE_COMMAND_NAMES.workgraphGoalStatus, params: { binding_id: "b-1" } },
  );
});

test("conflict refresh planning routes item mutations to get and binding mutations to goal/status", () => {
  // Item-addressed mutations (claim/release/close) → re-read the item.
  assert.deepEqual(
    workGraphConflictRefreshRequest({ id: "item-1", expected_revision: 4, owner: { kind: "principal", id: "ops" } }),
    { command: CONSOLE_COMMAND_NAMES.workgraphGet, params: { id: "item-1" } },
  );
  // Binding-addressed mutations (goal confirm/request-close, attention
  // pause/resume/reassign) → goal/status, which re-observes both CAS classes.
  assert.deepEqual(
    workGraphConflictRefreshRequest({ binding_id: "b-1", expected_revision: 7 }),
    { command: CONSOLE_COMMAND_NAMES.workgraphGoalStatus, params: { binding_id: "b-1" } },
  );
  // Only the addressing field is carried over — never the stale CAS token.
  const refresh = workGraphConflictRefreshRequest({ id: "item-1", expected_revision: 4 });
  assert.ok(refresh && !("expected_revision" in refresh.params));
});

test("conflict refresh planning declines when the failed params name no entity", () => {
  assert.equal(workGraphConflictRefreshRequest({}), null);
  assert.equal(workGraphConflictRefreshRequest({ id: "  " }), null);
  assert.equal(workGraphConflictRefreshRequest({ id: 42 as unknown as string }), null);
  assert.equal(workGraphConflictRefreshRequest({ binding_id: "" }), null);
});

test("operator-result frames stamp the real RPC method behind each console command", () => {
  assert.equal(consoleCommandMethod(CONSOLE_COMMAND_NAMES.workgraphClaim), "meta/workgraph/claim");
  assert.equal(consoleCommandMethod(CONSOLE_COMMAND_NAMES.workgraphGet), "meta/workgraph/get");
  assert.equal(consoleCommandMethod(CONSOLE_COMMAND_NAMES.workgraphGoalStatus), "meta/workgraph/goal/status");
  assert.equal(
    consoleCommandMethod(CONSOLE_COMMAND_NAMES.workgraphGoalRequestClose),
    "meta/workgraph/goal/request_close",
  );
});
