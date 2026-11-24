"use strict";

// The Console's methods over the mobs of a host (`meta/mobs/*` on
// `POST /console/rpc`), for two signed-in callers of one real gateway.
//
// Both callers hold the `runtime.admin` grant, by a rule that lists no mob.
// Only the administrator also has a rule that lists the second mob and the
// mob that is asleep. The administrator lists and reads the second mob, and
// stops, redefines and resumes it. The other caller is listed neither mob,
// and its read, stop, resume, redefinition and destruction of each, and a
// create under its id, are answered the door's refusal of a mob id it does
// not serve to its caller (`-32602`, `data.kind = "foreign_mob_target"`, the
// requested id and nothing else): the answer the same caller gets for an id
// no mob has. After each refusal the host's mobs are as they were, read
// beside the refused caller: the host's own list (the gateway's stdio door),
// the second mob's revision by the host's own read, its incarnation in the
// administrator's list, and the files of every mob's store. A mob that is
// not loaded stays not loaded.
//
// No page and no browser: the Console's page does not ask these methods, so
// the rows ask the request door themselves, each request with its caller's
// sign-in token.
//
// The gateway is the persistent gateway (the mcp_apps_gateway_fixture
// example) started with no provider key, as the two-mob scenario starts it.
// Its Console sign-in trusts a signing secret the harness makes at the start
// of the run (`crypto.randomBytes`), kept in memory and handed to the
// gateway in its start request alone; the harness signs the two callers'
// tokens with it. The callers' addresses are synthetic, of example.test. The
// secret and the tokens are written to no file: the run looks for them (as
// text, base64 and base64url) in everything it keeps before it writes it.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { createHmac, randomBytes } = require("node:crypto");
const { spawn } = require("node:child_process");
const { setTimeout: delay } = require("node:timers/promises");
const { exampleBackendSpec } = require("../example-backend.cjs");
const { eventually } = require("../acceptance-runtime.cjs");
const { childEnvironment, mobToml, gatewayStdio, checkBook, stopChild, filesUnder, sha256 } = require("./real-hosted-mobs.cjs");

const consoleDir = path.join(__dirname, "..");
const repoRoot = path.resolve(consoleDir, "..");

const LAUNCH_MOB = "console-scope-launch";
const SECOND_MOB = "console-scope-second";
/** A mob of the host that no request has named since the gateway started: not loaded. */
const ASLEEP_MOB = "console-scope-asleep";
/** An id no mob of the host has. */
const NO_MOB = "console-scope-nowhere";

/** The administrator: the admin grant, and a rule that lists the second mob and the mob that is asleep. */
const ADMINISTRATOR = "administrator@example.test";
/** The other caller: the admin grant, and no rule that lists a mob. */
const SCOPED = "scoped-admin@example.test";
/** The access configuration's required administrator; no request is made as it. */
const OWNER = "owner@example.test";

/** The access configuration the gateway starts with (TOML, `runtime_options.access_config_path`). */
function accessToml() {
  return [
    "enabled = true",
    `admins = [${JSON.stringify(OWNER)}]`,
    "",
    "[[rules]]",
    'id = "both-hold-the-admin-grant"',
    `subjects = [${JSON.stringify(ADMINISTRATOR)}, ${JSON.stringify(SCOPED)}]`,
    'actions = ["runtime.admin"]',
    "",
    "[[rules]]",
    'id = "the-administrator-in-the-second-mob"',
    `subjects = [${JSON.stringify(ADMINISTRATOR)}]`,
    'actions = ["runtime.admin", "agent.view"]',
    `mobs = [${JSON.stringify(SECOND_MOB)}, ${JSON.stringify(ASLEEP_MOB)}]`,
    "",
  ].join("\n");
}

/** A mob definition that differs from {@link mobToml}'s by one more profile: a redefinition. */
function redefinedToml(mobId) {
  return `${mobToml(mobId)}\n[profiles.second]\nmodel = "gpt-5.5"\nexternal_addressable = true\n\n[profiles.second.tools]\ncomms = true\n`;
}

