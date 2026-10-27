import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp, writeFile, rm} from 'node:fs/promises';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL} from 'node:url';
import {spawnSync} from 'node:child_process';
import {authenticationGuidance} from '../dist/auth-guidance.js';
import {configurationHelp, listProfiles} from '../dist/config-fields.js';
import {operationConfiguration} from '../dist/mcp-config.js';
import {configurations} from './fixtures/config-assistance.mjs';

const KINDS = new Set(['account', 'client_authentication', 'administrator',
  'developer_registration', 'host_deployment', 'artifact_preparation', 'application_binding']);
const TOKEN = 'fixture-selected-credential-never-in-help';
const MEANINGS = {
  apiKey: /API key/i, apiToken: /API token/i, accessToken: /access token|internal integration token/i,
  serverToken: /server token/i, databaseToken: /database token/i, apiSecret: /API secret/i,
  personalToken: /personal token/i, applicationPassword: /application password/i,
  adminApiKey: /Admin API key/i, secretAccessKey: /secret access key/i,
  serviceRoleKey: /service role key/i,
};

test('all 63 profiles explain the exact credential selected by the existing operation mapping', () => {
  assert.equal(configurations.length, 63);
  assert.deepEqual(configurations.map(item => item.profile).sort(), listProfiles().profiles.map(item => item.profile).sort());
  for (const config of configurations) {
    const selection = operationConfiguration(config, TOKEN, async () => { throw new Error('Provider forbidden'); });
    const bound = Object.entries(selection.auth).filter(([, value]) => value === TOKEN);
    assert.equal(bound.length, 1, config.profile);
    const meaning = MEANINGS[bound[0][0]];
    assert.ok(meaning, config.profile);
    const help = configurationHelp(config.profile);
    const authentication = help.prerequisites.filter(item => item.kind === 'client_authentication');
    assert.equal(authentication.length, 1, config.profile);
    assert.match(authentication[0].instruction, meaning, config.profile);
    assert.match(authentication[0].instruction, /token_env names an environment variable/, config.profile);
    assert.match(authentication[0].instruction, /not for the MCP client-to-server connection/, config.profile);
    assert.ok(help.prerequisites.some(item => item.kind === 'account'), config.profile);
    assert.ok(help.prerequisites.some(item => item.kind === 'application_binding'), config.profile);
    for (const row of help.prerequisites) {
      assert.deepEqual(Object.keys(row).sort(), ['instruction', 'kind']);
      assert.ok(KINDS.has(row.kind));
      assert.equal(typeof row.instruction, 'string');
      assert.ok(row.instruction.length > 0);
    }
    assert.ok(help.prerequisites.some(row => row.kind === 'developer_registration'), config.profile);
    assert.ok(help.prerequisites.some(row => /checked 2026-10-03/i.test(row.instruction)), config.profile);
    assert.ok(help.unresolved_details.some(value => value.includes('unverified')), config.profile);
    assert.ok(help.unresolved_details.some(value => /do(?:es)? not verify/.test(value)), config.profile);
    assert.ok(!help.unresolved_details.some(value => value.includes('requirements are not established by this configuration help')), config.profile);
    assert.ok(!JSON.stringify(help).includes(TOKEN));
  }
});

test('credentials with similar names retain their distinct bootstrap requirements', () => {
  const text = profile => JSON.stringify(authenticationGuidance(profile));
  assert.match(text('kit.get-subscriber'), /V3 API secret, not a V4 API key/);
  assert.match(text('kit.get-subscriber'), /credential-bearing URL/);
  assert.match(text('wordpress.get-page'), /application password.*not an OAuth token/);
  assert.match(text('baserow.get-row'), /database token.*JWT acquisition are unsupported/);
  assert.match(text('notion.get-page'), /internal integration token/);
  assert.match(text('contentful.get-entry'), /Management access token/);
  assert.match(text('ghost.get-post'), /24-hex-key-id:64-hex-secret/);
  assert.match(text('amazon-s3.list-files'), /secret access key; access_key_id_env.*session_token_env/);
  assert.match(text('amazon-s3.list-files'), /No ambient credential chain, STS, OIDC/);
  assert.match(text('supabase.get-user'), /administrator.*service role key/);
  for (const profile of ['activecampaign.create-contact', 'freshservice.create-ticket', 'gitea.create-issue', 'datadog.submit-log']) {
    assert.equal(configurationHelp(profile).effect, 'write');
    assert.match(text(profile), /host must separately approve the write/);
  }
});

