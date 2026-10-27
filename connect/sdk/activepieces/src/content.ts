import { ActivepiecesError } from './errors.js';
import { record, configurationKeys as keys, selectedSecret as token, canonicalOrigin, type Selection } from './profiles.js';

export const CONTENT_PROFILE_IDS = [
  'freshdesk.get-contact', 'google-forms.get-form', 'google-slides.get-presentation', 'onedrive.get-item',
  'google-docs.get-document', 'google-drive.get-file', 'contentful.get-entry', 'outlook.get-message',
  'hubspot.get-contact', 'supabase.get-user', 'sendgrid.find-list-by-name', 'webflow.get-collection-item',
] as const;
type ContentProfile = typeof CONTENT_PROFILE_IDS[number];
export type ContentConfiguration =
  { readonly profile: 'freshdesk.get-contact'; readonly auth: { readonly origin: string; readonly apiKey: string } } |
  { readonly profile: 'google-forms.get-form' | 'google-slides.get-presentation' | 'google-docs.get-document' | 'google-drive.get-file' | 'hubspot.get-contact' | 'webflow.get-collection-item'; readonly auth: { readonly accessToken: string } } |
  { readonly profile: 'onedrive.get-item'; readonly auth: { readonly accessToken: string; readonly cloud: 'commercial' | 'government' } } |
  { readonly profile: 'outlook.get-message'; readonly auth: { readonly accessToken: string; readonly cloud: 'commercial' | 'government'; readonly mailbox?: string } } |
  { readonly profile: 'contentful.get-entry'; readonly auth: { readonly accessToken: string; readonly space: string; readonly environment: string } } |
  { readonly profile: 'supabase.get-user'; readonly auth: { readonly origin: string; readonly serviceRoleKey: string } } |
  { readonly profile: 'sendgrid.find-list-by-name'; readonly auth: { readonly apiKey: string; readonly region: 'us' | 'eu' } };
const hasProfile = (profile: string): profile is ContentProfile => (CONTENT_PROFILE_IDS as readonly string[]).includes(profile);
const whole = '(?![\\s\\S])';
const idSchema = { type: 'string', minLength: 1, maxLength: 128, pattern: `^[A-Za-z0-9_-]+${whole}` };
const textSchema = { type: 'string', minLength: 1, maxLength: 256,
  pattern: `^(?:[^\\u0000-\\u001f\\u007f\\ud800-\\udfff]|[\\ud800-\\udbff][\\udc00-\\udfff])+${whole}` };
const text = (value: unknown, max = 256): value is string => typeof value === 'string' && value.length <= max * 2 && [...value].length <= max &&
  !/[\x00-\x1f\x7f]/.test(value) && !/[\uD800-\uDFFF]/u.test(value);
const id = (value: unknown): value is string => typeof value === 'string' && new RegExp(idSchema.pattern).test(value) && value.length <= 128;
const positive = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) > 0;
const positiveSchema = { type: 'integer', minimum: 1, maximum: Number.MAX_SAFE_INTEGER };
const uuidPattern = `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}${whole}`;
const hexPattern = `^[a-f0-9]{24}${whole}`;
const slidePattern = `^(?:[A-Za-z0-9_-]{10,128}|https://docs\\.google\\.com/presentation/d/[A-Za-z0-9_-]{10,128}/edit)${whole}`;
const fieldPattern = `^[A-Za-z0-9_*,()/ ]*${whole}`;
// Explicit ECMAScript trim whitespace also keeps non-JavaScript schema
// validators from substituting their different Unicode meaning of \s.
const trimWhitespace = '[\\u0009-\\u000d\\u0020\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000\\ufeff]';
// Runtime trims first, then removes outer slashes. The repeated-slash branch
// must therefore match wholly inside the trimmed text: surrounding whitespace
// is not an empty path segment, but whitespace inside outer slashes is data.
const invalidOneDrivePath = `^${trimWhitespace}*/*${trimWhitespace}*${whole}|` +
  `(?:^${trimWhitespace}*/*|/)\\.{1,2}(?:/|${trimWhitespace}*${whole})|` +
  `^${trimWhitespace}*(?!${trimWhitespace})[\\s\\S]*[^/]/{2,}[^/][\\s\\S]*(?<!${trimWhitespace})${trimWhitespace}*${whole}`;
