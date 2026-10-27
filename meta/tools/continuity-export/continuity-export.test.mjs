// Contracts for tools/continuity-export/continuity-export.mjs.
//
// The provider fixtures behave like a host's 0.9.3 provider object:
// resolveMany answers in the snake_case wire shape the 0.9.3 TS builder passed
// to the gateway unchanged, snapshots carry `data: Uint8Array`, and
// resolveRecordBySession answers with a camelCase record.
//
// The "jsonl provider" cases run test_continuity_export.py's JsonlProviderTests
// against jsonl-provider.mjs.
//
// Run: node --test tools/continuity-export/continuity-export.test.mjs
// Rewrite this script's golden file: META_WRITE_CONTINUITY_EXPORT_FIXTURE=1 node --test ...

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { constants as bufferConstants } from "node:buffer";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";
import * as ce from "./continuity-export.mjs";
import * as jp from "./jsonl-provider.mjs";

const SCRIPT = fileURLToPath(new URL("./continuity-export.mjs", import.meta.url));
const FIXTURES = fileURLToPath(
  new URL("../../crates/meerkat-meta/tests/fixtures/continuity_export_v1/", import.meta.url),
);
const WRITE_FIXTURE_ENV = "META_WRITE_CONTINUITY_EXPORT_FIXTURE";

const SESSION_ALPHA = "019ecb20-4899-7292-a2b4-c81bc7a44258";
const SESSION_SMILE = "019f0000-0000-7000-8000-000000000001";
const SESSION_OTHER = "019f0000-0000-7000-8000-000000000002";
const SESSION_RENAMED = "019f0000-0000-7000-8000-000000000003";
const IDENTITY_HALFWIDTH = "agent:\uff61";
const IDENTITY_SMILE = "agent:\u{1f600}";
const DOC_ALPHA = new TextEncoder().encode('{"synthetic":"continuity export golden document alpha"}\n');
const DOC_SMILE = Uint8Array.from({ length: 256 }, (_, index) => index);
const encode = (text) => new TextEncoder().encode(text);

function wireRecord(identity, agentRuntimeId, sessionId, generation, checkpointVersion) {
  return {
    identity,
    agent_runtime_id: agentRuntimeId,
    session_id: sessionId,
    generation,
    checkpoint_version: checkpointVersion,
  };
}

/** An in-memory 0.9.3 SDK continuity provider; the write methods fail the test. */
class Store093 {
  constructor(records = new Map(), snapshots = new Map()) {
    this.records = records;
    this.snapshots = snapshots;
    this.broken = new Map();
    this.calls = [];
  }

  async resolveMany(identities) {
    this.calls.push(["resolveMany", [...identities]]);
    const answer = {};
    for (const identity of identities) {
      const failure = this.broken.get(identity);
      const record = this.records.get(identity);
      if (failure) {
        answer[identity] = {
          state: "broken",
          failure: { identity, kind: failure.kind, record: null, detail: failure.detail },
        };
      } else if (record) {
        answer[identity] = { state: "ready", record };
      } else {
        answer[identity] = { state: "uninitialized" };
      }
    }
    return answer;
  }

  async loadSessionSnapshot(sessionId) {
    this.calls.push(["loadSessionSnapshot", sessionId]);
    const data = this.snapshots.get(sessionId);
    return data === undefined ? null : { data };
  }

  async resolveRecordBySession(sessionId) {
    this.calls.push(["resolveRecordBySession", sessionId]);
    for (const record of this.records.values()) {
      if (record.session_id === sessionId) {
        return {
          record: {
            identity: record.identity,
            agentRuntimeId: record.agent_runtime_id,
            sessionId: record.session_id,
            generation: record.generation,
            checkpointVersion: record.checkpoint_version,
          },
          fencingToken: 7,
          checkpointVersion: record.checkpoint_version,
        };
      }
    }
    return null;
  }

  async saveSessionSnapshot() {
    throw new Error("the export must not write to the provider");
  }

  async upsertContinuityRecord() {
    throw new Error("the export must not write to the provider");
  }

  async deleteContinuityRecord() {
    throw new Error("the export must not write to the provider");
  }
}

/** The store every golden export file is written from (the Python test uses the same). */
function goldenStore() {
  return new Store093(
    new Map([
      ["agent:alpha", wireRecord("agent:alpha", "rt-alpha", SESSION_ALPHA, 2, 7)],
      [IDENTITY_SMILE, wireRecord(IDENTITY_SMILE, "rt-smile", SESSION_SMILE, 0, 1)],
    ]),
    new Map([
      [SESSION_ALPHA, DOC_ALPHA],
      [SESSION_SMILE, DOC_SMILE],
    ]),
  );
}

// Unordered on purpose: the file orders entries by identity code points.
const GOLDEN_IDENTITIES = [IDENTITY_SMILE, "agent:beta", IDENTITY_HALFWIDTH, "agent:alpha"];

function assertGoldenEntries(exported) {
  assert.deepEqual(
    exported.entries.map((entry) => entry.identity),
    ["agent:alpha", "agent:beta", IDENTITY_HALFWIDTH, IDENTITY_SMILE],
  );
  const [alpha, beta, halfwidth, smile] = exported.entries;
  assert.equal(alpha.state, "ready");
  assert.deepEqual(alpha.record, {
    identity: "agent:alpha",
    agentRuntimeId: "rt-alpha",
    sessionId: SESSION_ALPHA,
    generation: 2,
    checkpointVersion: 7,
  });
  assert.deepEqual(documentOf(alpha), described(DOC_ALPHA));
  for (const entry of [beta, halfwidth]) {
    assert.deepEqual(
      [entry.state, entry.record, entry.documentLength, entry.documentSha256],
      ["uninitialized", null, null, null],
    );
  }
  assert.equal(smile.state, "ready");
  assert.equal(smile.record.sessionId, SESSION_SMILE);
  assert.deepEqual(documentOf(smile), described(DOC_SMILE));
}

/** What an entry says about its document: its length and SHA-256. */
function documentOf(entry) {
  return entry.documentLength === null ? null : [entry.documentLength, entry.documentSha256];
}

function described(document) {
  return [document.length, sha256Hex(document)];
}

