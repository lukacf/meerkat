"use strict";
const assert = require("node:assert/strict");
const http = require("node:http");
const { performance } = require("node:perf_hooks");

// A transparent test boundary between two isolated real runtime authorities.
// It preserves successful owner response bytes while delaying their delivery.
async function startScopeProxy(owners) {
  const observations = [], held = [], streams = new Set(), pending = new Set();
  let hold = null;
  const now = () => performance.now();
  const server = http.createServer(async (request, response) => {
    const match = /^\/(scope-[ab])(?=\/)/.exec(request.url);
    const scope = match?.[1] || null;
    const pathname = scope ? request.url.slice(scope.length + 1) : request.url;
    const target = new URL(pathname, owners[scope || "scope-a"].baseUrl);
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = Buffer.concat(chunks);
    const observation = { scope, path: pathname, method: request.method, request: body.toString(),
      cursor: request.headers["last-event-id"] || null, startedAt: now(), status: null, response: "" };
    observations.push(observation);
    const upstream = http.request(target, { method: request.method, headers: { ...request.headers, host: target.host } });
    pending.add(upstream);
    response.on("close", () => {
      if (!response.writableFinished) observation.abortedAt = now();
      // A held successful body is already fully owned by this proxy. Keep its
      // immutable bytes for the late-release witness after the client aborts.
      if (!observation.heldAt) upstream.destroy();
    });
    upstream.on("response", incoming => {
      observation.status = incoming.statusCode;
      let rpc;
      try { rpc = JSON.parse(observation.request); } catch { /* Non-RPC request. */ }
      const shouldHold = hold && scope === hold.scope && incoming.statusCode === 200 &&
        rpc?.method === "meta/console/query_timeline" && rpc.params?.mode === "recent" &&
        rpc.params?.limit === 200 && !rpc.params?.before && !rpc.params?.after &&
        (hold.host === "shared" ? rpc.params?.identity === "router:main" : !rpc.params?.identity);
      const isStream = pathname.startsWith("/console/timeline/stream");
      if (shouldHold) {
        const bytes = [];
        incoming.on("data", chunk => bytes.push(chunk));
        incoming.on("end", () => {
          const result = Buffer.concat(bytes);
          observation.response = result.toString(); observation.heldAt = now();
          const item = { observation, release() {
            observation.releasedAt = now();
            if (!response.destroyed) { response.writeHead(incoming.statusCode, incoming.headers); response.end(result); }
          } };
          held.push(item); pending.delete(upstream);
        });
      } else {
        const stream = { incoming, response, observation };
        if (isStream) streams.add(stream);
        response.writeHead(incoming.statusCode, incoming.headers);
        if (!isStream && scope) incoming.on("data", chunk => { observation.response += chunk; });
        incoming.on("end", () => { streams.delete(stream); pending.delete(upstream); observation.endedAt = now(); });
        incoming.pipe(response);
        response.on("close", () => { streams.delete(stream); incoming.destroy(); });
      }
      incoming.on("error", error => { pending.delete(upstream); if (!response.destroyed) response.destroy(error); });
    });
    upstream.on("error", error => {
      pending.delete(upstream); observation.upstreamError = error.message;
      if (!response.headersSent && !response.destroyed) response.writeHead(502);
      if (!response.destroyed) response.end(error.message);
    });
    upstream.end(body);
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  return {
    baseUrl: `http://127.0.0.1:${server.address().port}`, observations, held, now,
    holdHistory(scope, host) { assert.equal(hold, null); hold = { scope, host }; },
    releaseHistory() { hold = null; for (const item of held) if (!item.observation.releasedAt) item.release(); },
    disconnect(scope) {
      for (const stream of streams) if (stream.observation.scope === scope) {
        // A real, well-framed EOF forces recovery without turning this fault
        // into an unrelated browser network failure that needs allowlisting.
        stream.observation.deliberateEndAt = now();
        stream.response.end(); stream.incoming.destroy();
      }
    },
    async close() {
      hold = null;
      for (const item of held) item.release();
      for (const request of pending) request.destroy();
      for (const stream of streams) { stream.response.destroy(); stream.incoming.destroy(); }
      server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
    },
  };
}

module.exports = { startScopeProxy };
