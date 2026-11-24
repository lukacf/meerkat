"use strict";

// The harness parts of the two-mob scenario, without a gateway and without a
// browser: the gateway's environment, the stdio host, the recording proxy,
// how a request's member is read, and how the page's roster is grouped.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { test } = require("node:test");
const {
  scenarios, ACTIONS, OFFERED_FOR_EVERY_MOB, NOT_OFFERED_FOR_ANOTHER_MOB, offerFault, PLAN, NOT_COVERED,
  childEnvironment, mobToml, gatewayStdio, startRecordingProxy, callsOf, namedMember, groupRoster, rowOf, checkBook,
  BUNDLE_ASSETS, bundleEvidence, fixtureIdentity, storeReadback, runtimeKeyOf, classifyPeerFrames, classifyStop,
  multipartRequest, sendCarrying, storeHolding, modelRequestsCarrying, imageBlobIds, framesHolding,
  fileNeedles, runsSharedWith, filesHolding, typedRefusalFault, MOB_REFUSAL, mobRefusalFault, attachmentParamsFor, logCounts,
} = require("./real-hosted-mobs.cjs");
const { createHash } = require("node:crypto");

test("the scenario is one real-backend console scenario, run from its own file: the default list does not hold it", () => {
  assert.deepEqual(scenarios.map(({ id, family, backend }) => ({ id, family, backend })),
    [{ id: "real-hosted-mobs", family: "console", backend: "real" }]);
  assert.equal(typeof scenarios[0].run, "function");
  const listed = require("../browser-e2e.cjs").scenarios.map((scenario) => scenario.id);
  assert(!listed.includes("real-hosted-mobs"), "the default list does not hold the scenario");
});

test("the gateway is given a short list of plain variables and a home of its own", () => {
  const source = {
    PATH: "/usr/bin", LANG: "C.UTF-8", TZ: "UTC", HOME: "/home/someone", USER: "someone",
    OPENAI_API_KEY: "present", ANTHROPIC_API_KEY: "present", GEMINI_API_KEY: "present", GOOGLE_API_KEY: "present",
    RKAT_OPENAI_API_KEY: "present", GITHUB_TOKEN: "present", AWS_SECRET_ACCESS_KEY: "present",
    XDG_CONFIG_HOME: "/home/someone/.config", META_EXAMPLE_BIN_DIR: "/somewhere",
  };
  const env = childEnvironment(source, "/private/home");
  assert.deepEqual(Object.keys(env).sort(), [
    "HOME", "LANG", "PATH", "TMPDIR", "TZ", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME",
  ]);
  assert.equal(env.HOME, "/private/home");
  assert.equal(env.XDG_CONFIG_HOME, path.join("/private/home", "config"));
  assert(!Object.values(env).includes("present"), "no value of a key or a token is handed on");
  assert(!Object.values(env).some((value) => value.includes("/home/someone")), "nothing of the user's home is handed on");
});

test("a mob definition names its id and one profile", () => {
  const toml = mobToml("some-mob");
  assert.match(toml, /^\[mob\]\nid = "some-mob"\n/);
  assert.match(toml, /\n\[profiles\.agent\]\nmodel = "gpt-5\.5"\nexternal_addressable = true\n/);
});

test("the request's member is read from the member's mob, else the request's mob, else the launch mob", () => {
  assert.deepEqual(namedMember({ identity: "reviewer", member_mob_id: "second" }, "identity", "launch"),
    { mob: "second", name: "reviewer", form: "member_mob_id" });
  assert.deepEqual(namedMember({ identity: "reviewer", mob_id: "second" }, "identity", "launch"),
    { mob: "second", name: "reviewer", form: "mob_id" });
  assert.deepEqual(namedMember({ identity: "reviewer", mob_id: "second", member_mob_id: "inner" }, "identity", "launch"),
    { mob: "inner", name: "reviewer", form: "member_mob_id" });
  assert.deepEqual(namedMember({ identity: "reviewer" }, "identity", "launch"),
    { mob: "launch", name: "reviewer", form: "no mob" });
  // A name that carries its mob in one string is not the member's name in that mob.
  assert.deepEqual(namedMember({ identity: "second/reviewer" }, "identity", "launch"),
    { mob: "launch", name: "second/reviewer", form: "no mob" });
  assert.deepEqual(namedMember({ member_id: "reviewer", mob_id: "second" }, "member_id", "launch"),
    { mob: "second", name: "reviewer", form: "mob_id" });
});

test("the page's roster is grouped by the header each row stands under", () => {
  const mobs = groupRoster([
    { id: "sidebar-mob:launch", text: "Launch 2" },
    { id: "sidebar-agent:planner", text: "Planner" },
    { id: "sidebar-agent:reviewer", text: "Reviewer" },
    { id: "sidebar-mob:second", text: "Second 2" },
    { id: "sidebar-agent:second/reviewer", text: "Reviewer" },
    { id: "sidebar-agent:second/triage", text: "Triage" },
  ]);
  assert.deepEqual(mobs.map((mob) => [mob.mobId, mob.agents.map((agent) => agent.key)]),
    [["launch", ["planner", "reviewer"]], ["second", ["second/reviewer", "second/triage"]]]);
  assert.equal(rowOf(mobs, "launch", "reviewer").key, "reviewer");
  assert.equal(rowOf(mobs, "second", "reviewer").key, "second/reviewer");
  assert.throws(() => rowOf(mobs, "second", "planner"), /has one row for planner/);
  assert.throws(() => rowOf(mobs, "third", "reviewer"), /lists no mob third/);
  assert.throws(() => groupRoster([{ id: "sidebar-agent:alone", text: "Alone" }]), /stands under no mob header/);
});

