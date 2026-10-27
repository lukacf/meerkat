#!/usr/bin/env node
/**
 * Export a 0.9.3 SDK-hosted continuity store into a continuity export file.
 *
 * Meta 0.9.4 keeps session and identity truth in Core's local stores, so the
 * 0.9.3 SDK continuity provider (`continuityStore` on the 0.9.3 builder) has
 * no 0.9.4 runtime. A host whose continuity lived behind such a provider:
 *
 * 1. stops every 0.9.3 gateway that uses the store;
 * 2. runs this script once, in its 0.9.3 environment, against its existing
 *    provider object;
 * 3. upgrades to 0.9.4;
 * 4. imports the file with `meta_gateway storage-migrate-native`.
 *
 * This script is standalone: it needs Node.js 18 or later and imports nothing
 * from the Meta SDK of either version. The export calls only the 0.9.3 read
 * methods: `resolveMany`, `loadSessionSnapshot` and, when the provider has
 * it, `resolveRecordBySession`. It reads their results the way the 0.9.3 SDK
 * forwarded them to the gateway (resolveMany's answer through the transport's
 * JSON path, a snapshot's `data` bytes) and checks them with the gateway's
 * rules. It writes nothing to the provider, and it cannot see changes made
 * after it finishes.
 *
 * When any listed identity cannot be exported as it stands, the whole export
 * is refused, no file is written, and every such identity is reported with
 * its class and the action that clears it. A provider exception, from a read
 * method or from reading the value it returned, aborts the export; it is never
 * written down as a fact, and the export can be run again.
 *
 * A ready record whose session the provider holds no snapshot for is refused
 * (RuntimeMissing) unless `--missing-snapshot fresh` is given. A 0.9.3 host
 * built with the Python or TypeScript SDK never saved a snapshot to its
 * provider: its gateway kept sessions in memory and started every identity
 * fresh at each restart. With `fresh` the export writes each such identity as
 * `uninitialized`, so it starts fresh after the upgrade as well, with an empty
 * transcript, and reports it with its 0.9.3 record.
 *
 * Usage:
 *
 *     node continuity-export.mjs --provider ./provider.mjs[#export] \
 *         (--identity ID ... | --identities-file FILE) --output FILE \
 *         [--missing-snapshot refuse|fresh]
 *     node continuity-export.mjs --verify FILE [FILE ...]
 *
 * `--provider` names an ES module or CommonJS file, relative to the current
 * directory, and its export holding the provider object or a function
 * returning it (it may be async); the export defaults to `default`. A
 * TypeScript provider module can be loaded by running this script under tsx.
 * List every identity the store holds a record for, including retired
 * identities and identities the roster no longer lists: the 0.9.3 protocol
 * cannot enumerate them, and only the listed identities are exported. An
 * identities file is UTF-8 with one identity per line; a byte order mark at
 * its start, whitespace around an identity and blank lines are ignored. Only
 * each identity's current session is exported.
 *
 * Exit status: 0 written or verified; 1 refused (each row is printed); 2 not
 * started or failed locally (a usage error, or the output exists or cannot be
 * written); 3 the provider failed (its module, its factory, a read or the value
 * a read returned; run it again); 4 a file failed verification.
 *
 * File format, version 1. The file is UTF-8, and every line ends in a newline:
 *
 * - line 1, the header: `{"format": "meerkat-meta.continuity-export",
 *   "version": 1, "writer": <name>, "entries": <count>, "entries_length":
 *   <bytes>, "entries_sha256": <hex>}`;
 * - then one line per listed identity, its entry, in compact JSON.
 *
 * `entries` counts the entry lines, `entries_length` is the number of bytes
 * after the header line, and `entries_sha256` is their SHA-256. Entries are
 * sorted by identity in code point order and are unique. An entry line is at
 * most 536,870,888 bytes, the longest string Node.js holds, so that either
 * script reads any file; a session whose entry would be longer is refused
 * (TooLarge). A `ready` entry is
 * `{"identity", "state", "record", "document"}`: `record` carries the 0.9.3
 * continuity record's five fields (snake_case, as on the wire), with
 * `session_id` in canonical form (lowercase and hyphenated, the form 0.9.3
 * handed the provider), and
 * `document` is `{"length", "sha256", "base64"}` over the exact bytes
 * `loadSessionSnapshot` returned. An `uninitialized` entry is
 * `{"identity", "state"}`. Documents are never decoded here; the migration
 * verb decodes and checks them.
 *
 * Both directions stream. The export writes each entry to a temporary file as
 * soon as its document is read, then writes the file from there and verifies
 * it, so its memory follows the largest document (up to about 20 times its
 * size), not the store, and it needs free space for about twice the file.
 * Reading a file holds one entry at a time.
 */

