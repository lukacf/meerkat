"use strict";

// Two mobs on one real gateway process, one member name in both, in a real
// browser on the built console.
//
// The gateway is the persistent gateway itself (the mcp_apps_gateway_fixture
// example: the gateway's own composition with the acceptance fixture's
// scripted model as its model client). The harness drives it over stdio as
// an SDK host does: `meta/init` with the launch mob, `meta/spawn_member`,
// `meta/mobs/create` for a second mob, `meta/spawn_member` with that mob's
// id. Both mobs get a member `reviewer`. The page is reached through a
// recording proxy that serves the built console from console/dist and
// forwards every other request to the gateway, so a check reads what the
// browser sent and what the gateway answered. What each mob holds is read
// from the gateway itself, per mob, beside the page.
//
// No provider key and no credential: the gateway starts with a short list of
// plain variables and a home directory of its own, and nothing else of the
// harness's environment. The scripted model answers every member.
//
// It needs a gateway that hosts mobs (`hosted_mobs` in `meta/capabilities`)
// and prebuilt example binaries (META_EXAMPLE_BIN_DIR): it never starts
// Cargo.
//
// The checks are separate rows of one result file. What the gateway holds of
// the other mob after an action is a row of its own. An action the console
// offers for a member of the second mob has to reach that member and no
// other; an action it does not offer has to change neither mob, also after
// the person switched between the mobs. Which of the two holds for an action
// is recorded, and the four actions of OFFERED_FOR_EVERY_MOB have to be
// offered: such an action that is missing fails its row, it is never skipped.
// Each action is a part of its own: a failed row ends its action's part (its
// later rows stay "not reached"), the error is recorded with the part, and
// the next action still runs. The scenario passes only when no row failed
// and every row of its plan ran.
//
// It does not cover two things, and its result file says so on every run
// (NOT_COVERED, rows of verdict "not covered"): that a question or an
// approval of the second mob's member is decided for that member alone, and
// that an action of a work card in that member's chat acts on that mob's
// item alone. Of the first, the question half has no gateway to run on: a
// set of questions the gateway lists carries its asker's name and no mob,
// so a set of the second mob's member is not told from one of the launch
// mob's member of the same name. The approval half has one: a pending
// entry of another mob's member names its mob.
//
// A message to the second mob's member is words; its recovery is checked for
// that member too. A message with a file is taken for the launch mob alone
// (NOT_OFFERED_FOR_ANOTHER_MOB): the launch mob's own goes out with a file,
// as a control; on the second mob's member's composer the page stages no
// image and asks nothing; and the request the page would make, made beside
// the page, is answered the door's refusal of a mob id it does not serve
// (MOB_REFUSAL), after which neither mob's log nor any file under the state
// root holds anything of it.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const { createHash, randomUUID } = require("node:crypto");
const { spawn } = require("node:child_process");
const { setTimeout: delay } = require("node:timers/promises");
const { exampleBackendSpec } = require("../example-backend.cjs");
const { eventually } = require("../acceptance-runtime.cjs");

const consoleDir = path.join(__dirname, "..");
const repoRoot = path.resolve(consoleDir, "..");

const LAUNCH_MOB = "console-two-mobs-launch";
const SECOND_MOB = "console-two-mobs-second";
/** The name both mobs have a member of. */
const SHARED = "reviewer";
const MOBS = [
  { id: LAUNCH_MOB, default: true, members: ["planner", SHARED] },
  { id: SECOND_MOB, default: false, members: [SHARED, "triage"] },
];

/**
 * The member actions of a chat, as requests: the methods and routes that
 * carry one, and the member of the params that names the member.
 */
const ACTIONS = {
  send: { methods: ["meta/console/send"], routes: ["/console/send"], member: "identity" },
  // A message with a file: the same method, on the request door that takes files.
  attach: { methods: ["meta/console/send"], routes: [], member: "identity" },
  inspect: { methods: ["meta/console/inspect_identity", "meta/inspect_identity"], routes: [], member: "identity" },
  stop: { methods: ["meta/stop_member_run"], routes: [], member: "member_id" },
  recover: { methods: ["meta/console/recover_send"], routes: ["/console/send/recover"], member: "identity" },
  retire: { methods: ["meta/retire"], routes: [], member: "identity" },
};

/**
 * The actions the console has to offer for a member of every mob it lists.
 * An action outside this list may be offered or not: offered, it has to
 * reach the member it is shown for; not offered, it has to change nothing.
 */
const OFFERED_FOR_EVERY_MOB = ["send", "inspect", "stop", "recover"];

/**
 * The actions the console must not offer for a member of a mob other than
 * the launch mob: the gateway serves them for the launch mob alone, so the
 * page shows them as unavailable there and asks nothing.
 */
const NOT_OFFERED_FOR_ANOTHER_MOB = ["attach"];

/**
 * Why what the page offers for the second mob's member is wrong for
 * `action`, or `null`: an action it has to offer is missing, or one it must
 * not offer there is offered.
 */
function offerFault(action, offered) {
  if (OFFERED_FOR_EVERY_MOB.includes(action) && !offered) return "the console has to offer this action for a member of every mob it lists";
  if (NOT_OFFERED_FOR_ANOTHER_MOB.includes(action) && offered) return "the console must not offer this action for a member of a mob other than the launch mob";
  return null;
}

/** The rows of the result, in the order they run. */
const PLAN = [
  ["fixture:no-credential", "the gateway starts with no provider key and a home directory of its own"],
  ["fixture:two-mobs", "the gateway hosts two mobs and each has a member of the shared name"],
  ["fixture:bundle", "the gateway serves the console page and assets byte for byte as the page is served them and the tree built them"],
  ["display:mobs", "the page lists the mobs the gateway lists"],
  ["display:members", "each mob's members stand under its header, the shared name once in each, under two keys"],
  ["history:second", "the second mob's member's chat shows its own earlier turn and nothing of its namesake's"],
  ["history:launch", "the launch mob's member's chat shows its own earlier turn and nothing of its namesake's"],
  ["send:offer", "whether a message can be written to the second mob's member"],
  ["send:target", "the message names the second mob's member and is accepted"],
  ["send:effect", "one delivery, to the second mob's member, answered in its chat"],
  ["send:log", "the second mob's log holds the message, read from its file, and the launch mob's log has no row of it and no frame more"],
  ["send:other-mob", "the launch mob holds nothing of that message and is unchanged"],
  ["send:unavailable", "a message that cannot be written changes neither mob, also after switching mobs"],
  ["send-launch:target", "a message with a file to the launch mob's member names no other mob and is accepted"],
  ["send-launch:attachment-request", "that message names the launch mob's member by its bare name and no mob, and carries the file"],
  ["send-launch:effect", "one delivery, to the launch mob's member, answered in its chat"],
  ["send-launch:log", "the launch mob's log holds the message, read from its file, and the second mob's log has no row of it and no frame more"],
  ["send-launch:attachment-model", "the scripted model's recorded request holds the message with the exact bytes of the file"],
  ["send-launch:attachment-stored", "a file under the state root holds the upload's bytes: the search that has to find none of a refused file finds this one"],
  ["send-launch:attachment-other-mob", "the second mob holds no byte of the file, by the gateway's reads and by its store"],
  ["send-launch:other-mob", "the second mob holds nothing of that message and is unchanged"],
  ["attach:offer", "whether an image can be put on a message for the second mob's member: it must not be, a message with a file is taken for the launch mob alone"],
  ["attach:unavailable", "an image dropped on the second mob's member's composer is not staged, the composer says so, no request carries it, and neither mob changes, also after switching mobs"],
  ["attach:refused", "a message with a file that names the second mob's member, made beside the page as the page makes one, is answered the door's refusal of a mob id it does not serve: the code, the kind, the requested id and nothing more"],
  ["attach:other-mob", "after that refusal neither mob's log has a frame more or a row of the message or the file, and no file under the state root holds the file's bytes"],
  ["inspect:offer", "whether the second mob's member's details can be opened"],
  ["inspect:target", "the details read names the second mob's member and is answered"],
  ["inspect:other-mob", "the details read changes neither mob"],
  ["inspect:unavailable", "details that cannot be opened change neither mob, also after switching mobs"],
  ["stop:offer", "whether a run of the second mob's member can be stopped"],
  ["stop:target", "the stop names the second mob's member and is answered"],
  ["stop:effect", "the second mob's member's run stops"],
  ["stop:other-mob", "the run of the launch mob's member of the same name goes on to its end"],
  ["stop:unavailable", "a run that cannot be stopped from the page goes on, and neither mob changes"],
  ["recover:sent", "the page sends the message whose answer is then lost: a free composer and an empty queue first"],
  ["recover:offer", "whether a message whose answer was lost can be checked for the second mob's member"],
  ["recover:target", "the check names the second mob's member and is answered"],
  ["recover:effect", "the message stays delivered once"],
  ["recover:other-mob", "the launch mob holds nothing of that message and is unchanged"],
  ["recover:unavailable", "a message that cannot be checked changes neither mob, also after switching mobs"],
  ["retire:offer", "whether the second mob's member can be retired"],
  ["retire:target", "the retirement names the second mob's member and is answered"],
  ["retire:effect", "the second mob's member leaves its mob"],
  ["retire:other-mob", "the launch mob's member of the same name stays and still answers"],
  ["retire:unavailable", "a member that cannot be retired stays, and neither mob changes"],
];

/**
 * What this scenario does not cover, each as a row of the result that no
 * check fills (verdict "not covered"). The exit code tells the rows of the
 * plan; these rows say on every run what the scenario leaves out, in the
 * result file and on the line that ends a passing run.
 */
const NOT_COVERED = [
  ["decide:second", "a question or an approval of the second mob's member is decided for that member alone"],
  ["card:second", "an action of a work card in the second mob's member's chat acts on that mob's item alone"],
];

// ---------------------------------------------------------------------------
// The gateway process
// ---------------------------------------------------------------------------

/** The variables of the harness's environment the gateway is given. */
const CHILD_ENVIRONMENT_NAMES = ["PATH", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "RUST_BACKTRACE", "RUST_LOG"];

/**
 * The gateway's environment: the listed plain variables of `source` and a
 * home of its own under `privateHome`. Nothing else is handed on, so no key,
 * token or configuration of the harness's user reaches the gateway.
 */
function childEnvironment(source, privateHome) {
  const env = {};
  for (const name of CHILD_ENVIRONMENT_NAMES) {
    if (typeof source[name] === "string") env[name] = source[name];
  }
  return {
    ...env,
    HOME: privateHome,
    XDG_CONFIG_HOME: path.join(privateHome, "config"),
    XDG_STATE_HOME: path.join(privateHome, "state"),
    XDG_DATA_HOME: path.join(privateHome, "data"),
    XDG_CACHE_HOME: path.join(privateHome, "cache"),
    TMPDIR: path.join(privateHome, "tmp"),
  };
}

