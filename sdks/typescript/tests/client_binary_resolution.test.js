import assert from "node:assert/strict";
import { describe, it } from "node:test";
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
});
