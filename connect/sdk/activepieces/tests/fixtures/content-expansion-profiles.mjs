export const KEY = 'fixture-content-account-token';
const bearer = key => ({ accept: 'application/json', authorization: `Bearer ${key}` });
const oauth = accessToken => ({ accessToken });
const basic = { id: 'abc123', value: 'Useful selected record' };
export const contentExpansionProfiles = [
  { profile: 'freshdesk.get-contact', action: 'get_contact_from_id', export: 'freshdeskGetContact', config: { origin: 'https://fixture.freshdesk.com' },
    auth: apiKey => ({ apiKey, origin: 'https://fixture.freshdesk.com' }), args: { contactid: 123 },
    url: () => 'https://fixture.freshdesk.com/api/v2/contacts/123', body: { id: 123, name: 'Customer', email: 'customer@example.invalid' } },
  { profile: 'google-forms.get-form', action: 'forms_get_form', export: 'googleFormsGetForm', auth: oauth, args: { form_id: 'form_123' },
    url: () => 'https://forms.googleapis.com/v1/forms/form_123', body: { formId: 'form_123', info: { title: 'Feedback' }, items: [] } },
  { profile: 'google-slides.get-presentation', action: 'get_presentation', export: 'googleSlidesGetPresentation', auth: oauth, args: { presentation_id: 'presentation_123' },
    url: () => 'https://slides.googleapis.com/v1/presentations/presentation_123', body: { presentationId: 'presentation_123', title: 'Quarterly review', slides: [] } },
  { profile: 'onedrive.get-item', action: 'onedrive_get_item', export: 'oneDriveGetItem', config: { cloud: 'commercial' }, auth: accessToken => ({ accessToken, cloud: 'commercial' }), args: { itemId: 'drive_item_123' },
    url: () => 'https://graph.microsoft.com/v1.0/me/drive/items/drive_item_123', body: { id: 'drive_item_123', name: 'Report.txt', file: { mimeType: 'text/plain' }, size: 0 } },
  { profile: 'google-docs.get-document', action: 'get_document', export: 'googleDocsGetDocument', auth: oauth, args: { documentId: 'doc_123' },
    url: () => 'https://docs.googleapis.com/v1/documents/doc_123', body: { documentId: 'doc_123', title: 'Project notes', body: { content: [] } } },
  { profile: 'google-drive.get-file', action: 'drive_get_file', export: 'googleDriveGetFile', auth: oauth, args: { file_id: 'file_123' },
    url: () => 'https://www.googleapis.com/drive/v3/files/file_123?supportsAllDrives=true', body: { id: 'file_123', name: 'Notes.txt', mimeType: 'text/plain' } },
  { profile: 'contentful.get-entry', action: 'contentful_record_get', export: 'contentfulGetRecord', config: { space: 'space_123', environment: 'master' },
    auth: accessToken => ({ accessToken, space: 'space_123', environment: 'master' }), args: { entityId: 'entry_123' },
    url: () => 'https://api.contentful.com/spaces/space_123/environments/master/entries/entry_123', body: { sys: { id: 'entry_123', type: 'Entry' }, fields: { title: { 'en-US': 'Article' } } } },
  { profile: 'outlook.get-message', action: 'outlook_get_message', export: 'outlookGetMessage', config: { cloud: 'commercial' },
    auth: accessToken => ({ accessToken, cloud: 'commercial' }), args: { messageId: 'message_123' },
    url: () => 'https://graph.microsoft.com/v1.0/me/messages/message_123', body: { id: 'message_123', subject: 'Status', body: { contentType: 'text', content: 'Ready' } } },
  { profile: 'hubspot.get-contact', action: 'get-contact', export: 'hubspotGetContact', auth: oauth, args: { contactId: '123' },
    url: () => 'https://api.hubapi.com/crm/v3/objects/contacts/123?properties=firstname%2Clastname%2Cemail%2Ccompany%2Cwebsite%2Cmobilephone%2Cphone%2Cfax%2Caddress%2Ccity%2Cstate%2Czip%2Csalutation%2Ccountry%2Cjobtitle%2Chs_createdate%2Chs_email_domain%2Chs_object_id%2Clastmodifieddate%2Chs_persona%2Chs_language%2Clifecyclestage%2Ccreatedate%2Cnumemployees%2Cannualrevenue%2Cindustry',
    body: { id: '123', properties: { firstname: 'Ada', email: null }, createdAt: '2026-01-02T03:04:05.000Z', updatedAt: '2026-02-03T04:05:06.000Z', archived: false } },
  { profile: 'supabase.get-user', action: 'get_user', export: 'supabaseGetUser', config: { origin: 'https://selected.supabase.co' },
    auth: serviceRoleKey => ({ serviceRoleKey, origin: 'https://selected.supabase.co' }), args: { userId: '01234567-89ab-cdef-0123-456789abcdef' },
    url: () => 'https://selected.supabase.co/auth/v1/admin/users/01234567-89ab-cdef-0123-456789abcdef', body: { id: '01234567-89ab-cdef-0123-456789abcdef', email: 'user@example.invalid', created_at: '2026-01-02T03:04:05Z' } },
  { profile: 'sendgrid.find-list-by-name', action: 'find_list_by_name', export: 'sendgridFindList', config: { region: 'us' }, auth: apiKey => ({ apiKey, region: 'us' }), args: { name: '  Newsletter  ' },
    url: () => 'https://api.sendgrid.com/v3/marketing/lists?page_size=1000', body: { result: [{ id: 'list_1', name: 'newsletter' }, { id: 'list_2', name: 'Other' }] } },
  { profile: 'webflow.get-collection-item', action: 'get_collection_item', export: 'webflowGetItem', auth: oauth,
    args: { collection_id: '0123456789abcdef01234567', collection_item_id: '89abcdef0123456789abcdef' },
    url: () => 'https://api.webflow.com/v2/collections/0123456789abcdef01234567/items/89abcdef0123456789abcdef', body: { id: '89abcdef0123456789abcdef', fieldData: { name: 'Published article' }, isDraft: false } },
];
for (const d of contentExpansionProfiles) d.result = d.body;

