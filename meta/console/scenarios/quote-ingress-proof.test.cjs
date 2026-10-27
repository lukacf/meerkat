"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { boundaryRecords, expectedContextContent, assertExactContextMessage, serializeConsoleContextMessage } = require("./quote-ingress-proof.cjs");

test("shipping serializer accepts eight records at the complete 64 KiB UTF-8 boundary", () => {
  const records = boundaryRecords("http://127.0.0.1:12345/__context-source-must-not-fetch");
  assert.equal(records.length, 8);
  assert.equal(Buffer.byteLength(JSON.stringify(records)), 65_536);
  const instruction = "  Compare these snapshots.\nPreserve the exact source.  ";
  const actual = serializeConsoleContextMessage(instruction, records);
  const expected = expectedContextContent(instruction, records);
  assertExactContextMessage({ role: "user", content: actual }, expected);
  assert.equal(actual[1].text.split("\n").length, 4, "hostile quote delimiters remain JSON data");
  assert.equal(JSON.parse(actual[1].text.split("\n")[2]).quote, records[0].quote);
});

test("ninth quote and one additional UTF-8 byte fail before any dispatch", () => {
  const records = boundaryRecords("local snapshot");
  let dispatches = 0;
  const send = values => { const content = serializeConsoleContextMessage("Compare", values); dispatches++; return content; };
  assert.throws(() => send([...records, { ...records[7], id: "quote-8" }]), /at most 8 quotes/);
  assert.throws(() => send(records.map((record, index) => index === 0 ? { ...record, quote: record.quote + "x" } : record)), /64 KiB/);
  assert.equal(dispatches, 0);
});

test("final ingress oracle rejects substring-only, reordered, normalized and extra context", () => {
  const records = boundaryRecords("local snapshot");
  const expected = expectedContextContent("  Compare\nexactly  ", records);
  const changed = mutate => { const value = structuredClone(expected); mutate(value); return { role: "user", content: value }; };
  for (const message of [
    { role: "assistant", content: expected },
    { role: "user", content: JSON.stringify(expected) },
    changed(value => { [value[1], value[2]] = [value[2], value[1]]; }),
    changed(value => { value[0].text = value[0].text.trim(); }),
    changed(value => { value[1].text = value[1].text.normalize("NFC"); }),
    changed(value => { value.push({ type: "text", text: "extra" }); }),
  ]) assert.throws(() => assertExactContextMessage(message, expected));
});
