import test from 'node:test';
import assert from 'node:assert/strict';
import {authenticationGuidance} from '../dist/auth-guidance.js';

const profiles = [
  'tally.get-form', 'nocodb.get-record', 'baserow.get-row',
  'clockify.find-running-timer', 'coda.get-row', 'postmark.get-delivery-stats',
  'brevo.find-contact', 'jotform.get-submission', 'kit.get-subscriber',
  'mailer-lite.get-campaign', 'campaign-monitor.find-subscriber', 'airtable.get-record',
  'toggl.find-project', 'wordpress.get-page', 'notion.get-page',
  'freshdesk.get-contact', 'contentful.get-entry', 'linear.get-issue',
];
const text = profile => authenticationGuidance(profile).prerequisites.map(row => row.instruction).join('\n');

test('all 18 token routes carry dated setup instructions without replacing credential custody', () => {
  for (const profile of profiles) {
    const help = authenticationGuidance(profile);
    assert.deepEqual(Object.keys(help).sort(), ['prerequisites', 'unresolved_details']);
    assert.ok(help.prerequisites.some(row => row.kind === 'developer_registration'), profile);
    assert.match(text(profile), /[Ss]ources? checked 2026-10-03: https:\/\//, profile);
    assert.match(text(profile), /configuration and agent input contain references, never its value/, profile);
    assert.match(text(profile), /server performs no acquisition or refresh/, profile);
    assert.ok(help.unresolved_details.some(row => row.includes('remain unverified')), profile);
    assert.ok(help.unresolved_details.some(row => row.includes('operation readiness')), profile);
    assert.ok(!help.unresolved_details.some(row => row.includes('not established by this configuration help')), profile);
  }
});

test('record permissions distinguish table/base restrictions from token identity', () => {
  const nocodb = text('nocodb.get-record');
  assert.match(nocodb, /Records: Read/);
  assert.match(nocodb, /intersection of token permissions and the user role/);
  assert.match(nocodb, /Community\/unlicensed.*broader access/);
  assert.match(nocodb, /version 3.*\/api\/v2\/tables.*version 4.*\/api\/v3\/data/);
  const baserow = text('baserow.get-row');
  assert.match(baserow, /Read enabled and Create, Update and Delete disabled/);
  assert.match(baserow, /Enterprise.*Admins and Builders/);
  const airtable = text('airtable.get-record');
  assert.match(airtable, /data.records:read.*required base/);
  assert.match(airtable, /schema.bases:read and record-write permission are not needed/);
  assert.match(airtable, /scope does not grant missing base membership/);
});

test('legacy Kit and current MailerLite are not silently exchanged for newer or older APIs', () => {
  const kit = text('kit.get-subscriber');
  assert.match(kit, /V3 API secret/);
  assert.match(kit, /not the public V3 API key and not a V4 API key/);
  assert.match(kit, /V3 as legacy and no longer actively developed/);
  assert.match(kit, /V3 protocol puts api_secret in the provider request query/);
  assert.match(kit, /not a read-only credential/);
  const mailer = text('mailer-lite.get-campaign');
  assert.match(mailer, /connect.mailerlite.com\/api/);
  assert.match(mailer, /MailerLite Classic.*different product route/);
  assert.match(mailer, /removing them invalidates/);
});

test('Notion and WordPress bind resource sharing and user capabilities without extra operations', () => {
  const notion = text('notion.get-page');
  assert.match(notion, /workspace owner.*Internal connection/);
  assert.match(notion, /Read content capability alone does not grant page access/);
  assert.match(notion, /Add connections/);
  assert.match(notion, /Notion-Version 2022-02-22/);
  assert.match(notion, /not body blocks or all paginated property values/);
  const wordpress = text('wordpress.get-page');
  assert.match(wordpress, /WordPress 5.6 or later/);
  assert.match(wordpress, /do not use the main login password or an OAuth token/);
  assert.match(wordpress, /no independent page-read-only scope/);
  assert.match(wordpress, /does not request edit context/);
});

test('Contentful management and Linear read permissions remain distinct from unrelated token types', () => {
  const contentful = text('contentful.get-entry');
  assert.match(contentful, /not a Content Delivery or Preview API key/);
  assert.match(contentful, /content_management_read.*content_management_manage/);
  assert.match(contentful, /scope does not independently restrict it to this one space/);
  const linear = text('linear.get-issue');
  assert.match(linear, /Read permission and only the required teams/);
  assert.match(linear, /does not need Write, Admin, Create issues or Create comments/);
  assert.match(linear, /POST contains read queries/);
  assert.match(linear, /private-team membership/);
  assert.doesNotMatch(linear, /content_management_read/);
});

test('mail service setup uses the exact selected auth mechanism without overgranting an OAuth route', () => {
  const postmark = text('postmark.get-delivery-stats');
  assert.match(postmark, /X-Postmark-Server-Token, not an Account API token/);
  assert.match(postmark, /Server API token can authorize other server operations/);
  const brevo = text('brevo.find-contact');
  assert.match(brevo, /Do not choose Create MCP server API key/);
  assert.match(brevo, /contacts:read OAuth scope.*not a permission setting on this selected API-key route/);
  const campaign = text('campaign-monitor.find-subscriber');
  assert.match(campaign, /Full access and a verified email address/);
  assert.match(campaign, /does not subscribe, resubscribe or send a campaign/);
});

test('form/time/helpdesk routes preserve account, region and bounded-read distinctions', () => {
  assert.match(text('tally.get-form'), /does not retrieve submissions/);
  assert.match(text('jotform.get-submission'), /Read Access rather than Full Access/);
  assert.match(text('jotform.get-submission'), /hipaa is routing, not a compliance assessment/);
  assert.match(text('clockify.find-running-timer'), /global https:\/\/api.clockify.me/);
  assert.match(text('toggl.find-project'), /Toggl Focus\/2.0 Bearer token is not interchangeable/);
  assert.match(text('toggl.find-project'), /one bounded project-list page/);
  assert.match(text('freshdesk.get-contact'), /API access follows that user's permissions/);
  assert.match(text('coda.get-row'), /locking.*not a server-side restriction/s);
});

test('new provider rows remain detached and do not conflate account setup across profiles', () => {
  const initial = authenticationGuidance('notion.get-page');
  const snapshot = JSON.stringify(initial);
  initial.prerequisites.find(row => row.kind === 'developer_registration').instruction = 'mutated';
  initial.unresolved_details.length = 0;
  assert.equal(JSON.stringify(authenticationGuidance('notion.get-page')), snapshot);
  for (const profile of profiles.filter(value => value !== 'notion.get-page')) {
    assert.doesNotMatch(text(profile), /Notion-Version 2022-02-22/, profile);
  }
});
