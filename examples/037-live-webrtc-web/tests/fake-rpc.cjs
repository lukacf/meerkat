#!/usr/bin/env node
// Harmless offline RPC peer. It stays alive until its owning SDK closes stdin.
const readline = require("node:readline");
const mode = process.env.EXAMPLE_TEST_RPC_MODE;
const version = require("../../../sdks/typescript/package.json").version;
if (process.argv.includes("--version")) {
  console.log(`rkat-rpc ${version}`);
} else {
  readline.createInterface({ input: process.stdin }).on("line", (line) => {
    const request = JSON.parse(line);
    let result = {};
    if (request.method === "initialize") result = {
      contract_version: mode === "version" ? "99.0.0" : version,
      methods: mode === "success" ? ["mob/create"] : [],
    };
    if (request.method === "capabilities/get") result = {
      capabilities: mode === "malformed" ? "not-a-list" : [],
    };
    process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id: request.id, result })}\n`);
  });
}
