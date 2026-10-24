// The Web SDK hands a tool callback the call's dedupe key, which the WASM
// runtime passes as an optional second argument (a JSON string). Unknown
// fields are ignored; a missing or malformed key is no key, never a failed
// call. Unrun until the candidate's validation.
import assert from 'node:assert/strict';
import test from 'node:test';
import { MeerkatRuntime } from '../dist/runtime.js';
import { parseToolCallDedupeKey } from '../dist/types.js';

const wireKey = () => ({
  run_id: '0b9f3c55-6b8e-4f5c-9a51-2a7d1c4e8f10',
  round: 2,
  call_id: 'call-1',
  call_digest: 'a'.repeat(64),
});

test('a dedupe key parses from its JSON form, ignoring unknown fields', () => {
  assert.deepEqual(parseToolCallDedupeKey(JSON.stringify(wireKey())), wireKey());
  assert.deepEqual(
    parseToolCallDedupeKey(JSON.stringify({ ...wireKey(), later_field: true })),
    wireKey(),
  );
});

test('an absent or malformed dedupe key is no key', () => {
  const { call_digest: _omitted, ...withoutDigest } = wireKey();
  for (const raw of [
    undefined,
    null,
    '',
    'not json',
    '[]',
    '{}',
    JSON.stringify(withoutDigest),
    JSON.stringify({ ...wireKey(), round: -1 }),
    JSON.stringify({ ...wireKey(), round: '2' }),
    wireKey(),
  ]) {
    assert.equal(parseToolCallDedupeKey(raw), undefined, String(raw));
  }
});

test('a registered tool callback receives the key, or undefined without one', async () => {
  let wrapper;
  const wasm = {
    register_tool_callback: (_name, _description, _schema, callback) => {
      wrapper = callback;
    },
  };
  const seen = [];
  MeerkatRuntime.registerTool(wasm, 'search', 'Search', { type: 'object' }, async (args, dedupeKey) => {
    seen.push([JSON.parse(args), dedupeKey]);
    return { content: 'done', is_error: false };
  });

  const args = JSON.stringify({ q: 'meerkat' });
  assert.deepEqual(JSON.parse(await wrapper(args, JSON.stringify(wireKey()))), {
    content: 'done',
    is_error: false,
  });
  await wrapper(args);
  await wrapper(args, 'not json');
  assert.deepEqual(seen, [
    [{ q: 'meerkat' }, wireKey()],
    [{ q: 'meerkat' }, undefined],
    [{ q: 'meerkat' }, undefined],
  ]);
});