/** A mob definition with one profile, as `meta/init` and `meta/mobs/create` take it. */
function mobToml(mobId) {
  return [
    "[mob]", `id = ${JSON.stringify(mobId)}`, "",
    "[profiles.agent]", 'model = "gpt-5.5"', "external_addressable = true", "",
    "[profiles.agent.tools]", "comms = true", "",
  ].join("\n");
}

/**
 * The gateway's stdio as its host: one JSON-RPC request per line, answers by
 * id. A request of the gateway to its host is answered "no handler", as a
 * host without providers answers.
 */
function gatewayStdio(child) {
  const pending = new Map();
  let buffer = "";
  let next = 0;
  let exit = null;
  child.once("exit", (code, signal) => {
    exit = { code, signal };
    for (const waiter of pending.values()) {
      waiter.fail(new Error(`gateway exited (code ${code}, signal ${signal}) before answering ${waiter.method}`));
    }
    pending.clear();
  });
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    buffer += chunk;
    let end;
    while ((end = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, end).trim();
      buffer = buffer.slice(end + 1);
      let message;
      try { message = JSON.parse(line); } catch { continue; }
      if (!message || typeof message !== "object") continue;
      if (typeof message.method === "string") {
        if (message.id !== undefined && message.id !== null) {
          child.stdin.write(`${JSON.stringify({
            jsonrpc: "2.0", id: message.id, error: { code: -32601, message: `no handler for ${message.method}` },
          })}\n`);
        }
        continue;
      }
      const waiter = typeof message.id === "string" ? pending.get(message.id) : undefined;
      if (!waiter) continue;
      pending.delete(message.id);
      if (message.error) {
        const error = new Error(`${waiter.method} failed: ${JSON.stringify(message.error)}`);
        error.rpc = message.error;
        waiter.fail(error);
      } else waiter.done(message.result);
    }
  });
  return {
    request(method, params = {}, timeoutMs = 60_000) {
      if (exit) return Promise.reject(new Error(`gateway exited (code ${exit.code}, signal ${exit.signal}) before ${method}`));
      const id = `harness-${++next}`;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error(`${method} got no answer within ${timeoutMs} ms`));
        }, timeoutMs);
        pending.set(id, {
          method,
          done: (value) => { clearTimeout(timer); resolve(value); },
          fail: (error) => { clearTimeout(timer); reject(error); },
        });
        child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
      });
    },
  };
}

async function stopChild(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  await Promise.race([new Promise((resolve) => child.once("exit", resolve)), delay(3_000)]);
  if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
}

// ---------------------------------------------------------------------------
// The recording proxy
// ---------------------------------------------------------------------------

const RECORD_LIMIT = 1_048_576;
/** How much of an event stream's answer a record keeps, in `stream`. */
const STREAM_RECORD_LIMIT = 16 * RECORD_LIMIT;

/**
 * A local server between the page and the gateway. The console's page and
 * its two assets come from `assetsDir`; every other request goes to
 * `upstream()` and is recorded with its body, its status and its answer: an
 * event stream's answer in `stream` (what the page was sent live), any
 * other in `response`.
 */