const googleOrigins: Record<string, string> = { 'google-forms.get-form': 'https://forms.googleapis.com', 'google-slides.get-presentation': 'https://slides.googleapis.com',
  'google-docs.get-document': 'https://docs.googleapis.com', 'google-drive.get-file': 'https://www.googleapis.com', 'hubspot.get-contact': 'https://api.hubapi.com', 'webflow.get-collection-item': 'https://api.webflow.com' };
export function selectContent(profile: string, auth: Record<string, unknown>): Pick<Selection, 'origin' | 'secret' | 'auth'> | undefined {
  if (!hasProfile(profile)) return undefined;
  if (googleOrigins[profile] && keys(auth, ['accessToken']) && token(auth.accessToken)) return {
    origin: googleOrigins[profile]!, secret: auth.accessToken, auth: { type: 'OAUTH2', access_token: auth.accessToken },
  };
  if (profile === 'freshdesk.get-contact' && keys(auth, ['origin', 'apiKey']) && token(auth.apiKey) && !auth.apiKey.includes(':')) return {
    origin: canonicalOrigin(auth.origin), secret: auth.apiKey,
    auth: { type: 'CUSTOM_AUTH', props: { base_url: canonicalOrigin(auth.origin), access_token: `Basic ${Buffer.from(`${auth.apiKey}:X`).toString('base64')}` } },
  };
  if ((profile === 'onedrive.get-item' || profile === 'outlook.get-message') &&
    keys(auth, ['accessToken', 'cloud', ...(profile === 'outlook.get-message' ? ['mailbox'] : [])], ['accessToken', 'cloud']) && token(auth.accessToken) && typeof auth.cloud === 'string' && ['commercial', 'government'].includes(auth.cloud)) {
    if (Object.hasOwn(auth, 'mailbox') && (!text(auth.mailbox) || !auth.mailbox.trim() || ['.', '..'].includes(auth.mailbox.trim()))) throw 0;
    return { origin: auth.cloud === 'government' ? 'https://graph.microsoft.us' : 'https://graph.microsoft.com', secret: auth.accessToken,
      auth: { type: 'OAUTH2', access_token: auth.accessToken, props: { cloud: auth.cloud === 'government' ? 'login.microsoftonline.us' : 'login.microsoftonline.com',
        ...(Object.hasOwn(auth, 'mailbox') ? { mailbox: (auth.mailbox as string).trim() } : {}) } } };
  }
  if (profile === 'contentful.get-entry' && keys(auth, ['accessToken', 'space', 'environment']) && token(auth.accessToken) && id(auth.space) && id(auth.environment)) return {
    origin: 'https://api.contentful.com', secret: auth.accessToken, auth: { type: 'CUSTOM_AUTH', props: { apiKey: auth.accessToken, space: auth.space, environment: auth.environment } },
  };
  if (profile === 'supabase.get-user' && keys(auth, ['origin', 'serviceRoleKey']) && token(auth.serviceRoleKey)) return {
    origin: canonicalOrigin(auth.origin), secret: auth.serviceRoleKey, auth: { type: 'CUSTOM_AUTH', props: { url: canonicalOrigin(auth.origin), apiKey: auth.serviceRoleKey } },
  };
  if (profile === 'sendgrid.find-list-by-name' && keys(auth, ['apiKey', 'region']) && token(auth.apiKey) && typeof auth.region === 'string' && ['us', 'eu'].includes(auth.region)) return {
    origin: auth.region === 'eu' ? 'https://api.eu.sendgrid.com' : 'https://api.sendgrid.com', secret: auth.apiKey,
    auth: { type: 'CUSTOM_AUTH', props: { apiKey: auth.apiKey, dataResidency: auth.region === 'eu' ? 'EU' : 'US' } },
  };
  throw 0;
}

