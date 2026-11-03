#!/usr/bin/env node
// Inert child for the real SDK transport, not a native runtime or grant owner.
import { appendFileSync } from "node:fs";
import { basename } from "node:path";
import { createInterface } from "node:readline";
import { Socket } from "node:net";

const mode = basename(process.argv[1], ".mjs");
const log = event => appendFileSync(`${process.argv[1]}.log`, JSON.stringify({ pid: process.pid, ...event }) + "\n");
log({ kind: "start", args: process.argv.slice(2) });
process.on("exit", code => log({ kind: "exit", code }));
const commands = new Socket({ fd: 3, readable: true, writable: false });
const responses = new Socket({ fd: 4, readable: false, writable: true });
commands.on("error", () => {}); responses.on("error", () => {});
const privateLines = createInterface({ input: commands });
const nativeLines = createInterface({ input: process.stdin });
let privateOpen = true;
let selectedId;
let preparation;
let preparingInit;
let prepared = false;
let initSettled = false;
let initAccepted = false;
let waitingForTerm;
if (mode === "term-zero") process.on("SIGTERM", () => {
  log({ kind: "sigterm-observed" });
  clearInterval(waitingForTerm);
  privateLines.close(); commands.destroy(); responses.end();
});
privateLines.on("close", () => {
  privateOpen = false; log({ kind: "private-eof" });
  if (preparingInit && !initSettled) {
    initSettled = true;
    native({ jsonrpc: "2.0", method: "meta/init_settled", params: {
      init_id: preparingInit.params.init_id, outcome: "failed", code: -32014,
      message: "fixture private preparation closed", data: { preparation: true }, durable_effects: "possible" } });
  }
});
const native = value => process.stdout.write(JSON.stringify(value) + "\n");
const privateReply = (id, result) => responses.write(JSON.stringify({ version: 1, id, result }) + "\n");

function startPreparationInit() {
  if (!preparation || !preparingInit || initAccepted) return;
  initAccepted = true;
  native({ jsonrpc: "2.0", id: preparingInit.id, result: {
    init_state: "accepted", init_id: preparingInit.params.init_id,
    stdio_shutdown_handshake: true, stdio_shutdown_horizon_ms: 2000 } });
  if (!["prepare-pending", "prepare-refused"].includes(mode)) native({
    jsonrpc: "2.0", id: "before-prepared", method: "callback/roster_provider/roster",
    params: { context: { mob_definition: null, previous_identities: [] } } });
}

privateLines.on("line", line => {
  const request = JSON.parse(line); log({ kind: "private", request });
  if (request.command.kind === "prepare_application") {
    preparation = request;
    if (mode === "prepare-refused") privateReply(request.id, { kind: "error", error: "denied" });
    startPreparationInit();
    return;
  }
  if (preparation && request.command.kind === "bootstrap") {
    if (!prepared || request.id !== 2) {
      privateReply(request.id, { kind: "error", error: "denied" }); return;
    }
    if (mode !== "prepare-adopt-lost") privateReply(request.id, { kind: "bootstrap_accepted" });
    initSettled = true;
    native({ jsonrpc: "2.0", method: "meta/init_settled", params: mode === "prepare-native-failed"
      ? { init_id: preparingInit.params.init_id, outcome: "failed", code: -32014,
          message: "fixture entered preparation failed", data: { marker: 44 }, durable_effects: "possible" }
      : { init_id: preparingInit.params.init_id, outcome: "ready" } });
    return;
  }
  if (["bootstrap", "replace_application"].includes(request.command.kind)) {
    selectedId = request.id;
    if (mode === "private-refused") {
      privateReply(request.id, { kind: "error", error: "denied" });
    } else {
      // This round trip must finish before private selection can return.
      // It uses the native SDK's registered callback reader and dispatcher.
      native({ jsonrpc: "2.0", id: "before-private-ack", method: "callback/roster_provider/roster",
        params: { context: { mob_definition: null, previous_identities: [] } } });
    }
  } else if (request.command.kind === "open_component") {
    privateReply(request.id, { kind: "component", observation: {
      incarnation: "00000000-0000-0000-0000-000000000001", component: request.command.component,
      revision: 0, state: "open" } });
  } else privateReply(request.id, { kind: "error", error: "denied" });
});

nativeLines.on("line", line => {
  const request = JSON.parse(line); log({ kind: "native", request, privateOpen });
  if (request.id === "before-prepared") {
    if (request.error || !Array.isArray(request.result)) {
      privateReply(preparation.id, { kind: "error", error: "unavailable" });
    } else {
      prepared = true;
      privateReply(preparation.id, { kind: "application_prepared", manifest: {
        path: "/private/completed-manifest.json", sha256: "4".repeat(64) } });
    }
    return;
  }
  if (request.id === "before-private-ack") {
    if (request.error || !Array.isArray(request.result)) {
      privateReply(selectedId, { kind: "error", error: "unavailable" });
    } else privateReply(selectedId, { kind: "bootstrap_accepted" });
    return;
  }
  if (request.method === "meta/init") {
    if (mode.startsWith("prepare-")) {
      // Separate pipes can arrive in either order. The real Rust startup first
      // reads request 1; this inert owner holds init until that request arrives.
      preparingInit = request;
      startPreparationInit();
      return;
    }
    if (mode === "init-refused") {
      native({ jsonrpc: "2.0", id: request.id, error: {
        code: -32014, message: "fixture selected revision refused", data: { kind: "fixture_conflict", marker: 42 } } });
      return;
    }
    if (mode === "init-unknown") { process.stdout.end(); return; }
    native({ jsonrpc: "2.0", id: request.id, result: { init_state: "accepted", init_id: request.params.init_id,
      stdio_shutdown_handshake: true, stdio_shutdown_horizon_ms: 2000 } });
    native({ jsonrpc: "2.0", method: "meta/init_settled", params: mode === "settled-failed"
      ? { init_id: request.params.init_id, outcome: "failed", code: -32014,
          message: "fixture entered init failed", data: { marker: 43 }, durable_effects: "possible" }
      : { init_id: request.params.init_id, outcome: "ready" } });
  } else if (request.method === "meta/shutdown") {
    native({ jsonrpc: "2.0", id: request.id, ...(mode === "shutdown-failed"
      ? { error: { code: -32000, message: "fixture cleanup failed" } }
      : { result: { shutdown: true, runtime_cleanup_completed: true } }) });
  } else if (request.method) {
    native({ jsonrpc: "2.0", id: request.id, error: { code: -32601, message: "inert fixture" } });
  }
});
nativeLines.on("close", () => {
  log({ kind: "native-eof" });
  if (mode === "term-zero") {
    // A signal handler exits naturally with code 0. The native stop's actual
    // escalation record must still prevent a clean-settlement claim.
    waitingForTerm = setInterval(() => {}, 1000);
    return;
  }
  if (mode === "exit-nonzero") process.exitCode = 7;
  // Native stop owns EOF and process join. Do not wait for Toolkit to close
  // its endpoints after that join, which would create a shutdown cycle.
  privateLines.close(); commands.destroy(); responses.end();
});
