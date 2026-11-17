/**
 * `close()` leaves nothing behind that keeps the Node process alive, and a
 * child that exits by itself fails the requests waiting on it.
 *
 * `close()` ends the child's stdin, signals it, and waits for it to exit, up
 * to 5 s before it escalates to SIGKILL. A timer left running after the child
 * exited first kept every caller's process alive for those 5 s. Waiting for
 * the child's pipes to close instead of its exit let a descendant that holds
 * them keep `close()` waiting for the whole 5 s, or for ever after the kill.
 *
 * A child that exits by itself, a crash for instance, fails the pending
 * requests once its pipes have closed, so the error carries its whole stderr.
 * A descendant holding those pipes left the pending requests, and every
 * request sent after the exit, waiting for ever.
 */

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, it } from "node:test";

import { MeerkatClient } from "../dist/client.js";
import { CONTRACT_VERSION } from "../dist/generated/types.js";

const CLIENT_URL = new URL("../dist/client.js", import.meta.url).href;

/**
 * A descendant's script. Its arguments are the pid of the process that
 * started it and a path: it creates that file first, to say that it runs.
 * Once the process that started it has exited and been reaped, it writes one
 * more line to the stderr it inherited. It never lives longer than 10 s.
 */
const LATE_STDERR = `
require("node:fs").writeFileSync(process.argv[2], "");
setTimeout(() => process.exit(1), 10000).unref();
const parent = Number(process.argv[1]);
const poll = setInterval(() => {
  try {
    process.kill(parent, 0);
  } catch {
    clearInterval(poll);
    process.stderr.write("late stderr line");
  }
}, 10);`;

/**
 * A fake rkat-rpc that answers the handshake. `stubborn` ignores SIGTERM and
 * stdin EOF; `holder` leaves a descendant that inherited its stdout and
 * stderr; `exitAfterHandshake` exits once the handshake is answered.
 * `exitOnRequest` writes two stderr lines and exits with code 7, without a
 * reply, on the first request after the handshake; `lateStderr` leaves a
 * descendant on its stderr that writes one more line just after that exit;
 * the exit then waits until that descendant runs, and after 2 s, inside the
 * 3 s the exit probe allows the exit, says on stderr that it never did.
 * `corruptOnRequest` answers that first request with a line that is not
 * JSON and keeps running.
 */
function writeFixture(
  root,
  {
    stubborn = false,
    holder = false,
    exitAfterHandshake = false,
    exitOnRequest = false,
    lateStderr = false,
    corruptOnRequest = false,
  },
) {
  const bin = path.join(root, "rkat-rpc.cjs");
  const ready = JSON.stringify(path.join(root, "descendant.ready"));
  const exitNow = lateStderr ? "() => exitOnceDescendantRuns(Date.now() + 2000)" : "() => process.exit(7)";
  writeFileSync(bin, `#!/usr/bin/env node
const rl = require("node:readline").createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const msg = JSON.parse(line);
  ${exitOnRequest ? `if (msg.method !== "initialize" && msg.method !== "capabilities/get") {
    process.stderr.write("fixture: first stderr line\\nfixture: last stderr line\\n", ${exitNow});
    return;
  }` : ""}
  ${corruptOnRequest ? `if (msg.method !== "initialize" && msg.method !== "capabilities/get") {
    process.stdout.write("not a JSON-RPC frame\\n");
    return;
  }` : ""}
  const result = msg.method === "initialize"
    ? { contract_version: ${JSON.stringify(CONTRACT_VERSION)}, methods: [] }
    : { capabilities: [] };
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: msg.id, result }) + "\\n");
  ${exitAfterHandshake ? 'if (msg.method !== "initialize") process.exit(0);' : ""}
});
${stubborn ? 'process.on("SIGTERM", () => {}); setInterval(() => {}, 1000);' : ""}
${holder ? 'require("node:child_process").spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], { stdio: ["ignore", "inherit", "inherit"] }).unref();' : ""}
${lateStderr ? `require("node:child_process").spawn(process.execPath, ["-e", ${JSON.stringify(LATE_STDERR)}, String(process.pid), ${ready}], { stdio: ["ignore", "ignore", "inherit"] }).unref();
const exitOnceDescendantRuns = (deadline) => {
  if (require("node:fs").existsSync(${ready})) process.exit(7);
  else if (Date.now() < deadline) setTimeout(exitOnceDescendantRuns, 5, deadline);
  else process.stderr.write("fixture: the descendant never ran\\n", () => process.exit(7));
};` : ""}
`);
  chmodSync(bin, 0o755);
  return bin;
}