// These are the unchanged pinned helper's selected CONTACT defaults. The donor
// still computes the same list; the independent seam binds its concrete query.
export const HUBSPOT_CONTACT_PROPERTIES = [
  'firstname', 'lastname', 'email', 'company', 'website', 'mobilephone', 'phone', 'fax', 'address', 'city', 'state', 'zip', 'salutation', 'country', 'jobtitle', 'hs_createdate', 'hs_email_domain', 'hs_object_id', 'lastmodifieddate', 'hs_persona', 'hs_language', 'lifecyclestage', 'createdate', 'numemployees', 'annualrevenue', 'industry',
];
const actions: Record<ContentProfile, string> = {
  'freshdesk.get-contact': 'get_contact_from_id', 'google-forms.get-form': 'forms_get_form', 'google-slides.get-presentation': 'get_presentation',
  'onedrive.get-item': 'onedrive_get_item', 'google-docs.get-document': 'get_document', 'google-drive.get-file': 'drive_get_file',
  'contentful.get-entry': 'contentful_record_get', 'outlook.get-message': 'outlook_get_message', 'hubspot.get-contact': 'get-contact',
  'supabase.get-user': 'get_user', 'sendgrid.find-list-by-name': 'find_list_by_name', 'webflow.get-collection-item': 'get_collection_item',
};
// Use the same JSON-schema scalar rule as the existing encoded text profiles.
const scalarSchema = { ...textSchema };
export function describeContent(profile: string): { action: string; inputSchema: Record<string, unknown> } | undefined {
  if (!hasProfile(profile)) return undefined;
  const encoded = { ...scalarSchema, minLength: 1, not: { pattern: '^\\s*(?:\\.{1,2})?\\s*$' } };
  const properties: Record<string, unknown> = profile === 'freshdesk.get-contact' ? { contactid: { ...positiveSchema } } :
    profile === 'google-forms.get-form' ? { form_id: { ...idSchema } } :
    profile === 'google-slides.get-presentation' ? { presentation_id: { type: 'string', maxLength: 176, pattern: slidePattern },
      fields: { anyOf: [{ type: 'string', maxLength: 1024, pattern: fieldPattern }, { type: 'null' }] } } :
    profile === 'onedrive.get-item' ? { itemId: structuredClone(encoded), path: { ...structuredClone(encoded), not: { pattern: invalidOneDrivePath }, description: 'Exactly one itemId or path. Trimmed path relative to drive root; no empty, dot or parent segments.' } } :
    profile === 'google-docs.get-document' ? { documentId: { ...idSchema } } :
    profile === 'google-drive.get-file' ? { file_id: { ...idSchema } } :
    profile === 'contentful.get-entry' ? { entityId: { ...idSchema } } :
    profile === 'outlook.get-message' ? { messageId: structuredClone(encoded) } :
    profile === 'hubspot.get-contact' ? { contactId: { type: 'string', maxLength: 64, pattern: `^[0-9]+${whole}` },
      additionalPropertiesToRetrieve: { anyOf: [{ type: 'array', maxItems: 100, uniqueItems: true, items: { type: 'string', maxLength: 128, pattern: `^[a-z][a-z0-9_]*${whole}` } }, { type: 'null' }] } } :
    profile === 'supabase.get-user' ? { userId: { type: 'string', pattern: uuidPattern } } :
    profile === 'sendgrid.find-list-by-name' ? { name: { ...scalarSchema, not: { pattern: '^\\s*$' } } } :
    { collection_id: { type: 'string', pattern: hexPattern }, collection_item_id: { type: 'string', pattern: hexPattern } };
  return { action: actions[profile], inputSchema: { type: 'object', properties, additionalProperties: false,
    required: profile === 'google-slides.get-presentation' ? ['presentation_id'] : profile === 'hubspot.get-contact' ? ['contactId'] : profile === 'onedrive.get-item' ? [] : Object.keys(properties),
    ...(profile === 'onedrive.get-item' ? { minProperties: 1, maxProperties: 1 } : {}) } };
}
export function contentInputs(selection: Selection, value: Record<string, unknown>): { props: Record<string, unknown>; pathname: string; query?: Record<string, string>; inlineQuery?: string } | undefined {
  const profile = selection.profile;
  if (!hasProfile(profile)) return undefined;
  const fail = (): never => { throw new ActivepiecesError('invalid_arguments'); };
  if (profile === 'freshdesk.get-contact' && keys(value, ['contactid']) && positive(value.contactid)) return {
    props: { contactid: String(value.contactid) }, pathname: `/api/v2/contacts/${value.contactid}`,
  };
  const simple = profile === 'google-forms.get-form' ? ['form_id', '/v1/forms/'] : profile === 'google-docs.get-document' ? ['documentId', '/v1/documents/'] :
    profile === 'google-drive.get-file' ? ['file_id', '/drive/v3/files/'] : undefined;
  if (simple && keys(value, [simple[0]!]) && id(value[simple[0]!])) return { props: { ...value }, pathname: simple[1]! + value[simple[0]!],
    ...(profile === 'google-drive.get-file' ? { query: { supportsAllDrives: 'true' } } : {}) };
  if (profile === 'google-slides.get-presentation' && keys(value, ['presentation_id', 'fields'], ['presentation_id']) &&
    typeof value.presentation_id === 'string' && value.presentation_id.length <= 176 && new RegExp(slidePattern).test(value.presentation_id)) {
    const query: Record<string, string> = {};
    if (Object.hasOwn(value, 'fields') && value.fields !== null) {
      if (typeof value.fields !== 'string' || value.fields.length > 1024 || !new RegExp(fieldPattern).test(value.fields)) return fail();
      if (value.fields.trim()) query.fields = value.fields.trim();
    }
    const selected = value.presentation_id.startsWith('https:') ? value.presentation_id.split('/')[5]! : value.presentation_id;
    return { props: { ...value }, pathname: `/v1/presentations/${selected}`, ...(Object.keys(query).length ? { query } : {}) };
  }
  if (profile === 'onedrive.get-item' && Object.keys(value).length === 1 && keys(value, ['itemId', 'path'], [])) {
    const item = value.itemId ?? value.path;
    if (!text(item) || !item.trim() || ['.', '..'].includes(item.trim())) return fail();
    if (Object.hasOwn(value, 'itemId')) return { props: { ...value }, pathname: item.trim() === 'root' ? '/v1.0/me/drive/root' : `/v1.0/me/drive/items/${encodeURIComponent(item.trim())}` };
    const path = item.trim().replace(/^\/+|\/+$/g, '');
    if (!path || path.split('/').some(part => !part || ['.', '..'].includes(part))) return fail();
    return { props: { ...value }, pathname: `/v1.0/me/drive/root:/${path.split('/').map(encodeURIComponent).join('/')}:` };
  }
  if (profile === 'contentful.get-entry' && keys(value, ['entityId']) && id(value.entityId)) {
    const props = (selection.auth as { props: { space: string; environment: string } }).props;
    return { props: { ...value }, pathname: `/spaces/${props.space}/environments/${props.environment}/entries/${value.entityId}` };
  }
  if (profile === 'outlook.get-message' && keys(value, ['messageId']) && text(value.messageId) && value.messageId.trim() && !['.', '..'].includes(value.messageId.trim())) {
    const props = (selection.auth as { props: { mailbox?: string } }).props;
    return { props: { ...value }, pathname: `/v1.0/${props.mailbox ? 'users/' + encodeURIComponent(props.mailbox) : 'me'}/messages/${encodeURIComponent(value.messageId.trim())}` };
  }
  if (profile === 'hubspot.get-contact' && keys(value, ['contactId', 'additionalPropertiesToRetrieve'], ['contactId']) &&
    typeof value.contactId === 'string' && /^[0-9]{1,64}$/.exec(value.contactId)?.[0] === value.contactId) {
    const extra = value.additionalPropertiesToRetrieve ?? [];
    if (!Array.isArray(extra) || extra.length > 100 || new Set(extra).size !== extra.length ||
      !extra.every(item => typeof item === 'string' && /^[a-z][a-z0-9_]{0,127}$/.exec(item)?.[0] === item)) return fail();
    return { props: { contactId: value.contactId, ...(Object.hasOwn(value, 'additionalPropertiesToRetrieve') ? { additionalPropertiesToRetrieve: structuredClone(value.additionalPropertiesToRetrieve) } : {}) },
      pathname: `/crm/v3/objects/contacts/${value.contactId}`, query: { properties: [...HUBSPOT_CONTACT_PROPERTIES, ...extra].join(',') } };
  }
  if (profile === 'supabase.get-user' && keys(value, ['userId']) && typeof value.userId === 'string' && new RegExp(uuidPattern).test(value.userId)) return {
    props: { ...value }, pathname: `/auth/v1/admin/users/${value.userId}`,
  };
  if (profile === 'sendgrid.find-list-by-name' && keys(value, ['name']) && text(value.name) && value.name.trim()) return {
    props: { ...value }, pathname: '/v3/marketing/lists', query: { page_size: '1000' }, inlineQuery: '?page_size=1000',
  };
  if (profile === 'webflow.get-collection-item' && keys(value, ['collection_id', 'collection_item_id']) &&
    typeof value.collection_id === 'string' && new RegExp(hexPattern).test(value.collection_id) &&
    typeof value.collection_item_id === 'string' && new RegExp(hexPattern).test(value.collection_item_id)) return {
      props: { ...value }, pathname: `/v2/collections/${value.collection_id}/items/${value.collection_item_id}`, query: {},
    };
  return fail();
}

