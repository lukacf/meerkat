import { EFFECT_PROFILE_IDS, isWriteProfile, selectEffect, describeEffect, effectInputs, type EffectReadConfiguration, type WriteConfiguration, type EffectPlan } from './effect-profiles.js';
import {SDK_PROFILE_IDS,selectSdk,describeSdk,sdkInputs,type SdkConfiguration,type SdkPlan} from './sdk-profiles.js';
import { READ_PROFILE_IDS, selectRead, describeRead, readInputs, type ReadConfiguration, type ReadPlan } from './read-profiles.js';
import { CONTENT_PROFILE_IDS, selectContent, describeContent, contentInputs, type ContentConfiguration } from "./content.js";
import { ActivepiecesError } from "./errors.js";

export const PROFILE_IDS = [
  ...CONTENT_PROFILE_IDS, ...READ_PROFILE_IDS, ...SDK_PROFILE_IDS, ...EFFECT_PROFILE_IDS,
  "tally.get-form", "asana.get-task", "nocodb.get-record", "baserow.get-row", "bitly.get-bitlink-details",
  "clockify.find-running-timer", "coda.get-row", "pagerduty.get-incident", "postmark.get-delivery-stats",
  "typeform.get-form-messages", "google-contacts.search-contact", "figma.get-comments", "acuity.find-appointments",
  "confluence.get-page-content", "brevo.find-contact", "jotform.get-submission",
  "kit.get-subscriber", "mailer-lite.get-campaign", "calendly.get-scheduled-event",
  "google-tasks.get-task", "campaign-monitor.find-subscriber", "okta.find-user-by-email", "help-scout.find-conversation",
  "airtable.get-record", "toggl.find-project", "stripe.get-invoice", "pipedrive.get-activity",
  "wordpress.get-page", "google-calendar.get-event", "notion.get-page", "intercom.get-conversation",
] as const;
export type ProfileId = typeof PROFILE_IDS[number];
export function isProfile(value: unknown): value is ProfileId {
  return typeof value === "string" && (PROFILE_IDS as readonly string[]).includes(value);
}
/** A trusted host transport must obey URL, signal and redirect:error without replay or retargeting. */
export type HostTransport = (url: string, init: RequestInit) => Promise<Response>;
export interface CommonConfiguration {
  readonly transport: HostTransport;
  readonly timeoutMs?: number;
  readonly maxResponseBytes?: number;
}
export type ReaderConfiguration = CommonConfiguration & (ContentConfiguration | ReadConfiguration | EffectReadConfiguration | SdkConfiguration |
  { readonly profile: "tally.get-form"; readonly auth: { readonly apiKey: string } } |
  { readonly profile: "asana.get-task"; readonly auth: { readonly accessToken: string } } |
  { readonly profile: "nocodb.get-record"; readonly auth: {
    readonly origin: string; readonly apiToken: string; readonly version: 3 | 4;
  } } |
  { readonly profile: "baserow.get-row"; readonly auth: { readonly origin: string; readonly databaseToken: string } } |
  { readonly profile: "bitly.get-bitlink-details" | "typeform.get-form-messages" | "google-contacts.search-contact" | "figma.get-comments" | "acuity.find-appointments"; readonly auth: { readonly accessToken: string } } |
  { readonly profile: "clockify.find-running-timer" | "coda.get-row" | "pagerduty.get-incident"; readonly auth: { readonly apiKey: string } } |
  { readonly profile: "postmark.get-delivery-stats"; readonly auth: { readonly serverToken: string } } |
  { readonly profile: "brevo.find-contact"; readonly auth: { readonly apiKey: string } } |
  { readonly profile: "confluence.get-page-content"; readonly auth: { readonly origin: string; readonly username: string; readonly apiToken: string } } |
  { readonly profile: "jotform.get-submission"; readonly auth: { readonly apiKey: string; readonly region: "us" | "eu" | "hipaa" } } |
  { readonly profile: "kit.get-subscriber"; readonly auth: { readonly apiSecret: string } } |
  { readonly profile: "mailer-lite.get-campaign"; readonly auth: { readonly apiKey: string } } |
  { readonly profile: "calendly.get-scheduled-event"; readonly auth: { readonly personalToken: string } } |
  { readonly profile: "google-tasks.get-task" | "help-scout.find-conversation"; readonly auth: { readonly accessToken: string } } |
  { readonly profile: "campaign-monitor.find-subscriber"; readonly auth: { readonly apiKey: string } } |
  { readonly profile: "okta.find-user-by-email"; readonly auth: { readonly origin: string; readonly apiToken: string } } |
  { readonly profile: "airtable.get-record" | "notion.get-page" | "google-calendar.get-event"; readonly auth: { readonly accessToken: string } } |
  { readonly profile: "stripe.get-invoice"; readonly auth: { readonly apiKey: string } } |
  { readonly profile: "toggl.find-project"; readonly auth: { readonly apiToken: string } } |
  { readonly profile: "pipedrive.get-activity"; readonly auth: { readonly origin: string; readonly accessToken: string } } |
  { readonly profile: "wordpress.get-page"; readonly auth: { readonly origin: string; readonly username: string; readonly applicationPassword: string } } |
  { readonly profile: "intercom.get-conversation"; readonly auth: { readonly accessToken: string; readonly region: "us" | "eu" | "au" } }
);
export type WriterConfiguration = CommonConfiguration & WriteConfiguration;
export type OperationConfiguration = ReaderConfiguration | WriterConfiguration;
export interface Description {
  readonly profile: ProfileId;
  readonly action: string;
  readonly effect: "read" | "write";
  readonly inputSchema: Record<string, unknown>;
  readonly omittedSetupCallbacks: readonly string[];
}
export interface Selection {
  profile: ProfileId;
  transport: HostTransport;
  timeoutMs: number;
  maxResponseBytes: number;
  origin: string;
  auth: Record<string, unknown> | string;
  secret: string;
  version?: 3 | 4;
}
export function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value) &&
    (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);
}
export function configurationKeys(value: Record<string, unknown>, allowed: string[], required = allowed): boolean {
  return Object.keys(value).every(key => allowed.includes(key)) && required.every(key => Object.hasOwn(value, key));
}
export function selectedSecret(value: unknown): value is string {
  return typeof value === "string" && /^[\x21-\x7e]{1,4096}$/.exec(value)?.[0] === value;
}
export function canonicalOrigin(value: unknown): string {
  if (typeof value !== "string") throw 0;
  const url = new URL(value);
  if (url.protocol !== "https:" || url.origin !== value || url.username || url.password || url.search || url.hash) throw 0;
  return url.origin;
}
const keys = configurationKeys, token = selectedSecret;
export function select(configuration: OperationConfiguration): Selection {
  try {
    if (!record(configuration) || !keys(configuration, ["profile", "auth", "transport", "timeoutMs", "maxResponseBytes"],
      ["profile", "auth", "transport"]) || typeof configuration.transport !== "function" || !record(configuration.auth)) throw 0;
    const timeoutMs = configuration.timeoutMs === undefined ? 10_000 : configuration.timeoutMs;
    const maxResponseBytes = configuration.maxResponseBytes === undefined ? 1_048_576 : configuration.maxResponseBytes;
    if (!Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 60_000 ||
      !Number.isInteger(maxResponseBytes) || maxResponseBytes < 1 || maxResponseBytes > 4_194_304) throw 0;
    if (!isProfile(configuration.profile)) throw 0;
    const base = { profile: configuration.profile, transport: configuration.transport, timeoutMs, maxResponseBytes };
    const effect = selectEffect(configuration.profile, configuration.auth);
    if (effect) return { ...base, ...effect };
    const sdk = selectSdk(configuration.profile, configuration.auth);
    if (sdk) return {...base,...sdk};
    const read = selectRead(configuration.profile, configuration.auth);
    if (read) return { ...base, ...read };
    const content = selectContent(configuration.profile, configuration.auth);
    if (content) return { ...base, ...content };
    if (configuration.profile === "tally.get-form" && keys(configuration.auth, ["apiKey"]) && token(configuration.auth.apiKey)) {
      return { ...base, origin: "https://api.tally.so", secret: configuration.auth.apiKey,
        auth: { type: "SECRET_TEXT", secret_text: configuration.auth.apiKey } };
    }
    if (configuration.profile === "asana.get-task" && keys(configuration.auth, ["accessToken"]) && token(configuration.auth.accessToken)) {
      return { ...base, origin: "https://app.asana.com", secret: configuration.auth.accessToken,
        auth: { access_token: configuration.auth.accessToken } };
    }
    if (configuration.profile === "nocodb.get-record" && keys(configuration.auth, ["origin", "apiToken", "version"]) &&
      token(configuration.auth.apiToken) && (configuration.auth.version === 3 || configuration.auth.version === 4)) {
      const origin = canonicalOrigin(configuration.auth.origin);
      return { ...base, origin, secret: configuration.auth.apiToken, version: configuration.auth.version,
        auth: { type: "CUSTOM_AUTH", props: { baseUrl: origin, apiToken: configuration.auth.apiToken, version: configuration.auth.version } } };
    }
    if (configuration.profile === "baserow.get-row" && keys(configuration.auth, ["origin", "databaseToken"]) && token(configuration.auth.databaseToken)) {
      const origin = canonicalOrigin(configuration.auth.origin);
      return { ...base, origin, secret: configuration.auth.databaseToken,
        auth: { type: "CUSTOM_AUTH", props: { authType: "database_token", apiUrl: origin, token: configuration.auth.databaseToken } } };
    }
    if (configuration.profile === "bitly.get-bitlink-details" && keys(configuration.auth, ["accessToken"]) && token(configuration.auth.accessToken)) {
      return { ...base, origin: "https://api-ssl.bitly.com", secret: configuration.auth.accessToken,
        auth: { type: "CUSTOM_AUTH", props: { accessToken: configuration.auth.accessToken } } };
    }
    if (configuration.profile === "typeform.get-form-messages" && keys(configuration.auth, ["accessToken"]) && token(configuration.auth.accessToken)) {
      return { ...base, origin: "https://api.typeform.com", secret: configuration.auth.accessToken,
        auth: { access_token: configuration.auth.accessToken } };
    }
    if ((configuration.profile === "clockify.find-running-timer" || configuration.profile === "coda.get-row" || configuration.profile === "pagerduty.get-incident") &&
      keys(configuration.auth, ["apiKey"]) && token(configuration.auth.apiKey)) {
      const origin = configuration.profile === "clockify.find-running-timer" ? "https://api.clockify.me" :
        configuration.profile === "coda.get-row" ? "https://coda.io" : "https://api.pagerduty.com";
      return { ...base, origin, secret: configuration.auth.apiKey,
        auth: { type: "SECRET_TEXT", secret_text: configuration.auth.apiKey } };
    }
    if ((configuration.profile === "google-contacts.search-contact" || configuration.profile === "figma.get-comments" || configuration.profile === "acuity.find-appointments") &&
      keys(configuration.auth, ["accessToken"]) && token(configuration.auth.accessToken)) {
      const origin = configuration.profile === "google-contacts.search-contact" ? "https://people.googleapis.com" :
        configuration.profile === "figma.get-comments" ? "https://api.figma.com" : "https://acuityscheduling.com";
      return { ...base, origin, secret: configuration.auth.accessToken, auth: { access_token: configuration.auth.accessToken } };
    }
    if (configuration.profile === "postmark.get-delivery-stats" && keys(configuration.auth, ["serverToken"]) && token(configuration.auth.serverToken)) {
      return { ...base, origin: "https://api.postmarkapp.com", secret: configuration.auth.serverToken,
        auth: { type: "SECRET_TEXT", secret_text: configuration.auth.serverToken } };
    }
    if (configuration.profile === "brevo.find-contact" && keys(configuration.auth, ["apiKey"]) && token(configuration.auth.apiKey)) {
      return { ...base, origin: "https://api.brevo.com", secret: configuration.auth.apiKey,
        auth: { type: "SECRET_TEXT", secret_text: configuration.auth.apiKey } };
    }
    if (configuration.profile === "confluence.get-page-content" && keys(configuration.auth, ["origin", "username", "apiToken"]) &&
      token(configuration.auth.apiToken) && typeof configuration.auth.username === "string" &&
      /^[\x21-\x39\x3b-\x7e]{1,254}$/.exec(configuration.auth.username)?.[0] === configuration.auth.username) {
      const origin = canonicalOrigin(configuration.auth.origin);
      return { ...base, origin, secret: configuration.auth.apiToken,
        auth: { type: "CUSTOM_AUTH", props: { confluenceDomain: origin,
          username: configuration.auth.username, password: configuration.auth.apiToken } } };
    }
    if (configuration.profile === "jotform.get-submission" && keys(configuration.auth, ["apiKey", "region"]) &&
      token(configuration.auth.apiKey) && ["us", "eu", "hipaa"].includes(configuration.auth.region)) {
      const origin = configuration.auth.region === "eu" ? "https://eu-api.jotform.com" :
        configuration.auth.region === "hipaa" ? "https://hipaa-api.jotform.com" : "https://api.jotform.com";
      return { ...base, origin, secret: configuration.auth.apiKey,
        auth: { type: "CUSTOM_AUTH", props: { apiKey: configuration.auth.apiKey, region: configuration.auth.region } } };
    }
    if (configuration.profile === "kit.get-subscriber" && keys(configuration.auth, ["apiSecret"]) && token(configuration.auth.apiSecret)) {
      return { ...base, origin: "https://api.convertkit.com", secret: configuration.auth.apiSecret,
        auth: { type: "SECRET_TEXT", secret_text: configuration.auth.apiSecret } };
    }
    if (configuration.profile === "mailer-lite.get-campaign" && keys(configuration.auth, ["apiKey"]) && token(configuration.auth.apiKey)) {
      return { ...base, origin: "https://connect.mailerlite.com", secret: configuration.auth.apiKey,
        auth: { type: "SECRET_TEXT", secret_text: configuration.auth.apiKey } };
    }
    if (configuration.profile === "calendly.get-scheduled-event" && keys(configuration.auth, ["personalToken"]) && token(configuration.auth.personalToken)) {
      return { ...base, origin: "https://api.calendly.com", secret: configuration.auth.personalToken,
        auth: { type: "SECRET_TEXT", secret_text: configuration.auth.personalToken } };
    }
    if ((configuration.profile === "google-tasks.get-task" || configuration.profile === "help-scout.find-conversation") &&
      keys(configuration.auth, ["accessToken"]) && token(configuration.auth.accessToken)) {
      return { ...base, origin: configuration.profile === "google-tasks.get-task" ? "https://tasks.googleapis.com" : "https://api.helpscout.net",
        secret: configuration.auth.accessToken, auth: { access_token: configuration.auth.accessToken } };
    }
    if (configuration.profile === "campaign-monitor.find-subscriber" && keys(configuration.auth, ["apiKey"]) &&
      token(configuration.auth.apiKey) && !configuration.auth.apiKey.includes(":")) {
      return { ...base, origin: "https://api.createsend.com", secret: configuration.auth.apiKey,
        auth: { type: "SECRET_TEXT", secret_text: configuration.auth.apiKey } };
    }
    if (configuration.profile === "okta.find-user-by-email" && keys(configuration.auth, ["origin", "apiToken"]) && token(configuration.auth.apiToken)) {
      const origin = canonicalOrigin(configuration.auth.origin);
      return { ...base, origin, secret: configuration.auth.apiToken,
        auth: { type: "CUSTOM_AUTH", props: { domain: origin, apiToken: configuration.auth.apiToken } } };
    }
    if ((configuration.profile === "airtable.get-record" || configuration.profile === "notion.get-page" || configuration.profile === "google-calendar.get-event") &&
      keys(configuration.auth, ["accessToken"]) && token(configuration.auth.accessToken)) {
      const secret = configuration.auth.accessToken;
      return { ...base, origin: configuration.profile === "airtable.get-record" ? "https://api.airtable.com" :
        configuration.profile === "notion.get-page" ? "https://api.notion.com" : "https://www.googleapis.com", secret,
        auth: configuration.profile === "airtable.get-record" ? { type: "SECRET_TEXT", secret_text: secret } :
          configuration.profile === "notion.get-page" ? { type: "CUSTOM_AUTH", props: { accessToken: secret } } :
            { type: "OAUTH2", access_token: secret } };
    }
    if (configuration.profile === "stripe.get-invoice" && keys(configuration.auth, ["apiKey"]) && token(configuration.auth.apiKey)) {
      return { ...base, origin: "https://api.stripe.com", secret: configuration.auth.apiKey,
        auth: { type: "SECRET_TEXT", secret_text: configuration.auth.apiKey } };
    }
    if (configuration.profile === "toggl.find-project" && keys(configuration.auth, ["apiToken"]) && token(configuration.auth.apiToken) && !configuration.auth.apiToken.includes(":")) {
      return { ...base, origin: "https://api.track.toggl.com", secret: configuration.auth.apiToken, auth: configuration.auth.apiToken };
    }
    if (configuration.profile === "pipedrive.get-activity" && keys(configuration.auth, ["origin", "accessToken"]) && token(configuration.auth.accessToken)) {
      const origin = canonicalOrigin(configuration.auth.origin);
      return { ...base, origin, secret: configuration.auth.accessToken, auth: { access_token: configuration.auth.accessToken, data: { api_domain: origin } } };
    }
    if (configuration.profile === "wordpress.get-page" && keys(configuration.auth, ["origin", "username", "applicationPassword"]) &&
      typeof configuration.auth.applicationPassword === "string" && /^[\x20-\x7e]{1,4096}$/.exec(configuration.auth.applicationPassword)?.[0] === configuration.auth.applicationPassword && configuration.auth.applicationPassword.trim().length > 0 && typeof configuration.auth.username === "string" &&
      /^[\x21-\x39\x3b-\x7e]{1,254}$/.exec(configuration.auth.username)?.[0] === configuration.auth.username) {
      const origin = canonicalOrigin(configuration.auth.origin);
      return { ...base, origin, secret: configuration.auth.applicationPassword,
        auth: { type: "CUSTOM_AUTH", props: { website_url: origin, username: configuration.auth.username, password: configuration.auth.applicationPassword } } };
    }
    if (configuration.profile === "intercom.get-conversation" && keys(configuration.auth, ["accessToken", "region"]) && token(configuration.auth.accessToken) &&
      ["us", "eu", "au"].includes(configuration.auth.region as string)) {
      const region = configuration.auth.region === "us" ? "intercom" : `${configuration.auth.region}.intercom`;
      return { ...base, origin: `https://api.${region}.io`, secret: configuration.auth.accessToken,
        auth: { type: "CUSTOM_AUTH", props: { accessToken: configuration.auth.accessToken, region } } };
    }
  } catch { /* All configuration failures have a fixed safe boundary. */ }
  throw new ActivepiecesError("invalid_configuration", false, false, undefined, isWriteProfile(configuration?.profile) ? "write" : "read");
}