function sha256Hex(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

async function withTempDir(run) {
  const directory = await mkdtemp(join(tmpdir(), "meta-continuity-export-"));
  try {
    await run(directory);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

async function noPartialFiles(directory) {
  return (await readdir(directory)).every((name) => !name.endsWith(".partial"));
}

async function exists(path) {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

/** The header, and the entry bytes it describes (every entry line with its newline). */
function splitFile(data) {
  const bytes = Buffer.from(data);
  const first = bytes.indexOf(0x0a);
  return [JSON.parse(bytes.subarray(0, first).toString("utf8")), bytes.subarray(first + 1)];
}

function entryLines(data) {
  const [, entries] = splitFile(data);
  const lines = entries.toString("utf8").split("\n");
  assert.equal(lines.pop(), "");
  return lines;
}

/** A file whose header describes exactly the entry bytes `data`, unless overridden. */
function frameBytes(data, headerOverrides = {}) {
  const newlines = data.reduce((count, byte) => count + (byte === 0x0a ? 1 : 0), 0);
  const header = {
    format: ce.CONTINUITY_EXPORT_FORMAT,
    version: ce.CONTINUITY_EXPORT_VERSION,
    writer: "node",
    entries: newlines + (data.length === 0 || data[data.length - 1] === 0x0a ? 0 : 1),
    entries_length: data.length,
    entries_sha256: sha256Hex(data),
    ...headerOverrides,
  };
  return Buffer.concat([Buffer.from(JSON.stringify(header), "utf8"), Buffer.from("\n"), data]);
}

/** A file with these entry lines, each ending in a newline. */
function frame(lines, headerOverrides = {}) {
  const data = Buffer.concat(lines.flatMap((line) => [Buffer.from(line), Buffer.from("\n")]));
  return frameBytes(data, headerOverrides);
}

function readyEntry(identity, sessionId, document) {
  return {
    identity,
    state: "ready",
    record: wireRecord(identity, "rt-1", sessionId, 0, 1),
    document: {
      length: document.length,
      sha256: sha256Hex(document),
      base64: Buffer.from(document).toString("base64"),
    },
  };
}

function baselineEntries() {
  return [readyEntry("agent:a", SESSION_ALPHA, encode("doc-a")), { identity: "agent:b", state: "uninitialized" }];
}

function mutateEntries(change) {
  const entries = baselineEntries();
  change(entries);
  return frame(entries.map((entry) => JSON.stringify(entry)));
}

async function refused(run, directory, target = join(directory, "export.jsonl")) {
  let caught;
  try {
    await run(target);
  } catch (error) {
    caught = error;
  }
  assert.ok(caught instanceof ce.ContinuityExportRefusedError, `expected a refusal, got ${String(caught)}`);
  assert.equal(await exists(target), false);
  assert.ok(await noPartialFiles(directory));
  return caught;
}

function rows(error) {
  return error.refusals.map((row) => `${row.identity}|${row.rowClass}`).sort();
}

function readyProvider({ state, snapshot } = {}) {
  return {
    resolveMany: async () => ({
      "agent:a": state ?? { state: "ready", record: wireRecord("agent:a", "rt", SESSION_ALPHA, 0, 1) },
    }),
    loadSessionSnapshot: async () => (snapshot === undefined ? { data: encode("doc") } : snapshot),
  };
}

// -- Export and read back ------------------------------------------------------

describe("exportContinuity", () => {
  it("round-trips ready and uninitialized identities", async () => {
    await withTempDir(async (directory) => {
      const target = join(directory, "export.jsonl");
      const exported = await ce.exportContinuity(goldenStore(), GOLDEN_IDENTITIES, target);
      assert.equal(exported.path, target);
      assert.equal(exported.version, ce.CONTINUITY_EXPORT_VERSION);
      assert.equal(exported.writer, "node");
      assertGoldenEntries(exported);
      assert.deepEqual(await ce.readContinuityExport(target), exported);
      const [header, entries] = splitFile(await readFile(target));
      assert.deepEqual(header, {
        format: ce.CONTINUITY_EXPORT_FORMAT,
        version: ce.CONTINUITY_EXPORT_VERSION,
        writer: "node",
        entries: 4,
        entries_length: entries.length,
        entries_sha256: sha256Hex(entries),
      });
      assert.equal(exported.entriesSha256, header.entries_sha256);
      // One identity per line, in the file's identity order.
      assert.deepEqual(
        entryLines(await readFile(target)).map((line) => JSON.parse(line).identity),
        exported.entries.map((entry) => entry.identity),
      );
    });
  });

  it("exports and reads back an 8 MiB document", async () => {
    await withTempDir(async (directory) => {
      const document = Uint8Array.from({ length: 8 << 20 }, (_, index) => index % 251);
      const store = new Store093(
        new Map([["agent:a", wireRecord("agent:a", "rt", SESSION_ALPHA, 0, 1)]]),
        new Map([[SESSION_ALPHA, document]]),
      );
      const target = join(directory, "export.jsonl");
      const exported = await ce.exportContinuity(store, ["agent:a"], target);
      assert.deepEqual(documentOf(exported.entries[0]), described(document));
      assert.deepEqual(await ce.readContinuityExport(target), exported);
    });
  });

  it("writes each entry before it loads the next document", async () => {
    // The export streams: an entry is on disk before the next document is
    // asked for, and no document is held after its entry is written.
    await withTempDir(async (directory) => {
      const store = goldenStore();
      const load = store.loadSessionSnapshot.bind(store);
      const writtenBeforeEachLoad = [];
      store.loadSessionSnapshot = async (sessionId) => {
        const pending = (await readdir(directory)).filter((name) => name.endsWith(".entries.partial"));
        const written = [];
        for (const name of pending) {
          const text = await readFile(join(directory, name), "utf8");
          for (const line of text.split("\n").filter(Boolean)) written.push(JSON.parse(line).identity);
        }
        writtenBeforeEachLoad.push(written);
        return load(sessionId);
      };
      await ce.exportContinuity(store, GOLDEN_IDENTITIES, join(directory, "export.jsonl"));
      assert.deepEqual(writtenBeforeEachLoad, [[], ["agent:alpha", "agent:beta", IDENTITY_HALFWIDTH]]);
      assert.ok(await noPartialFiles(directory));
    });
  });

  it("only calls the 0.9.3 read methods", async () => {
    await withTempDir(async (directory) => {
      const store = goldenStore();
      await ce.exportContinuity(store, GOLDEN_IDENTITIES, join(directory, "export.jsonl"));
      assert.deepEqual(
        new Set(store.calls.map(([operation]) => operation)),
        new Set(["resolveMany", "loadSessionSnapshot", "resolveRecordBySession"]),
      );
      assert.deepEqual(
        store.calls.filter(([operation]) => operation === "resolveMany").map(([, args]) => args),
        [GOLDEN_IDENTITIES],
      );
    });
  });

  it("writes byte-identical files for an unchanged store", async () => {
    await withTempDir(async (directory) => {
      const first = join(directory, "one.jsonl");
      const second = join(directory, "two.jsonl");
      await ce.exportContinuity(goldenStore(), GOLDEN_IDENTITIES, first);
      await ce.exportContinuity(goldenStore(), [...GOLDEN_IDENTITIES].reverse(), second);
      assert.deepEqual(await readFile(first), await readFile(second));
    });
  });

  it("reads values through the 0.9.3 transport's JSON path", async () => {
    await withTempDir(async (directory) => {
      class Record093 {
        constructor(identity, agentRuntimeId, sessionId, generation, checkpointVersion) {
          this.identity = identity;
          this.agent_runtime_id = agentRuntimeId;
          this.session_id = sessionId;
          this.generation = generation;
          this.checkpoint_version = checkpointVersion;
          this.ignored = undefined;
        }
      }
      const provider = {
        // Class instances and synchronous answers are what the 0.9.3
        // transport serialized; undefined fields vanished on the wire.
        resolveMany: () => ({
          "agent:alpha": {
            state: "ready",
            record: new Record093("agent:alpha", "rt-alpha", SESSION_ALPHA, 2, 7),
            failure: undefined,
          },
          "agent:beta": { state: "uninitialized" },
        }),
        loadSessionSnapshot: (sessionId) => {
          assert.equal(sessionId, SESSION_ALPHA);
          return { data: Buffer.from(DOC_ALPHA) };
        },
      };
      const exported = await ce.exportContinuity(provider, ["agent:alpha", "agent:beta"], join(directory, "x"));
      const [alpha, beta] = exported.entries;
      assert.deepEqual(documentOf(alpha), described(DOC_ALPHA));
      assert.equal(alpha.record.checkpointVersion, 7);
      assert.equal(beta.state, "uninitialized");
    });
  });

  it("writes a private file and leaves no partial files", async () => {
    await withTempDir(async (directory) => {
      const target = join(directory, "export.jsonl");
      await ce.exportContinuity(goldenStore(), GOLDEN_IDENTITIES, target);
      assert.ok(await noPartialFiles(directory));
      if (process.platform !== "win32") assert.equal((await stat(target)).mode & 0o077, 0);
    });
  });

  it("never overwrites an existing file", async () => {
    await withTempDir(async (directory) => {
      const target = join(directory, "export.jsonl");
      await writeFile(target, "keep me");
      await assert.rejects(
        ce.exportContinuity(goldenStore(), GOLDEN_IDENTITIES, target),
        (error) => error.code === "EEXIST" && /already exists/.test(error.message),
      );
      assert.equal(await readFile(target, "utf8"), "keep me");
      assert.ok(await noPartialFiles(directory));
    });
  });

  it("never replaces a file that appears during the export", async () => {
    await withTempDir(async (directory) => {
      const target = join(directory, "export.jsonl");
      const store = goldenStore();
      const load = store.loadSessionSnapshot.bind(store);
      store.loadSessionSnapshot = async (sessionId) => {
        if (!(await exists(target))) await writeFile(target, "keep me");
        return load(sessionId);
      };
      await assert.rejects(
        ce.exportContinuity(store, GOLDEN_IDENTITIES, target),
        (error) => error.code === "EEXIST" && /appeared while the export ran/.test(error.message),
      );
      assert.equal(await readFile(target, "utf8"), "keep me");
      assert.ok(await noPartialFiles(directory));
    });
  });

  it("publishing fails rather than replace a file", async () => {
    await withTempDir(async (directory) => {
      const partial = join(directory, "export.partial");
      const target = join(directory, "export.jsonl");
      await writeFile(partial, "new");
      await writeFile(target, "keep me");
      await assert.rejects(
        ce.publishExclusive(partial, target),
        (error) => error.code === "EEXIST" && /appeared while the export ran/.test(error.message),
      );
      assert.equal(await readFile(target, "utf8"), "keep me");
      assert.equal(await readFile(partial, "utf8"), "new");
      await rm(target);
      await ce.publishExclusive(partial, target);
      assert.equal(await readFile(target, "utf8"), "new");
      assert.equal(await exists(partial), false);
    });
  });

  it("publishing without hard links still never replaces a file", async () => {
    // FAT and exFAT have no hard links; the export checks and renames there.
    await withTempDir(async (directory) => {
      const partial = join(directory, "export.partial");
      const target = join(directory, "export.jsonl");
      const noLinks = async () => {
        throw Object.assign(new Error("operation not permitted"), { code: "EPERM" });
      };
      await writeFile(partial, "new");
      await writeFile(target, "keep me");
      await assert.rejects(
        ce.publishExclusive(partial, target, noLinks),
        (error) => error.code === "EEXIST" && /appeared while the export ran/.test(error.message),
      );
      assert.equal(await readFile(target, "utf8"), "keep me");
      await rm(target);
      await ce.publishExclusive(partial, target, noLinks);
      assert.equal(await readFile(target, "utf8"), "new");
      assert.equal(await exists(partial), false);
    });
  });

  // Core 0.9.3's SessionId parsed every spelling the uuid crate accepts and
  // gave the provider the canonical form; the file holds that form.
  const spellings = [
    SESSION_ALPHA.toUpperCase(),
    SESSION_ALPHA.replaceAll("-", ""),
    SESSION_ALPHA.replaceAll("-", "").toUpperCase(),
    `{${SESSION_ALPHA.toUpperCase()}}`,
    `urn:uuid:${SESSION_ALPHA.toUpperCase()}`,
  ];
  for (const spelling of spellings) {
    it(`accepts the session id spelling ${spelling}`, async () => {
      await withTempDir(async (directory) => {
        const store = new Store093(
          new Map([["agent:a", wireRecord("agent:a", "rt", spelling, 0, 1)]]),
          new Map([[SESSION_ALPHA, encode("doc")]]),
        );
        store.resolveRecordBySession = undefined;
        const exported = await ce.exportContinuity(store, ["agent:a"], join(directory, "export.jsonl"));
        assert.equal(exported.entries[0].record.sessionId, SESSION_ALPHA);
        assert.deepEqual(documentOf(exported.entries[0]), described(encode("doc")));
        assert.ok(store.calls.some(([name, id]) => name === "loadSessionSnapshot" && id === SESSION_ALPHA));
      });
    });
  }

  it("lets the session index spell the session id differently", async () => {
    await withTempDir(async (directory) => {
      const store = new Store093(
        new Map([["agent:a", wireRecord("agent:a", "rt", SESSION_ALPHA, 0, 1)]]),
        new Map([[SESSION_ALPHA, encode("doc")]]),
      );
      const asked = [];
      store.resolveRecordBySession = async (sessionId) => {
        asked.push(sessionId);
        return { record: { identity: "agent:a", sessionId: `{${SESSION_ALPHA.toUpperCase()}}` }, fencingToken: 7, checkpointVersion: 1 };
      };
      const exported = await ce.exportContinuity(store, ["agent:a"], join(directory, "export.jsonl"));
      assert.equal(exported.entries[0].record.sessionId, SESSION_ALPHA);
      assert.deepEqual(asked, [SESSION_ALPHA]);
    });
  });
});

// -- Identities files: one rule in both scripts -----------------------------------

const IDENTITIES_FIXTURE_LIST = ["agent:alpha", "agent:beta", "agent:gamma", `agent:${String.fromCodePoint(0x1f600)}`, "agent:delta"];
const BOM = String.fromCodePoint(0xfeff);

describe("readIdentitiesFile", () => {
  it("reads the shared fixture", async () => {
    assert.deepEqual(await ce.readIdentitiesFile(join(FIXTURES, "identities_bom_crlf.txt")), IDENTITIES_FIXTURE_LIST);
  });

  it("trims exactly Unicode White_Space", async () => {
    // NEL is White_Space; the information separators are not, though
    // Python's str.strip() and JavaScript's trim() disagree on both.
    await withTempDir(async (directory) => {
      const nel = String.fromCodePoint(0x85);
      const separator = String.fromCodePoint(0x1c);
      const path = join(directory, "identities.txt");
      await writeFile(path, `${nel}agent:a${nel}\nagent:b${separator}\n`);
      assert.deepEqual(await ce.readIdentitiesFile(path), ["agent:a", `agent:b${separator}`]);
    });
  });

  it("refuses a byte order mark after the start", async () => {
    await withTempDir(async (directory) => {
      const path = join(directory, "identities.txt");
      await writeFile(path, Buffer.from([...Buffer.from("agent:a\n"), 0xef, 0xbb, 0xbf, ...Buffer.from("agent:b\n")]));
      await assert.rejects(ce.readIdentitiesFile(path), /line 2 .*U\+FEFF/);
    });
  });

  it("refuses a file that is not UTF-8", async () => {
    await withTempDir(async (directory) => {
      const path = join(directory, "identities.txt");
      await writeFile(path, Buffer.from([...Buffer.from("agent:"), 0xff, 0x0a]));
      await assert.rejects(ce.readIdentitiesFile(path), /not valid UTF-8/);
    });
  });
});

// -- Refusals: the whole export, nothing written, every row listed ---------------

describe("exportContinuity refusals", () => {
  it("refuses every unexportable row and writes nothing", async () => {
    await withTempDir(async (directory) => {
      const store = new Store093(
        new Map([
          ["agent:ok", wireRecord("agent:ok", "rt-ok", SESSION_ALPHA, 0, 1)],
          ["agent:no-snapshot", wireRecord("agent:no-snapshot", "rt-n", SESSION_SMILE, 0, 1)],
          ["agent:shared-1", wireRecord("agent:shared-1", "rt-s1", SESSION_OTHER, 0, 1)],
          ["agent:shared-2", wireRecord("agent:shared-2", "rt-s2", SESSION_OTHER, 0, 1)],
          ["agent:renamed", wireRecord("agent:elsewhere", "rt-r", SESSION_RENAMED, 0, 1)],
        ]),
        new Map([
          [SESSION_ALPHA, encode("ok")],
          [SESSION_OTHER, encode("shared")],
          [SESSION_RENAMED, encode("renamed")],
        ]),
      );
      store.broken.set("agent:broken", { kind: "snapshot_corrupted", detail: "bad checksum" });
      const error = await refused(
        (target) =>
          ce.exportContinuity(
            store,
            ["agent:ok", "agent:no-snapshot", "agent:shared-1", "agent:shared-2", "agent:renamed", "agent:broken"],
            target,
          ),
        directory,
      );
      assert.deepEqual(
        rows(error),
        [
          `agent:broken|${ce.BROKEN}`,
          `agent:no-snapshot|${ce.RUNTIME_MISSING}`,
          `agent:renamed|${ce.IDENTITY_DISAGREES}`,
          `agent:shared-1|${ce.IDENTITY_DISAGREES}`,
          `agent:shared-2|${ce.IDENTITY_DISAGREES}`,
        ],
      );
      const byIdentity = new Map(error.refusals.map((row) => [row.identity, row]));
      assert.match(byIdentity.get("agent:broken").detail, /snapshot_corrupted/);
      assert.match(byIdentity.get("agent:broken").detail, /bad checksum/);
      assert.equal(byIdentity.get("agent:no-snapshot").sessionId, SESSION_SMILE);
      assert.equal(
        byIdentity.get("agent:no-snapshot").detail,
        `the provider holds no snapshot for session ${SESSION_SMILE} ` +
          "(0.9.3 record: agent_runtime_id rt-n, generation 0, checkpoint_version 1)",
      );
      assert.match(byIdentity.get("agent:shared-1").detail, /agent:shared-2/);
      for (const row of error.refusals) assert.equal(row.action, ce.ACTIONS[row.rowClass]);
      assert.ok(error instanceof ce.ContinuityExportError);
    });
  });

  it("refuses an answer that does not match the list", async () => {
    await withTempDir(async (directory) => {
      const error = await refused(
        (target) =>
          ce.exportContinuity(
            {
              resolveMany: async () => ({
                "agent:a": { state: "uninitialized" },
                "agent:unlisted": { state: "uninitialized" },
              }),
              loadSessionSnapshot: async () => {
                throw new Error("no ready record to load");
              },
            },
            ["agent:a", "agent:b"],
            target,
          ),
        directory,
      );
      assert.deepEqual(rows(error), [`agent:b|${ce.UNREADABLE}`, `agent:unlisted|${ce.UNLISTED}`]);
    });
  });

  const record = wireRecord("agent:a", "rt", SESSION_ALPHA, 0, 1);
  const shapeCases = [
    ["unknown state", { state: "warming" }, "unknown state"],
    ["ready without record", { state: "ready" }, "no record"],
    ["uninitialized with record", { state: "uninitialized", record }, "carries"],
    ["negative generation", { state: "ready", record: { ...record, generation: -1 } }, "generation"],
    ["boolean generation", { state: "ready", record: { ...record, generation: true } }, "generation"],
    ["fractional checkpoint", { state: "ready", record: { ...record, checkpoint_version: 1.5 } }, "checkpoint_version"],
    ["bigint checkpoint", { state: "ready", record: { ...record, checkpoint_version: 1n } }, "checkpoint_version"],
    ["uppercase urn prefix", { state: "ready", record: { ...record, session_id: `urn:UUID:${SESSION_ALPHA}` } }, "session_id"],
    ["unclosed brace", { state: "ready", record: { ...record, session_id: `{${SESSION_ALPHA}` } }, "session_id"],
    ["short session id", { state: "ready", record: { ...record, session_id: SESSION_ALPHA.slice(0, -1) } }, "session_id"],
    ["misplaced hyphens", { state: "ready", record: { ...record, session_id: SESSION_ALPHA.replace("-", "") } }, "session_id"],
    ["non-hex session id", { state: "ready", record: { ...record, session_id: SESSION_ALPHA.replace("a", "g") } }, "session_id"],
    ["runtime id with whitespace", { state: "ready", record: { ...record, agent_runtime_id: "rt 1" } }, "agent_runtime_id"],
    ["camelCase record", { state: "ready", record: { identity: "agent:a", agentRuntimeId: "rt", sessionId: SESSION_ALPHA } }, "agent_runtime_id"],
    ["not an object", "ready", "not an object"],
  ];
  for (const [name, state, needle] of shapeCases) {
    it(`refuses values outside the 0.9.3 wire shape: ${name}`, async () => {
      await withTempDir(async (directory) => {
        const error = await refused((target) => ce.exportContinuity(readyProvider({ state }), ["agent:a"], target), directory);
        assert.deepEqual(rows(error), [`agent:a|${ce.UNREADABLE}`]);
        assert.ok(error.refusals[0].detail.includes(needle), error.refusals[0].detail);
      });
    });
  }

  it("refuses a record the 0.9.3 transport would have dropped on the wire", async () => {
    await withTempDir(async (directory) => {
      // The 0.9.3 transport copied own enumerable properties only, so a
      // record whose fields are prototype getters reached the gateway empty.
      class GetterRecord {
        get identity() { return "agent:a"; }
        get agent_runtime_id() { return "rt"; }
        get session_id() { return SESSION_ALPHA; }
        get generation() { return 0; }
        get checkpoint_version() { return 1; }
      }
      const error = await refused(
        (target) => ce.exportContinuity(readyProvider({ state: { state: "ready", record: new GetterRecord() } }), ["agent:a"], target),
        directory,
      );
      assert.deepEqual(rows(error), [`agent:a|${ce.UNREADABLE}`]);
    });
  });

  const snapshotCases = [
    ["base64 string data", { data: "ZG9j" }, "no data"],
    ["empty data", { data: new Uint8Array() }, "empty"],
    ["no data", { nothing: "here" }, "no data"],
    ["a number", 42, "no data"],
  ];
  for (const [name, snapshot, needle] of snapshotCases) {
    it(`refuses unreadable snapshots: ${name}`, async () => {
      await withTempDir(async (directory) => {
        const error = await refused((target) => ce.exportContinuity(readyProvider({ snapshot }), ["agent:a"], target), directory);
        assert.deepEqual(rows(error), [`agent:a|${ce.UNREADABLE}`]);
        assert.equal(error.refusals[0].sessionId, SESSION_ALPHA);
        assert.ok(error.refusals[0].detail.includes(needle), error.refusals[0].detail);
      });
    });
  }

  const indexCases = [
    ["no record", null, ce.IDENTITY_DISAGREES],
    ["another identity", { record: { identity: "agent:other", sessionId: SESSION_ALPHA } }, ce.IDENTITY_DISAGREES],
    ["another session", { record: { identity: "agent:a", sessionId: SESSION_OTHER } }, ce.IDENTITY_DISAGREES],
    ["not a bound record", "not a bound record", ce.UNREADABLE],
  ];
  for (const [name, bound, rowClass] of indexCases) {
    it(`cross-checks the session index when the provider has one: ${name}`, async () => {
      await withTempDir(async (directory) => {
        const store = new Store093(new Map([["agent:a", record]]), new Map([[SESSION_ALPHA, encode("doc")]]));
        store.resolveRecordBySession = async () => bound;
        const error = await refused((target) => ce.exportContinuity(store, ["agent:a"], target), directory);
        assert.deepEqual(rows(error), [`agent:a|${rowClass}`]);
      });
    });
  }

  it("skips the cross-check when the provider has no session index", async () => {
    await withTempDir(async (directory) => {
      const exported = await ce.exportContinuity(readyProvider(), ["agent:a"], join(directory, "export.jsonl"));
      assert.deepEqual(documentOf(exported.entries[0]), described(encode("doc")));
    });
  });

  it("treats two spellings of one session as one shared session", async () => {
    await withTempDir(async (directory) => {
      const store = new Store093(
        new Map([
          ["agent:a", wireRecord("agent:a", "rt-a", SESSION_ALPHA, 0, 1)],
          ["agent:b", wireRecord("agent:b", "rt-b", SESSION_ALPHA.toUpperCase(), 0, 1)],
        ]),
        new Map([[SESSION_ALPHA, encode("doc")]]),
      );
      store.resolveRecordBySession = undefined;
      const error = await refused((target) => ce.exportContinuity(store, ["agent:a", "agent:b"], target), directory);
      assert.deepEqual(rows(error), [`agent:a|${ce.IDENTITY_DISAGREES}`, `agent:b|${ce.IDENTITY_DISAGREES}`]);
      assert.deepEqual(error.refusals.map((row) => row.sessionId), [SESSION_ALPHA, SESSION_ALPHA]);
    });
  });

  it("passes the 0.9.3 callback context to resolveRecordBySession", async () => {
    await withTempDir(async (directory) => {
      const store = goldenStore();
      const seen = [];
      const original = store.resolveRecordBySession.bind(store);
      store.resolveRecordBySession = async (sessionId, context) => {
        seen.push(context);
        return original(sessionId);
      };
      await ce.exportContinuity(store, ["agent:alpha"], join(directory, "export.jsonl"));
      assert.equal(seen[0].signal.aborted, false);
      assert.ok(seen[0].deadlineMs > Date.now());
    });
  });
});

// -- Provider faults abort, are retryable, and are never written down as facts ---

// -- A ready record without a snapshot: refused, or written fresh on request ----

const MISSING_RECORD = {
  identity: "agent:no-snapshot",
  agentRuntimeId: "rt:agent:no-snapshot:1",
  sessionId: SESSION_SMILE,
  generation: 1,
  checkpointVersion: 0,
};

/** What a 0.9.3 host built with an SDK left: records, and snapshots only where saved. */
function missingSnapshotStore() {
  return new Store093(
    new Map([
      ["agent:ok", wireRecord("agent:ok", "rt-ok", SESSION_ALPHA, 0, 1)],
      ["agent:no-snapshot", wireRecord("agent:no-snapshot", "rt:agent:no-snapshot:1", SESSION_SMILE, 1, 0)],
    ]),
    new Map([[SESSION_ALPHA, encode("ok")]]),
  );
}

describe("exportContinuity missing snapshots", () => {
  it("refuses by default, naming the record and the flag", async () => {
    for (const options of [undefined, { missingSnapshot: "refuse" }]) {
      await withTempDir(async (directory) => {
        const error = await refused(
          (target) => ce.exportContinuity(missingSnapshotStore(), ["agent:ok", "agent:no-snapshot"], target, options),
          directory,
        );
        assert.equal(error.refusals.length, 1);
        const [row] = error.refusals;
        assert.deepEqual([row.rowClass, row.identity, row.sessionId], [ce.RUNTIME_MISSING, "agent:no-snapshot", SESSION_SMILE]);
        assert.equal(
          row.detail,
          `the provider holds no snapshot for session ${SESSION_SMILE} (0.9.3 record: ` +
            "agent_runtime_id rt:agent:no-snapshot:1, generation 1, checkpoint_version 0)",
        );
        assert.ok(row.action.includes("--missing-snapshot fresh"), row.action);
      });
    }
  });

  it("fresh writes the identity uninitialized and returns its record", async () => {
    await withTempDir(async (directory) => {
      const store = missingSnapshotStore();
      const target = join(directory, "export.jsonl");
      const exported = await ce.exportContinuity(store, ["agent:ok", "agent:no-snapshot"], target, {
        missingSnapshot: "fresh",
      });
      assert.deepEqual(
        exported.entries.map((entry) => [entry.identity, entry.state]),
        [["agent:no-snapshot", "uninitialized"], ["agent:ok", "ready"]],
      );
      assert.deepEqual(exported.missingSnapshots, [MISSING_RECORD]);
      assert.equal(entryLines(await readFile(target))[0].toString("utf8"), '{"identity":"agent:no-snapshot","state":"uninitialized"}');
      // The file cannot say why an identity is uninitialized; reading it back
      // gives the same export without the list.
      assert.deepEqual(await ce.readContinuityExport(target), { ...exported, missingSnapshots: [] });
      // The abandoned session is never checked against the session index.
      assert.ok(!store.calls.some(([name, id]) => name === "resolveRecordBySession" && id === SESSION_SMILE));
    });
  });

  it("fresh writes the golden file for a record without a snapshot", async () => {
    await withTempDir(async (directory) => {
      // The golden store lists agent:beta without a record; a record with no
      // snapshot must give the same bytes.
      const store = goldenStore();
      store.records.set("agent:beta", wireRecord("agent:beta", "rt-beta", SESSION_OTHER, 2, 0));
      const target = join(directory, "export.jsonl");
      const exported = await ce.exportContinuity(store, GOLDEN_IDENTITIES, target, { missingSnapshot: "fresh" });
      assert.deepEqual(await readFile(target), await readFile(join(FIXTURES, "written_by_node.jsonl")));
      assert.deepEqual(exported.missingSnapshots, [
        { identity: "agent:beta", agentRuntimeId: "rt-beta", sessionId: SESSION_OTHER, generation: 2, checkpointVersion: 0 },
      ]);
      assertGoldenEntries(exported);
    });
  });

  it("fresh leaves every other refusal in place", async () => {
    await withTempDir(async (directory) => {
      const store = missingSnapshotStore();
      store.broken.set("agent:broken", { kind: "snapshot_corrupted", detail: "bad checksum" });
      const error = await refused(
        (target) =>
          ce.exportContinuity(store, ["agent:ok", "agent:no-snapshot", "agent:broken"], target, {
            missingSnapshot: "fresh",
          }),
        directory,
      );
      assert.deepEqual(rows(error), [`agent:broken|${ce.BROKEN}`]);
    });
  });

  it("an unknown policy is refused before the provider is called", async () => {
    const untouchable = {
      resolveMany: async () => assert.fail("the provider must not be called"),
      loadSessionSnapshot: async () => assert.fail("the provider must not be called"),
    };
    for (const missingSnapshot of ["skip", "Fresh", "", null, 1]) {
      await withTempDir(async (directory) => {
        const target = join(directory, "export.jsonl");
        await assert.rejects(ce.exportContinuity(untouchable, ["agent:a"], target, { missingSnapshot }), RangeError);
        assert.equal(await exists(target), false);
      });
    }
  });
});

describe("exportContinuity provider faults", () => {
  for (const [failing, subject] of [
    ["resolveMany", null],
    ["loadSessionSnapshot", SESSION_ALPHA],
    ["resolveRecordBySession", SESSION_ALPHA],
  ]) {
    it(`aborts on a ${failing} fault and writes nothing`, async () => {
      await withTempDir(async (directory) => {
        const store = goldenStore();
        const cause = new Error("store unreachable");
        store[failing] = async () => {
          throw cause;
        };
        const target = join(directory, "export.jsonl");
        let caught;
        try {
          await ce.exportContinuity(store, ["agent:alpha"], target);
        } catch (error) {
          caught = error;
        }
        assert.ok(caught instanceof ce.ContinuityExportProviderError);
        assert.deepEqual([caught.operation, caught.subject, caught.retryable], [failing, subject, true]);
        assert.equal(caught.cause, cause);
        assert.equal(await exists(target), false);
        assert.ok(await noPartialFiles(directory));
      });
    });
  }

  // Reading what a read method returned runs provider code too: a getter or a
  // toJSON method.
  for (const [failing, subject, change] of [
    [
      "resolveMany",
      null,
      (answer, cause) =>
        Object.defineProperty({ ...answer }, "agent:alpha", {
          enumerable: true,
          get() {
            throw cause;
          },
        }),
    ],
    [
      "loadSessionSnapshot",
      SESSION_ALPHA,
      (_snapshot, cause) => ({
        get data() {
          throw cause;
        },
      }),
    ],
    [
      "resolveRecordBySession",
      SESSION_ALPHA,
      (bound, cause) => ({
        ...bound,
        get record() {
          throw cause;
        },
      }),
    ],
  ]) {
    it(`aborts when the value ${failing} returned fails to read, and writes nothing`, async () => {
      await withTempDir(async (directory) => {
        const store = goldenStore();
        const cause = new Error("store unreachable");
        const read = store[failing].bind(store);
        store[failing] = async (...args) => change(await read(...args), cause);
        const target = join(directory, "export.jsonl");
        let caught;
        try {
          await ce.exportContinuity(store, ["agent:alpha"], target);
        } catch (error) {
          caught = error;
        }
        assert.ok(caught instanceof ce.ContinuityExportProviderError, String(caught));
        assert.deepEqual([caught.operation, caught.subject, caught.retryable], [failing, subject, true]);
        assert.equal(caught.cause, cause);
        assert.equal(await exists(target), false);
        assert.ok(await noPartialFiles(directory));
      });
    });
  }
});

// -- Caller errors ---------------------------------------------------------------

describe("exportContinuity caller errors", () => {
  for (const [name, identities, errorType] of [
    ["empty list", [], RangeError],
    ["duplicate", ["agent:a", "agent:a"], RangeError],
    ["one string", "agent:a", TypeError],
    ["non-string", [1], TypeError],
    ["empty identity", [""], RangeError],
    ["space", ["agent a"], RangeError],
    ["ideographic space", ["agent\u3000a"], RangeError],
    ["slash", ["team/agent"], RangeError],
    ["lone surrogate", ["agent:\ud800"], RangeError],
  ]) {
    it(`refuses a malformed identity list before calling the provider: ${name}`, async () => {
      await withTempDir(async (directory) => {
        const store = goldenStore();
        await assert.rejects(ce.exportContinuity(store, identities, join(directory, "export.jsonl")), errorType);
        assert.deepEqual(store.calls, []);
      });
    });
  }

  it("requires the 0.9.3 read methods", async () => {
    await withTempDir(async (directory) => {
      await assert.rejects(
        ce.exportContinuity({ resolveMany: async () => ({}) }, ["agent:a"], join(directory, "export.jsonl")),
        (error) => error instanceof TypeError && error.message.includes("loadSessionSnapshot"),
      );
    });
  });
});

// -- The reader refuses anything but an intact version 1 file ---------------------

const GOOD_LINES = baselineEntries().map((entry) => JSON.stringify(entry));
const GOOD_ENTRIES = Buffer.from(GOOD_LINES.map((line) => `${line}\n`).join(""), "utf8");
const PRETTY_LINE = JSON.stringify(baselineEntries()[0], null, 1).replace(/\n/g, "");

function withoutHeaderKey(key) {
  const data = frame(GOOD_LINES);
  const newline = data.indexOf(0x0a);
  const header = JSON.parse(data.subarray(0, newline).toString("utf8"));
  delete header[key];
  return Buffer.concat([Buffer.from(JSON.stringify(header), "utf8"), data.subarray(newline)]);
}

function replaced(data, from, to) {
  const text = Buffer.from(data).toString("latin1");
  assert.ok(text.includes(from), from);
  return Buffer.from(text.replace(from, to), "latin1");
}

async function readFrom(directory, data, options) {
  const target = join(directory, "export.jsonl");
  await writeFile(target, data);
  return ce.readContinuityExport(target, options);
}

async function refusedFile(directory, data, options) {
  let caught;
  try {
    await readFrom(directory, data, options);
  } catch (error) {
    caught = error;
  }
  assert.ok(caught instanceof ce.ContinuityExportFileError, `expected a file error, got ${String(caught)}`);
  assert.equal(caught.path, join(directory, "export.jsonl"));
  return caught.reason;
}

describe("readContinuityExport", () => {
  const badUtf8 = Buffer.concat([
    Buffer.from(GOOD_LINES[1].slice(0, GOOD_LINES[1].indexOf("agent:b") + 6), "utf8"),
    Buffer.from([0xff]),
    Buffer.from(GOOD_LINES[1].slice(GOOD_LINES[1].indexOf("agent:b") + 7), "utf8"),
  ]);
  const refusals = [
    ["empty file", Buffer.alloc(0), "header"],
    ["header without a newline", frame(GOOD_LINES).subarray(0, frame(GOOD_LINES).indexOf(0x0a)), "header"],
    ["header lists no entries", frame([]), "no entries"],
    ["no trailing newline", frameBytes(GOOD_ENTRIES.subarray(0, -1)), "newline"],
    ["truncated", frame(GOOD_LINES).subarray(0, -1), "entries_length"],
    ["extra line", Buffer.concat([frame(GOOD_LINES), Buffer.from("{}\n")]), "entries_length"],
    ["count mismatch", frame(GOOD_LINES, { entries: 3 }), "entry lines"],
    ["other format", frame(GOOD_LINES, { format: "something-else" }), "format"],
    ["newer version", frame(GOOD_LINES, { version: 2 }), "unsupported version 2"],
    ["extra header key", frame(GOOD_LINES, { note: "x" }), "header keys"],
    ["missing header key", withoutHeaderKey("writer"), "header keys"],
    ["empty writer", frame(GOOD_LINES, { writer: "" }), "writer"],
    ["negative length", frame(GOOD_LINES, { entries_length: -1 }), "entries_length"],
    ["uppercase digest", frame(GOOD_LINES, { entries_sha256: sha256Hex(GOOD_ENTRIES).toUpperCase() }), "entries_sha256"],
    ["digest mismatch", replaced(frame(GOOD_LINES), '"agent:a"', '"agent:c"'), "entries_sha256"],
    ["header not compact", replaced(frame(GOOD_LINES), '"version":1,', '"version": 1,'), "canonical"],
    ["entry not compact", frame([PRETTY_LINE, GOOD_LINES[1]]), "canonical"],
    ["CRLF line ends", frame(GOOD_LINES.map((line) => `${line}\r`)), "canonical"],
    ["duplicate key", frame([GOOD_LINES[0].replace('{"identity":"agent:a",', '{"identity":"agent:a","identity":"agent:a",'), GOOD_LINES[1]]), "canonical"],
    ["invalid utf-8", frame([GOOD_LINES[0], badUtf8]), "UTF-8"],
    ["blank line", frame([GOOD_LINES[0], "", GOOD_LINES[1]]), "JSON"],
    ["entry not an object", frame(["[]", GOOD_LINES[1]]), "not an object"],
    ["unsorted", mutateEntries((entries) => entries.reverse()), "order"],
    ["repeated identity", mutateEntries((entries) => entries.push({ ...entries[1] })), "order"],
    ["unknown state", mutateEntries((entries) => (entries[1].state = "warming")), "state"],
    ["uninitialized extra key", mutateEntries((entries) => (entries[1].record = null)), "keys"],
    ["ready without document", mutateEntries((entries) => delete entries[0].document), "keys"],
    ["record names another identity", mutateEntries((entries) => (entries[0].record.identity = "agent:z")), "record"],
    ["shared session", mutateEntries((entries) => (entries[1] = readyEntry("agent:b", SESSION_ALPHA, encode("doc-b")))), "session"],
    ["uppercase session id", mutateEntries((entries) => (entries[0].record.session_id = SESSION_ALPHA.toUpperCase())), "session_id"],
    ["negative generation", mutateEntries((entries) => (entries[0].record.generation = -1)), "generation"],
    ["boolean generation", mutateEntries((entries) => (entries[0].record.generation = true)), "generation"],
    ["beyond exact integers", mutateEntries((entries) => (entries[0].record.checkpoint_version = 2 ** 64)), "checkpoint_version"],
    ["identity with whitespace", mutateEntries((entries) => (entries[1].identity = "agent:b c")), "identity"],
    ["identity with slash", mutateEntries((entries) => (entries[1].identity = "agent:b/c")), "identity"],
    ["document sha mismatch", mutateEntries((entries) => (entries[0].document.sha256 = "0".repeat(64))), "document"],
    ["document length mismatch", mutateEntries((entries) => (entries[0].document.length = 4)), "document"],
    ["non-canonical base64", mutateEntries((entries) => (entries[0].document.base64 = "ZG9jLWF=")), "base64"],
    ["base64 alphabet", mutateEntries((entries) => (entries[0].document.base64 = "ZG9j*WE=")), "base64"],
  ];
  for (const [name, data, needle] of refusals) {
    it(`refuses anything but an intact v1 file: ${name}`, async () => {
      await withTempDir(async (directory) => {
        const reason = await refusedFile(directory, data);
        assert.ok(reason.includes(needle), reason);
      });
    });
  }

  it("accepts the baseline entries", async () => {
    await withTempDir(async (directory) => {
      const exported = await readFrom(directory, frame(GOOD_LINES));
      assert.deepEqual(exported.entries.map((entry) => entry.identity), ["agent:a", "agent:b"]);
      assert.deepEqual(documentOf(exported.entries[0]), described(encode("doc-a")));
      assert.equal(exported.entriesSha256, sha256Hex(GOOD_ENTRIES));
    });
  });

  it("says why it cannot check an integer above 2^53 - 1, which the format allows", async () => {
    // A Python-written file may hold any unsigned 64-bit integer; this
    // script cannot hold one above 2^53 - 1 exactly, and says so.
    const line = GOOD_LINES[0].replace('"generation":0', '"generation":1152921504606846976');
    await withTempDir(async (directory) => {
      const reason = await refusedFile(directory, frame([line, GOOD_LINES[1]]));
      assert.match(reason, /entry 0 field record\.generation is above 2\^53 - 1/);
      assert.match(reason, /verify the file with the Python script or the migration verb/);
      assert.doesNotMatch(reason, /canonical/);
    });
  });

  it("reports a damaged file's digest before its content", async () => {
    // A changed line fails its own checks too, but the header no longer
    // describes the file, and that is the fault to report.
    await withTempDir(async (directory) => {
      const damaged = replaced(frame(GOOD_LINES), GOOD_LINES[0], PRETTY_LINE);
      const longer = await refusedFile(directory, damaged);
      assert.ok(longer.includes("entries_length"), longer);
      assert.ok(!longer.includes("canonical"), longer);
      const sameLength = await refusedFile(directory, replaced(frame(GOOD_LINES), '"state":"ready"', '"state":"reaDy"'));
      assert.ok(sameLength.includes("entries_sha256"), sameLength);
      assert.ok(!sameLength.includes("state"), sameLength);
    });
  });

  it("refuses an entry line longer than the limit", async () => {
    const longest = Math.max(...GOOD_LINES.map((line) => Buffer.byteLength(line)));
    await withTempDir(async (directory) => {
      await readFrom(directory, frame(GOOD_LINES), { maxEntryLineBytes: longest });
      const reason = await refusedFile(directory, frame(GOOD_LINES), { maxEntryLineBytes: longest - 1 });
      assert.ok(reason.includes(`entry 0 is longer than ${longest - 1} bytes`), reason);
    });
  });

  it("limits each line, not the file", async () => {
    const lines = Array.from({ length: 8 }, (_, index) =>
      JSON.stringify(readyEntry(`agent:${index}`, `019f0000-0000-7000-8000-${String(index).padStart(12, "0")}`, Buffer.alloc(600, 120))),
    );
    const limit = Math.max(...lines.map((line) => Buffer.byteLength(line)));
    const data = frame(lines);
    assert.ok(data.length > 4 * limit);
    await withTempDir(async (directory) => {
      const exported = await readFrom(directory, data, { maxEntryLineBytes: limit });
      assert.equal(exported.entries.length, 8);
    });
  });

  it("refuses a line nested too deeply to check", async () => {
    const deep = `${"[".repeat(200_000)}${"]".repeat(200_000)}`;
    await withTempDir(async (directory) => {
      const reason = await refusedFile(directory, frame([deep]));
      assert.ok(reason.includes("entry 0 is nested too deeply to check"), reason);
    });
  });

  it("refuses a header line longer than its limit", async () => {
    await withTempDir(async (directory) => {
      const reason = await refusedFile(directory, frame(GOOD_LINES, { writer: "w".repeat(70_000) }));
      assert.ok(reason.includes("header line"), reason);
    });
  });
});

describe("exportContinuity entry limit", () => {
  // One entry line holds at most MAX_ENTRY_LINE_BYTES, so that this script
  // can hold any entry as one string. Both scripts refuse a document whose
  // entry would be longer, and say so up front.
  function store() {
    return new Store093(
      new Map([
        ["agent:a", wireRecord("agent:a", "rt", SESSION_ALPHA, 0, 1)],
        ["agent:b", wireRecord("agent:b", "rt", SESSION_OTHER, 0, 1)],
      ]),
      new Map([
        [SESSION_ALPHA, Uint8Array.from({ length: 768 }, (_, index) => index % 256)],
        [SESSION_OTHER, encode("doc")],
      ]),
    );
  }

  it("refuses a document whose entry would be over the limit", async () => {
    await withTempDir(async (directory) => {
      const fits = join(directory, "fits.jsonl");
      await ce.exportContinuity(store(), ["agent:a", "agent:b"], fits);
      const longest = Buffer.byteLength(entryLines(await readFile(fits))[0]);
      await ce.exportContinuity(store(), ["agent:a", "agent:b"], join(directory, "exactly.jsonl"), {
        maxEntryLineBytes: longest,
      });
      const error = await refused(
        (target) => ce.exportContinuity(store(), ["agent:a", "agent:b"], target, { maxEntryLineBytes: longest - 1 }),
        directory,
      );
      assert.deepEqual(
        error.refusals.map((row) => [row.identity, row.rowClass, row.sessionId]),
        [["agent:a", ce.TOO_LARGE, SESSION_ALPHA]],
      );
      assert.ok(error.refusals[0].detail.includes(`${longest} bytes`), error.refusals[0].detail);
      assert.ok(error.refusals[0].detail.includes(`${longest - 1}`), error.refusals[0].detail);
      assert.equal(error.refusals[0].action, ce.ACTIONS[ce.TOO_LARGE]);
    });
  });

  it("is the longest string Node.js can hold", () => {
    assert.equal(ce.MAX_ENTRY_LINE_BYTES, 536_870_888);
    assert.equal(ce.MAX_ENTRY_LINE_BYTES, bufferConstants.MAX_STRING_LENGTH);
  });
});

// -- Golden files shared with the Python script and the migration verb -----------

describe("continuity export golden files", () => {
  it("the file written by this script is byte-stable", async () => {
    await withTempDir(async (directory) => {
      const target = join(directory, "export.jsonl");
      const exported = await ce.exportContinuity(goldenStore(), GOLDEN_IDENTITIES, target);
      const golden = join(FIXTURES, "written_by_node.jsonl");
      if (process.env[WRITE_FIXTURE_ENV] === "1") await writeFile(golden, await readFile(target));
      assert.deepEqual(await readFile(target), await readFile(golden));
      assertGoldenEntries(exported);
    });
  });

  it("reads the file written by the Python script", async () => {
    const exported = await ce.readContinuityExport(join(FIXTURES, "written_by_python.jsonl"));
    assert.equal(exported.writer, "python");
    assertGoldenEntries(exported);
  });

  it("both writers write the same entries for the same store", async () => {
    const [pythonHeader, pythonEntries] = splitFile(await readFile(join(FIXTURES, "written_by_python.jsonl")));
    const [nodeHeader, nodeEntries] = splitFile(await readFile(join(FIXTURES, "written_by_node.jsonl")));
    assert.deepEqual(pythonEntries, nodeEntries);
    assert.deepEqual({ ...pythonHeader, writer: "node" }, nodeHeader);
  });
});

// -- The export guide ----------------------------------------------------------------

const GUIDE = fileURLToPath(new URL("../../docs/guides/continuity-export.mdx", import.meta.url));

/** Prose compared case-insensitively, without a final period. */
function guideText(value) {
  return value.split(/\s+/).filter(Boolean).join(" ").replace(/\.$/, "").toLowerCase();
}

describe("the export guide", () => {
  // The guide copies the scripts' exit codes and refusal actions into its
  // tables; the Python suite checks its own script against the same rows.
  it("documents every exit code and every refusal action", async () => {
    const guide = await readFile(GUIDE, "utf8");
    const actions = new Map([...guide.matchAll(/^\| `(\w+)` \| .* \| (.*) \|$/gm)].map((match) => [match[1], match[2]]));
    assert.deepEqual([...actions.keys()].sort(), Object.keys(ce.ACTIONS).sort());
    for (const [rowClass, action] of Object.entries(ce.ACTIONS)) {
      assert.equal(guideText(actions.get(rowClass)), guideText(action), rowClass);
    }
    const codes = [...guide.matchAll(/^\| ([0-9]+) \| .* \|$/gm)].map((match) => Number(match[1]));
    assert.deepEqual(codes.sort(), [0, ce.EXIT_REFUSED, ce.EXIT_USAGE, ce.EXIT_PROVIDER, ce.EXIT_INVALID_FILE].sort());
  });
});

// -- Command line ----------------------------------------------------------------

const PROVIDER_MODULE = `
import { writeFileSync } from "node:fs";
class Store {
  constructor({ broken = false, failing = false } = {}) {
    this.broken = broken;
    this.failing = failing;
  }
  async resolveMany(identities) {
    if (this.failing) throw new Error("store unreachable");
    const answer = {};
    for (const identity of identities) {
      if (identity === "agent:a" && this.broken) {
        answer[identity] = { state: "broken", failure: { identity, kind: "store_unavailable", record: null, detail: "offline" } };
      } else if (identity === "agent:a") {
        answer[identity] = { state: "ready", record: { identity, agent_runtime_id: "rt-a", session_id: "${SESSION_ALPHA}", generation: 0, checkpoint_version: 1 } };
      } else {
        answer[identity] = { state: "uninitialized" };
      }
    }
    return answer;
  }
  async loadSessionSnapshot() {
    return { data: new TextEncoder().encode("document") };
  }
}
export default async () => new Store();
export const brokenStore = new Store({ broken: true });
export const failingStore = new Store({ failing: true });
export function failingFactory() {
  throw new Error("database connection refused");
}
// A snapshot whose bytes cannot be read once the export is under way.
export const unreadableSnapshotStore = Object.assign(new Store(), {
  async loadSessionSnapshot() {
    return {
      get data() {
        throw new Error("snapshot storage went away");
      },
    };
  },
});
// Another writer creates the output while the export reads.
export const racingStore = Object.assign(new Store(), {
  async loadSessionSnapshot() {
    writeFileSync("out.jsonl", "keep me", { flag: "wx" });
    return { data: new TextEncoder().encode("document") };
  },
});
// What a 0.9.3 host built with an SDK left: a record, and no snapshot.
export const noSnapshotStore = {
  async resolveMany(identities) {
    const answer = {};
    for (const identity of identities) {
      answer[identity] =
        identity === "agent:a"
          ? { state: "ready", record: { identity, agent_runtime_id: "rt:agent:a:1", session_id: "${SESSION_ALPHA}", generation: 1, checkpoint_version: 0 } }
          : { state: "uninitialized" };
    }
    return answer;
  },
  async loadSessionSnapshot() {
    return null;
  },
};
`;

// What both scripts print for noSnapshotStore (the Python suite pins the same
// text): the refusal, and the summary of an export with --missing-snapshot fresh.
const NO_SNAPSHOT_REFUSAL =
  "continuity export refused; nothing was written:\n" +
  `  class=RuntimeMissing identity=agent:a session=${SESSION_ALPHA}\n` +
  `    detail: the provider holds no snapshot for session ${SESSION_ALPHA} ` +
  "(0.9.3 record: agent_runtime_id rt:agent:a:1, generation 1, checkpoint_version 0)\n" +
  "    action: if a 0.9.3 host built with the Python or TypeScript SDK wrote the store, it never " +
  "saved snapshots: rerun the export with --missing-snapshot fresh, which writes the identity as " +
  "uninitialized, so it starts fresh with an empty transcript, as at every 0.9.3 restart; " +
  "otherwise restore the session's snapshot, or delete the identity with the 0.9.3 binary, " +
  "then rerun the export\n";
const noSnapshotFreshSummary = (writer) =>
  `out.jsonl: meerkat-meta.continuity-export version 1, writer ${writer}, entries sha256 ` +
  "ec47968e29095ea3744ceb047d4eba4adae99445d7d7182f615e54223ffe2d52\n" +
  "  0 ready, 2 uninitialized\n" +
  "  uninitialized  agent:a\n" +
  "  uninitialized  agent:b\n" +
  "  1 written fresh by --missing-snapshot fresh (uninitialized, with an empty transcript):\n" +
  `  fresh          agent:a  session ${SESSION_ALPHA}  ` +
  "(0.9.3 record: agent_runtime_id rt:agent:a:1, generation 1, checkpoint_version 0): " +
  "transcript not carried, because 0.9.3 never saved a snapshot of this session\n";

function runScript(directory, ...args) {
  // The script prints no color. Node.js 18 warns on stderr when FORCE_COLOR
  // (which its test runner may set) meets NO_COLOR, and the tests read stderr.
  const env = { ...process.env };
  delete env.FORCE_COLOR;
  delete env.NO_COLOR;
  return spawnSync(process.execPath, [SCRIPT, ...args], { cwd: directory, encoding: "utf8", timeout: 60_000, env });
}

describe("continuity export command line", () => {
  const refusedModules = [
    ["an unknown file extension", "provider.data", (path) => writeFile(path, "export default {};\n")],
    ["a directory", "provider-directory", (path) => mkdir(path)],
    ["a JSON module", "provider.json", (path) => writeFile(path, "{}\n")],
  ];
  for (const [name, file, create] of refusedModules) {
    it(`a provider module the loader refuses (${name}) is a usage error`, async () => {
      await withTempDir(async (directory) => {
        await create(join(directory, file));
        const result = runScript(directory, "--provider", `./${file}`, "--identity", "agent:a", "--output", "out.jsonl");
        assert.equal(result.status, ce.EXIT_USAGE, result.stderr);
        assert.match(result.stderr, /continuity export not started: cannot load/);
        assert.equal(await exists(join(directory, "out.jsonl")), false);
      });
    });
  }

  it("exports through a provider factory and verifies the file", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "host-provider.mjs"), PROVIDER_MODULE);
      await writeFile(join(directory, "ids.txt"), "agent:a\n\nagent:b\n");
      const result = runScript(
        directory,
        "--provider", "./host-provider.mjs",
        "--identities-file", "ids.txt",
        "--identity", "agent:c",
        "--output", "out.jsonl",
      );
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /1 ready, 2 uninitialized/);
      const exported = await ce.readContinuityExport(join(directory, "out.jsonl"));
      assert.deepEqual(exported.entries.map((entry) => entry.state), ["ready", "uninitialized", "uninitialized"]);
      const verified = runScript(directory, "--verify", "out.jsonl", join(FIXTURES, "written_by_python.jsonl"));
      assert.equal(verified.status, 0, verified.stderr);
      assert.match(verified.stdout, /writer python/);
    });
  });

  it("a refusal lists each row with its class and action", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "host-provider.mjs"), PROVIDER_MODULE);
      const result = runScript(directory, "--provider", "./host-provider.mjs#brokenStore", "--identity", "agent:a", "--output", "out.jsonl");
      assert.equal(result.status, ce.EXIT_REFUSED);
      assert.match(result.stderr, /class=Broken identity=agent:a/);
      assert.ok(result.stderr.includes(ce.ACTIONS[ce.BROKEN]));
      assert.equal(await exists(join(directory, "out.jsonl")), false);
    });
  });

  it("a record without a snapshot is refused, naming the flag", async () => {
    for (const extra of [[], ["--missing-snapshot", "refuse"]]) {
      await withTempDir(async (directory) => {
        await writeFile(join(directory, "host-provider.mjs"), PROVIDER_MODULE);
        const result = runScript(
          directory,
          "--provider", "./host-provider.mjs#noSnapshotStore",
          "--identity", "agent:a",
          "--identity", "agent:b",
          "--output", "out.jsonl",
          ...extra,
        );
        assert.equal(result.status, ce.EXIT_REFUSED);
        assert.equal(result.stdout, "");
        assert.equal(result.stderr, NO_SNAPSHOT_REFUSAL);
        assert.equal(await exists(join(directory, "out.jsonl")), false);
      });
    }
  });

  it("--missing-snapshot fresh writes it uninitialized and lists its record", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "host-provider.mjs"), PROVIDER_MODULE);
      const result = runScript(
        directory,
        "--provider", "./host-provider.mjs#noSnapshotStore",
        "--identity", "agent:a",
        "--identity", "agent:b",
        "--output", "out.jsonl",
        "--missing-snapshot", "fresh",
      );
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, noSnapshotFreshSummary("node"));
      assert.equal(result.stderr, "");
      const exported = await ce.readContinuityExport(join(directory, "out.jsonl"));
      assert.deepEqual(
        exported.entries.map((entry) => [entry.identity, entry.state]),
        [["agent:a", "uninitialized"], ["agent:b", "uninitialized"]],
      );
    });
  });

  it("--missing-snapshot takes refuse or fresh, and only with an export", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "host-provider.mjs"), PROVIDER_MODULE);
      const unknown = runScript(
        directory,
        "--provider", "./host-provider.mjs#noSnapshotStore",
        "--identity", "agent:a",
        "--output", "out.jsonl",
        "--missing-snapshot", "skip",
      );
      assert.equal(unknown.status, ce.EXIT_USAGE);
      assert.match(unknown.stderr, /--missing-snapshot/);
      const withVerify = runScript(directory, "--verify", join(FIXTURES, "written_by_node.jsonl"), "--missing-snapshot", "fresh");
      assert.equal(withVerify.status, ce.EXIT_USAGE);
      assert.match(withVerify.stderr, /--verify takes only files/);
      assert.equal(await exists(join(directory, "out.jsonl")), false);
    });
  });

  it("a provider fault exits retryable", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "host-provider.mjs"), PROVIDER_MODULE);
      const result = runScript(directory, "--provider", "./host-provider.mjs#failingStore", "--identity", "agent:a", "--output", "out.jsonl");
      assert.equal(result.status, ce.EXIT_PROVIDER);
      assert.match(result.stderr, /run the export again/);
      assert.equal(await exists(join(directory, "out.jsonl")), false);
    });
  });

  it("an existing output is a usage error", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "host-provider.mjs"), PROVIDER_MODULE);
      await writeFile(join(directory, "out.jsonl"), "keep");
      const result = runScript(directory, "--provider", "./host-provider.mjs", "--identity", "agent:a", "--output", "out.jsonl");
      assert.equal(result.status, ce.EXIT_USAGE);
      assert.match(result.stderr, /continuity export not started: .*already exists/);
      assert.equal(await readFile(join(directory, "out.jsonl"), "utf8"), "keep");
    });
  });

  it("a byte order mark does not hide the first identity", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "host-provider.mjs"), PROVIDER_MODULE);
      await writeFile(join(directory, "ids.txt"), Buffer.from([0xef, 0xbb, 0xbf, ...Buffer.from("agent:a\r\nagent:b\r\n")]));
      const result = runScript(directory, "--provider", "./host-provider.mjs", "--identities-file", "ids.txt", "--output", "out.jsonl");
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /1 ready, 1 uninitialized/);
    });
  });

  it("a byte order mark in an identity argument is a usage error", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "host-provider.mjs"), PROVIDER_MODULE);
      const result = runScript(directory, "--provider", "./host-provider.mjs", "--identity", `${BOM}agent:a`, "--output", "out.jsonl");
      assert.equal(result.status, ce.EXIT_USAGE, result.stderr);
      assert.match(result.stderr, /U\+FEFF/);
      assert.equal(await exists(join(directory, "out.jsonl")), false);
    });
  });

  it("a provider factory that throws exits retryable", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "host-provider.mjs"), PROVIDER_MODULE);
      const result = runScript(directory, "--provider", "./host-provider.mjs#failingFactory", "--identity", "agent:a", "--output", "out.jsonl");
      assert.equal(result.status, ce.EXIT_PROVIDER, result.stderr);
      assert.match(result.stderr, /database connection refused/);
      assert.match(result.stderr, /run the export again/);
      assert.equal(await exists(join(directory, "out.jsonl")), false);
    });
  });

  it("a provider module that throws on import exits retryable", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "exploding-provider.mjs"), 'throw new Error("database connection refused");\n');
      const result = runScript(directory, "--provider", "./exploding-provider.mjs", "--identity", "agent:a", "--output", "out.jsonl");
      assert.equal(result.status, ce.EXIT_PROVIDER, result.stderr);
      assert.match(result.stderr, /database connection refused/);
    });
  });

  it("a missing provider module is a usage error", async () => {
    await withTempDir(async (directory) => {
      const result = runScript(directory, "--provider", "./no-such-provider.mjs", "--identity", "agent:a", "--output", "out.jsonl");
      assert.equal(result.status, ce.EXIT_USAGE, result.stderr);
    });
  });

  it("a missing output directory is a usage error", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "host-provider.mjs"), PROVIDER_MODULE);
      const result = runScript(directory, "--provider", "./host-provider.mjs", "--identity", "agent:a", "--output", "missing/out.jsonl");
      assert.equal(result.status, ce.EXIT_USAGE, result.stderr);
      assert.match(result.stderr, /missing/);
      assert.ok(await noPartialFiles(directory));
    });
  });

  it("a missing identities file is a usage error", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "provider.mjs"), PROVIDER_MODULE);
      const result = runScript(
        directory, "--provider", "./provider.mjs", "--identities-file", "missing.txt", "--output", "out.jsonl",
      );
      assert.equal(result.status, ce.EXIT_USAGE, result.stderr);
      assert.match(result.stderr, /continuity export not started: cannot read --identities-file: .*missing\.txt/);
      assert.equal(await exists(join(directory, "out.jsonl")), false);
    });
  });

  it("a snapshot that cannot be read exits retryable", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "provider.mjs"), PROVIDER_MODULE);
      const result = runScript(
        directory, "--provider", "./provider.mjs#unreadableSnapshotStore", "--identity", "agent:a", "--output", "out.jsonl",
      );
      assert.equal(result.status, ce.EXIT_PROVIDER, result.stderr);
      assert.ok(
        result.stderr.includes(
          `continuity provider loadSessionSnapshot for session ${SESSION_ALPHA} failed, nothing written; ` +
            "run the export again",
        ),
        result.stderr,
      );
      assert.match(result.stderr, /snapshot storage went away/);
      assert.equal(await exists(join(directory, "out.jsonl")), false);
      assert.ok(await noPartialFiles(directory));
    });
  });

  it("an output that appears during the export is a failure after the start", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "provider.mjs"), PROVIDER_MODULE);
      const result = runScript(
        directory, "--provider", "./provider.mjs#racingStore", "--identity", "agent:a", "--output", "out.jsonl",
      );
      assert.equal(result.status, ce.EXIT_USAGE, result.stderr);
      assert.match(result.stderr, /continuity export failed, nothing written: .*appeared while the export ran/);
      assert.doesNotMatch(result.stderr, /not started/);
      assert.equal(await readFile(join(directory, "out.jsonl"), "utf8"), "keep me");
      assert.ok(await noPartialFiles(directory));
    });
  });

  it("verify accepts a file with an 8 MiB document", async () => {
    // Base64 is checked without a regexp that recurses per character group.
    const document = Buffer.alloc(8 << 20, 7);
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "large.jsonl"), frame([JSON.stringify(readyEntry("agent:a", SESSION_ALPHA, document))]));
      const result = runScript(directory, "--verify", "large.jsonl");
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /1 ready, 0 uninitialized/);
    });
  });

  it("verify refuses a damaged file", async () => {
    await withTempDir(async (directory) => {
      const data = Buffer.from(await readFile(join(FIXTURES, "written_by_node.jsonl")));
      data[data.length - 10] ^= 0x01;
      await writeFile(join(directory, "damaged.jsonl"), data);
      const result = runScript(directory, "--verify", "damaged.jsonl");
      assert.equal(result.status, ce.EXIT_INVALID_FILE);
      assert.match(result.stderr, /refused/);
    });
  });
});