import { createHash, randomBytes } from "node:crypto";
import { link, lstat, open, readFile, rename, rm } from "node:fs/promises";
import { realpathSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const CONTINUITY_EXPORT_FORMAT = "meerkat-meta.continuity-export";
export const CONTINUITY_EXPORT_VERSION = 1;
export const WRITER = "node";

// The 0.9.3 transport gave provider callbacks this completion deadline.
const PROVIDER_CALLBACK_DEADLINE_MS = 125_000;
const HEADER_KEYS = ["format", "version", "writer", "entries", "entries_length", "entries_sha256"];
const READY_KEYS = ["identity", "state", "record", "document"];
const UNINITIALIZED_KEYS = ["identity", "state"];
const RECORD_KEYS = ["identity", "agent_runtime_id", "session_id", "generation", "checkpoint_version"];
const DOCUMENT_KEYS = ["length", "sha256", "base64"];
// The longest entry line, without its newline, either script writes or reads:
// the longest string Node.js can hold (buffer.constants.MAX_STRING_LENGTH on
// 64-bit platforms), so that this script holds any entry line whole.
export const MAX_ENTRY_LINE_BYTES = 536_870_888;
const MAX_HEADER_LINE_BYTES = 65_536;
const READ_CHUNK_BYTES = 1 << 20;
const NEWLINE = Buffer.from("\n");
// The form an export file holds: SessionId's Display, lowercase and hyphenated.
const SESSION_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
// Every spelling Core 0.9.3's SessionId parsed (the uuid crate): simple,
// hyphenated, braced and urn, with hex digits in either case. The 0.9.3
// gateway handed the provider's session methods the canonical form.
const HYPHENATED = "[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}";
const SESSION_ID_SPELLINGS = new RegExp(
  `^(?:[0-9A-Fa-f]{32}|${HYPHENATED}|\\{${HYPHENATED}\\}|urn:uuid:${HYPHENATED})$`,
);
const SHA256_HEX = /^[0-9a-f]{64}$/;
// Padded standard base64, checked by length and one flat character class: a
// regexp that repeats a 4-character group overflows V8's backtracking stack
// on a document of a few MiB.
const BASE64_CHARACTERS = /^[A-Za-z0-9+/]*={0,2}$/;
const LONE_SURROGATE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;
// The Unicode White_Space set (Rust char::is_whitespace), which AgentIdentity
// and AgentRuntimeId refuse along with the empty string and "/".
const WHITE_SPACE = new Set([
  "\t", "\n", "\v", "\f", "\r", " ", "\u0085", "\u00a0", "\u1680",
  "\u2000", "\u2001", "\u2002", "\u2003", "\u2004", "\u2005", "\u2006",
  "\u2007", "\u2008", "\u2009", "\u200a",
  "\u2028", "\u2029", "\u202f", "\u205f", "\u3000",
]);

// What the export does with a ready record whose session has no snapshot:
// refuse it (RuntimeMissing), or write the identity as uninitialized.
export const MISSING_SNAPSHOT_REFUSE = "refuse";
export const MISSING_SNAPSHOT_FRESH = "fresh";
export const MISSING_SNAPSHOT_POLICIES = Object.freeze([MISSING_SNAPSHOT_REFUSE, MISSING_SNAPSHOT_FRESH]);

// Refusal classes, in the migration verb's doctor vocabulary, and the action
// that clears each.
export const BROKEN = "Broken";
export const RUNTIME_MISSING = "RuntimeMissing";
export const IDENTITY_DISAGREES = "IdentityDisagrees";
export const UNREADABLE = "Unreadable";
export const UNLISTED = "Unlisted";
export const TOO_LARGE = "TooLarge";
export const ACTIONS = Object.freeze({
  [BROKEN]: "settle or delete the identity with the 0.9.3 binary, then rerun the export",
  [RUNTIME_MISSING]:
    "if a 0.9.3 host built with the Python or TypeScript SDK wrote the store, it never " +
    "saved snapshots: rerun the export with --missing-snapshot fresh, which writes the " +
    "identity as uninitialized, so it starts fresh with an empty transcript, as at every " +
    "0.9.3 restart; otherwise restore the session's snapshot, or delete the identity with " +
    "the 0.9.3 binary, then rerun the export",
  [IDENTITY_DISAGREES]:
    "settle the identity's record with the 0.9.3 binary (or delete the identity), " +
    "then rerun the export",
  [UNREADABLE]: "make the provider answer in the 0.9.3 wire shape, then rerun the export",
  [UNLISTED]: "add the identity to the identity list, then rerun the export",
  [TOO_LARGE]:
    "reset or delete the identity with the 0.9.3 binary (either drops its session), " +
    "then rerun the export",
});

export const EXIT_REFUSED = 1;
export const EXIT_USAGE = 2;
export const EXIT_PROVIDER = 3;
export const EXIT_INVALID_FILE = 4;

/**
 * @typedef {object} ContinuityExportRecord The 0.9.3 continuity record; its
 *   session id is in canonical form, lowercase and hyphenated.
 * @property {string} identity
 * @property {string} agentRuntimeId
 * @property {string} sessionId
 * @property {number} generation
 * @property {number} checkpointVersion
 */

/**
 * @typedef {object} ContinuityExportEntry One listed identity. `state` is
 *   "ready" (record, documentLength and documentSha256 describe the session
 *   and its verified document) or "uninitialized" (the provider holds no
 *   continuity for the identity; all three are null). The document bytes stay
 *   in the file.
 * @property {string} identity
 * @property {"ready" | "uninitialized"} state
 * @property {ContinuityExportRecord | null} record
 * @property {number | null} documentLength
 * @property {string | null} documentSha256
 */

/**
 * @typedef {object} ContinuityExport A verified continuity export file.
 *   `missingSnapshots` holds, for an export run with missingSnapshot "fresh",
 *   the 0.9.3 records whose session the provider held no snapshot for; the
 *   file lists those identities as uninitialized. It is empty for a file read
 *   back.
 * @property {string} path
 * @property {number} version
 * @property {string} writer
 * @property {string} entriesSha256
 * @property {ContinuityExportEntry[]} entries
 * @property {ContinuityExportRecord[]} missingSnapshots
 */

/**
 * @typedef {object} ContinuityExportOptions
 * @property {number} [maxEntryLineBytes] a lower entry line limit than
 *   MAX_ENTRY_LINE_BYTES, for tests; the command line never sets it
 * @property {"refuse" | "fresh"} [missingSnapshot] a ready record whose
 *   session the provider holds no snapshot for: "refuse" (the default)
 *   refuses it as RuntimeMissing; "fresh" writes the identity as
 *   uninitialized and returns its record in missingSnapshots. Export only.
 */

/**
 * @typedef {object} ContinuityExportRefusal A listed identity the export cannot
 *   carry: its class and the action that clears it.
 * @property {string} rowClass
 * @property {string | null} identity
 * @property {string} detail
 * @property {string | null} sessionId
 * @property {string} action
 */

// -- Errors ------------------------------------------------------------------

// A malformed argument, refused before the export reads anything; the
// command line reports it as "not started".
class ArgumentTypeError extends TypeError {}
class ArgumentRangeError extends RangeError {}

// The target existed before the export read anything, so the command line
// reports it as "not started" too.
class TargetExistsError extends Error {
  constructor(target) {
    super(`continuity export target already exists; it is never overwritten: ${target}`);
    this.code = "EEXIST";
    this.path = target;
  }
}

/** Base class of the continuity export errors. */
export class ContinuityExportError extends Error {
  constructor(message, options) {
    super(message, options);
    this.name = "ContinuityExportError";
  }
}

/** Listed identities cannot be exported as they stand; no file was written. */
export class ContinuityExportRefusedError extends ContinuityExportError {
  /** @param {ContinuityExportRefusal[]} refusals */
  constructor(refusals) {
    const rows = refusals
      .map((refusal) => `${refusal.identity ?? "-"}: ${refusal.rowClass} (${refusal.detail})`)
      .join("; ");
    super(`continuity export refused, nothing written: ${rows}`);
    this.name = "ContinuityExportRefusedError";
    this.refusals = refusals;
  }
}

/**
 * A provider read failed; nothing was written and the export can be run again.
 * `operation` names the provider method, `subject` the session it was asked
 * about (null for resolveMany), and `cause` is the provider's error: thrown by
 * the method, or by the value it returned while the export read it.
 */
export class ContinuityExportProviderError extends ContinuityExportError {
  constructor(operation, subject, cause) {
    const about = subject !== null ? ` for session ${subject}` : "";
    super(
      `continuity provider ${operation}${about} failed, nothing written; ` +
        `run the export again: ${describe(cause)}`,
      { cause },
    );
    this.name = "ContinuityExportProviderError";
    this.operation = operation;
    this.subject = subject;
    this.retryable = true;
  }
}

/** A file is not an intact version 1 continuity export. */
export class ContinuityExportFileError extends ContinuityExportError {
  constructor(path, reason) {
    super(`continuity export file ${path} refused: ${reason}`);
    this.name = "ContinuityExportFileError";
    this.path = path;
    this.reason = reason;
  }
}

// -- Export ------------------------------------------------------------------

/**
 * Export the provider's continuity for `identities` into a new file at `path`.
 *
 * `path` must not exist. Each entry is written to a temporary file in the same
 * directory as soon as its document is read; the export file is then written
 * next to it under a temporary name, verified, and given its name. Only its
 * owner can read it, because it holds full transcripts.
 *
 * Throws TypeError or RangeError for a malformed identity list, an unknown
 * missingSnapshot or a provider without the 0.9.3 read methods (the provider
 * is not called), an EEXIST error when `path` exists or a file appears there
 * while the export runs (it is never replaced), ContinuityExportRefusedError
 * for identities that cannot be exported, and the retryable
 * ContinuityExportProviderError when a provider read, or reading the value it
 * returned, fails.
 *
 * @param {object} provider the host's 0.9.3 continuity provider object
 * @param {Iterable<string>} identities every identity the store holds a record for
 * @param {string} path the file to create
 * @param {ContinuityExportOptions} [options]
 * @returns {Promise<ContinuityExport>}
 */
export async function exportContinuity(provider, identities, path, options = {}) {
  const limit = entryLineLimit(options);
  const missingSnapshot = missingSnapshotPolicy(options);
  const requested = requestedIdentities(identities);
  const resolveMany = providerMethod(provider, "resolveMany");
  const loadSessionSnapshot = providerMethod(provider, "loadSessionSnapshot");
  const resolveRecordBySession =
    typeof provider.resolveRecordBySession === "function" ? provider.resolveRecordBySession : null;
  if (typeof path !== "string" || path.length === 0) {
    throw new ArgumentTypeError("path must be a non-empty string");
  }
  if (await pathExists(path)) throw new TargetExistsError(path);
  const stem = join(dirname(resolve(path)), `.${basename(path)}.${process.pid}.${randomBytes(8).toString("hex")}`);
  const entries = await EntryFile.create(`${stem}.entries.partial`);
  try {
    const methods = { resolveMany, loadSessionSnapshot, resolveRecordBySession };
    const policy = { limit, missingSnapshot };
    return await exportEntries(provider, requested, methods, entries, policy, path, `${stem}.partial`);
  } finally {
    await entries.discard();
  }
}

async function exportEntries(provider, requested, methods, entries, policy, path, partial) {
  const { resolveMany, loadSessionSnapshot, resolveRecordBySession } = methods;
  const { limit, missingSnapshot } = policy;
  // The 0.9.3 TS builder passed resolveMany's answer to the transport
  // unchanged, which sanitized it and sent it as JSON.
  const returned = await providerCall("resolveMany", null, () => resolveMany.call(provider, [...requested]));
  const answer = providerValue("resolveMany", null, () => toWire(returned));
  if (!isPlainObject(answer)) {
    throw new ContinuityExportRefusedError([
      refusal(UNREADABLE, null, "resolveMany did not return an object"),
    ]);
  }

  /** @type {ContinuityExportRefusal[]} */
  const refusals = [];
  const requestedSet = new Set(requested);
  for (const key of Object.keys(answer)) {
    if (!requestedSet.has(key)) {
      refusals.push(
        refusal(UNLISTED, key, "resolveMany answered for an identity that is not in the identity list"),
      );
    }
  }

  /** @type {Map<string, ContinuityExportRecord>} */
  const records = new Map();
  for (const identity of requested) {
    if (!Object.prototype.hasOwnProperty.call(answer, identity)) {
      refusals.push(refusal(UNREADABLE, identity, "resolveMany returned no entry for this identity"));
      continue;
    }
    const outcome = parseResolveState(identity, answer[identity]);
    if (outcome === null) continue;
    if ("rowClass" in outcome) refusals.push(outcome);
    else records.set(identity, outcome);
  }

  /** @type {Map<string, string[]>} */
  const owners = new Map();
  for (const [identity, record] of records) {
    const bound = owners.get(record.sessionId) ?? [];
    bound.push(identity);
    owners.set(record.sessionId, bound);
  }
  for (const [sessionId, bound] of owners) {
    if (bound.length < 2) continue;
    for (const identity of bound) {
      const others = sortByCodePoint(bound.filter((other) => other !== identity)).join(", ");
      refusals.push(
        refusal(IDENTITY_DISAGREES, identity, `session ${sessionId} is also bound to ${others}`, sessionId),
      );
      records.delete(identity);
    }
  }

  // Entries are written in file order as their documents are read, so at most
  // one document is held. After the first refusal nothing more is written, but
  // every document is still read, to report every refusal.
  /** @type {ContinuityExportRecord[]} */
  const missing = [];
  for (const identity of sortByCodePoint([...requested])) {
    const record = records.get(identity);
    if (record === undefined) {
      if (refusals.length === 0) await entries.write(uninitializedLine(identity));
      continue;
    }
    const sessionId = record.sessionId;
    const snapshot = await providerCall("loadSessionSnapshot", sessionId, () =>
      loadSessionSnapshot.call(provider, sessionId),
    );
    if (snapshot === null || snapshot === undefined) {
      if (missingSnapshot === MISSING_SNAPSHOT_FRESH) {
        // The entry an identity without a record gets: it starts fresh.
        missing.push(record);
        if (refusals.length === 0) await entries.write(uninitializedLine(identity));
        continue;
      }
      refusals.push(
        refusal(
          RUNTIME_MISSING,
          identity,
          `the provider holds no snapshot for session ${sessionId} (${recordValues(record)})`,
          sessionId,
        ),
      );
      continue;
    }
    const document = providerValue("loadSessionSnapshot", sessionId, () => snapshotBytes(snapshot));
    if (typeof document === "string") {
      refusals.push(refusal(UNREADABLE, identity, document, sessionId));
      continue;
    }
    if (resolveRecordBySession !== null) {
      // The 0.9.3 transport passed this callback context.
      const context = {
        signal: new AbortController().signal,
        deadlineMs: Date.now() + PROVIDER_CALLBACK_DEADLINE_MS,
      };
      const bound = await providerCall("resolveRecordBySession", sessionId, () =>
        resolveRecordBySession.call(provider, sessionId, context),
      );
      const disagreement = providerValue("resolveRecordBySession", sessionId, () =>
        sessionIndexRefusal(identity, sessionId, bound),
      );
      if (disagreement !== null) {
        refusals.push(disagreement);
        continue;
      }
    }
    const line = readyEntryLine(record, document, limit);
    if ("rowClass" in line) refusals.push(line);
    else if (refusals.length === 0) await entries.write(line);
  }

  if (refusals.length > 0) {
    refusals.sort(
      (left, right) =>
        compareCodePoints(left.identity ?? "", right.identity ?? "") ||
        compareCodePoints(left.rowClass, right.rowClass),
    );
    throw new ContinuityExportRefusedError(refusals);
  }

  const exported = await writeExport(path, entries, partial);
  return { ...exported, missingSnapshots: missing };
}

/**
 * Read and verify a continuity export file. Throws ContinuityExportFileError
 * unless the file is an intact version 1 export: the header, the count,
 * length and SHA-256 of the entry lines and every entry are checked, and
 * documents must match their recorded length and SHA-256. The file is read
 * one line at a time.
 *
 * @param {string} path
 * @param {ContinuityExportOptions} [options]
 * @returns {Promise<ContinuityExport>}
 */
export async function readContinuityExport(path, options = {}) {
  const limit = entryLineLimit(options);
  const handle = await open(path, "r");
  try {
    return await readExport(path, handle, limit);
  } finally {
    await handle.close();
  }
}

function missingSnapshotPolicy(options) {
  // Only an absent option takes the default, as in the Python script.
  const policy = options.missingSnapshot === undefined ? MISSING_SNAPSHOT_REFUSE : options.missingSnapshot;
  if (!MISSING_SNAPSHOT_POLICIES.includes(policy)) {
    throw new ArgumentRangeError(
      `missingSnapshot must be ${MISSING_SNAPSHOT_POLICIES.map((value) => JSON.stringify(value)).join(" or ")}, ` +
        `not ${typeof policy === "string" ? JSON.stringify(policy) : String(policy)}`,
    );
  }
  return policy;
}

function entryLineLimit(options) {
  const limit = options.maxEntryLineBytes ?? MAX_ENTRY_LINE_BYTES;
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > MAX_ENTRY_LINE_BYTES) {
    throw new ArgumentRangeError(`maxEntryLineBytes must be an integer from 1 to ${MAX_ENTRY_LINE_BYTES}`);
  }
  return limit;
}