/** Past this a script and the fixture it spawned are killed, so a stuck client cannot hang the suite. */
const CLOSER_DEADLINE_MS = 15_000;

/**
 * Half the client's 1 s grace for a child's pipes after it exits by itself:
 * a grace timer left running would keep the process alive about that long.
 */
const EXIT_LINGER_LIMIT_MS = 500;

/**
 * Run a separate Node process that connects to `bin`, closes the client and
 * then has nothing left to do; with `afterExit`, it closes only once the
 * child has exited. Reports how long close() took, how long the process lived
 * after close() returned, what it wrote to stderr, and its process group.
 */
function runCloser(root, bin, { afterExit = false } = {}) {
  return runScript(root, "closer.mjs", `
import { MeerkatClient } from ${JSON.stringify(CLIENT_URL)};
const client = new MeerkatClient(${JSON.stringify(bin)});
await client.connect();
const child = client.process;
const pid = child.pid;
${afterExit ? 'if (child.exitCode === null && child.signalCode === null) await new Promise((resolve) => child.once("exit", resolve));' : ""}
const started = performance.now();
await client.close();
process.stdout.write(JSON.stringify({ pid, closeMs: performance.now() - started }) + "\\n");
`);
}

/**
 * Run a separate Node process that connects to `bin`, a fixture that exits on
 * the first request after the handshake. It reports how three requests
 * settled: the one the child exits on, one sent right after the exit, and one
 * sent once the child's pipes have closed, the first two with the time from
 * the exit. It also reports whether those pipes closed and how often the
 * client closed its queues, then closes the client and has nothing left to do.
 */
function runExitProbe(root, bin) {
  return runScript(root, "exit-probe.mjs", `
import { MeerkatClient } from ${JSON.stringify(CLIENT_URL)};
// Settles to "unsettled" after ms; the timer never keeps this process alive.
function within(promise, ms) {
  let timer;
  const cap = new Promise((resolve) => {
    timer = setTimeout(resolve, ms, "unsettled");
    timer.unref();
  });
  return Promise.race([promise, cap]).finally(() => clearTimeout(timer));
}
function outcome(request) {
  return request.then(
    () => ({ code: "resolved" }),
    (error) => ({ code: error.code, message: error.message, at: performance.now() }),
  );
}
const client = new MeerkatClient(${JSON.stringify(bin)});
await client.connect();
const child = client.process;
let queueCloses = 0;
const closeQueues = client.closeQueues.bind(client);
client.closeQueues = () => {
  queueCloses += 1;
  closeQueues();
};
const exited = new Promise((resolve) => child.once("exit", () => resolve(performance.now())));
const pipesClosed = new Promise((resolve) => child.once("close", () => resolve(true)));
const pending = outcome(client.getConfig());
const exitedAt = await within(exited, 3_000);
const afterExit = outcome(client.getConfig());
const report = { exited: exitedAt !== "unsettled" };
report.pending = await within(pending, 3_000);
report.afterExit = await within(afterExit, 1_000);
report.pipesClosed = (await within(pipesClosed, 1_000)) === true;
report.later = await within(outcome(client.getConfig()), 1_000);
report.queueCloses = queueCloses;
for (const settled of [report.pending, report.afterExit]) {
  if (report.exited && typeof settled === "object") settled.ms = settled.at - exitedAt;
}
const started = performance.now();
await client.close();
report.closeMs = performance.now() - started;
process.stdout.write(JSON.stringify(report) + "\\n");
`);
}