// -- The JSON Lines provider (jsonl-provider.mjs) -------------------------------
//
// The same cases as JsonlProviderTests in test_continuity_export.py, against
// jsonl-provider.mjs.

const PROVIDER = fileURLToPath(new URL("./jsonl-provider.mjs", import.meta.url));
const H_REMOTE = fileURLToPath(
  new URL("../../crates/meerkat-meta/tests/fixtures/released_0_9_3_h_remote/", import.meta.url),
);
const JSONL_RECORDS_HEADER = "identity, agent_runtime_id, session_id, generation, checkpoint_version";

const jsonl = (rows) => rows.map((row) => `${JSON.stringify(row)}\n`).join("");
const sessionRow = (sessionId, document) => ({ session_id: sessionId, data: Buffer.from(document).toString("base64") });

/**
 * goldenStore()'s records as a host's tables give them: a session id spelled in
 * upper case, and integers some SQL tools write as strings.
 */
function goldenRecordRows() {
  return [
    wireRecord("agent:alpha", "rt-alpha", SESSION_ALPHA.toUpperCase(), "2", 7),
    wireRecord(IDENTITY_SMILE, "rt-smile", SESSION_SMILE, 0, "1"),
  ];
}

/** goldenStore()'s snapshots, in two more spellings, and one session no record names. */
function goldenSessionRows() {
  return [
    sessionRow(`{${SESSION_ALPHA}}`, DOC_ALPHA),
    sessionRow(SESSION_SMILE.replaceAll("-", ""), DOC_SMILE),
    sessionRow(SESSION_OTHER, encode("not the current session of any identity")),
  ];
}