// -- Reading provider values (the 0.9.3 wire shapes) -------------------------

function requestedIdentities(identities) {
  if (typeof identities === "string") {
    throw new ArgumentTypeError("identities must be a list of identity strings, not one string");
  }
  if (identities === null || typeof identities !== "object" || !(Symbol.iterator in identities)) {
    throw new ArgumentTypeError("identities must be an iterable of identity strings");
  }
  const requested = [...identities];
  if (requested.length === 0) throw new ArgumentRangeError("identities must name at least one identity");
  const seen = new Set();
  for (const identity of requested) {
    if (typeof identity !== "string") throw new ArgumentTypeError(`identity ${String(identity)} is not a string`);
    const problem = identityProblem(identity);
    if (problem !== null) throw new ArgumentRangeError(`identity ${JSON.stringify(identity)} ${problem}`);
    if (seen.has(identity)) throw new ArgumentRangeError(`identity ${JSON.stringify(identity)} is listed twice`);
    seen.add(identity);
  }
  return requested;
}

function providerMethod(provider, name) {
  const method = provider !== null && typeof provider === "object" ? provider[name] : undefined;
  if (typeof method !== "function") {
    throw new ArgumentTypeError(`the continuity provider has no ${name} method (a 0.9.3 read method)`);
  }
  return method;
}