/**
 * Run a separate Node process that connects to `bin`, a fixture that answers
 * the first request after the handshake with a line that is not JSON. It
 * reports how that request and a later one failed and how long close() took
 * after them, then has nothing left to do. With `closeAfterChildExit` it
 * calls close() only once the stopped child has closed (or after 3 s), so the
 * client sees the child's exit first. As it exits it writes a second line:
 * how often the client closed its queues in all.
 */
function runFaultProbe(root, bin, { closeAfterChildExit = false } = {}) {
  return runScript(root, "fault-probe.mjs", `
import { writeSync } from "node:fs";
import { MeerkatClient } from ${JSON.stringify(CLIENT_URL)};
const client = new MeerkatClient(${JSON.stringify(bin)});
await client.connect();
const child = client.process;
let queueCloses = 0;
const closeQueues = client.closeQueues.bind(client);
client.closeQueues = () => {
  queueCloses += 1;
  closeQueues();
};
process.on("exit", () => writeSync(1, JSON.stringify({ queueCloses }) + "\\n"));
${
  closeAfterChildExit
    ? `const childClosed = new Promise((resolve) => {
  const timer = setTimeout(resolve, 3_000, false);
  child.once("close", () => {
    clearTimeout(timer);
    resolve(true);
  });
});`
    : ""
}
const fault = await client.getConfig().then(() => "resolved", (error) => error.code);
const later = await client.getConfig().then(() => "resolved", (error) => error.code);
const report = { pid: child.pid, fault, later };
${closeAfterChildExit ? "report.childClosed = await childClosed;" : ""}
const started = performance.now();
await client.close();
report.closeMs = performance.now() - started;
try {
  process.kill(child.pid, 0);
  report.childAliveAfterClose = true;
} catch (error) {
  if (error.code !== "ESRCH") throw error;
  report.childAliveAfterClose = false;
}
process.stdout.write(JSON.stringify(report) + "\\n");
`);
}

/**
 * Run `source` as a separate Node process in a process group of its own; the
 * fixture joins it, so one kill takes both. The script writes one JSON line
 * when it is done and should then have nothing left to do; it may write one
 * more as it exits. Resolves with the first report, the second as `final`,
 * how long the process lived after the first, what it wrote to stderr, and its
 * process group.
 */
function runScript(root, name, source) {
  const script = path.join(root, name);
  writeFileSync(script, source);
  const env = { ...process.env };
  delete env.MEERKAT_BIN_PATH;
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script], {
      env,
      stdio: ["ignore", "pipe", "pipe"],
      detached: true,
    });
    let stdout = "";
    let stderr = "";
    let reportedAt;
    let killed = false;
    const deadline = setTimeout(() => {
      killed = true;
      try {
        process.kill(-child.pid, "SIGKILL");
      } catch (error) {
        if (error.code !== "ESRCH") throw error;
      }
    }, CLOSER_DEADLINE_MS);
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
      if (reportedAt === undefined && stdout.includes("\n")) reportedAt = performance.now();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.once("error", (error) => {
      clearTimeout(deadline);
      reject(error);
    });
    // "close", not "exit": every byte the script wrote has been read by then.
    child.once("close", (code, signal) => {
      clearTimeout(deadline);
      const exitedAt = performance.now();
      if (killed) {
        const state = reportedAt === undefined ? "before it reported" : `after it reported ${stdout.trim()}`;
        reject(new Error(`killed ${name} and its fixture ${CLOSER_DEADLINE_MS} ms in, ${state}`));
        return;
      }
      if (code !== 0 || reportedAt === undefined) {
        reject(new Error(`${name} failed (code ${code}, signal ${signal}): ${stderr}`));
        return;
      }
      const [report, final] = stdout.trim().split("\n").map((line) => JSON.parse(line));
      resolve({ ...report, final, lingerMs: exitedAt - reportedAt, stderr, group: child.pid });
    });
  });
}

/** Kill what is left of a script's process group, such as a fixture's descendant. */
function killGroup(group) {
  try {
    process.kill(-group, "SIGKILL");
  } catch (error) {
    if (error.code !== "ESRCH") throw error;
  }
}

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === "ESRCH") return false;
    throw error;
  }
}

