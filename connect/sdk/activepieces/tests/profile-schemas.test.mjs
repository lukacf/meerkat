import test from 'node:test';
import assert from 'node:assert/strict';
import { createReader, ActivepiecesError } from '../dist/index.js';
import { inputs, select } from '../dist/profiles.js';
import { nextProfiles } from './fixtures/next-profiles.mjs';

test('advertised string patterns and runtime checks agree on complete input including line terminators', () => {
  const definitions = [
    { profile: 'tally.get-form', auth: () => ({ apiKey: 'selected' }), args: { form_id: 'form_1' } },
    { profile: 'asana.get-task', auth: () => ({ accessToken: 'selected' }), args: { task: '123' } },
    { profile: 'nocodb.get-record', auth: () => ({ origin: 'https://noco.invalid', apiToken: 'selected', version: 4 }), args: { baseId: 'base_1', tableId: 'table_1', recordId: 1 } },
    ...nextProfiles,
  ];
  for (const definition of definitions) {
    const configuration = { profile: definition.profile, auth: definition.auth('selected'), transport: async () => { throw new Error('Schema test must not dispatch'); } };
    const properties = createReader(configuration).describe().inputSchema.properties;
    const selection = select(configuration);
    for (const [key, schema] of Object.entries(properties)) {
      if (schema.type !== 'string') continue;
      const pattern = new RegExp(schema.pattern);
      assert.equal(pattern.test(definition.args[key]), true, `${definition.profile}/${key} valid input`);
      for (const ending of ['\n', '\r', '\r\n', '\u2028', '\u2029']) {
        const value = definition.args[key] + ending;
        // Coda names allow these well-formed Unicode separators, while CR/LF
        // remain excluded control characters. Its document ID is ASCII-only.
        const allowed = definition.profile === 'coda.get-row' && key !== 'docId' && (ending === '\u2028' || ending === '\u2029');
        const label = `${definition.profile}/${key}/${JSON.stringify(ending)}`;
        assert.equal(pattern.test(value), allowed, label);
        const arguments_ = { ...definition.args, [key]: value };
        if (allowed) {
          assert.doesNotThrow(() => inputs(selection, arguments_), label);
        } else {
          assert.throws(() => inputs(selection, arguments_), error => error instanceof ActivepiecesError && error.kind === 'invalid_arguments', label);
        }
      }
    }
  }
});

test('Coda schema accepts Unicode scalar names and rejects unpaired surrogates with or without Unicode regex mode', () => {
  const definition = nextProfiles.find(item => item.profile === 'coda.get-row');
  const configuration = { profile: definition.profile, auth: definition.auth('selected'), transport: async () => { throw new Error('Schema test must not dispatch'); } };
  const properties = createReader(configuration).describe().inputSchema.properties;
  const selection = select(configuration);
  const cases = [
    ['normal / name', true], ['\u{1f600}', true], ['x\u{1f600}y', true], ['\u{1f600}'.repeat(256), true],
    ['\ud800', false], ['\udfff', false], ['x\ud800y', false], ['\ud800\ud800', false], ['\udfff\ud800', false],
  ];
  for (const key of ['tableId', 'rowIdOrName']) {
    for (const [value, allowed] of cases) {
      for (const flags of ['', 'u']) {
        assert.equal(new RegExp(properties[key].pattern, flags).test(value), allowed, `${key}/${JSON.stringify(value)}/${flags}`);
      }
      const arguments_ = { ...definition.args, [key]: value };
      if (allowed) assert.doesNotThrow(() => inputs(selection, arguments_));
      else assert.throws(() => inputs(selection, arguments_));
    }
  }
});
