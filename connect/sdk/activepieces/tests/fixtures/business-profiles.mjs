export const KEY = 'fixture-business-secret+/%?&';
const email = 'Alice+weekly&notes@example.com';
const googleTask = { kind: 'tasks#task', id: 'task_1==', title: 'Read release notes', notes: 'Compare changes', status: 'needsAction', completed: null };
const subscriber = { EmailAddress: email, Name: 'Alice', State: 'Active', CustomFields: [
  { Key: 'Topics', Value: 'software' }, { Key: 'Topics', Value: 'research' },
  { Key: 'EmptyFirst', Value: '' }, { Key: 'EmptyFirst', Value: 'later' },
] };
const oktaUser = { id: 'user_123', status: 'ACTIVE', profile: { email, firstName: 'Alice' } };
const conversation = { id: 23, subject: 'Account question', status: 'active', preview: 'How do I export my records?' };
export const businessProfiles = [
  {
    profile: 'google-tasks.get-task', action: 'get_task', auth: key => ({ accessToken: key }), config: {},
    args: { tasks_list: 'list_A==', task_id: 'task_1==' },
    url: () => 'https://tasks.googleapis.com/tasks/v1/lists/list_A==/tasks/task_1==',
    headers: key => ({ accept: 'application/json', authorization: `Bearer ${key}` }),
    body: googleTask, result: googleTask,
    accountBody: account => ({ ...googleTask, account }), accountResult: account => ({ ...googleTask, account }),
    invalidBodies: [null, [], 'task', 0, {}, { id: 'another-task' }],
  },
  {
    profile: 'campaign-monitor.find-subscriber', action: 'find_subscriber', auth: key => ({ apiKey: key }), config: {},
    args: { listId: 'list_1', email },
    url: () => 'https://api.createsend.com/api/v3.3/subscribers/list_1.json?' + new URLSearchParams({ email, includetrackingpreference: 'true' }),
    headers: key => ({ accept: 'application/json', authorization: `Basic ${Buffer.from(`${key}:x`).toString('base64')}`, 'content-type': 'application/json' }),
    body: subscriber, result: { found: true, result: { ...subscriber, CustomFields: { Topics: ['software', 'research'], EmptyFirst: 'later' } } },
    accountBody: account => ({ ...subscriber, account }),
    accountResult: account => ({ found: true, result: { ...subscriber, account, CustomFields: { Topics: ['software', 'research'], EmptyFirst: 'later' } } }),
    invalidBodies: [null, [], {}, { ...subscriber, EmailAddress: 'other@example.com' }, { ...subscriber, CustomFields: null },
      { ...subscriber, CustomFields: [{ Key: 'x', Value: false }] }],
  },
  {
    profile: 'okta.find-user-by-email', action: 'find_user_by_email', auth: key => ({ origin: 'https://selected.okta.invalid', apiToken: key }),
    config: { origin: 'https://selected.okta.invalid' }, args: { email },
    url: () => 'https://selected.okta.invalid/api/v1/users?' + new URLSearchParams({ search: `profile.email eq "${email}"` }),
    headers: key => ({ accept: 'application/json', authorization: `SSWS ${key}`, 'content-type': 'application/json' }),
    body: [oktaUser], result: oktaUser,
    accountBody: account => [{ ...oktaUser, account }], accountResult: account => ({ ...oktaUser, account }),
    invalidBodies: [null, {}, 'users', [null], [{}], [{ profile: { email: 'other@example.com' } }]],
  },
  {
    profile: 'help-scout.find-conversation', action: 'find_conversation', auth: key => ({ accessToken: key }), config: {},
    args: { subject: 'Account question', email, mailboxId: '321', status: 'active', assignTo: '456', tags: ['support', 'new_user'], query: 'createdAt:[2026-01-01 TO 2026-12-31]' },
    url: () => 'https://api.helpscout.net/v2/conversations?' + new URLSearchParams({
      mailbox: '321', status: 'active', tags: 'support,new_user', assigned_to: '456',
      query: `(subject:"Account question" AND email:"${email}" AND createdAt:[2026-01-01 TO 2026-12-31])`,
    }),
    headers: key => ({ accept: 'application/json', authorization: `Bearer ${key}`, 'content-type': 'application/json' }),
    body: { _embedded: { conversations: [conversation] } }, result: { found: true, data: conversation },
    accountBody: account => ({ _embedded: { conversations: [{ ...conversation, account }] } }),
    accountResult: account => ({ found: true, data: { ...conversation, account } }),
    invalidBodies: [null, [], {}, { _embedded: {} }, { _embedded: { conversations: null } }, { _embedded: { conversations: [null] } }],
  },
];
