"use strict";

// Fixture servers bind the address the harness passes, which is always port 0
// on loopback, and announce the address the OS actually bound as one stdout
// line:
//
//   META_FIXTURE_READY {"addr":"127.0.0.1:41234"}
//
// A harness that instead reserved a free port, closed it and handed the
// number to the child raced every other process on the host for that port
// between the close and the child's bind (CI saw AddrInUse, OS error 98).
// Binding port 0 in the process that serves leaves nothing to race.

const READY_PREFIX = "META_FIXTURE_READY ";

/** The address a fixture should bind: any free loopback port. */
const LOOPBACK_ANY_PORT = "127.0.0.1:0";

/**
 * Resolve with the fixture's bound address once `child` prints its ready
 * line. Rejects if the child exits first or the line does not arrive within
 * `timeoutMs`. Other stdout listeners on the child are unaffected.
 *
 * Call it in the same tick as `spawn`: stdout chunks are not replayed for a
 * listener attached later, so awaiting anything in between can miss the line.
 */
function awaitFixtureReady(child, { label = "fixture", timeoutMs = 60_000 } = {}) {
  return new Promise((resolve, reject) => {
    let buffer = "";
    let settled = false;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.stdout.off("data", onData);
      child.off("exit", onExit);
      child.off("error", onError);
      if (error) reject(error);
      else resolve(value);
    };
    const onData = (chunk) => {
      buffer += chunk;
      let end;
      while ((end = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, end).replace(/\r$/, "");
        buffer = buffer.slice(end + 1);
        if (!line.startsWith(READY_PREFIX)) continue;
        let addr;
        try {
          addr = JSON.parse(line.slice(READY_PREFIX.length)).addr;
        } catch (error) {
          finish(new Error(`${label} printed a malformed ready line: ${line}`));
          return;
        }
        if (typeof addr !== "string" || !/:\d+$/.test(addr) || addr.endsWith(":0")) {
          finish(new Error(`${label} announced no bound address: ${line}`));
          return;
        }
        finish(null, { addr, baseUrl: `http://${addr}` });
        return;
      }
    };
    const onExit = (code, signal) => {
      finish(new Error(`${label} exited before it was ready (code ${code}, signal ${signal})`));
    };
    const onError = (error) => finish(error);
    const timer = setTimeout(
      () => finish(new Error(`${label} did not announce a bound address within ${timeoutMs} ms`)),
      timeoutMs,
    );
    if (child.exitCode !== null || child.signalCode !== null) {
      onExit(child.exitCode, child.signalCode);
      return;
    }
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", onData);
    child.once("exit", onExit);
    child.once("error", onError);
  });
}

module.exports = { LOOPBACK_ANY_PORT, READY_PREFIX, awaitFixtureReady };