async function providerCall(operation, subject, call) {
  try {
    return await call();
  } catch (error) {
    throw new ContinuityExportProviderError(operation, subject, error);
  }
}

// `read` reads a value `operation` returned. That runs provider code too
// (getters, toJSON), so what it throws is a provider fault, like a failed call.
function providerValue(operation, subject, read) {
  try {
    return read();
  } catch (error) {
    throw new ContinuityExportProviderError(operation, subject, error);
  }
}

/** The value as the 0.9.3 transport sent it: sanitized, then JSON. */
function toWire(value) {
  const text = JSON.stringify(sanitizeForJson(value));
  return text === undefined ? undefined : JSON.parse(text);
}

function sanitizeForJson(value) {
  if (value === null || value === undefined) return value;
  if (typeof value === "boolean" || typeof value === "number" || typeof value === "string") return value;
  if (Array.isArray(value)) return value.map(sanitizeForJson);
  if (typeof value === "object") {
    const result = {};
    for (const [key, entry] of Object.entries(value)) result[key] = sanitizeForJson(entry);
    return result;
  }
  return String(value);
}

function parseResolveState(identity, wire) {
  const unreadable = (detail) => refusal(UNREADABLE, identity, detail);
  if (!isPlainObject(wire)) return unreadable("the resolve state is not an object");
  const state = wire.state;
  const record = wire.record ?? null;
  const failure = wire.failure ?? null;
  if (state === "uninitialized") {
    if (record !== null || failure !== null) {
      return unreadable("the uninitialized state carries a record or a failure");
    }
    return null;
  }
  if (state === "broken") {
    const reported = isPlainObject(failure)
      ? `kind ${String(failure.kind)}, detail ${String(failure.detail)}`
      : "no failure detail";
    return refusal(BROKEN, identity, `the provider reports broken continuity: ${reported}`);
  }
  if (state !== "ready") return unreadable(`unknown state ${JSON.stringify(state)}`);
  if (failure !== null) return unreadable("the ready state carries a failure");
  if (record === null) return unreadable("the ready state has no record");
  const parsed = parseRecord(record);
  if (typeof parsed === "string") return unreadable(parsed);
  if (parsed.identity !== identity) {
    return refusal(
      IDENTITY_DISAGREES,
      identity,
      `the record under this identity names identity ${JSON.stringify(parsed.identity)}`,
      parsed.sessionId,
    );
  }
  return parsed;
}

/** Check a wire record by the gateway's rules; a string is the reason it fails. */
function parseRecord(wire) {
  if (!isPlainObject(wire)) return "the record is not an object";
  for (const field of ["identity", "agent_runtime_id"]) {
    const value = wire[field];
    if (typeof value !== "string") return `record ${field} is missing or not a string`;
    const problem = identityProblem(value);
    if (problem !== null) return `record ${field} ${JSON.stringify(value)} ${problem}`;
  }
  const sessionId = canonicalSessionId(wire.session_id);
  if (sessionId === null) {
    return `record session_id ${JSON.stringify(wire.session_id)} is not a session id`;
  }
  for (const field of ["generation", "checkpoint_version"]) {
    if (!isU64(wire[field])) {
      return `record ${field} ${JSON.stringify(wire[field])} is not an unsigned 64-bit integer`;
    }
  }
  return {
    identity: wire.identity,
    agentRuntimeId: wire.agent_runtime_id,
    sessionId,
    generation: wire.generation,
    checkpointVersion: wire.checkpoint_version,
  };
}

/** The snapshot's exact bytes, or the reason it is unreadable. */
function snapshotBytes(snapshot) {
  // The 0.9.3 TS builder base64-encoded `snapshot.data` byte by byte.
  const data = snapshot !== null && typeof snapshot === "object" ? snapshot.data : undefined;
  if (!(data instanceof Uint8Array)) return "the snapshot has no data (a Uint8Array)";
  if (data.length === 0) return "the snapshot is empty";
  return new Uint8Array(data);
}

/** A session id in any spelling 0.9.3 accepted, in canonical form; else null. */
function canonicalSessionId(value) {
  if (typeof value !== "string" || !SESSION_ID_SPELLINGS.test(value)) return null;
  const digits = value.replace(/^urn:uuid:/, "").replace(/[{}-]/g, "").toLowerCase();
  return `${digits.slice(0, 8)}-${digits.slice(8, 12)}-${digits.slice(12, 16)}-${digits.slice(16, 20)}-${digits.slice(20)}`;
}

