/**
 * A continuity provider over two JSON Lines files, for continuity-export.mjs.
 *
 * `continuity-export.mjs` reads a 0.9.3 continuity provider object. A host
 * whose 0.9.3 continuity records and session snapshots sit in its own tables,
 * rather than behind a provider object it can import, writes them to two files
 * and names this module as the provider instead of writing one:
 *
 *     CONTINUITY_EXPORT_RECORDS=records.jsonl \
 *     CONTINUITY_EXPORT_SESSIONS=sessions.jsonl \
 *     node continuity-export.mjs --provider ./jsonl-provider.mjs#fromEnv \
 *         --identities-file identities.txt --output continuity-export.jsonl
 *
 * `--provider` takes a path relative to the current directory. This is the
 * provider `jsonl_provider.py` is for `continuity_export.py`: both read the
 * same files by the same rules, and the module docstring there describes
 * them. In short, both files are UTF-8 JSON Lines, one JSON object per line;
 * a byte order mark at the start of a file and lines that hold only
 * whitespace are skipped.
 *
 * - The records file holds each identity's current 0.9.3 continuity record,
 *   one line per identity, with exactly the keys `identity`,
 *   `agent_runtime_id`, `session_id`, `generation` and `checkpoint_version`.
 * - The sessions file holds session snapshots, one line per session, with
 *   exactly the keys `session_id` and `data`: the snapshot's exact bytes in
 *   standard base64, with padding and without line breaks.
 *
 * `generation` and `checkpoint_version` may be JSON integers or strings of
 * decimal digits. This script holds integers exactly up to 2^53 - 1, as
 * `continuity-export.mjs` does; a file with a larger one is exported with the
 * Python scripts.
 *
 * `resolveMany` answers for every identity the records file holds, listed or
 * not, so an identity list that misses one is refused (`Unlisted`); a listed
 * identity the file holds no record for is `uninitialized`.
 * `loadSessionSnapshot` answers from the sessions file, and the export asks
 * only for the session each record names. `resolveRecordBySession` answers
 * from the records file, with no fencing token (`null`).
 *
 * A file this provider cannot use stops the export before it reads a record,
 * and the error names the file and the line. Opening the provider reads the
 * records file and scans the sessions file once, keeping where each session's
 * line starts, not the snapshots.
 *
 * Node.js 18 or later, no packages.
 */

import { closeSync, openSync, readSync } from "node:fs";

export const RECORDS_ENV = "CONTINUITY_EXPORT_RECORDS";
export const SESSIONS_ENV = "CONTINUITY_EXPORT_SESSIONS";
export const RECORD_KEYS = ["identity", "agent_runtime_id", "session_id", "generation", "checkpoint_version"];
export const SESSION_KEYS = ["session_id", "data"];

const INTEGER_FIELDS = ["generation", "checkpoint_version"];
const DECIMAL_DIGITS = /^(?:0|[1-9][0-9]*)$/;
const INTEGER_LITERAL = /^-?(?:0|[1-9][0-9]*)$/;
// Every spelling Core 0.9.3's SessionId parsed: simple, hyphenated, braced and
// urn, with hex digits in either case (continuity-export.mjs has the same rule).
const HYPHENATED = "[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}";
const SESSION_ID_SPELLINGS = new RegExp(
  `^(?:[0-9A-Fa-f]{32}|${HYPHENATED}|\\{${HYPHENATED}\\}|urn:uuid:${HYPHENATED})$`,
);
// Padded standard base64, checked by length and one flat character class.
const BASE64_CHARACTERS = /^[A-Za-z0-9+/]*={0,2}$/;
const WHITESPACE_LINE = /^[ \t\r\n]*$/;
const BOM = [0xef, 0xbb, 0xbf];
const CHUNK_BYTES = 1 << 20;

/** A records or sessions file the provider cannot use. */
export class JsonlProviderInputError extends Error {
  constructor(message) {
    super(message);
    this.name = "JsonlProviderInputError";
  }
}

