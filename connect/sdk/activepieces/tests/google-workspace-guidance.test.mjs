import test from 'node:test';
import assert from 'node:assert/strict';
import {authenticationGuidance} from '../dist/auth-guidance.js';
import {configurationHelp, listProfiles} from '../dist/config-fields.js';
import {operationConfiguration} from '../dist/mcp-config.js';
import {inputs, select} from '../dist/profiles.js';
import {READ_OPERATION_METADATA} from '../dist/read-profiles.js';
import {configurations} from './fixtures/config-assistance.mjs';

const cases = [
  ['google-contacts.search-contact', 'people.googleapis.com', 'people.searchContacts', 'contacts.readonly'],
  ['google-tasks.get-task', 'tasks.googleapis.com', 'tasks.get', 'tasks.readonly'],
  ['google-calendar.get-event', 'calendar-json.googleapis.com', 'events.get', 'calendar.events.readonly'],
  ['google-forms.get-form', 'forms.googleapis.com', 'forms.get', 'forms.body.readonly'],
  ['google-slides.get-presentation', 'slides.googleapis.com', 'presentations.get', 'presentations.readonly'],
  ['google-docs.get-document', 'docs.googleapis.com', 'documents.get', 'documents.readonly'],
  ['google-drive.get-file', 'drive.googleapis.com', 'files.get', 'drive.metadata.readonly'],
  ['google-sheets.get-values', 'sheets.googleapis.com', 'spreadsheets.values.get', 'spreadsheets.readonly'],
  ['gmail.get-thread', 'gmail.googleapis.com', 'users.threads.get', 'gmail.readonly'],
];
const text = profile => authenticationGuidance(profile).prerequisites.map(row => row.instruction).join('\n');
const selection = profile => select(operationConfiguration(configurations.find(row => row.profile === profile),
  'fixture-only-token', async () => { throw Error('Network forbidden'); }));

test('nine exact selected reads expose their own dated API and read-scope guidance', () => {
  for (const [profile, api, method, scope] of cases) {
    const help = configurationHelp(profile);
    const value = text(profile);
    assert.equal(help.effect, 'read', profile);
    assert.ok(value.includes(api), profile);
    assert.ok(value.includes(method), profile);
    assert.ok(value.includes(`https://www.googleapis.com/auth/${scope}`), profile);
    assert.ok(value.includes('Source checked 2026-10-03:'), profile);
    assert.match(value, /static options.*not scope detection, a permission grant/);
    assert.ok(help.unresolved_details.some(row => row.includes('actual granted scopes')));
    assert.ok(!help.unresolved_details.some(row => row.includes('not established by this configuration help')));
    assert.deepEqual(Object.keys(authenticationGuidance(profile)).sort(), ['prerequisites', 'unresolved_details']);
    // Broader donor OAuth registration lists must not become recommended scopes.
    const urls = value.match(/https:\/\/www\.googleapis\.com\/auth\/[a-z.]+/g) ?? [];
    const expected = [`https://www.googleapis.com/auth/${scope}`];
    if (profile === 'gmail.get-thread') expected.push('https://www.googleapis.com/auth/gmail.metadata');
    if (profile === 'google-drive.get-file') expected.push('https://www.googleapis.com/auth/drive.file');
    assert.deepEqual([...new Set(urls)].sort(), expected.sort(), profile);
  }
});

test('registration guidance leaves client type, tenant policy and credential custody with existing owners', () => {
  for (const [profile] of cases) {
    const help = authenticationGuidance(profile);
    const value = text(profile);
    assert.match(value, /existing approved Google OAuth application/);
    assert.match(value, /create a Google Cloud project and enable/);
    assert.match(value, /Branding, Audience and Data Access/);
    assert.match(value, /Internal audience.*organization; otherwise choose External/);
    assert.match(value, /External Testing.*test users.*token-lifetime/);
    assert.match(value, /web application.*registered redirect URI.*desktop.*loopback/);
    assert.match(value, /Personal\/testing\/internal exceptions have conditions/);
    const admin = help.prerequisites.find(row => row.kind === 'administrator').instruction;
    assert.match(admin, /For a managed Google Workspace account/);
    assert.match(admin, /If restricted/);
    assert.match(admin, /Personal Google accounts do not have this/);
    assert.match(value, /never its value/);
    assert.match(value, /server performs no acquisition or refresh/);
    assert.match(help.unresolved_details.join(' '), /account identity.*resource remain unverified/);
    assert.match(help.unresolved_details.join(' '), /does not verify.*operation readiness/);
  }
});

