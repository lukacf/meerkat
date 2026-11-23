"use strict";

// The harness parts of the caller scope scenario, without a gateway: the
// access configuration, the callers' tokens, the requests over a mob, the
// refusal of a mob id read for any id, the lists, the snapshots and what one
// did not read, the rest before the comparisons, and the check that the run
// keeps neither its secret nor a token.

const assert = require("node:assert/strict");
const { createHmac } = require("node:crypto");
const { test } = require("node:test");
const {
  scenarios, PLAN, REFUSED, ADMINISTRATOR_MANAGES, ADMINISTRATOR, SCOPED, OWNER, LAUNCH_MOB, SECOND_MOB, ASLEEP_MOB, NO_MOB,
  accessToml, redefinedToml, requestsOver, signToken, principalOf, refusalShape, mobIdRefusalFault, listedIds,
  storeFilesOf, differences, unreadOf, atRest, holdersOf,
} = require("./real-hosted-mobs-caller-scope.cjs");
const { mobToml } = require("./real-hosted-mobs.cjs");

test("the scenario is one real-backend console scenario, run from its own file: the default list does not hold it", () => {
  assert.deepEqual(scenarios.map(({ id, family, backend }) => ({ id, family, backend })),
    [{ id: "real-hosted-mobs-caller-scope", family: "console", backend: "real" }]);
  const listed = require("../browser-e2e.cjs").scenarios.map((scenario) => scenario.id);
  assert(!listed.includes("real-hosted-mobs-caller-scope"));
});

test("both callers hold the admin grant and only the administrator has a rule that lists the second mob", () => {
  const toml = accessToml();
  assert.match(toml, /^enabled = true$/m);
  assert.match(toml, new RegExp(`^admins = \\["${OWNER}"\\]$`, "m"));
  const rules = toml.split("[[rules]]").slice(1);
  assert.equal(rules.length, 2);
  assert.match(rules[0], new RegExp(`subjects = \\["${ADMINISTRATOR}", "${SCOPED}"\\]`));
  assert.match(rules[0], /actions = \["runtime.admin"\]/);
  assert.doesNotMatch(rules[0], /mobs =/, "the shared grant lists no mob");
  assert.match(rules[1], new RegExp(`subjects = \\["${ADMINISTRATOR}"\\]`));
  assert.match(rules[1], new RegExp(`mobs = \\["${SECOND_MOB}", "${ASLEEP_MOB}"\\]`));
  for (const email of [ADMINISTRATOR, SCOPED, OWNER]) assert.match(email, /@example\.test$/, "the callers are synthetic");
  assert.equal(new Set([LAUNCH_MOB, SECOND_MOB, ASLEEP_MOB, NO_MOB]).size, 4);
  for (const id of [LAUNCH_MOB, SECOND_MOB, ASLEEP_MOB, NO_MOB]) {
    assert.deepEqual([LAUNCH_MOB, SECOND_MOB, ASLEEP_MOB, NO_MOB].filter((other) => other !== id && other.includes(id)), [], "no mob's id is a part of another's");
  }
});

test("a caller's token is signed with the run's secret, for that caller, with the gateway's issuer and audience", () => {
  const token = signToken("a-secret", SCOPED, 1_000, 60);
  const [header, claims, signature] = token.split(".");
  const decode = (part) => JSON.parse(Buffer.from(part.replace(/-/g, "+").replace(/_/g, "/"), "base64").toString());
  assert.deepEqual(decode(header), { alg: "HS256", typ: "JWT" });
  assert.deepEqual(decode(claims), {
    iss: "http://127.0.0.1/meta-gateway", aud: "persistent-gateway", sub: SCOPED, email: SCOPED, email_verified: true, iat: 1_000, exp: 1_060,
  });
  const expected = createHmac("sha256", "a-secret").update(`${header}.${claims}`).digest("base64url");
  assert.equal(signature, expected);
  assert.notEqual(signToken("another-secret", SCOPED, 1_000, 60), token);
  assert.doesNotMatch(token, /a-secret/, "the token does not carry the secret");
});

