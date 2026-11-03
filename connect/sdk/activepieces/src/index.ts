import { createDonorBundle, type DonorBundle } from "./donor-factory.js";
import { ActivepiecesError } from "./errors.js";
import { describe, inputs, record, select, type Description, type ProfileId, type ReaderConfiguration, type WriterConfiguration, type OperationConfiguration } from "./profiles.js";
import { isWriteProfile } from './effect-profiles.js';
import { Invocation } from "./transport.js";
import { contentResult } from "./content.js";
export { ActivepiecesError } from "./errors.js";
export type { FailureKind } from "./errors.js";
export type { ProfileId, HostTransport, ReaderConfiguration, WriterConfiguration, OperationConfiguration, Description } from "./profiles.js";
export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export interface Reader {
  describe(): Description;
  invoke(arguments_: Record<string, unknown>, options?: { readonly signal?: AbortSignal }): Promise<JsonValue>;
}

const ACTIONS: Record<ProfileId, keyof DonorBundle> = {
  "activecampaign.create-contact":"activeCampaignCreateContact", "freshservice.create-ticket":"freshserviceCreateTicket", "gitea.create-issue":"giteaCreateIssue", "datadog.submit-log":"datadogSubmitLog", "gmail.get-thread":"gmailGetThread", "slack.get-thread-replies":"slackGetThreadReplies", "linear.get-issue":"linearGetIssue", "google-cloud-storage.search-objects":"gcsSearchObjects",
  "ghost.get-post":"ghostGetPost", "monday.get-items":"mondayGetItems", "mailchimp.get-campaign-report":"mailchimpGetReport", "amazon-s3.list-files":"s3ListFiles",
  "jira-cloud.get-issue":"jiraGetIssue", "dropbox.get-file-metadata":"dropboxGetFileMetadata", "google-sheets.get-values":"googleSheetsGetValues",
  "microsoft-excel-365.get-table-columns":"excelGetTableColumns", "microsoft-teams.get-channel-message":"teamsGetChannelMessage",
  "quickbooks.find-invoice":"quickbooksFindInvoice", "xero.find-invoice":"xeroFindInvoice", "algolia.browse-records":"algoliaBrowseRecords",
  "freshdesk.get-contact": "freshdeskGetContact", "google-forms.get-form": "googleFormsGetForm", "google-slides.get-presentation": "googleSlidesGetPresentation",
  "onedrive.get-item": "oneDriveGetItem", "google-docs.get-document": "googleDocsGetDocument", "google-drive.get-file": "googleDriveGetFile",
  "contentful.get-entry": "contentfulGetRecord", "outlook.get-message": "outlookGetMessage", "hubspot.get-contact": "hubspotGetContact",
  "supabase.get-user": "supabaseGetUser", "sendgrid.find-list-by-name": "sendgridFindList", "webflow.get-collection-item": "webflowGetItem",
  "airtable.get-record": "airtableGetRecord", "toggl.find-project": "togglFindProject", "stripe.get-invoice": "stripeGetInvoice",
  "pipedrive.get-activity": "pipedriveGetActivity", "wordpress.get-page": "wordpressGetPage", "google-calendar.get-event": "googleCalendarGetEvent",
  "notion.get-page": "notionGetPage", "intercom.get-conversation": "intercomGetConversation",
  "tally.get-form": "tallyGetForm", "asana.get-task": "asanaGetTask", "nocodb.get-record": "nocodbGetRecord",
  "baserow.get-row": "baserowGetRow", "bitly.get-bitlink-details": "bitlyGetBitlinkDetails",
  "clockify.find-running-timer": "clockifyFindRunningTimer", "coda.get-row": "codaGetRow",
  "pagerduty.get-incident": "pagerdutyGetIncident", "postmark.get-delivery-stats": "postmarkGetDeliveryStats",
  "typeform.get-form-messages": "typeformGetFormMessages",
  "google-contacts.search-contact": "googleContactsSearch", "figma.get-comments": "figmaGetComments", "acuity.find-appointments": "acuityFindAppointments",
  "confluence.get-page-content": "confluenceGetPageContent", "brevo.find-contact": "brevoFindContact", "jotform.get-submission": "jotformGetSubmission",
  "google-tasks.get-task": "googleTasksGetTask", "campaign-monitor.find-subscriber": "campaignMonitorFindSubscriber",
  "okta.find-user-by-email": "oktaFindUserByEmail", "help-scout.find-conversation": "helpScoutFindConversation",
  "kit.get-subscriber": "kitGetSubscriber", "mailer-lite.get-campaign": "mailerLiteGetCampaign", "calendly.get-scheduled-event": "calendlyGetScheduledEvent",
};