test('nested output mutation cannot contaminate other accounts or later help', () => {
  const original = authenticationGuidance('supabase.get-user');
  const expected = JSON.stringify(original);
  original.prerequisites[0].instruction = TOKEN;
  original.prerequisites.find(item => item.kind === 'administrator').instruction = TOKEN;
  original.prerequisites.push({kind: 'account', instruction: TOKEN});
  original.unresolved_details[0] = TOKEN;
  assert.equal(JSON.stringify(authenticationGuidance('supabase.get-user')), expected);
  assert.ok(!JSON.stringify(configurationHelp('tally.get-form')).includes(TOKEN));
  const help = configurationHelp('kit.get-subscriber');
  help.prerequisites.find(item => item.kind === 'host_deployment').instruction = TOKEN;
  help.prerequisites.length = 0;
  help.unresolved_details.length = 0;
  assert.ok(configurationHelp('kit.get-subscriber').prerequisites.length > 0);
  assert.ok(configurationHelp('kit.get-subscriber').unresolved_details.length > 0);
  assert.ok(!JSON.stringify(configurationHelp('kit.get-subscriber')).includes(TOKEN));
});

test('invalid profiles are rejected without invoking coercion or exposing their values', () => {
  let coerced = false;
  const hostile = {toString() { coerced = true; throw new Error(TOKEN); }};
  for (const input of [TOKEN, 'Tally', '', null, undefined, 7, [], hostile, '__proto__']) {
    assert.throws(() => authenticationGuidance(input), error => {
      assert.equal(error.message, 'Activepieces profile is invalid');
      assert.ok(!String(error).includes(TOKEN));
      return true;
    });
  }
  assert.equal(coerced, false);
});

const guard = `
import {registerHooks} from 'node:module';
import {basename} from 'node:path';
registerHooks({resolve(specifier, context, nextResolve) {
  if (['mcp.js','mcp-http.js','index.js','donor-factory.js','transport.js'].includes(basename(specifier)) ||
      (!specifier.startsWith('.') && !specifier.startsWith('/') && !specifier.startsWith('file:') && !specifier.startsWith('node:')))
    throw new Error('Runtime import forbidden');
  return nextResolve(specifier, context);
}});
process.env = new Proxy({}, {get(_target, key) {
  if (['FORCE_COLOR','NO_COLOR','NODE_DISABLE_COLORS','NODE_V8_COVERAGE','WATCH_REPORT_DEPENDENCIES'].includes(key)) return undefined;
  throw new Error('Environment read forbidden');
}, ownKeys() { throw new Error('Environment enumeration forbidden'); }});
globalThis.fetch = () => { throw new Error('Network forbidden'); };
`;

test('every public config-help command is cold, credential-free and emits only its selected guidance', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'toolkit-auth-guidance-'));
  const hook = join(directory, 'guard.mjs');
  await writeFile(hook, guard);
  const run = args => spawnSync(process.execPath, ['--import', pathToFileURL(hook).href, 'dist/mcp-cli.js', ...args], {
    cwd: new URL('..', import.meta.url), env: {LANG: 'C.UTF-8'}, encoding: 'utf8', timeout: 10000,
  });
  try {
    for (const {profile} of configurations) {
      const result = run(['config-help', '--profile', profile]);
      assert.equal(result.status, 0, `${profile}: ${result.stderr}`);
      assert.equal(result.stderr, '');
      assert.deepEqual(JSON.parse(result.stdout), configurationHelp(profile));
      assert.ok(!result.stdout.includes(TOKEN));
    }
    const rejected = run(['config-help', '--profile', TOKEN]);
    assert.equal(rejected.status, 1);
    assert.equal(rejected.stdout, '');
    assert.ok(!rejected.stderr.includes(TOKEN));
  } finally { await rm(directory, {recursive: true}); }
});
