"use strict";

const assert = require("node:assert/strict");
const { spawn } = require("node:child_process");
const test = require("node:test");

const { LOOPBACK_ANY_PORT, awaitFixtureReady } = require("./fixture-ready.cjs");

function child(source, env = {}) {
  return spawn(process.execPath, ["-e", source], {
    env: { ...process.env, ...env },
    stdio: ["ignore", "pipe", "pipe"],
  });
}

test("a fixture that binds port 0 reports the port the OS bound", async () => {
  const server = child(`
    const http = require("node:http");
    const [host, port] = process.env.FIXTURE_ADDR.split(":");
    const server = http.createServer((req, res) => res.end("ok"));
    server.listen(Number(port), host, () => {
      process.stdout.write("starting\\n");
      process.stdout.write("META_FIXTURE_READY " + JSON.stringify({ addr: host + ":" + server.address().port }) + "\\n");
    });
  `, { FIXTURE_ADDR: LOOPBACK_ANY_PORT });
  const forwarded = [];
  server.stdout.on("data", chunk => forwarded.push(String(chunk)));
  try {
    const { addr, baseUrl } = await awaitFixtureReady(server, { label: "test fixture", timeoutMs: 10_000 });
    assert.match(addr, /^127\.0\.0\.1:[1-9]\d*$/);
    assert.equal(await (await fetch(baseUrl)).text(), "ok");
    assert.ok(forwarded.join("").includes("starting"), "other stdout listeners still see output");
  } finally {
    server.kill();
  }
});

test("a fixture that exits first rejects instead of waiting out the timeout", async () => {
  const dead = child(`process.stdout.write("no ready line\\n"); process.exit(3);`);
  await assert.rejects(
    awaitFixtureReady(dead, { label: "dead fixture", timeoutMs: 10_000 }),
    /dead fixture exited before it was ready \(code 3/,
  );
});

test("a ready line without a bound port is refused", async () => {
  const unbound = child(`process.stdout.write('META_FIXTURE_READY {"addr":"127.0.0.1:0"}\\n'); setTimeout(() => {}, 5000);`);
  try {
    await assert.rejects(
      awaitFixtureReady(unbound, { label: "unbound fixture", timeoutMs: 10_000 }),
      /unbound fixture announced no bound address/,
    );
  } finally {
    unbound.kill();
  }
});

test("no harness reserves a port for a fixture to bind later", () => {
  const { execFileSync } = require("node:child_process");
  const path = require("node:path");
  // Reserve-then-bind is a TOCTOU race against every process on the host.
  // Fixtures bind port 0 and announce the bound address instead.
  let found = "";
  try {
    found = execFileSync("git", [
      "grep", "-n", "-E", "reservePort|pick_port|_pick_free_port|getFreePort|findFreePort",
      "--", "console", "flow-editor", "examples", "sdk", "tests", "scripts",
      ":!**/node_modules/**", ":!console/dist/**", ":!console/fixture-ready.test.cjs",
    ], { cwd: path.join(__dirname, ".."), encoding: "utf8" }).trim();
  } catch (error) {
    // git grep exits 1 when nothing matches.
    if (error.status !== 1) throw error;
  }
  assert.equal(found, "", found);
});