test("the requests over a mob name it as each method reads it, a create in its definition alone, the destroy last", () => {
  const requests = requestsOver("a-mob", 7);
  assert.deepEqual(Object.keys(requests), ["get", "stop", "resume", "redefine", "create", "destroy"]);
  assert.deepEqual(REFUSED, Object.keys(requests));
  assert.deepEqual(requests.get, ["meta/mobs/get", { mob_id: "a-mob" }]);
  assert.deepEqual(requests.stop, ["meta/mobs/lifecycle", { mob_id: "a-mob", action: "stop" }]);
  assert.deepEqual(requests.resume, ["meta/mobs/lifecycle", { mob_id: "a-mob", action: "resume" }]);
  assert.deepEqual(requests.redefine, ["meta/mobs/redefine", { mob_id: "a-mob", mob_config: redefinedToml("a-mob"), expected_revision: 7 }]);
  assert.deepEqual(requests.create, ["meta/mobs/create", { mob_config: mobToml("a-mob") }]);
  assert.deepEqual(requests.destroy, ["meta/mobs/destroy", { mob_id: "a-mob" }]);
  assert.equal(requestsOver("a-mob").redefine[1].expected_revision, 1);
  // A redefinition is the same mob's definition with one more profile, declared as the first is.
  const redefined = redefinedToml("a-mob");
  assert(redefined.startsWith(mobToml("a-mob")));
  assert.notEqual(redefined, mobToml("a-mob"));
  assert.match(redefined, /^\[profiles\.second\]$/m);
  assert.match(redefined, /^\[profiles\.second\.tools\]\ncomms = true$/m);
});

test("the administrator replaces the second mob's definition while it is stopped", () => {
  assert.deepEqual(ADMINISTRATOR_MANAGES, ["stop", "redefine", "resume"]);
  for (const name of ADMINISTRATOR_MANAGES) assert(REFUSED.includes(name), name);
});

test("a caller is named by the address its sign-in answer carries, and by nothing else", () => {
  assert.equal(principalOf({ result: { actor: { kind: "principal", principal: SCOPED } } }), SCOPED);
  assert.equal(principalOf({ result: { actor: { principal: "" } } }), null);
  assert.equal(principalOf({ result: { principal: SCOPED } }), null);
  assert.equal(principalOf({ error: { code: -32056 } }), null);
  assert.equal(principalOf(null), null);
});

/** The request door's refusal of a mob id it does not serve to its caller. */
const refusal = (id) => ({ error: { code: -32602, message: `request targets mob '${id}', which this gateway does not serve to the caller`, data: { kind: "foreign_mob_target", requested_mob_id: id } } });

test("a refusal reads the same for any id, and an answer that is no refusal has no shape", () => {
  assert.equal(refusalShape(refusal("console-scope-second"), "console-scope-second"), refusalShape(refusal("nowhere"), "nowhere"));
  assert.notEqual(refusalShape({ error: { code: -32030, message: "denied" } }, "x"), refusalShape(refusal("x"), "x"));
  assert.equal(refusalShape({ result: { mob: {} } }, "x"), null);
  assert.equal(refusalShape(null, "x"), null);
});

