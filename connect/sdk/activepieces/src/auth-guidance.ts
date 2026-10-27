import { isProfile, type ProfileId } from "./profiles.js";

// Same inert vocabulary as toolkit-package::NewcomerPrerequisite. These are
// instructions, not an auth state machine, permission grant or credential API.
export interface NewcomerPrerequisite {
  kind: "account" | "client_authentication" | "administrator" |
    "developer_registration" | "host_deployment" | "artifact_preparation" | "application_binding";
  instruction: string;
}

interface ProfileGuidance {
  credential: string;
  account: string;
  additional?: NewcomerPrerequisite[];
  unresolved?: string[];
}

// Static instructions for the selected donor calls, checked against Google docs
// on 2026-10-03. This helper formats text only; it never chooses or checks scopes.
function googleWorkspace(api: string, operation: string, source: string): Pick<ProfileGuidance, "additional" | "unresolved"> {
  return {
    additional: [
      { kind: "developer_registration", instruction: `Use an existing approved Google OAuth application, or have its operator create a Google Cloud project and enable ${api} in that project. The access token must be issued for the chosen application's client. Project/API setup: https://developers.google.com/workspace/guides/create-project and https://developers.google.com/workspace/guides/enable-apis .` },
      { kind: "developer_registration", instruction: "For an application you operate, configure Google Auth Platform Branding, Audience and Data Access. Internal audience is for users of the project's Workspace/Cloud Identity organization; otherwise choose External. For External Testing, add the intended test users and account for Google's testing/token-lifetime limits before relying on unattended use. Consent setup: https://developers.google.com/workspace/guides/configure-oauth-consent ." },
      { kind: "developer_registration", instruction: "Choose the OAuth client type for the host that actually handles authorization: a web application uses its registered redirect URI; a desktop application can use the supported desktop loopback flow. The host's existing auth owner supplies the matching callback, consent, token exchange and protected storage. Do not use this stdio connector as an OAuth redirect or paste a token into agent input. Client setup: https://developers.google.com/workspace/guides/create-credentials ; desktop flow: https://developers.google.com/identity/protocols/oauth2/native-app ." },
      { kind: "developer_registration", instruction: `${operation} These are static options for this selected read, not scope detection, a permission grant or a restriction on an already-issued token. Source checked 2026-10-03: ${source} .` },
      { kind: "developer_registration", instruction: "The OAuth application operator must check whether its audience, publishing status and requested sensitive/restricted scopes require Google verification. Personal/testing/internal exceptions have conditions and are not a general production approval. Verification exceptions: https://support.google.com/cloud/answer/13464323 ." },
      { kind: "administrator", instruction: "For a managed Google Workspace account, the organization's API controls may block this OAuth client or its requested Google data even after user consent. If restricted, ask the Workspace administrator to review this client and required scopes under its policy; do not assume broad Trusted access is necessary. Personal Google accounts do not have this Workspace administrator step. https://support.google.com/a/answer/7281227 ." },
    ],
    unresolved: [
      "The selected project's enabled API, OAuth client and redirect, audience/publishing or verification status, Workspace policy, actual granted scopes, account identity and access to the requested resource remain unverified. Confirm them with the existing auth owner; this help does not inspect credentials or contact Google.",
      "A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness.",
    ],
  };
}