async function startRecordingProxy({ assetsDir, upstream }) {
  const assets = new Map([
    ["/console", ["index.html", "text/html; charset=utf-8"]],
    ["/console/", ["index.html", "text/html; charset=utf-8"]],
    ["/console/assets/console-app.js", ["console-app.js", "text/javascript"]],
    ["/console/assets/console-app.css", ["console-app.css", "text/css"]],
  ].map(([url, [name, type]]) => [url, { type, bytes: fs.readFileSync(path.join(assetsDir, name)) }]));
  const records = [];
  const server = http.createServer((req, res) => {
    const pathname = new URL(req.url, "http://console.test").pathname;
    const asset = req.method === "GET" ? assets.get(pathname) : undefined;
    if (asset) {
      res.writeHead(200, { "content-type": asset.type, "cache-control": "no-store" });
      res.end(asset.bytes);
      return;
    }
    const base = upstream();
    if (!base) { res.writeHead(503); res.end(); return; }
    const target = new URL(req.url, base);
    const record = {
      seq: records.length, atMs: Date.now(), method: req.method, path: target.pathname,
      query: [...target.searchParams.entries()], body: "", status: null, response: "", finishedAt: null,
    };
    records.push(record);
    const isStream = target.pathname.endsWith("/stream");
    if (isStream) record.stream = "";
    req.on("data", (chunk) => { if (record.body.length < RECORD_LIMIT) record.body += chunk; });
    const outgoing = http.request(target, { method: req.method, headers: { ...req.headers, host: target.host } }, (incoming) => {
      record.status = incoming.statusCode;
      res.writeHead(incoming.statusCode, incoming.headers);
      incoming.on("data", (chunk) => {
        if (isStream) { if (record.stream.length < STREAM_RECORD_LIMIT) record.stream += chunk; }
        else if (record.response.length < RECORD_LIMIT) record.response += chunk;
      });
      incoming.on("end", () => { record.finishedAt = Date.now(); });
      incoming.on("error", (error) => res.destroy(error));
      res.on("close", () => incoming.destroy());
      incoming.pipe(res);
    });
    outgoing.on("error", () => { if (!res.headersSent) res.writeHead(502); res.end(); });
    req.pipe(outgoing);
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  return {
    baseUrl: `http://127.0.0.1:${server.address().port}`,
    records,
    /** The position after the newest record: `since(mark)` are the requests made from there on. */
    mark: () => records.length,
    since: (mark) => records.slice(mark),
    async close() {
      server.closeAllConnections?.();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

function parsed(text) {
  try { return JSON.parse(text); } catch { return null; }
}

/**
 * The requests among `records` that carry `action`: on the request door by
 * method, on a route by path. Each with its params, whether it was answered
 * yet, and whether the gateway refused it.
 */
/**
 * The JSON-RPC request of a multipart request door's body (its `payload`
 * part) and the names of its file parts (`file:<upload id>`), else null.
 */
function multipartRequest(text) {
  const at = text.indexOf('name="payload"');
  if (at < 0) return null;
  const start = text.indexOf("\r\n\r\n", at);
  if (start < 0) return null;
  const end = text.indexOf("\r\n--", start + 4);
  const body = parsed(text.slice(start + 4, end < 0 ? undefined : end));
  if (!body || typeof body !== "object") return null;
  const files = [...text.matchAll(/name="(file:[^"]+)"(?:; filename="([^"]*)")?/g)].map((match) => ({ part: match[1], filename: match[2] ?? null }));
  return { body, files };
}

/**
 * Whether a request body (JSON-RPC, or a multipart body whose payload part
 * is JSON-RPC) is a send of the console that carries `text`.
 */
function sendCarrying(raw, text) {
  const body = parsed(raw) ?? multipartRequest(raw)?.body ?? null;
  return Boolean(body && ACTIONS.send.methods.includes(body.method) && JSON.stringify(body.params ?? {}).includes(text));
}

function callsOf(action, records) {
  const calls = [];
  for (const record of records) {
    if (record.method !== "POST") continue;
    const multipart = record.path === "/console/rpc/multipart" ? multipartRequest(record.body) : null;
    const body = multipart ? multipart.body : parsed(record.body);
    if (!body || typeof body !== "object") continue;
    const answered = record.finishedAt !== null;
    const answer = answered ? parsed(record.response) : null;
    if ((record.path === "/console/rpc" || multipart) && action.methods.includes(body.method)) {
      calls.push({
        seq: record.seq, door: multipart ? `${body.method} (multipart)` : body.method, params: body.params ?? {}, text: record.body, answered,
        refused: answered && (record.status !== 200 || !answer || answer.error !== undefined), answer,
        files: multipart ? multipart.files : [],
      });
    } else if (action.routes.includes(record.path)) {
      calls.push({
        seq: record.seq, door: record.path, params: body, text: record.body, answered,
        refused: answered && record.status !== 200, answer,
      });
    }
  }
  return calls;
}

/**
 * The member a request names, read from its params: the mob the member sits
 * in when the request says it (`member_mob_id`), else the mob the request is
 * for (`mob_id`), else the launch mob; and the name in `memberField`.
 */
function namedMember(params, memberField, launchMobId) {
  const name = params ? params[memberField] : undefined;
  if (params && typeof params.member_mob_id === "string") return { mob: params.member_mob_id, name, form: "member_mob_id" };
  if (params && typeof params.mob_id === "string") return { mob: params.mob_id, name, form: "mob_id" };
  return { mob: launchMobId, name, form: "no mob" };
}

// ---------------------------------------------------------------------------
// The roster as the page lays it out
// ---------------------------------------------------------------------------

/**
 * The page's mob headers and agent rows in document order, grouped: each
 * agent row belongs to the header before it. A row's key is what the page
 * keys its chat by; no check derives a key from a name.
 */
function groupRoster(nodes) {
  const mobs = [];
  let current = null;
  for (const node of nodes) {
    if (node.id.startsWith("sidebar-mob:")) {
      current = { mobId: node.id.slice("sidebar-mob:".length), agents: [] };
      mobs.push(current);
    } else if (node.id.startsWith("sidebar-agent:")) {
      assert(current, `the agent row ${node.id} stands under no mob header`);
      current.agents.push({ key: node.id.slice("sidebar-agent:".length), text: node.text ?? "" });
    }
  }
  return mobs;
}

/** The one row of `mobId` whose label holds `name`. */
function rowOf(mobs, mobId, name) {
  const mob = mobs.find((item) => item.mobId === mobId);
  assert(mob, `the page lists no mob ${mobId}: ${JSON.stringify(mobs.map((item) => item.mobId))}`);
  const word = new RegExp(`(^|[^a-z0-9])${name}([^a-z0-9]|$)`, "i");
  const rows = mob.agents.filter((agent) => word.test(agent.text) || word.test(agent.key));
  assert.equal(rows.length, 1, `${mobId} has one row for ${name}: ${JSON.stringify(mob.agents)}`);
  return rows[0];
}

// ---------------------------------------------------------------------------
// The rows of the result
// ---------------------------------------------------------------------------

function checkBook(plan, notCovered) {
  const rows = [
    ...plan.map(([id, about]) => ({ id, about, verdict: "not reached" })),
    ...notCovered.map(([id, about]) => ({ id, about, verdict: "not covered" })),
  ];
  const row = (id) => {
    const found = rows.find((item) => item.id === id);
    assert(found, `no such row: ${id}`);
    return found;
  };
  const sections = [];
  return {
    rows,
    /** Each action's part of the scenario, with the error that ended it early, if one did. */
    sections,
    /**
     * One action's part: an error in it ends that part (its rows not run yet
     * stay "not reached") and is recorded, and the next action's part runs.
     */
    async section(name, body) {
      try {
        await body();
        sections.push({ name, ended: "complete" });
      } catch (error) {
        sections.push({ name, ended: "early", error: String((error && error.message) || error) });
      }
    },
    /** The ids of the rows that failed. */
    failed: () => rows.filter((item) => item.verdict === "fail").map((item) => item.id),
    /** The ids of the rows the scenario does not cover. */
    notCovered: () => rows.filter((item) => item.verdict === "not covered").map((item) => item.id),
    async run(id, body) {
      const item = row(id);
      try {
        const detail = await body();
        item.verdict = "pass";
        if (detail !== undefined) item.detail = detail;
      } catch (error) {
        item.verdict = "fail";
        item.detail = String((error && error.message) || error);
        throw error;
      }
    },
    /** A row that does not apply to what the page offers, with why. */
    notApplicable(id, why) {
      const item = row(id);
      item.verdict = "not applicable";
      item.detail = why;
    },
  };
}

// ---------------------------------------------------------------------------
// Evidence beside the page: the bundle's bytes, the fixture's stores, the
// frames an action left, and what became of a run that was asked to stop
// ---------------------------------------------------------------------------

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

/** The console's page and its two assets, by the path the page loads them from. */
const BUNDLE_ASSETS = [
  ["index.html", "/console"],
  ["console-app.js", "/console/assets/console-app.js"],
  ["console-app.css", "/console/assets/console-app.css"],
];

/**
 * The sha256 of each asset as the gateway serves it, as the recording proxy
 * served it to the page and as the tree's built files hold it, and whether
 * the three agree. `sources` maps a source name to a map of asset name to
 * bytes (a missing asset is `null`).
 */
function bundleEvidence(sources) {
  const assets = {};
  for (const [name] of BUNDLE_ASSETS) {
    const hashes = {};
    for (const [source, bytesOf] of Object.entries(sources)) {
      const bytes = bytesOf[name];
      hashes[source] = bytes == null ? "missing" : sha256(bytes);
    }
    const values = Object.values(hashes);
    assets[name] = { ...hashes, equal: !values.includes("missing") && new Set(values).size === 1 };
  }
  const index = sources.gateway?.["index.html"];
  const version = index == null ? null : (/console-app\.js\?v=([0-9a-z]+)/.exec(String(index)) || [])[1] || null;
  return { assets, app_script_version: version, equal: Object.values(assets).every((asset) => asset.equal) };
}

/** Whether `file` starts with SQLite's own header. */
function isSqliteFile(file) {
  try {
    const fd = fs.openSync(file, "r");
    try {
      const head = Buffer.alloc(16);
      return fs.readSync(fd, head, 0, 16, 0) === 16 && head.toString("latin1") === "SQLite format 3\u0000";
    } finally { fs.closeSync(fd); }
  } catch { return false; }
}

/** The files under `dir`, relative, sorted. */
function filesUnder(dir) {
  const out = [];
  const walk = (current) => {
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.isFile()) out.push(path.relative(dir, full));
    }
  };
  if (fs.existsSync(dir)) walk(dir);
  return out.sort();
}

/**
 * Run `read(db)` on the SQLite file read only. An answer that cannot be made
 * is `{ missing: <why> }`, never a guess.
 */
function readSqlite(file, read) {
  let DatabaseSync;
  try { ({ DatabaseSync } = require("node:sqlite")); } catch (error) { return { missing: `no SQLite reader in this node: ${error.message}` }; }
  let db;
  try {
    db = new DatabaseSync(file, { readOnly: true });
    return read(db);
  } catch (error) {
    return { missing: `${path.basename(file)}: ${error.message}` };
  } finally {
    try { db?.close(); } catch { /* closed already */ }
  }
}

const tablesOf = (db) => db.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").all().map((row) => row.name);

/**
 * What the fixture is, all of it synthetic: the state root, every file the
 * gateway wrote under it with its size, and for a SQLite file its tables and
 * the schema versions its ledger (`meerkat_schema`) records; the mobs and
 * their members as the harness made them.
 */
function fixtureIdentity(stateRoot, stateDir, mobs) {
  const files = filesUnder(stateDir).map((relative) => {
    const full = path.join(stateDir, relative);
    const entry = { path: relative, bytes: fs.statSync(full).size };
    if (!isSqliteFile(full)) return entry;
    entry.sqlite = readSqlite(full, (db) => {
      const tables = tablesOf(db);
      const schema = tables.includes("meerkat_schema")
        ? db.prepare("SELECT domain, version FROM meerkat_schema ORDER BY domain").all().map((row) => ({ domain: row.domain, version: row.version }))
        : { missing: "no meerkat_schema ledger in this file" };
      return { tables, schema };
    });
    return entry;
  });
  return {
    state_root: stateRoot,
    state_dir: stateDir,
    files,
    mobs: mobs.map((mob) => ({ id: mob.id, default: mob.default, members: [...mob.members] })),
  };
}

/** The console log's key for the frames of `mob`: the launch mob's own, or the hosted mob's. */
const runtimeKeyOf = (mob) => (mob.default ? "default" : `mob/${mob.id}`);

/** Frame kinds that end a run; a stop that lands ends its run with one that is no completion. */
const RUN_END_KINDS = ["run_completed", "run_failed", "run_cancelled", "interaction_failed", "interaction_complete"];

/**
 * What the stores under `stateDir` hold of each mob, read from the SQLite
 * files themselves and not through the page or the gateway: the console
 * log's frames of the mob (by member and kind), and each member's newest run
 * with the kinds that ended it. What cannot be read is said as missing; the
 * runtime's own record of a member's run is not read here and is said so.
 */
function storeReadback(stateDir, mobs) {
  const logs = filesUnder(stateDir).map((relative) => path.join(stateDir, relative))
    .filter((file) => isSqliteFile(file) && readSqlite(file, (db) => tablesOf(db).includes("console_frames")) === true);
  const out = { console_log_files: logs.map((file) => path.relative(stateDir, file)), mobs: {} };
  for (const mob of mobs) {
    const key = runtimeKeyOf(mob);
    if (logs.length === 0) {
      out.mobs[mob.id] = { runtime_key: key, console_log: { missing: "no file under the state root holds console_frames" }, member_runs: { missing: "no console log" }, runtime_run_state: { missing: "not read: the runtime store's run record has no reader here" } };
      continue;
    }
    const frames = [];
    const errors = [];
    for (const file of logs) {
      const rows = readSqlite(file, (db) => db.prepare(
        "SELECT cursor_seq, id, identity, kind, status, session_id, run_id FROM console_frames WHERE runtime_key = ? ORDER BY cursor_seq",
      ).all(key));
      if (Array.isArray(rows)) frames.push(...rows); else errors.push(rows.missing);
    }
    const byMember = {};
    for (const frame of frames) {
      const member = (byMember[frame.identity] ??= { frames: 0, kinds: {}, last_cursor: null, runs: [] });
      member.frames += 1;
      member.kinds[frame.kind] = (member.kinds[frame.kind] || 0) + 1;
      member.last_cursor = frame.cursor_seq;
      if (frame.kind === "run_started" && frame.run_id) member.runs.push({ run_id: frame.run_id, ended_by: [] });
      else if (frame.run_id && RUN_END_KINDS.includes(frame.kind)) {
        const run = member.runs.find((item) => item.run_id === frame.run_id);
        if (run) run.ended_by.push(frame.kind);
      }
    }
    const memberRuns = {};
    for (const [name, member] of Object.entries(byMember)) {
      const newest = member.runs[member.runs.length - 1] ?? null;
      memberRuns[name] = newest ? { run_id: newest.run_id, state: newest.ended_by.length ? "ended" : "running", ended_by: newest.ended_by } : { state: "no run" };
      delete member.runs;
    }
    out.mobs[mob.id] = {
      runtime_key: key,
      console_log: errors.length ? { missing: errors.join("; "), members: byMember } : { frames: frames.length, members: byMember },
      member_runs: memberRuns,
      runtime_run_state: { missing: "not read: the runtime store's run record has no reader here" },
    };
  }
  return out;
}

/**
 * The rows of the console log under `stateDir` that hold any of `needles`
 * (an attachment's blob id, a run of its bytes as base64), for each
 * runtime key: read from the files, not through the page or the gateway.
 */
function storeHolding(stateDir, needles) {
  const wanted = needles.filter((needle) => typeof needle === "string" && needle.length > 0);
  const logs = filesUnder(stateDir).map((relative) => path.join(stateDir, relative))
    .filter((file) => isSqliteFile(file) && readSqlite(file, (db) => tablesOf(db).includes("console_frames")) === true);
  if (logs.length === 0) return { missing: "no file under the state root holds console_frames" };
  const byKey = {};
  for (const file of logs) {
    const rows = readSqlite(file, (db) => db.prepare("SELECT runtime_key, id, payload_json FROM console_frames").all());
    if (!Array.isArray(rows)) return rows;
    for (const row of rows) {
      if (!wanted.some((needle) => String(row.payload_json).includes(needle))) continue;
      (byKey[row.runtime_key] ??= []).push(row.id);
    }
  }
  return { by_runtime_key: byKey };
}

/**
 * The indexes of the model requests one of whose user messages carries
 * `text` and the image whose bytes are `base64`, exactly.
 */
function modelRequestsCarrying(requests, text, base64) {
  const found = [];
  for (const [index, request] of (Array.isArray(requests) ? requests : []).entries()) {
    for (const message of request?.messages || []) {
      if (message?.role !== "user" || !Array.isArray(message.content)) continue;
      const hasText = message.content.some((block) => block?.type === "text" && typeof block.text === "string" && block.text.includes(text));
      const hasImage = message.content.some((block) => block?.type === "image" && block.data === base64);
      if (hasText && hasImage) { found.push(index); break; }
    }
  }
  return found;
}

/** The blob ids of the image blocks a user input frame holds. */
function imageBlobIds(frame) {
  const content = frame?.payload?.content;
  return Array.isArray(content) ? content.filter((block) => block?.type === "image" && typeof block.blob_id === "string").map((block) => block.blob_id) : [];
}

/** The ids of the frames that hold any of `needles`. */
function framesHolding(frames, needles) {
  const wanted = needles.filter((needle) => typeof needle === "string" && needle.length > 0);
  return frames.filter((frame) => wanted.some((needle) => JSON.stringify(frame).includes(needle))).map((frame) => frame.id);
}

/**
 * Runs of a file's bytes to look for where a store may keep it: three raw
 * runs and three runs of its base64 text, each from a quarter, the middle
 * and three quarters of the file. None is taken from the file's start or
 * end, which two pictures of one size share.
 */
function fileNeedles(bytes) {
  const base64 = bytes.toString("base64");
  const at = (length, run) => [0.25, 0.5, 0.75].map((part) => Math.max(0, Math.min(length - run, Math.floor(length * part))));
  return {
    raw: at(bytes.length, 48).map((start) => Buffer.from(bytes.subarray(start, start + 48))),
    base64: at(base64.length, 64).map((start) => base64.slice(start, start + 64)),
  };
}

/**
 * The runs of `runs` ({@link fileNeedles} of one file) that another file's
 * `bytes` hold too: its raw runs in the bytes, its base64 runs in their
 * base64 text. Two files told apart by their runs share none.
 */
function runsSharedWith(runs, bytes) {
  const base64 = bytes.toString("base64");
  return [
    ...runs.raw.filter((run) => bytes.includes(run)).map((run) => run.toString("base64")),
    ...runs.base64.filter((run) => base64.includes(run)),
  ];
}

/** The files under `dir` (relative, sorted) that hold any of `needles`: runs of bytes, or texts read as bytes. */
function filesHolding(dir, needles) {
  const wanted = needles.map((needle) => (typeof needle === "string" ? Buffer.from(needle, "latin1") : needle))
    .filter((needle) => Buffer.isBuffer(needle) && needle.length > 0);
  const found = [];
  for (const relative of filesUnder(dir)) {
    let bytes;
    try { bytes = fs.readFileSync(path.join(dir, relative)); } catch { continue; }
    if (wanted.some((needle) => bytes.includes(needle))) found.push(relative);
  }
  return found;
}

/**
 * Why a request door's answer is not a typed refusal, or `null` when it is
 * one: a JSON-RPC error with an integer code and a kind, and no result.
 */
function typedRefusalFault(answer) {
  if (!answer || typeof answer !== "object") return "it is no JSON answer";
  if (answer.result !== undefined) return "it carries a result";
  const error = answer.error;
  if (!error || typeof error !== "object") return "it carries no error";
  if (!Number.isInteger(error.code)) return `its code is ${JSON.stringify(error.code)}`;
  const kind = error.data?.kind;
  if (typeof kind !== "string" || kind === "") return "its data names no kind";
  return null;
}

/** A request door's one refusal of a mob id it does not serve to its caller, as the contract has it. */
const MOB_REFUSAL = { status: 200, code: -32602, kind: "foreign_mob_target" };

/**
 * Why a request door's answer (`status`, `answer`) to a request that named
 * the mob `mobId` is not the door's refusal of that id, or `null` when it
 * is: a typed refusal with the status, the code and the kind of
 * {@link MOB_REFUSAL}, the requested id, and no other member of its data.
 * The words of its message are not held to anything.
 */
function mobRefusalFault(status, answer, mobId) {
  const untyped = typedRefusalFault(answer);
  if (untyped) return untyped;
  if (status !== MOB_REFUSAL.status) return `its HTTP status is ${JSON.stringify(status)}`;
  const { code, data } = answer.error;
  if (code !== MOB_REFUSAL.code) return `its code is ${JSON.stringify(code)}`;
  if (data.kind !== MOB_REFUSAL.kind) return `its kind is ${JSON.stringify(data.kind)}`;
  if (data.requested_mob_id !== mobId) return `it names ${JSON.stringify(data.requested_mob_id)} as the requested mob`;
  const more = Object.keys(data).filter((name) => name !== "kind" && name !== "requested_mob_id");
  if (more.length > 0) return `its data also carries ${more.join(", ")}`;
  return null;
}

/**
 * The params of a message with a file for the member `identity` of the mob
 * `mobId`, made as the page made one (`pageParams`, of its own message with
 * a file): the page's other members as they are, and this message's own
 * member, mob, key, words and upload.
 */
function attachmentParamsFor(pageParams, { mobId, identity, text, uploadId, alt, idempotencyKey }) {
  return {
    ...(pageParams || {}), identity, mob_id: mobId, idempotency_key: idempotencyKey,
    content: [{ type: "text", text }, { type: "image_upload", upload_id: uploadId, media_type: "image/png", alt }],
  };
}

/**
 * How many frames each mob's console log holds, from a store readback
 * ({@link storeReadback}); a log that was not read is `{ missing }`, never
 * a number.
 */
function logCounts(readback) {
  return Object.fromEntries(Object.entries(readback?.mobs ?? {}).map(([id, mob]) => [
    id, Number.isInteger(mob?.console_log?.frames) ? mob.console_log.frames : { missing: String(mob?.console_log?.missing ?? "not read") },
  ]));
}

/** Frame kinds the console log writes again from a member's own session history. */
const OWN_HISTORY_SNAPSHOT_KINDS = ["runtime_notice_snapshot", "assistant_history_snapshot"];

/**
 * The frames the peer mob gained while a message went to another mob's
 * member, told apart: an own-history snapshot repeat (snapshot kinds only,
 * of the peer's own mob and of sessions the peer's frames had before, with
 * no byte of the message), a wrong-target disclosure (any frame of the peer
 * that holds the message's text or its answer), or another change.
 */
function classifyPeerFrames({ beforeIds, after, peerMobId, message, knownSessions }) {
  const known = new Set(beforeIds);
  const sessions = new Set(knownSessions);
  const extra = after.filter((frame) => !known.has(frame.id)).map((frame) => ({
    id: frame.id, kind: frame.kind, mob_id: frame.mob_id ?? null, identity: frame.identity ?? null,
    runtime_key: frame.runtime_key ?? null, session_id: frame.session_id ?? null,
  }));
  const needles = [message.text, message.answer].filter((item) => typeof item === "string" && item.length > 0);
  const carrying = after.filter((frame) => needles.some((needle) => JSON.stringify(frame).includes(needle))).map((frame) => frame.id);
  let verdict;
  if (carrying.length > 0) verdict = "wrong-target disclosure";
  else if (extra.length === 0) verdict = "unchanged";
  else if (extra.every((frame) => OWN_HISTORY_SNAPSHOT_KINDS.includes(frame.kind)
    && frame.mob_id === peerMobId && frame.session_id !== null && sessions.has(frame.session_id))) verdict = "own-history snapshot repeat";
  else verdict = "other change";
  return { verdict, extra, message_bytes_found: carrying.length > 0, frames_with_message_bytes: carrying };
}

/**
 * What became of a run the page asked to stop: an actual stop needs the
 * gateway's receipt `stopped` for that run, a frame that ends the run, and no
 * completion of it (no `run_completed` and no frame holding the whole answer
 * the run was writing). A run that completed is a natural completion
 * whatever the receipt said.
 */
function classifyStop({ answer, frames, fullText }) {
  const receipt = answer?.result?.receipt ?? null;
  // The run the receipt names; with no receipt (no stop was sent) the newest run.
  const newest = [...frames].reverse().find((frame) => frame.kind === "run_started" && typeof frame.run_id === "string");
  const runId = typeof receipt?.run_id === "string" ? receipt.run_id : newest?.run_id ?? null;
  const ofRun = runId ? frames.filter((frame) => frame.run_id === runId || JSON.stringify(frame.payload ?? null).includes(runId)) : [];
  const ends = ofRun.filter((frame) => RUN_END_KINDS.includes(frame.kind)).map((frame) => frame.kind);
  const completed = ends.includes("run_completed")
    || frames.some((frame) => frame.kind === "text_complete" && JSON.stringify(frame.payload ?? null).includes(fullText));
  let verdict;
  if (completed) verdict = "natural completion";
  else if (receipt?.outcome === "stopped" && ends.length > 0) verdict = "stopped";
  else verdict = "undetermined";
  return { verdict, accepted: receipt?.outcome === "stopped", outcome: receipt?.outcome ?? null, run_id: runId, ended_by: ends, completed };
}

// ---------------------------------------------------------------------------
// The scenario
// ---------------------------------------------------------------------------

async function runRealHostedMobs() {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Set META_EXAMPLE_BIN_DIR to the prebuilt native backend examples; this scenario never starts Cargo.");
  // Loaded here: the helpers above are used without a browser too.
  const { chromium } = require("playwright");
  const evidenceDir = path.join(process.env.META_BROWSER_EVIDENCE || path.join(consoleDir, "../output/playwright/console-acceptance"), "real-hosted-mobs");
  fs.mkdirSync(evidenceDir, { recursive: true });
  const stateRoot = fs.mkdtempSync(path.join(os.tmpdir(), "meta-console-two-mobs-"));
  const privateHome = path.join(stateRoot, "home");
  for (const dir of ["config", "state", "data", "cache", "tmp"]) fs.mkdirSync(path.join(privateHome, dir), { recursive: true });
  const stateDir = path.join(stateRoot, "gateway-state");
  fs.mkdirSync(stateDir);

  const book = checkBook(PLAN, NOT_COVERED);
  const offers = {};
  /** Evidence read beside the page, written into result.json whatever the rows say. */
  const evidence = { bundle: null, fixture_identity: {}, readbacks: [], peer_frames: [], stops: [], attachments: [], recovery: [] };
  let backendUrl = null;
  let child, proxy, browser, page, logs = "";
  const pageErrors = [];
  try {
    // -- the gateway, its two mobs and their members -------------------------
    const env = childEnvironment(process.env, privateHome);
    await book.run("fixture:no-credential", async () => {
      const names = Object.keys(env).sort();
      assert.deepEqual(names.filter((name) => /KEY|TOKEN|SECRET|CREDENTIAL/i.test(name)), [], "no variable that names a key or a token");
      assert.equal(env.HOME, privateHome, "the gateway's home is its own");
      return { variables: names };
    });
    const spec = exampleBackendSpec(repoRoot, "mcp_apps_gateway_fixture");
    child = spawn(spec.command, spec.args, { cwd: repoRoot, stdio: ["pipe", "pipe", "pipe"], env });
    const gateway = gatewayStdio(child);
    for (const output of [child.stdout, child.stderr]) output.on("data", (chunk) => { logs = (logs + chunk).slice(-60_000); });
    const init = await gateway.request("meta/init", {
      mob_config: mobToml(LAUNCH_MOB),
      persistent_state: stateDir,
      runtime_options: { console_require_app_auth: false },
    }, 120_000);
    assert.equal(typeof init.http_base_url, "string", `meta/init answers the gateway's address: ${JSON.stringify(init)}`);
    backendUrl = init.http_base_url;

    /** The params that address `mob` on a door that takes `mob_id`: none for the launch mob. */
    const doorOf = (mob) => (mob.default ? {} : { mob_id: mob.id });
    const consoleRpc = async (method, params) => {
      const response = await fetch(`${backendUrl}/console/rpc`, {
        method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
      });
      assert.equal(response.status, 200, `${method}: HTTP ${response.status}`);
      return response.json();
    };
    /** The scripted model's next answers. */
    const model = async (plan) => {
      const response = await fetch(`${backendUrl}/host/__fixture/model`, {
        method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(plan),
      });
      assert.equal(response.status, 200, `the scripted model takes its plan: HTTP ${response.status}`);
    };
    const modelRequests = async () => {
      const response = await fetch(`${backendUrl}/host/__fixture/requests`);
      assert.equal(response.status, 200);
      return response.json();
    };
    /** The frames the gateway keeps for the member `name` of `mob`, read on that mob's door. */
    const history = async (mob, name) => {
      const answer = await consoleRpc("meta/console/query_timeline", { identity: name, mode: "recent", limit: 1000, ...doorOf(mob) });
      assert(!answer.error, `the history of ${mob.id} ${name}: ${JSON.stringify(answer.error)}`);
      assert(Array.isArray(answer.result?.frames), `the history of ${mob.id} ${name} is a list of frames`);
      return answer.result.frames;
    };
    const framesWith = (frames, text, kind) => frames
      .filter((frame) => (!kind || frame.kind === kind) && JSON.stringify(frame.payload ?? null).includes(text));
    const memberNameOf = (row) => row.member_id ?? row.meerkat_id ?? row.agent_identity ?? row.identity ?? null;
    const roster = async (mob) => {
      const answer = await gateway.request("meta/list_members", doorOf(mob));
      const rows = Array.isArray(answer) ? answer : answer?.members;
      assert(Array.isArray(rows), `the members of ${mob.id}: ${JSON.stringify(answer)}`);
      return rows.map((row) => [memberNameOf(row), typeof row.status === "string" ? row.status : null]).sort();
    };
    /** What the gateway holds of `mob`: its members, and the ids of each member's frames. */
    const holdings = async (mob, members = mob.members) => {
      const frames = {};
      for (const name of members) frames[name] = (await history(mob, name)).map((frame) => frame.id).sort();
      return { members: await roster(mob), frames };
    };
    /** `holdings` once two reads in a row agree: nothing of an earlier turn is still landing. */
    const settled = async (mob, members) => {
      let last = await holdings(mob, members);
      const deadline = Date.now() + 30_000;
      for (;;) {
        await delay(500);
        const now = await holdings(mob, members);
        if (JSON.stringify(now) === JSON.stringify(last)) return now;
        assert(Date.now() < deadline, `${mob.id} does not come to rest`);
        last = now;
      }
    };
    const [launch, second] = MOBS;
    /**
     * Both mobs as their stores hold them, read from the files and not
     * through the page, with each mob's members and status as the gateway's
     * stdio door lists them; recorded for `action` `at` before or after.
     */
    const readback = async (action, at) => {
      const entry = { action, at, at_ms: Date.now(), stores: storeReadback(stateRoot, MOBS), gateway_stdio_members: {} };
      for (const mob of MOBS) {
        try { entry.gateway_stdio_members[mob.id] = await roster(mob); }
        catch (error) { entry.gateway_stdio_members[mob.id] = { missing: String((error && error.message) || error) }; }
      }
      evidence.readbacks.push(entry);
    };

    await book.run("fixture:two-mobs", async () => {
      const capabilities = await gateway.request("meta/capabilities", {});
      assert(Array.isArray(capabilities.feature_capabilities) && capabilities.feature_capabilities.includes("hosted_mobs"),
        `the gateway hosts mobs: ${JSON.stringify(capabilities.feature_capabilities)}`);
      for (const name of launch.members) {
        await gateway.request("meta/spawn_member", { profile: "agent", meerkat_id: name });
      }
      const created = await gateway.request("meta/mobs/create", { mob_config: mobToml(SECOND_MOB) });
      assert.deepEqual(
        { mob_id: created.mob?.mob_id, default: created.mob?.default, provenance: created.mob?.provenance },
        { mob_id: SECOND_MOB, default: false, provenance: "host" },
        "the second mob is a mob the host created",
      );
      for (const name of second.members) {
        await gateway.request("meta/spawn_member", { mob_id: SECOND_MOB, profile: "agent", meerkat_id: name });
      }
      for (const mob of MOBS) {
        await eventually(async () => {
          const ready = await gateway.request("meta/wait_ready", { ...doorOf(mob), timeout_ms: 1000 });
          return ready?.timeout === false && ready.ready?.length === mob.members.length;
        }, `the members of ${mob.id} finish their start`, 60_000);
      }
      const listed = await gateway.request("meta/mobs/list", {});
      assert.deepEqual(listed.mobs.map((mob) => [mob.mob_id, mob.default]), [[LAUNCH_MOB, true], [SECOND_MOB, false]]);
      const names = {};
      for (const mob of MOBS) names[mob.id] = (await roster(mob)).map(([name]) => name);
      assert.deepEqual(names, { [LAUNCH_MOB]: [...launch.members].sort(), [SECOND_MOB]: [...second.members].sort() },
        "each mob has its two members, the shared name in both");
      return names;
    });

    // An earlier turn of each namesake, made beside the page: what its chat
    // shows on its first load is then that member's own history.
    const seed = {};
    for (const mob of MOBS) {
      const text = `Earlier note for ${mob.id} ${randomUUID()}`;
      const answer = `Earlier answer ${randomUUID()}.`;
      await model({ source: answer, delay_ms: 0, chunk_chars: 1024 });
      await gateway.request("meta/send_message", { ...doorOf(mob), member_id: SHARED, message: text });
      await eventually(async () => framesWith(await history(mob, SHARED), answer).length > 0, `${mob.id} ${SHARED} answers its earlier turn`, 60_000);
      seed[mob.id] = { text, answer };
    }
    evidence.fixture_identity.after_setup = fixtureIdentity(stateRoot, stateRoot, MOBS);

    // -- the page -----------------------------------------------------------
    proxy = await startRecordingProxy({ assetsDir: path.join(consoleDir, "dist"), upstream: () => backendUrl });
    // A part of its own: bytes that differ fail the row and the actions still run.
    await book.section("bundle", () => book.run("fixture:bundle", async () => {
      /** The asset bytes at `base`, by asset name; an answer that is no 200 is `null`. */
      const fetchAssets = async (base) => {
        const out = {};
        for (const [name, route] of BUNDLE_ASSETS) {
          const response = await fetch(`${base}${route}`);
          out[name] = response.status === 200 ? Buffer.from(await response.arrayBuffer()) : null;
        }
        return out;
      };
      const dist = Object.fromEntries(BUNDLE_ASSETS.map(([name]) => {
        const file = path.join(consoleDir, "dist", name);
        return [name, fs.existsSync(file) ? fs.readFileSync(file) : null];
      }));
      evidence.bundle = bundleEvidence({ gateway: await fetchAssets(backendUrl), proxy: await fetchAssets(proxy.baseUrl), dist });
      assert(evidence.bundle.equal, `the gateway serves the bytes the page is served and the tree built: ${JSON.stringify(evidence.bundle)}`);
      return evidence.bundle;
    }));
    browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({ viewport: { width: 1500, height: 1000 } });
    page = await context.newPage();
    page.setDefaultTimeout(20_000);
    page.on("pageerror", (error) => pageErrors.push(error.message));
    const shot = (name) => page.screenshot({ path: path.join(evidenceDir, `${name}.png`) });
    await page.goto(`${proxy.baseUrl}/console`, { waitUntil: "domcontentloaded", timeout: 30_000 });

    const readRoster = async () => groupRoster(await page.evaluate(() =>
      [...document.querySelectorAll('[data-testid^="sidebar-mob:"], [data-testid^="sidebar-agent:"]')]
        .map((node) => ({ id: node.getAttribute("data-testid"), text: node.innerText }))));
    let laidOut;
    await book.run("display:mobs", async () => {
      laidOut = await eventually(async () => {
        const mobs = await readRoster();
        return MOBS.every((mob) => mobs.some((item) => item.mobId === mob.id && item.agents.length === mob.members.length)) ? mobs : null;
      }, "the page lists both mobs with their members", 30_000);
      assert.deepEqual(laidOut.map((mob) => mob.mobId).sort(), MOBS.map((mob) => mob.id).sort(), "the mobs of the gateway and no other");
      return laidOut.map((mob) => mob.mobId);
    });
    const rows = {};
    await book.run("display:members", async () => {
      for (const mob of MOBS) {
        rows[mob.id] = {};
        for (const name of mob.members) rows[mob.id][name] = rowOf(laidOut, mob.id, name);
      }
      assert.notEqual(rows[LAUNCH_MOB][SHARED].key, rows[SECOND_MOB][SHARED].key, "the two members of one name have two keys");
      await shot("1-roster");
      return Object.fromEntries(MOBS.map((mob) => [mob.id, mob.members.map((name) => rows[mob.id][name].key)]));
    });

    /** The chat of `mob`'s member of the shared name, opened and in front. */
    const openChat = async (mob) => {
      const key = rows[mob.id][SHARED].key;
      await page.getByTestId(`sidebar-agent:${key}`).click();
      const pane = page.getByTestId(`chat-pane:${key}`);
      await pane.waitFor({ timeout: 15_000 });
      return { key, pane, composer: page.getByTestId(`chat-composer:${key}`) };
    };
    const paneText = async (chat) => chat.pane.innerText();
    const namesShared = (call, action, mob) => {
      const named = namedMember(call.params, action.member, LAUNCH_MOB);
      assert.deepEqual({ mob: named.mob, name: named.name }, { mob: mob.id, name: SHARED },
        `the request names the member of ${mob.id}: ${JSON.stringify(call.params)}`);
      return named.form;
    };
    /** Whether the page offers `action` for the second mob's member, held against what it has to offer. */
    const offer = async (action, offered, how) => {
      offers[action] = { offered, how };
      await book.run(`${action}:offer`, async () => {
        const fault = offerFault(action, offered);
        assert.equal(fault, null, `${fault}: ${how}`);
        return offers[action];
      });
      return offered;
    };
    /**
     * An action the page does not offer for the second mob's member: nothing
     * carries it for either namesake, and neither mob changes, also after the
     * person went to the other mob's member and came back.
     */
    const unavailable = async (action, rowsOfAction, attempt) => {
      for (const id of rowsOfAction) book.notApplicable(id, "the page does not offer the action");
      await book.run(`${action}:unavailable`, async () => {
        const before = { [LAUNCH_MOB]: await settled(launch), [SECOND_MOB]: await settled(second) };
        const mark = proxy.mark();
        if (attempt) await attempt();
        await delay(1_000);
        await openChat(launch);
        await openChat(second);
        if (attempt) await attempt();
        await delay(1_000);
        const accepted = callsOf(ACTIONS[action], proxy.since(mark)).filter((call) => call.answered && !call.refused);
        assert.deepEqual(accepted.map((call) => call.params), [], "the gateway accepted no request of the action");
        assert.deepEqual({ [LAUNCH_MOB]: await settled(launch), [SECOND_MOB]: await settled(second) }, before, "neither mob changed");
      });
    };

    // -- history ------------------------------------------------------------
    let secondChat = await openChat(second);
    await book.run("history:second", async () => {
      await eventually(async () => (await paneText(secondChat)).includes(seed[SECOND_MOB].answer), "the second mob's member's earlier answer is in its chat");
      const text = await paneText(secondChat);
      assert(!text.includes(seed[LAUNCH_MOB].text) && !text.includes(seed[LAUNCH_MOB].answer), "nothing of its namesake's turn is in it");
      await shot("2-second-history");
    });
    let launchChat = await openChat(launch);
    await book.run("history:launch", async () => {
      await eventually(async () => (await paneText(launchChat)).includes(seed[LAUNCH_MOB].answer), "the launch mob's member's earlier answer is in its chat");
      const text = await paneText(launchChat);
      assert(!text.includes(seed[SECOND_MOB].text) && !text.includes(seed[SECOND_MOB].answer), "nothing of its namesake's turn is in it");
      await shot("3-launch-history");
    });

    // -- a message ----------------------------------------------------------
    /** A picture of its own: a PNG of two colours, drawn by the browser. */
    const picture = async (from, to) => {
      const sheet = await browser.newPage({ viewport: { width: 64, height: 48 }, deviceScaleFactor: 1 });
      try {
        await sheet.setContent(`<body style="margin:0"><div style="width:64px;height:48px;background:linear-gradient(90deg,${from},${to})"></div></body>`);
        return await sheet.screenshot();
      } finally { await sheet.close(); }
    };
    /** Drop `file` ({ name, bytes }) on the composer of `chat`, as a person drops a picture on it. */
    const dropOn = (chat, file) => chat.composer.evaluate((node, dropped) => {
      const transfer = new DataTransfer();
      transfer.items.add(new File([Uint8Array.from(dropped.data)], dropped.name, { type: "image/png" }));
      node.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: transfer }));
    }, { name: file.name, data: Array.from(file.bytes) });
    /** The page's own message with a file, as it was sent and where its upload was found: what a message with a file for another mob is made from and held against. */
    let launchUpload = null;
    /**
     * One message written in `chat`, with what has to hold of it for `mob`
     * and of the other mob `peer`. With `attachment` ({ name, bytes }) the
     * file is dropped on the composer first and goes out with the message.
     */
    const sendAndWitness = async (prefix, chat, mob, peer, attachment = null) => {
      const text = `Message for ${mob.id} ${randomUUID()}`;
      const answer = `Answer ${randomUUID()}.`;
      await model({ source: answer, delay_ms: 0, chunk_chars: 1024 });
      const before = await settled(peer);
      const peerFramesBefore = {};
      for (const name of peer.members) peerFramesBefore[name] = await history(peer, name);
      const logBefore = logCounts(storeReadback(stateRoot, MOBS));
      await readback(prefix, "before");
      const mark = proxy.mark();
      if (attachment) {
        await dropOn(chat, attachment);
        await chat.pane.locator(".composer__attachment img").waitFor({ timeout: 15_000 });
      }
      await chat.composer.fill(text);
      await chat.composer.press("Enter");
      const sent = () => callsOf(ACTIONS.send, proxy.since(mark)).filter((call) => call.text.includes(text));
      let form;
      try {
        await book.run(`${prefix}:target`, async () => {
          await eventually(() => sent().some((call) => call.answered), "the message is sent and answered", 20_000);
          const calls = sent();
          assert.equal(calls.length, 1, "the page sends the message once");
          form = namesShared(calls[0], ACTIONS.send, mob);
          assert(!calls[0].refused, `the gateway accepts it: ${JSON.stringify(calls[0].answer)}`);
          return { door: calls[0].door, form };
        });
        if (attachment) {
          await book.run(`${prefix}:attachment-request`, async () => {
            const [call] = sent();
            assert(call, "the message was sent");
            assert.equal(call.params.mob_id, mob.default ? undefined : mob.id, `the request names the mob as its door is named: ${JSON.stringify(call.params)}`);
            assert.equal(call.params.identity, SHARED, "the request names the member by its bare name");
            const uploads = (call.params.content || []).filter((block) => block?.type === "image_upload");
            assert.equal(uploads.length, 1, `the request's content names one upload: ${JSON.stringify(call.params.content)}`);
            assert.deepEqual(call.files.map((file) => [file.part, file.filename]), [[`file:${uploads[0].upload_id}`, attachment.name]],
              "the request carries the file as that upload");
            launchUpload = { params: call.params, bytes: attachment.bytes, stored: null };
            return { door: call.door, upload: uploads[0], files: call.files };
          });
        }
        await book.run(`${prefix}:effect`, async () => {
          await eventually(async () => framesWith(await history(mob, SHARED), answer).length > 0, "the member answers the message", 60_000);
          assert.equal(framesWith(await history(mob, SHARED), text, "user_input").length, 1, "its history holds the message once");
          assert((await modelRequests()).some((request) => JSON.stringify(request).includes(text)), "the message reached a model turn");
          await eventually(async () => (await paneText(chat)).includes(answer), "the answer is in the member's chat");
        });
        // The same, read from the log's file and not through the gateway.
        await book.run(`${prefix}:log`, async () => {
          const held = await eventually(async () => {
            const found = storeHolding(stateRoot, [text]);
            return !found.missing && (found.by_runtime_key[runtimeKeyOf(mob)] ?? []).length > 0 ? found : null;
          }, `the log's file holds the message under ${mob.id}`, 20_000);
          assert.deepEqual(held.by_runtime_key[runtimeKeyOf(peer)] ?? [], [], `the log's file holds no row of the message under ${peer.id}`);
          const logAfter = logCounts(storeReadback(stateRoot, MOBS));
          for (const id of [mob.id, peer.id]) {
            assert(Number.isInteger(logBefore[id]) && Number.isInteger(logAfter[id]), `the log of ${id} is counted before and after: ${JSON.stringify({ logBefore, logAfter })}`);
          }
          assert.equal(logAfter[peer.id], logBefore[peer.id], `the log of ${peer.id} has as many frames as before`);
          assert(logAfter[mob.id] > logBefore[mob.id], `the log of ${mob.id} gained the message's frames: ${JSON.stringify({ logBefore, logAfter })}`);
          return { before: logBefore, after: logAfter, rows: held.by_runtime_key };
        });
        if (attachment) {
          const base64 = attachment.bytes.toString("base64");
          const input = framesWith(await history(mob, SHARED), text, "user_input")[0];
          const blobIds = imageBlobIds(input);
          const runs = fileNeedles(attachment.bytes);
          // Needles for "no byte of it": the blob id, and runs of the bytes as base64.
          const needles = [...blobIds, ...runs.base64];
          await book.run(`${prefix}:attachment-model`, async () => {
            const carrying = modelRequestsCarrying(await modelRequests(), text, base64);
            assert(carrying.length >= 1, "a model request carries the message with the exact bytes of the file");
            assert.equal(blobIds.length, 1, `the member's input holds the file as one blob: ${JSON.stringify(input?.payload)}`);
            return { model_request: carrying[0], blob_id: blobIds[0] };
          });
          await book.run(`${prefix}:attachment-stored`, async () => {
            const found = filesHolding(stateRoot, [...runs.raw, ...runs.base64]);
            evidence.attachments.push({ action: prefix, at: "stored", files: found });
            if (launchUpload) launchUpload.stored = found;
            assert(found.length > 0, "a file under the state root holds a run of the upload's bytes");
            return found;
          });
          await book.run(`${prefix}:attachment-other-mob`, async () => {
            const held = {};
            for (const name of peer.members) held[name] = framesHolding(await history(peer, name), needles);
            const stored = storeHolding(stateRoot, needles);
            evidence.attachments.push({ action: prefix, peer: peer.id, gateway_reads: held, store: stored, blob_ids: blobIds });
            assert.deepEqual(Object.values(held).flat(), [], `${peer.id} holds no byte of the file: ${JSON.stringify(held)}`);
            assert(!stored.missing, `the console log is read from its file: ${stored.missing}`);
            assert.deepEqual(stored.by_runtime_key[runtimeKeyOf(peer)] ?? [], [], `the store holds no byte of the file under ${peer.id}: ${JSON.stringify(stored)}`);
            return { gateway_reads: held, store: stored };
          });
        }
        await book.run(`${prefix}:other-mob`, async () => {
          // What the other mob gained is told apart before any check: an own
          // history snapshot repeat is not a disclosure of the message.
          const told = {};
          for (const name of peer.members) {
            const frames = await history(peer, name);
            told[name] = classifyPeerFrames({
              beforeIds: peerFramesBefore[name].map((frame) => frame.id), after: frames, peerMobId: peer.id,
              message: { text, answer }, knownSessions: peerFramesBefore[name].map((frame) => frame.session_id).filter(Boolean),
            });
          }
          evidence.peer_frames.push({ action: prefix, peer: peer.id, members: told });
          for (const name of peer.members) {
            const frames = await history(peer, name);
            assert.equal(framesWith(frames, text).length + framesWith(frames, answer).length, 0, `${peer.id} ${name} holds nothing of the message: ${JSON.stringify(told[name])}`);
          }
          for (const name of mob.members.filter((item) => item !== SHARED)) {
            assert.equal(framesWith(await history(mob, name), text).length, 0, `${mob.id} ${name} holds nothing of the message`);
          }
          assert.deepEqual(await settled(peer), before, `${peer.id} is unchanged; what it gained: ${JSON.stringify(told)}`);
          const other = await openChat(peer);
          const shown = await paneText(other);
          assert(!shown.includes(text) && !shown.includes(answer), "the namesake's chat shows nothing of it");
          return told;
        });
      } finally {
        await readback(prefix, "after");
      }
      return { text, answer, form };
    };

    // Each action below is a part of its own: a failed row ends its action's
    // part, and the next action still runs on the same gateway and page.
    let sendOffered = false;
    await book.section("send", async () => {
      secondChat = await openChat(second);
      sendOffered = await offer("send",
        (await secondChat.composer.count()) === 1 && await secondChat.composer.isEditable(),
        "the chat's message field is there and can be written in");
      if (sendOffered) {
        book.notApplicable("send:unavailable", "the page offers the action");
        try {
          // Words alone: a message with a file is taken for the launch mob only (the attach part below).
          offers.send.sent = await sendAndWitness("send", secondChat, second, launch);
        } finally {
          await shot("4-second-message");
        }
      } else {
        await unavailable("send", ["send:target", "send:effect", "send:log", "send:other-mob"], null);
      }
    });
    // The launch mob's own message goes out with a file, as it always could:
    // the control of the attach part, whose request is made as this one was.
    await book.section("send-launch", async () => {
      launchChat = await openChat(launch);
      try {
        await sendAndWitness("send-launch", launchChat, launch, second, { name: "two-mobs-launch.png", bytes: await picture("#c33", "#36c") });
      } finally {
        await shot("5-launch-message");
      }
    });

    // -- a file on a message for the second mob's member ----------------------
    // The request door that takes a message with a file serves the launch
    // mob alone. On the page: an image dropped on the second mob's member's
    // composer is not staged and nothing is asked. At the door, beside the
    // page: the request the page would make is refused, and nothing is
    // written for either mob.
    const refusedFile = { name: "two-mobs-second.png", bytes: null };
    await book.section("attach-page", async () => {
      refusedFile.bytes = await picture("#2a8", "#a82");
      secondChat = await openChat(second);
      const before = { [LAUNCH_MOB]: await settled(launch), [SECOND_MOB]: await settled(second) };
      const mark = proxy.mark();
      const stagedImage = secondChat.pane.locator(".composer__attachment img");
      await dropOn(secondChat, refusedFile);
      const staged = await stagedImage.first().waitFor({ timeout: 3_000 }).then(() => true, () => false);
      // An image the page did stage is taken off again: it is not sent by a later part.
      if (staged) await secondChat.pane.getByRole("button", { name: "Remove attachment" }).first().click().catch(() => {});
      await offer("attach", staged, "an image dropped on the member's composer is staged for its message");
      await book.run("attach:unavailable", async () => {
        const said = await paneText(secondChat);
        assert(/images? (cannot be attached|not available)/i.test(said), `the composer says that no image is taken for this member: ${JSON.stringify(said.slice(-400))}`);
        await delay(1_000);
        await openChat(launch);
        secondChat = await openChat(second);
        await dropOn(secondChat, refusedFile);
        await delay(1_000);
        assert.equal(await secondChat.pane.locator(".composer__attachment img").count(), 0, "nothing is staged after switching mobs either");
        const carried = callsOf(ACTIONS.attach, proxy.since(mark)).filter((call) => call.files.length > 0);
        assert.deepEqual(carried.map((call) => call.params), [], "the page made no request with a file");
        assert.deepEqual({ [LAUNCH_MOB]: await settled(launch), [SECOND_MOB]: await settled(second) }, before, "neither mob changed");
        await shot("5b-second-no-image");
        return { said: said.slice(-200) };
      });
    });
    await book.section("attach-door", async () => {
      if (!refusedFile.bytes) refusedFile.bytes = await picture("#2a8", "#a82");
      const runs = fileNeedles(refusedFile.bytes);
      const needles = [...runs.raw, ...runs.base64];
      const text = `File for ${SECOND_MOB} ${randomUUID()}`;
      const uploadId = `upload-beside-the-page-${randomUUID()}`;
      const before = { [LAUNCH_MOB]: await settled(launch), [SECOND_MOB]: await settled(second) };
      const logBefore = logCounts(storeReadback(stateRoot, MOBS));
      const heldBefore = filesHolding(stateRoot, needles);
      await readback("attach", "before");
      const refused = () => book.run("attach:refused", async () => {
        assert(launchUpload, "the page's own message with a file was recorded: the request is made as the page made that one");
        assert.deepEqual(runsSharedWith(runs, launchUpload.bytes), [], "the two pictures share none of the runs looked for");
        assert.deepEqual(heldBefore, [], "no file under the state root holds a run of the file before the request");
        const params = attachmentParamsFor(launchUpload.params, {
          mobId: SECOND_MOB, identity: SHARED, text, uploadId, alt: refusedFile.name, idempotencyKey: `beside-the-page-${randomUUID()}`,
        });
        // A request with uploads that names `member_mob_id` is refused for its params before its mob is judged: this one names none.
        assert(!("member_mob_id" in params), `the request names its member's mob by mob_id alone: ${JSON.stringify(Object.keys(params))}`);
        const form = new FormData();
        form.append(`file:${uploadId}`, new Blob([refusedFile.bytes], { type: "image/png" }), refusedFile.name);
        form.append("payload", JSON.stringify({ jsonrpc: "2.0", id: `beside-the-page:${uploadId}`, method: ACTIONS.attach.methods[0], params }));
        const response = await fetch(`${backendUrl}/console/rpc/multipart`, { method: "POST", body: form });
        const answer = await response.json().catch(() => null);
        evidence.attachments.push({ action: "attach", at: "refused", status: response.status, answer, params });
        const fault = mobRefusalFault(response.status, answer, SECOND_MOB);
        assert.equal(fault, null, `the door answers its refusal of a mob id it does not serve: ${fault}: HTTP ${response.status} ${JSON.stringify(answer)}`);
        return { status: response.status, error: answer.error };
      });
      const nothingWritten = () => book.run("attach:other-mob", async () => {
        await delay(1_000);
        const texts = [text, ...runs.base64];
        const rows = storeHolding(stateRoot, texts);
        const logAfter = logCounts(storeReadback(stateRoot, MOBS));
        const reads = {};
        for (const mob of MOBS) for (const name of mob.members) reads[`${mob.id} ${name}`] = framesHolding(await history(mob, name), texts);
        const files = filesHolding(stateRoot, needles);
        evidence.attachments.push({ action: "attach", at: "after", log_rows: rows, log_counts: { before: logBefore, after: logAfter }, gateway_reads: reads, files, control_files: launchUpload?.stored ?? null });
        assert(!rows.missing, `the console log is read from its file: ${rows.missing}`);
        assert.deepEqual(rows.by_runtime_key, {}, "no row of either mob's log holds the message or the file");
        for (const mob of MOBS) assert(Number.isInteger(logBefore[mob.id]), `the log of ${mob.id} is counted: ${JSON.stringify(logBefore)}`);
        assert.deepEqual(logAfter, logBefore, "each mob's log has as many frames as before");
        assert.deepEqual(Object.values(reads).flat(), [], `the gateway's reads hold nothing of it: ${JSON.stringify(reads)}`);
        assert.deepEqual({ [LAUNCH_MOB]: await settled(launch), [SECOND_MOB]: await settled(second) }, before, "neither mob changed");
        // The search is shown to find an upload: it found the launch mob's own.
        assert((launchUpload?.stored ?? []).length > 0, "the search found the launch mob's own upload under the state root, so that it finds none here says something");
        assert.deepEqual(files, [], "no file under the state root holds a run of the file's bytes");
        return { log_counts: logAfter, files };
      });
      try {
        // The second row runs whatever the first found: what was written is read either way.
        try { await refused(); } finally { await nothingWritten(); }
      } finally {
        await readback("attach", "after");
      }
    });

    // -- details ------------------------------------------------------------
    await book.section("inspect", async () => {
      secondChat = await openChat(second);
      const details = secondChat.pane.getByTestId("conv-action:details");
      const inspectOffered = await offer("inspect", (await details.count()) === 1 && await details.isEnabled(), "the chat's details action is there and enabled");
      if (inspectOffered) {
        book.notApplicable("inspect:unavailable", "the page offers the action");
        const before = { [LAUNCH_MOB]: await settled(launch), [SECOND_MOB]: await settled(second) };
        await readback("inspect", "before");
        const mark = proxy.mark();
        await details.click();
        try {
          await book.run("inspect:target", async () => {
            const read = await eventually(() => callsOf(ACTIONS.inspect, proxy.since(mark))
              .find((call) => call.answered && namedMember(call.params, ACTIONS.inspect.member, LAUNCH_MOB).mob === SECOND_MOB), "the details are read for the second mob's member", 20_000);
            const form = namesShared(read, ACTIONS.inspect, second);
            assert(!read.refused, `the gateway answers the read: ${JSON.stringify(read.answer)}`);
            return { door: read.door, form };
          });
          await book.run("inspect:other-mob", async () => {
            assert.deepEqual({ [LAUNCH_MOB]: await settled(launch), [SECOND_MOB]: await settled(second) }, before, "a read changes neither mob");
          });
        } finally {
          await shot("6-second-details");
          await readback("inspect", "after");
        }
      } else {
        await unavailable("inspect", ["inspect:target", "inspect:other-mob"], null);
      }
    });

    // -- a stop -------------------------------------------------------------
    // Both namesakes run at once, each on a long answer the harness starts
    // beside the page. The page stops the second mob's.
    await book.section("stop", async () => {
      // About two minutes at one character per 300 ms: a stop clicked once
      // the control shows lands well before the run could end by itself.
      const long = `Long answer ${randomUUID()} ${"that runs on and on ".repeat(18)}to its end.`;
      const prompts = Object.fromEntries(MOBS.map((mob) => [mob.id, `Run for ${mob.id} ${randomUUID()}`]));
      const bystanders = { [LAUNCH_MOB]: ["planner"], [SECOND_MOB]: ["triage"] };
      const before = { [LAUNCH_MOB]: await settled(launch, bystanders[LAUNCH_MOB]), [SECOND_MOB]: await settled(second, bystanders[SECOND_MOB]) };
      await model({ source: long, delay_ms: 300, chunk_chars: 1 });
      for (const mob of MOBS) {
        await gateway.request("meta/send_message", { ...doorOf(mob), member_id: SHARED, message: prompts[mob.id] });
        await eventually(async () => framesWith(await history(mob, SHARED), prompts[mob.id]).length > 0, `${mob.id} ${SHARED} starts its run`, 30_000);
      }
      secondChat = await openChat(second);
      const stopControl = secondChat.pane.getByTestId("conv-action:stop-run");
      const appeared = await stopControl.waitFor({ timeout: 15_000 }).then(() => true, () => false);
      // A stop the page has to offer and does not fails its offer row; what
      // the two runs then do is still read (the unavailable row), so the
      // next action does not meet a run still going.
      let offerError = null;
      const stopOffered = await offer("stop", appeared && await stopControl.isEnabled(), "the chat's stop action appears while the member runs")
        .catch((error) => { offerError = error; return false; });
      if (!stopOffered) await shot("7-second-no-stop");
      const launchRunsOn = async () => {
        await eventually(async () => framesWith(await history(launch, SHARED), long).length > 0, "the launch mob's member's run reaches its end", 300_000);
        evidence.stops.push({ mob: LAUNCH_MOB, member: SHARED, stop_sent: false, ...classifyStop({ answer: null, frames: await history(launch, SHARED), fullText: long }) });
        for (const mob of MOBS) {
          assert.deepEqual(await settled(mob, bystanders[mob.id]), before[mob.id], `the other member of ${mob.id} is unchanged`);
        }
      };
      await readback("stop", "before");
      try {
        if (stopOffered) {
          book.notApplicable("stop:unavailable", "the page offers the action");
          const mark = proxy.mark();
          await stopControl.click();
          await book.run("stop:target", async () => {
            const stop = await eventually(() => callsOf(ACTIONS.stop, proxy.since(mark)).find((call) => call.answered), "the stop is sent and answered", 20_000);
            assert.equal(callsOf(ACTIONS.stop, proxy.since(mark)).length, 1, "the page sends one stop");
            const form = namesShared(stop, ACTIONS.stop, second);
            assert(!stop.refused, `the gateway answers the stop: ${JSON.stringify(stop.answer)}`);
            return { door: stop.door, form, answer: stop.answer };
          });
          // What the other mob's namesake does is read even when the page
          // shows no stop: the effect and the other mob are two rows.
          let effectError = null;
          await book.run("stop:effect", async () => {
            try {
              // An actual stop, told from a run that ended by itself: the
              // receipt, a frame that ends the run, and no completion of it.
              const stopCall = callsOf(ACTIONS.stop, proxy.since(mark)).find((call) => call.answered);
              let told = classifyStop({ answer: stopCall?.answer, frames: await history(second, SHARED), fullText: long });
              await eventually(async () => {
                told = classifyStop({ answer: stopCall?.answer, frames: await history(second, SHARED), fullText: long });
                return told.verdict !== "undetermined";
              }, "the run the page stopped ends", 30_000).catch(() => {});
              evidence.stops.push({ mob: SECOND_MOB, member: SHARED, stop_sent: true, ...told });
              assert.equal(told.verdict, "stopped", `the run stopped and did not complete: ${JSON.stringify(told)}`);
              await secondChat.pane.getByTestId(`run-stop-notice:${secondChat.key}`).waitFor({ timeout: 20_000 });
            } finally {
              await shot("7-second-stopped");
            }
          }).catch((error) => { effectError = error; });
          await book.run("stop:other-mob", async () => {
            await launchRunsOn();
            assert.equal(framesWith(await history(second, SHARED), long).length, 0, "the stopped run did not reach its end");
          });
          if (effectError) throw effectError;
        } else {
          for (const id of ["stop:target", "stop:effect", "stop:other-mob"]) book.notApplicable(id, "the page does not offer the action");
          await book.run("stop:unavailable", async () => {
            const mark = proxy.mark();
            await launchRunsOn();
            await eventually(async () => framesWith(await history(second, SHARED), long).length > 0, "the second mob's member's run reaches its end too", 300_000);
            evidence.stops.push({ mob: SECOND_MOB, member: SHARED, stop_sent: false, ...classifyStop({ answer: null, frames: await history(second, SHARED), fullText: long }) });
            const accepted = callsOf(ACTIONS.stop, proxy.since(mark)).filter((call) => call.answered && !call.refused);
            assert.deepEqual(accepted.map((call) => call.params), [], "the gateway accepted no stop");
          });
        }
      } finally {
        await model({ source: "Ready.", delay_ms: 0, chunk_chars: 1024 });
        await readback("stop", "after");
      }
      if (offerError) throw offerError;
    });

    // -- a message whose answer was lost --------------------------------------
    await book.section("recover", async () => {
      await readback("recover", "before");
      try {
        secondChat = await openChat(second);
        if (!sendOffered) {
          await offer("recover", false, "no message can be written to the member, so none can be checked");
          await unavailable("recover", ["recover:sent", "recover:target", "recover:effect", "recover:other-mob"], null);
          return;
        }
        const text = `Lost answer for ${SECOND_MOB} ${randomUUID()}`;
        const answer = `Answer ${randomUUID()}.`;
        await model({ source: answer, delay_ms: 0, chunk_chars: 1024 });
        const before = await settled(launch);
        // The page writes only with a free composer and nothing waiting in
        // the chat's queue: wait for both, and say what held it if they do not come.
        const queue = secondChat.pane.locator('[data-testid^="pending-item:"]');
        const free = await eventually(async () => (await secondChat.composer.isEditable() && await queue.count() === 0) || null,
          "the chat's composer is free and its queue is empty", 30_000).then(() => true, () => false);
        let dropped = false;
        // The gateway takes the message and answers; the answer is dropped on
        // its way to the page, on either request door the send can take.
        const dropAnswer = async (route) => {
          if (dropped || !sendCarrying(route.request().postData() || "", text)) return route.continue();
          await route.fetch({ maxRetries: 0 });
          dropped = true;
          await route.abort("failed");
        };
        await page.route("**/console/rpc", dropAnswer);
        await page.route("**/console/rpc/multipart", dropAnswer);
        try {
          await book.run("recover:sent", async () => {
            const state = async () => ({ composer_free: free, queue: await queue.allInnerTexts().catch(() => []) });
            const held = await state();
            evidence.recovery.push({ at: "before writing", ...held });
            await secondChat.composer.fill(text);
            await secondChat.composer.press("Enter");
            const sentIt = await eventually(() => dropped || null, "the page sends the message", 20_000).then(() => true, () => false);
            const after = await state();
            evidence.recovery.push({ at: "after writing", sent: sentIt, ...after });
            assert(sentIt, `the page did not send the message; the chat's queue: ${JSON.stringify(after.queue)}; free before writing: ${held.composer_free}`);
            return { before: held, after };
          });
        } finally {
          await page.unroute("**/console/rpc", dropAnswer);
          await page.unroute("**/console/rpc/multipart", dropAnswer);
        }
        await eventually(async () => framesWith(await history(second, SHARED), answer).length > 0, "the member answered the message all the same", 60_000);
        const pendingRow = page.locator('[data-testid^="pending-item:"]').filter({ hasText: text });
        const check = pendingRow.getByRole("button", { name: "Check", exact: true });
        const shown = await check.waitFor({ timeout: 15_000 }).then(() => true, () => false);
        const recoverOffered = await offer("recover", shown && await check.isEnabled(), "the unconfirmed message offers Check");
        if (recoverOffered) {
          book.notApplicable("recover:unavailable", "the page offers the action");
          const mark = proxy.mark();
          await check.click();
          await book.run("recover:target", async () => {
            const call = await eventually(() => callsOf(ACTIONS.recover, proxy.since(mark)).find((item) => item.answered), "the check is sent and answered", 20_000);
            const form = namesShared(call, ACTIONS.recover, second);
            assert(!call.refused, `the gateway answers the check: ${JSON.stringify(call.answer)}`);
            return { door: call.door, form };
          });
          await book.run("recover:effect", async () => {
            await pendingRow.waitFor({ state: "detached", timeout: 20_000 });
            assert.equal(framesWith(await history(second, SHARED), text, "user_input").length, 1, "the member's history holds the message once");
            assert.equal(callsOf(ACTIONS.send, proxy.records).filter((item) => item.text.includes(text)).length, 1, "the message was sent once");
          });
          await book.run("recover:other-mob", async () => {
            for (const name of launch.members) {
              const frames = await history(launch, name);
              assert.equal(framesWith(frames, text).length + framesWith(frames, answer).length, 0, `${LAUNCH_MOB} ${name} holds nothing of the message`);
            }
            assert.deepEqual(await settled(launch), before, `${LAUNCH_MOB} is unchanged`);
          });
          await shot("8-second-checked");
        } else {
          await unavailable("recover", ["recover:target", "recover:effect", "recover:other-mob"], null);
        }
      } finally {
        await readback("recover", "after");
      }
    });

    // -- a retirement, last: it ends the second mob's member ------------------
    await book.section("retire", async () => {
      secondChat = await openChat(second);
      const menu = secondChat.pane.getByTestId("conv-menu");
      let item = null;
      if ((await menu.count()) === 1) {
        await menu.click();
        item = page.getByTestId("conv-action:retire");
        if ((await item.count()) !== 1) { item = null; await page.keyboard.press("Escape"); }
      }
      const retireOffered = await offer("retire", item !== null, "the chat's menu lists the retirement");
      if (retireOffered) {
        book.notApplicable("retire:unavailable", "the page offers the action");
        const before = await roster(launch);
        const mark = proxy.mark();
        await item.click();
        await secondChat.pane.getByTestId("conv-confirm-accept:retire").click();
        await book.run("retire:target", async () => {
          const call = await eventually(() => callsOf(ACTIONS.retire, proxy.since(mark)).find((entry) => entry.answered), "the retirement is sent and answered", 20_000);
          assert.equal(callsOf(ACTIONS.retire, proxy.since(mark)).length, 1, "the page sends one retirement");
          const form = namesShared(call, ACTIONS.retire, second);
          assert(!call.refused, `the gateway answers the retirement: ${JSON.stringify(call.answer)}`);
          return { door: call.door, form };
        });
        await book.run("retire:effect", async () => {
          await eventually(async () => !(await roster(second)).some(([name, status]) => name === SHARED && (status === null || status === "active")),
            "the second mob's member is no longer an active member of its mob", 30_000);
          await shot("9-second-retired");
        });
        await book.run("retire:other-mob", async () => {
          assert.deepEqual(await roster(launch), before, `the members of ${LAUNCH_MOB} are as they were`);
          const text = `After the retirement ${randomUUID()}`;
          const answer = `Answer ${randomUUID()}.`;
          await model({ source: answer, delay_ms: 0, chunk_chars: 1024 });
          await gateway.request("meta/send_message", { member_id: SHARED, message: text });
          await eventually(async () => framesWith(await history(launch, SHARED), answer).length > 0, "the launch mob's member still answers", 60_000);
        });
      } else {
        for (const id of ["retire:target", "retire:effect", "retire:other-mob"]) book.notApplicable(id, "the page does not offer the action");
        await book.run("retire:unavailable", async () => {
          const before = { [LAUNCH_MOB]: await settled(launch), [SECOND_MOB]: await settled(second) };
          const mark = proxy.mark();
          await openChat(launch);
          await openChat(second);
          await delay(1_000);
          const accepted = callsOf(ACTIONS.retire, proxy.since(mark)).filter((call) => call.answered && !call.refused);
          assert.deepEqual(accepted.map((call) => call.params), [], "the gateway accepted no retirement");
          assert.deepEqual({ [LAUNCH_MOB]: await settled(launch), [SECOND_MOB]: await settled(second) }, before, "neither mob changed");
        });
      }
    });

    assert.deepEqual(book.failed(), [], `every row passed or did not apply; the parts: ${JSON.stringify(book.sections)}`);
    assert.deepEqual(pageErrors, [], "the page raised no error");
    assert.deepEqual(book.rows.filter((row) => row.verdict === "not reached").map((row) => row.id), [], "every row ran");
    process.stdout.write(`browser real hosted mobs ok; not covered: ${book.notCovered().join(", ") || "nothing"}\n`);
  } catch (error) {
    if (page && !page.isClosed()) await page.screenshot({ path: path.join(evidenceDir, "failure.png"), fullPage: true }).catch(() => {});
    throw error;
  } finally {
    try { evidence.fixture_identity.at_end = fixtureIdentity(stateRoot, stateRoot, MOBS); }
    catch (error) { evidence.fixture_identity.at_end = { missing: String((error && error.message) || error) }; }
    fs.writeFileSync(path.join(evidenceDir, "result.json"), JSON.stringify({
      scenario: "real-hosted-mobs", mobs: MOBS.map((mob) => ({ id: mob.id, default: mob.default, members: mob.members })),
      offered_for_every_mob: OFFERED_FOR_EVERY_MOB, not_offered_for_another_mob: NOT_OFFERED_FOR_ANOTHER_MOB, offers, rows: book.rows, sections: book.sections, page_errors: pageErrors,
      evidence,
    }, null, 2));
    if (proxy) fs.writeFileSync(path.join(evidenceDir, "requests.json"), JSON.stringify(proxy.records, null, 2));
    fs.writeFileSync(path.join(evidenceDir, "gateway.log"), logs);
    const closed = await Promise.allSettled([browser?.close(), proxy?.close(), stopChild(child)]);
    fs.rmSync(stateRoot, { recursive: true, force: true });
    const failures = closed.filter((item) => item.status === "rejected").map((item) => item.reason);
    if (failures.length) throw new AggregateError(failures, "real hosted mobs teardown failed");
  }
}

module.exports = {
  scenarios: [{ id: "real-hosted-mobs", family: "console", backend: "real", run: runRealHostedMobs }],
  ACTIONS, OFFERED_FOR_EVERY_MOB, NOT_OFFERED_FOR_ANOTHER_MOB, offerFault, PLAN, NOT_COVERED,
  childEnvironment, mobToml, gatewayStdio, startRecordingProxy, callsOf, namedMember, groupRoster, rowOf, checkBook,
  BUNDLE_ASSETS, bundleEvidence, fixtureIdentity, storeReadback, runtimeKeyOf, classifyPeerFrames, classifyStop,
  multipartRequest, sendCarrying, storeHolding, modelRequestsCarrying, imageBlobIds, framesHolding,
  fileNeedles, runsSharedWith, filesHolding, typedRefusalFault, MOB_REFUSAL, mobRefusalFault, attachmentParamsFor, logCounts,
  stopChild, filesUnder, sha256,
};
if (require.main === module) {
  require("../scenario-registry.cjs").runScenarios(module.exports.scenarios).catch((error) => {
    process.stderr.write(`${error.stack || error.message}\n`);
    process.exit(1);
  });
}