test("the refusal of a mob id is told from every other answer, by its code, its kind, the id it names and nothing more", () => {
  const others = [LAUNCH_MOB, SECOND_MOB, ASLEEP_MOB, NO_MOB];
  assert.equal(mobIdRefusalFault(refusal(SECOND_MOB), SECOND_MOB, others), null);
  assert.equal(mobIdRefusalFault(refusal(NO_MOB), NO_MOB, others), null);
  assert.match(mobIdRefusalFault({ result: { mob: { mob_id: SECOND_MOB } } }, SECOND_MOB, others), /no refusal/);
  assert.match(mobIdRefusalFault(null, SECOND_MOB, others), /no refusal/);
  const changed = (change) => { const answer = refusal(SECOND_MOB); change(answer.error); return answer; };
  assert.match(mobIdRefusalFault(changed((error) => { error.code = -32030; }), SECOND_MOB, others), /code/);
  assert.match(mobIdRefusalFault(changed((error) => { delete error.data; }), SECOND_MOB, others), /no data/);
  assert.match(mobIdRefusalFault(changed((error) => { error.data.kind = "access_denied"; }), SECOND_MOB, others), /kind/);
  assert.match(mobIdRefusalFault(changed((error) => { error.data.requested_mob_id = ASLEEP_MOB; }), SECOND_MOB, others), /requested mob/);
  assert.match(mobIdRefusalFault(changed((error) => { error.data.state = "running"; }), SECOND_MOB, others), /also carries state/);
  assert.match(mobIdRefusalFault(changed((error) => { error.message += ` (the host has ${ASLEEP_MOB})`; }), SECOND_MOB, others), /another mob/);
  // The requested id itself is in the refusal and is no other mob.
  assert.equal(mobIdRefusalFault(refusal(SECOND_MOB), SECOND_MOB, [SECOND_MOB]), null);
});

test("a list's ids are read from its mobs, and nothing else is a list", () => {
  assert.deepEqual(listedIds({ result: { mobs: [{ mob_id: "a" }, { mob_id: 1 }, { mob_id: "b" }] } }), ["a", "b"]);
  assert.equal(listedIds({ error: { code: -32602 } }), null);
  assert.equal(listedIds(null), null);
});

test("a mob's store is the files that hold its id, without the file a reader writes too", () => {
  const files = [
    "meta_console.sqlite3", `mobs/mob--${ASLEEP_MOB}.db`,
    `mobs/mob--${SECOND_MOB}.db`, `mobs/mob--${SECOND_MOB}.db-shm`, `mobs/mob--${SECOND_MOB}.db-wal`, `mobs/mob--${SECOND_MOB}.db.mfence`,
  ];
  assert.deepEqual(storeFilesOf(files, SECOND_MOB), [`mobs/mob--${SECOND_MOB}.db`, `mobs/mob--${SECOND_MOB}.db-wal`, `mobs/mob--${SECOND_MOB}.db.mfence`]);
  assert.deepEqual(storeFilesOf(files, ASLEEP_MOB), [`mobs/mob--${ASLEEP_MOB}.db`]);
  assert.deepEqual(storeFilesOf(files, NO_MOB), []);
});

/** A snapshot of the second mob, loaded, with every member read. */
const loadedSnapshot = () => ({
  mob: SECOND_MOB, loaded: true,
  inventory: [{ mob_id: LAUNCH_MOB, state: "running" }, { mob_id: SECOND_MOB, state: "running" }],
  phase: "running", revision: 1, incarnation: "i1",
  stores: { [SECOND_MOB]: { [`mobs/mob--${SECOND_MOB}.db`]: { bytes: 1, sha256: "a" } }, [ASLEEP_MOB]: {}, [NO_MOB]: {} },
});

test("two snapshots differ in the members that changed, and in none when equal", () => {
  const before = loadedSnapshot();
  assert.deepEqual(differences(before, structuredClone(before)), []);
  const after = structuredClone(before);
  after.revision = 2;
  after.stores[NO_MOB] = { [`mobs/mob--${NO_MOB}.db`]: { bytes: 1, sha256: "b" } };
  assert.deepEqual(differences(before, after), ["revision", "stores"]);
  assert.deepEqual(differences(before, { ...before, extra: true }), ["extra"]);
});

