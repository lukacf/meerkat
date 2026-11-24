#!/usr/bin/env node
// Protocol stand-in: echo init options and return each SDK callback envelope.
// It deliberately implements no scheduler or durable occurrence store.
const readline = require("node:readline");
const lines = readline.createInterface({ input: process.stdin });
const pending = new Map();
let sequence = 0;
let options;
const emit = (value) => process.stdout.write(JSON.stringify(value) + "\n");
lines.on("line", (line) => {
  const message = JSON.parse(line);
  const answer = (result) => emit({ jsonrpc: "2.0", id: message.id, result });
  if (pending.has(message.id)) {
    emit({ jsonrpc: "2.0", id: pending.get(message.id), result: message });
    pending.delete(message.id);
  } else if (message.method === "meta/init") {
    options = message.params.runtime_options;
    answer({});
  } else if (message.method === "test/init-options") {
    answer(options);
  } else if (message.method === "test/fire") {
    const id = `callback-${++sequence}`;
    pending.set(id, message.id);
    emit({ jsonrpc: "2.0", id, method: "callback/schedule_fire", params: message.params });
  } else {
    answer({});
  }
});
lines.on("close", () => process.exit(0));