/** Bind actual SDK-generated headers; incidental SDK headers remain explicit. */
export function contentHeaders(profile: string, secret: string): Record<string, string> | undefined {
  if (profile === 'freshdesk.get-contact') return { Authorization: `Basic ${Buffer.from(`${secret}:X`).toString('base64')}`, 'Content-Type': 'application/json' };
  if (profile === 'google-forms.get-form') return { Authorization: `Bearer ${secret}` };
  if (profile === 'google-docs.get-document' || profile === 'google-drive.get-file') return {
    'accept-encoding': 'gzip', authorization: `Bearer ${secret}`, 'user-agent': 'google-api-nodejs-client/8.0.3 (gzip)',
    'x-goog-api-client': `gdcl/8.0.3 gl-node/${process.versions.node}`,
  };
  if (profile === 'contentful.get-entry') {
    const os: Record<string, string> = { android: 'Android', aix: 'Linux', darwin: 'macOS', freebsd: 'Linux', linux: 'Linux', openbsd: 'Linux', sunos: 'Linux', win32: 'Windows' };
    return { accept: 'application/json, text/plain, */*', authorization: `Bearer ${secret}`, 'content-type': 'application/vnd.contentful.management.v1+json',
      'x-contentful-user-agent': `sdk contentful-management-plain.js/11.48.1; platform node.js/${process.version};${os[process.platform] ? ` os ${os[process.platform]}/${process.version};` : ''}` };
  }
  if (profile === 'outlook.get-message') return { authorization: `Bearer ${secret}`, prefer: 'outlook.body-content-type="text"' };
  if (profile === 'hubspot.get-contact') return { authorization: `Bearer ${secret}`, accept: 'application/json, */*;q=0.8' };
  if (profile === 'supabase.get-user') return { authorization: `Bearer ${secret}`, apikey: secret, 'x-supabase-api-version': '2024-01-01' };
  return undefined;
}