// JSON Schema patterns use ECMAScript regexes: $ also matches before a final
// line terminator. This assertion requires the actual end of the input.
const endOfInput = "(?![\\s\\S])";
const segment = { type: "string", minLength: 1, maxLength: 128, pattern: `^[A-Za-z0-9_-]+${endOfInput}` };
const positiveInteger = { type: "integer", minimum: 1, maximum: Number.MAX_SAFE_INTEGER };
const encodedName = { type: "string", minLength: 1, maxLength: 256,
  // Accept Unicode scalar values in validators using either UTF-16 code units
  // or Unicode code points, without allowing an unpaired surrogate.
  pattern: `^(?:[^\\u0000-\\u001f\\u007f\\ud800-\\udfff]|[\\ud800-\\udbff][\\udc00-\\udfff])+${endOfInput}`,
  not: { enum: [".", ".."] }, description: "Manual ID or name, encoded as one URL path segment; must be well-formed Unicode." };
const domainLabel = "[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?";
const bitlinkPattern = `^(?=[^/]{1,253}/)(?:${domainLabel}\\.)+${domainLabel}/[A-Za-z0-9_-]{1,128}${endOfInput}`;
const contactFields = ["addresses", "ageRanges", "biographies", "birthdays", "calendarUrls", "clientData", "coverPhotos",
  "emailAddresses", "events", "externalIds", "genders", "imClients", "interests", "locales", "locations", "memberships",
  "metadata", "miscKeywords", "names", "nicknames", "occupations", "organizations", "phoneNumbers", "photos", "relations",
  "sipAddresses", "skills", "urls", "userDefined"];
