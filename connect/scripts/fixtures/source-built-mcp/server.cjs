"use strict";

// A bounded local MCP fixture. Every successful tool result calls the compiled
// addon; there is no JavaScript arithmetic fallback or direct-answer argument.
const addon = require("./build/Release/addon.node");
const inputSchema = require("./operation.schema.json");
const decoder = new TextDecoder("utf-8", {fatal: true});
const versions = new Set(["2024-11-05", "2025-03-26", "2025-06-18"]);
let initialized = false;
let ready = false;
let pending = Buffer.alloc(0);

function send(value) {
  process.stdout.write(JSON.stringify(value) + "\n");
}

function error(id, code, message) {
  send({jsonrpc: "2.0", id, error: {code, message}});
}

function dispatch(raw) {
  let request;
  try {
    request = JSON.parse(decoder.decode(raw));
  } catch (_) {
    error(null, -32700, "Invalid JSON");
    return;
  }
  if (!request || typeof request !== "object" || Array.isArray(request) ||
      request.jsonrpc !== "2.0" || typeof request.method !== "string") {
    error(null, -32600, "Invalid request");
    return;
  }
  const hasId = Object.prototype.hasOwnProperty.call(request, "id");
  if (!hasId) {
    if (request.method === "notifications/initialized" && initialized) ready = true;
    return;
  }
  const id = request.id;
  if (!(typeof id === "string" || (typeof id === "number" && Number.isSafeInteger(id)))) {
    error(null, -32600, "Invalid request id");
    return;
  }
  if (request.method === "ping") {
    send({jsonrpc: "2.0", id, result: {}});
    return;
  }
  if (request.method === "initialize") {
    if (initialized || !request.params || typeof request.params.protocolVersion !== "string") {
      error(id, -32602, "Invalid initialization");
      return;
    }
    initialized = true;
    const requested = request.params.protocolVersion;
    send({jsonrpc: "2.0", id, result: {
      protocolVersion: versions.has(requested) ? requested : "2025-03-26",
      capabilities: {tools: {}},
      serverInfo: {name: "source-built-mcp-fixture", version: "1.0.0"}
    }});
    return;
  }
  if (!ready) {
    error(id, -32000, "Initialization is incomplete");
    return;
  }
  if (request.method === "tools/list") {
    send({jsonrpc: "2.0", id, result: {tools: [{
      name: "source_add",
      description: "Add two bounded integers by invoking the locally compiled fixture addon.",
      inputSchema,
      annotations: {readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false}
    }]}});
    return;
  }
  if (request.method === "tools/call") {
    const params = request.params;
    const args = params && params.arguments;
    if (!params || params.name !== "source_add" || !args || typeof args !== "object" ||
        Array.isArray(args) || Object.keys(args).sort().join(",") !== "left,right" ||
        ![args.left, args.right].every(value => Number.isSafeInteger(value) && Math.abs(value) <= 1000000)) {
      error(id, -32602, "Expected source_add with exactly two bounded integer arguments");
      return;
    }
    const sum = addon.add(args.left, args.right);
    if (!Number.isSafeInteger(sum)) throw new Error("Compiled addon returned an invalid result");
    send({jsonrpc: "2.0", id, result: {
      content: [{type: "text", text: JSON.stringify({sum})}], isError: false
    }});
    return;
  }
  error(id, -32601, "Method not found");
}

process.stdin.on("data", chunk => {
  pending = Buffer.concat([pending, chunk]);
  let newline;
  while ((newline = pending.indexOf(10)) !== -1) {
    if (newline > 65536) {
      process.stderr.write("Fixture input exceeds the line bound\n");
      process.exit(1);
    }
    const line = pending.subarray(0, newline);
    pending = pending.subarray(newline + 1);
    if (line.length) dispatch(line);
  }
  if (pending.length > 65536) {
    process.stderr.write("Fixture input exceeds the line bound\n");
    process.exit(1);
  }
});
process.stdin.on("end", () => {
  if (pending.length) process.exitCode = 1;
});
process.stdin.on("error", () => { process.exitCode = 1; });