test("an action's requests are found on the request door by method and on a route by path", () => {
  const record = (overrides) => ({ seq: 0, method: "POST", path: "/console/rpc", body: "", status: 200, response: "", finishedAt: 1, ...overrides });
  const records = [
    record({ seq: 1, body: JSON.stringify({ method: "meta/console/send", params: { identity: "reviewer", mob_id: "second", content: "hello" } }),
      response: JSON.stringify({ result: { accepted: true } }) }),
    record({ seq: 2, body: JSON.stringify({ method: "meta/console/send", params: { identity: "reviewer", content: "refused" } }),
      response: JSON.stringify({ error: { code: -32602, data: { kind: "foreign_mob_target" } } }) }),
    record({ seq: 3, body: JSON.stringify({ method: "meta/console/send", params: { identity: "reviewer", content: "open" } }), status: null, finishedAt: null }),
    record({ seq: 4, path: "/console/send", body: JSON.stringify({ identity: "reviewer", content: "route" }), status: 404, response: "{}" }),
    record({ seq: 5, body: JSON.stringify({ method: "meta/status", params: {} }), response: JSON.stringify({ result: {} }) }),
    record({ seq: 6, method: "GET", path: "/console/timeline", body: "" }),
  ];
  const calls = callsOf(ACTIONS.send, records);
  assert.deepEqual(calls.map((call) => [call.seq, call.door, call.answered, call.refused]), [
    [1, "meta/console/send", true, false],
    [2, "meta/console/send", true, true],
    [3, "meta/console/send", false, false],
    [4, "/console/send", true, true],
  ]);
  assert.equal(calls[0].params.mob_id, "second");
  assert.deepEqual(callsOf(ACTIONS.retire, records), []);
});

test("the rows of the result say what ran, what did not apply and what the scenario does not cover", async () => {
  const book = checkBook([["a", "first"], ["b", "second"], ["c", "third"]], [["d", "left out"]]);
  await book.run("a", async () => ({ seen: 1 }));
  await assert.rejects(book.run("b", async () => { throw new Error("no"); }), /no/);
  book.notApplicable("c", "not offered");
  assert.deepEqual(book.rows.map((row) => [row.id, row.verdict, row.detail]), [
    ["a", "pass", { seen: 1 }], ["b", "fail", "no"], ["c", "not applicable", "not offered"], ["d", "not covered", undefined],
  ]);
  await assert.rejects(book.run("missing", async () => {}), /no such row/);
});

test("a row the scenario does not cover is no failed row, and the result names it", async () => {
  const book = checkBook([["a", "first"]], [["d", "left out"], ["e", "left out too"]]);
  await book.run("a", async () => {});
  assert.deepEqual(book.failed(), []);
  assert.deepEqual(book.notCovered(), ["d", "e"]);
  assert.deepEqual(book.rows.filter((row) => row.verdict === "not reached"), []);
  assert.deepEqual(checkBook([["a", "first"]], []).notCovered(), []);
});

test("a failed row ends its action's part and the next action's part still runs", async () => {
  const book = checkBook([["a:target", "first"], ["a:effect", "second"], ["b:target", "third"]], []);
  await book.section("a", async () => {
    await book.run("a:target", async () => { throw new Error("refused"); });
    await book.run("a:effect", async () => {});
  });
  await book.section("b", async () => {
    await book.run("b:target", async () => ({ seen: 1 }));
  });
  assert.deepEqual(book.rows.map((row) => [row.id, row.verdict]), [
    ["a:target", "fail"], ["a:effect", "not reached"], ["b:target", "pass"],
  ]);
  assert.deepEqual(book.sections, [
    { name: "a", ended: "early", error: "refused" },
    { name: "b", ended: "complete" },
  ]);
  assert.deepEqual(book.failed(), ["a:target"]);
});

test("the plan holds an offer row and an unavailable row for every action it can find missing", () => {
  const ids = PLAN.map(([id]) => id);
  assert.equal(new Set([...ids, ...NOT_COVERED.map(([id]) => id)]).size, ids.length + NOT_COVERED.length, "row ids are unique");
  for (const action of Object.keys(ACTIONS)) {
    assert(ids.includes(`${action}:offer`), `${action} has an offer row`);
    assert(ids.includes(`${action}:unavailable`), `${action} has an unavailable row`);
    assert(ids.includes(`${action}:other-mob`), `${action} has a row for the other mob`);
  }
  for (const action of OFFERED_FOR_EVERY_MOB) assert(ACTIONS[action], `${action} is an action`);
  // An action the console must not offer for another mob is an action, is not one it has to offer, and has a row for its refusal at the door.
  assert.deepEqual(NOT_OFFERED_FOR_ANOTHER_MOB, ["attach"]);
  for (const action of NOT_OFFERED_FOR_ANOTHER_MOB) {
    assert(ACTIONS[action], `${action} is an action`);
    assert(!OFFERED_FOR_EVERY_MOB.includes(action), `${action} is not also to be offered`);
    assert(ids.includes(`${action}:refused`), `${action} has a row for its refusal`);
  }
  assert.deepEqual(ACTIONS.attach.methods, ACTIONS.send.methods, "a message with a file is the send's method");
});

