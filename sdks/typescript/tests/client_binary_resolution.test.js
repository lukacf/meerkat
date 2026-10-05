import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { EventEmitter } from "node:events";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";

import { MeerkatClient } from "../dist/client.js";
import { CONTRACT_VERSION } from "../dist/generated/types.js";

describe("MeerkatClient binary resolution", () => {
  it("respects MEERKAT_BIN_PATH when set", async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-bin-"));
    const bin = path.join(root, "rkat-rpc");
    writeFileSync(bin, "#!/usr/bin/env bash\necho ok\n");
    chmodSync(bin, 0o755);

    const previous = process.env.MEERKAT_BIN_PATH;
    process.env.MEERKAT_BIN_PATH = bin;
    try {
      const resolved = await MeerkatClient.resolveBinaryPath("whatever");
      assert.equal(resolved.command, bin);
      assert.equal(resolved.useLegacySubcommand, false);
    } finally {
      if (previous === undefined) {
        delete process.env.MEERKAT_BIN_PATH;
      } else {
        process.env.MEERKAT_BIN_PATH = previous;
      }
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("falls back to downloaded binary when rkat-rpc is not on PATH", async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-fallback-"));
    const downloaded = path.join(root, "rkat-rpc");
    writeFileSync(downloaded, "downloaded");
    chmodSync(downloaded, 0o755);

    const originalCommandPath = MeerkatClient.commandPath;
    const originalEnsureDownloadedBinary = MeerkatClient.ensureDownloadedBinary;
    const originalMeerkatBinPath = process.env.MEERKAT_BIN_PATH;

    process.env.MEERKAT_BIN_PATH = "";
    MeerkatClient.commandPath = () => null;
    MeerkatClient.ensureDownloadedBinary = async () => downloaded;

    try {
      const resolved = await MeerkatClient.resolveBinaryPath("rkat-rpc");
      assert.equal(resolved.command, downloaded);
      assert.equal(resolved.useLegacySubcommand, false);
    } finally {
      delete process.env.MEERKAT_BIN_PATH;
      if (originalMeerkatBinPath === undefined) {
        delete process.env.MEERKAT_BIN_PATH;
      } else {
        process.env.MEERKAT_BIN_PATH = originalMeerkatBinPath;
      }
      MeerkatClient.commandPath = originalCommandPath;
      MeerkatClient.ensureDownloadedBinary = originalEnsureDownloadedBinary;
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("falls back to legacy rkat when default download fails", async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-legacy-"));
    const legacy = path.join(root, "rkat");
    writeFileSync(legacy, "#!/usr/bin/env bash\necho legacy\n");
    chmodSync(legacy, 0o755);

    const originalCommandPath = MeerkatClient.commandPath;
    const originalEnsureDownloadedBinary = MeerkatClient.ensureDownloadedBinary;
    const originalMeerkatBinPath = process.env.MEERKAT_BIN_PATH;

    process.env.MEERKAT_BIN_PATH = "";
    MeerkatClient.commandPath = (command) => {
      if (command === "rkat-rpc") {
        return null;
      }
      if (command === "rkat") {
        return legacy;
      }
      return null;
    };
    MeerkatClient.ensureDownloadedBinary = async () => {
      throw new Error("download failed");
    };

    try {
      const resolved = await MeerkatClient.resolveBinaryPath("rkat-rpc");
      assert.equal(resolved.command, legacy);
      assert.equal(resolved.useLegacySubcommand, true);
    } finally {
      delete process.env.MEERKAT_BIN_PATH;
      if (originalMeerkatBinPath === undefined) {
        delete process.env.MEERKAT_BIN_PATH;
      } else {
        process.env.MEERKAT_BIN_PATH = originalMeerkatBinPath;
      }
      MeerkatClient.commandPath = originalCommandPath;
      MeerkatClient.ensureDownloadedBinary = originalEnsureDownloadedBinary;
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("throws when MEERKAT_BIN_PATH points at a missing executable", async () => {
    const previous = process.env.MEERKAT_BIN_PATH;
    process.env.MEERKAT_BIN_PATH = path.join(os.tmpdir(), "nope-rkat-bin");

    try {
    await assert.rejects(
        () => MeerkatClient.resolveBinaryPath("rkat-rpc"),
        /Binary not found/,
      );
    } finally {
      if (previous === undefined) {
        delete process.env.MEERKAT_BIN_PATH;
      } else {
        process.env.MEERKAT_BIN_PATH = previous;
      }
    }
  });

  it("throws for explicit requested binaries that do not exist", async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-missing-"));
    const missing = path.join(root, "missing");

    try {
    await assert.rejects(
        () => MeerkatClient.resolveBinaryPath(missing),
        /Binary not found/,
      );
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("does not enable live transports by default", () => {
    assert.deepEqual(MeerkatClient.buildArgs(false), []);
  });

  it("enables live websocket transport only when requested", () => {
    assert.deepEqual(
      MeerkatClient.buildArgs(false, { liveWs: true }),
      ["--live-ws", "127.0.0.1:0"],
    );
  });

  it("enables live WebRTC transport only when requested", () => {
    assert.deepEqual(
      MeerkatClient.buildArgs(false, { liveWebrtc: true }),
      ["--live-webrtc"],
    );
  });

  it("passes live tool timeout when requested", () => {
    assert.deepEqual(
      MeerkatClient.buildArgs(false, { liveWebrtc: true, liveToolTimeoutMs: 180000 }),
      ["--live-webrtc", "--live-tool-timeout-ms", "180000"],
    );
  });
});

const RELEASE_BASE = "https://github.com/lukacf/meerkat/releases/download";

describe("MeerkatClient release asset", () => {
  it("matches the published naming", () => {
    // Release assets carry no "v" before the version; the tag does.
    assert.deepEqual(
      MeerkatClient.releaseAsset("0.8.50", "x86_64-unknown-linux-gnu", "tar.gz"),
      {
        asset: "rkat-rpc-0.8.50-x86_64-unknown-linux-gnu.tar.gz",
        url: `${RELEASE_BASE}/v0.8.50/rkat-rpc-0.8.50-x86_64-unknown-linux-gnu.tar.gz`,
      },
    );
  });

  for (const [platform, arch, expected] of [
    ["linux", "x64", "rkat-rpc-0.8.50-x86_64-unknown-linux-gnu.tar.gz"],
    ["linux", "arm64", "rkat-rpc-0.8.50-aarch64-unknown-linux-gnu.tar.gz"],
    ["darwin", "arm64", "rkat-rpc-0.8.50-aarch64-apple-darwin.tar.gz"],
    ["darwin", "x64", "rkat-rpc-0.8.50-x86_64-apple-darwin.tar.gz"],
    ["win32", "x64", "rkat-rpc-0.8.50-x86_64-pc-windows-msvc.zip"],
  ]) {
    it(`maps ${platform}/${arch} to its release asset`, () => {
      const { target, archiveExt } = MeerkatClient.platformTarget(platform, arch);
      assert.deepEqual(MeerkatClient.releaseAsset("0.8.50", target, archiveExt), {
        asset: expected,
        url: `${RELEASE_BASE}/v0.8.50/${expected}`,
      });
    });
  }

  it("maps Intel macOS to the x86_64 darwin asset", () => {
    assert.deepEqual(MeerkatClient.platformTarget("darwin", "x64"), {
      target: "x86_64-apple-darwin",
      archiveExt: "tar.gz",
      binaryName: "rkat-rpc",
    });
  });

  it(
    "exists as a published asset",
    { skip: process.env.MEERKAT_SDK_NETWORK_TESTS !== "1" && "set MEERKAT_SDK_NETWORK_TESTS=1 to check the published asset over the network" },
    async () => {
      const { url } = MeerkatClient.releaseAsset("0.8.50", "x86_64-unknown-linux-gnu", "tar.gz");
      const response = await fetch(url, { method: "HEAD", redirect: "follow" });
      assert.equal(response.status, 200);
    },
  );
});

describe("MeerkatClient callback connection lifetime", () => {
  it("retires pending work before awaiting close so cleanup cannot reject a replacement", async () => {
    const client = new MeerkatClient();
    const original = Object.assign(new EventEmitter(), {
      stdin: { write() {}, end() {}, destroy() {} },
      kill() {}, exitCode: null, signalCode: null,
    });
    client.process = original;
    const originalWork = client.request("old", {});
    const originalClosed = assert.rejects(originalWork, /Client closed/);
    const closing = client.close();
    const replacement = { stdin: { write() {} } };
    client.process = replacement;
    let outcome;
    const replacementWork = client.request("current", {}).then(
      (value) => { outcome = value; },
      (error) => { outcome = error; },
    );
    const replacementId = client.requestId;
    original.emit("close", 0, null);
    await closing;
    await originalClosed;
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(outcome, undefined, "original close rejected replacement work");
    client.handleLine(JSON.stringify({ id: replacementId, result: "current reply" }));
    await replacementWork;
    assert.equal(outcome, "current reply");
    assert.equal(client.process, replacement);
  });

  it("ignores retired reader frames without completing or failing replacement work", async () => {
    const client = new MeerkatClient();
    const original = {};
    const replacement = { kill: () => assert.fail("retired corruption killed replacement") };
    let response;
    let calls = 0;
    client.registerTool("delayed", "Delayed", { type: "object" }, async () => { calls += 1; return "old"; });
    client.process = replacement;
    client.pendingRequests.set(77, {
      resolve: (value) => { response = value; },
      reject: () => assert.fail("retired frame failed replacement work"),
    });
    client.handleLine('{"id":77,"result":"old"}', original);
    assert.equal(response, undefined);
    client.handleLine('{"id":"cb-old","method":"tool/execute","params":{"name":"delayed"}}', original);
    client.handleLine("corrupted old frame", original);
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(calls, 0);
    assert.equal(client.process, replacement);
    client.handleLine('{"id":77,"result":"current"}', replacement);
    assert.equal(response, "current");
  });

  for (const oldOutcome of ["success", "error"]) {
    it(`drops an old callback ${oldOutcome} after reconnect with a reused callback id`, { timeout: 10_000 }, async () => {
      const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-callback-"));
      const bin = path.join(root, "rkat-rpc.cjs");
      writeFileSync(bin, `#!/usr/bin/env node
const rl = require("node:readline").createInterface({ input: process.stdin });
const received = [];
let callbackRequest;
const send = (value) => process.stdout.write(JSON.stringify(value) + "\\n");
rl.on("line", (line) => {
  const msg = JSON.parse(line);
  if (msg.method === "test/callback") {
    callbackRequest = msg.id;
    send({ jsonrpc: "2.0", id: "cb-reused", method: "tool/execute",
      params: { name: "delayed", arguments: msg.params } });
  } else if (msg.method) {
    const result = msg.method === "initialize"
      ? { contract_version: ${JSON.stringify(CONTRACT_VERSION)}, methods: [] }
      : msg.method === "capabilities/get" ? { capabilities: [] }
      : msg.method === "test/received" ? received : { registered: 1 };
    send({ jsonrpc: "2.0", id: msg.id, result });
  } else {
    received.push(msg);
    if (callbackRequest !== undefined) {
      send({ jsonrpc: "2.0", id: callbackRequest, result: msg });
      callbackRequest = undefined;
    }
  }
});
`);
      chmodSync(bin, 0o755);
      const client = new MeerkatClient(bin);
      let releaseOld;
      let releaseCurrent;
      let enteredOld;
      let enteredCurrent;
      const oldGate = new Promise((resolve) => { releaseOld = resolve; });
      const currentGate = new Promise((resolve) => { releaseCurrent = resolve; });
      const oldEntered = new Promise((resolve) => { enteredOld = resolve; });
      const currentEntered = new Promise((resolve) => { enteredCurrent = resolve; });
      const blocks = [{ type: "text", text: "current result" },
        { type: "image", media_type: "image/png", source: "blob", blob_id: "current-blob" }];
      client.registerTool("delayed", "Delayed", { type: "object" }, async ({ origin }) => {
        if (origin === "old") {
          enteredOld();
          await oldGate;
          if (oldOutcome === "error") throw new Error("old failure");
          return "old result";
        }
        if (origin === "current") {
          enteredCurrent();
          await currentGate;
          return blocks;
        }
        if (origin === "control-error") throw new Error("original error control");
        return "original connection works";
      });
      try {
        await client.connect();
        const original = await client.request("test/callback", { origin: "control" });
        assert.equal(original.result.content, "original connection works");
        const originalError = await client.request("test/callback", { origin: "control-error" });
        assert.deepEqual(originalError.result, {
          content: "Tool error: Error: original error control", is_error: true,
        });
        const oldWork = client.request("test/callback", { origin: "old" });
        const oldClosed = assert.rejects(oldWork, /Client closed/);
        await oldEntered;
        await client.close();
        await oldClosed;
        await client.connect();
        const currentWork = client.request("test/callback", { origin: "current" });
        currentWork.catch(() => {});
        await currentEntered;
        releaseOld();
        await new Promise((resolve) => setImmediate(resolve));
        // This request is a pipe-order barrier after the old promise settled.
        assert.deepEqual(await client.request("test/received", {}), [],
          "a retired callback must not answer the replacement process's reused id");
        releaseCurrent();
        const current = await currentWork;
        assert.deepEqual(current.result, { content: blocks, is_error: false });
        assert.equal((await client.request("test/received", {})).length, 1);
      } finally {
        releaseOld();
        releaseCurrent();
        await client.close();
        rmSync(root, { recursive: true, force: true });
      }
    });
  }
});