function sessionIndexRefusal(identity, sessionId, bound) {
  if (bound === null || bound === undefined) {
    return refusal(
      IDENTITY_DISAGREES,
      identity,
      "resolveRecordBySession holds no record for this session",
      sessionId,
    );
  }
  // The 0.9.3 TS builder read bound.record through continuityRecordToDict.
  const record = typeof bound === "object" ? bound.record : undefined;
  if (record === null || typeof record !== "object") {
    return refusal(
      UNREADABLE,
      identity,
      "resolveRecordBySession did not return { record, fencingToken, checkpointVersion }",
      sessionId,
    );
  }
  if (record.identity !== identity || canonicalSessionId(record.sessionId) !== sessionId) {
    return refusal(
      IDENTITY_DISAGREES,
      identity,
      "resolveRecordBySession binds this session to identity " +
        `${JSON.stringify(record.identity)} and session ${JSON.stringify(record.sessionId)}`,
      sessionId,
    );
  }
  return null;
}

// -- Writing the file ----------------------------------------------------------

/**
 * A ready entry's line in parts, the document's base64 produced in chunks so no
 * part is a long string, or a TooLarge refusal when the line would be longer
 * than `limit` bytes.
 */
function readyEntryLine(record, document, limit) {
  const skeleton = Buffer.from(
    JSON.stringify({
      identity: record.identity,
      state: "ready",
      record: {
        identity: record.identity,
        agent_runtime_id: record.agentRuntimeId,
        session_id: record.sessionId,
        generation: record.generation,
        checkpoint_version: record.checkpointVersion,
      },
      document: { length: document.length, sha256: sha256Hex(document), base64: "" },
    }),
    "utf8",
  );
  const size = skeleton.length + 4 * Math.ceil(document.length / 3);
  if (size > limit) {
    return refusal(
      TOO_LARGE,
      record.identity,
      `the snapshot is ${document.length} bytes, so its entry would be ${size} bytes; ` +
        `an entry holds at most ${limit} bytes`,
      record.sessionId,
    );
  }
  // The skeleton ends with the empty base64 string and the two closing braces.
  return [skeleton.subarray(0, -3), ...base64Chunks(document), skeleton.subarray(-3)];
}

function* base64Chunks(document) {
  const bytes = Buffer.from(document.buffer, document.byteOffset, document.byteLength);
  // Whole 3-byte groups, so only the last chunk can carry padding.
  const step = 3 << 18;
  for (let offset = 0; offset < bytes.length; offset += step) {
    yield Buffer.from(bytes.subarray(offset, offset + step).toString("base64"), "latin1");
  }
}

/** A record's 0.9.3 values, as the export prints them. */
function recordValues(record) {
  return (
    `0.9.3 record: agent_runtime_id ${record.agentRuntimeId}, ` +
    `generation ${record.generation}, checkpoint_version ${record.checkpointVersion}`
  );
}

function uninitializedLine(identity) {
  return [Buffer.from(JSON.stringify({ identity, state: "uninitialized" }), "utf8")];
}

/** The entry lines, written as they are made, and their count, length and SHA-256. */
class EntryFile {
  static async create(path) {
    return new EntryFile(path, await open(path, "wx", 0o600));
  }

  constructor(path, handle) {
    this.path = path;
    this.handle = handle;
    this.open = true;
    this.count = 0;
    this.length = 0;
    this.hash = createHash("sha256");
  }

  /** @param {Buffer[]} parts one line without its newline */
  async write(parts) {
    for (const part of [...parts, NEWLINE]) {
      await writeAll(this.handle, part);
      this.hash.update(part);
      this.length += part.length;
    }
    this.count += 1;
  }

  async close() {
    if (!this.open) return;
    this.open = false;
    await this.handle.close();
  }

  async discard() {
    await this.close().catch(() => {});
    await rm(this.path, { force: true });
  }
}

async function writeAll(handle, bytes) {
  for (let offset = 0; offset < bytes.length; ) {
    const { bytesWritten } = await handle.write(bytes, offset, bytes.length - offset, null);
    offset += bytesWritten;
  }
}

/** Write the header and the entries to `partial`, verify it, and give it its name. */
async function writeExport(target, entries, partial) {
  await entries.close();
  const header = Buffer.from(
    JSON.stringify({
      format: CONTINUITY_EXPORT_FORMAT,
      version: CONTINUITY_EXPORT_VERSION,
      writer: WRITER,
      entries: entries.count,
      entries_length: entries.length,
      entries_sha256: entries.hash.digest("hex"),
    }),
    "utf8",
  );
  const handle = await open(partial, "wx", 0o600);
  let exported;
  try {
    try {
      await writeAll(handle, Buffer.concat([header, NEWLINE]));
      const source = await open(entries.path, "r");
      try {
        const chunk = Buffer.allocUnsafe(READ_CHUNK_BYTES);
        for (;;) {
          const { bytesRead } = await source.read(chunk, 0, chunk.length, null);
          if (bytesRead === 0) break;
          await writeAll(handle, chunk.subarray(0, bytesRead));
        }
      } finally {
        await source.close();
      }
      await handle.sync();
    } finally {
      await handle.close();
    }
    await entries.discard();
    exported = await readContinuityExport(partial);
    await publishExclusive(partial, target);
  } catch (error) {
    await rm(partial, { force: true });
    throw error;
  }
  await syncDirectory(dirname(resolve(target)));
  return { ...exported, path: target };
}

/**
 * Give the verified file its name without replacing anything there. A hard
 * link fails when the target exists, so a file that appears after the
 * export's first check is never overwritten. On a filesystem without hard
 * links the export checks again and renames, which leaves a short window
 * open.
 * @param {string} partial the verified file
 * @param {string} target the name it gets
 * @param {(existing: string, created: string) => Promise<void>} [linkFile] fs.link, or a stand-in for tests
 */
export async function publishExclusive(partial, target, linkFile = link) {
  try {
    await linkFile(partial, target);
  } catch (error) {
    if (error.code === "EEXIST" || (await pathExists(target))) throw targetAppeared(target);
    await rename(partial, target);
    return;
  }
  await rm(partial, { force: true }).catch(() => {});
}

async function syncDirectory(directory) {
  if (process.platform === "win32") return;
  try {
    const handle = await open(directory, "r");
    try {
      await handle.sync();
    } finally {
      await handle.close();
    }
  } catch {
    // Best effort: the provider still holds every byte, so a lost rename
    // only means running the export again.
  }
}

async function pathExists(target) {
  try {
    await lstat(target);
    return true;
  } catch (error) {
    if (error.code === "ENOENT") return false;
    throw error;
  }
}

function targetAppeared(target) {
  const error = new Error(
    `continuity export target appeared while the export ran; it is never overwritten: ${target}`,
  );
  error.code = "EEXIST";
  error.path = target;
  return error;
}

// -- Reading the file --------------------------------------------------------