for (const d of contentExpansionProfiles) {
 d.headers = key => {
  const headers = bearer(key);
  if (d.profile === 'freshdesk.get-contact') Object.assign(headers, { authorization: `Basic ${Buffer.from(`${key}:X`).toString('base64')}`, 'content-type': 'application/json' });
  if (['google-docs.get-document','google-drive.get-file'].includes(d.profile)) Object.assign(headers, { 'accept-encoding': 'gzip', 'user-agent': 'google-api-nodejs-client/8.0.3 (gzip)', 'x-goog-api-client': `gdcl/8.0.3 gl-node/${process.versions.node}` });
  if (d.profile === 'contentful.get-entry') Object.assign(headers, { accept: 'application/json, text/plain, */*', 'content-type': 'application/vnd.contentful.management.v1+json', 'x-contentful-user-agent': `sdk contentful-management-plain.js/11.48.1; platform node.js/${process.version}; os ${{darwin:'macOS',linux:'Linux',win32:'Windows'}[process.platform]}/${process.version};` });
  if (d.profile === 'outlook.get-message') headers.prefer = 'outlook.body-content-type="text"';
  if (d.profile === 'hubspot.get-contact') headers.accept = 'application/json, */*;q=0.8';
  if (d.profile === 'supabase.get-user') Object.assign(headers, { apikey:key, 'x-supabase-api-version':'2024-01-01' });
  return Object.fromEntries(Object.entries(headers).sort());
 };
}
Object.assign(contentExpansionProfiles[3], {result:{ id:'drive_item_123',name:'Report.txt',type:'file',mimeType:'text/plain',size:0,webUrl:null,createdDateTime:null,lastModifiedDateTime:null,childCount:null,parentId:null,parentPath:null,driveId:null,driveType:null,createdByName:null,createdByEmail:null,lastModifiedByName:null,lastModifiedByEmail:null,description:null,deleted:false,remoteItemId:null,remoteDriveId:null }});
Object.assign(contentExpansionProfiles[9], {result:{ id:'01234567-89ab-cdef-0123-456789abcdef',email:'user@example.invalid',phone:null,user_metadata:{},app_metadata:{},created_at:'2026-01-02T03:04:05Z',last_sign_in_at:null,confirmed_at:null }});
Object.assign(contentExpansionProfiles[10], {result:{ found:true,lists:[{id:'list_1',name:'newsletter'}] }});