/**
 * The requests of the methods over the mobs of a host that name the mob
 * `mobId`, by a word for each, the destroy last: the method and its
 * parameters. A create names the mob in its definition and takes no
 * `mob_id`. `revision` is the revision a redefinition names.
 */
function requestsOver(mobId, revision = 1) {
  return {
    get: ["meta/mobs/get", { mob_id: mobId }],
    stop: ["meta/mobs/lifecycle", { mob_id: mobId, action: "stop" }],
    resume: ["meta/mobs/lifecycle", { mob_id: mobId, action: "resume" }],
    redefine: ["meta/mobs/redefine", { mob_id: mobId, mob_config: redefinedToml(mobId), expected_revision: revision }],
    create: ["meta/mobs/create", { mob_config: mobToml(mobId) }],
    destroy: ["meta/mobs/destroy", { mob_id: mobId }],
  };
}

/** The requests the other caller is refused, in the order it asks them. */
const REFUSED = Object.keys(requestsOver(SECOND_MOB));

/** What the administrator does with the second mob, in order: a definition is replaced while its mob is stopped. */
const ADMINISTRATOR_MANAGES = ["stop", "redefine", "resume"];

const base64url = (value) => Buffer.from(value).toString("base64").replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");

/**
 * A Console sign-in token (HS256) for `email`, as the gateway's shared-secret
 * sign-in reads it: its default issuer and audience, valid for `seconds`.
 */
function signToken(secret, email, now = Math.floor(Date.now() / 1000), seconds = 3600) {
  const header = base64url(JSON.stringify({ alg: "HS256", typ: "JWT" }));
  const claims = base64url(JSON.stringify({
    iss: "http://127.0.0.1/meta-gateway", aud: "persistent-gateway",
    sub: email, email, email_verified: true, iat: now, exp: now + seconds,
  }));
  const signature = createHmac("sha256", secret).update(`${header}.${claims}`).digest("base64").replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
  return `${header}.${claims}.${signature}`;
}

/** The caller a `meta/console/whoami` answer names, or `null` when it names none. */
function principalOf(answer) {
  const principal = answer?.result?.actor?.principal;
  return typeof principal === "string" && principal !== "" ? principal : null;
}

/**
 * A refusal as it reads for any mob id: the error with `mobId` replaced by
 * one word. Two refusals that differ only in the id they name are equal so.
 */
function refusalShape(answer, mobId) {
  if (!answer || answer.error === undefined) return null;
  return JSON.stringify(answer.error).split(mobId).join("<mob>");
}

/**
 * Why `answer` is not the request door's refusal of the mob id `mobId` it
 * does not serve to its caller, or `null` when it is that refusal: `-32602`,
 * `data.kind = "foreign_mob_target"`, the requested id and no other member,
 * and no word of it names one of `otherMobs`.
 */
function mobIdRefusalFault(answer, mobId, otherMobs = []) {
  const error = answer?.error;
  if (!error || typeof error !== "object") return "it is no refusal";
  if (error.code !== -32602) return `its code is ${JSON.stringify(error.code)}`;
  const data = error.data;
  if (!data || typeof data !== "object" || Array.isArray(data)) return "it carries no data";
  if (data.kind !== "foreign_mob_target") return `its kind is ${JSON.stringify(data.kind)}`;
  if (data.requested_mob_id !== mobId) return `it names ${JSON.stringify(data.requested_mob_id)} as the requested mob`;
  const more = Object.keys(data).filter((name) => name !== "kind" && name !== "requested_mob_id");
  if (more.length > 0) return `its data also carries ${more.join(", ")}`;
  const text = JSON.stringify(error);
  const named = otherMobs.filter((id) => id !== mobId && text.includes(id));
  if (named.length > 0) return `it names another mob: ${named.join(", ")}`;
  return null;
}

/** The ids a `meta/mobs/list` answer lists. */
function listedIds(answer) {
  const mobs = answer?.result?.mobs;
  return Array.isArray(mobs) ? mobs.map((mob) => mob?.mob_id).filter((id) => typeof id === "string") : null;
}