async function readExport(path, handle, limit) {
  const fail = (reason) => {
    throw new ContinuityExportFileError(path, reason);
  };
  const lines = new LineReader(handle);
  const headerLine = await lines.next(MAX_HEADER_LINE_BYTES);
  if (headerLine === null || !headerLine.terminated || headerLine.overlong) {
    if (headerLine?.overlong) fail(`the header line is longer than ${MAX_HEADER_LINE_BYTES} bytes`);
    fail("the file must start with a header line that ends in a newline");
  }
  const header = canonicalJson(headerLine.bytes, "header", fail);
  if (!isPlainObject(header)) fail("the header is not an object");
  if (header.format !== CONTINUITY_EXPORT_FORMAT) {
    fail(
      `the header format is ${JSON.stringify(header.format)}, not ${JSON.stringify(CONTINUITY_EXPORT_FORMAT)}`,
    );
  }
  if (header.version !== CONTINUITY_EXPORT_VERSION) {
    fail(
      `unsupported version ${JSON.stringify(header.version)}; this script reads version ` +
        `${CONTINUITY_EXPORT_VERSION}`,
    );
  }
  if (!hasExactKeys(header, HEADER_KEYS)) {
    fail(`the header keys must be exactly ${JSON.stringify([...HEADER_KEYS].sort())}`);
  }
  const writer = header.writer;
  if (typeof writer !== "string" || writer.length === 0 || LONE_SURROGATE.test(writer)) {
    fail("the header writer must be a non-empty string");
  }
  for (const field of ["entries", "entries_length"]) {
    if (!isU64(header[field])) {
      fail(`the header ${field} ${JSON.stringify(header[field])} is not an unsigned 64-bit integer`);
    }
  }
  if (header.entries === 0) fail("the header lists no entries");
  const entriesSha256 = header.entries_sha256;
  if (typeof entriesSha256 !== "string" || !SHA256_HEX.test(entriesSha256)) {
    fail("the header entries_sha256 is not a lowercase SHA-256 hex digest");
  }

  // Every line is read, hashed and counted even after an entry fails its
  // checks: when the header no longer describes the file, the damage is what
  // to report.
  lines.startHashing();
  /** @type {ContinuityExportEntry[]} */
  const entries = [];
  const sessions = new Map();
  let count = 0;
  let problem = null;
  for (let line = await lines.next(limit); line !== null; line = await lines.next(limit)) {
    const index = count;
    count += 1;
    if (line.overlong) problem ??= `entry ${index} is longer than ${limit} bytes`;
    else if (!line.terminated) problem ??= `entry ${index} does not end in a newline`;
    if (problem !== null) continue;
    try {
      const entry = parseEntry(line.bytes, index, fail);
      const previous = entries.at(-1);
      if (previous !== undefined && compareCodePoints(previous.identity, entry.identity) >= 0) {
        fail(
          `entry ${index} (${JSON.stringify(entry.identity)}) breaks the identity order ` +
            "(strictly ascending by code point)",
        );
      }
      if (entry.record !== null) {
        if (sessions.has(entry.record.sessionId)) {
          fail(
            `entry ${index} session ${entry.record.sessionId} is also bound to ` +
              JSON.stringify(sessions.get(entry.record.sessionId)),
          );
        }
        sessions.set(entry.record.sessionId, entry.identity);
      }
      entries.push(entry);
    } catch (error) {
      if (!(error instanceof ContinuityExportFileError)) throw error;
      problem = error.reason;
    }
  }

  if (lines.length !== header.entries_length) {
    fail(`the entries are ${lines.length} bytes, the header's entries_length says ${header.entries_length}`);
  }
  if (lines.hash.digest("hex") !== entriesSha256) fail("the entries do not match the header's entries_sha256");
  if (count !== header.entries) fail(`the file holds ${count} entry lines, the header says ${header.entries}`);
  if (problem !== null) fail(problem);
  return { path, version: CONTINUITY_EXPORT_VERSION, writer, entriesSha256, entries, missingSnapshots: [] };
}

/** Lines of a file, each read whole only when it is at most a given length. */
class LineReader {
  constructor(handle) {
    this.handle = handle;
    this.buffer = Buffer.alloc(0);
    this.ended = false;
    this.hash = null;
    this.length = 0;
  }

  /** Hash and count every byte from here on. */
  startHashing() {
    this.hash = createHash("sha256");
  }

  /**
   * The next line without its newline: `bytes` is null when the line is
   * longer than `limit` (`overlong`), and `terminated` is false for a last
   * line with no newline. Null at the end of the file.
   */
  async next(limit) {
    const pieces = [];
    let size = 0;
    let overlong = false;
    let started = false;
    for (;;) {
      if (this.buffer.length === 0) {
        if (this.ended) break;
        await this.fill();
        continue;
      }
      started = true;
      const newline = this.buffer.indexOf(0x0a);
      const end = newline < 0 ? this.buffer.length : newline;
      const piece = this.buffer.subarray(0, end);
      this.consume(newline < 0 ? end : end + 1);
      if (!overlong && size + piece.length > limit) {
        overlong = true;
        pieces.length = 0;
      }
      if (!overlong) {
        pieces.push(piece);
        size += piece.length;
      }
      if (newline >= 0) return { bytes: overlong ? null : Buffer.concat(pieces, size), terminated: true, overlong };
    }
    if (!started) return null;
    return { bytes: overlong ? null : Buffer.concat(pieces, size), terminated: false, overlong };
  }

  async fill() {
    const chunk = Buffer.allocUnsafe(READ_CHUNK_BYTES);
    const { bytesRead } = await this.handle.read(chunk, 0, chunk.length, null);
    if (bytesRead === 0) this.ended = true;
    else this.buffer = chunk.subarray(0, bytesRead);
  }

  consume(count) {
    if (this.hash !== null) {
      this.hash.update(this.buffer.subarray(0, count));
      this.length += count;
    }
    this.buffer = this.buffer.subarray(count);
  }
}

function parseEntry(raw, index, fail) {
  const item = canonicalJson(raw, `entry ${index}`, fail);
  if (!isPlainObject(item)) fail(`entry ${index} is not an object`);
  const identity = item.identity;
  if (typeof identity !== "string") fail(`entry ${index} identity is missing or not a string`);
  const problem = identityProblem(identity);
  if (problem !== null) fail(`entry ${index} identity ${JSON.stringify(identity)} ${problem}`);
  const state = item.state;
  if (state === "uninitialized") {
    if (!hasExactKeys(item, UNINITIALIZED_KEYS)) {
      fail(`entry ${index} keys must be exactly ${JSON.stringify([...UNINITIALIZED_KEYS].sort())}`);
    }
    return { identity, state, record: null, documentLength: null, documentSha256: null };
  }
  if (state !== "ready") fail(`entry ${index} has unknown state ${JSON.stringify(state)}`);
  if (!hasExactKeys(item, READY_KEYS)) {
    fail(`entry ${index} keys must be exactly ${JSON.stringify([...READY_KEYS].sort())}`);
  }
  if (!isPlainObject(item.record) || !hasExactKeys(item.record, RECORD_KEYS)) {
    fail(`entry ${index} record keys must be exactly ${JSON.stringify([...RECORD_KEYS].sort())}`);
  }
  const record = parseRecord(item.record);
  if (typeof record === "string") fail(`entry ${index} ${record}`);
  if (!SESSION_ID.test(item.record.session_id)) {
    fail(`entry ${index} session_id ${JSON.stringify(item.record.session_id)} is not in canonical form`);
  }
  if (record.identity !== identity) {
    fail(`entry ${index} record names identity ${JSON.stringify(record.identity)}`);
  }
  const [documentLength, documentSha256] = parseDocument(item.document, index, fail);
  return { identity, state: "ready", record, documentLength, documentSha256 };
}

