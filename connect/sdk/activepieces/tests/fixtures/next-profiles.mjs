// Expected contracts read from the seven pinned donor action/helper paths.
const value = { enabled: false, count: 0, optional: null };
export const nextProfiles = [
  {
    profile: 'baserow.get-row', config: { origin: 'https://baserow.example.invalid' },
    auth: token => ({ origin: 'https://baserow.example.invalid', databaseToken: token }),
    args: { table_id: 12, row_id: 34 },
    url: 'https://baserow.example.invalid/api/database/rows/table/12/34/?user_field_names=true',
    headers: token => ({ authorization: `Token ${token}` }),
    donorHeaders: token => ({ Authorization: `Token ${token}` }), query: { user_field_names: 'true' },
    body: value, result: value,
  },
  {
    profile: 'bitly.get-bitlink-details', config: {}, auth: token => ({ accessToken: token }),
    args: { bitlink: 'bit.ly/Abc_1' }, url: 'https://api-ssl.bitly.com/v4/bitlinks/bit.ly/Abc_1',
    headers: token => ({ authorization: `Bearer ${token}`, 'content-type': 'application/json' }),
    donorHeaders: token => ({ Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' }), query: {},
    body: value, result: value,
  },
  {
    profile: 'clockify.find-running-timer', config: {}, auth: token => ({ apiKey: token }),
    args: { workspaceId: 'workspace_123' }, url: 'https://api.clockify.me/api/v1/workspaces/workspace_123/time-entries/status/in-progress',
    headers: token => ({ 'x-api-key': token }), donorHeaders: token => ({ 'X-Api-Key': token }), query: {},
    body: [value], result: [value],
  },
  {
    profile: 'coda.get-row', config: {}, auth: token => ({ apiKey: token }),
    args: { docId: 'doc_123', tableId: 'Table / Å', rowIdOrName: 'Row/1?x=%#😀' },
    url: 'https://coda.io/apis/v1/docs/doc_123/tables/Table%20%2F%20%C3%85/rows/Row%2F1%3Fx%3D%25%23%F0%9F%98%80?useColumnNames=true&valueFormat=simpleWithArrays',
    headers: token => ({ authorization: `Bearer ${token}` }), query: { useColumnNames: 'true', valueFormat: 'simpleWithArrays' },
    body: value, result: value,
  },
  {
    profile: 'pagerduty.get-incident', config: {}, auth: token => ({ apiKey: token }),
    args: { incidentId: 'P123ABC' }, url: 'https://api.pagerduty.com/incidents/P123ABC',
    headers: token => ({ authorization: `Token token=${token}`, accept: 'application/vnd.pagerduty+json;version=2', 'content-type': 'application/json' }),
    donorHeaders: token => ({ Authorization: `Token token=${token}`, Accept: 'application/vnd.pagerduty+json;version=2', 'Content-Type': 'application/json' }),
    body: { incident: value }, result: value,
  },
  {
    profile: 'postmark.get-delivery-stats', config: {}, auth: token => ({ serverToken: token }),
    args: {}, url: 'https://api.postmarkapp.com/deliverystats',
    headers: token => ({ 'x-postmark-server-token': token, accept: 'application/json', 'content-type': 'application/json' }),
    donorHeaders: token => ({ 'X-Postmark-Server-Token': token, Accept: 'application/json', 'Content-Type': 'application/json' }),
    body: { InactiveMails: 0, Bounces: [] }, result: { InactiveMails: 0, Bounces: [] },
  },
  {
    profile: 'typeform.get-form-messages', config: {}, auth: token => ({ accessToken: token }),
    args: { form_id: 'form_123' }, url: 'https://api.typeform.com/forms/form_123/messages',
    headers: token => ({ authorization: `Bearer ${token}` }), query: {},
    body: { 'label.buttonHint.default': 'Send', ...value }, result: { 'label.buttonHint.default': 'Send', ...value },
  },
];