/**
 * The files of the mob `mobId` among `files` (paths under the state
 * directory): those that hold its id, without the `-shm` file a database
 * keeps beside itself, which a reader writes too.
 */
function storeFilesOf(files, mobId) {
  return files.filter((file) => file.includes(mobId) && !file.endsWith("-shm"));
}

/** The members of two snapshots that differ, by name. */
function differences(before, after) {
  const names = new Set([...Object.keys(before || {}), ...Object.keys(after || {})]);
  return [...names].filter((name) => JSON.stringify(before?.[name]) !== JSON.stringify(after?.[name])).sort();
}

/**
 * What a snapshot of a mob did not read, by name: a comparison shows nothing
 * unchanged by a member that was not read. The host's list has to hold the
 * mob with a phase, and the mob's store has to have a file; of a mob that is
 * loaded the revision and the incarnation have to be read too (a mob that is
 * not loaded is asked neither: the read of its revision would load it).
 */
function unreadOf(snapshot) {
  const unread = [];
  if (!Array.isArray(snapshot?.inventory) || !snapshot.inventory.some((mob) => mob?.mob_id === snapshot.mob)) unread.push("inventory");
  if (typeof snapshot?.phase !== "string") unread.push("phase");
  if (snapshot?.loaded) {
    if (!Number.isInteger(snapshot.revision)) unread.push("revision");
    if (typeof snapshot.incarnation !== "string" || snapshot.incarnation === "") unread.push("incarnation");
  }
  if (Object.keys(snapshot?.stores?.[snapshot?.mob] ?? {}).length === 0) unread.push("store");
  return unread;
}

/**
 * Ask `read()` every `gapMs` until `needed` readings running are equal, at
 * most `most` times: whether what it reads is at rest, and after how many
 * readings.
 */
async function atRest(read, { needed = 3, most = 30, gapMs = 1_000 } = {}) {
  let last;
  let equal = 0;
  for (let readings = 1; readings <= most; readings += 1) {
    const value = JSON.stringify(await read());
    equal = value === last ? equal + 1 : 1;
    last = value;
    if (equal >= needed) return { at_rest: true, readings };
    if (readings < most) await delay(gapMs);
  }
  return { at_rest: false, readings: most };
}

/** The retained texts (by name) that hold any of `secrets`: as text, as base64 or as base64url. */
function holdersOf(texts, secrets) {
  const needles = secrets.filter((secret) => typeof secret === "string" && secret.length > 0)
    .flatMap((secret) => [secret, Buffer.from(secret).toString("base64").replace(/=+$/, ""), base64url(secret)]);
  return Object.entries(texts).filter(([, text]) => needles.some((needle) => String(text).includes(needle))).map(([name]) => name).sort();
}

/** The rows of the result, in the order they run. */
const PLAN = [
  ["fixture:no-credential", "the gateway starts with no provider key and a home directory of its own"],
  ["fixture:two-callers", "the gateway signs in the two callers by the run's own secret and names each by its own address"],
  ["fixture:asleep", "after a restart on the same state, a mob no request has named is listed not loaded"],
  ["administrator:list", "the administrator is listed the launch mob, the second mob and the mob that is asleep"],
  ["administrator:get", "the administrator reads the second mob"],
  ["fixture:at-rest", "before the other caller asks anything, readings of the two mobs a second apart are equal three times running"],
  ["scoped:list", "the other caller is listed the launch mob and neither the second mob nor the one asleep"],
  ...REFUSED.flatMap((name) => [
    [`scoped:${name}:refused`, `the other caller's ${name} of the second mob is answered the refusal of a mob id the door does not serve to its caller, as its ${name} of an id no mob has is`],
    [`scoped:${name}:unchanged`, "after that refusal the host's mobs, the second mob's phase, revision and incarnation and the files of every mob's store are as before"],
  ]),
  ["scoped:asleep:refused", `the other caller's list leaves the mob that is asleep out, and its ${REFUSED.join(", ")} of it are refused as those of an id no mob has are`],
  ["scoped:asleep:unloaded", "after those requests the mob that is asleep is still not loaded, and the host's mobs and the files of every mob's store are as before"],
  ["administrator:manages", `the administrator still does its ${ADMINISTRATOR_MANAGES.join(", ")} of the second mob, and destroys the mob that is asleep`],
  ["fixture:secret-unretained", "neither the run's signing secret nor a caller's token is in anything the run keeps"],
];

