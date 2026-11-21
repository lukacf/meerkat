import { test } from "node:test";
import assert from "node:assert/strict";
import { parseWorkGraphItem, workGraphCreateOptionsToDict, workGraphGoalCreateOptionsToDict, workGraphUpdateOptionsToDict } from "../src/types.js";

test("finish-by explicit update encoding and returned value survive SDK wrappers", () => {
  const value = "2026-11-20T08:00:00Z";
  assert.deepEqual(workGraphCreateOptionsToDict({ finishBy: value }), { finish_by: value });
  assert.deepEqual(workGraphGoalCreateOptionsToDict({ finishBy: value }), { finish_by: value });
  assert.deepEqual(workGraphUpdateOptionsToDict({}), {});
  assert.deepEqual(workGraphUpdateOptionsToDict({ finishBy: null }), { finish_by: null });
  assert.deepEqual(workGraphUpdateOptionsToDict({ finishBy: { op: "set", value } }), { finish_by: { op: "set", value } });
  assert.deepEqual(workGraphUpdateOptionsToDict({ finishBy: { op: "clear" } }), { finish_by: { op: "clear" } });
  assert.equal(parseWorkGraphItem({ id: "work_test", finish_by: value }).finishBy, value);
  assert.equal(parseWorkGraphItem({ id: "work_test" }).finishBy, null);
  assert.deepEqual(workGraphUpdateOptionsToDict({ dueAt: value }), { due_at: value });
});
