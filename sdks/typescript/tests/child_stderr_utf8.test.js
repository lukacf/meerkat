/**
 * A child's stderr reaches an error message with every character intact. A
 * pipe read can end inside a multi-byte character; stderr is decoded as one
 * stream, so a character split across two reads stays whole (decoding each
 * read on its own turned it into U+FFFD). The retained tail of a long stderr
 * never starts inside a surrogate pair either.
 */

import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, it } from "node:test";

import { MeerkatClient } from "../dist/client.js";
import { CONTRACT_VERSION } from "../dist/generated/types.js";

delete process.env.MEERKAT_BIN_PATH;

const TEXT = "fixture: don\u2019t panic \u{1F642}";

/**
 * Every way to cut `TEXT` and its line break into two writes inside one of
 * its multi-byte characters: the 3-byte one after 1 and 2 bytes, the 4-byte
 * one after 1, 2 and 3.
 */
function splitCases() {
  const bytes = Buffer.from(`${TEXT}\n`);
  const cases = [];
  let offset = 0;
  for (const ch of TEXT) {
    const width = Buffer.byteLength(ch);
    for (let cut = 1; cut < width; cut += 1) {
      cases.push({
        name: `U+${ch.codePointAt(0).toString(16).toUpperCase()} split ${cut}|${width - cut}`,
        head: [...bytes.subarray(0, offset + cut)],
        tail: [...bytes.subarray(offset + cut)],
      });
    }
    offset += width;
  }
  return cases;
}

/**
 * Node source that writes `head` to stderr, then `tail` 100 ms later (two
 * pipe reads), then exits with `code`.
 */
function splitStderrSource({ head, tail }, code) {
  return `
process.stderr.write(Buffer.from(${JSON.stringify(head)}), () => {
  setTimeout(() => {
    process.stderr.write(Buffer.from(${JSON.stringify(tail)}), () => process.exit(${code}));
  }, 100);
});`;
}

/**
 * A fake rkat-rpc that answers the handshake, then runs `onRequest` (Node
 * source) on the first request after it, without a reply.
 */
function writeFixture(root, onRequest) {
  const bin = path.join(root, "rkat-rpc.cjs");
  writeFileSync(bin, `#!/usr/bin/env node
const rl = require("node:readline").createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const msg = JSON.parse(line);
  if (msg.method !== "initialize" && msg.method !== "capabilities/get") {
    rl.close();
    ${onRequest}
    return;
  }
  const result = msg.method === "initialize"
    ? { contract_version: ${JSON.stringify(CONTRACT_VERSION)}, methods: [] }
    : { capabilities: [] };
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: msg.id, result }) + "\\n");
});
`);
  chmodSync(bin, 0o755);
  return bin;
}

/** The error a request fails with once the fixture ran `onRequest`. */
async function failedRequest(onRequest) {
  const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-stderr-"));
  const client = new MeerkatClient(writeFixture(root, onRequest));
  try {
    await client.connect();
    return await client.getConfig().then(
      () => assert.fail("the request must fail when the child exits"),
      (error) => error,
    );
  } finally {
    await client.close();
    rmSync(root, { recursive: true, force: true });
  }
}

describe("A child's stderr in an error message", () => {
  it("keeps a character split across two pipe reads whole", { timeout: 60_000 }, async () => {
    const failures = [];
    for (const split of splitCases()) {
      const error = await failedRequest(splitStderrSource(split, 7));
      if (error.code !== "PROCESS_EXITED" || !error.message.endsWith(`: ${TEXT}`)) {
        failures.push(`${split.name}: ${error.code} ${JSON.stringify(error.message)}`);
      }
    }
    assert.deepEqual(failures, []);
  });

  it("keeps a tail that never starts inside a surrogate pair", { timeout: 20_000 }, async () => {
    // 8194 UTF-16 units: the client keeps the last 8192, which would start
    // with the second half of the emoji's surrogate pair.
    const stderr = `a\u{1F642}${"x".repeat(8191)}`;
    const error = await failedRequest(
      `process.stderr.write(${JSON.stringify(stderr)}, () => process.exit(7));`,
    );
    assert.equal(error.code, "PROCESS_EXITED", error.message);
    assert.ok(error.message.isWellFormed(), "the message must not hold a lone surrogate");
    assert.ok(error.message.endsWith(`: ${"x".repeat(8191)}`), error.message.slice(-40));
  });

  it("keeps a split character whole in a failed command's error", { timeout: 60_000 }, async () => {
    const failures = [];
    for (const split of splitCases()) {
      const error = await MeerkatClient.runCommand(process.execPath, [
        "-e",
        splitStderrSource(split, 3),
      ]).then(
        () => assert.fail("the command must fail"),
        (failure) => failure,
      );
      if (error.code !== "ARCHIVE_EXTRACT_FAILED" || !error.message.endsWith(`: ${TEXT}`)) {
        failures.push(`${split.name}: ${error.code} ${JSON.stringify(error.message)}`);
      }
    }
    assert.deepEqual(failures, []);
  });
});
