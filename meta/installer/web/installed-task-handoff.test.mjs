import assert from "node:assert/strict";
import test from "node:test";
import { INSTALLED_TASK_FRAGMENT_PREFIX, INSTALLED_TASK_FRAGMENT_VERSION, INSTALLED_TASK_FRAGMENT_MAX,
  installedTaskFragment, installedTaskHandoffUrl } from "./installed-task-handoff.mjs";

const base = "http://127.0.0.1:8820/workspace";
const decode = (fragment) => JSON.parse(decodeURIComponent(fragment.slice(INSTALLED_TASK_FRAGMENT_PREFIX.length)));

test("versioned fragment preserves the editable draft, Unicode and allowed whitespace without a query", () => {
  const task = "  Compare my \u00e5 and \u{1f9ed} context.\nKeep\r\nthese\ttabs.  ";
  const url = installedTaskHandoffUrl(base, task);
  assert.equal(url.split("#")[0], base);
  assert.equal(new URL(url).search, "");
  assert.equal(url.includes(task), false);
  assert.deepEqual(decode(new URL(url).hash), { version: 1, task });
  assert.deepEqual(Object.keys(decode(new URL(url).hash)), ["version", "task"]);
  assert.equal(INSTALLED_TASK_FRAGMENT_VERSION, 1);
  assert.equal(INSTALLED_TASK_FRAGMENT_MAX, 40000);
});

test("payload budget accepts 3000 astral codepoints and rejects blank, oversized or malformed Unicode drafts", () => {
  const maximum = "\u{1f9ed}".repeat(3000), fragment = installedTaskFragment(maximum);
  assert.ok(fragment);
  assert.ok(fragment.length - INSTALLED_TASK_FRAGMENT_PREFIX.length <= 40000);
  assert.equal(decode(fragment).task, maximum);
  for (const task of [null, {}, "", " \t\r\n ", "x".repeat(3001), maximum + "x", "\ud800", "\udfff", "ok\ud800after", "ok\udfffafter"])
    assert.equal(installedTaskFragment(task), null);
  for (let code = 0; code <= 31; code += 1) {
    const task = "before" + String.fromCharCode(code) + "after";
    assert.equal(!!installedTaskFragment(task), [9, 10, 13].includes(code), "control " + code);
  }
  assert.equal(installedTaskFragment("before\u007fafter"), null);
});

test("handoff only appends to canonical checked loopback application addresses and never native Console", () => {
  for (const value of ["https://127.0.0.1:8820/workspace", "http://localhost:8820/workspace", "http://example.invalid:8820/workspace",
    "http://user:password@127.0.0.1:8820/workspace", base + "?task=unsafe", base + "#old", base + " ",
    "http://127.0.0.1:80/workspace", "http://127.0.0.1:08820/workspace", "http://127.0.0.1:8820/console",
    "http://127.0.0.1:8820/console/threads", "not-a-url"])
    assert.equal(installedTaskHandoffUrl(value, "Current editable job"), null, value);
  assert.equal(installedTaskHandoffUrl(base, "\u0000bad"), null);
  assert.equal(decode(new URL(installedTaskHandoffUrl(base, "Edited job")).hash).task, "Edited job");
});

test("bounded route decoding excludes Console aliases and malformed paths while preserving valid authored encoding", () => {
  const origin = "http://127.0.0.1:8820";
  for (const path of ["/%63onsole", "/con%73ole", "/console%2Fthreads", "/%63onsole%2fthreads",
    "/%2563onsole", "/console%252Fthreads", "/%252563onsole", "/console%25252Fthreads",
    "/%25252563onsole", "/console%2525252Fthreads", "/workspace%", "/workspace%ZZ", "/workspace%C3%28",
    "/workspace%FF", "/workspace%25", "/work%25252573pace", "/workspace%2525252525"])
    assert.equal(installedTaskHandoffUrl(origin + path, "Current edited task"), null, path);
  for (const path of ["/workspace", "/console-tools", "/app/console", "/work%73pace", "/work%2573pace", "/work%252573pace",
    "/work%2Fspace", "/caf%C3%A9", "/caf%25C3%25A9", "/" + "%C3%A9".repeat(180)])
    assert.equal(installedTaskHandoffUrl(origin + path, "Current edited task")?.split("#")[0], origin + path, path);
});