test("what the page offers for the second mob's member is held against what it has to offer and what it must not", () => {
  for (const action of OFFERED_FOR_EVERY_MOB) {
    assert.equal(offerFault(action, true), null, action);
    assert.match(offerFault(action, false), /has to offer/, action);
  }
  assert.equal(offerFault("attach", false), null);
  assert.match(offerFault("attach", true), /must not offer/);
  // An action in neither list may be offered or not.
  assert.equal(offerFault("retire", true), null);
  assert.equal(offerFault("retire", false), null);
});

test("the stdio host sends one request per line, reads answers by id and answers the gateway no handler", async () => {
  // A stand-in for the gateway: it answers each request, asks its host one
  // thing first, and reports what the host answered.
  const script = `
    const lines = require("node:readline").createInterface({ input: process.stdin });
    let asked = false; let hostAnswer = null;
    lines.on("line", (line) => {
      const message = JSON.parse(line);
      if (message.id === "to-host") { hostAnswer = message; return; }
      if (!asked) { asked = true; process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: "to-host", method: "host/provide", params: {} }) + "\\n"); }
      if (message.method === "fails") process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: message.id, error: { code: -32602, message: "refused", data: { kind: "some_kind" } } }) + "\\n");
      else if (message.method === "host-answer") setTimeout(() => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: message.id, result: hostAnswer }) + "\\n"), 100);
      else if (message.method === "silent") { /* no answer */ }
      else process.stdout.write("not json\\n" + JSON.stringify({ jsonrpc: "2.0", id: message.id, result: { method: message.method, params: message.params } }) + "\\n");
    });
  `;
  const child = spawn(process.execPath, ["-e", script], { stdio: ["pipe", "pipe", "inherit"] });
  try {
    const gateway = gatewayStdio(child);
    assert.deepEqual(await gateway.request("meta/init", { a: 1 }), { method: "meta/init", params: { a: 1 } });
    const refusal = await gateway.request("fails", {}).then(() => null, (error) => error);
    assert.match(refusal.message, /fails failed/);
    assert.equal(refusal.rpc.data.kind, "some_kind");
    const answered = await gateway.request("host-answer", {});
    assert.deepEqual({ id: answered.id, code: answered.error.code }, { id: "to-host", code: -32601 });
    await assert.rejects(gateway.request("silent", {}, 200), /silent got no answer within 200 ms/);
    child.kill("SIGKILL");
    await new Promise((resolve) => child.once("exit", resolve));
    await assert.rejects(gateway.request("after", {}), /gateway exited/);
  } finally {
    child.kill("SIGKILL");
  }
});

test("the proxy serves the console's page and assets itself and records every other request with its answer", async () => {
  const assetsDir = fs.mkdtempSync(path.join(os.tmpdir(), "two-mobs-proxy-"));
  fs.writeFileSync(path.join(assetsDir, "index.html"), "<!doctype html><title>page</title>");
  fs.writeFileSync(path.join(assetsDir, "console-app.js"), "// app");
  fs.writeFileSync(path.join(assetsDir, "console-app.css"), "/* css */");
  const upstream = http.createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => { body += chunk; });
    req.on("end", () => {
      if (req.url.startsWith("/console/timeline/stream")) {
        res.writeHead(200, { "content-type": "text/event-stream" });
        res.write("event: frame\ndata: {}\n\n");
        return; // held open
      }
      res.writeHead(req.url === "/refused" ? 404 : 200, { "content-type": "application/json" });
      res.end(JSON.stringify({ echoed: body, url: req.url }));
    });
  });
  await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
  let base = null;
  const proxy = await startRecordingProxy({ assetsDir, upstream: () => base });
  try {
    assert.equal((await fetch(`${proxy.baseUrl}/console/rpc`, { method: "POST", body: "{}" })).status, 503, "no gateway yet");
    base = `http://127.0.0.1:${upstream.address().port}`;
    const page = await fetch(`${proxy.baseUrl}/console`);
    assert.equal(await page.text(), "<!doctype html><title>page</title>");
    assert.equal((await (await fetch(`${proxy.baseUrl}/console/assets/console-app.js`)).text()), "// app");
    assert.equal(proxy.records.length, 0, "the page and its assets are not requests to the gateway");
    const mark = proxy.mark();
    const sent = JSON.stringify({ method: "meta/console/send", params: { identity: "reviewer" } });
    const answer = await (await fetch(`${proxy.baseUrl}/console/rpc?x=1`, { method: "POST", headers: { "content-type": "application/json" }, body: sent })).json();
    assert.deepEqual(answer, { echoed: sent, url: "/console/rpc?x=1" });
    assert.equal((await fetch(`${proxy.baseUrl}/refused`, { method: "POST", body: "{}" })).status, 404);
    const abort = new AbortController();
    const stream = await fetch(`${proxy.baseUrl}/console/timeline/stream?mobs=*`, { signal: abort.signal });
    assert.equal(stream.status, 200);
    await stream.body.getReader().read();
    abort.abort();
    const records = proxy.since(mark);
    assert.deepEqual(records.map((record) => [record.method, record.path, record.status]), [
      ["POST", "/console/rpc", 200], ["POST", "/refused", 404], ["GET", "/console/timeline/stream", 200],
    ]);
    assert.equal(records[0].body, sent);
    assert.deepEqual(records[0].query, [["x", "1"]]);
    assert.deepEqual(JSON.parse(records[0].response), answer);
    assert.equal(typeof records[0].finishedAt, "number");
    assert.deepEqual(records[2].query, [["mobs", "*"]]);
    assert.equal(records[2].response, "", "an event stream's answer is not kept as a response");
    assert.equal(records[2].stream, "event: frame\ndata: {}\n\n", "what the stream sent the page is kept in stream");
    assert.equal(records[0].stream, undefined, "a request that is no stream has no stream");
    assert.equal(callsOf(ACTIONS.send, records).length, 1);
  } finally {
    await proxy.close();
    upstream.closeAllConnections?.();
    await new Promise((resolve) => upstream.close(resolve));
    fs.rmSync(assetsDir, { recursive: true, force: true });
  }
});

