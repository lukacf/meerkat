import { isProfile, record, select, describe, type ProfileId, type HostTransport, type ReaderConfiguration, type OperationConfiguration, type Description } from "./profiles.js";
import { open } from "node:fs/promises";
import { isWriteProfile } from './effect-profiles.js';
import {ghostKey} from './sdk-profiles.js';
import { configurationFields } from "./config-fields.js";

export interface McpConfiguration {
  readonly profile: ProfileId;
  readonly data_center?:string;
  readonly aws_region?:string;
  
  readonly access_key_id_env?:string;
  readonly session_token_env?:string;
  readonly token_env: string;
  readonly domain?: string;
  readonly owner?: string;
  readonly repository?: string;
  readonly site?: string;
  readonly bucket?: string;
  readonly origin?: string;
  readonly version?: 3 | 4;
  readonly username?: string;
  readonly region?: "us" | "eu" | "hipaa" | "au";
  readonly cloud?: "commercial" | "government";
  readonly mailbox?: string;
  readonly company_id?: string;
  readonly tenant_id?: string;
  readonly application_id?: string;
  readonly index_name?: string;
  readonly space?: string;
  readonly environment?: string;
  readonly timeout_ms?: number;
  readonly max_response_bytes?: number;
}
export class McpConfigurationError extends Error {
  constructor() { super("Activepieces MCP configuration is invalid"); this.name = "McpConfigurationError"; }
}
export function operationConfiguration(config: McpConfiguration, token: string, transport: HostTransport, environment?:Readonly<Record<string,string|undefined>>): OperationConfiguration {
  const common = { transport, timeoutMs: config.timeout_ms, maxResponseBytes: config.max_response_bytes };
  if (config.profile === "activecampaign.create-contact") return {...common,profile:config.profile,auth:{origin:config.origin!,apiKey:token}};
  if (config.profile === "freshservice.create-ticket") return {...common,profile:config.profile,auth:{domain:config.domain!,apiKey:token}};
  if (config.profile === "gitea.create-issue") return {...common,profile:config.profile,auth:{origin:config.origin!,accessToken:token,owner:config.owner!,repository:config.repository!}};
  if (config.profile === "datadog.submit-log") return {...common,profile:config.profile,auth:{site:config.site!,apiKey:token}};
  if (config.profile === "gmail.get-thread" || config.profile === "slack.get-thread-replies") return {...common,profile:config.profile,auth:{accessToken:token}};
  if (config.profile === "linear.get-issue") return {...common,profile:config.profile,auth:{apiKey:token}};
  if (config.profile === "google-cloud-storage.search-objects") return {...common,profile:config.profile,auth:{accessToken:token,bucket:config.bucket!}};
  if(config.profile==='ghost.get-post')return {...common,profile:config.profile,auth:{origin:config.origin!,adminApiKey:token}};
  if(config.profile==='monday.get-items')return {...common,profile:config.profile,auth:{apiToken:token}};
  if(config.profile==='mailchimp.get-campaign-report')return {...common,profile:config.profile,auth:{accessToken:token,dataCenter:config.data_center!}};
  if(config.profile==='amazon-s3.list-files')return {...common,profile:config.profile,auth:{secretAccessKey:token,accessKeyId:environment?environment[config.access_key_id_env!]!:'CONFIGURATIONCHECK',region:config.aws_region!,bucket:config.bucket!,...(config.session_token_env?{sessionToken:environment?environment[config.session_token_env]!:'configuration-check'}:{})}};
  if (config.profile === "jira-cloud.get-issue") return { ...common, profile:config.profile, auth:{origin:config.origin!,username:config.username!,apiToken:token} };
  if (config.profile === "dropbox.get-file-metadata" || config.profile === "google-sheets.get-values") return {...common,profile:config.profile,auth:{accessToken:token}};
  if (config.profile === "microsoft-excel-365.get-table-columns" || config.profile === "microsoft-teams.get-channel-message") return {...common,profile:config.profile,auth:{accessToken:token,cloud:config.cloud!}};
  if (config.profile === "quickbooks.find-invoice") return {...common,profile:config.profile,auth:{accessToken:token,companyId:config.company_id!}};
  if (config.profile === "xero.find-invoice") return {...common,profile:config.profile,auth:{accessToken:token,tenantId:config.tenant_id!}};
  if (config.profile === "algolia.browse-records") return {...common,profile:config.profile,auth:{apiKey:token,applicationId:config.application_id!,indexName:config.index_name!}};
  if (config.profile === "freshdesk.get-contact") return { ...common, profile: config.profile, auth: { origin: config.origin!, apiKey: token } };
  if (config.profile === "google-forms.get-form" || config.profile === "google-slides.get-presentation" || config.profile === "google-docs.get-document" || config.profile === "google-drive.get-file" || config.profile === "hubspot.get-contact" || config.profile === "webflow.get-collection-item") return { ...common, profile: config.profile, auth: { accessToken: token } };
  if (config.profile === "onedrive.get-item" || config.profile === "outlook.get-message") return { ...common, profile: config.profile, auth: { accessToken: token, cloud: config.cloud!, ...(config.mailbox !== undefined ? { mailbox: config.mailbox } : {}) } };
  if (config.profile === "contentful.get-entry") return { ...common, profile: config.profile, auth: { accessToken: token, space: config.space!, environment: config.environment! } };
  if (config.profile === "supabase.get-user") return { ...common, profile: config.profile, auth: { origin: config.origin!, serviceRoleKey: token } };
  if (config.profile === "sendgrid.find-list-by-name") return { ...common, profile: config.profile, auth: { apiKey: token, region: config.region as "us" | "eu" } };
  if (config.profile === "tally.get-form") return { ...common, profile: config.profile, auth: { apiKey: token } };
  if (config.profile === "asana.get-task") return { ...common, profile: config.profile, auth: { accessToken: token } };
  if (config.profile === "nocodb.get-record") return { ...common, profile: config.profile, auth: { origin: config.origin!, apiToken: token, version: config.version! } };
  if (config.profile === "baserow.get-row") return { ...common, profile: config.profile, auth: { origin: config.origin!, databaseToken: token } };
  if (config.profile === "bitly.get-bitlink-details" || config.profile === "typeform.get-form-messages" || config.profile === "google-contacts.search-contact" || config.profile === "figma.get-comments" || config.profile === "acuity.find-appointments") {
    return { ...common, profile: config.profile, auth: { accessToken: token } };
  }
  if (config.profile === "clockify.find-running-timer" || config.profile === "coda.get-row" || config.profile === "pagerduty.get-incident") {
    return { ...common, profile: config.profile, auth: { apiKey: token } };
  }
  if (config.profile === "postmark.get-delivery-stats") return { ...common, profile: config.profile, auth: { serverToken: token } };
  if (config.profile === "brevo.find-contact") return { ...common, profile: config.profile, auth: { apiKey: token } };
  if (config.profile === "confluence.get-page-content") {
    return { ...common, profile: config.profile, auth: { origin: config.origin!, username: config.username!, apiToken: token } };
  }
  if (config.profile === "jotform.get-submission") {
    return { ...common, profile: config.profile, auth: { apiKey: token, region: config.region as "us" | "eu" | "hipaa" } };
  }
  if (config.profile === "kit.get-subscriber") return { ...common, profile: config.profile, auth: { apiSecret: token } };
  if (config.profile === "mailer-lite.get-campaign") return { ...common, profile: config.profile, auth: { apiKey: token } };
  if (config.profile === "calendly.get-scheduled-event") return { ...common, profile: config.profile, auth: { personalToken: token } };
  if (config.profile === "google-tasks.get-task" || config.profile === "help-scout.find-conversation") return { ...common, profile: config.profile, auth: { accessToken: token } };
  if (config.profile === "campaign-monitor.find-subscriber") return { ...common, profile: config.profile, auth: { apiKey: token } };
  if (config.profile === "okta.find-user-by-email") return { ...common, profile: config.profile, auth: { origin: config.origin!, apiToken: token } };
  if (config.profile === "airtable.get-record" || config.profile === "notion.get-page" || config.profile === "google-calendar.get-event") {
    return { ...common, profile: config.profile, auth: { accessToken: token } };
  }
  if (config.profile === "stripe.get-invoice") return { ...common, profile: config.profile, auth: { apiKey: token } };
  if (config.profile === "toggl.find-project") return { ...common, profile: config.profile, auth: { apiToken: token } };
  if (config.profile === "pipedrive.get-activity") return { ...common, profile: config.profile, auth: { origin: config.origin!, accessToken: token } };
  if (config.profile === "wordpress.get-page") return { ...common, profile: config.profile, auth: { origin: config.origin!, username: config.username!, applicationPassword: token } };
  if (config.profile === "intercom.get-conversation") return { ...common, profile: config.profile, auth: { accessToken: token, region: config.region as "us" | "eu" | "au" } };
  throw new McpConfigurationError();
}
export function readerConfiguration(config:McpConfiguration,token:string,transport:HostTransport,environment?:Readonly<Record<string,string|undefined>>):ReaderConfiguration {
  if(isWriteProfile(config.profile))throw new McpConfigurationError();
  return operationConfiguration(config,token,transport,environment) as ReaderConfiguration;
}
export function parseConfiguration(value: unknown): McpConfiguration {
  try {
    if (!record(value) || !Object.hasOwn(value, "profile") || !Object.hasOwn(value, "token_env") ||
      !isProfile(value.profile) ||
      typeof value.token_env !== "string" || /^[A-Z][A-Z0-9_]{0,127}$/.exec(value.token_env)?.[0] !== value.token_env) throw 0;
    const fields = configurationFields(value.profile);
    const allowed = fields.map(field => field.name);
    if (fields.some(field => field.required && !Object.hasOwn(value, field.name))) throw 0;
    if (Object.keys(value).some(key => !allowed.includes(key))) throw 0;
    if(value.profile==='amazon-s3.list-files'){
      if(typeof value.access_key_id_env!=='string'||!/^[A-Z][A-Z0-9_]{0,127}(?![\s\S])/.test(value.access_key_id_env))throw 0;
      if(Object.hasOwn(value,'session_token_env')&&(typeof value.session_token_env!=='string'||!/^[A-Z][A-Z0-9_]{0,127}(?![\s\S])/.test(value.session_token_env)))throw 0;
    }
    const config = Object.freeze({ ...value }) as unknown as McpConfiguration;
    // Reuse the operation's semantic selection without importing its runtime.
    select(operationConfiguration(config, config.profile==="ghost.get-post"?"a".repeat(24)+":"+"b".repeat(64):"configuration-check", async () => { throw 0; }));
    return config;
  } catch { throw new McpConfigurationError(); }
}
export async function loadConfiguration(path: string): Promise<McpConfiguration> {
  let file;
  try {
    file = await open(path, "r");
    const stat = await file.stat();
    if (!stat.isFile() || stat.size > 8192) throw 0;
    const buffer = Buffer.alloc(8193);
    const { bytesRead } = await file.read(buffer, 0, buffer.length, 0);
    if (bytesRead > 8192) throw 0;
    return parseConfiguration(JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(buffer.subarray(0, bytesRead))));
  } catch { throw new McpConfigurationError(); }
  finally { await file?.close(); }
}
export function describeConfiguration(value: unknown): Description {
  const config = parseConfiguration(value);
  return describe(config.profile);
}
export function selectedToken(config: McpConfiguration, environment: Readonly<Record<string, string | undefined>>): string {
  try {
    if (!Object.hasOwn(environment, config.token_env)) throw 0;
    const token = environment[config.token_env];
    const pattern = config.profile === "wordpress.get-page" ? /^[\x20-\x7e]{1,4096}$/ : /^[\x21-\x7e]{1,4096}$/;
    if (typeof token !== "string" || pattern.exec(token)?.[0] !== token || token.trim().length === 0) throw 0;
    if(config.profile==="ghost.get-post"&&!ghostKey(token))throw 0;
    return token;
  } catch { throw new McpConfigurationError(); }
}

export function selectedEnvironment(config:McpConfiguration, environment:Readonly<Record<string,string|undefined>>):Readonly<Record<string,string>>{
  const names=[config.token_env,...(config.profile==='amazon-s3.list-files'?[config.access_key_id_env!,...(config.session_token_env?[config.session_token_env]:[])]:[])];
  const snapshot:Record<string,string>={};for(const name of names)snapshot[name]=selectedToken({...config,token_env:name},environment);
  try{select(operationConfiguration(config,snapshot[config.token_env]!,async()=>{throw 0},snapshot));}catch{throw new McpConfigurationError();}
  return Object.freeze(snapshot);
}