/** The 0.9.3 read methods over a records file and a sessions file. */
export class JsonlContinuityProvider {
  #recordsPath;
  #sessionsPath;
  /** @type {Map<string, Record<string, unknown>>} */
  #records = new Map();
  /** @type {Map<string, string[]>} */
  #identitiesBySession = new Map();
  /** Canonical session id -> [offset, length, line number] of its line. */
  #sessions = new Map();

  constructor(recordsPath, sessionsPath) {
    this.#recordsPath = String(recordsPath);
    this.#sessionsPath = String(sessionsPath);
    this.#readRecords();
    this.#indexSessions();
  }

  resolveMany(identities) {
    const answer = Object.create(null);
    for (const [identity, record] of this.#records) answer[identity] = { state: "ready", record: { ...record } };
    for (const identity of identities) {
      if (!Object.prototype.hasOwnProperty.call(answer, identity)) answer[identity] = { state: "uninitialized" };
    }
    return answer;
  }

  loadSessionSnapshot(sessionId) {
    const canonical = canonicalSessionId(sessionId);
    const place = canonical === null ? undefined : this.#sessions.get(canonical);
    if (place === undefined) return null;
    const [offset, length, number] = place;
    const raw = Buffer.alloc(length);
    const fd = openSync(this.#sessionsPath, "r");
    try {
      let done = 0;
      while (done < length) {
        const read = readSync(fd, raw, done, length - done, offset + done);
        if (read === 0) break;
        done += read;
      }
      if (done !== length) fail(this.#sessionsPath, number, "the file changed after the provider opened it");
    } finally {
      closeSync(fd);
    }
    let session;
    try {
      session = sessionLine(this.#sessionsPath, number, raw);
    } catch (error) {
      if (!(error instanceof JsonlProviderInputError)) throw error;
      session = null;
    }
    if (session === null || canonicalSessionId(session.session_id) !== canonical) {
      fail(this.#sessionsPath, number, "the file changed after the provider opened it");
    }
    return { data: Buffer.from(session.data, "base64") };
  }

  resolveRecordBySession(sessionId) {
    const canonical = canonicalSessionId(sessionId);
    const bound = canonical === null ? [] : this.#identitiesBySession.get(canonical) ?? [];
    if (bound.length !== 1) return null;
    const record = this.#records.get(bound[0]);
    return {
      record: {
        identity: record.identity,
        agentRuntimeId: record.agent_runtime_id,
        sessionId: record.session_id,
        generation: record.generation,
        checkpointVersion: record.checkpoint_version,
      },
      fencingToken: null,
      checkpointVersion: record.checkpoint_version,
    };
  }

  #readRecords() {
    const lines = new Map();
    for (const { number, bytes } of fileLines(this.#recordsPath)) {
      const parsed = objectLine(this.#recordsPath, number, bytes, RECORD_KEYS);
      if (parsed === null) continue;
      const { value: record, tokens } = parsed;
      const identity = record.identity;
      if (typeof identity !== "string") fail(this.#recordsPath, number, "identity is not a string");
      if (lines.has(identity)) {
        fail(this.#recordsPath, number, `identity ${quote(identity)} is repeated (first on line ${lines.get(identity)})`);
      }
      lines.set(identity, number);
      for (const field of INTEGER_FIELDS) record[field] = integerField(this.#recordsPath, number, field, record[field], tokens.get(field));
      const kept = {};
      for (const key of RECORD_KEYS) kept[key] = record[key];
      this.#records.set(identity, kept);
      const canonical = canonicalSessionId(record.session_id);
      if (canonical !== null) {
        const bound = this.#identitiesBySession.get(canonical) ?? [];
        bound.push(identity);
        this.#identitiesBySession.set(canonical, bound);
      }
    }
  }

  #indexSessions() {
    for (const { number, offset, bytes } of fileLines(this.#sessionsPath)) {
      const session = sessionLine(this.#sessionsPath, number, bytes);
      if (session === null) continue;
      const canonical = canonicalSessionId(session.session_id);
      const first = this.#sessions.get(canonical);
      if (first !== undefined) fail(this.#sessionsPath, number, `session ${canonical} is repeated (first on line ${first[2]})`);
      this.#sessions.set(canonical, [offset, bytes.length, number]);
    }
  }
}

/** The provider over the files CONTINUITY_EXPORT_RECORDS and CONTINUITY_EXPORT_SESSIONS name. */
export function fromEnv() {
  const paths = [];
  for (const [name, file] of [[RECORDS_ENV, "records"], [SESSIONS_ENV, "sessions"]]) {
    const value = process.env[name] ?? "";
    if (value === "") throw new JsonlProviderInputError(`${name} is not set: set it to the path of the ${file} file`);
    paths.push(value);
  }
  return new JsonlContinuityProvider(...paths);
}

/** A session id in any spelling 0.9.3 accepted, in canonical form; else null. */
export function canonicalSessionId(value) {
  if (typeof value !== "string" || !SESSION_ID_SPELLINGS.test(value)) return null;
  const digits = value.replace(/^urn:uuid:/, "").replace(/[{}-]/g, "").toLowerCase();
  return `${digits.slice(0, 8)}-${digits.slice(8, 12)}-${digits.slice(12, 16)}-${digits.slice(16, 20)}-${digits.slice(20)}`;
}

/**
 * Each line of `path` as { number, offset, bytes }, its line end kept. A byte
 * order mark at the start of the file is not part of the first line.
 */
function* fileLines(path) {
  const fd = openSync(path, "r");
  try {
    const chunk = Buffer.alloc(CHUNK_BYTES);
    let pending = [];
    let position = 0;
    let lineStart = 0;
    let number = 0;
    const emit = (parts) => {
      let bytes = parts.length === 1 ? parts[0] : Buffer.concat(parts);
      let offset = lineStart;
      lineStart += bytes.length;
      number += 1;
      if (number === 1 && bytes.length >= 3 && BOM.every((byte, index) => bytes[index] === byte)) {
        bytes = bytes.subarray(3);
        offset += 3;
      }
      return { number, offset, bytes };
    };
    for (;;) {
      const read = readSync(fd, chunk, 0, CHUNK_BYTES, position);
      if (read === 0) break;
      position += read;
      let from = 0;
      for (;;) {
        const newline = chunk.indexOf(0x0a, from);
        if (newline === -1 || newline >= read) {
          if (from < read) pending.push(Buffer.from(chunk.subarray(from, read)));
          break;
        }
        pending.push(Buffer.from(chunk.subarray(from, newline + 1)));
        yield emit(pending);
        pending = [];
        from = newline + 1;
      }
    }
    if (pending.length > 0) yield emit(pending);
  } finally {
    closeSync(fd);
  }
}

function sessionLine(path, number, bytes) {
  const parsed = objectLine(path, number, bytes, SESSION_KEYS);
  if (parsed === null) return null;
  const session = parsed.value;
  if (canonicalSessionId(session.session_id) === null) {
    fail(path, number, `session_id ${quote(session.session_id)} is not a session id`);
  }
  const data = session.data;
  if (typeof data !== "string" || data.length % 4 !== 0 || !BASE64_CHARACTERS.test(data)) {
    fail(path, number, "data is not standard base64 with padding and without line breaks");
  }
  return session;
}

/**
 * The line's JSON object, checked to hold exactly `keys`, with the raw text of
 * each value; null for a line of whitespace.
 */
function objectLine(path, number, bytes, keys) {
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch (error) {
    if (error instanceof TypeError) fail(path, number, "the line is not valid UTF-8");
    fail(
      path,
      number,
      `the line is longer than this script can read (${bytes.length} bytes); ` +
        "export with the Python scripts, which read any line",
    );
  }
  if (WHITESPACE_LINE.test(text)) return null;
  let value;
  try {
    value = JSON.parse(text);
  } catch (error) {
    if (error instanceof RangeError) fail(path, number, "the line is nested too deeply to read");
    fail(path, number, `the line is not JSON: ${error.message}`);
  }
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(path, number, "the line is not a JSON object");
  }
  // JSON.parse keeps the last of repeated keys; the Python provider refuses
  // them, and so does this one.
  const tokens = new Map();
  for (const [key, raw] of topLevelEntries(text)) {
    if (tokens.has(key)) fail(path, number, `key ${quote(key)} is repeated`);
    tokens.set(key, raw);
  }
  const missing = keys.filter((key) => !tokens.has(key));
  const unexpected = [...tokens.keys()].filter((key) => !keys.includes(key)).sort(codePointOrder);
  if (missing.length > 0 || unexpected.length > 0) {
    const problems = [];
    if (missing.length > 0) problems.push(`missing ${missing.join(", ")}`);
    if (unexpected.length > 0) problems.push(`unexpected ${unexpected.map(quote).join(", ")}`);
    fail(path, number, `the object must have exactly the keys ${keys.join(", ")} (${problems.join("; ")})`);
  }
  return { value, tokens };
}

/**
 * A record's integer field as the Python provider reads it: a JSON integer or a
 * string of decimal digits becomes a number, anything else is handed to the
 * export as the file holds it (and the export refuses it). A float or exponent
 * literal is handed on as its text, since JSON.parse would read 1.0 as 1.
 */
function integerField(path, number, field, value, raw) {
  const digits = typeof value === "string" && DECIMAL_DIGITS.test(value) ? value : null;
  const literal = typeof value === "number" && raw !== undefined && INTEGER_LITERAL.test(raw) ? raw : null;
  if (digits === null && literal === null) {
    return typeof value === "number" && raw !== undefined ? raw : value;
  }
  const integer = Number(digits ?? literal);
  if (!Number.isSafeInteger(integer)) {
    fail(
      path,
      number,
      `${field} ${digits ?? literal} is above 2^53 - 1, the largest integer this script holds exactly; ` +
        "export with the Python scripts",
    );
  }
  return integer;
}

/**
 * The [key, raw value text] pairs of the top-level object in `text`, which
 * JSON.parse has already accepted as an object.
 */
function topLevelEntries(text) {
  const entries = [];
  let at = skipWhitespace(text, text.indexOf("{") + 1);
  if (text[at] === "}") return entries;
  for (;;) {
    at = skipWhitespace(text, at);
    const keyEnd = stringEnd(text, at);
    const key = JSON.parse(text.slice(at, keyEnd));
    at = skipWhitespace(text, skipWhitespace(text, keyEnd) + 1);
    const valueStart = at;
    at = valueEnd(text, at);
    entries.push([key, text.slice(valueStart, at)]);
    at = skipWhitespace(text, at);
    if (text[at] === "}") return entries;
    at += 1;
  }
}

function skipWhitespace(text, at) {
  while (at < text.length && " \t\r\n".includes(text[at])) at += 1;
  return at;
}

/** The index after the string that starts at `at`. */
function stringEnd(text, at) {
  for (let index = at + 1; ; index += 1) {
    const code = text.charCodeAt(index);
    if (code === 0x5c) index += 1;
    else if (code === 0x22) return index + 1;
  }
}

/** The index after the value that starts at `at`. */
function valueEnd(text, at) {
  const first = text[at];
  if (first === '"') return stringEnd(text, at);
  if (first === "{" || first === "[") {
    let depth = 0;
    for (let index = at; ; index += 1) {
      const character = text[index];
      if (character === '"') index = stringEnd(text, index) - 1;
      else if (character === "{" || character === "[") depth += 1;
      else if (character === "}" || character === "]") {
        depth -= 1;
        if (depth === 0) return index + 1;
      }
    }
  }
  let index = at;
  while (index < text.length && !",}] \t\r\n".includes(text[index])) index += 1;
  return index;
}

function codePointOrder(left, right) {
  const a = [...left].map((character) => character.codePointAt(0));
  const b = [...right].map((character) => character.codePointAt(0));
  for (let index = 0; index < Math.min(a.length, b.length); index += 1) {
    if (a[index] !== b[index]) return a[index] - b[index];
  }
  return a.length - b.length;
}

/** `value` in an error message, as JSON (the Python provider's form). */
function quote(value) {
  return JSON.stringify(value);
}

function fail(path, number, problem) {
  throw new JsonlProviderInputError(`${path} line ${number}: ${problem}`);
}