const hex = (text) => createHash("sha256").update(text).digest("hex");
const assetsOf = (index, js, css) => ({ "index.html": index, "console-app.js": js, "console-app.css": css });

test("the bundle's bytes from the gateway, the proxy and the tree are compared asset by asset", () => {
  assert.deepEqual(BUNDLE_ASSETS.map(([name, route]) => [name, route]), [
    ["index.html", "/console"], ["console-app.js", "/console/assets/console-app.js"], ["console-app.css", "/console/assets/console-app.css"],
  ]);
  const index = Buffer.from('<script src="/console/assets/console-app.js?v=abc123"></script>');
  const same = bundleEvidence({ gateway: assetsOf(index, "js", "css"), proxy: assetsOf(index, "js", "css"), dist: assetsOf(index, "js", "css") });
  assert.equal(same.equal, true);
  assert.equal(same.app_script_version, "abc123");
  assert.deepEqual(same.assets["console-app.js"], { gateway: hex("js"), proxy: hex("js"), dist: hex("js"), equal: true });
  const differs = bundleEvidence({ gateway: assetsOf(index, "js2", "css"), proxy: assetsOf(index, "js", "css"), dist: assetsOf(index, "js", "css") });
  assert.equal(differs.equal, false);
  assert.equal(differs.assets["console-app.js"].equal, false);
  assert.equal(differs.assets["console-app.css"].equal, true);
  const missing = bundleEvidence({ gateway: assetsOf(index, null, "css"), proxy: assetsOf(index, null, "css"), dist: assetsOf(index, null, "css") });
  assert.equal(missing.assets["console-app.js"].gateway, "missing");
  assert.equal(missing.assets["console-app.js"].equal, false, "a missing asset is never equal");
  assert.equal(missing.equal, false);
});

/** A SQLite file at `file` made by `statements`. */
function sqliteFile(file, statements) {
  const { DatabaseSync } = require("node:sqlite");
  const db = new DatabaseSync(file);
  try { for (const statement of statements) db.exec(statement); } finally { db.close(); }
}

const CONSOLE_FRAMES = "CREATE TABLE console_frames (cursor_seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL, runtime_key TEXT NOT NULL, identity TEXT NOT NULL, kind TEXT NOT NULL, status TEXT NOT NULL, session_id TEXT, run_id TEXT)";
const frameRow = (id, key, identity, kind, run) => `INSERT INTO console_frames (id, runtime_key, identity, kind, status, session_id, run_id) VALUES ('${id}', '${key}', '${identity}', '${kind}', 'delivered', 's-${identity}', ${run ? `'${run}'` : "NULL"})`;