test('Gmail metadata is conditional on each call; default full remains unchanged', () => {
  const profile = 'gmail.get-thread';
  const selected = selection(profile);
  const before = JSON.stringify(authenticationGuidance(profile));
  assert.equal(inputs(selected, {thread_id: 'thread1'}).effect.query.format, 'full');
  for (const format of ['full', 'metadata', 'minimal']) {
    assert.equal(inputs(selected, {thread_id: 'thread1', format}).effect.query.format, format);
  }
  assert.throws(() => inputs(selected, {thread_id: 'thread1', format: 'raw'}));
  assert.equal(JSON.stringify(authenticationGuidance(profile)), before);
  assert.match(text(profile), /format full \(the default\)/);
  assert.match(text(profile), /every requested read explicitly uses metadata or minimal/);
  assert.match(text(profile), /cannot serve full/);
  assert.match(text(profile), /Both are restricted scopes/);
  assert.match(text(profile), /later operation format choices consistent.*does not enforce/);
  assert.match(text(profile), /No send, compose or modify permission/);
});

test('Drive file authorization is distinct from metadata scope and unchanged request projection', () => {
  const plan = inputs(selection('google-drive.get-file'), {file_id: 'file1'});
  assert.equal(plan.pathname, '/drive/v3/files/file1');
  assert.deepEqual(plan.query, {supportsAllDrives: 'true'});
  assert.equal(Object.hasOwn(plan.query, 'fields'), false);
  assert.equal(Object.hasOwn(plan.query, 'alt'), false);
  const value = text('google-drive.get-file');
  assert.match(value, /drive.metadata.readonly is a restricted scope/);
  assert.match(value, /drive.file is non-sensitive/);
  assert.match(value, /knowing a file ID or sharing it with the user alone does not establish/);
  assert.match(value, /provides no picker or file-authorization flow/);
  assert.match(value, /also permits writes.*not a read-only token/);
  assert.match(value, /neither downloads content nor promises every metadata field/);
});

test('response selectors do not become scope restrictions or extra selected calls', () => {
  const plan = inputs(selection('google-docs.get-document'), {documentId: 'document1'});
  assert.equal(plan.pathname, '/v1/documents/document1');
  assert.equal(plan.query, undefined);
  assert.match(text('google-docs.get-document'), /omits includeTabsContent.*first-tab/);
  assert.match(text('google-contacts.search-contact'), /not a Workspace Directory search/);
  assert.match(text('google-contacts.search-contact'), /readMask limits returned fields, not the token/);
  assert.match(text('google-forms.get-form'), /does not read submitted responses/);
  assert.match(text('google-sheets.get-values'), /range limits this request, not the token/);
  assert.equal(READ_OPERATION_METADATA['google-sheets.get-values'].maxRequests, 1);
});

test('shared Google instructions stay detached and other 54 profiles have their own guidance', () => {
  const all = listProfiles().profiles.map(row => row.profile);
  const covered = new Set(cases.map(([profile]) => profile));
  assert.equal(all.filter(profile => !covered.has(profile)).length, 54);
  const original = new Map(cases.map(([profile]) => [profile, JSON.stringify(authenticationGuidance(profile))]));
  for (const [profile] of cases) {
    const value = authenticationGuidance(profile);
    value.prerequisites.find(row => row.kind === 'developer_registration').instruction = 'altered';
    value.unresolved_details.length = 0;
  }
  for (const [profile, value] of original) assert.equal(JSON.stringify(authenticationGuidance(profile)), value);
  for (const profile of all.filter(profile => !covered.has(profile))) {
    const help = authenticationGuidance(profile);
    assert.ok(!help.unresolved_details.some(row => row.includes('exact permission/scope requirements are not established')));
    assert.ok(help.prerequisites.some(row => /checked 2026-10-03/i.test(row.instruction)), profile);
    assert.ok(!help.prerequisites.some(row => row.instruction.includes('Google Auth Platform Branding')), profile);
  }
});