function result(value: unknown, limit: number): JsonValue {
  let nodes = 0;
  const walk = (item: unknown, depth: number): void => {
    if (++nodes > 100_000 || depth > 64) throw 0;
    if (item === null || typeof item === "boolean" || typeof item === "string") return;
    if (typeof item === "number" && Number.isFinite(item)) return;
    if (Array.isArray(item)) { for (const child of item) walk(child, depth + 1); return; }
    if (record(item)) { for (const child of Object.values(item)) walk(child, depth + 1); return; }
    throw 0;
  };
  walk(value, 0);
  const encoded = JSON.stringify(value);
  if (new TextEncoder().encode(encoded).byteLength > limit) throw 0;
  return JSON.parse(encoded) as JsonValue;
}

/** Bind one selected connection and a borrowed host transport. No setup or refresh is performed. */
export function createReader(configuration: ReaderConfiguration): Reader {
  if (isWriteProfile(configuration?.profile)) throw new ActivepiecesError("invalid_configuration");
  return createOperation(configuration);
}
export interface Writer extends Reader {}
/** Explicitly write-capable local API. The host selects the profile and credential. */
export function createWriter(configuration: WriterConfiguration): Writer {
  if (!isWriteProfile(configuration?.profile)) throw new ActivepiecesError("invalid_configuration", false, false, undefined, "write");
  try { return createOperation(configuration); }
  catch { throw new ActivepiecesError("invalid_configuration", false, false, undefined, "write"); }
}
function createOperation(configuration: OperationConfiguration): Reader {
  const selection = select(configuration);
  return Object.freeze({
    describe: () => describe(selection.profile),
    async invoke(arguments_: Record<string, unknown>, options: { readonly signal?: AbortSignal } = {}): Promise<JsonValue> {
      let prepared: ReturnType<typeof inputs>;
      let signal: AbortSignal | undefined;
      try {
        prepared = inputs(selection, arguments_);
        if (!record(options) || Object.keys(options).some(key => key !== "signal")) throw 0;
        signal = options.signal;
        if (signal !== undefined && !(signal instanceof AbortSignal)) throw 0;
      } catch { throw new ActivepiecesError("invalid_arguments", false, false, undefined, isWriteProfile(selection.profile) ? "write" : "read"); }
      const invocation = new Invocation(selection, prepared.pathname, signal, prepared.query, prepared.inlineQuery, prepared.effect ?? prepared.read);
      try {
        const donor = createDonorBundle(invocation.request,{mailchimpDataCenter:selection.profile==="mailchimp.get-campaign-report"?(selection.auth as {props:{dataCenter:string}}).props.dataCenter:undefined,invalidResponse:()=>{throw invocation.fail("invalid_response")},s3Page:token=>invocation.s3Page(token)});
        const action = donor[ACTIONS[selection.profile]];
        invocation.check();
        invocation.entered = true;
        let value = await action.run({ auth: structuredClone(selection.auth), propsValue: prepared.props });
        invocation.check();
        if (invocation.fault) throw invocation.fault;
        // The unchanged Figma helper adds request_body: undefined for its GET.
        // Preserve its response wrapper while omitting that non-JSON placeholder.
        if (selection.profile === "figma.get-comments") {
          if (!record(value) || value.success !== true || value.request_body !== undefined ||
            Object.keys(value).some(key => !["success", "request_body", "response_body"].includes(key)) || !Object.hasOwn(value, "response_body")) {
            throw invocation.fail("invalid_response");
          }
          value = { success: true, response_body: value.response_body };
        }
        if (selection.profile === "acuity.find-appointments" &&
          (!record(value) || !Array.isArray(value.data) || value.found !== (value.data.length > 0) ||
            value.data.length > (prepared.props.maxResults as number) || !value.data.every(record))) {
          throw invocation.fail("invalid_response");
        }
        if (selection.profile === "brevo.find-contact" &&
          (!record(value) || typeof value.found !== "boolean" || !record(value.data) ||
            Object.keys(value).some(key => !["found", "data"].includes(key)) ||
            (!value.found && Object.keys(value.data).length !== 0))) {
          throw invocation.fail("invalid_response");
        }
        if (["jotform.get-submission", "kit.get-subscriber", "mailer-lite.get-campaign", "calendly.get-scheduled-event"].includes(selection.profile) && !record(value)) throw invocation.fail("invalid_response");
        let detached: JsonValue;
        try { detached = result(contentResult(selection.profile, invocation.projectResult(value)), selection.maxResponseBytes); }
        catch { throw invocation.fail("invalid_response"); }
        invocation.check();
        return detached;
      } catch (error) {
        if (invocation.fault) throw invocation.fault;
        if (error instanceof ActivepiecesError) throw error;
        throw invocation.fail("upstream_error");
      } finally { await invocation.finish(); }
    },
  });
}