const boundedText = { type: "string", minLength: 1, maxLength: 256, pattern: encodedName.pattern };
const brevoIdentifierTypes = ["email_id", "phone_id", "contact_id", "ext_id", "whatsapp_id", "landline_number_id"];
const numericId = { type: "string", minLength: 1, maxLength: 64, pattern: `^[0-9]+${endOfInput}` };
const calendlyEventPrefix = "https://api.calendly.com/scheduled_events/";
const taskId = { type: "string", minLength: 1, maxLength: 1024, pattern: `^[A-Za-z0-9_=-]+${endOfInput}` };
// These donor searches interpolate quoted strings without escaping them.
// Scope the public contract to ASCII dot-atom addresses, never SCIM operators.
const emailDomainLabel = "[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?";
const emailPattern = `^[A-Za-z0-9!#$%&'*+/=?^_\x60{|}~-]+(?:\\.[A-Za-z0-9!#$%&'*+/=?^_\x60{|}~-]+)*@(?:${emailDomainLabel}\\.)+${emailDomainLabel}${endOfInput}`;
const emailAddress = { type: "string", minLength: 3, maxLength: 254, pattern: emailPattern,
  description: "ASCII dot-atom email with a DNS domain; quoted, escaped and Unicode addresses are outside this profile." };
const searchLiteral = { ...boundedText, pattern: boundedText.pattern,
  allOf: [{ not: { pattern: '["\\\\]' } }, { not: { pattern: '^\\s*$' } }], description: "Nonblank literal subject, without quotes or backslashes." };
