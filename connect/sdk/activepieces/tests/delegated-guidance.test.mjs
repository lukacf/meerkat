// Copy into sdk/activepieces/tests after the reviewed static records are applied.
// These cases assert important operation/auth distinctions, not provider success.
import test from 'node:test';
import assert from 'node:assert/strict';
import {authenticationGuidance} from '../dist/auth-guidance.js';

const profiles = [
  'asana.get-task', 'bitly.get-bitlink-details', 'typeform.get-form-messages',
  'figma.get-comments', 'acuity.find-appointments', 'calendly.get-scheduled-event',
  'help-scout.find-conversation', 'pipedrive.get-activity', 'intercom.get-conversation',
  'onedrive.get-item', 'outlook.get-message', 'hubspot.get-contact',
  'webflow.get-collection-item', 'dropbox.get-file-metadata',
  'microsoft-excel-365.get-table-columns', 'microsoft-teams.get-channel-message',
  'quickbooks.find-invoice', 'xero.find-invoice',
];
const text = profile => authenticationGuidance(profile).prerequisites.map(x => x.instruction).join('\n');

test('delegated/cloud guidance preserves references, unresolved live state and detached output', () => {
  assert.equal(profiles.length, 18);
  for (const profile of profiles) {
    const help = authenticationGuidance(profile);
    assert.equal(help.prerequisites.filter(x => x.kind === 'client_authentication').length, 1, profile);
    assert.match(text(profile), /token_env names an environment variable/, profile);
    assert.match(text(profile), /2026-10-03/, profile);
    assert.match(text(profile), /https:\/\//, profile);
    assert.ok(help.unresolved_details.some(x => x.includes('exact permission/scope')), profile);
    assert.ok(help.unresolved_details.some(x => x.includes('does not verify')), profile);
    const expected = JSON.stringify(help);
    help.prerequisites.find(x => x.kind === 'developer_registration').instruction = 'mutation';
    help.unresolved_details[0] = 'mutation';
    assert.equal(JSON.stringify(authenticationGuidance(profile)), expected, profile);
  }
});

test('fixed Asana relationship fields and scoped scheduling reads are not understated', () => {
  assert.match(text('asana.get-task'), /tasks:read/);
  assert.match(text('asana.get-task'), /projects:read and project_sections:read/);
  assert.match(text('calendly.get-scheduled-event'), /scheduled_events:read/);
  assert.match(text('calendly.get-scheduled-event'), /personal token/);
  assert.match(text('acuity.find-appointments'), /api-v1.*broader/);
  assert.match(text('acuity.find-appointments'), /Basic-auth.*not the OAuth/);
});

test('form messages and Figma comments do not borrow unrelated donor scopes', () => {
  assert.match(text('typeform.get-form-messages'), /forms:read/);
  assert.match(text('typeform.get-form-messages'), /not submitted response answers/);
  assert.match(text('figma.get-comments'), /file_comments:read/);
  assert.match(text('figma.get-comments'), /X-Figma-Token is not a drop-in replacement/);
  assert.match(text('pipedrive.get-activity'), /activities:read/);
});

test('token-only guides do not manufacture granular read-only grants', () => {
  assert.match(text('bitly.get-bitlink-details'), /does not establish a separate read-only scope/);
  assert.match(text('help-scout.find-conversation'), /does not establish a granular conversation-read scope/);
  assert.match(text('help-scout.find-conversation'), /first|that page/);
  assert.match(text('intercom.get-conversation'), /Read conversations/);
  assert.match(text('intercom.get-conversation'), /Read tickets/);
  assert.match(text('intercom.get-conversation'), /Intercom-Version 2\.11/);
});

test('Microsoft cloud names do not promise unsupported sovereign endpoints', () => {
  for (const p of ['onedrive.get-item', 'outlook.get-message', 'microsoft-excel-365.get-table-columns', 'microsoft-teams.get-channel-message']) {
    assert.match(text(p), /commercial uses graph\.microsoft\.com/);
    assert.match(text(p), /government uses graph\.microsoft\.us/);
    assert.match(text(p), /DoD.*China.*not represented/);
    if (p !== 'microsoft-teams.get-channel-message')
      assert.match(text(p), /application permissions require administrator consent/);
  }
});

test('OneDrive and Excel delegated-only restrictions survive read-only labels', () => {
  assert.match(text('onedrive.get-item'), /delegated Files\.Read/);
  assert.match(text('onedrive.get-item'), /\/me\/drive/);
  assert.match(text('onedrive.get-item'), /do not make app-only tokens work/);
  const excel = text('microsoft-excel-365.get-table-columns');
  assert.match(excel, /delegated Files\.ReadWrite/);
  assert.match(excel, /application permissions are not supported/);
  assert.match(excel, /OneDrive Consumer/);
  assert.match(excel, /does no separate resource discovery/);
});

test('mail content, shared mailbox and application paths remain distinct', () => {
  const mail = text('outlook.get-message');
  assert.match(mail, /Mail\.ReadBasic/);
  assert.match(mail, /omit body/);
  assert.match(mail, /Mail\.Read\.Shared/);
  assert.match(mail, /\/me\/messages.*delegated/);
  assert.match(mail, /app-only Mail\.Read needs admin consent/);
});

test('Teams per-team application access requires an actual RSC grant', () => {
  const teams = text('microsoft-teams.get-channel-message');
  assert.match(teams, /ChannelMessage\.Read\.All/);
  assert.match(teams, /ChannelMessage\.Read\.Group with resource-specific consent/);
  assert.match(teams, /installation\/consent/);
  assert.match(teams, /does not universally require tenant-admin consent/);
  assert.doesNotMatch(teams, /application permissions require administrator consent/);
  assert.match(teams, /Personal Microsoft accounts are unsupported/);
  assert.match(teams, /not a chat read, message sender or agent frontend/);
});

test('content metadata permissions do not imply arbitrary account or file visibility', () => {
  assert.match(text('hubspot.get-contact'), /crm\.objects\.contacts\.read/);
  assert.match(text('hubspot.get-contact'), /account-level.*user-level/);
  assert.match(text('webflow.get-collection-item'), /cms:read/);
  assert.match(text('webflow.get-collection-item'), /staged-item endpoint/);
  const dropbox = text('dropbox.get-file-metadata');
  assert.match(dropbox, /files\.metadata\.read/);
  assert.match(dropbox, /App folder or Full Dropbox/);
  assert.match(dropbox, /without Dropbox-API-Select-User, Select-Admin or Path-Root/);
});

test('accounting guidance preserves company and scope-generation boundaries', () => {
  const qbo = text('quickbooks.find-invoice');
  assert.match(qbo, /com\.intuit\.quickbooks\.accounting/);
  assert.match(qbo, /company_id to the realmId/);
  assert.match(qbo, /production quickbooks\.api\.intuit\.com/);
  assert.match(qbo, /sandbox.*not selected/);
  const xero = text('xero.find-invoice');
  assert.match(xero, /accounting\.invoices\.read/);
  assert.match(xero, /accounting\.transactions\.read.*legacy alternative/);
  assert.match(xero, /2 March 2026/);
  assert.match(xero, /September 2027/);
  assert.match(xero, /tenant_id.*actual connection/);
  assert.match(xero, /Standard, Adviser or Administrator access with Connected Apps permission/);
});