async function runCallerScope() {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Set META_EXAMPLE_BIN_DIR to the prebuilt native backend examples; this scenario never starts Cargo.");
  const evidenceDir = path.join(process.env.META_BROWSER_EVIDENCE || path.join(consoleDir, "../output/playwright/console-acceptance"), "real-hosted-mobs-caller-scope");
  fs.mkdirSync(evidenceDir, { recursive: true });
  const stateRoot = fs.mkdtempSync(path.join(os.tmpdir(), "meta-console-scope-"));
  const privateHome = path.join(stateRoot, "home");
  for (const dir of ["config", "state", "data", "cache", "tmp"]) fs.mkdirSync(path.join(privateHome, dir), { recursive: true });
  const stateDir = path.join(stateRoot, "gateway-state");
  fs.mkdirSync(stateDir);
  const accessPath = path.join(stateRoot, "access.toml");
  fs.writeFileSync(accessPath, accessToml());
  // The run's own signing secret: in memory, in the start request, nowhere else.
  const secret = randomBytes(32).toString("hex");
  const tokens = { administrator: signToken(secret, ADMINISTRATOR), scoped: signToken(secret, SCOPED) };
  const emails = { administrator: ADMINISTRATOR, scoped: SCOPED };

  const book = checkBook(PLAN, []);
  const evidence = { snapshots: [], answers: [] };
  let child = null;
  let logs = "";
  let backendUrl = null;
  const env = childEnvironment(process.env, privateHome);
  const initParams = {
    mob_config: mobToml(LAUNCH_MOB),
    persistent_state: stateDir,
    runtime_options: {
      auth_config: { provider: "jwt", shared_secret: secret, email_allowlist: [ADMINISTRATOR, SCOPED, OWNER] },
      access_config_path: accessPath,
    },
  };
  const start = async () => {
    const spec = exampleBackendSpec(repoRoot, "mcp_apps_gateway_fixture");
    child = spawn(spec.command, spec.args, { cwd: repoRoot, stdio: ["pipe", "pipe", "pipe"], env });
    for (const output of [child.stdout, child.stderr]) output.on("data", (chunk) => { logs = (logs + chunk).slice(-60_000); });
    const gateway = gatewayStdio(child);
    const init = await gateway.request("meta/init", initParams, 120_000);
    assert.equal(typeof init.http_base_url, "string", `meta/init answers the gateway's address: ${JSON.stringify(Object.keys(init || {}))}`);
    backendUrl = init.http_base_url;
    return gateway;
  };
  /** `method` on the Console's request door, as `caller`. */
  const consoleRpc = async (caller, method, params = {}) => {
    const response = await fetch(`${backendUrl}/console/rpc`, {
      method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${tokens[caller]}` },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
    });
    const answer = await response.json().catch(() => null);
    evidence.answers.push({ caller, method, params, status: response.status, answer });
    return answer;
  };

  try {
    await book.run("fixture:no-credential", async () => {
      const names = Object.keys(env).sort();
      assert.deepEqual(names.filter((name) => /KEY|TOKEN|SECRET|CREDENTIAL/i.test(name)), [], "no variable that names a key or a token");
      return { variables: names };
    });
    let gateway = await start();
    await gateway.request("meta/mobs/create", { mob_config: mobToml(SECOND_MOB) });
    await gateway.request("meta/mobs/create", { mob_config: mobToml(ASLEEP_MOB) });
    // A restart on the same state: a mob no request names after it is not loaded.
    await stopChild(child);
    gateway = await start();

    /** The host's own list of its mobs (stdio): it loads no mob and reads no mob's store. */
    const hostList = async () => (await gateway.request("meta/mobs/list", {}))?.mobs ?? [];
    /** The incarnation of each mob the administrator is listed, from its read of the experience over every mob, which loads none. */
    const incarnations = async () => {
      const response = await fetch(`${backendUrl}/console/experience?mobs=*`, { headers: { authorization: `Bearer ${tokens.administrator}` } });
      const document = await response.json().catch(() => null);
      return Object.fromEntries((document?.topology?.live_snapshot?.mobs ?? []).map((mob) => [mob.mob_id, mob.incarnation ?? null]));
    };
    /** The files of a mob's store, each with its size and digest. */
    const storeOf = (files, mobId) => Object.fromEntries(storeFilesOf(files, mobId).map((file) => {
      try {
        const bytes = fs.readFileSync(path.join(stateDir, file));
        return [file, { bytes: bytes.length, sha256: sha256(bytes) }];
      } catch (error) {
        return [file, { missing: error.code || error.message }];
      }
    }));
    /**
     * What has to be unchanged of `mobId` after a refusal, read beside the
     * refused caller. Of a mob that is loaded the revision is read first, by
     * the host's own read of the mob (a read of one mob loads it, so a mob
     * that is not loaded is not asked). The stores are those of every mob
     * beside the launch mob, and of the id no mob has.
     */
    const reading = async (mobId, { loaded }) => {
      const revision = loaded
        ? (await gateway.request("meta/mobs/get", { mob_id: mobId }).catch(() => null))?.mob?.revision ?? null
        : null;
      const inventory = (await hostList()).map((mob) => ({ mob_id: mob.mob_id, state: mob.state }));
      const incarnation = (await incarnations())[mobId] ?? null;
      const files = filesUnder(stateDir);
      return {
        mob: mobId, loaded, inventory,
        phase: inventory.find((mob) => mob.mob_id === mobId)?.state ?? null,
        revision, incarnation,
        stores: Object.fromEntries([SECOND_MOB, ASLEEP_MOB, NO_MOB].map((id) => [id, storeOf(files, id)])),
      };
    };
    const snapshot = async (mobId, label, options) => {
      const value = await reading(mobId, options);
      evidence.snapshots.push({ label, ...value });
      return value;
    };
    const unchangedAfter = async (before, label) => {
      const after = await snapshot(before.mob, label, { loaded: before.loaded });
      const changed = differences(before, after);
      assert.deepEqual(changed, [], `${before.mob} changed in ${JSON.stringify(changed)}: before ${JSON.stringify(before)} after ${JSON.stringify(after)}`);
      assert.deepEqual(unreadOf(before), [], `the reading of ${before.mob} did not read these, so it shows none of them unchanged: ${JSON.stringify(before)}`);
      return after;
    };

    await book.run("fixture:two-callers", async () => {
      const named = {};
      for (const caller of ["administrator", "scoped"]) {
        const answer = await consoleRpc(caller, "meta/console/whoami");
        assert(answer && answer.error === undefined, `${caller} is signed in: ${JSON.stringify(answer)}`);
        named[caller] = principalOf(answer);
        assert.equal(named[caller], emails[caller], `${caller} is named by its own address: ${JSON.stringify(answer.result)}`);
      }
      return named;
    });
    let asleep = false;
    await book.section("asleep", () => book.run("fixture:asleep", async () => {
      const entry = (await hostList()).find((mob) => mob.mob_id === ASLEEP_MOB);
      asleep = entry?.state === "dormant";
      assert(asleep, `the mob no request named since the restart is not loaded: ${JSON.stringify(entry)}`);
      return entry;
    }));

    await book.section("administrator-reads", async () => {
      await book.run("administrator:list", async () => {
        const ids = listedIds(await consoleRpc("administrator", "meta/mobs/list"));
        assert(ids, "the administrator's list answers mobs");
        for (const id of [LAUNCH_MOB, SECOND_MOB, ASLEEP_MOB]) assert(ids.includes(id), `the administrator is listed ${id}: ${JSON.stringify(ids)}`);
        return ids;
      });
      await book.run("administrator:get", async () => {
        const answer = await consoleRpc("administrator", ...requestsOver(SECOND_MOB).get);
        assert.equal(answer?.result?.mob?.mob_id, SECOND_MOB, `the administrator reads the second mob: ${JSON.stringify(answer)}`);
        return answer.result;
      });
    });

    // The start's own reads of its stored mobs end before the comparisons begin.
    await book.section("at-rest", () => book.run("fixture:at-rest", async () => {
      const rest = await atRest(async () => [
        await reading(SECOND_MOB, { loaded: true }),
        ...(asleep ? [await reading(ASLEEP_MOB, { loaded: false })] : []),
      ]);
      assert(rest.at_rest, `the two mobs' readings did not come to rest in ${rest.readings} readings`);
      return rest;
    }));

    await book.section("scoped-list", () => book.run("scoped:list", async () => {
      const ids = listedIds(await consoleRpc("scoped", "meta/mobs/list"));
      assert(ids, "the other caller's list answers mobs");
      assert(ids.includes(LAUNCH_MOB), `the launch mob is listed: ${JSON.stringify(ids)}`);
      assert.deepEqual(ids.filter((id) => id === SECOND_MOB || id === ASLEEP_MOB), [], `neither the second mob nor the one asleep is listed: ${JSON.stringify(ids)}`);
      return ids;
    }));

    /**
     * The other caller's request `name` of `mobId`: the door's refusal of
     * that id, and the answer the same caller gets for an id no mob has.
     */
    const refusedAsNoMob = async (name, mobId, revision) => {
      const answer = await consoleRpc("scoped", ...requestsOver(mobId, revision)[name]);
      const nowhere = await consoleRpc("scoped", ...requestsOver(NO_MOB, revision)[name]);
      const fault = mobIdRefusalFault(answer, mobId, [LAUNCH_MOB, SECOND_MOB, ASLEEP_MOB, NO_MOB]);
      assert.equal(fault, null, `${name} of ${mobId} is not the refusal of a mob id the door does not serve to its caller: ${fault}: ${JSON.stringify(answer)}`);
      assert.equal(refusalShape(answer, mobId), refusalShape(nowhere, NO_MOB), "the refusal is the answer for an id no mob has");
      return answer.error;
    };
    for (const name of REFUSED) {
      await book.section(`scoped-${name}`, async () => {
        const before = await snapshot(SECOND_MOB, `before the other caller's ${name}`, { loaded: true });
        try {
          await book.run(`scoped:${name}:refused`, () => refusedAsNoMob(name, SECOND_MOB, before.revision ?? 1));
        } finally {
          await book.run(`scoped:${name}:unchanged`, () => unchangedAfter(before, `after the other caller's ${name}`));
        }
      });
    }

    await book.section("scoped-asleep", async () => {
      if (!asleep) {
        for (const id of ["scoped:asleep:refused", "scoped:asleep:unloaded"]) book.notApplicable(id, "no mob of this run is listed not loaded");
        return;
      }
      const before = await snapshot(ASLEEP_MOB, "before the other caller's requests of the mob that is asleep", { loaded: false });
      try {
        await book.run("scoped:asleep:refused", async () => {
          const ids = listedIds(await consoleRpc("scoped", "meta/mobs/list"));
          assert(ids && !ids.includes(ASLEEP_MOB), `the other caller's list leaves the mob that is asleep out: ${JSON.stringify(ids)}`);
          const refusals = {};
          for (const name of REFUSED) refusals[name] = await refusedAsNoMob(name, ASLEEP_MOB, 1);
          return refusals;
        });
      } finally {
        await book.run("scoped:asleep:unloaded", async () => {
          const after = await unchangedAfter(before, "after the other caller's requests of the mob that is asleep");
          assert.equal(after.phase, "dormant", "the mob is still not loaded");
          return after;
        });
      }
    });

    await book.section("administrator-manages", () => book.run("administrator:manages", async () => {
      const done = {};
      /** The second mob's phase in the host's own list. */
      const phaseIs = (state) => eventually(async () => (await hostList()).find((mob) => mob.mob_id === SECOND_MOB)?.state === state, `the host lists the second mob ${state}`, 30_000);
      for (const name of ADMINISTRATOR_MANAGES) {
        let revision;
        if (name === "redefine") {
          // The revision the administrator read: a definition is replaced on it.
          const read = await consoleRpc("administrator", ...requestsOver(SECOND_MOB).get);
          revision = read?.result?.mob?.revision;
          assert(Number.isInteger(revision), `the administrator reads the second mob's revision: ${JSON.stringify(read)}`);
        }
        const answer = await consoleRpc("administrator", ...requestsOver(SECOND_MOB, revision)[name]);
        assert.equal(answer?.result?.mob?.mob_id, SECOND_MOB, `the administrator's ${name} of the second mob is served: ${JSON.stringify(answer)}`);
        done[name] = answer.result;
        if (name === "stop") await phaseIs("stopped");
        if (name === "resume") await phaseIs("running");
        if (name === "redefine") {
          assert.equal(answer.result.receipt?.previous_revision, revision, `the definition was replaced on the revision read: ${JSON.stringify(answer.result.receipt)}`);
          assert(answer.result.receipt?.committed_revision > revision, `the mob is at a later revision: ${JSON.stringify(answer.result.receipt)}`);
        }
      }
      const destroyed = await consoleRpc("administrator", ...requestsOver(ASLEEP_MOB).destroy);
      assert.equal(destroyed?.result?.mob_id, ASLEEP_MOB, `the administrator's destroy of the mob that is asleep is served: ${JSON.stringify(destroyed)}`);
      done.destroy = destroyed.result;
      await eventually(async () => !(await hostList()).some((mob) => mob.mob_id === ASLEEP_MOB), "the destroyed mob leaves the host's list", 30_000);
      done.files_left_of_the_destroyed_mob = storeFilesOf(filesUnder(stateDir), ASLEEP_MOB);
      return done;
    }));

    assert.deepEqual(book.failed(), [], `every row passed or did not apply; the parts: ${JSON.stringify(book.sections)}`);
    process.stdout.write("real hosted mobs caller scope ok\n");
  } finally {
    await stopChild(child).catch(() => {});
    // What the run keeps, checked for the secret and the tokens before it is written.
    const draft = () => JSON.stringify({ scenario: "real-hosted-mobs-caller-scope", rows: book.rows, sections: book.sections, evidence }, null, 2);
    const holders = holdersOf({ "result.json": draft(), "gateway.log": logs }, [secret, ...Object.values(tokens)]);
    await book.run("fixture:secret-unretained", async () => {
      assert.deepEqual(holders, [], "the signing secret and the callers' tokens are in no file the run keeps");
      return { checked: ["result.json", "gateway.log"] };
    }).catch(() => {});
    const keep = holders.length === 0;
    fs.writeFileSync(path.join(evidenceDir, "result.json"), keep ? draft() : JSON.stringify({ scenario: "real-hosted-mobs-caller-scope", rows: book.rows, withheld: "the evidence held the secret or a token and is not written" }, null, 2));
    fs.writeFileSync(path.join(evidenceDir, "gateway.log"), keep ? logs : "withheld: it held the secret or a token\n");
    fs.rmSync(stateRoot, { recursive: true, force: true });
    if (!keep) throw new Error(`the signing secret or a token was in ${holders.join(", ")}: nothing of it was written`);
  }
}

module.exports = {
  scenarios: [{ id: "real-hosted-mobs-caller-scope", family: "console", backend: "real", run: runCallerScope }],
  PLAN, REFUSED, ADMINISTRATOR_MANAGES, ADMINISTRATOR, SCOPED, OWNER, LAUNCH_MOB, SECOND_MOB, ASLEEP_MOB, NO_MOB,
  accessToml, redefinedToml, requestsOver, signToken, principalOf, refusalShape, mobIdRefusalFault, listedIds,
  storeFilesOf, differences, unreadOf, atRest, holdersOf,
};
if (require.main === module) {
  require("../scenario-registry.cjs").runScenarios(module.exports.scenarios).catch((error) => {
    process.stderr.write(`${error.stack || error.message}\n`);
    process.exit(1);
  });
}