const helpScoutProperties = {
  subject: searchLiteral, email: emailAddress, mailboxId: { ...numericId, pattern: `^[1-9][0-9]*${endOfInput}` }, assignTo: { ...numericId, pattern: `^[1-9][0-9]*${endOfInput}` },
  status: { type: "string", enum: ["active", "open", "closed", "pending", "spam", "all"] },
  tags: { type: "array", minItems: 1, maxItems: 20, uniqueItems: true, items: { ...segment } },
  query: { ...boundedText, not: { pattern: '^\\s*$' }, description: "Nonblank explicit Help Scout search expression; combined with other filters using AND." },
};
const calendlyEvent = { type: "string", minLength: 1, maxLength: calendlyEventPrefix.length + 128,
  pattern: `^(?:https://api\\.calendly\\.com/scheduled_events/)?[A-Za-z0-9_-]{1,128}${endOfInput}`,
  description: "Scheduled-event UUID or exact https://api.calendly.com/scheduled_events/<UUID> URI." };
const datePattern = `^[0-9]{4}-(?:0[1-9]|1[0-2])-(?:0[1-9]|[12][0-9]|3[01])${endOfInput}`;
const dateOnly = { type: "string", format: "date", pattern: datePattern,
  description: "Calendar date YYYY-MM-DD, preserved without timezone conversion." };
const acuityProperties = {
  firstName: { ...boundedText }, lastName: { ...boundedText }, email: { ...boundedText }, phone: { ...boundedText },
  minDate: { ...dateOnly }, maxDate: { ...dateOnly }, calendarID: { ...positiveInteger }, appointmentTypeID: { ...positiveInteger },
  status: { type: "string", enum: ["scheduled", "canceled", "all"], default: "scheduled" },
  maxResults: { type: "integer", minimum: 1, maximum: 100, default: 100 },
  direction: { type: "string", enum: ["ASC", "DESC"], default: "DESC" },
};
const prefixedId = (prefix: string) => ({ ...segment, pattern: `^${prefix}[A-Za-z0-9]+${endOfInput}` });
const notionPage = { type: "string", pattern: `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}${endOfInput}`,
  description: "Lowercase UUID with hyphens. Returns page metadata and properties, not body blocks." };
