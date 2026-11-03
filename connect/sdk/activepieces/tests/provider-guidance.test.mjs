import test from 'node:test';
import assert from 'node:assert/strict';
import {authenticationGuidance} from '../dist/auth-guidance.js';
import {configurationHelp} from '../dist/config-fields.js';

const text = p => authenticationGuidance(p).prerequisites.map(x => x.instruction).join('\n');

test('Atlassian direct-site credentials cannot be confused with scoped gateway or app onboarding', () => {
  for (const p of ['confluence.get-page-content', 'jira-cloud.get-issue']) {
    assert.match(text(p), /unscoped API token/);
    assert.match(text(p), /api.atlassian.com\/ex/);
    assert.match(text(p), /direct-site profile does not implement/);
    assert.match(text(p), /personal direct-site compatibility route/);
    assert.match(text(p), /not made read-only/);
  }
});

test('cloud listing scope and IAM grants are distinct from filtering and credential issuance', () => {
  const s3 = text('amazon-s3.list-files');
  assert.match(s3, /s3:ListBucket/);
  assert.match(s3, /does not require s3:GetObject/);
  assert.match(s3, /input prefix by itself is only a filter/);
  assert.match(s3, /matching access-key ID, secret access key and session token together/);
  const gcs = text('google-cloud-storage.search-objects');
  assert.match(gcs, /storage.objects.list/);
  assert.match(gcs, /OAuth scope and IAM permission are both needed/);
  assert.match(gcs, /devstorage.read_only/);
  assert.match(gcs, /does not run gcloud, read ADC/);
});

test('broad admin keys are disclosed while granular read options stay granular', () => {
  assert.match(text('supabase.get-user'), /service_role.*bypasses Row Level Security/);
  assert.match(text('supabase.get-user'), /New secret-key compatibility.*not been established/);
  assert.match(text('okta.find-user-by-email'), /SSWS authentication/);
  assert.match(text('okta.find-user-by-email'), /inherits its creator's current privileges/);
  assert.match(text('stripe.get-invoice'), /Invoices Read/);
  assert.match(text('stripe.get-invoice'), /no Stripe-Account or Stripe-Context override/);
  assert.match(text('algolia.browse-records'), /browse ACL.*configured index/);
  assert.match(text('algolia.browse-records'), /search-only key does not authorize browsing/);
  assert.match(text('ghost.get-post'), /Admin API key, not its Content API key/);
  assert.match(text('ghost.get-post'), /fixed Admin API permissions that include writing/);
});

test('Gitea Bearer route and all four writes retain host approval and uncertainty handling', () => {
  assert.match(text('gitea.create-issue'), /Gitea OAuth application/);
  assert.match(text('gitea.create-issue'), /Authorization: token.*PAT is interchangeable/);
  assert.match(text('gitea.create-issue'), /Gitea 1.23 or later.*write:issue/);
  for (const p of ['activecampaign.create-contact', 'freshservice.create-ticket', 'gitea.create-issue', 'datadog.submit-log']) {
    assert.equal(configurationHelp(p).effect, 'write');
    assert.match(text(p), /host must separately approve the write/);
    assert.match(text(p), /uncertain/);
  }
  assert.match(text('activecampaign.create-contact'), /not the upsert endpoint/);
  assert.match(text('freshservice.create-ticket'), /does not create requesters or select workspace_id/);
  assert.match(text('datadog.submit-log'), /202 acknowledgement means accepted for processing/);
});

test('message and marketing reads preserve exact token identity and bounded results', () => {
  const slack = text('slack.get-thread-replies');
  assert.match(slack, /channels:history.*groups:history.*im:history.*mpim:history/);
  assert.match(slack, /not a Slack agent frontend/);
  assert.match(slack, /asks for 200 per page/);
  assert.match(slack, /non-Marketplace commercially distributed/);
  assert.match(text('mailchimp.get-campaign-report'), /OAuth Metadata endpoint/);
  assert.match(text('mailchimp.get-campaign-report'), /no separate report-read scope/);
  assert.match(text('sendgrid.find-list-by-name'), /does not follow additional list pages/);
  assert.match(text('sendgrid.find-list-by-name'), /marketing.read scope grants both read and write/);
  assert.match(text('monday.get-items'), /not a separately selectable boards:read scope/);
  assert.match(text('pagerduty.get-incident'), /not an Events API routing key/);
});