/** A released 0.9.3 remote-continuity capture's export, taken apart into rows. */
async function captureRows(capture) {
  const exported = gunzipSync(await readFile(join(H_REMOTE, `${capture}-export.jsonl.gz`)));
  const identities = [];
  const records = [];
  const sessions = [];
  for (const line of entryLines(exported)) {
    const entry = JSON.parse(line);
    identities.push(entry.identity);
    if (entry.state === "ready") {
      records.push(entry.record);
      sessions.push({ session_id: entry.record.session_id, data: entry.document.base64 });
    }
  }
  return { identities, records, sessions, exported };
}

// Input errors both providers report the same way: [name, records lines,
// sessions lines, the file the error names, the message after "<path> line "].
// test_continuity_export.py holds the same table.
const RECORD_OK = `{"identity":"agent:a","agent_runtime_id":"rt","session_id":"${SESSION_ALPHA}","generation":0,"checkpoint_version":1}`;
const SESSION_OK = `{"session_id":"${SESSION_ALPHA}","data":"aGk="}`;
const JSONL_INPUT_ERRORS = [
  ["not JSON", ["{"], [SESSION_OK], "records", "1: the line is not JSON: "],
  ["not an object", ["[1]"], [SESSION_OK], "records", "1: the line is not a JSON object"],
  [
    "a non-JSON number",
    [`{"identity":"agent:a","agent_runtime_id":"rt","session_id":"${SESSION_ALPHA}","generation":NaN,"checkpoint_version":1}`],
    [SESSION_OK],
    "records",
    "1: the line is not JSON: ",
  ],
  [
    "a repeated key",
    [`{"identity":"agent:a","identity":"agent:b","agent_runtime_id":"rt","session_id":"${SESSION_ALPHA}","generation":0,"checkpoint_version":1}`],
    [SESSION_OK],
    "records",
    '1: key "identity" is repeated',
  ],
  [
    "a missing key",
    [`{"identity":"agent:a","agent_runtime_id":"rt","session_id":"${SESSION_ALPHA}","generation":0}`],
    [SESSION_OK],
    "records",
    `1: the object must have exactly the keys ${JSONL_RECORDS_HEADER} (missing checkpoint_version)`,
  ],
  [
    "unexpected keys",
    [`${RECORD_OK.slice(0, -1)},"zeta":1,"deleted":false}`],
    [SESSION_OK],
    "records",
    `1: the object must have exactly the keys ${JSONL_RECORDS_HEADER} (unexpected "deleted", "zeta")`,
  ],
  ["an identity that is not a string", [RECORD_OK.replace('"agent:a"', "5")], [SESSION_OK], "records", "1: identity is not a string"],
  ["a repeated identity", [RECORD_OK, "", RECORD_OK], [SESSION_OK], "records", '3: identity "agent:a" is repeated (first on line 1)'],
  ["a session id it cannot read", [RECORD_OK], ['{"session_id":"nope","data":"aGk="}'], "sessions", '1: session_id "nope" is not a session id'],
  [
    "a repeated session",
    [RECORD_OK],
    [SESSION_OK, SESSION_OK.replace(SESSION_ALPHA, SESSION_ALPHA.toUpperCase())],
    "sessions",
    `2: session ${SESSION_ALPHA} is repeated (first on line 1)`,
  ],
  [
    "base64 with a line break",
    [RECORD_OK],
    [SESSION_OK.replace("aGk=", "aGk=\\naGk=")],
    "sessions",
    "1: data is not standard base64 with padding and without line breaks",
  ],
  [
    "base64 without padding",
    [RECORD_OK],
    [SESSION_OK.replace("aGk=", "aGk")],
    "sessions",
    "1: data is not standard base64 with padding and without line breaks",
  ],
  [
    "the URL-safe alphabet",
    [RECORD_OK],
    [SESSION_OK.replace("aGk=", "-_8=")],
    "sessions",
    "1: data is not standard base64 with padding and without line breaks",
  ],
  [
    "data that is not a string",
    [RECORD_OK],
    [SESSION_OK.replace('"aGk="', "7")],
    "sessions",
    "1: data is not standard base64 with padding and without line breaks",
  ],
];

