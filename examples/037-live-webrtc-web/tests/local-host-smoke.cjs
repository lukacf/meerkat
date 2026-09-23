// Provider-independent HTTP/runtime smoke. No live/open or media acquisition.
const assert = require("node:assert/strict");
const { spawn, execFileSync } = require("node:child_process");
const { once } = require("node:events");
const { createServer } = require("node:net");
const { join } = require("node:path");
const { mkdirSync, rmSync } = require("node:fs");
const { setTimeout: wait } = require("node:timers/promises");

(async () => {
  const reserve = createServer();
  await new Promise((resolve) => reserve.listen(0, "127.0.0.1", resolve));
  const port = reserve.address().port;
  await new Promise((resolve) => reserve.close(resolve));
  const fixtureRoot = join(__dirname, `.local-host-fixture-${process.pid}`);
  mkdirSync(fixtureRoot);
  const host = spawn(process.execPath, [join(__dirname, "../dist/server.js"), `--port=${port}`], {
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, HOME: fixtureRoot, XDG_CONFIG_HOME: fixtureRoot, XDG_DATA_HOME: fixtureRoot },
  });
  let output = "", rpcPids = [];
  host.stdout.on("data", (chunk) => { output += chunk; });
  host.stderr.on("data", (chunk) => { output += chunk; });
  const closed = once(host, "exit");
  try {
    const deadline = Date.now() + 20_000;
    while (!output.includes("runtime warmed")) {
      if (host.exitCode !== null || output.includes("warmup failed") || Date.now() > deadline) {
        throw new Error(`local runtime failed to warm: ${output}`);
      }
      await wait(50);
    }
    rpcPids = execFileSync("ps", ["-axo", "pid=,ppid="], { encoding: "utf8" })
      .trim().split("\n").map((line) => line.trim().split(/\s+/).map(Number))
      .filter(([, parent]) => parent === host.pid).map(([pid]) => pid);
    assert.equal(rpcPids.length, 1, "one owned RPC process");
    const base = `http://127.0.0.1:${port}`;
    for (const asset of ["/", "/app.js", "/live-session.js", "/transcript.js", "/styles.css"]) {
      const response = await fetch(base + asset, { signal: AbortSignal.timeout(2000) });
      assert.equal(response.status, 200, asset);
      assert.ok((await response.text()).length > 0);
    }
    const state = await (await fetch(base + "/api/state", { signal: AbortSignal.timeout(2000) })).json();
    assert.equal(state.active, false);
    assert.deepEqual(state.mobs, []);
    const unknown = await fetch(base + "/api/live/synthetic-missing/interrupt", {
      method: "POST", body: "{}", signal: AbortSignal.timeout(2000),
    });
    assert.equal(unknown.status, 404);
  } finally {
    host.kill("SIGTERM");
    const timeout = setTimeout(() => host.kill("SIGKILL"), 8000);
    await closed;
    clearTimeout(timeout);
    for (const pid of rpcPids) assert.throws(() => process.kill(pid, 0), { code: "ESRCH" });
    rmSync(fixtureRoot, { recursive: true, force: true });
  }
  console.log("PASS: real HTTP assets/state/control refusal, one native RPC child, shutdown reaped it; no provider/media used");
})().catch((error) => { console.error(error); process.exitCode = 1; });