function parseDocument(raw, index, fail) {
  if (!isPlainObject(raw) || !hasExactKeys(raw, DOCUMENT_KEYS)) {
    fail(`entry ${index} document keys must be exactly ${JSON.stringify([...DOCUMENT_KEYS].sort())}`);
  }
  const { length, sha256, base64 } = raw;
  if (typeof base64 !== "string") fail(`entry ${index} document base64 is not a string`);
  if (base64.length % 4 !== 0 || !BASE64_CHARACTERS.test(base64)) {
    fail(`entry ${index} document base64 does not decode`);
  }
  const document = Buffer.from(base64, "base64");
  if (document.toString("base64") !== base64) {
    fail(`entry ${index} document base64 is not in canonical padded form`);
  }
  if (!isU64(length) || length === 0 || document.length !== length) {
    fail(`entry ${index} document is ${document.length} bytes, its length says ${JSON.stringify(length)}`);
  }
  if (typeof sha256 !== "string" || !SHA256_HEX.test(sha256)) {
    fail(`entry ${index} document sha256 is not a lowercase SHA-256 hex digest`);
  }
  if (sha256Hex(document) !== sha256) fail(`entry ${index} document bytes do not match their sha256`);
  return [length, sha256];
}

/**
 * Parse one line and require the writers' compact form. Re-encoding must give
 * back the exact text, which refuses repeated keys, whitespace, unneeded
 * escapes and non-integer numbers in one check.
 */
function canonicalJson(raw, part, fail) {
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(raw);
  } catch {
    fail(`the ${part} is not valid UTF-8`);
  }
  let value;
  let inexact;
  let canonical;
  try {
    value = JSON.parse(text);
    inexact = inexactInteger(value, null);
    canonical = JSON.stringify(value);
  } catch (error) {
    if (error instanceof RangeError) fail(`the ${part} is nested too deeply to check`);
    fail(`the ${part} is not JSON: ${describe(error)}`);
  }
  // Checked before the canonical form, which such an integer also fails:
  // the file may be intact, and only this script cannot tell.
  if (inexact !== null) {
    fail(
      `the ${part} field ${inexact} is above 2^53 - 1, the largest integer this script holds exactly; ` +
        "verify the file with the Python script or the migration verb",
    );
  }
  if (canonical !== text) fail(`the ${part} is not in canonical compact form`);
  return value;
}

/** The path of the first integer above Number.MAX_SAFE_INTEGER in `value`, or null. */
function inexactInteger(value, path) {
  if (typeof value === "number") {
    return Number.isInteger(value) && value > Number.MAX_SAFE_INTEGER ? path ?? "(the value)" : null;
  }
  if (value === null || typeof value !== "object") return null;
  for (const [key, item] of Object.entries(value)) {
    const found = inexactInteger(item, path === null ? key : `${path}.${key}`);
    if (found !== null) return found;
  }
  return null;
}

// -- Shared checks -----------------------------------------------------------

/** Why `value` is not a valid AgentIdentity, or null. */
function identityProblem(value) {
  if (value.length === 0) return "is empty";
  for (const character of value) {
    if (WHITE_SPACE.has(character)) return "contains whitespace";
  }
  if (value.includes("/")) return "contains a slash";
  if (LONE_SURROGATE.test(value)) return "is not valid Unicode";
  return null;
}