// Credential meanings follow mcp-config.operationConfiguration and the selected
// auth paths in profiles/read-profiles/expansion-profiles/sdk-profiles/effect-profiles.
// README.md records the supported setup contract. Dated source-backed provider
// instructions are not evidence that a particular account/app is configured.
const GUIDANCE: Record<ProfileId, ProfileGuidance> = {
  "tally.get-form": {
    credential: "Tally API key",
    account: "Select the Tally account and a form that this key can read.",
    additional: [
      {"kind":"developer_registration","instruction":"In the intended Tally account, open Settings > API keys and create a named API key, retaining the one-time value only with the host credential owner. The public API is available on all plans. This route uses a Bearer API key, not an OAuth app registration. Source checked 2026-10-03: https://developers.tally.so/api-reference/api-keys and https://tally.so/help/api ."},
      {"kind":"application_binding","instruction":"Select a form accessible to that account. This operation retrieves its form definition, blocks and settings by ID; it does not retrieve submissions. The documented API-key setup does not establish a separate form-read-only scope, so do not describe the key itself as read-only. Source checked 2026-10-03: https://tally.so/help/api ."}
    ],
    unresolved: ["The key account, access to the chosen form and any workspace restrictions remain unverified; removal of the key owner can invalidate its keys.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "asana.get-task": {
    credential: "Asana access token",
    account: "Select the Asana account and task visible to that token.",
    additional: [
      {"kind":"developer_registration","instruction":"For your own Asana account, create a personal access token in the Asana developer console; for an application serving other users, use an approved OAuth application with its registered redirect URI. The host's auth owner handles consent and token storage. A PAT acts with its owner's access and is not narrowed by this read-only tool. Sources checked 2026-10-03: https://developers.asana.com/docs/personal-access-token and https://developers.asana.com/docs/oauth ."},
      {"kind":"developer_registration","instruction":"The selected GET /tasks/{task_gid} needs tasks:read for a scoped OAuth app. Its fixed opt_fields also requests memberships.project and memberships.section, which Asana documents as needing projects:read and project_sections:read. Do not copy unrelated write or time-tracking scopes from a wider integration. Source checked 2026-10-03: https://developers.asana.com/reference/gettask ."},
      {"kind":"administrator","instruction":"The account must already be allowed to see this task and its related project data. Organization app/PAT restrictions can require an Asana administrator's review; knowing a task ID does not grant access. Source checked 2026-10-03: https://developers.asana.com/docs/personal-access-token ."}
    ],
    unresolved: ["The exact permission/scope grant, task/project visibility, chosen PAT versus OAuth application and organization restrictions remain unverified; the host must confirm them for this account.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "nocodb.get-record": {
    credential: "NocoDB API token",
    account: "Select the canonical HTTPS origin and version (3 or 4), then an accessible base, table and record.",
    additional: [
      {"kind":"developer_registration","instruction":"Open Account Settings > API Tokens and create a named token. Where fine-grained tokens are supported, select the intended base as its resource and Records: Read; remove default record-write permissions that this GET does not need. Effective access is the intersection of token permissions and the user role. Source checked 2026-10-03: https://nocodb.com/docs/product/account-settings/api-tokens ."},
      {"kind":"administrator","instruction":"Fine-grained resource/permission controls depend on edition: the current docs support them on NocoDB Cloud and licensed self-hosted Business or above; Community/unlicensed tokens have broader access. For an SSO-enforced workspace, use a token created after the required SSO login. Ask the workspace operator for an appropriately authorized identity if these controls are unavailable; do not claim a broad token is read-only. Source checked 2026-10-03: https://nocodb.com/docs/product/account-settings/api-tokens ."},
      {"kind":"host_deployment","instruction":"Match the configured origin and adapter version to the actual deployment. This adapter uses version 3 for the /api/v2/tables/... record route and version 4 for /api/v3/data/{baseId}/{tableId}/records/...; these selector numbers are not API version numbers. Supply the base/table/record IDs for that deployment. API-token guidance checked 2026-10-03: https://nocodb.com/docs/product/account-settings/api-tokens ."}
    ],
    unresolved: ["The deployment version/edition, SSO history, user role and selected token/base permissions remain unverified; this help does not probe the NocoDB instance.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "baserow.get-row": {
    credential: "Baserow database token; email/password login and JWT acquisition are unsupported",
    account: "Select the canonical HTTPS origin and a table/row readable by that database token.",
    additional: [
      {"kind":"developer_registration","instruction":"In the chosen workspace, open Settings > Database tokens and create a named database token. Limit it to the required table with Read enabled and Create, Update and Delete disabled. This direct row read needs a database token, not a user password or JWT login. Source checked 2026-10-03: https://baserow.io/user-docs/personal-api-tokens and https://baserow.io/user-docs/database-api ."},
      {"kind":"administrator","instruction":"On Baserow Enterprise, token creation is limited to workspace Admins and Builders. If your role cannot create the required token, have an authorized workspace operator provision it through the host credential owner. Supply the correct table ID and row ID at the configured Baserow origin. Source checked 2026-10-03: https://baserow.io/user-docs/personal-api-tokens ."}
    ],
    unresolved: ["The token workspace/table permissions, instance policy and existence of the requested row remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "bitly.get-bitlink-details": {
    credential: "Bitly access token",
    account: "Select the Bitly account and a Bitlink accessible to that token.",
    additional: [
      {"kind":"developer_registration","instruction":"For an integration with your own Bitly account, generate a generic access token using Bitly's access-token page. For other users, use a registered Bitly OAuth application with an approved callback and let the host's auth owner perform authorization. Supply the resulting access token, not a client secret or account password. Source checked 2026-10-03: https://dev.bitly.com/docs/getting-started/authentication/ ."},
      {"kind":"developer_registration","instruction":"This profile only retrieves GET /v4/bitlinks/{bitlink}; it does not create or edit links. The cited API/auth documentation does not establish a separate read-only scope for this token, so do not describe the generic token as restricted to this read. Sources checked 2026-10-03: https://dev.bitly.com/api-reference/ and https://dev.bitly.com/docs/getting-started/authentication/ ."},
      {"kind":"application_binding","instruction":"Choose a Bitlink, including its domain and hash, visible to the connected Bitly user/group. Account access and API plan limits still apply; this connector does not enroll the user in a group or discover the correct account."}
    ],
    unresolved: ["The selected Bitly user's group access, API entitlement and actual token authority remain unverified; the cited documentation does not establish an exact permission/scope reduction to this single endpoint.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "clockify.find-running-timer": {
    credential: "Clockify API key",
    account: "Select the Clockify account and workspace whose running timer may be read.",
    additional: [
      {"kind":"developer_registration","instruction":"Generate an API key in the selected Clockify user profile settings and retain it with the host credential owner. The API follows that user's workspace permissions; this route does not request an OAuth scope or establish a read-only key. It reads the authenticated user's in-progress timer for the supplied workspace ID. Source checked 2026-10-03: https://docs.clockify.me/ and https://clockify.me/help/getting-started/clockify-api-overview ."},
      {"kind":"host_deployment","instruction":"This selected adapter uses the global https://api.clockify.me origin. Clockify workspaces with a dedicated subdomain can require a key generated for that workspace and a different API base; do not assume such a deployment works through this fixed origin. Plan-specific API quotas also apply. Source checked 2026-10-03: https://docs.clockify.me/ and https://clockify.me/help/getting-started/clockify-api-overview ."}
    ],
    unresolved: ["The key user, workspace membership, matching global API deployment, applicable quota and presence of a running timer remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "coda.get-row": {
    credential: "Coda API key",
    account: "Select the Coda account and an accessible document, table and row.",
    additional: [
      {"kind":"developer_registration","instruction":"Generate a REST API token from the intended Coda account's API settings at https://coda.io/account and keep its value with the host credential owner. Give that identity read access to the intended document and ensure any token restrictions include it. This direct REST request does not use a Coda MCP-only token or require OAuth application registration. Source checked 2026-10-03: https://coda.io/developers/apis/v1 and https://help.coda.io/hc/en-us/articles/39555941038349-Does-Coda-have-an-API ."},
      {"kind":"application_binding","instruction":"Supply the document, table and row IDs for the selected GET row call. It asks for column names and simpleWithArrays values; no write operation is selected. Coda document/page locking is a UI feature and is not a server-side restriction on API writes by an otherwise authorized identity, so locking does not turn a broad token into a read-only token. Source checked 2026-10-03: https://coda.io/developers/apis/v1 and https://help.coda.io/hc/en-us/articles/39555860595469-Add-locking-to-the-pages-of-your-doc ."}
    ],
    unresolved: ["The account's document access, actual token restrictions and workspace policy remain unverified. The reviewed public docs establish restricted tokens but do not establish the current creation UI's exact restriction choices; do not invent a scope string.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "pagerduty.get-incident": {
    credential: "PagerDuty API key",
    account: "Select the PagerDuty account and an incident this key may read.",
    additional: [
      {"kind":"developer_registration","instruction":"Have an authorized PagerDuty administrator use Integrations > API Access Keys to create a REST API key with Read-only API Key selected, or use an approved existing read-only key. This selected GET /incidents/{id} uses the REST Token token= header, not an Events API routing key or OAuth Bearer token. Source checked 2026-10-03: https://support.pagerduty.com/main/docs/api-access-keys ."},
      {"kind":"application_binding","instruction":"Select an incident in the intended PagerDuty account. General-access keys have account-wide access, while user tokens follow the creating user's permissions. Read-only limits writes but does not isolate one incident. OAuth incidents.read is a different auth route and is not a scope to put on this API key. Sources checked 2026-10-03: https://docs.pagerduty.com/developer/authentication and https://docs.pagerduty.com/developer/api/reference/rest/incidents/get-incident ."}
    ],
    unresolved: ["The key type, account, user restrictions, API entitlement and requested incident access remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "postmark.get-delivery-stats": {
    credential: "Postmark server token",
    account: "Select the Postmark server whose delivery statistics may be read.",
    additional: [
      {"kind":"developer_registration","instruction":"Select the Postmark server and obtain its Server API token from the server's API Tokens tab through the host credential owner. GET /deliverystats requires X-Postmark-Server-Token, not an Account API token. Account Owners, Account Admins and Server Admins can access server tokens. Source checked 2026-10-03: https://www.postmarkapp.com/developer/api/bounce-api and https://www.postmarkapp.com/developer/api/overview ."},
      {"kind":"application_binding","instruction":"This operation reads delivery statistics for that server; it does not send mail or require configuring a sending domain to perform a send. A Server API token can authorize other server operations, so the selected GET does not make the credential read-only. Source checked 2026-10-03: https://www.postmarkapp.com/developer/api/overview ."}
    ],
    unresolved: ["The selected server, token validity and operator permission to obtain it remain unverified; no email delivery or sender verification is performed.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "typeform.get-form-messages": {
    credential: "Typeform access token",
    account: "Select the Typeform account and a form whose messages are accessible.",
    additional: [
      {"kind":"developer_registration","instruction":"For your own Typeform account, generate a personal access token under its account settings; for other users, use a registered Typeform OAuth application and its callback. The host retains any client secret, access token and refresh token. Sources checked 2026-10-03: https://www.typeform.com/developers/get-started/personal-access-token/ and https://www.typeform.com/developers/get-started/applications/ ."},
      {"kind":"developer_registration","instruction":"For scoped authorization, forms:read covers GET /forms/{form_id}/messages. These are the form's customizable interface messages, not submitted response answers, so responses:read and form write scopes are not needed for this selected call. Source checked 2026-10-03: https://www.typeform.com/developers/get-started/scopes/ ."},
      {"kind":"application_binding","instruction":"Select a form accessible to the connected account. A pasted public form URL or form ID is not proof that this credential can read the form's settings. Refresh-token acquisition, if the host needs it, is separate from this connector's one read."}
    ],
    unresolved: ["The exact permission/scope grant, selected form ownership or workspace access, token expiry and OAuth client/callback remain unverified; no Typeform account or form is contacted by this help.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "google-contacts.search-contact": { credential: "Google OAuth access token", account: "Select the Google account whose contacts may be searched.",
    ...googleWorkspace("People API (people.googleapis.com)", "For people.searchContacts, request https://www.googleapis.com/auth/contacts.readonly to search the signed-in user's contacts. This is not a Workspace Directory search; readMask limits returned fields, not the token's permission. The selected action makes one search request and does not automate Google's recommended empty-query cache warmup.", "https://developers.google.com/people/api/rest/v1/people/searchContacts") },
  "figma.get-comments": {
    credential: "Figma OAuth access token",
    account: "Select the Figma account and file whose comments may be read.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an approved Figma OAuth application, or have its operator register one with the intended redirect URI and scopes. This profile sends an OAuth access token as Bearer; a Figma personal access token using X-Figma-Token is not a drop-in replacement for this selected auth path. Source checked 2026-10-03: https://developers.figma.com/docs/rest-api/oauth-apps/ ."},
      {"kind":"developer_registration","instruction":"Request file_comments:read for GET /v1/files/{file_key}/comments. Do not copy comment-write or broader file scopes from the donor app; file_read is a deprecated OAuth scope. Sources checked 2026-10-03: https://developers.figma.com/docs/rest-api/comments-endpoints/ and https://developers.figma.com/docs/rest-api/scopes/ ."},
      {"kind":"application_binding","instruction":"The authorizing user must have access to the selected Figma file. Organization app restrictions, account/seat eligibility and API rate limits still apply; granting the scope does not share a private file with the user."}
    ],
    unresolved: ["The exact permission/scope grant, OAuth app approval and callback, user/file access and applicable account limits remain unverified. The host's auth owner must resolve them without exposing credentials to the agent.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "acuity.find-appointments": {
    credential: "Acuity Scheduling OAuth access token",
    account: "Select the Acuity scheduling account whose appointments may be read.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an Acuity OAuth2 client registered for the host's redirect URI and authorize the scheduling account with scope api-v1. Have the existing auth owner handle the code exchange and protected token storage. Source checked 2026-10-03: https://developers.acuityscheduling.com/docs/oauth2 ."},
      {"kind":"developer_registration","instruction":"The selected call is GET /appointments with bounded filters. Acuity's documented api-v1 OAuth scope is broader than reading appointments; it is not an appointment-only or read-only grant. The separate user-ID/API-key Basic-auth quick start is not the OAuth credential accepted by this profile. Sources checked 2026-10-03: https://developers.acuityscheduling.com/docs/oauth2 and https://developers.acuityscheduling.com/reference/quick-start ."},
      {"kind":"application_binding","instruction":"Choose the authorized scheduling account and any calendar/appointment-type filters within it. Confirm that this account's subscription and user permissions permit API access before relying on the connection; this help does not check plan eligibility."}
    ],
    unresolved: ["The exact permission/scope grant, scheduling account, OAuth app/callback and account API eligibility remain unverified; api-v1 does not independently limit the token to this selected read.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "confluence.get-page-content": {
    credential: "Confluence Cloud API token paired with the configured username/email for Basic authentication",
    account: "Select the canonical HTTPS site origin, username and page that this account may read.",
    additional: [
      {"kind":"developer_registration","instruction":"This selected Confluence Cloud route uses an Atlassian account email and an unscoped API token created at https://id.atlassian.com/manage-profile/security/api-tokens . Choose a name and expiry through the authorized account owner; keep the token with the host secret owner. Scoped tokens use api.atlassian.com/ex/.../{cloudId}, which this direct-site profile does not implement. Do not paste that gateway path into origin or silently substitute a scoped token. Source checked 2026-10-03: https://support.atlassian.com/atlassian-account/docs/manage-api-tokens-for-your-atlassian-account ."},
      {"kind":"application_binding","instruction":"The selected call is GET /wiki/api/v2/pages/{id}?body-format=storage; the account still needs permission to view that resource. An unscoped token follows the user's permissions and is not made read-only by this one GET. The host must account for expiry and organization restrictions. Source checked 2026-10-03: https://developer.atlassian.com/cloud/confluence/basic-auth-for-rest-apis/ ."},
      {"kind":"developer_registration","instruction":"Atlassian documents Basic authentication for personal scripts/manual calls and directs distributed integrations toward approved app authentication. Its current guidance disallows apps that collect customers' API tokens or ask customers to create individual OAuth apps. Treat this profile as a personal direct-site compatibility route, not a supported customer-onboarding design; a distributed integration needs a separately implemented appropriate app route. Source checked 2026-10-03: https://developer.atlassian.com/cloud/confluence/basic-auth-for-rest-apis/ ."}
    ],
    unresolved: ["The authorized personal-use context, account/site permissions, token expiry and policy remain unverified. Scoped gateway, service-account scoped tokens and OAuth app routes are not implemented by this profile.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "brevo.find-contact": {
    credential: "Brevo API key",
    account: "Select the Brevo account with permission to read the requested contact.",
    additional: [
      {"kind":"developer_registration","instruction":"In Brevo, open Settings > SMTP & API > API Keys & MCP and generate a named ordinary API key. The account owner or a user with API Keys permission must do this. Keep it with the host credential owner. Do not choose Create MCP server API key: Brevo converts/deactivates the ordinary key for that different use, while this route requires the REST api-key header. Source checked 2026-10-03: https://help.brevo.com/hc/en-us/articles/209467485-Create-and-manage-your-API-keys ."},
      {"kind":"administrator","instruction":"Use an account permitted to read the contact. Brevo's contacts:read OAuth scope belongs to its OAuth flow and is not a permission setting on this selected API-key route. If account IP security blocks the host, have the account owner review the host's actual egress IP through Brevo's authorized-IP controls; do not disable the policy indiscriminately. Source checked 2026-10-03: https://developers.brevo.com/docs/how-it-works and https://help.brevo.com/hc/en-us/articles/209467485-Create-and-manage-your-API-keys ."}
    ],
    unresolved: ["The account permissions, key expiry, authorized egress IP and requested contact/identifier type remain unverified; this help does not grant OAuth scopes or contact Brevo.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "jotform.get-submission": {
    credential: "Jotform API key",
    account: "Select the submission and the account's explicit region: us, eu or hipaa. A region label does not establish compliance.",
    additional: [
      {"kind":"developer_registration","instruction":"Open the intended Jotform account's API settings, create a named API key and select Read Access rather than Full Access. The selected submission GET is a read and does not need write access or OAuth application registration. Retain the key only with the host credential owner. Source checked 2026-10-03: https://www.jotform.com/help/253-how-to-create-a-jotform-api-key/ and https://api.jotform.com/docs/ ."},
      {"kind":"application_binding","instruction":"Select the account's actual US, EU or HIPAA API region and a submission ID accessible to that account. The adapter sends the APIKEY header to the matching fixed regional endpoint; choosing hipaa is routing, not a compliance assessment or enrollment. Source checked 2026-10-03: https://api.jotform.com/docs/ ."}
    ],
    unresolved: ["The key read permission, account region/entitlement and access to the submission remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "kit.get-subscriber": {
    credential: "Kit/ConvertKit V3 API secret, not a V4 API key",
    account: "Select the Kit account whose subscriber may be read.",
    additional: [
      { kind: "host_deployment", instruction: "The V3 protocol puts api_secret in the provider request query. Treat that credential-bearing URL as secret in host transport logs and diagnostics." },
      {"kind":"developer_registration","instruction":"In Kit Settings > Developer, use the V3 credentials section and have the host credential owner retain the V3 API secret. GET /v3/subscribers/{subscriber_id} requires api_secret, not the public V3 API key and not a V4 API key. Kit describes V3 as legacy and no longer actively developed, although still available; switching this connector to V4 would require a different implementation. Sources checked 2026-10-03: https://help.kit.com/en/articles/9902901-kit-api-overview and https://developers.kit.com/api-reference/v3/subscribers ."},
      {"kind":"application_binding","instruction":"Choose a subscriber in that Kit account. This route only reads the subscriber, but the V3 secret also authorizes modifying subscriber endpoints and is not a read-only credential. Do not substitute a V4 OAuth/app setup for this pinned V3 route. Source checked 2026-10-03: https://developers.kit.com/api-reference/v3/subscribers ."}
    ],
    unresolved: ["V3 credential availability for the selected account, secret validity and subscriber access remain unverified. The existing secret-bearing query warning still applies.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "mailer-lite.get-campaign": {
    credential: "MailerLite API key (sent as a Bearer token)",
    account: "Select the MailerLite account whose campaign may be read.",
    additional: [
      {"kind":"developer_registration","instruction":"Use the current MailerLite account's Integrations > MailerLite API > Generate new token flow, name the token and retain it with the host credential owner. This route uses a Bearer token at connect.mailerlite.com/api; MailerLite Classic credentials/API are a different product route. No OAuth client registration is performed here. Source checked 2026-10-03: https://developers.mailerlite.com/getting-started ."},
      {"kind":"application_binding","instruction":"Keep the token's creating user active: MailerLite ties the token to that user and removing them invalidates it. Select a campaign ID in the intended account. The selected GET does not establish a separately scoped read-only token. Source checked 2026-10-03: https://developers.mailerlite.com/getting-started ."}
    ],
    unresolved: ["The account generation, creator's current access, token validity and campaign existence remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "calendly.get-scheduled-event": {
    credential: "Calendly personal token",
    account: "Select the Calendly account with permission to read the scheduled event.",
    additional: [
      {"kind":"developer_registration","instruction":"Generate a Calendly personal access token for the intended account using its API integration settings. This profile specifically uses a personal token; it does not register an OAuth application or perform an OAuth exchange. Sources checked 2026-10-03: https://developer.calendly.com/docs/authentication/scopes and https://developer.calendly.com/docs/getting-started/how-to-migrate-from-api-v1-to-api-v2 ."},
      {"kind":"developer_registration","instruction":"Choose scheduled_events:read for GET /scheduled_events/{uuid}. Current Calendly docs require explicit scopes on newly generated personal tokens; older broadly authorized tokens do not prove that a new token is correctly scoped. Invitee, webhook and scheduling-write scopes are not needed for this one call. Sources checked 2026-10-03: https://developer.calendly.com/docs/authentication/scopes and https://developer.calendly.com/api-docs/calendly-api/scheduled-events/get-scheduled-event ."},
      {"kind":"administrator","instruction":"The token owner's role limits which events can be read. Organization-wide visibility can require an organization owner/admin; a normal user's token is not automatically organization-wide. Source checked 2026-10-03: https://developer.calendly.com/docs/api-guides/track-and-report-on-all-scheduled-events-across-your-organization ."}
    ],
    unresolved: ["The exact permission/scope grant, token owner's event visibility, organization role and token expiry/revocation remain unverified; supplying an event UUID does not establish access.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "google-tasks.get-task": { credential: "Google OAuth access token", account: "Select the Google account and an accessible task list and task.",
    ...googleWorkspace("Google Tasks API (tasks.googleapis.com)", "For tasks.get, request https://www.googleapis.com/auth/tasks.readonly . Supply the task list ID and task ID explicitly; this selected read does not create tasks or discover lists.", "https://developers.google.com/workspace/tasks/reference/rest/v1/tasks/get") },
  "campaign-monitor.find-subscriber": {
    credential: "Campaign Monitor API key",
    account: "Select the Campaign Monitor account and list whose subscriber may be read.",
    additional: [
      {"kind":"developer_registration","instruction":"An operator with Full access and a verified email address can open Account settings > API keys to obtain or generate the account API key. Deliver it through the host credential owner. This selected API-key route uses HTTP Basic authentication with the key as username and x as the unused password; it does not use OAuth scopes. Source checked 2026-10-03: https://help.campaignmonitor.com/articles/Knowledge/api-keys and https://www.campaignmonitor.com/api/v3-3/getting-started/ ."},
      {"kind":"application_binding","instruction":"Supply the intended list ID and subscriber email. The selected v3.3 subscriber-details GET reads that list membership and does not subscribe, resubscribe or send a campaign. The account API key is broader than this read operation; no list-read-only key is established by the reviewed setup docs. Source checked 2026-10-03: https://www.campaignmonitor.com/api/v3-3/subscribers/ ."}
    ],
    unresolved: ["The authorized account/client/list, operator's access and verified email, key validity and subscriber membership remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "okta.find-user-by-email": {
    credential: "Okta API token",
    account: "Select the canonical HTTPS organization origin and a token permitted to look up the requested user.",
    additional: [
      {"kind":"developer_registration","instruction":"Have an appropriately privileged Okta administrator create an API token in Admin Console > Security > API > Tokens. This profile sends SSWS authentication; it does not implement an OAuth service app or exchange okta.users.read scopes for a token. The token inherits its creator's current privileges. Source checked 2026-10-03: https://developer.okta.com/docs/guides/create-an-api-token/main/ ."},
      {"kind":"administrator","instruction":"Use the least privileged approved administrator identity able to search users, and bind the configured HTTPS Okta organization. Check token network-zone restrictions against the host egress. Tokens expire after 30 days without use and stop working when their owning user is deactivated. The selected call searches /api/v1/users by profile.email; it does not provision users. Source checked 2026-10-03: https://developer.okta.com/docs/guides/create-an-api-token/main/ ."}
    ],
    unresolved: ["The administrator privileges, token network zones/lifetime, organization and requested user's visibility remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "help-scout.find-conversation": {
    credential: "Help Scout access token",
    account: "Select the Help Scout account and conversation filters within that token's access.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an existing approved Help Scout OAuth application, or create one in your profile's My apps area. Use authorization-code OAuth for another user's account; Help Scout also documents client credentials for internal use. In either case token_env contains the resulting access token, not the app ID or secret. Source checked 2026-10-03: https://developer.helpscout.com/mailbox-api/overview/authentication/ ."},
      {"kind":"developer_registration","instruction":"This profile lists GET /v2/conversations with its supplied filters and returns a match from that page. The cited Help Scout v2 authentication guide does not establish a granular conversation-read scope; do not label an access token as read-only just because this profile is. It does not fetch every thread or page. Sources checked 2026-10-03: https://developer.helpscout.com/mailbox-api/overview/authentication/ and https://developer.helpscout.com/mailbox-api/endpoints/conversations/list/ ."},
      {"kind":"administrator","instruction":"Help Scout associates these credentials with a user: that user must remain active/invited and have the necessary mailbox access. Review account/API eligibility and mailbox restrictions with the account administrator when needed; a client-credentials token is not a guarantee of unrestricted mailbox access. Source checked 2026-10-03: https://developer.helpscout.com/mailbox-api/overview/authentication/ ."}
    ],
    unresolved: ["The connected user's active status, mailbox access and account API eligibility remain unverified; an exact permission/scope reduction to this endpoint is not established by the cited v2 guide. The host owns access-token renewal.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "airtable.get-record": {
    credential: "Airtable access token through the selected secret-text auth path",
    account: "Select the Airtable account and base, table and record readable by that token.",
    additional: [
      {"kind":"developer_registration","instruction":"Create a Personal Access Token in Airtable's developer hub. Select data.records:read and add only the required base as a resource. The token's access is also limited by its creator's permissions. A legacy account API key is not a substitute. Retain the PAT with the host credential owner. Source checked 2026-10-03: https://support.airtable.com/articles/9934989703-creating-personal-access-tokens and https://support.airtable.com/articles/1128328104-visualizing-airtable-records-in-microsoft-power-bi-power-query ."},
      {"kind":"application_binding","instruction":"Supply the exact app..., tbl... and rec... IDs required by this selected route. It performs a direct record GET, not base/schema discovery, so schema.bases:read and record-write permission are not needed for the selected call. If the creator lacks base access or organization policy blocks tokens, an authorized administrator must address that restriction; adding a PAT scope does not grant missing base membership. Source checked 2026-10-03: https://support.airtable.com/articles/9934989703-creating-personal-access-tokens ."}
    ],
    unresolved: ["The actual PAT scope/resource selection, creator's base permissions, organization restrictions and record identity remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "toggl.find-project": {
    credential: "Toggl Track API token used as the Basic username with the literal password api_token",
    account: "Select the Toggl account and workspace whose project page may be read.",
    additional: [
      {"kind":"developer_registration","instruction":"Use the selected Toggl Track user's Profile settings API token and retain it with the host credential owner. This Track v9 route authenticates with the token as the Basic username and the literal password api_token; a Toggl Focus/2.0 Bearer token is not interchangeable. Source checked 2026-10-03: https://support.toggl.com/en-us/article/where-is-my-api-key-located-e9ki6p/ and https://engineering.toggl.com/docs/authentication/index.html ."},
      {"kind":"application_binding","instruction":"Supply the workspace ID and any intended project filters. This operation reads one bounded project-list page, not every project in the account. Workspace/project membership still governs visibility, including private projects; no per-operation read-only API-token scope is established here. Source checked 2026-10-03: https://engineering.toggl.com/docs/projects/index.html and https://support.toggl.com/en-us/article/access-rights-and-privileges-108y786/ ."}
    ],
    unresolved: ["The Track account/token, workspace and private-project visibility, applicable API limits and completeness beyond the requested page remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "stripe.get-invoice": {
    credential: "Stripe API key",
    account: "Select the Stripe account/key context containing the invoice to read; this operation does not pay or send it.",
    additional: [
      {"kind":"developer_registration","instruction":"In the intended Stripe account and sandbox/live mode, create a named restricted API key from the Dashboard API keys page. Start with no permissions and enable Invoices Read for this invoice retrieval; do not use a publishable key or assume an unrestricted secret key is required. Retain the key with the host credential owner. Source checked 2026-10-03: https://docs.stripe.com/keys/restricted-api-keys ."},
      {"kind":"application_binding","instruction":"Supply an invoice ID from that key's account and mode. The selected GET /v1/invoices/{id} does not pay, finalize or send an invoice. This profile supplies no Stripe-Account or Stripe-Context override, so connected-account and organization-key routing must not be inferred from an invoice ID. Sources checked 2026-10-03: https://docs.stripe.com/keys and https://docs.stripe.com/api/invoices/retrieve ."}
    ],
    unresolved: ["The actual restricted-key permissions, account/mode, network restrictions and invoice access remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "pipedrive.get-activity": {
    credential: "Pipedrive OAuth access token",
    account: "Select the connection's canonical HTTPS API origin and an accessible activity.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an approved Pipedrive OAuth app, or register a private app in Developer Hub with its callback and required scopes. Let the host obtain and renew the access token. Configure the connection's canonical API origin for the authorized Pipedrive company; do not substitute an API token for this OAuth path. Sources checked 2026-10-03: https://pipedrive.readme.io/docs/marketplace-registering-a-private-app and https://pipedrive.readme.io/docs/marketplace-oauth-api ."},
      {"kind":"developer_registration","instruction":"Request activities:read for the selected GET /api/v2/activities/{id}, including its attendee field. This scope is broader than a single activity but does not require activities:full or admin access merely to perform this read. Source checked 2026-10-03: https://pipedrive.readme.io/docs/marketplace-scopes-and-permissions-explanations ."},
      {"kind":"application_binding","instruction":"The authorizing user's company and activity visibility still apply. An activity ID from another company and an arbitrary HTTPS origin are not a valid account binding; the host must retain the OAuth connection's company/origin association."}
    ],
    unresolved: ["The exact permission/scope grant, authorized company/API origin, app/callback, user visibility and any organization app-install restrictions remain unverified.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "wordpress.get-page": {
    credential: "WordPress application password paired with the configured username for Basic authentication; it is not an OAuth token",
    account: "Select the canonical HTTPS origin and username allowed to read the page. Subdirectory installations are unsupported.",
    additional: [
      {"kind":"developer_registration","instruction":"On the intended HTTPS WordPress site, create a named Application Password from the selected user's profile/Edit User page (WordPress 5.6 or later). Pair it with that username through the host credential owner; do not use the main login password or an OAuth token. Application Passwords may be disabled by site policy, in which case the site administrator must enable an approved method before this route can work. Source checked 2026-10-03: https://developer.wordpress.org/rest-api/using-the-rest-api/authentication/ and https://developer.wordpress.org/advanced-administration/security/application-passwords/ ."},
      {"kind":"application_binding","instruction":"The password inherits the user's capabilities; it has no independent page-read-only scope. Public published pages can ordinarily be read without authentication, while private/draft access needs appropriate user capabilities. This adapter nevertheless requires its configured credentials and makes a default-view GET /wp-json/wp/v2/pages/{id}; it does not request edit context or support a subdirectory site origin. Source checked 2026-10-03: https://developer.wordpress.org/rest-api/reference/pages/ ."}
    ],
    unresolved: ["The site's REST/Application Password policy, selected user capabilities, canonical root origin and access to the requested page remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "google-calendar.get-event": { credential: "Google OAuth access token; service-account JWT signing is not performed", account: "Select the Google account and an accessible calendar/event; choosing primary does not verify account identity.",
    ...googleWorkspace("Google Calendar API (calendar-json.googleapis.com)", "For events.get, https://www.googleapis.com/auth/calendar.events.readonly is a read-only scope option for events on calendars accessible to the selected account. The account still needs access to the selected calendar/event; a calendar ID or primary alias does not grant it. Broader calendar write permission is not needed for this read.", "https://developers.google.com/workspace/calendar/api/v3/reference/events/get") },
  "notion.get-page": {
    credential: "Notion internal integration token through the selected custom-auth path",
    account: "Select the integration and a page visible to it; this operation reads page metadata/properties, not body blocks.",
    additional: [
      {"kind":"developer_registration","instruction":"Have a workspace owner create an Internal connection for the intended Notion workspace in the developer portal. Enable Read content and retain its internal integration token with the host credential owner. This selected custom-auth path does not perform public OAuth or accept a different connection type by assumption. Source checked 2026-10-03: https://developers.notion.com/guides/get-started/authorization and https://developers.notion.com/reference/retrieve-a-page ."},
      {"kind":"application_binding","instruction":"Share the target page with that connection using the page menu > Add connections (and ensure referenced content is shared when needed). Read content capability alone does not grant page access. The selected call uses Notion-Version 2022-02-22 and retrieves page metadata/properties, not body blocks or all paginated property values; do not upgrade its version header through configuration. Source checked 2026-10-03: https://developers.notion.com/guides/get-started/authorization and https://developers.notion.com/reference/retrieve-a-page ."}
    ],
    unresolved: ["The workspace owner's setup, Read content capability, page sharing and compatibility of this pinned API version with the live workspace remain unverified; a missing page can also mean it was not shared.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "intercom.get-conversation": {
    credential: "Intercom access token",
    account: "Select the Intercom account, an accessible conversation and the matching region: us, eu or au.",
    additional: [
      {"kind":"developer_registration","instruction":"For your own Intercom workspace, create/configure a private app in Developer Hub and obtain its access token under Authentication. For other customers' workspaces, use an OAuth app and its redirect flow; do not ask customers to hand over private workspace tokens. Source checked 2026-10-03: https://developers.intercom.com/docs/build-an-integration/learn-more/authentication ."},
      {"kind":"developer_registration","instruction":"Enable Read conversations for this GET /conversations/{id}. Do not add Write conversations merely to read. Intercom documents a separate Read tickets permission when tickets are accessed through the Conversations API; confirm that requirement if the requested object is a ticket rather than an ordinary conversation. Sources checked 2026-10-03: https://developers.intercom.com/docs/build-an-integration/learn-more/authentication/setting-up-oauth and https://developers.intercom.com/docs/references/preview/changelog ."},
      {"kind":"application_binding","instruction":"Choose the matching us, eu or au workspace region. The selected SDK request pins Intercom-Version 2.11; this help does not upgrade that protocol or establish that every feature described by newer Intercom docs exists in it. Workspace visibility and app permissions remain the authority for access."}
    ],
    unresolved: ["The exact permission/scope grant, conversation-versus-ticket access, workspace region, app approval and continued compatibility of API version 2.11 remain unverified; the cited current permission guidance is not a live compatibility test.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "freshdesk.get-contact": {
    credential: "Freshdesk API key used as Basic apiKey:X",
    account: "Select the canonical HTTPS Freshdesk origin and an accessible contact.",
    additional: [
      {"kind":"developer_registration","instruction":"In the intended Freshdesk agent account, open Profile settings from the profile picture to obtain the personal API key, then retain it with the host credential owner. The selected v2 API uses Basic authentication with the key as username and X as password, not the normal account password or an OAuth app registration. Source checked 2026-10-03: https://developer.freshdesk.com/api/ ."},
      {"kind":"administrator","instruction":"Use an agent whose role permits viewing the selected contact; Freshdesk API access follows that user's permissions. Ask the account administrator for an appropriately authorized identity if restricted, rather than assuming every contact read needs an administrator key. Configure the actual Freshdesk API origin and contact ID. Source checked 2026-10-03: https://developer.freshdesk.com/api/ ."}
    ],
    unresolved: ["The agent identity/role, account API availability, canonical origin, key validity and contact access remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "google-forms.get-form": { credential: "Google OAuth access token", account: "Select the Google account and an accessible form.",
    ...googleWorkspace("Google Forms API (forms.googleapis.com)", "For forms.get, request https://www.googleapis.com/auth/forms.body.readonly to read form structure. This call does not read submitted responses or require forms.responses.readonly . The selected account still needs access to the form.", "https://developers.google.com/workspace/forms/api/reference/rest/v1/forms/get") },
  "google-slides.get-presentation": { credential: "Google OAuth access token", account: "Select the Google account and an accessible presentation.",
    ...googleWorkspace("Google Slides API (slides.googleapis.com)", "For presentations.get, request https://www.googleapis.com/auth/presentations.readonly . It permits reading presentations accessible to this account; the selected presentation ID and optional response field mask do not narrow the token grant to one file.", "https://developers.google.com/workspace/slides/api/reference/rest/v1/presentations/get") },
  "onedrive.get-item": {
    credential: "Microsoft Graph OAuth access token",
    account: "Select the Microsoft account and matching cloud (commercial or government), then an accessible drive item.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an approved Microsoft Entra app, or register one for the host's account types and redirect URI. The host owns authorization, client secrets and token renewal; supply the Graph access token, not client credentials. Checked 2026-10-03: https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app ."},
      {"kind":"developer_registration","instruction":"Use delegated Files.Read for the selected drive-item metadata read. Every supported item path is under /me/drive, so this profile requires a signed-in user's delegated token; the generic API's application-permission alternatives do not make app-only tokens work here. The request does not download file contents. Source checked 2026-10-03: https://learn.microsoft.com/en-us/graph/api/driveitem-get?view=graph-rest-1.0 ."},
      {"kind":"application_binding","instruction":"Match the token's cloud to this profile: commercial uses graph.microsoft.com (including ordinary GCC); government uses graph.microsoft.us (GCC High). DoD's dod-graph.microsoft.us and China endpoints are not represented by these two choices. Source checked 2026-10-03: https://learn.microsoft.com/en-us/graph/deployments ."},
      {"kind":"administrator","instruction":"Tenant consent policy and resource access still apply. An administrator may need to approve delegated access; application permissions require administrator consent. Do not add broad write permissions merely to bypass denied access. Checked 2026-10-03: https://learn.microsoft.com/en-us/entra/identity-platform/permissions-consent-overview ."}
    ],
    unresolved: ["The exact permission/scope grant, delegated user, supported cloud, app/callback, tenant policy and access to the requested drive item remain unverified; a file path or ID does not grant access.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "google-docs.get-document": { credential: "Google OAuth access token", account: "Select the Google account and an accessible document.",
    ...googleWorkspace("Google Docs API (docs.googleapis.com)", "For documents.get, request https://www.googleapis.com/auth/documents.readonly . This is not a grant for only the supplied document ID. The selected call omits includeTabsContent and uses Google's default first-tab content behavior; it does not request every tab.", "https://developers.google.com/workspace/docs/api/reference/rest/v1/documents/get") },
  "google-drive.get-file": { credential: "Google OAuth access token", account: "Select the Google account and a file whose metadata may be read.",
    ...googleWorkspace("Google Drive API (drive.googleapis.com)", "For files.get default metadata, https://www.googleapis.com/auth/drive.metadata.readonly is a restricted scope permitting general metadata reads for files accessible to the account. Alternatively, https://www.googleapis.com/auth/drive.file is non-sensitive and works only for files already authorized to this OAuth app through a supported app-created/opened or picker selection flow; knowing a file ID or sharing it with the user alone does not establish that app authorization. This connector provides no picker or file-authorization flow. drive.file is file-scoped but also permits writes to those files; it is not a read-only token. The selected call sets supportsAllDrives, omits fields and alt=media, and neither downloads content nor promises every metadata field. Check restricted-scope verification and any applicable server-side data security assessment; do not substitute drive.file without its file-authorization flow. Scope distinctions: https://developers.google.com/workspace/drive/api/guides/api-specific-auth ; default fields: https://developers.google.com/workspace/drive/api/guides/fields-parameter .", "https://developers.google.com/workspace/drive/api/reference/rest/v3/files/get") },
  "contentful.get-entry": {
    credential: "Contentful Management access token",
    account: "Select the Contentful space and environment and an entry accessible to this management credential.",
    additional: [
      {"kind":"developer_registration","instruction":"Use a Content Management API personal access token, not a Content Delivery or Preview API key. An authorized token owner can create a PAT through Settings > CMA tokens; for this entry GET, provision the documented content_management_read scope through the supported token-management flow rather than the broader content_management_manage scope. Do not assume every UI token was created read-only. Retain it with the host credential owner. Source checked 2026-10-03: https://www.contentful.com/developers/docs/references/authentication/ and https://www.contentful.com/developers/docs/references/content-management-api/access-tokens/ ."},
      {"kind":"application_binding","instruction":"Select the exact space, environment and entry accessible to the token owner. A PAT inherits that user's space/organization permissions; its scope does not independently restrict it to this one space. This adapter uses api.contentful.com and the management entry endpoint, so an EU-region API origin or Delivery-key setup is not interchangeable. Space roles/content permissions can still restrict the read. Source checked 2026-10-03: https://www.contentful.com/developers/docs/references/content-management-api/access-tokens/ and https://www.contentful.com/developers/docs/references/content-management-api/roles/ ."}
    ],
    unresolved: ["The actual PAT scope, user's space/environment/content permissions, matching API region and entry access remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "outlook.get-message": {
    credential: "Microsoft Graph OAuth access token",
    account: "Select the Microsoft account, matching cloud (commercial or government) and any explicit mailbox. Addressing a mailbox does not prove delegated/application permissions.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an approved Microsoft Entra app, or register one for the host's account types and redirect URI. The host owns authorization, client secrets and token renewal; supply the Graph access token, not client credentials. Checked 2026-10-03: https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app ."},
      {"kind":"developer_registration","instruction":"For reading message content, use Mail.Read. Microsoft also offers Mail.ReadBasic (delegated) or Mail.ReadBasic.All (application) when only basic properties are needed; those omit body, body preview, attachments and extended properties. This selected request asks for a text body and does not deliberately project a basic-only field set, so do not promise content under a basic grant. Sources checked 2026-10-03: https://learn.microsoft.com/en-us/graph/api/message-get?view=graph-rest-1.0 and https://learn.microsoft.com/en-us/graph/permissions-reference ."},
      {"kind":"developer_registration","instruction":"Without mailbox, the route is /me/messages and requires delegated authorization. An explicit mailbox uses /users/{mailbox}/messages: delegated access to another mailbox needs actual sharing/delegation and the applicable shared-mail permission (for example Mail.Read.Shared); app-only Mail.Read needs admin consent and approved mailbox scope. A mailbox string alone creates no delegation. No Mail.Send or Mail.ReadWrite is needed for this read. Source checked 2026-10-03: https://learn.microsoft.com/en-us/graph/api/message-get?view=graph-rest-1.0 ."},
      {"kind":"application_binding","instruction":"Match the token's cloud to this profile: commercial uses graph.microsoft.com (including ordinary GCC); government uses graph.microsoft.us (GCC High). DoD's dod-graph.microsoft.us and China endpoints are not represented by these two choices. Source checked 2026-10-03: https://learn.microsoft.com/en-us/graph/deployments ."},
      {"kind":"administrator","instruction":"Tenant consent policy and resource access still apply. An administrator may need to approve delegated access; application permissions require administrator consent. Do not add broad write permissions merely to bypass denied access. Checked 2026-10-03: https://learn.microsoft.com/en-us/entra/identity-platform/permissions-consent-overview ."}
    ],
    unresolved: ["The exact permission/scope grant, delegated versus application mode, mailbox sharing or application access scope, cloud, tenant policy and message access remain unverified. This help does not inspect the token or mailbox.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "hubspot.get-contact": {
    credential: "HubSpot OAuth access token",
    account: "Select the HubSpot account whose contact may be read.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an approved HubSpot OAuth app, with its operator configuring the host's redirect URI and requested scopes. Install/authorize it into the intended HubSpot account and let the host retain the token and account binding. App installation eligibility and required versus optional scopes depend on the app and account. Source checked 2026-10-03: https://developers.hubspot.com/docs/apps/developer-platform/build-apps/authentication/scopes ."},
      {"kind":"developer_registration","instruction":"Use crm.objects.contacts.read for ordinary contact-property reads. The selected call is GET /crm/v3/objects/contacts/{id} with fixed defaults plus explicitly requested additional properties; it does not perform a property-schema lookup. Sensitive or highly sensitive properties can need their own approved read scopes and an eligible account, so do not assume the ordinary scope unlocks every additional property. Sources checked 2026-10-03: https://developers.hubspot.com/docs/api-reference/legacy/crm/objects/contacts/get-contact and https://developers.hubspot.com/docs/apps/legacy-apps/authentication/working-with-oauth ."},
      {"kind":"application_binding","instruction":"Confirm whether this OAuth application is account-level or one of HubSpot's current user-level apps. User-level apps enforce the authorizing user's CRM permissions; do not assume all OAuth apps have that limitation or all tokens can read every contact. The connector does not choose or convert the app model. Source checked 2026-10-03: https://developers.hubspot.com/changelog/fall-2026-spotlight ."}
    ],
    unresolved: ["The exact permission/scope grant, authorized HubSpot account, app access model, installation approval, sensitive-property eligibility and contact visibility remain unverified; additional property names are not grants.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "supabase.get-user": {
    credential: "Supabase service role key for the selected Auth admin SDK path",
    account: "Select the canonical HTTPS project origin and the user record to read.",
    additional: [
      { kind: "administrator", instruction: "Have the project's existing credential owner supply the service role key to the trusted host. Do not substitute a user's session token or expose this admin credential to an agent." },
      {"kind":"developer_registration","instruction":"For this pinned service-role auth path, have the project's authorized owner obtain its legacy service_role key from Dashboard > Settings > API Keys and retain it only in the trusted host. The selected SDK sends it as both apikey and Bearer authentication to /auth/v1/admin/users/{id}; an anon/publishable key or ordinary user session is not an administrator credential. Sources checked 2026-10-03: https://supabase.com/docs/guides/getting-started/api-keys and https://supabase.com/docs/reference/javascript/auth-admin-getuserbyid ."},
      {"kind":"application_binding","instruction":"service_role is a broad project administrator credential that bypasses Row Level Security, not a per-user read grant. Supabase now recommends secret keys and documents legacy-key deprecation by the end of 2026. New secret-key compatibility with this pinned SDK/transport has not been established here; do not describe legacy-key setup as the long-term integration design or revoke shared project keys as a setup shortcut. Source checked 2026-10-03: https://supabase.com/docs/guides/getting-started/api-keys ."}
    ],
    unresolved: ["The project's legacy-key availability, trusted-host custody, requested user and compatibility with the provider's key migration remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "sendgrid.find-list-by-name": {
    credential: "SendGrid API key",
    account: "Select the SendGrid account and matching region (us or eu) with permission to read its contact lists.",
    additional: [
      {"kind":"developer_registration","instruction":"In SendGrid Settings > API Keys, create a named Custom Access key through an authorized account owner for Marketing access. The documented marketing.read scope grants both read and write access to the Email Marketing API despite its name; do not present it as read-only. The selected list GET does not require Mail Send or sender-domain verification. Keep the key with the host credential owner. Sources checked 2026-10-03: https://www.twilio.com/docs/sendgrid/ui/account-and-settings/api-keys and https://www.twilio.com/docs/sendgrid/api-reference/api-key-permissions ."},
      {"kind":"application_binding","instruction":"The selected call reads GET /v3/marketing/lists with page_size=1000, then matches the requested name locally. Select the account's US or EU regional API and check its Marketing access; the profile does not follow additional list pages or impersonate a subuser with an on-behalf-of header. Source checked 2026-10-03: https://www.twilio.com/docs/sendgrid/api-reference/lists/get-all-lists ."}
    ],
    unresolved: ["The actual Marketing permission grant, regional entitlement/account, list visibility and coverage beyond the single page remain unverified. Confirm the current permission control offered by the account before creating its key.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "webflow.get-collection-item": {
    credential: "Webflow OAuth access token",
    account: "Select the Webflow account and an accessible collection/item for the selected v2 read.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an approved Webflow OAuth app. If operating your own, a Workspace administrator registers a Data Client app with its redirect URI and scopes; confirm its private/testing or published distribution is appropriate for the target workspace. Sources checked 2026-10-03: https://developers.webflow.com/apps/data/docs/register-an-app ."},
      {"kind":"developer_registration","instruction":"Request cms:read for the selected v2 collection-item GET. This is the staged-item endpoint, not a guarantee of the published/live site's content, and it does not require CMS write permission. Source checked 2026-10-03: https://developers.webflow.com/data/reference/cms/collection-items/staged-items/get-item ."},
      {"kind":"application_binding","instruction":"The user must authorize the app for the intended site/workspace and have access to its collection. Supplying collection_id and collection_item_id does not authorize a different site. The host must keep the OAuth installation and selected site association."}
    ],
    unresolved: ["The exact permission/scope grant, app distribution/approval, OAuth callback, authorized site/workspace and collection access remain unverified. This profile does not publish content or discover a different installation.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "jira-cloud.get-issue": {
    credential: "Jira Cloud API token paired with the configured username/email for Basic authentication",
    account: "Select the canonical HTTPS Jira Cloud origin and an accessible issue. Jira Data Center is outside this profile.",
    additional: [
      {"kind":"developer_registration","instruction":"This selected Jira Cloud route uses an Atlassian account email and an unscoped API token created at https://id.atlassian.com/manage-profile/security/api-tokens . Choose a name and expiry through the authorized account owner; keep the token with the host secret owner. Scoped tokens use api.atlassian.com/ex/.../{cloudId}, which this direct-site profile does not implement. Do not paste that gateway path into origin or silently substitute a scoped token. Source checked 2026-10-03: https://support.atlassian.com/atlassian-account/docs/manage-api-tokens-for-your-atlassian-account ."},
      {"kind":"application_binding","instruction":"The selected call is GET /rest/api/3/issue/{id}; the account still needs permission to view that resource. An unscoped token follows the user's permissions and is not made read-only by this one GET. The host must account for expiry and organization restrictions. Source checked 2026-10-03: https://developer.atlassian.com/cloud/jira/platform/basic-auth-for-rest-apis/ ."},
      {"kind":"developer_registration","instruction":"Atlassian documents Basic authentication for personal scripts/manual calls and directs distributed integrations toward approved app authentication. Its current guidance disallows apps that collect customers' API tokens or ask customers to create individual OAuth apps. Treat this profile as a personal direct-site compatibility route, not a supported customer-onboarding design; a distributed integration needs a separately implemented appropriate app route. Source checked 2026-10-03: https://developer.atlassian.com/cloud/jira/platform/basic-auth-for-rest-apis/ ."}
    ],
    unresolved: ["The authorized personal-use context, account/site permissions, token expiry and policy remain unverified. Scoped gateway, service-account scoped tokens and OAuth app routes are not implemented by this profile.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "dropbox.get-file-metadata": {
    credential: "Dropbox OAuth access token",
    account: "Select the Dropbox account and an accessible provider path, file ID or revision.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an existing Dropbox OAuth app or have its operator create a Scoped access app in App Console, choosing App folder or Full Dropbox according to the intended files. Configure its redirect URI and permissions, then let the host perform user authorization and store/refresh tokens. App folder access cannot read arbitrary files outside that folder. Source checked 2026-10-03: https://docs.dropboxapi.com/dropbox-api/docs/oauth ."},
      {"kind":"developer_registration","instruction":"Enable files.metadata.read for /2/files/get_metadata. Although Dropbox uses POST for this RPC, it reads metadata and needs neither file-content download nor write permission. Source checked 2026-10-03: https://raw.githubusercontent.com/dropbox/dropbox-api-spec/main/files.stone ."},
      {"kind":"application_binding","instruction":"This profile uses a user access token without Dropbox-API-Select-User, Select-Admin or Path-Root headers. Do not substitute a team token expecting automatic member selection or assume it can traverse the whole team space. File paths/IDs must be reachable within the selected user's app/root access. Source checked 2026-10-03: https://developers.dropbox.com/en-us/dbx-team-files-guide ."}
    ],
    unresolved: ["The exact permission/scope grant, app-folder versus full-Dropbox access, selected user/root, team policy and file visibility remain unverified; this connector does not configure team-member or namespace delegation.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "google-sheets.get-values": { credential: "Google OAuth access token", account: "Select the Google account and an accessible spreadsheet, worksheet and bounded A1 range.",
    ...googleWorkspace("Google Sheets API (sheets.googleapis.com)", "For spreadsheets.values.get, request https://www.googleapis.com/auth/spreadsheets.readonly . Supply the spreadsheet ID, worksheet title and bounded A1 range. That range limits this request, not the token's access to other spreadsheets. This selected call does not require a Drive file search or write scope.", "https://developers.google.com/workspace/sheets/api/reference/rest/v4/spreadsheets.values/get") },
  "microsoft-excel-365.get-table-columns": {
    credential: "Microsoft Graph OAuth access token",
    account: "Select the Microsoft account and matching cloud (commercial or government), then the accessible OneDrive or SharePoint table.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an approved Microsoft Entra app, or register one for the host's account types and redirect URI. The host owns authorization, client secrets and token renewal; supply the Graph access token, not client credentials. Checked 2026-10-03: https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app ."},
      {"kind":"developer_registration","instruction":"Microsoft documents delegated Files.ReadWrite for listing table columns; application permissions are not supported. The token is therefore write-capable even though this selected tool only reads. Do not claim Files.Read suffices or request Sites.ReadWrite.All merely because the workbook is in SharePoint. Source checked 2026-10-03: https://learn.microsoft.com/en-us/graph/api/table-list-columns?view=graph-rest-1.0 ."},
      {"kind":"application_binding","instruction":"Use a workbook in supported OneDrive for Business or SharePoint storage and a user who can access it. The Excel API overview excludes OneDrive Consumer, so the generic permission table's personal-account entry is not a promise that a consumer workbook works. Supply the workbook/worksheet/table IDs (and SharePoint site/drive IDs when selected); this call does no separate resource discovery. Source checked 2026-10-03: https://learn.microsoft.com/en-us/graph/api/resources/excel?view=graph-rest-1.0 ."},
      {"kind":"application_binding","instruction":"Match the token's cloud to this profile: commercial uses graph.microsoft.com (including ordinary GCC); government uses graph.microsoft.us (GCC High). DoD's dod-graph.microsoft.us and China endpoints are not represented by these two choices. Source checked 2026-10-03: https://learn.microsoft.com/en-us/graph/deployments ."},
      {"kind":"administrator","instruction":"Tenant consent policy and resource access still apply. An administrator may need to approve delegated access; application permissions require administrator consent. Do not add broad write permissions merely to bypass denied access. Checked 2026-10-03: https://learn.microsoft.com/en-us/entra/identity-platform/permissions-consent-overview ."}
    ],
    unresolved: ["The exact permission/scope grant, delegated user, workbook storage/type/access, supported cloud and tenant policy remain unverified. This read-only tool does not narrow the required write-capable token.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "microsoft-teams.get-channel-message": {
    credential: "Microsoft Graph OAuth access token",
    account: "Select the Microsoft account and matching cloud (commercial or government), then an accessible team/channel/message. This read is not an agent frontend.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an approved Microsoft Entra app, or register one for the host's account types and redirect URI. The host owns authorization, client secrets and token renewal; supply the Graph access token, not client credentials. Checked 2026-10-03: https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app ."},
      {"kind":"developer_registration","instruction":"For a work/school user's delegated token, use ChannelMessage.Read.All for this channel-message or reply GET. For an app-only token, Microsoft lists ChannelMessage.Read.Group with resource-specific consent to the actual team, or the broader ChannelMessage.Read.All application permission. The Group option needs the appropriate Teams app installation/consent; adding a scope string alone does not grant it. Personal Microsoft accounts are unsupported. Source checked 2026-10-03: https://learn.microsoft.com/en-us/graph/api/chatmessage-get?view=graph-rest-1.0 ."},
      {"kind":"application_binding","instruction":"This is GET /teams/{team}/channels/{channel}/messages/{message}, optionally a reply. It is not a chat read, message sender or agent frontend. Avoid copying the donor app's chat/write/group-management scopes or its unused user-profile validation into this selected read. Source checked 2026-10-03: https://learn.microsoft.com/en-us/graph/api/chatmessage-get?view=graph-rest-1.0 ."},
      {"kind":"application_binding","instruction":"Match the token's cloud to this profile: commercial uses graph.microsoft.com (including ordinary GCC); government uses graph.microsoft.us (GCC High). DoD's dod-graph.microsoft.us and China endpoints are not represented by these two choices. Source checked 2026-10-03: https://learn.microsoft.com/en-us/graph/deployments ."},
      {"kind":"administrator","instruction":"For the broad ChannelMessage.Read.All application grant, obtain tenant administrator consent. ChannelMessage.Read.Group instead uses resource-specific consent through Teams app installation by an authorized resource owner under the tenant's app/RSC policy; it does not universally require tenant-admin consent. The host must confirm which grant the app actually has. Source checked 2026-10-03: https://learn.microsoft.com/en-us/microsoftteams/platform/graph-api/rsc/resource-specific-consent ."}
    ],
    unresolved: ["The exact permission/scope grant, delegated membership or app-only team consent, administrator policy, cloud and channel/message access remain unverified. A team ID does not establish resource-specific consent.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "quickbooks.find-invoice": {
    credential: "QuickBooks Online OAuth access token",
    account: "Select the configured company_id and an invoice number accessible to that connection.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an approved Intuit OAuth2 app for QuickBooks Online, or have the operator create one and configure the host's redirect URI and appropriate production credentials. Request com.intuit.quickbooks.accounting, not Payments, for this Accounting API query. That scope covers wider accounting reads/writes, not just invoices. Sources checked 2026-10-03: https://developer.intuit.com/app/developer/qbo/docs/develop/authentication-and-authorization/scopes and https://help.developer.intuit.com/s/article/Configuring-the-Oauth-2-0-Playground-for-QuickBooks-Online ."},
      {"kind":"application_binding","instruction":"Bind company_id to the realmId of the same QuickBooks company authorized in the OAuth connection. The selected invoice query uses the production quickbooks.api.intuit.com endpoint and minorversion 75; sandbox companies/credentials require a different endpoint and are not selected by this profile. Source checked 2026-10-03: https://developer.intuit.com/app/developer/qbo/docs/api/accounting/most-commonly-used/invoice ."},
      {"kind":"administrator","instruction":"Have an authorized company administrator approve the connection and confirm the app's production eligibility. Intuit Developer Support states that connecting requires company admin access; possessing an app's client credentials alone does not authorize a company's books. Source checked 2026-10-03: https://help.developer.intuit.com/s/question/0D5TR00001OXV0m0AH/does-authorizing-a-thirdparty-oauth2-app-to-a-quickbooks-online-company-require-an-admin-user-or-can-a-nonadmin-authorize-it-is-the-apps-access-strictly-limited-to-the-scopes-it-requests-and-can-an-admin-revoke-it-anytime ."}
    ],
    unresolved: ["The exact permission/scope grant, production app eligibility, authorizing company administrator, realmId/token pairing and invoice access remain unverified; a configured company_id is not a tenant authorization.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "xero.find-invoice": {
    credential: "Xero OAuth access token",
    account: "Select the configured tenant_id and permission to search invoices in that tenant.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an approved Xero OAuth2 application with the host's appropriate registered redirect/authorization flow. Have the host authorize the intended organization and bind tenant_id to an actual connection for that token. This profile sends Xero-Tenant-Id and does not discover connections, sign JWTs or perform token refresh. Sources checked 2026-10-03: https://developer.xero.com/documentation/guides/oauth2/auth-flow/ and https://developer.xero.com/documentation/guides/oauth2/scopes/ ."},
      {"kind":"developer_registration","instruction":"For an app using Xero's granular scopes, accounting.invoices.read is the read-only invoice-family option for this GET /Invoices. It covers more than one invoice resource. Do not copy the donor's broad accounting.transactions write scope. Existing apps still on the documented legacy transition may have accounting.transactions.read; that is a legacy alternative, not the scope to select for a newly created app. Sources checked 2026-10-03: https://developer.xero.com/faq/granular-scopes and https://raw.githubusercontent.com/XeroAPI/xero-prompt-library/main/python/SKILL.md ."},
      {"kind":"developer_registration","instruction":"Xero's current transition requires granular scopes for apps created on or after 2 March 2026; earlier apps must migrate by September 2027 and users must reauthorize changed scopes. Confirm the actual app's scope generation and grant before reuse. offline_access, when the host needs refresh tokens, is separate from permission to read invoices. Source checked 2026-10-03: https://developer.xero.com/faq/granular-scopes ."},
      {"kind":"application_binding","instruction":"The selected call searches one bounded invoice page in the connected tenant, optionally filtering receivable/payable invoices. Xero requires the connecting user to have Standard, Adviser or Administrator access with Connected Apps permission; a read-only OAuth scope does not turn a read-only user role into an eligible connector. A paid Custom Connection is a different authorization model, not an automatic replacement for this tenanted OAuth setup. Sources checked 2026-10-03: https://developer.xero.com/faq/permissions and https://developer.xero.com/faq/custom-integration ."}
    ],
    unresolved: ["The exact permission/scope grant, granular-versus-legacy app status, tenant/token connection, user organization permissions and reauthorization or refresh state remain unverified. Current generated API specs still contain legacy transaction scopes, so the host must check the app's actual transition status.","A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness."]
  },
  "algolia.browse-records": {
    credential: "Algolia API key",
    account: "Select the application_id and index_name that the key may browse.",
    additional: [
      {"kind":"developer_registration","instruction":"Create a dedicated Algolia API key in the intended application's API Keys settings with the browse ACL and restrict it to the configured index. A search-only key does not authorize browsing; the Admin API key is broader than this operation needs. The host secret owner retains the key. Source checked 2026-10-03: https://www.algolia.com/doc/guides/security/api-keys ."},
      {"kind":"application_binding","instruction":"Bind application_id and index_name to that key. The selected POST /1/indexes/{index}/browse is a read and can enumerate index records; index/key restrictions, validity and rate limits still apply. The configured result limit is a request bound, not an API-key permission boundary. Source checked 2026-10-03: https://www.algolia.com/doc/guides/security/api-keys ."}
    ],
    unresolved: ["The browse ACL, exact index restriction, application identity, key validity and accessible records remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "ghost.get-post": {
    credential: "Ghost Admin API key in 24-hex-key-id:64-hex-secret format; the helper signs a short-lived Admin JWT locally",
    account: "Select the canonical HTTPS Ghost origin and a post accessible to that Admin API key.",
    additional: [
      {"kind":"developer_registration","instruction":"Create a Custom Integration in Ghost Admin's Integrations settings and obtain its Admin API key, not its Content API key. The trusted host keeps the id:secret value; the selected connector signs the short-lived Admin JWT locally. The configured origin must be the actual HTTPS admin origin. Source checked 2026-10-03: https://docs.ghost.org/admin-api ."},
      {"kind":"application_binding","instruction":"An integration has fixed Admin API permissions that include writing; it is not a read-only credential. The selected call retrieves one post with HTML, tags and authors, and does not publish it. A site using another admin domain must use that origin, rather than relying on redirects. Source checked 2026-10-03: https://docs.ghost.org/admin-api ."}
    ],
    unresolved: ["The integration's key, admin origin, site compatibility and post access remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "monday.get-items": {
    credential: "Monday API token",
    account: "Select the Monday account and item IDs visible to that token.",
    additional: [
      {"kind":"developer_registration","instruction":"For the selected personal-token path, open the intended monday.com account's profile menu > Developers > API token > Show. The user must have API access. Retain the personal V2 token with the host credential owner; it carries all scopes allowed by that user's UI permissions, not a separately selectable boards:read scope. Source checked 2026-10-03: https://developer.monday.com/api-reference/docs/authentication ."},
      {"kind":"application_binding","instruction":"Select item IDs visible under the user's board, item, column and account permissions. This GraphQL POST contains an items query, not a mutation. App OAuth tokens have separate scopes and registration; this personal-token setup does not register an app. Regenerating the personal token invalidates existing uses, so setup must not rotate a shared token casually. Source checked 2026-10-03: https://developer.monday.com/api-reference/docs/authentication ."}
    ],
    unresolved: ["The user's API eligibility, item/board/column permissions, token validity and compatibility with the selected query remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "mailchimp.get-campaign-report": {
    credential: "Mailchimp OAuth access token",
    account: "Select the connection's data_center and a campaign report readable by that account. Data-center discovery does not run.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an approved Mailchimp Marketing OAuth app or register one under the account's Registered Apps page with the host's actual redirect URI. The auth owner retains the client secret, obtains user authorization, exchanges the code and reads the OAuth Metadata endpoint to obtain the user's server prefix. Configure that value as data_center alongside the access-token reference; this connector performs none of those setup exchanges. Source checked 2026-10-03: https://mailchimp.com/developer/marketing/guides/access-user-data-oauth-2/ ."},
      {"kind":"application_binding","instruction":"The selected GET /3.0/reports/{campaign_id} reads a campaign report in that account. Mailchimp Marketing OAuth has no separate report-read scope to request here; the authorizing user's role governs access. Tokens can be revoked even though the documented Marketing OAuth flow does not use refresh tokens. Sources checked 2026-10-03: https://mailchimp.com/developer/marketing/guides/access-user-data-oauth-2/ and https://mailchimp.com/developer/marketing/docs/fundamentals/ ."}
    ],
    unresolved: ["The registered callback, authorized account/role, OAuth metadata's data_center, token validity and campaign report access remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "amazon-s3.list-files": {
    credential: "AWS secret access key; access_key_id_env separately names its access key ID and optional session_token_env names its session token",
    account: "Select the AWS identity, aws_region and general-purpose bucket with permission to list its objects. No ambient credential chain, STS, OIDC or credential refresh runs.",
    additional: [
      {"kind":"administrator","instruction":"Have the AWS account's IAM owner authorize s3:ListBucket on the intended general-purpose bucket ARN. This ListObjectsV2 read does not require s3:GetObject or a wildcard write policy. The owner can constrain allowed prefixes in policy; the input prefix by itself is only a filter. Directory buckets and their session flow are outside this profile. Source checked 2026-10-03: https://docs.aws.amazon.com/AmazonS3/latest/API/API_ListObjectsV2.html ."},
      {"kind":"developer_registration","instruction":"Use host-managed temporary credentials where available, supplying the matching access-key ID, secret access key and session token together. If an approved IAM user access key is used, its owner creates it in IAM Security credentials and protects it; never use root-user keys. This connector accepts explicit credentials only and does not perform role assumption, ambient discovery or refresh. The host must replace expired temporary credentials. Source checked 2026-10-03: https://docs.aws.amazon.com/IAM/latest/UserGuide/id_credentials_access-keys.html ."}
    ],
    unresolved: ["The IAM identity, bucket/region, policy restrictions, matching credential set and expiry remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "activecampaign.create-contact": {
    credential: "ActiveCampaign API key",
    account: "Select the canonical HTTPS account origin and a credential allowed to create contacts. The host must separately approve the write.",
    additional: [
      {"kind":"developer_registration","instruction":"In the intended ActiveCampaign account, open Settings > Developer and obtain that user's API URL and key. Configure the canonical HTTPS origin from that API URL, not the marketing website; the host credential owner keeps the key. Each user has distinct API credentials and deleting that user breaks integrations using them. Source checked 2026-10-03: https://help.activecampaign.com/hc/en-us/articles/207317590-Getting-started-with-the-API ."},
      {"kind":"application_binding","instruction":"Use a user authorized to create contacts. The selected POST /api/3/contacts creates a contact with the supplied fields; it is not the upsert endpoint and does not subscribe a contact to a list. The host must approve this concrete write and reconcile uncertain outcomes before retrying. Source checked 2026-10-03: https://developers.activecampaign.com/reference/create-a-new-contact ."}
    ],
    unresolved: ["The user's actual create permission, API origin/key, custom-field IDs and authorization for this contact write remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "freshservice.create-ticket": {
    credential: "Freshservice API key",
    account: "Select the Freshservice domain and a credential allowed to create tickets. The host must separately approve the write.",
    additional: [
      {"kind":"developer_registration","instruction":"Obtain the intended Freshservice agent's API key from Profile settings > Your API Key through the authorized credential owner. This route uses Basic authentication with the key as username and X as password, not an OAuth app or the agent's account password. Configure the actual Freshservice domain. Sources checked 2026-10-03: https://support.freshservice.com/support/solutions/articles/50000000306-where-do-i-find-my-api-key- and https://api.freshservice.com/ ."},
      {"kind":"administrator","instruction":"The selected POST /api/v2/tickets requires the agent's role to permit ticket creation and access to the relevant workspace. Supply an existing requester_id and the account's required ticket/custom fields; this profile does not create requesters or select workspace_id. Do not use an administrator key merely to bypass a missing role grant. The host must approve the concrete write and reconcile uncertain outcomes before retrying. Source checked 2026-10-03: https://support.freshservice.com/support/solutions/articles/50000005577-understanding-roles-for-agents ."}
    ],
    unresolved: ["The agent role/workspace, configured domain, requester, required fields and concrete write authorization remain unverified; workspace-specific routing beyond this profile's fields is not established.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "gitea.create-issue": {
    credential: "Gitea access token",
    account: "Select the canonical HTTPS origin, owner and repository where this token may create issues. The host must separately approve the write.",
    additional: [
      {"kind":"developer_registration","instruction":"This selected route sends Bearer authentication. Use an approved Gitea OAuth application on the configured instance, or register one in Settings > Applications with the host's redirect URI. The host auth owner performs authorization-code exchange and protected token custody. Gitea's documented personal API token route uses Authorization: token, so do not assume a PAT is interchangeable with this Bearer path. Sources checked 2026-10-03: https://docs.gitea.com/development/oauth2-provider/ and https://docs.gitea.com/development/api-usage/ ."},
      {"kind":"application_binding","instruction":"On Gitea 1.23 or later, request the documented granular write:issue scope for the selected POST /api/v1/repos/{owner}/{repository}/issues and verify the user can create issues in that repository. Older OAuth behavior can grant broader access; unrecognized scopes must not be treated as restrictions. The host approves the title/body write and handles an uncertain outcome before retrying. Source checked 2026-10-03: https://docs.gitea.com/development/oauth2-provider/ ."}
    ],
    unresolved: ["The instance version, OAuth callback/token/scopes, repository issues availability, user permission and concrete write authorization remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "datadog.submit-log": {
    credential: "Datadog API key",
    account: "Select the account's supported site and a key permitted to submit logs there. The host must separately approve the write.",
    additional: [
      {"kind":"developer_registration","instruction":"Have an authorized Datadog operator create a named API key under Organization Settings > API Keys for the intended organization. Keep it with the host credential owner. This log intake route needs the organization's API key, not an application key or a browser client token. Source checked 2026-10-03: https://docs.datadoghq.com/account_management/api-app-keys/ ."},
      {"kind":"application_binding","instruction":"Select the organization's supported Datadog site; API keys are not interchangeable across sites. The selected POST /api/v2/logs submits a log and can create billable ingestion. A 202 acknowledgement means accepted for processing, not proof the log was indexed or delivered to a reader. The host must approve the concrete payload and reconcile an uncertain submission before retrying. Source checked 2026-10-03: https://docs.datadoghq.com/api/latest/logs/send-logs/ ."}
    ],
    unresolved: ["The organization/site, key validity and ingestion entitlement, payload approval and downstream log availability remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "gmail.get-thread": { credential: "Gmail OAuth access token", account: "Select the Google mailbox account and a readable thread; the request uses users/me, which is not an independent account-identity check.",
    ...googleWorkspace("Gmail API (gmail.googleapis.com)", "For users.threads.get with format full (the default), request https://www.googleapis.com/auth/gmail.readonly . If every requested read explicitly uses metadata or minimal, https://www.googleapis.com/auth/gmail.metadata is an alternative; it cannot serve full. Both are restricted scopes. No send, compose or modify permission is needed for this read. The host must keep later operation format choices consistent with its granted scope; this help does not enforce that choice. The OAuth application operator must assess restricted-scope verification and, if restricted data is stored or transmitted on servers, Google's security-assessment requirements and applicable exceptions. https://developers.google.com/workspace/gmail/api/auth/scopes .", "https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.threads/get") },
  "slack.get-thread-replies": {
    credential: "Slack OAuth access token",
    account: "Select the Slack workspace identity and a channel/thread whose replies this token may read. This connector is not an agent frontend.",
    additional: [
      {"kind":"developer_registration","instruction":"Use an approved Slack OAuth app or create one for the target workspace. Configure OAuth & Permissions and the host's HTTPS redirect; the host auth owner performs consent and code exchange. Request only the conversation-type history scopes needed: channels:history for public channels, groups:history for private channels, im:history for DMs or mpim:history for group DMs. Select the appropriate bot or user token and preserve its identity. Sources checked 2026-10-03: https://docs.slack.dev/authentication/installing-with-oauth/ and https://docs.slack.dev/reference/methods/conversations.replies/ ."},
      {"kind":"application_binding","instruction":"Install/authorize the app under workspace policy and ensure the token identity can access the conversation. Supply its channel ID and parent message timestamp. This connector reads replies and is not a Slack agent frontend; it registers no events or message sender. Slack documents stricter limits for new non-Marketplace commercially distributed installations than internal/Marketplace apps. This pinned action asks for 200 per page and has bounded pagination, so a valid token does not prove completeness or compatibility with every distribution tier. Source checked 2026-10-03: https://docs.slack.dev/reference/methods/conversations.replies/ ."}
    ],
    unresolved: ["The installed app's scopes, token identity, workspace policy, conversation membership/access and distribution-tier rate limits remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "linear.get-issue": {
    credential: "Linear API key",
    account: "Select the Linear account and an issue visible to this key.",
    additional: [
      {"kind":"developer_registration","instruction":"Create a named personal API key under Linear Settings > Account > Security & access. Select Read permission and only the required teams; this issue query does not need Write, Admin, Create issues or Create comments. Keep the key with the host credential owner. If member API-key creation is disabled, a workspace administrator must address that policy. Source checked 2026-10-03: https://linear.app/docs/api-and-webhooks and https://linear.app/docs/security-and-access ."},
      {"kind":"application_binding","instruction":"Supply the issue UUID or team issue identifier. The selected GraphQL POST contains read queries and may follow label pagination; HTTP POST does not make this a mutation. The key's team restrictions and user's private-team membership still constrain access. No OAuth app or webhook registration is required for this personal-key route. Source checked 2026-10-03: https://linear.app/developers/graphql and https://linear.app/docs/private-teams ."}
    ],
    unresolved: ["The actual key permissions/team restrictions, member-key policy, private-team membership and requested issue visibility remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
  "google-cloud-storage.search-objects": {
    credential: "Google Cloud access token",
    account: "Select the Google Cloud identity and configured bucket with permission to list its objects; no credential acquisition or refresh runs.",
    additional: [
      {"kind":"administrator","instruction":"The Google Cloud IAM owner must grant storage.objects.list on the intended bucket to the selected user or service identity. OAuth scope and IAM permission are both needed; a bucket name or prefix filter does not grant access. This default listing does not request object ACLs or download object contents. Source checked 2026-10-03: https://docs.cloud.google.com/storage/docs/json_api/v1/objects/list ."},
      {"kind":"developer_registration","instruction":"The host's auth owner obtains a Google access token with a scope permitting storage reads, such as https://www.googleapis.com/auth/devstorage.read_only . User OAuth requires the approved client/consent flow; workload or service identities use their existing host credential flow. This connector consumes the resulting access token and does not run gcloud, read ADC, sign service-account keys, impersonate or refresh credentials. Sources checked 2026-10-03: https://docs.cloud.google.com/storage/docs/oauth-scopes and https://docs.cloud.google.com/storage/docs/authentication ."}
    ],
    unresolved: ["The Google identity, actual OAuth scope, bucket IAM policy, token expiry and requested listing access remain unverified.","These instructions do not verify the live account, credential validity, resource permissions or operation readiness; the existing host credential owner must establish those facts."]
  },
};

export function authenticationGuidance(profile: unknown): {
  prerequisites: NewcomerPrerequisite[]; unresolved_details: string[];
} {
  if (!isProfile(profile)) throw new Error("Activepieces profile is invalid");
  const selected = GUIDANCE[profile];
  return {
    prerequisites: [
      { kind: "artifact_preparation", instruction: "Install the exact supported local Activepieces artifact with its retained consumer lock and compatible Node runtime. The packaged README describes setup; source code alone does not select a supported artifact." },
      { kind: "account", instruction: selected.account },
      { kind: "client_authentication", instruction: `token_env names an environment variable containing the selected ${selected.credential}. It is a credential for this local connector's provider requests, not for the MCP client-to-server connection. The host's existing secret owner supplies it only to the selected process; configuration and agent input contain references, never its value.` },
      ...(selected.additional ?? []).map(item => ({ ...item })),
      { kind: "application_binding", instruction: "The MCP client launches this local stdio server with serve --config and an explicit child environment. Authenticating a remote MCP endpoint does not issue the provider credential used here. The host owns account selection, provider login/consent, credential expiry/refresh, grants and operation approval; this server performs no acquisition or refresh." },
    ],
    unresolved_details: selected.unresolved ? [...selected.unresolved] : [
      "Current provider developer-registration, administrator-consent and exact permission/scope requirements are not established by this configuration help. The agent or host must verify them with the provider and account owner for the selected operation before provisioning credentials.",
      "A syntactically valid configuration, a supplied credential or a started MCP server does not verify the live account, its audience, permissions, credential validity or operation readiness.",
    ],
  };
}
