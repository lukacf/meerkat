export const KEY = 'fixture-content-account';
export const FIXTURES = [
  {
    profile: 'confluence.get-page-content',
    action: 'getPageContent',
    auth: key => ({ origin: 'https://fixture.atlassian.net', username: 'fixture@example.invalid', apiToken: key }),
    mcp: { origin: 'https://fixture.atlassian.net', username: 'fixture@example.invalid' },
    args: { pageId: '123' },
    url: 'https://fixture.atlassian.net/wiki/api/v2/pages/123?body-format=storage',
    header: key => ['authorization', `Basic ${Buffer.from(`fixture@example.invalid:${key}`).toString('base64')}`],
    wire: { id: '123', title: 'Fixture Page', body: { storage: { value: '<p>Page content</p>', representation: 'storage' } }, optional: null },
    result: { id: '123', title: 'Fixture Page', body: { storage: { value: '<p>Page content</p>', representation: 'storage' } }, optional: null },
  },
  {
    profile: 'brevo.find-contact', action: 'find_contact',
    auth: key => ({ apiKey: key }), mcp: {},
    args: { identifier: 'fixture+one@example.invalid' },
    url: 'https://api.brevo.com/v3/contacts/fixture%2Bone%40example.invalid?identifierType=email_id',
    header: key => ['api-key', key],
    wire: { id: 12, email: 'fixture+one@example.invalid', emailBlacklisted: false, attributes: { SCORE: 0 }, optional: null },
    result: { found: true, data: { id: 12, email: 'fixture+one@example.invalid', emailBlacklisted: false, attributes: { SCORE: 0 }, optional: null } },
  },
  {
    profile: 'jotform.get-submission', action: 'jotform_get_submission',
    auth: key => ({ apiKey: key, region: 'eu' }), mcp: { region: 'eu' },
    args: { submissionId: '9876543210123456789' },
    url: 'https://eu-api.jotform.com/submission/9876543210123456789',
    header: key => ['apikey', key],
    wire: { responseCode: 200, content: { id: '9876543210123456789', status: 'ACTIVE', answers: { '1': { text: 'Feedback', answer: 'Useful form' } }, optional: null } },
    result: { id: '9876543210123456789', status: 'ACTIVE', answers: { '1': { text: 'Feedback', answer: 'Useful form' } }, optional: null },
  },
];