test("the fixture's identity lists its files, the SQLite files' tables and their schema ledger", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "two-mobs-identity-"));
  try {
    fs.mkdirSync(path.join(root, "state", "mobs"), { recursive: true });
    sqliteFile(path.join(root, "state", "console.sqlite3"), [
      "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL)",
      "INSERT INTO meerkat_schema VALUES ('meta_console', 3), ('a_domain', 1)", CONSOLE_FRAMES,
    ]);
    sqliteFile(path.join(root, "state", "mobs", "plain.sqlite"), ["CREATE TABLE t (x INTEGER)"]);
    fs.writeFileSync(path.join(root, "state", "notes.txt"), "not a database");
    const identity = fixtureIdentity(root, root, [{ id: "launch", default: true, members: ["a"] }, { id: "second", default: false, members: ["a", "b"] }]);
    assert.equal(identity.state_root, root);
    assert.deepEqual(identity.files.map((file) => file.path), ["state/console.sqlite3", "state/mobs/plain.sqlite", "state/notes.txt"]);
    assert.deepEqual(identity.files[0].sqlite.schema, [{ domain: "a_domain", version: 1 }, { domain: "meta_console", version: 3 }]);
    assert.deepEqual(identity.files[0].sqlite.tables, ["console_frames", "meerkat_schema", "sqlite_sequence"]);
    assert.deepEqual(identity.files[1].sqlite.schema, { missing: "no meerkat_schema ledger in this file" });
    assert.equal(identity.files[2].sqlite, undefined, "a file that is no SQLite file is listed with its size only");
    assert.equal(identity.files[2].bytes, "not a database".length);
    assert.deepEqual(identity.mobs, [{ id: "launch", default: true, members: ["a"] }, { id: "second", default: false, members: ["a", "b"] }]);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("the stores are read back per mob from the files, and what cannot be read is missing", () => {
  const mobs = [{ id: "launch", default: true, members: ["reviewer"] }, { id: "second", default: false, members: ["reviewer"] }];
  assert.deepEqual(mobs.map(runtimeKeyOf), ["default", "mob/second"]);
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "two-mobs-readback-"));
  try {
    const none = storeReadback(root, mobs);
    assert.deepEqual(none.console_log_files, []);
    assert.match(none.mobs.second.console_log.missing, /no file under the state root holds console_frames/);
    sqliteFile(path.join(root, "console.sqlite3"), [
      CONSOLE_FRAMES,
      frameRow("f1", "default", "reviewer", "run_started", "r1"),
      frameRow("f2", "default", "reviewer", "run_completed", "r1"),
      frameRow("f3", "mob/second", "reviewer", "run_started", "r2"),
      frameRow("f4", "mob/second", "reviewer", "text_delta", "r2"),
      frameRow("f5", "mob/second", "triage", "user_input", null),
    ]);
    const read = storeReadback(root, mobs);
    assert.deepEqual(read.console_log_files, ["console.sqlite3"]);
    assert.equal(read.mobs.launch.console_log.frames, 2);
    assert.deepEqual(read.mobs.launch.member_runs, { reviewer: { run_id: "r1", state: "ended", ended_by: ["run_completed"] } });
    assert.equal(read.mobs.second.console_log.frames, 3);
    assert.deepEqual(read.mobs.second.console_log.members.reviewer.kinds, { run_started: 1, text_delta: 1 });
    assert.deepEqual(read.mobs.second.member_runs, { reviewer: { run_id: "r2", state: "running", ended_by: [] }, triage: { state: "no run" } });
    assert.match(read.mobs.second.runtime_run_state.missing, /not read/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("the frames a peer mob gained are an own-history snapshot repeat, a disclosure or another change", () => {
  const old = [{ id: "a", kind: "text_complete", mob_id: "second", session_id: "s1", payload: {} }];
  const message = { text: "Message for launch 42", answer: "Answer 43." };
  const told = (after, peer = "second") => classifyPeerFrames({ beforeIds: ["a"], after, peerMobId: peer, message, knownSessions: ["s1"] });
  assert.equal(told(old).verdict, "unchanged");
  const snapshots = [...old,
    { id: "b", kind: "runtime_notice_snapshot", mob_id: "second", identity: "reviewer", runtime_key: "mob/second", session_id: "s1", payload: { notices: ["Earlier note"] } },
    { id: "c", kind: "assistant_history_snapshot", mob_id: "second", session_id: "s1", payload: {} }];
  const repeat = told(snapshots);
  assert.equal(repeat.verdict, "own-history snapshot repeat");
  assert.equal(repeat.message_bytes_found, false);
  assert.deepEqual(repeat.extra.map((frame) => [frame.id, frame.kind, frame.mob_id, frame.runtime_key, frame.session_id]), [
    ["b", "runtime_notice_snapshot", "second", "mob/second", "s1"], ["c", "assistant_history_snapshot", "second", null, "s1"],
  ]);
  const disclosed = told([...old, { id: "d", kind: "runtime_notice_snapshot", mob_id: "second", session_id: "s1", payload: { body: "Answer 43." } }]);
  assert.equal(disclosed.verdict, "wrong-target disclosure");
  assert.deepEqual(disclosed.frames_with_message_bytes, ["d"]);
  assert.equal(told([...old, { id: "e", kind: "runtime_notice_snapshot", mob_id: "second", session_id: "s9", payload: {} }]).verdict, "other change", "a session the peer did not have");
  assert.equal(told([...old, { id: "f", kind: "text_complete", mob_id: "second", session_id: "s1", payload: {} }]).verdict, "other change", "a kind that is no snapshot");
  assert.equal(told([...old, { id: "g", kind: "runtime_notice_snapshot", mob_id: "launch", session_id: "s1", payload: {} }]).verdict, "other change", "a frame of another mob");
});

test("an actual stop is told from a run that completed by itself", () => {
  const full = "Long answer that runs on to its end.";
  const stopped = { result: { receipt: { outcome: "stopped", run_id: "r1", contributors: [] } } };
  const frames = (...kinds) => kinds.map((kind, index) => ({ id: `f${index}`, kind, run_id: "r1", payload: kind === "text_complete" ? { content: full } : { delta: "L" } }));
  const actual = classifyStop({ answer: stopped, frames: frames("run_started", "text_delta", "run_cancelled"), fullText: full });
  assert.deepEqual(actual, { verdict: "stopped", accepted: true, outcome: "stopped", run_id: "r1", ended_by: ["run_cancelled"], completed: false });
  assert.equal(classifyStop({ answer: stopped, frames: frames("run_started", "text_delta", "run_completed"), fullText: full }).verdict, "natural completion");
  assert.equal(classifyStop({ answer: stopped, frames: frames("run_started", "text_complete", "interaction_complete"), fullText: full }).verdict, "natural completion", "the whole answer was written");
  assert.equal(classifyStop({ answer: stopped, frames: frames("run_started", "text_delta"), fullText: full }).verdict, "undetermined", "no frame ends the run yet");
  const notCurrent = { result: { receipt: { outcome: "not_current", run_id: "r1" } } };
  const late = classifyStop({ answer: notCurrent, frames: frames("run_started", "text_delta", "run_cancelled"), fullText: full });
  assert.equal(late.verdict, "undetermined", "a stop the gateway did not take is no stop");
  assert.equal(late.accepted, false);
  assert.equal(classifyStop({ answer: null, frames: frames("run_started", "run_completed"), fullText: full }).verdict, "natural completion");
});

/** A multipart body as a browser writes it: a file part, then the JSON-RPC payload part. */
function multipartBody(payload, files = [["file:upload-1", "picture.png"]]) {
  const parts = files.map(([name, filename]) =>
    `--b\r\nContent-Disposition: form-data; name="${name}"; filename="${filename}"\r\nContent-Type: image/png\r\n\r\n\u0089PNG bytes\r\n`);
  return `${parts.join("")}--b\r\nContent-Disposition: form-data; name="payload"\r\n\r\n${JSON.stringify(payload)}\r\n--b--\r\n`;
}

test("a send through the multipart request door is read by its payload part, with its files", () => {
  const payload = { jsonrpc: "2.0", id: "1", method: "meta/console/send", params: { identity: "reviewer", mob_id: "second", content: [{ type: "text", text: "hi" }, { type: "image_upload", upload_id: "upload-1" }] } };
  const read = multipartRequest(multipartBody(payload));
  assert.deepEqual(read, { body: payload, files: [{ part: "file:upload-1", filename: "picture.png" }] });
  assert.equal(multipartRequest(JSON.stringify(payload)), null, "a plain JSON body is no multipart body");
  assert.equal(multipartRequest('--b\r\nContent-Disposition: form-data; name="payload"\r\n\r\nnot json\r\n--b--'), null);
  const records = [
    { seq: 0, method: "POST", path: "/console/rpc/multipart", body: multipartBody(payload), status: 200, response: '{"result":{}}', finishedAt: 1 },
    { seq: 1, method: "POST", path: "/console/other", body: multipartBody(payload), status: 200, response: "{}", finishedAt: 1 },
  ];
  const calls = callsOf(ACTIONS.send, records);
  assert.deepEqual(calls.map((call) => [call.door, call.params.mob_id, call.params.identity, call.files.length, call.refused]),
    [["meta/console/send (multipart)", "second", "reviewer", 1, false]]);
  assert.deepEqual(callsOf(ACTIONS.inspect, records), []);
});

test("a body is a send carrying a text whether it is JSON or multipart, and only in its params", () => {
  const send = { method: "meta/console/send", params: { content: [{ type: "text", text: "the lost message" }] } };
  assert.equal(sendCarrying(JSON.stringify(send), "the lost message"), true);
  assert.equal(sendCarrying(multipartBody(send), "the lost message"), true);
  assert.equal(sendCarrying(JSON.stringify(send), "another message"), false);
  assert.equal(sendCarrying(JSON.stringify({ ...send, method: "meta/console/query_timeline" }), "the lost message"), false);
  assert.equal(sendCarrying(JSON.stringify({ method: "meta/console/send", id: "the lost message", params: {} }), "the lost message"), false);
  assert.equal(sendCarrying("", "the lost message"), false);
});

test("the model request that carries a message with the exact bytes of its file is found", () => {
  const user = (text, data) => ({ role: "user", content: [{ type: "text", text }, { type: "image", media_type: "image/png", data }] });
  const requests = [
    { messages: [user("Message for second 1", "QUJD")] },
    { messages: [user("Message for second 1", "QUJE")] },
    { messages: [{ role: "assistant", content: [{ type: "text", text: "Message for second 1" }, { type: "image", data: "QUJD" }] }] },
    { messages: [user("Another message", "QUJD")] },
    { messages: [{ role: "system", content: "x" }, user("Message for second 1 and more", "QUJD")] },
  ];
  assert.deepEqual(modelRequestsCarrying(requests, "Message for second 1", "QUJD"), [0, 4]);
  assert.deepEqual(modelRequestsCarrying(null, "x", "QUJD"), []);
});

test("an input frame's files are its image blocks' blobs, and a frame holding a needle is found", () => {
  const input = { id: "f1", kind: "user_input", payload: { content: [{ type: "text", text: "hi" }, { type: "image", blob_id: "blob-1" }, { type: "image" }] } };
  assert.deepEqual(imageBlobIds(input), ["blob-1"]);
  assert.deepEqual(imageBlobIds({ payload: { content: "text" } }), []);
  assert.deepEqual(imageBlobIds(undefined), []);
  const frames = [input, { id: "f2", payload: { text: "QUJDREVG" } }, { id: "f3", payload: { text: "nothing" } }];
  assert.deepEqual(framesHolding(frames, ["blob-1", "REVG", ""]), ["f1", "f2"]);
  assert.deepEqual(framesHolding(frames, ["", null]), [], "an empty needle finds nothing");
});

test("the console log's rows that hold a file are read per runtime key from the store's file", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "two-mobs-holding-"));
  try {
    assert.match(storeHolding(root, ["blob-1"]).missing, /no file under the state root holds console_frames/);
    sqliteFile(path.join(root, "meta_console.sqlite3"), [
      "CREATE TABLE console_frames (cursor_seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL, runtime_key TEXT NOT NULL, payload_json TEXT NOT NULL)",
      "INSERT INTO console_frames (id, runtime_key, payload_json) VALUES ('a', 'mob/second', '{\"blob_id\":\"blob-1\"}'), ('b', 'default', '{\"text\":\"QUJDREVG\"}'), ('c', 'default', '{}')",
    ]);
    assert.deepEqual(storeHolding(root, ["blob-1", "REVG", ""]), { by_runtime_key: { "mob/second": ["a"], default: ["b"] } });
    assert.deepEqual(storeHolding(root, ["nowhere"]), { by_runtime_key: {} });
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("a file is looked for by runs from inside it, raw and as base64, and two files of one size share none", () => {
  // 400 bytes in which no run comes twice.
  const bytes = Buffer.concat(Array.from({ length: 13 }, (_, index) => createHash("sha256").update(`block ${index}`).digest())).subarray(0, 400);
  const runs = fileNeedles(bytes);
  assert.deepEqual(runs.raw.map((run) => run.length), [48, 48, 48]);
  assert.deepEqual(runs.base64.map((run) => run.length), [64, 64, 64]);
  assert.deepEqual(runs.raw.map((run) => bytes.indexOf(run)), [100, 200, 300], "a quarter, the middle and three quarters of the file");
  const base64 = bytes.toString("base64");
  assert.deepEqual(runs.base64.map((run) => base64.indexOf(run)), [134, 268, 402]);
  // The start and the end, which two pictures of one size share, are in no run.
  const other = Buffer.concat([bytes.subarray(0, 60), Buffer.alloc(280, 7), bytes.subarray(340)]);
  assert.deepEqual(runsSharedWith(runs, other), []);
  assert.deepEqual(runsSharedWith(runs, bytes).length, 6, "a file shares every run with itself");
  assert.deepEqual(runsSharedWith(runs, Buffer.concat([Buffer.alloc(3), bytes.subarray(190, 260)])), [runs.raw[1].toString("base64")],
    "a raw run is found wherever it sits; a base64 run only where the text lines up");
  const short = fileNeedles(Buffer.from("short"));
  assert.deepEqual(short.raw.map((run) => run.toString()), ["short", "short", "short"]);
});

test("the files under a root that hold a run of bytes or a text are found by name, and an empty needle finds none", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "two-mobs-files-"));
  try {
    fs.mkdirSync(path.join(root, "blobs"));
    fs.writeFileSync(path.join(root, "blobs", "one.bin"), Buffer.from([0, 1, 2, 250, 251, 252, 3, 4]));
    fs.writeFileSync(path.join(root, "log.txt"), "a line with QUJDREVG in it");
    fs.writeFileSync(path.join(root, "other.txt"), "nothing");
    assert.deepEqual(filesHolding(root, [Buffer.from([250, 251, 252])]), ["blobs/one.bin"]);
    assert.deepEqual(filesHolding(root, ["QUJDREVG"]), ["log.txt"]);
    assert.deepEqual(filesHolding(root, [Buffer.from([250, 251, 252]), "QUJDREVG"]), ["blobs/one.bin", "log.txt"]);
    assert.deepEqual(filesHolding(root, [Buffer.from([252, 251, 250]), "nowhere"]), []);
    assert.deepEqual(filesHolding(root, ["", Buffer.alloc(0), null]), [], "an empty needle finds nothing");
    assert.deepEqual(filesHolding(path.join(root, "absent"), ["nothing"]), []);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("a typed refusal is an error with an integer code and a kind, and no result", () => {
  const refusal = { jsonrpc: "2.0", id: 1, error: { code: -32602, message: "not served", data: { kind: "foreign_mob_target", requested_mob_id: "second" } } };
  assert.equal(typedRefusalFault(refusal), null);
  assert.match(typedRefusalFault({ result: { interaction_id: "i" } }), /carries a result/);
  assert.match(typedRefusalFault({ result: {}, error: refusal.error }), /carries a result/);
  assert.match(typedRefusalFault({ error: "foreign_mob_target" }), /no error/);
  assert.match(typedRefusalFault({ error: { code: "-32602", data: { kind: "x" } } }), /code/);
  assert.match(typedRefusalFault({ error: { code: -32602, message: "refused" } }), /no kind/);
  assert.match(typedRefusalFault({ error: { code: -32602, data: { kind: "" } } }), /no kind/);
  assert.match(typedRefusalFault(null), /no JSON answer/);
  assert.match(typedRefusalFault("refused"), /no JSON answer/);
});

test("the door's refusal of a mob id is told by its status, code, kind and the id it names, and by nothing more", () => {
  const refusal = (id) => ({ jsonrpc: "2.0", id: 1, error: { code: -32602, message: `request targets mob '${id}', which this gateway does not serve to the caller`, data: { kind: "foreign_mob_target", requested_mob_id: id } } });
  assert.deepEqual(MOB_REFUSAL, { status: 200, code: -32602, kind: "foreign_mob_target" });
  assert.equal(mobRefusalFault(200, refusal("second"), "second"), null);
  // The words of the message are not held to anything.
  const reworded = refusal("second");
  reworded.error.message = "other words";
  assert.equal(mobRefusalFault(200, reworded, "second"), null);
  assert.match(mobRefusalFault(400, refusal("second"), "second"), /HTTP status is 400/);
  const changed = (change) => { const answer = refusal("second"); change(answer.error); return answer; };
  assert.match(mobRefusalFault(200, changed((error) => { error.code = -32600; }), "second"), /code is -32600/);
  assert.match(mobRefusalFault(200, changed((error) => { error.data.kind = "invalid_params"; }), "second"), /kind is "invalid_params"/);
  assert.match(mobRefusalFault(200, changed((error) => { error.data.requested_mob_id = "launch"; }), "second"), /names "launch" as the requested mob/);
  assert.match(mobRefusalFault(200, changed((error) => { delete error.data.requested_mob_id; }), "second"), /as the requested mob/);
  assert.match(mobRefusalFault(200, changed((error) => { error.data.upload_id = "upload-1"; }), "second"), /also carries upload_id/);
  // An answer that is no typed refusal is said as that first.
  assert.match(mobRefusalFault(200, { result: { interaction_id: "i" } }, "second"), /carries a result/);
  assert.match(mobRefusalFault(200, null, "second"), /no JSON answer/);
});

test("a message with a file for another mob's member is made from the page's own: its other members kept, its own replaced", () => {
  const page = {
    identity: "reviewer", origin: "console:panel-1", origin_kind: "operator", handling_mode: "queue", expected_caller_scope: "console-caller:v1:abc",
    idempotency_key: "the page's key", content: [{ type: "text", text: "the page's words" }, { type: "image_upload", upload_id: "upload-page", media_type: "image/png", alt: "page.png" }],
  };
  const made = attachmentParamsFor(page, { mobId: "second", identity: "reviewer", text: "other words", uploadId: "upload-2", alt: "second.png", idempotencyKey: "key-2" });
  assert.deepEqual(made, {
    origin: "console:panel-1", origin_kind: "operator", handling_mode: "queue", expected_caller_scope: "console-caller:v1:abc",
    identity: "reviewer", mob_id: "second", idempotency_key: "key-2",
    content: [{ type: "text", text: "other words" }, { type: "image_upload", upload_id: "upload-2", media_type: "image/png", alt: "second.png" }],
  });
  assert.equal(page.idempotency_key, "the page's key", "the page's own params are not changed");
  // A page request that named a mob does not hand that mob on.
  assert.equal(attachmentParamsFor({ ...page, mob_id: "third" }, { mobId: "second", identity: "reviewer", text: "t", uploadId: "u", alt: "a", idempotencyKey: "k" }).mob_id, "second");
});

test("each mob's log is counted from a store readback, and a log that was not read is missing, never a number", () => {
  const readback = { mobs: {
    launch: { console_log: { frames: 12, members: {} } },
    second: { console_log: { missing: "no console log", members: {} } },
    third: {},
  } };
  assert.deepEqual(logCounts(readback), { launch: 12, second: { missing: "no console log" }, third: { missing: "not read" } });
  assert.deepEqual(logCounts(null), {});
  assert.deepEqual(logCounts({ mobs: { launch: { console_log: { frames: 0 } } } }), { launch: 0 });
});

test("the plan sends words to the second mob, a file with the launch mob's own message, and refuses a file for the second mob", () => {
  const ids = PLAN.map(([id]) => id);
  assert(ids.indexOf("recover:sent") < ids.indexOf("recover:offer"), "the send is a row before the check");
  // The second mob's message is words: no row of it is about a file, and its log is read.
  assert.deepEqual(ids.filter((id) => id.startsWith("send:")), ["send:offer", "send:target", "send:effect", "send:log", "send:other-mob", "send:unavailable"]);
  // The launch mob's own message goes out with a file: the control, whose upload the search has to find.
  for (const id of ["send-launch:attachment-request", "send-launch:attachment-model", "send-launch:attachment-stored", "send-launch:attachment-other-mob", "send-launch:log"]) assert(ids.includes(id), id);
  // A file for the second mob's member: not offered on the page, refused at the door, nothing written; after the control.
  assert.deepEqual(ids.filter((id) => id.startsWith("attach:")), ["attach:offer", "attach:unavailable", "attach:refused", "attach:other-mob"]);
  assert(ids.indexOf("send-launch:attachment-stored") < ids.indexOf("attach:refused"), "the control's upload is found before the refusal is asked");
  // The caller scope of the mobs door is a scenario of its own (two signed-in callers): real-hosted-mobs-caller-scope.cjs.
  assert(!NOT_COVERED.some(([id]) => id.startsWith("mobs-door:")));
  assert.equal(require("./real-hosted-mobs-caller-scope.cjs").scenarios[0].id, "real-hosted-mobs-caller-scope");
});
