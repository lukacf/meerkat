"use strict";

// Playwright calls a route handler outside the scenario's promise chain, so an
// error thrown there is an unhandled rejection that ends the process before the
// scenario reports. A handler still inside route.fetch when the scenario closes
// the browser fails that way even after every check passed. A guarded handler
// records its error and fails the request; settle() waits for the handlers
// still running while the browser is open, and check() fails the scenario with
// the first recorded error.
function routeGuard(page, label, { settleTimeoutMs = 15_000, log = line => process.stderr.write(`${line}\n`) } = {}) {
  const failures = [];
  const running = new Set();
  let ended = false;
  let settling = null;
  function describe(request) {
    let method;
    try { method = JSON.parse(request.postData() || "null")?.method; } catch { /* not a JSON-RPC body */ }
    let target = request.url();
    try { target = new URL(target).pathname; } catch { /* keep the raw URL */ }
    return `${request.method()} ${target}${typeof method === "string" ? ` ${method}` : ""}`;
  }
  function record(source, error) {
    // Only a bounded settle that gave up leaves handlers running after the
    // scenario ended; their failures are evidence, not the scenario's result.
    const failure = { source, error: String(error?.stack || error), afterEnd: ended };
    failures.push(failure);
    log(`${label}: ${source} failed${ended ? " after the scenario ended" : ""}: ${failure.error}`);
  }
  const runningRequests = () => [...running].map(call => call.request).join(", ");
  return {
    failures,
    async route(url, handler) {
      await page.route(url, async route => {
        const call = { request: describe(route.request()) };
        running.add(call);
        try {
          await handler(route);
        } catch (error) {
          record(`route handler for ${call.request}`, error);
          // An unhandled route stalls its request; fail it so the page sees the
          // failure now. The route may already be handled or the page closed.
          await route.abort("failed").catch(() => {});
        } finally {
          running.delete(call);
        }
      });
    },
    // Waits for the handlers still running, then stops routing. It never
    // throws, so a scenario's own error stays the one it reports.
    settle() {
      if (settling) return settling;
      if (running.size) log(`${label}: waiting for route handlers still running: ${runningRequests()}`);
      let timer;
      const expired = new Promise(resolve => { timer = setTimeout(() => resolve(true), settleTimeoutMs); });
      const unrouted = page.unrouteAll({ behavior: "wait" }).then(() => false, error => {
        record("unrouteAll", error);
        return false;
      });
      settling = Promise.race([unrouted, expired]).then(timedOut => {
        clearTimeout(timer);
        if (timedOut) record(`route handler for ${runningRequests()}`, new Error(`still running ${settleTimeoutMs} ms after the scenario ended`));
        ended = true;
      });
      return settling;
    },
    check() {
      const [first] = failures;
      if (first) throw new Error(`${label}: ${first.source} failed: ${first.error}`);
    },
  };
}

module.exports = { routeGuard };
