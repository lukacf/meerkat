import assert from "node:assert/strict";
import test from "node:test";

import { CONSOLE_COMMAND_NAMES, consoleCommandMethod } from "./headless";
import {
  WORKGRAPH_CONFLICT_CODE,
  WORKGRAPH_UNOBSERVED_REVISION_MESSAGE,
  workGraphClaimOwnedBy,
  workGraphConflictRefreshRequest,
  workGraphMayRelease,
} from "./workgraph-actions";

test("a claim is owned only by the principal its owner key names, and no principal owns nothing", () => {
  const claimed = (kind: string, id: string) => ({ claim: { owner: { key: { kind, id } } } });
  assert.equal(workGraphClaimOwnedBy(claimed("principal", "ops@example.com"), "ops@example.com"), true);
  assert.equal(workGraphClaimOwnedBy(claimed("principal", "ops@example.com"), "other@example.com"), false);
  assert.equal(workGraphClaimOwnedBy(claimed("agent", "ops@example.com"), "ops@example.com"), false);
  assert.equal(workGraphClaimOwnedBy({}, "ops@example.com"), false);
  assert.equal(workGraphClaimOwnedBy(claimed("principal", "ops@example.com"), undefined), false);
});

test("a claim is released by its owner or by an access admin with a principal, and nothing unclaimed is", () => {
  const claimed = { claim: { owner: { key: { kind: "principal", id: "ops@example.com" } } } };
  assert.equal(workGraphMayRelease(claimed, { principal: "ops@example.com" }), true);
  assert.equal(workGraphMayRelease(claimed, { principal: "admin@example.com", canAdminister: true }), true);
  assert.equal(workGraphMayRelease(claimed, { principal: "other@example.com" }), false);
  assert.equal(workGraphMayRelease(claimed, { canAdminister: true }), false);
  assert.equal(workGraphMayRelease({}, { principal: "admin@example.com", canAdminister: true }), false);
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