async function jsonlProvider(directory, records, sessions) {
  const recordsPath = join(directory, "records.jsonl");
  const sessionsPath = join(directory, "sessions.jsonl");
  await writeFile(recordsPath, records);
  await writeFile(sessionsPath, sessions);
  return new jp.JsonlContinuityProvider(recordsPath, sessionsPath);
}

function inputError(run) {
  try {
    run();
  } catch (error) {
    assert.ok(error instanceof jp.JsonlProviderInputError, `expected an input error, got ${String(error)}`);
    return error.message;
  }
  assert.fail("expected an input error");
}

function runWithProvider(directory, env, ...args) {
  const childEnv = { ...process.env, ...env };
  delete childEnv.FORCE_COLOR;
  delete childEnv.NO_COLOR;
  for (const [name, value] of Object.entries(env)) if (value === undefined) delete childEnv[name];
  return spawnSync(process.execPath, [SCRIPT, ...args], { cwd: directory, encoding: "utf8", timeout: 60_000, env: childEnv });
}

describe("jsonl provider", () => {
  it("writes the golden file from a host's rows", async () => {
    await withTempDir(async (directory) => {
      // CRLF line ends, a byte order mark, a blank line and a whitespace line.
      const crlf = (text) => text.replaceAll("\n", "\r\n");
      const provider = await jsonlProvider(
        directory,
        crlf(jsonl(goldenRecordRows())),
        `\ufeff${crlf(jsonl(goldenSessionRows()))}\n \t\r\n`,
      );
      const target = join(directory, "export.jsonl");
      const exported = await ce.exportContinuity(provider, GOLDEN_IDENTITIES, target);
      assert.deepEqual(await readFile(target), await readFile(join(FIXTURES, "written_by_node.jsonl")));
      assertGoldenEntries(exported);
    });
  });

  it("rebuilds the released remote captures' entries byte for byte", async () => {
    // storage_migrate_native_census.rs imports these exact files (for example
    // a_remote_export_imports_into_a_new_directory). The Python script wrote
    // them, so only the header's writer differs.
    for (const capture of ["memory", "durable"]) {
      await withTempDir(async (directory) => {
        const { identities, records, sessions, exported } = await captureRows(capture);
        assert.equal(records.length, 4);
        const provider = await jsonlProvider(directory, jsonl(records), jsonl(sessions));
        const target = join(directory, `${capture}.jsonl`);
        await ce.exportContinuity(provider, identities, target);
        const [header, entries] = splitFile(await readFile(target));
        const [capturedHeader, capturedEntries] = splitFile(exported);
        assert.deepEqual({ ...header, writer: "python" }, capturedHeader);
        assert.equal(header.writer, "node");
        assert.deepEqual(entries, capturedEntries);
      });
    }
  });

  it("the command line takes the provider from the environment", async () => {
    await withTempDir(async (directory) => {
      await writeFile(join(directory, "records.jsonl"), jsonl(goldenRecordRows()));
      await writeFile(join(directory, "sessions.jsonl"), jsonl(goldenSessionRows()));
      const identities = GOLDEN_IDENTITIES.flatMap((identity) => ["--identity", identity]);
      const env = {
        [jp.RECORDS_ENV]: join(directory, "records.jsonl"),
        [jp.SESSIONS_ENV]: join(directory, "sessions.jsonl"),
      };
      const result = runWithProvider(directory, env, "--provider", `${PROVIDER}#fromEnv`, ...identities, "--output", "export.jsonl");
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(await readFile(join(directory, "export.jsonl")), await readFile(join(FIXTURES, "written_by_node.jsonl")));

      const missing = runWithProvider(
        directory,
        { ...env, [jp.SESSIONS_ENV]: undefined },
        "--provider",
        `${PROVIDER}#fromEnv`,
        ...identities,
        "--output",
        "other.jsonl",
      );
      assert.equal(missing.status, ce.EXIT_PROVIDER);
      assert.match(missing.stderr, new RegExp(`${jp.SESSIONS_ENV} is not set`));
      assert.equal(await exists(join(directory, "other.jsonl")), false);
    });
  });

  it("refuses an identity the list misses as Unlisted", async () => {
    await withTempDir(async (directory) => {
      const provider = await jsonlProvider(directory, jsonl(goldenRecordRows()), jsonl(goldenSessionRows()));
      const error = await refused((target) => ce.exportContinuity(provider, ["agent:alpha"], target), directory);
      assert.deepEqual(rows(error), [`${IDENTITY_SMILE}|${ce.UNLISTED}`]);
    });
  });

  it("exports a listed identity without a record as uninitialized", async () => {
    await withTempDir(async (directory) => {
      const provider = await jsonlProvider(directory, jsonl(goldenRecordRows()), jsonl(goldenSessionRows()));
      const exported = await ce.exportContinuity(provider, [...GOLDEN_IDENTITIES, "agent:gamma"], join(directory, "export.jsonl"));
      assert.equal(exported.entries.find((entry) => entry.identity === "agent:gamma").state, "uninitialized");
    });
  });

  it("leaves a missing snapshot to the export's policy", async () => {
    await withTempDir(async (directory) => {
      const provider = await jsonlProvider(
        directory,
        jsonl([wireRecord("agent:a", "rt", SESSION_RENAMED, 0, 1)]),
        jsonl(goldenSessionRows()),
      );
      const error = await refused((target) => ce.exportContinuity(provider, ["agent:a"], target), directory);
      assert.deepEqual(rows(error), [`agent:a|${ce.RUNTIME_MISSING}`]);
      const exported = await ce.exportContinuity(provider, ["agent:a"], join(directory, "fresh.jsonl"), { missingSnapshot: "fresh" });
      assert.deepEqual(exported.entries.map((entry) => [entry.identity, entry.state]), [["agent:a", "uninitialized"]]);
    });
  });

  it("hands other values to the export as the file holds them", async () => {
    // The export refuses each of these as Unreadable. JSON.parse reads 1.0 as
    // 1, so a float literal is handed on as its text.
    for (const generation of ["007", 1.5, -1, "1e3", true, "__1.0__"]) {
      await withTempDir(async (directory) => {
        const record = JSON.stringify(wireRecord("agent:a", "rt", SESSION_ALPHA, 0, 1)).replace(
          '"generation":0',
          `"generation":${generation === "__1.0__" ? "1.0" : JSON.stringify(generation)}`,
        );
        const provider = await jsonlProvider(directory, `${record}\n`, jsonl([sessionRow(SESSION_ALPHA, encode("doc"))]));
        const error = await refused((target) => ce.exportContinuity(provider, ["agent:a"], target), directory);
        assert.deepEqual(rows(error), [`agent:a|${ce.UNREADABLE}`], String(generation));
      });
    }
    await withTempDir(async (directory) => {
      const largest = Number.MAX_SAFE_INTEGER;
      const provider = await jsonlProvider(
        directory,
        jsonl([wireRecord("agent:a", "rt", SESSION_ALPHA, String(largest), largest)]),
        jsonl([sessionRow(SESSION_ALPHA, encode("doc"))]),
      );
      const exported = await ce.exportContinuity(provider, ["agent:a"], join(directory, "export.jsonl"));
      assert.deepEqual([exported.entries[0].record.generation, exported.entries[0].record.checkpointVersion], [largest, largest]);
    });
  });

  it("refuses an integer above 2^53 - 1 and names the Python scripts", async () => {
    for (const [generation, text] of [['"9007199254740992"', "9007199254740992"], ["18446744073709551615", "18446744073709551615"]]) {
      await withTempDir(async (directory) => {
        const record = RECORD_OK.replace('"generation":0', `"generation":${generation}`);
        await writeFile(join(directory, "records.jsonl"), `${record}\n`);
        await writeFile(join(directory, "sessions.jsonl"), `${SESSION_OK}\n`);
        const message = inputError(
          () => new jp.JsonlContinuityProvider(join(directory, "records.jsonl"), join(directory, "sessions.jsonl")),
        );
        assert.equal(
          message,
          `${join(directory, "records.jsonl")} line 1: generation ${text} is above 2^53 - 1, the largest integer ` +
            "this script holds exactly; export with the Python scripts",
        );
      });
    }
  });

  it("answers resolveRecordBySession from the records file", async () => {
    await withTempDir(async (directory) => {
      const provider = await jsonlProvider(
        directory,
        jsonl([
          ...goldenRecordRows(),
          wireRecord("agent:b", "rt-b", SESSION_OTHER, 0, 0),
          wireRecord("agent:c", "rt-c", SESSION_OTHER.toUpperCase(), 0, 0),
        ]),
        jsonl(goldenSessionRows()),
      );
      assert.deepEqual(provider.resolveRecordBySession(SESSION_ALPHA), {
        record: {
          identity: "agent:alpha",
          agentRuntimeId: "rt-alpha",
          sessionId: SESSION_ALPHA.toUpperCase(),
          generation: 2,
          checkpointVersion: 7,
        },
        fencingToken: null,
        checkpointVersion: 7,
      });
      // A session no record names, or two records name, has no single record.
      assert.equal(provider.resolveRecordBySession(SESSION_RENAMED), null);
      assert.equal(provider.resolveRecordBySession(SESSION_OTHER), null);
    });
  });

  it("reads a snapshot only when the export asks", async () => {
    await withTempDir(async (directory) => {
      const provider = await jsonlProvider(directory, jsonl(goldenRecordRows()), jsonl(goldenSessionRows()));
      assert.deepEqual(new Uint8Array(provider.loadSessionSnapshot(SESSION_SMILE).data), DOC_SMILE);
      assert.equal(provider.loadSessionSnapshot(SESSION_RENAMED), null);
      // The provider keeps where each line starts, so a file rewritten after it
      // opened is caught rather than read as another session.
      const sessionsPath = join(directory, "sessions.jsonl");
      await writeFile(sessionsPath, jsonl([sessionRow(SESSION_RENAMED, DOC_ALPHA), ...goldenSessionRows()]));
      assert.equal(
        inputError(() => provider.loadSessionSnapshot(SESSION_ALPHA)),
        `${sessionsPath} line 1: the file changed after the provider opened it`,
      );
    });
  });

  it("names the file and the line of an input error", async () => {
    for (const [name, records, sessions, which, message] of JSONL_INPUT_ERRORS) {
      await withTempDir(async (directory) => {
        const recordsPath = join(directory, "records.jsonl");
        const sessionsPath = join(directory, "sessions.jsonl");
        await writeFile(recordsPath, records.map((line) => `${line}\n`).join(""));
        await writeFile(sessionsPath, sessions.map((line) => `${line}\n`).join(""));
        const error = inputError(() => new jp.JsonlContinuityProvider(recordsPath, sessionsPath));
        const path = which === "records" ? recordsPath : sessionsPath;
        assert.ok(error.startsWith(`${path} line ${message}`), `${name}: ${error}`);
      });
    }
    await withTempDir(async (directory) => {
      const recordsPath = join(directory, "records.jsonl");
      await writeFile(recordsPath, Buffer.from([0xff, 0x0a]));
      await writeFile(join(directory, "sessions.jsonl"), `${SESSION_OK}\n`);
      assert.equal(
        inputError(() => new jp.JsonlContinuityProvider(recordsPath, join(directory, "sessions.jsonl"))),
        `${recordsPath} line 1: the line is not valid UTF-8`,
      );
    });
  });

  it("fromEnv names a missing variable", () => {
    for (const missing of [jp.RECORDS_ENV, jp.SESSIONS_ENV]) {
      const saved = { [jp.RECORDS_ENV]: process.env[jp.RECORDS_ENV], [jp.SESSIONS_ENV]: process.env[jp.SESSIONS_ENV] };
      try {
        process.env[jp.RECORDS_ENV] = "records.jsonl";
        process.env[jp.SESSIONS_ENV] = "sessions.jsonl";
        delete process.env[missing];
        assert.ok(inputError(() => jp.fromEnv()).startsWith(`${missing} is not set`));
      } finally {
        for (const [name, value] of Object.entries(saved)) {
          if (value === undefined) delete process.env[name];
          else process.env[name] = value;
        }
      }
    }
  });
});