describe("MeerkatClient.close()", () => {
  it("lets the process exit as soon as the child has closed", { timeout: 20_000 }, async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-close-"));
    try {
      const run = await runCloser(root, writeFixture(root, { stubborn: false }));
      assert.ok(run.closeMs < 2_000, `close() took ${Math.round(run.closeMs)} ms`);
      assert.ok(
        run.lingerMs < 1_000,
        `the process stayed alive ${Math.round(run.lingerMs)} ms after close() returned`,
      );
      assert.equal(run.stderr, "");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("still waits 5 s, then kills a child that ignores SIGTERM", { timeout: 20_000 }, async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-close-"));
    try {
      const run = await runCloser(root, writeFixture(root, { stubborn: true }));
      assert.ok(
        run.closeMs >= 4_500 && run.closeMs < 9_000,
        `close() took ${Math.round(run.closeMs)} ms, not the 5 s cap`,
      );
      assert.equal(alive(run.pid), false, "close() must kill the stubborn child");
      assert.ok(
        run.lingerMs < 1_000,
        `the process stayed alive ${Math.round(run.lingerMs)} ms after close() returned`,
      );
      assert.equal(run.stderr, "");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("returns once a killed child has exited, even when a descendant holds its pipes", { timeout: 20_000 }, async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-close-"));
    let group;
    try {
      const run = await runCloser(root, writeFixture(root, { stubborn: true, holder: true }));
      group = run.group;
      assert.ok(
        run.closeMs >= 4_500 && run.closeMs < 9_000,
        `close() took ${Math.round(run.closeMs)} ms, not the 5 s cap`,
      );
      assert.equal(alive(run.pid), false, "close() must kill the stubborn child");
      assert.ok(
        run.lingerMs < 1_000,
        `the process stayed alive ${Math.round(run.lingerMs)} ms after close() returned`,
      );
      assert.equal(run.stderr, "");
    } finally {
      if (group !== undefined) killGroup(group);
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("returns as soon as the child exits, even when a descendant holds its pipes", { timeout: 20_000 }, async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-close-"));
    let group;
    try {
      const run = await runCloser(root, writeFixture(root, { holder: true }));
      group = run.group;
      assert.ok(run.closeMs < 1_000, `close() took ${Math.round(run.closeMs)} ms`);
      assert.equal(alive(run.pid), false, "the child must have exited");
      assert.ok(
        run.lingerMs < 1_000,
        `the process stayed alive ${Math.round(run.lingerMs)} ms after close() returned`,
      );
      assert.equal(run.stderr, "");
    } finally {
      if (group !== undefined) killGroup(group);
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("returns at once for a child that has already exited while a descendant holds its pipes", { timeout: 20_000 }, async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-close-"));
    let group;
    try {
      const fixture = writeFixture(root, { holder: true, exitAfterHandshake: true });
      const run = await runCloser(root, fixture, { afterExit: true });
      group = run.group;
      // This child exited by itself, so the client's 1 s grace for its pipes
      // is running: close() must let go of them itself, not leave it to that.
      assert.ok(run.closeMs < EXIT_LINGER_LIMIT_MS, `close() took ${Math.round(run.closeMs)} ms`);
      assert.ok(
        run.lingerMs < EXIT_LINGER_LIMIT_MS,
        `the process stayed alive ${Math.round(run.lingerMs)} ms after close() returned`,
      );
      assert.equal(run.stderr, "");
    } finally {
      if (group !== undefined) killGroup(group);
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("returns when signalling the child fails and no exit follows", { timeout: 10_000 }, async () => {
    const client = new MeerkatClient();
    // Node reports a kill that fails, with EPERM for instance, as an "error"
    // event from kill() itself; the child is still running.
    const child = Object.assign(new EventEmitter(), {
      stdin: { end() {}, destroy() {} },
      exitCode: null,
      signalCode: null,
      kill() {
        this.emit("error", Object.assign(new Error("kill EPERM"), { code: "EPERM" }));
        return false;
      },
    });
    client.process = child;
    let timer;
    const outcome = await Promise.race([
      client.close().then(() => "returned"),
      new Promise((resolve) => {
        timer = setTimeout(resolve, 1_000, "still waiting after 1 s");
      }),
    ]);
    clearTimeout(timer);
    assert.equal(outcome, "returned");
  });
});

describe("A child that exits by itself", () => {
  it("fails pending and later requests, even when a descendant holds its pipes", { timeout: 20_000 }, async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-close-"));
    let group;
    try {
      const run = await runExitProbe(root, writeFixture(root, { holder: true, exitOnRequest: true }));
      group = run.group;
      assert.ok(run.exited, "the fixture must exit on the request");
      for (const key of ["pending", "afterExit"]) {
        const settled = run[key];
        assert.equal(settled.code, "PROCESS_EXITED", `${key}: ${JSON.stringify(settled)}`);
        assert.ok(settled.ms < 3_000, `${key} settled ${Math.round(settled.ms)} ms after the exit`);
      }
      assert.match(
        run.pending.message,
        /exited before replying \(code 7, signal null\): fixture: first stderr line\nfixture: last stderr line$/,
      );
      assert.equal(run.later.code, "NOT_CONNECTED", JSON.stringify(run.later));
      assert.ok(run.pipesClosed, "the client must let go of the pipes the descendant holds");
      assert.equal(run.queueCloses, 1, "the exit must be settled once");
      assert.ok(run.closeMs < 1_000, `close() took ${Math.round(run.closeMs)} ms`);
      assert.ok(
        run.lingerMs < EXIT_LINGER_LIMIT_MS,
        `the process stayed alive ${Math.round(run.lingerMs)} ms after its last request settled`,
      );
      assert.equal(run.stderr, "");
    } finally {
      if (group !== undefined) killGroup(group);
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("reports the child's whole stderr tail once its pipes close", { timeout: 20_000 }, async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-close-"));
    let group;
    try {
      const run = await runExitProbe(root, writeFixture(root, { exitOnRequest: true }));
      group = run.group;
      assert.ok(run.exited, "the fixture must exit on the request");
      assert.equal(run.pending.code, "PROCESS_EXITED", JSON.stringify(run.pending));
      assert.match(
        run.pending.message,
        /exited before replying \(code 7, signal null\): fixture: first stderr line\nfixture: last stderr line$/,
      );
      assert.ok(
        run.pending.ms < EXIT_LINGER_LIMIT_MS,
        `settled ${Math.round(run.pending.ms)} ms after the exit, not when the pipes closed`,
      );
      // Sent once the exit is known; its pipes may have closed by then.
      assert.ok(
        ["PROCESS_EXITED", "NOT_CONNECTED"].includes(run.afterExit.code),
        JSON.stringify(run.afterExit),
      );
      assert.equal(run.later.code, "NOT_CONNECTED", JSON.stringify(run.later));
      assert.ok(run.pipesClosed);
      assert.equal(run.queueCloses, 1, "the exit must be settled once");
      assert.ok(
        run.lingerMs < EXIT_LINGER_LIMIT_MS,
        `the process stayed alive ${Math.round(run.lingerMs)} ms after its last request settled`,
      );
      assert.equal(run.stderr, "");
    } finally {
      if (group !== undefined) killGroup(group);
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("waits briefly for a descendant to release the pipes, and reports what it wrote", { timeout: 20_000 }, async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-close-"));
    let group;
    try {
      const run = await runExitProbe(root, writeFixture(root, { exitOnRequest: true, lateStderr: true }));
      group = run.group;
      assert.ok(run.exited, "the fixture must exit on the request");
      for (const key of ["pending", "afterExit"]) {
        const settled = run[key];
        assert.equal(settled.code, "PROCESS_EXITED", `${key}: ${JSON.stringify(settled)}`);
        assert.match(
          settled.message,
          /\(code 7, signal null\): fixture: first stderr line\nfixture: last stderr line\nlate stderr line$/,
        );
      }
      assert.equal(run.later.code, "NOT_CONNECTED", JSON.stringify(run.later));
      assert.equal(run.queueCloses, 1, "the exit must be settled once");
      assert.ok(
        run.lingerMs < EXIT_LINGER_LIMIT_MS,
        `the process stayed alive ${Math.round(run.lingerMs)} ms after its last request settled`,
      );
      assert.equal(run.stderr, "");
    } finally {
      if (group !== undefined) killGroup(group);
      rmSync(root, { recursive: true, force: true });
    }
  });
});

describe("A protocol error from the child", () => {
  it("lets the process exit as soon as the stopped child has exited", { timeout: 20_000 }, async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-close-"));
    try {
      const run = await runFaultProbe(root, writeFixture(root, { corruptOnRequest: true }), {
        closeAfterChildExit: true,
      });
      assert.equal(run.fault, "PROTOCOL_ERROR");
      assert.equal(run.later, "PROTOCOL_ERROR");
      assert.equal(run.childClosed, true, "the stopped child must close before close() is called");
      assert.ok(run.closeMs < 1_000, `close() took ${Math.round(run.closeMs)} ms`);
      assert.equal(run.childAliveAfterClose, false, "the child must have exited when close() returns");
      // Once for the fault, once more for close(): the child's exit, seen
      // first, adds none.
      assert.deepEqual(run.final, { queueCloses: 2 });
      assert.equal(alive(run.pid), false, "the child must have been stopped");
      assert.ok(
        run.lingerMs < 1_000,
        `the process stayed alive ${Math.round(run.lingerMs)} ms after close() returned`,
      );
      assert.equal(run.stderr, "");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("lets go of the child's pipes, even when a descendant holds them", { timeout: 20_000 }, async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-close-"));
    let group;
    try {
      const run = await runFaultProbe(root, writeFixture(root, { holder: true, corruptOnRequest: true }), {
        closeAfterChildExit: true,
      });
      group = run.group;
      assert.equal(run.fault, "PROTOCOL_ERROR");
      assert.equal(run.later, "PROTOCOL_ERROR");
      assert.equal(run.childClosed, true, "the stopped child must close before close() is called");
      assert.ok(run.closeMs < 1_000, `close() took ${Math.round(run.closeMs)} ms`);
      assert.equal(run.childAliveAfterClose, false, "the child must have exited when close() returns");
      // Once for the fault, once more for close(): the child's exit, seen
      // first, adds none.
      assert.deepEqual(run.final, { queueCloses: 2 });
      assert.equal(alive(run.pid), false, "the child must have been stopped");
      assert.ok(
        run.lingerMs < 1_000,
        `the process stayed alive ${Math.round(run.lingerMs)} ms after close() returned`,
      );
      assert.equal(run.stderr, "");
    } finally {
      if (group !== undefined) killGroup(group);
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("kills a child that ignores SIGTERM 5 s later", { timeout: 20_000 }, async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-close-"));
    try {
      const run = await runFaultProbe(root, writeFixture(root, { stubborn: true, corruptOnRequest: true }));
      assert.equal(run.fault, "PROTOCOL_ERROR");
      assert.equal(run.later, "PROTOCOL_ERROR");
      // close(), called at once after the fault, waits for the stop the fault
      // started: the stubborn child is killed 5 s after the fault.
      assert.ok(
        run.closeMs >= 4_500 && run.closeMs < 9_000,
        `close() took ${Math.round(run.closeMs)} ms, not until the 5 s kill`,
      );
      assert.equal(run.childAliveAfterClose, false, "the stubborn child must be dead when close() returns");
      // Once for the fault, once more for close(): the child's exit adds none.
      assert.deepEqual(run.final, { queueCloses: 2 });
      assert.ok(
        run.lingerMs < 1_000,
        `the process stayed alive ${Math.round(run.lingerMs)} ms after close() returned`,
      );
      assert.equal(alive(run.pid), false, "the stubborn child must have been killed");
      assert.equal(run.stderr, "");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