test("a snapshot says what it did not read, and a mob that is not loaded is asked neither revision nor incarnation", () => {
  assert.deepEqual(unreadOf(loadedSnapshot()), []);
  assert.deepEqual(unreadOf({ ...loadedSnapshot(), revision: null }), ["revision"]);
  assert.deepEqual(unreadOf({ ...loadedSnapshot(), incarnation: null }), ["incarnation"]);
  assert.deepEqual(unreadOf({ ...loadedSnapshot(), incarnation: "" }), ["incarnation"]);
  assert.deepEqual(unreadOf({ ...loadedSnapshot(), phase: null }), ["phase"]);
  assert.deepEqual(unreadOf({ ...loadedSnapshot(), inventory: [{ mob_id: LAUNCH_MOB, state: "running" }] }), ["inventory"]);
  assert.deepEqual(unreadOf({ ...loadedSnapshot(), stores: { [SECOND_MOB]: {}, [ASLEEP_MOB]: { "x.db": {} } } }), ["store"]);
  assert.deepEqual(unreadOf({ ...loadedSnapshot(), loaded: false, revision: null, incarnation: null }), []);
  assert.deepEqual(unreadOf(null), ["inventory", "phase", "store"]);
});

test("what is read is at rest after as many equal readings running as asked, and not before", async () => {
  const reads = (values) => { let next = 0; return async () => values[Math.min(next++, values.length - 1)]; };
  assert.deepEqual(await atRest(reads(["a", "a", "a"]), { gapMs: 0 }), { at_rest: true, readings: 3 });
  assert.deepEqual(await atRest(reads(["a", "b", "b", "c", "c", "c"]), { gapMs: 0 }), { at_rest: true, readings: 6 });
  assert.deepEqual(await atRest(reads(["a", "a", "b", "b", "c", "d"]), { needed: 2, gapMs: 0 }), { at_rest: true, readings: 2 });
  let turn = 0;
  assert.deepEqual(await atRest(async () => turn++, { most: 5, gapMs: 0 }), { at_rest: false, readings: 5 });
  assert.deepEqual(await atRest(reads([{ a: 1 }, { a: 1 }, { a: 1 }]), { gapMs: 0 }), { at_rest: true, readings: 3 });
});

test("a kept text holds a secret as text, as base64 or as base64url, and an empty secret is in none", () => {
  const secret = ">>>???0123456789abcdef";
  const base64 = Buffer.from(secret).toString("base64").replace(/=+$/, "");
  const base64url = base64.replace(/\+/g, "-").replace(/\//g, "_");
  assert.notEqual(base64, base64url, "the two forms differ for this secret");
  assert.deepEqual(holdersOf({ a: `x${secret}y`, b: `z${base64}`, c: "clean", d: `k=${base64url}` }, [secret]), ["a", "b", "d"]);
  assert.deepEqual(holdersOf({ a: "anything" }, [""]), []);
  assert.deepEqual(holdersOf({ a: "one", b: "two" }, ["zero", "two"]), ["b"]);
});

test("the plan has a refusal row and an unchanged row for every request over the second mob, after the fixture is at rest", () => {
  const ids = PLAN.map(([id]) => id);
  assert.equal(new Set(ids).size, ids.length, "row ids are unique");
  for (const name of ["get", "stop", "resume", "redefine", "create", "destroy"]) {
    assert(ids.includes(`scoped:${name}:refused`), name);
    assert.equal(ids.indexOf(`scoped:${name}:unchanged`), ids.indexOf(`scoped:${name}:refused`) + 1, name);
  }
  assert(ids.indexOf("fixture:at-rest") < ids.indexOf("scoped:list"), "the rest is taken before the other caller asks");
  assert(ids.indexOf("administrator:get") < ids.indexOf("fixture:at-rest"), "the second mob is read, and so loaded, before the rest is taken");
  for (const id of ["administrator:list", "scoped:asleep:refused", "scoped:asleep:unloaded", "administrator:manages"]) assert(ids.includes(id), id);
  assert.equal(ids[ids.length - 1], "fixture:secret-unretained");
});