/** Minimal response identity/envelope checks before the real donor transforms. */
export function contentBodyFailure(selection: Selection, pathname: string, query: Readonly<Record<string, string>> | undefined, body: unknown): 'invalid_response' | 'response_too_large' | undefined {
  const profile = selection.profile;
  if (!hasProfile(profile)) return undefined;
  const bad = 'invalid_response';
  if (!record(body)) return bad;
  const last = decodeURIComponent(pathname.slice(pathname.lastIndexOf('/') + 1));
  if (profile === 'freshdesk.get-contact') return body.id === Number(last) ? undefined : bad;
  if (profile === 'google-forms.get-form') return body.formId === last ? undefined : bad;
  if (profile === 'google-docs.get-document') return body.documentId === last ? undefined : bad;
  if (profile === 'google-slides.get-presentation') return body.presentationId === last || (!!query?.fields && !Object.hasOwn(body, 'presentationId')) ? undefined : bad;
  if (profile === 'google-drive.get-file' || profile === 'outlook.get-message') return body.id === last ? undefined : bad;
  if (profile === 'onedrive.get-item') return typeof body.id === 'string' && body.id.length > 0 && typeof body.name === 'string' &&
    (!pathname.startsWith('/v1.0/me/drive/items/') || body.id === last) ? undefined : bad;
  if (profile === 'contentful.get-entry') return record(body.sys) && body.sys.id === last && record(body.fields) ? undefined : bad;
  if (profile === 'hubspot.get-contact') return body.id === last && record(body.properties) && Object.values(body.properties).every(value => value === null || typeof value === 'string') &&
    ['createdAt', 'updatedAt', 'archivedAt'].every(key => !Object.hasOwn(body, key) || body[key] === null || (typeof body[key] === 'string' && Number.isFinite(Date.parse(body[key])))) ? undefined : bad;
  if (profile === 'supabase.get-user') {
    const user = body.user ?? body;
    return record(user) && user.id === last && typeof user.created_at === 'string' ? undefined : bad;
  }
  if (profile === 'webflow.get-collection-item') return body.id === last && record(body.fieldData) ? undefined : bad;
  if (profile === 'sendgrid.find-list-by-name') {
    if (!Array.isArray(body.result) || !body.result.every(item => record(item) && typeof item.id === 'string' && item.id.length > 0 && typeof item.name === 'string') ||
      (body._metadata != null && !record(body._metadata))) return bad;
    const next = record(body._metadata) ? body._metadata.next : undefined;
    if (next != null && typeof next !== 'string') return bad;
    // The donor follows arbitrary next URLs and reports a complete exact-name
    // search. Reject a paginated catalog before its loop can return a false miss.
    return body.result.length > 1000 || !!next ? 'response_too_large' : undefined;
  }
  return bad;
}

/** The real HubSpot deserializer returns one model with Date-valued fields. */
export function contentResult(profile: string, value: unknown): unknown {
  if (profile !== 'hubspot.get-contact') return value;
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw 0;
  const projected = { ...value } as Record<string, unknown>;
  if (Object.keys(projected).some(key => !['createdAt', 'archived', 'archivedAt', 'propertiesWithHistory', 'id', 'properties', 'updatedAt'].includes(key))) throw 0;
  for (const key of ['createdAt', 'updatedAt', 'archivedAt']) {
    if (!Object.hasOwn(projected, key) || projected[key] === null) continue;
    const item = projected[key];
    if (!(item instanceof Date) || !Number.isFinite(item.getTime())) throw 0;
    projected[key] = item.toISOString();
  }
  return projected;
}
