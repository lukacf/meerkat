#!/usr/bin/env node
// Request a host runnable during meta/init, before a public build can return.
// This fixture proves callback ordering and envelopes, not durable scheduling.
const readline = require("node:readline");
const lines = readline.createInterface({ input: process.stdin });
const emit = (value) => process.stdout.write(JSON.stringify(value) + "\n");
let initId;
let initOptions;
let callback;
lines.on("line", (line) => {
  const message = JSON.parse(line);
  if (message.method === "meta/init") {
    initId = message.id;
    initOptions = message.params.runtime_options;
    emit({ jsonrpc: "2.0", id: "init-fire", method: "callback/schedule_fire", params: {
      runnable: initOptions.host_runnables[0],
      occurrence: { schedule_id: "stored", occurrence_id: "due-at-boot", payload: { boot: true } },
    } });
  } else if (message.id === "init-fire") {
    callback = message;
    emit(message.error
      ? { jsonrpc: "2.0", id: initId, error: message.error }
      : { jsonrpc: "2.0", id: initId, result: {} });
  } else {
    emit({ jsonrpc: "2.0", id: message.id, result:
      message.method === "test/bootstrap" ? { initOptions, callback } : {} });
  }
});
lines.on("close", () => process.exit(0));
