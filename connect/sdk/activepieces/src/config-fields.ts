import { isProfile, PROFILE_IDS, type ProfileId } from "./profiles.js";
import { isWriteProfile } from "./effect-profiles.js";
import { authenticationGuidance } from "./auth-guidance.js";

export interface ConfigurationField {
  readonly name: string;
  readonly kind: "string" | "integer";
  readonly required: boolean;
  readonly credential_reference: boolean;
}

// These declarations own configuration field presence, not semantic validation.
const EXTRA_FIELDS: Partial<Record<ProfileId, readonly string[]>> = {
  "activecampaign.create-contact": ["origin"],
  "freshservice.create-ticket": ["domain"],
  "gitea.create-issue": ["origin", "owner", "repository"],
  "datadog.submit-log": ["site"],
  "google-cloud-storage.search-objects": ["bucket"],
  "ghost.get-post": ["origin"],
  "mailchimp.get-campaign-report": ["data_center"],
  "amazon-s3.list-files": ["aws_region", "bucket", "access_key_id_env", "session_token_env"],
  "jira-cloud.get-issue": ["origin", "username"],
  "microsoft-excel-365.get-table-columns": ["cloud"],
  "microsoft-teams.get-channel-message": ["cloud"],
  "quickbooks.find-invoice": ["company_id"],
  "xero.find-invoice": ["tenant_id"],
  "algolia.browse-records": ["application_id", "index_name"],
  "freshdesk.get-contact": ["origin"],
  "supabase.get-user": ["origin"],
  "onedrive.get-item": ["cloud"],
  "outlook.get-message": ["cloud", "mailbox"],
  "contentful.get-entry": ["space", "environment"],
  "sendgrid.find-list-by-name": ["region"],
  "nocodb.get-record": ["origin", "version"],
  "baserow.get-row": ["origin"],
  "okta.find-user-by-email": ["origin"],
  "pipedrive.get-activity": ["origin"],
  "confluence.get-page-content": ["origin", "username"],
  "wordpress.get-page": ["origin", "username"],
  "jotform.get-submission": ["region"],
  "intercom.get-conversation": ["region"],
};
const OPTIONAL = new Set(["timeout_ms", "max_response_bytes", "mailbox", "session_token_env"]);
const INTEGER = new Set(["timeout_ms", "max_response_bytes", "version"]);
const CREDENTIAL_REFERENCE = new Set(["token_env", "access_key_id_env", "session_token_env"]);

export function configurationFields(profile: unknown): ConfigurationField[] {
  if (!isProfile(profile)) throw new Error("Activepieces profile is invalid");
  return ["profile", "token_env", "timeout_ms", "max_response_bytes", ...(EXTRA_FIELDS[profile] ?? [])]
    .map(name => ({ name, kind: INTEGER.has(name) ? "integer" : "string",
      required: !OPTIONAL.has(name), credential_reference: CREDENTIAL_REFERENCE.has(name) }));
}

export function mcpToolName(profile: ProfileId): string { return profile.replaceAll(".", "_"); }

export function listProfiles() {
  return { profiles: PROFILE_IDS.map(profile => ({ profile, operations: [mcpToolName(profile)],
    effect: isWriteProfile(profile) ? "write" : "read" })) };
}

export function configurationHelp(profile: unknown) {
  const fields = configurationFields(profile);
  const selected = profile as ProfileId;
  return { profile: selected, operations: [mcpToolName(selected)],
    effect: isWriteProfile(selected) ? "write" : "read", fields,
    ...authenticationGuidance(selected),
    guide: "README.md",
    scope: "Field requirements only, not a complete JSON Schema. The packaged guide describes semantic constraints and account prerequisites. Credential references name environment variables; help and check-config never read their values. Configuration validity is not authentication, authorization or readiness." };
}