const optionalText = { anyOf: [{ ...boundedText, minLength: 0, pattern: boundedText.pattern.replace("+", "*") }, { type: "null" }] };
const togglProperties = {
  workspace_id: { ...positiveInteger }, name: structuredClone(optionalText),
  active: { type: "string", enum: ["true", "false", "both"] },
  billable: { type: "boolean" }, only_me: { type: "boolean" }, only_templates: { type: "boolean" },
  page: { type: "integer", minimum: 1, maximum: 10_000, default: 1 },
  per_page: { type: "integer", minimum: 1, maximum: 200, default: 100 },
};
const calendarProperties = {
  calendar_id: { ...boundedText, not: { pattern: '^\\s*(?:\\.{1,2})?\\s*$' }, description: "Manual calendar ID, including primary or an email-like ID; outer whitespace is trimmed." },
  event_id: { type: "string", minLength: 5, maxLength: 1024, pattern: `^[A-Za-z0-9_-]+${endOfInput}` },
  max_attendees: { type: "integer", minimum: 1, maximum: 200 },
  time_zone: { ...structuredClone(optionalText), description: "Valid IANA time zone supported by this Node runtime. Null, empty or whitespace omits the query parameter." },
};
const actions: Partial<Record<ProfileId, string>> = {
  "airtable.get-record": "get_record", "toggl.find-project": "find_project", "stripe.get-invoice": "get_invoice",
  "pipedrive.get-activity": "get-activity", "wordpress.get-page": "get_page", "google-calendar.get-event": "google_calendar_get_event",
  "notion.get-page": "notion_get_page", "intercom.get-conversation": "get-conversation",
  "tally.get-form": "get_form", "asana.get-task": "get_task", "nocodb.get-record": "nocodb-get-record",
  "baserow.get-row": "baserow_get_row", "bitly.get-bitlink-details": "get_bitlink_details",
  "clockify.find-running-timer": "find-running-timer", "coda.get-row": "get-row",
  "pagerduty.get-incident": "get_incident", "postmark.get-delivery-stats": "get_delivery_stats",
  "typeform.get-form-messages": "get_form_messages",
  "google-contacts.search-contact": "search_contact", "figma.get-comments": "get_comments", "acuity.find-appointments": "find_appointment",
  "confluence.get-page-content": "getPageContent", "brevo.find-contact": "find_contact", "jotform.get-submission": "jotform_get_submission",
  "kit.get-subscriber": "kit_get_subscriber", "mailer-lite.get-campaign": "get_campaign", "calendly.get-scheduled-event": "get_scheduled_event",
  "google-tasks.get-task": "get_task", "campaign-monitor.find-subscriber": "find_subscriber",
  "okta.find-user-by-email": "find_user_by_email", "help-scout.find-conversation": "find_conversation",
};
export function describe(profile: ProfileId): Description {
  const effect = describeEffect(profile);
  if (effect) return effect;
  const content = describeSdk(profile) ?? describeRead(profile) ?? describeContent(profile);
  if (content) return { profile, effect: "read", ...content, omittedSetupCallbacks: ["dynamic property lookup", "auth validation", "connection identifier", "OAuth acquisition and refresh"] };
  const properties: Record<string, unknown> = profile === "tally.get-form" || profile === "typeform.get-form-messages" ? { form_id: { ...segment } } :
    profile === "asana.get-task" ? { task: { type: "string", minLength: 1, maxLength: 64, pattern: `^[0-9]+${endOfInput}` } } :
    profile === "nocodb.get-record" ? { baseId: { ...segment }, tableId: { ...segment }, recordId: { ...positiveInteger } } :
    profile === "baserow.get-row" ? { table_id: { ...positiveInteger }, row_id: { ...positiveInteger } } :
    profile === "bitly.get-bitlink-details" ? { bitlink: { type: "string", minLength: 5, maxLength: 382, pattern: bitlinkPattern, description: "Lowercase DNS domain and short-link hash, for example bit.ly/abc123. No URL scheme." } } :
    profile === "clockify.find-running-timer" ? { workspaceId: { ...segment } } :
    profile === "coda.get-row" ? { docId: { ...segment }, tableId: structuredClone(encodedName), rowIdOrName: structuredClone(encodedName) } :
    profile === "pagerduty.get-incident" ? { incidentId: { ...segment } } :
    profile === "figma.get-comments" ? { file_key: { ...segment } } :
    profile === "kit.get-subscriber" ? { subscriber_id: { ...numericId } } :
    profile === "mailer-lite.get-campaign" ? { campaign_id: { ...segment } } :
    profile === "calendly.get-scheduled-event" ? { scheduledEvent: { ...calendlyEvent } } :
    profile === "google-tasks.get-task" ? { tasks_list: { ...taskId }, task_id: { ...taskId } } :
    profile === "airtable.get-record" ? { base_id: prefixedId("app"), table_id_or_name: { ...prefixedId("tbl"), description: "Manual table ID only; table names are outside this profile." }, record_id: prefixedId("rec") } :
    profile === "toggl.find-project" ? structuredClone(togglProperties) :
    profile === "stripe.get-invoice" ? { invoice_id: prefixedId("in_") } :
    profile === "pipedrive.get-activity" ? { activityId: { ...positiveInteger } } :
    profile === "wordpress.get-page" ? { page_id: { ...positiveInteger } } :
    profile === "google-calendar.get-event" ? structuredClone(calendarProperties) :
    profile === "notion.get-page" ? { page_id: { ...notionPage } } :
    profile === "intercom.get-conversation" ? { conversationId: { ...numericId } } :
    profile === "campaign-monitor.find-subscriber" ? { listId: { ...segment }, email: { ...emailAddress } } :
    profile === "okta.find-user-by-email" ? { email: { ...emailAddress } } :
    profile === "help-scout.find-conversation" ? structuredClone(helpScoutProperties) :
    profile === "confluence.get-page-content" ? { pageId: { ...numericId } } :
    profile === "jotform.get-submission" ? { submissionId: { ...numericId } } :
    profile === "brevo.find-contact" ? { identifier: structuredClone(encodedName),
      identifier_type: { type: "string", enum: [...brevoIdentifierTypes], default: "email_id" } } :
    profile === "google-contacts.search-contact" ? { query: { ...boundedText },
      readMask: { type: "array", minItems: 1, maxItems: contactFields.length, uniqueItems: true, items: { type: "string", enum: [...contactFields] } },
      pageSize: { type: "integer", minimum: 1, maximum: 30, default: 30 } } :
    profile === "acuity.find-appointments" ? structuredClone(acuityProperties) : {};
  return { profile, effect: "read", action: actions[profile]!,
  inputSchema: { type: "object", properties,
    ...(profile === "help-scout.find-conversation" ? { minProperties: 1 } : {}),
    required: profile === "toggl.find-project" ? ["workspace_id"] : profile === "google-calendar.get-event" ? ["calendar_id", "event_id"] :
      profile === "google-contacts.search-contact" ? ["query", "readMask"] : profile === "acuity.find-appointments" || profile === "help-scout.find-conversation" ? [] :
      profile === "brevo.find-contact" ? ["identifier"] : Object.keys(properties),
    additionalProperties: false },
  omittedSetupCallbacks: ["dynamic property lookup", "auth validation", "connection identifier", "OAuth acquisition and refresh"] };
}
export function inputs(selection: Selection, value: unknown): { props: Record<string, unknown>; pathname: string; query?: Record<string, string>; inlineQuery?: string; read?: ReadPlan | SdkPlan; effect?: EffectPlan } {
  const fail = () => { throw new ActivepiecesError("invalid_arguments"); };
  if (!record(value)) return fail();
  const effect = effectInputs(selection, value);
  if (effect) return effect;
  const content = sdkInputs(selection,value) ?? readInputs(selection, value) ?? contentInputs(selection, value);
  if (content) return content;
  const id = (item: unknown): item is string => typeof item === "string" && /^[A-Za-z0-9_-]{1,128}$/.exec(item)?.[0] === item;
  const positive = (item: unknown): item is number => Number.isSafeInteger(item) && (item as number) >= 1;
  const name = (item: unknown): item is string => typeof item === "string" && item.length >= 1 && item.length <= 512 && [...item].length <= 256 &&
    item !== "." && item !== ".." && !/[\x00-\x1f\x7f]/.test(item);
  if (selection.profile === "tally.get-form" && keys(value, ["form_id"]) && id(value.form_id)) {
    return { props: { form_id: value.form_id }, pathname: `/forms/${value.form_id}` };
  }
  if (selection.profile === "asana.get-task" && keys(value, ["task"]) && typeof value.task === "string" && /^[0-9]{1,64}$/.exec(value.task)?.[0] === value.task) {
    return { props: { task: value.task }, pathname: `/api/1.0/tasks/${value.task}` };
  }
  if (selection.profile === "nocodb.get-record" && keys(value, ["baseId", "tableId", "recordId"]) &&
    id(value.baseId) && id(value.tableId) && Number.isSafeInteger(value.recordId) && (value.recordId as number) >= 1) {
    return { props: { baseId: value.baseId, tableId: value.tableId, recordId: value.recordId }, pathname: selection.version === 4 ?
      `/api/v3/data/${value.baseId}/${value.tableId}/records/${value.recordId}` : `/api/v2/tables/${value.tableId}/records/${value.recordId}` };
  }
  if (selection.profile === "baserow.get-row" && keys(value, ["table_id", "row_id"]) && positive(value.table_id) && positive(value.row_id)) {
    return { props: { table_id: value.table_id, row_id: value.row_id }, pathname: `/api/database/rows/table/${value.table_id}/${value.row_id}/` };
  }
  if (selection.profile === "bitly.get-bitlink-details" && keys(value, ["bitlink"]) && typeof value.bitlink === "string" &&
    value.bitlink.length <= 382 && new RegExp(bitlinkPattern).exec(value.bitlink)?.[0] === value.bitlink) {
    const [domain] = value.bitlink.split("/");
    if (!domain || domain.length > 253) return fail();
    return { props: { bitlink: value.bitlink }, pathname: `/v4/bitlinks/${value.bitlink}` };
  }
  if (selection.profile === "clockify.find-running-timer" && keys(value, ["workspaceId"]) && id(value.workspaceId)) {
    return { props: { workspaceId: value.workspaceId }, pathname: `/api/v1/workspaces/${value.workspaceId}/time-entries/status/in-progress` };
  }
  if (selection.profile === "coda.get-row" && keys(value, ["docId", "tableId", "rowIdOrName"]) && id(value.docId) && name(value.tableId) && name(value.rowIdOrName)) {
    // encodeURIComponent also refuses malformed surrogate sequences before donor entry.
    return { props: { docId: value.docId, tableId: value.tableId, rowIdOrName: value.rowIdOrName },
      pathname: `/apis/v1/docs/${value.docId}/tables/${encodeURIComponent(value.tableId)}/rows/${encodeURIComponent(value.rowIdOrName)}` };
  }
  if (selection.profile === "pagerduty.get-incident" && keys(value, ["incidentId"]) && id(value.incidentId)) {
    return { props: { incidentId: value.incidentId }, pathname: `/incidents/${encodeURIComponent(value.incidentId)}` };
  }
  if (selection.profile === "postmark.get-delivery-stats" && keys(value, [])) return { props: {}, pathname: "/deliverystats" };
  if (selection.profile === "typeform.get-form-messages" && keys(value, ["form_id"]) && id(value.form_id)) {
    return { props: { form_id: value.form_id }, pathname: `/forms/${encodeURIComponent(value.form_id)}/messages` };
  }
  if (selection.profile === "figma.get-comments" && keys(value, ["file_key"]) && id(value.file_key)) {
    return { props: { file_key: value.file_key }, pathname: `/v1/files/${value.file_key}/comments` };
  }
  const text = (item: unknown): item is string => typeof item === "string" && [...item].length <= 256 &&
    new RegExp(boundedText.pattern).exec(item)?.[0] === item;
  const numeric = (item: unknown): item is string => typeof item === "string" && /^[0-9]{1,64}$/.exec(item)?.[0] === item;
  const prefixed = (item: unknown, prefix: string): item is string => id(item) && new RegExp(prefixedId(prefix).pattern).test(item);
  if (selection.profile === "airtable.get-record" && keys(value, ["base_id", "table_id_or_name", "record_id"]) &&
    prefixed(value.base_id, "app") && prefixed(value.table_id_or_name, "tbl") && prefixed(value.record_id, "rec")) {
    return { props: { ...value }, pathname: `/v0/${value.base_id}/${value.table_id_or_name}/${value.record_id}` };
  }
  if (selection.profile === "stripe.get-invoice" && keys(value, ["invoice_id"]) && prefixed(value.invoice_id, "in_")) {
    return { props: { ...value }, pathname: `/v1/invoices/${value.invoice_id}` };
  }
  if (selection.profile === "pipedrive.get-activity" && keys(value, ["activityId"]) && positive(value.activityId)) {
    return { props: { ...value }, pathname: `/api/v2/activities/${value.activityId}`, query: { include_fields: "attendees" } };
  }
  if (selection.profile === "wordpress.get-page" && keys(value, ["page_id"]) && positive(value.page_id)) {
    return { props: { ...value }, pathname: `/wp-json/wp/v2/pages/${value.page_id}` };
  }
  if (selection.profile === "notion.get-page" && keys(value, ["page_id"]) && typeof value.page_id === "string" && new RegExp(notionPage.pattern).test(value.page_id)) {
    return { props: { ...value }, pathname: `/v1/pages/${value.page_id}` };
  }
  if (selection.profile === "intercom.get-conversation" && keys(value, ["conversationId"]) && numeric(value.conversationId)) {
    return { props: { ...value }, pathname: `/conversations/${value.conversationId}`, query: {} };
  }
  const optional = (item: unknown) => item === null || item === "" || text(item);
  if (selection.profile === "toggl.find-project" && keys(value, Object.keys(togglProperties), ["workspace_id"]) && positive(value.workspace_id)) {
    const props: Record<string, unknown> = { ...value }, query: Record<string, string> = {};
    if (Object.hasOwn(value, "name")) {
      if (!optional(value.name)) return fail();
      if (value.name) query.name = value.name as string;
    }
    if (Object.hasOwn(value, "active")) {
      if (typeof value.active !== "string" || !togglProperties.active.enum.includes(value.active)) return fail();
      query.active = value.active;
    }
    for (const key of ["billable", "only_me", "only_templates"]) {
      if (Object.hasOwn(value, key)) {
        if (typeof value[key] !== "boolean") return fail();
        query[key] = String(value[key]);
      }
    }
    for (const [key, fallback, maximum] of [["page", 1, 10_000], ["per_page", 100, 200]] as const) {
      const item = Object.hasOwn(value, key) ? value[key] : fallback;
      if (!positive(item) || item > maximum) return fail();
      props[key] = item; query[key] = String(item);
    }
    return { props, pathname: `/api/v9/workspaces/${value.workspace_id}/projects`, query };
  }
  if (selection.profile === "google-calendar.get-event" && keys(value, Object.keys(calendarProperties), ["calendar_id", "event_id"]) &&
    text(value.calendar_id) && value.calendar_id.trim().length > 0 && ![".", ".."].includes(value.calendar_id.trim()) && typeof value.event_id === "string" && value.event_id.length >= 5 && value.event_id.length <= 1024 && new RegExp(calendarProperties.event_id.pattern).test(value.event_id)) {
    const props: Record<string, unknown> = { ...value }, query: Record<string, string> = {};
    if (Object.hasOwn(value, "max_attendees")) {
      if (!positive(value.max_attendees) || value.max_attendees > 200) return fail();
      query.maxAttendees = String(value.max_attendees);
    }
    if (Object.hasOwn(value, "time_zone")) {
      if (!optional(value.time_zone)) return fail();
      if (typeof value.time_zone === "string" && value.time_zone.trim()) {
        const zone = value.time_zone.trim();
        try { new Intl.DateTimeFormat("en", { timeZone: zone }); } catch { return fail(); }
        query.timeZone = zone;
      }
    }
    return { props, pathname: `/calendar/v3/calendars/${encodeURIComponent(value.calendar_id.trim())}/events/${value.event_id}`, query };
  }
  const email = (item: unknown): item is string => typeof item === "string" && item.length <= 254 && new RegExp(emailPattern).test(item);
  if (selection.profile === "google-tasks.get-task" && keys(value, ["tasks_list", "task_id"]) &&
    [value.tasks_list, value.task_id].every(item => typeof item === "string" && item.length <= 1024 && new RegExp(taskId.pattern).test(item))) {
    return { props: { tasks_list: value.tasks_list, task_id: value.task_id }, pathname: `/tasks/v1/lists/${value.tasks_list}/tasks/${value.task_id}` };
  }
  if (selection.profile === "campaign-monitor.find-subscriber" && keys(value, ["listId", "email"]) && id(value.listId) && email(value.email)) {
    return { props: { listId: value.listId, email: value.email }, pathname: `/api/v3.3/subscribers/${value.listId}.json`,
      inlineQuery: `?email=${encodeURIComponent(value.email)}&includetrackingpreference=true`,
      query: { email: value.email, includetrackingpreference: "true" } };
  }
  if (selection.profile === "okta.find-user-by-email" && keys(value, ["email"]) && email(value.email)) {
    const search = `profile.email eq "${value.email}"`;
    return { props: { email: value.email }, pathname: "/api/v1/users", inlineQuery: `?search=${search}`, query: { search } };
  }
  if (selection.profile === "help-scout.find-conversation" && keys(value, Object.keys(helpScoutProperties), []) && Object.keys(value).length > 0) {
    const props: Record<string, unknown> = {}, query: Record<string, string> = {}, expression: string[] = [];
    for (const key of ["subject", "email", "query"] as const) {
      if (!Object.hasOwn(value, key)) continue;
      if (key === "email" ? !email(value[key]) : !text(value[key])) return fail();
      const item = value[key] as string;
      if (item.trim().length === 0) return fail();
      if (key === "subject" && /["\\]/.test(item)) return fail();
      props[key] = item;
      expression.push(key === "query" ? item : `${key}:"${item}"`);
    }
    for (const [key, parameter] of [["mailboxId", "mailbox"], ["assignTo", "assigned_to"]] as const) {
      if (!Object.hasOwn(value, key)) continue;
      if (!numeric(value[key]) || value[key].startsWith("0")) return fail();
      props[key] = value[key]; query[parameter] = value[key];
    }
    if (Object.hasOwn(value, "status")) {
      if (typeof value.status !== "string" || !helpScoutProperties.status.enum.includes(value.status)) return fail();
      props.status = value.status; query.status = value.status;
    }
    if (Object.hasOwn(value, "tags")) {
      if (!Array.isArray(value.tags) || value.tags.length < 1 || value.tags.length > 20 || !value.tags.every(id) || new Set(value.tags).size !== value.tags.length) return fail();
      props.tags = [...value.tags]; query.tags = value.tags.join(",");
    }
    if (expression.length) query.query = `(${expression.join(" AND ")})`;
    const ordered = Object.fromEntries(["mailbox", "status", "tags", "assigned_to", "query"]
      .filter(key => Object.hasOwn(query, key)).map(key => [key, query[key]!])) as Record<string, string>;
    return { props, pathname: "/v2/conversations", query: ordered };
  }
  if (selection.profile === "kit.get-subscriber" && keys(value, ["subscriber_id"]) && numeric(value.subscriber_id)) {
    return { props: { subscriber_id: value.subscriber_id }, pathname: `/v3/subscribers/${value.subscriber_id}` };
  }
  if (selection.profile === "mailer-lite.get-campaign" && keys(value, ["campaign_id"]) && id(value.campaign_id)) {
    return { props: { campaign_id: value.campaign_id }, pathname: `/api/campaigns/${value.campaign_id}` };
  }
  if (selection.profile === "calendly.get-scheduled-event" && keys(value, ["scheduledEvent"]) && typeof value.scheduledEvent === "string" &&
    new RegExp(calendlyEvent.pattern).exec(value.scheduledEvent)?.[0] === value.scheduledEvent) {
    const eventId = value.scheduledEvent.startsWith(calendlyEventPrefix) ? value.scheduledEvent.slice(calendlyEventPrefix.length) : value.scheduledEvent;
    return { props: { scheduledEvent: value.scheduledEvent }, pathname: `/scheduled_events/${eventId}`, query: {} };
  }
  if (selection.profile === "confluence.get-page-content" && keys(value, ["pageId"]) && numeric(value.pageId)) {
    return { props: { pageId: value.pageId, includeDescendants: false, dynamic: {} },
      pathname: `/wiki/api/v2/pages/${value.pageId}`, query: { "body-format": "storage" } };
  }
  if (selection.profile === "jotform.get-submission" && keys(value, ["submissionId"]) && numeric(value.submissionId)) {
    return { props: { submissionId: value.submissionId }, pathname: `/submission/${value.submissionId}` };
  }
  if (selection.profile === "brevo.find-contact" && keys(value, ["identifier", "identifier_type"], ["identifier"]) &&
    text(value.identifier) && value.identifier !== "." && value.identifier !== "..") {
    const identifierType = value.identifier_type === undefined ? "email_id" : value.identifier_type;
    if (typeof identifierType !== "string" || !brevoIdentifierTypes.includes(identifierType)) return fail();
    return { props: { identifier: value.identifier, identifier_type: identifierType },
      pathname: `/v3/contacts/${encodeURIComponent(value.identifier)}`, query: { identifierType } };
  }
  if (selection.profile === "google-contacts.search-contact" && keys(value, ["query", "readMask", "pageSize"], ["query", "readMask"]) &&
    text(value.query) && Array.isArray(value.readMask) && value.readMask.length >= 1 && value.readMask.length <= contactFields.length &&
    value.readMask.every(field => typeof field === "string" && contactFields.includes(field)) && new Set(value.readMask).size === value.readMask.length) {
    const size = value.pageSize === undefined ? 30 : value.pageSize;
    if (!positive(size) || size > 30) return fail();
    return { props: { query: value.query, readMask: [...value.readMask], pageSize: size }, pathname: "/v1/people:searchContacts",
      query: { query: value.query, readMask: value.readMask.join(","), pageSize: String(size) } };
  }
  if (selection.profile === "acuity.find-appointments" && keys(value, Object.keys(acuityProperties), [])) {
    const props: Record<string, unknown> = { status: "scheduled", maxResults: 100, direction: "DESC" };
    const query: Record<string, string> = {};
    for (const key of ["firstName", "lastName", "email", "phone"] as const) {
      if (Object.hasOwn(value, key)) {
        if (!text(value[key])) return fail();
        props[key] = value[key]; query[key] = value[key];
      }
    }
    for (const key of ["minDate", "maxDate"] as const) {
      if (Object.hasOwn(value, key)) {
        const date = value[key];
        if (typeof date !== "string" || new RegExp(datePattern).exec(date)?.[0] !== date ||
          new Date(`${date}T00:00:00.000Z`).toISOString().slice(0, 10) !== date) return fail();
        props[key] = date; query[key] = date;
      }
    }
    if (typeof props.minDate === "string" && typeof props.maxDate === "string" && props.minDate > props.maxDate) return fail();
    for (const key of ["calendarID", "appointmentTypeID"] as const) {
      if (Object.hasOwn(value, key)) {
        if (!positive(value[key])) return fail();
        props[key] = value[key]; query[key] = String(value[key]);
      }
    }
    if (Object.hasOwn(value, "status")) {
      if (value.status !== "scheduled" && value.status !== "canceled" && value.status !== "all") return fail();
      props.status = value.status;
    }
    if (Object.hasOwn(value, "maxResults")) {
      if (!positive(value.maxResults) || value.maxResults > 100) return fail();
      props.maxResults = value.maxResults;
    }
    if (Object.hasOwn(value, "direction")) {
      if (value.direction !== "ASC" && value.direction !== "DESC") return fail();
      props.direction = value.direction;
    }
    if (props.status === "canceled") query.canceled = "true";
    if (props.status === "all") query.showall = "true";
    query.max = String(props.maxResults); query.direction = String(props.direction);
    return { props, pathname: "/api/v1/appointments", query };
  }
  return fail();
}