function isU64(value) {
  // Exact JavaScript integers only; larger values cannot round-trip here.
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function hasExactKeys(value, keys) {
  const present = Object.keys(value);
  return present.length === keys.length && keys.every((key) => present.includes(key));
}

/** Order by Unicode code point (UTF-8 byte order), not UTF-16 code unit. */
function compareCodePoints(left, right) {
  return Buffer.compare(Buffer.from(left, "utf8"), Buffer.from(right, "utf8"));
}

function sortByCodePoint(values) {
  return values.sort(compareCodePoints);
}

function sha256Hex(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function refusal(rowClass, identity, detail, sessionId = null) {
  return { rowClass, identity, detail, sessionId, action: ACTIONS[rowClass] };
}

function describe(error) {
  return error instanceof Error ? `${error.name}: ${error.message}` : String(error);
}

// -- Command line ------------------------------------------------------------

class UsageError extends Error {}

function parseArguments(argv) {
  const options = {
    verify: [],
    provider: null,
    identities: [],
    identitiesFile: null,
    output: null,
    missingSnapshot: null,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    const value = () => {
      index += 1;
      if (index >= argv.length) throw new UsageError(`${argument} needs a value`);
      return argv[index];
    };
    switch (argument) {
      case "--verify":
        while (index + 1 < argv.length && !argv[index + 1].startsWith("--")) {
          index += 1;
          options.verify.push(argv[index]);
        }
        if (options.verify.length === 0) throw new UsageError("--verify needs at least one file");
        break;
      case "--provider":
        options.provider = value();
        break;
      case "--identity":
        options.identities.push(value());
        break;
      case "--identities-file":
        options.identitiesFile = value();
        break;
      case "--output":
        options.output = value();
        break;
      case "--missing-snapshot":
        options.missingSnapshot = value();
        if (!MISSING_SNAPSHOT_POLICIES.includes(options.missingSnapshot)) {
          throw new UsageError(
            `--missing-snapshot must be ${MISSING_SNAPSHOT_POLICIES.join(" or ")}, not ${options.missingSnapshot}`,
          );
        }
        break;
      case "--help":
      case "-h":
        options.help = true;
        break;
      default:
        throw new UsageError(`unknown argument ${argument}`);
    }
  }
  return options;
}

/**
 * Load the provider named by `PATH[#EXPORT]`: the export is the provider
 * itself, or a function (sync or async) that takes no arguments and returns it.
 */
// The module loader's own refusals, raised before any provider code runs.
const MODULE_LOADER_ERRORS = new Set([
  "ERR_MODULE_NOT_FOUND",
  "ERR_UNKNOWN_FILE_EXTENSION",
  "ERR_UNSUPPORTED_DIR_IMPORT",
  "ERR_IMPORT_ATTRIBUTE_MISSING",
  "ERR_IMPORT_ASSERTION_TYPE_MISSING",
  "ERR_UNKNOWN_MODULE_FORMAT",
  "ERR_INVALID_MODULE_SPECIFIER",
  "ERR_PACKAGE_PATH_NOT_EXPORTED",
  "ERR_UNSUPPORTED_ESM_URL_SCHEME",
]);

export async function loadProvider(spec) {
  const hash = spec.lastIndexOf("#");
  const modulePath = hash >= 0 ? spec.slice(0, hash) : spec;
  const exportName = hash >= 0 ? spec.slice(hash + 1) : "default";
  if (!modulePath || !exportName) throw new UsageError(`--provider must be PATH[#EXPORT], not ${spec}`);
  let module;
  try {
    module = await import(pathToFileURL(resolve(process.cwd(), modulePath)).href);
  } catch (error) {
    // A module the loader cannot find, parse or load is a usage error; one
    // that throws while it runs is a provider fault, and may succeed again.
    if (MODULE_LOADER_ERRORS.has(error?.code) || error instanceof SyntaxError) {
      const hint = /\.[cm]?ts$/.test(modulePath) ? " (run this script under tsx to load a TypeScript provider)" : "";
      throw new UsageError(`cannot load ${modulePath}: ${error.message}${hint}`);
    }
    throw new ContinuityExportProviderError("module import", null, error);
  }
  if (!(exportName in module)) throw new UsageError(`${modulePath} has no export ${exportName}`);
  let provider = module[exportName];
  if (typeof provider?.resolveMany !== "function" && typeof provider === "function") {
    try {
      provider = await provider();
    } catch (error) {
      throw new ContinuityExportProviderError("factory", null, error);
    }
  }
  return provider;
}

const BOM = "\ufeff";

/**
 * The identities an identities file lists, one per line. The file is UTF-8,
 * and a byte order mark at its start is dropped. Lines end at "\n"; Unicode
 * White_Space (the set the identity rules use, which includes the "\r" of a
 * CRLF line end) is trimmed from both ends of each line, and lines left empty
 * are skipped. A byte order mark anywhere else is refused: it is invisible,
 * and no identity an operator meant contains it. The Python script reads
 * identities files by the same rule.
 * @param {string} path
 * @returns {Promise<string[]>}
 */
export async function readIdentitiesFile(path) {
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(await readFile(path));
  } catch (error) {
    if (error instanceof TypeError) throw new UsageError(`${path} is not valid UTF-8`);
    throw error;
  }
  if (text.startsWith(BOM)) text = text.slice(1);
  const identities = [];
  for (const [index, line] of text.split("\n").entries()) {
    const identity = trimWhiteSpace(line);
    if (identity.length === 0) continue;
    if (identity.includes(BOM)) {
      throw new UsageError(`${path} line ${index + 1} contains U+FEFF (a byte order mark); remove it`);
    }
    identities.push(identity);
  }
  return identities;
}

function trimWhiteSpace(value) {
  let start = 0;
  let end = value.length;
  while (start < end && WHITE_SPACE.has(value[start])) start += 1;
  while (end > start && WHITE_SPACE.has(value[end - 1])) end -= 1;
  return value.slice(start, end);
}

function describeExport(exported) {
  const ready = exported.entries.filter((entry) => entry.state === "ready").length;
  const lines = [
    `${exported.path}: ${CONTINUITY_EXPORT_FORMAT} version ${exported.version}, ` +
      `writer ${exported.writer}, entries sha256 ${exported.entriesSha256}`,
    `  ${ready} ready, ${exported.entries.length - ready} uninitialized`,
  ];
  for (const entry of exported.entries) {
    if (entry.state === "ready") {
      lines.push(
        `  ready          ${entry.identity}  session ${entry.record.sessionId}  ` +
          `${entry.documentLength} bytes  sha256 ${entry.documentSha256}`,
      );
    } else {
      lines.push(`  uninitialized  ${entry.identity}`);
    }
  }
  return lines.join("\n");
}

function describeMissingSnapshots(exported) {
  const missing = exported.missingSnapshots;
  if (missing.length === 0) return [];
  return [
    `  ${missing.length} written fresh by --missing-snapshot fresh (uninitialized, with an empty transcript):`,
    ...missing.map(
      (record) =>
        `  fresh          ${record.identity}  session ${record.sessionId}  ` +
        `(${recordValues(record)}): transcript not carried, ` +
        "because 0.9.3 never saved a snapshot of this session",
    ),
  ];
}

const USAGE =
  "usage: node continuity-export.mjs --provider PATH[#EXPORT] " +
  "(--identity ID ... | --identities-file FILE) --output FILE [--missing-snapshot refuse|fresh]\n" +
  "       node continuity-export.mjs --verify FILE [FILE ...]";

/**
 * Run the command line; returns the exit status.
 * @param {string[]} argv the arguments after the script name
 */
export async function main(argv) {
  let options;
  try {
    options = parseArguments(argv);
  } catch (error) {
    console.error(`${error.message}\n${USAGE}`);
    return EXIT_USAGE;
  }
  if (options.help) {
    console.log(USAGE);
    return 0;
  }
  if (options.verify.length > 0) {
    if (
      options.provider ||
      options.identities.length ||
      options.identitiesFile ||
      options.output ||
      options.missingSnapshot !== null
    ) {
      console.error(`--verify takes only files\n${USAGE}`);
      return EXIT_USAGE;
    }
    let status = 0;
    for (const path of options.verify) {
      try {
        console.log(describeExport(await readContinuityExport(path)));
      } catch (error) {
        console.error(error instanceof ContinuityExportFileError ? `invalid: ${error.message}` : `cannot read ${path}: ${error.message}`);
        status = EXIT_INVALID_FILE;
      }
    }
    return status;
  }
  if (!options.provider || !options.output) {
    console.error(`an export needs --provider and --output (or use --verify)\n${USAGE}`);
    return EXIT_USAGE;
  }
  try {
    const identities = [...options.identities];
    for (const identity of identities) {
      if (identity.includes(BOM)) {
        throw new UsageError(`--identity ${JSON.stringify(identity)} contains U+FEFF (a byte order mark); remove it`);
      }
    }
    if (options.identitiesFile) {
      try {
        identities.push(...(await readIdentitiesFile(options.identitiesFile)));
      } catch (error) {
        throw new UsageError(`cannot read --identities-file: ${error.message}`);
      }
    }
    if (identities.length === 0) throw new UsageError("list the identities with --identity or --identities-file");
    const provider = await loadProvider(options.provider);
    const exported = await exportContinuity(provider, identities, options.output, {
      missingSnapshot: options.missingSnapshot ?? MISSING_SNAPSHOT_REFUSE,
    });
    console.log([describeExport(exported), ...describeMissingSnapshots(exported)].join("\n"));
    return 0;
  } catch (error) {
    if (error instanceof ContinuityExportRefusedError) {
      const rows = error.refusals.map(
        (row) =>
          `  class=${row.rowClass} identity=${row.identity ?? "-"} session=${row.sessionId ?? "-"}\n` +
          `    detail: ${row.detail}\n` +
          `    action: ${row.action}`,
      );
      console.error(["continuity export refused; nothing was written:", ...rows].join("\n"));
      return EXIT_REFUSED;
    }
    if (error instanceof ContinuityExportProviderError) {
      console.error(error.message);
      return EXIT_PROVIDER;
    }
    // Only a refusal before anything was read is "not started"; any other
    // failure came after the export started, and it wrote nothing.
    const notStarted =
      error instanceof UsageError ||
      error instanceof ArgumentTypeError ||
      error instanceof ArgumentRangeError ||
      error instanceof TargetExistsError;
    console.error(`continuity export ${notStarted ? "not started" : "failed, nothing written"}: ${error.message}`);
    return EXIT_USAGE;
  }
}

function invokedDirectly() {
  if (!process.argv[1]) return false;
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

if (invokedDirectly()) {
  process.exitCode = await main(process.argv.slice(2));
}
