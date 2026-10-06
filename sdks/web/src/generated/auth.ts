// Generated auth/connection wire contracts for @rkat/web
// Sources: artifacts/schemas/rpc-methods.json, wire-types.json, auth-connection-contracts.json

export const WIRE_AUTH_PROVIDERS = [
  "anthropic",
  "openai",
  "gemini",
  "self_hosted"
] as const;

export type WireAuthProvider = typeof WIRE_AUTH_PROVIDERS[number];

export const WIRE_OAUTH_PROVIDERS = [
  "anthropic",
  "openai",
  "google",
  "copilot"
] as const;

export type WireOAuthProvider = typeof WIRE_OAUTH_PROVIDERS[number];

export const WIRE_BACKEND_KINDS = [
  "openai_api",
  "chatgpt_backend",
  "azure_openai",
  "copilot",
  "anthropic_api",
  "bedrock",
  "vertex",
  "foundry",
  "google_genai",
  "vertex_ai",
  "google_code_assist",
  "self_hosted",
  "openai_compatible"
] as const;

export type WireBackendKind = typeof WIRE_BACKEND_KINDS[number];

export const WIRE_AUTH_METHODS = [
  "api_key",
  "azure_api_key",
  "static_bearer",
  "managed_chatgpt_oauth",
  "external_chatgpt_tokens",
  "external_authorizer",
  "github_copilot_oauth",
  "claude_ai_oauth",
  "oauth_to_api_key",
  "bedrock_bearer",
  "bedrock_aws_sigv4",
  "vertex_google_auth",
  "foundry_api_key",
  "foundry_azure_ad",
  "bearer_api_key",
  "adc",
  "api_key_express",
  "google_oauth",
  "compute_adc",
  "none"
] as const;

export type WireAuthMethod = typeof WIRE_AUTH_METHODS[number];

export const WIRE_PROVIDER_BACKEND_KINDS = {
  anthropic: [
    "anthropic_api",
    "bedrock",
    "vertex",
    "foundry",
    "copilot",
  ],
  openai: [
    "openai_api",
    "chatgpt_backend",
    "azure_openai",
    "copilot",
  ],
  gemini: [
    "google_genai",
    "vertex_ai",
    "google_code_assist",
    "copilot",
  ],
  self_hosted: [
    "self_hosted",
    "openai_compatible",
  ],
} as const satisfies Record<WireAuthProvider, readonly WireBackendKind[]>;

export const WIRE_PROVIDER_AUTH_METHODS = {
  anthropic: [
    "api_key",
    "static_bearer",
    "claude_ai_oauth",
    "oauth_to_api_key",
    "external_authorizer",
    "bedrock_bearer",
    "bedrock_aws_sigv4",
    "vertex_google_auth",
    "foundry_api_key",
    "foundry_azure_ad",
    "github_copilot_oauth",
  ],
  openai: [
    "api_key",
    "azure_api_key",
    "static_bearer",
    "managed_chatgpt_oauth",
    "external_chatgpt_tokens",
    "external_authorizer",
    "github_copilot_oauth",
  ],
  gemini: [
    "api_key",
    "bearer_api_key",
    "external_authorizer",
    "adc",
    "api_key_express",
    "google_oauth",
    "compute_adc",
    "github_copilot_oauth",
  ],
  self_hosted: [
    "api_key",
    "static_bearer",
    "none",
  ],
} as const satisfies Record<WireAuthProvider, readonly WireAuthMethod[]>;

export const WIRE_CREDENTIAL_SOURCE_KINDS = [
  "inline_secret",
  "managed_store",
  "env",
  "external_resolver",
  "platform_default",
  "command",
  "file_descriptor"
] as const;

export type WireCredentialSourceKind = typeof WIRE_CREDENTIAL_SOURCE_KINDS[number];

export const WIRE_AUTH_STATUS_STATES = [
  "valid",
  "expiring",
  "expired",
  "reauth_required",
  "refresh_failed",
  "released",
  "absent",
  "missing_credential"
] as const;

export type WireAuthStatusState = typeof WIRE_AUTH_STATUS_STATES[number];

export const WIRE_DEVICE_COMPLETE_STATES = [
  "pending",
  "slow_down",
  "access_denied",
  "expired",
  "ready"
] as const;

export type WireDeviceCompleteState = typeof WIRE_DEVICE_COMPLETE_STATES[number];

export const WIRE_LOGIN_READY_STATE = "ready" as const;

export const AUTH_RPC_METHODS = {
  profileList: "auth/profile/list",
  profileGet: "auth/profile/get",
  profileCreate: "auth/profile/create",
  profileDelete: "auth/profile/delete",
  loginStart: "auth/login/start",
  loginComplete: "auth/login/complete",
  loginCancel: "auth/login/cancel",
  loginDeviceStart: "auth/login/device_start",
  loginDeviceComplete: "auth/login/device_complete",
  loginProvisionApiKey: "auth/login/provision_api_key",
  statusGet: "auth/status/get",
  logout: "auth/logout",
} as const;

export type AuthRpcMethod = typeof AUTH_RPC_METHODS[keyof typeof AUTH_RPC_METHODS];

export interface WireAuthBindingRef {
  realm: string;
  binding: string;
  profile?: string | null;
}

export interface RealmIdParams {
  realm_id: string;
}

export interface BindingIdParams {
  realm_id: string;
  binding_id: string;
  profile_id?: string | null;
}

export interface CreateProfileParams extends BindingIdParams {
  auth_method: WireAuthMethod;
  secret: string;
}

/** MCP server addressed by auth/login/* and auth/status/get. */
export interface WireMcpAuthTarget {
  server_name: string;
  server_url: string;
  oauth_account?: string | null;
}

export interface ProviderLoginStartParams extends BindingIdParams {
  provider: WireOAuthProvider;
  redirect_uri: string;
}

export interface McpLoginStartParams {
  mcp: WireMcpAuthTarget;
  redirect_uri: string;
}

export type LoginStartParams = ProviderLoginStartParams | McpLoginStartParams;

export interface ProviderLoginCompleteParams extends BindingIdParams {
  provider: WireOAuthProvider;
  code: string;
  state: string;
  redirect_uri: string;
}

export interface McpLoginCompleteParams {
  mcp: WireMcpAuthTarget;
  code: string;
  state: string;
  redirect_uri: string;
}

export type LoginCompleteParams = ProviderLoginCompleteParams | McpLoginCompleteParams;

export interface McpLoginCancelParams {
  mcp: WireMcpAuthTarget;
  state: string;
}

/** Cancel by the non-secret `attempt.ref` that auth/status/get reports. */
export interface McpLoginCancelAttemptParams {
  mcp: WireMcpAuthTarget;
  attempt_ref: string;
}

export type LoginCancelParams = McpLoginCancelParams | McpLoginCancelAttemptParams;

export interface DeviceStartParams extends BindingIdParams {
  provider: WireOAuthProvider;
}

export interface DeviceCompleteParams extends BindingIdParams {
  provider: WireOAuthProvider;
  device_code: string;
}

export interface ProvisionApiKeyParams {
  access_token: string;
  realm_id?: string | null;
  binding_id?: string | null;
  profile_id?: string | null;
}

export interface WireBackendProfile {
  id: string;
  provider: WireAuthProvider;
  backend_kind: WireBackendKind;
  base_url?: string | null;
  options?: unknown;
  /** OpenAI backends: whether Meerkat's OpenAI prompt-cache fields apply here. Omitted for other providers. */
  prompt_cache_applicable?: boolean | null;
}

export interface WireAuthProfile {
  id: string;
  provider: WireAuthProvider;
  auth_method: WireAuthMethod;
  source_kind: WireCredentialSourceKind;
}

export interface WireProviderBinding {
  id: string;
  backend_profile: string;
  auth_profile: string;
  credential_account?: string | null;
  default_model?: string | null;
  allow_auth_override?: boolean;
  require_metadata_account?: boolean;
  require_metadata_workspace?: boolean;
}

export interface WireRealmConnectionSet {
  realm_id: string;
  backends: Record<string, WireBackendProfile>;
  auth_profiles: Record<string, WireAuthProfile>;
  bindings: Record<string, WireProviderBinding>;
  default_binding?: string | null;
}

export interface WireBindingIdentity {
  realm_id: string;
  binding_id: string;
  auth_binding: WireAuthBindingRef;
}

export interface WireAuthProfileCreated extends WireBindingIdentity {
  profile_id: string;
  provider: WireAuthProvider;
  auth_method: WireAuthMethod;
  stored: boolean;
}

export interface WireAuthProfileDetail {
  auth_binding: WireAuthBindingRef;
  binding_id: string;
  profile_id: string;
  auth_profile: WireAuthProfile;
}

export interface WireAuthProfileCleared extends WireBindingIdentity {
  profile_id: string;
  cleared: boolean;
}

export interface WireProviderLoginStart {
  authorize_url: string;
  state: string;
  redirect_uri: string;
  provider: WireOAuthProvider;
}

/** Host-channel data: never pass to an agent, tool result, transcript or log. */
export interface WireMcpLoginStart {
  authorize_url: string;
  state: string;
  redirect_uri: string;
  mcp: WireMcpAuthTarget;
  disposition: 'started' | 'joined';
}

export type WireLoginStart = WireProviderLoginStart | WireMcpLoginStart;

export interface WireProviderLoginReady extends WireBindingIdentity {
  state?: typeof WIRE_LOGIN_READY_STATE | null;
  profile_id: string;
  provider: WireOAuthProvider;
  expires_at?: string | null;
  has_refresh_token: boolean;
  scopes: string[];
}

export interface WireMcpLoginReady {
  mcp: WireMcpAuthTarget;
  account_id?: string | null;
  expires_at?: string | null;
  has_refresh_token: boolean;
  scopes: string[];
}

export type WireLoginReady = WireProviderLoginReady | WireMcpLoginReady;

export interface WireLoginCancelled {
  mcp: WireMcpAuthTarget;
  cancelled: boolean;
}

export type WireMcpAuthPhase = 'authorized' | 'reauth_required' | 'authorization_required';

export type WireMcpAuthAttemptPhase = 'pending';

/** A pending login attempt: a non-secret reference and its expiry, never the authorize URL or state. */
export interface WireMcpAuthAttempt {
  ref: string;
  phase: WireMcpAuthAttemptPhase;
  expires_at: string;
}

export interface WireMcpAuthStatus {
  mcp: WireMcpAuthTarget;
  phase: WireMcpAuthPhase;
  expires_at?: string | null;
  account_id?: string | null;
  attempt?: WireMcpAuthAttempt | null;
}

export interface WireMcpLoggedOut {
  mcp: WireMcpAuthTarget;
  cleared: boolean;
}

/** auth/logout params: a provider binding or an MCP server target. */
export type AuthLogoutParams = BindingIdParams | { mcp: WireMcpAuthTarget };

export type WireAuthLogoutResult = WireAuthProfileCleared | WireMcpLoggedOut;

export interface WireDeviceStart {
  device_code: string;
  user_code: string;
  verification_uri: string;
  verification_uri_complete?: string | null;
  expires_in: number;
  interval: number;
  provider: WireOAuthProvider;
}

export type WireDeviceCompletePending = { state: "pending" };
export type WireDeviceCompleteSlowDown = { state: "slow_down" };
export type WireDeviceCompleteAccessDenied = { state: "access_denied" };
export type WireDeviceCompleteExpired = { state: "expired" };
export type WireDeviceCompleteReady = WireProviderLoginReady & { state: typeof WIRE_LOGIN_READY_STATE };
export type WireDeviceCompleteResult =
  | WireDeviceCompletePending
  | WireDeviceCompleteSlowDown
  | WireDeviceCompleteAccessDenied
  | WireDeviceCompleteExpired
  | WireDeviceCompleteReady;

export interface WireProvisionApiKeyResult extends WireBindingIdentity {
  profile_id: string;
  provider: WireAuthProvider;
  auth_mode: WireAuthMethod;
  has_api_key: boolean;
  scopes: string[];
}

export interface WireRealmSummary {
  realm_id: string;
  default_binding?: string | null;
  backend_count: number;
  auth_profile_count: number;
  binding_count: number;
}

export interface WireRealmList {
  realms: WireRealmSummary[];
}

export interface WireAuthProfilesList {
  realm_id: string;
  auth_profiles: WireAuthProfile[];
  backend_profiles: WireBackendProfile[];
  bindings: WireProviderBinding[];
}

export interface WireAuthError {
  kind: string;
  [key: string]: unknown;
}

export interface WireAuthStatus {
  profile_id: string;
  provider: WireAuthProvider;
  auth_method: WireAuthMethod;
  state: WireAuthStatusState;
  expires_at?: string | null;
  last_refresh_at?: string | null;
  account_id?: string | null;
  last_error?: WireAuthError | null;
}

export interface WireAuthStatusDetail extends WireBindingIdentity {
  profile_id: string;
  provider: WireAuthProvider;
  auth_method: WireAuthMethod;
  state: WireAuthStatusState;
  expires_at?: string | null;
  last_refresh_at?: string | null;
  account_id?: string | null;
  has_refresh_token: boolean;
}

export class WebAuthContractError extends TypeError {
  constructor(message: string) {
    super(message);
    this.name = 'WebAuthContractError';
  }
}

function fail(path: string, expected: string): never {
  throw new WebAuthContractError(`${path}: expected ${expected}`);
}

function hasOwn(record: Record<string, unknown>, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(record, key);
}

function expectRecord(value: unknown, path: string): Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    fail(path, 'object');
  }
  return value as Record<string, unknown>;
}

function expectString(value: unknown, path: string): string {
  if (typeof value !== 'string') fail(path, 'string');
  return value;
}

function expectBoolean(value: unknown, path: string): boolean {
  if (typeof value !== 'boolean') fail(path, 'boolean');
  return value;
}

function expectNumber(value: unknown, path: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) fail(path, 'finite number');
  return value;
}

function optionalString(record: Record<string, unknown>, key: string, path: string): void {
  if (hasOwn(record, key) && record[key] !== null && record[key] !== undefined) {
    expectString(record[key], path);
  }
}

function optionalBoolean(record: Record<string, unknown>, key: string, path: string): void {
  if (hasOwn(record, key) && record[key] !== null && record[key] !== undefined) {
    expectBoolean(record[key], path);
  }
}

function expectStringArray(value: unknown, path: string): string[] {
  if (!Array.isArray(value)) fail(path, 'string array');
  value.forEach((item, index) => expectString(item, `${path}[${index}]`));
  return value as string[];
}

function parseLiteral<T extends string>(
  value: unknown,
  allowed: readonly T[],
  path: string,
  label: string,
): T {
  if (typeof value !== 'string' || !(allowed as readonly string[]).includes(value)) {
    fail(path, `${label} (${(allowed as readonly string[]).join(', ')})`);
  }
  return value as T;
}

function expectRecordMap<T>(
  value: unknown,
  path: string,
  parse: (entry: unknown, entryPath: string) => T,
): Record<string, T> {
  const record = expectRecord(value, path);
  const parsed: Record<string, T> = {};
  for (const [key, entry] of Object.entries(record)) {
    parsed[key] = parse(entry, `${path}.${key}`);
  }
  return parsed;
}

export function parseWireAuthProvider(value: unknown, path = 'provider'): WireAuthProvider {
  return parseLiteral(value, WIRE_AUTH_PROVIDERS, path, 'wire auth provider');
}

export function parseWireOAuthProvider(value: unknown, path = 'provider'): WireOAuthProvider {
  return parseLiteral(value, WIRE_OAUTH_PROVIDERS, path, 'wire OAuth provider');
}

export function parseWireBackendKind(value: unknown, path = 'backend_kind'): WireBackendKind {
  return parseLiteral(value, WIRE_BACKEND_KINDS, path, 'wire backend kind');
}

export function parseWireAuthMethod(value: unknown, path = 'auth_method'): WireAuthMethod {
  return parseLiteral(value, WIRE_AUTH_METHODS, path, 'wire auth method');
}

function validateProviderBackendKind(
  provider: WireAuthProvider,
  backendKind: WireBackendKind,
  path: string,
): void {
  const allowed = WIRE_PROVIDER_BACKEND_KINDS[provider];
  if (!(allowed as readonly string[]).includes(backendKind)) {
    fail(path, `wire backend kind for provider ${provider} (${allowed.join(', ')})`);
  }
}

function validateProviderAuthMethod(
  provider: WireAuthProvider,
  authMethod: WireAuthMethod,
  path: string,
): void {
  const allowed = WIRE_PROVIDER_AUTH_METHODS[provider];
  if (!(allowed as readonly string[]).includes(authMethod)) {
    fail(path, `wire auth method for provider ${provider} (${allowed.join(', ')})`);
  }
}

export function parseWireCredentialSourceKind(
  value: unknown,
  path = 'source_kind',
): WireCredentialSourceKind {
  return parseLiteral(value, WIRE_CREDENTIAL_SOURCE_KINDS, path, 'wire credential source kind');
}

export function parseWireAuthStatusState(value: unknown, path = 'state'): WireAuthStatusState {
  return parseLiteral(value, WIRE_AUTH_STATUS_STATES, path, 'wire auth status state');
}

export function parseWireAuthBindingRef(value: unknown, path = 'auth_binding'): WireAuthBindingRef {
  const record = expectRecord(value, path);
  expectString(record.realm, `${path}.realm`);
  expectString(record.binding, `${path}.binding`);
  optionalString(record, 'profile', `${path}.profile`);
  return value as WireAuthBindingRef;
}

function validateBindingIdentity(record: Record<string, unknown>, path: string): void {
  const realmId = expectString(record.realm_id, `${path}.realm_id`);
  const bindingId = expectString(record.binding_id, `${path}.binding_id`);
  const authBinding = parseWireAuthBindingRef(record.auth_binding, `${path}.auth_binding`);
  if (authBinding.realm !== realmId) fail(`${path}.auth_binding.realm`, `same value as ${path}.realm_id`);
  if (authBinding.binding !== bindingId) fail(`${path}.auth_binding.binding`, `same value as ${path}.binding_id`);
}

export function parseWireBackendProfile(value: unknown, path = 'backend_profile'): WireBackendProfile {
  const record = expectRecord(value, path);
  expectString(record.id, `${path}.id`);
  const provider = parseWireAuthProvider(record.provider, `${path}.provider`);
  const backendKind = parseWireBackendKind(record.backend_kind, `${path}.backend_kind`);
  validateProviderBackendKind(provider, backendKind, `${path}.backend_kind`);
  optionalString(record, 'base_url', `${path}.base_url`);
  optionalBoolean(record, 'prompt_cache_applicable', `${path}.prompt_cache_applicable`);
  return value as WireBackendProfile;
}

export function parseWireAuthProfile(value: unknown, path = 'auth_profile'): WireAuthProfile {
  const record = expectRecord(value, path);
  expectString(record.id, `${path}.id`);
  const provider = parseWireAuthProvider(record.provider, `${path}.provider`);
  const authMethod = parseWireAuthMethod(record.auth_method, `${path}.auth_method`);
  validateProviderAuthMethod(provider, authMethod, `${path}.auth_method`);
  parseWireCredentialSourceKind(record.source_kind, `${path}.source_kind`);
  return value as WireAuthProfile;
}

export function parseWireProviderBinding(value: unknown, path = 'binding'): WireProviderBinding {
  const record = expectRecord(value, path);
  expectString(record.id, `${path}.id`);
  expectString(record.backend_profile, `${path}.backend_profile`);
  expectString(record.auth_profile, `${path}.auth_profile`);
  optionalString(record, 'credential_account', `${path}.credential_account`);
  optionalString(record, 'default_model', `${path}.default_model`);
  optionalBoolean(record, 'allow_auth_override', `${path}.allow_auth_override`);
  optionalBoolean(record, 'require_metadata_account', `${path}.require_metadata_account`);
  optionalBoolean(record, 'require_metadata_workspace', `${path}.require_metadata_workspace`);
  return value as WireProviderBinding;
}

function indexWireProfiles<T extends { id: string }>(
  profiles: readonly T[],
  path: string,
  label: string,
): Record<string, T> {
  const index: Record<string, T> = {};
  profiles.forEach((profile, entryIndex) => {
    if (hasOwn(index, profile.id)) fail(`${path}[${entryIndex}].id`, `unique ${label} id`);
    index[profile.id] = profile;
  });
  return index;
}

function validateWireProviderBindings(
  bindings: readonly (readonly [WireProviderBinding, string])[],
  backends: Record<string, WireBackendProfile>,
  authProfiles: Record<string, WireAuthProfile>,
): void {
  bindings.forEach(([binding, bindingPath]) => {
    const backend = backends[binding.backend_profile];
    if (backend === undefined) fail(`${bindingPath}.backend_profile`, 'known backend profile id');
    const authProfile = authProfiles[binding.auth_profile];
    if (authProfile === undefined) fail(`${bindingPath}.auth_profile`, 'known auth profile id');
    if (backend.provider !== authProfile.provider) {
      fail(
        `${bindingPath}.auth_profile`,
        `auth profile with provider ${backend.provider} for backend ${binding.backend_profile}`,
      );
    }
  });
}

export function parseWireRealmConnectionSet(
  value: unknown,
  path = 'realm_connection_set',
): WireRealmConnectionSet {
  const record = expectRecord(value, path);
  expectString(record.realm_id, `${path}.realm_id`);
  const backends = expectRecordMap(record.backends, `${path}.backends`, parseWireBackendProfile);
  const authProfiles = expectRecordMap(record.auth_profiles, `${path}.auth_profiles`, parseWireAuthProfile);
  const bindings = expectRecordMap(record.bindings, `${path}.bindings`, parseWireProviderBinding);
  validateWireProviderBindings(
    Object.entries(bindings).map(([key, binding]) => [binding, `${path}.bindings.${key}`] as const),
    backends,
    authProfiles,
  );
  optionalString(record, 'default_binding', `${path}.default_binding`);
  return value as WireRealmConnectionSet;
}

export function parseWireAuthProfileCreated(
  value: unknown,
  path = 'auth_profile_created',
): WireAuthProfileCreated {
  const record = expectRecord(value, path);
  validateBindingIdentity(record, path);
  expectString(record.profile_id, `${path}.profile_id`);
  const provider = parseWireAuthProvider(record.provider, `${path}.provider`);
  const authMethod = parseWireAuthMethod(record.auth_method, `${path}.auth_method`);
  validateProviderAuthMethod(provider, authMethod, `${path}.auth_method`);
  expectBoolean(record.stored, `${path}.stored`);
  return value as WireAuthProfileCreated;
}

export function parseWireAuthProfileDetail(
  value: unknown,
  path = 'auth_profile_detail',
): WireAuthProfileDetail {
  const record = expectRecord(value, path);
  const bindingId = expectString(record.binding_id, `${path}.binding_id`);
  const authBinding = parseWireAuthBindingRef(record.auth_binding, `${path}.auth_binding`);
  if (authBinding.binding !== bindingId) fail(`${path}.auth_binding.binding`, `same value as ${path}.binding_id`);
  expectString(record.profile_id, `${path}.profile_id`);
  parseWireAuthProfile(record.auth_profile, `${path}.auth_profile`);
  return value as WireAuthProfileDetail;
}

export function parseWireAuthProfileCleared(
  value: unknown,
  path = 'auth_profile_cleared',
): WireAuthProfileCleared {
  const record = expectRecord(value, path);
  validateBindingIdentity(record, path);
  expectString(record.profile_id, `${path}.profile_id`);
  expectBoolean(record.cleared, `${path}.cleared`);
  return value as WireAuthProfileCleared;
}

export function parseWireMcpAuthTarget(value: unknown, path = 'mcp'): WireMcpAuthTarget {
  const record = expectRecord(value, path);
  expectString(record.server_name, `${path}.server_name`);
  expectString(record.server_url, `${path}.server_url`);
  optionalString(record, 'oauth_account', `${path}.oauth_account`);
  return value as WireMcpAuthTarget;
}

export function parseWireLoginStart(value: unknown, path = 'login_start'): WireLoginStart {
  const record = expectRecord(value, path);
  expectString(record.authorize_url, `${path}.authorize_url`);
  expectString(record.state, `${path}.state`);
  expectString(record.redirect_uri, `${path}.redirect_uri`);
  if (hasOwn(record, 'mcp')) {
    parseWireMcpAuthTarget(record.mcp, `${path}.mcp`);
    parseLiteral(record.disposition, ['started', 'joined'], `${path}.disposition`, 'MCP login disposition');
    return value as WireMcpLoginStart;
  }
  parseWireOAuthProvider(record.provider, `${path}.provider`);
  return value as WireProviderLoginStart;
}

export function parseWireProviderLoginReady(
  value: unknown,
  path = 'login_ready',
): WireProviderLoginReady {
  const record = expectRecord(value, path);
  if (hasOwn(record, 'state') && record.state !== null && record.state !== undefined) {
    parseLiteral(record.state, [WIRE_LOGIN_READY_STATE], `${path}.state`, 'wire login ready state');
  }
  validateBindingIdentity(record, path);
  expectString(record.profile_id, `${path}.profile_id`);
  parseWireOAuthProvider(record.provider, `${path}.provider`);
  optionalString(record, 'expires_at', `${path}.expires_at`);
  expectBoolean(record.has_refresh_token, `${path}.has_refresh_token`);
  expectStringArray(record.scopes, `${path}.scopes`);
  return value as WireProviderLoginReady;
}

export function parseWireLoginReady(value: unknown, path = 'login_ready'): WireLoginReady {
  const record = expectRecord(value, path);
  if (!hasOwn(record, 'mcp')) {
    return parseWireProviderLoginReady(value, path);
  }
  parseWireMcpAuthTarget(record.mcp, `${path}.mcp`);
  optionalString(record, 'account_id', `${path}.account_id`);
  optionalString(record, 'expires_at', `${path}.expires_at`);
  expectBoolean(record.has_refresh_token, `${path}.has_refresh_token`);
  expectStringArray(record.scopes, `${path}.scopes`);
  return value as WireMcpLoginReady;
}

export function parseLoginCancelParams(params: LoginCancelParams): LoginCancelParams {
  const record = expectRecord(params, 'login_cancel.params');
  parseWireMcpAuthTarget(record.mcp, 'login_cancel.params.mcp');
  if (hasOwn(record, 'attempt_ref')) {
    if (hasOwn(record, 'state')) {
      fail('login_cancel.params', 'either state or attempt_ref, not both');
    }
    expectString(record.attempt_ref, 'login_cancel.params.attempt_ref');
    return params;
  }
  expectString(record.state, 'login_cancel.params.state');
  return params;
}

export function parseWireLoginCancelled(value: unknown, path = 'login_cancelled'): WireLoginCancelled {
  const record = expectRecord(value, path);
  parseWireMcpAuthTarget(record.mcp, `${path}.mcp`);
  expectBoolean(record.cancelled, `${path}.cancelled`);
  return value as WireLoginCancelled;
}

export function parseWireMcpAuthAttempt(value: unknown, path = 'mcp_auth_attempt'): WireMcpAuthAttempt {
  const record = expectRecord(value, path);
  expectString(record.ref, `${path}.ref`);
  parseLiteral(record.phase, ['pending'], `${path}.phase`, 'MCP auth attempt phase');
  expectString(record.expires_at, `${path}.expires_at`);
  return value as WireMcpAuthAttempt;
}

export function parseWireMcpAuthStatus(value: unknown, path = 'mcp_auth_status'): WireMcpAuthStatus {
  const record = expectRecord(value, path);
  parseWireMcpAuthTarget(record.mcp, `${path}.mcp`);
  parseLiteral(
    record.phase,
    ['authorized', 'reauth_required', 'authorization_required'],
    `${path}.phase`,
    'MCP auth phase',
  );
  optionalString(record, 'expires_at', `${path}.expires_at`);
  optionalString(record, 'account_id', `${path}.account_id`);
  if (hasOwn(record, 'attempt') && record.attempt !== null && record.attempt !== undefined) {
    parseWireMcpAuthAttempt(record.attempt, `${path}.attempt`);
  }
  return value as WireMcpAuthStatus;
}

export function parseWireMcpLoggedOut(value: unknown, path = 'mcp_logged_out'): WireMcpLoggedOut {
  const record = expectRecord(value, path);
  parseWireMcpAuthTarget(record.mcp, `${path}.mcp`);
  expectBoolean(record.cleared, `${path}.cleared`);
  return value as WireMcpLoggedOut;
}

export function parseWireDeviceStart(value: unknown, path = 'device_start'): WireDeviceStart {
  const record = expectRecord(value, path);
  expectString(record.device_code, `${path}.device_code`);
  expectString(record.user_code, `${path}.user_code`);
  expectString(record.verification_uri, `${path}.verification_uri`);
  optionalString(record, 'verification_uri_complete', `${path}.verification_uri_complete`);
  expectNumber(record.expires_in, `${path}.expires_in`);
  expectNumber(record.interval, `${path}.interval`);
  parseWireOAuthProvider(record.provider, `${path}.provider`);
  return value as WireDeviceStart;
}

export function parseWireDeviceCompleteResult(
  value: unknown,
  path = 'device_complete',
): WireDeviceCompleteResult {
  const record = expectRecord(value, path);
  const state = parseLiteral(record.state, WIRE_DEVICE_COMPLETE_STATES, `${path}.state`, 'wire device complete state');
  if (state === WIRE_LOGIN_READY_STATE) {
    parseWireProviderLoginReady(value, path);
  }
  return value as WireDeviceCompleteResult;
}

export function parseWireProvisionApiKeyResult(
  value: unknown,
  path = 'provision_api_key',
): WireProvisionApiKeyResult {
  const record = expectRecord(value, path);
  validateBindingIdentity(record, path);
  expectString(record.profile_id, `${path}.profile_id`);
  const provider = parseWireAuthProvider(record.provider, `${path}.provider`);
  const authMode = parseWireAuthMethod(record.auth_mode, `${path}.auth_mode`);
  validateProviderAuthMethod(provider, authMode, `${path}.auth_mode`);
  expectBoolean(record.has_api_key, `${path}.has_api_key`);
  expectStringArray(record.scopes, `${path}.scopes`);
  return value as WireProvisionApiKeyResult;
}

export function parseWireAuthProfilesList(
  value: unknown,
  path = 'auth_profiles_list',
): WireAuthProfilesList {
  const record = expectRecord(value, path);
  expectString(record.realm_id, `${path}.realm_id`);
  if (!Array.isArray(record.auth_profiles)) fail(`${path}.auth_profiles`, 'array');
  const authProfiles = record.auth_profiles.map((entry, index) => parseWireAuthProfile(entry, `${path}.auth_profiles[${index}]`));
  if (!Array.isArray(record.backend_profiles)) fail(`${path}.backend_profiles`, 'array');
  const backendProfiles = record.backend_profiles.map((entry, index) => parseWireBackendProfile(entry, `${path}.backend_profiles[${index}]`));
  if (!Array.isArray(record.bindings)) fail(`${path}.bindings`, 'array');
  const bindings = record.bindings.map((entry, index) => [
    parseWireProviderBinding(entry, `${path}.bindings[${index}]`),
    `${path}.bindings[${index}]`,
  ] as const);
  validateWireProviderBindings(
    bindings,
    indexWireProfiles(backendProfiles, `${path}.backend_profiles`, 'backend profile'),
    indexWireProfiles(authProfiles, `${path}.auth_profiles`, 'auth profile'),
  );
  return value as WireAuthProfilesList;
}

export function parseWireAuthError(value: unknown, path = 'auth_error'): WireAuthError {
  const record = expectRecord(value, path);
  parseLiteral(
    record.kind,
    [
      'missing_secret',
      'unsupported_combination',
      'missing_required_metadata',
      'workspace_mismatch',
      'expired',
      'stale_credential',
      'refresh_required',
      'lease_absent',
      'user_reauth_required',
      'refresh_failed',
      'interactive_login_required',
      'host_owned_unavailable',
      'io',
      'other',
    ],
    `${path}.kind`,
    'wire auth error kind',
  );
  return value as WireAuthError;
}

export function parseWireAuthStatus(value: unknown, path = 'auth_status'): WireAuthStatus {
  const record = expectRecord(value, path);
  expectString(record.profile_id, `${path}.profile_id`);
  const provider = parseWireAuthProvider(record.provider, `${path}.provider`);
  const authMethod = parseWireAuthMethod(record.auth_method, `${path}.auth_method`);
  validateProviderAuthMethod(provider, authMethod, `${path}.auth_method`);
  parseWireAuthStatusState(record.state, `${path}.state`);
  optionalString(record, 'expires_at', `${path}.expires_at`);
  optionalString(record, 'last_refresh_at', `${path}.last_refresh_at`);
  optionalString(record, 'account_id', `${path}.account_id`);
  if (hasOwn(record, 'last_error') && record.last_error !== null && record.last_error !== undefined) {
    parseWireAuthError(record.last_error, `${path}.last_error`);
  }
  return value as WireAuthStatus;
}

export function parseWireAuthStatusDetail(
  value: unknown,
  path = 'auth_status_detail',
): WireAuthStatusDetail {
  const record = expectRecord(value, path);
  validateBindingIdentity(record, path);
  expectString(record.profile_id, `${path}.profile_id`);
  const provider = parseWireAuthProvider(record.provider, `${path}.provider`);
  const authMethod = parseWireAuthMethod(record.auth_method, `${path}.auth_method`);
  validateProviderAuthMethod(provider, authMethod, `${path}.auth_method`);
  parseWireAuthStatusState(record.state, `${path}.state`);
  optionalString(record, 'expires_at', `${path}.expires_at`);
  optionalString(record, 'last_refresh_at', `${path}.last_refresh_at`);
  optionalString(record, 'account_id', `${path}.account_id`);
  expectBoolean(record.has_refresh_token, `${path}.has_refresh_token`);
  return value as WireAuthStatusDetail;
}

export function parseCreateProfileParams(params: CreateProfileParams): CreateProfileParams {
  const record = expectRecord(params, 'create_profile.params');
  expectString(record.realm_id, 'create_profile.params.realm_id');
  expectString(record.binding_id, 'create_profile.params.binding_id');
  optionalString(record, 'profile_id', 'create_profile.params.profile_id');
  parseWireAuthMethod(record.auth_method, 'create_profile.params.auth_method');
  expectString(record.secret, 'create_profile.params.secret');
  return params;
}

export function parseLoginStartParams(params: LoginStartParams): LoginStartParams {
  const record = expectRecord(params, 'login_start.params');
  if (hasOwn(record, 'mcp')) {
    rejectProviderTargetFields(record, 'login_start.params');
    parseWireMcpAuthTarget(record.mcp, 'login_start.params.mcp');
    expectString(record.redirect_uri, 'login_start.params.redirect_uri');
    return params;
  }
  parseWireOAuthProvider(record.provider, 'login_start.params.provider');
  expectString(record.redirect_uri, 'login_start.params.redirect_uri');
  expectString(record.realm_id, 'login_start.params.realm_id');
  expectString(record.binding_id, 'login_start.params.binding_id');
  optionalString(record, 'profile_id', 'login_start.params.profile_id');
  return params;
}

function rejectProviderTargetFields(record: Record<string, unknown>, path: string): void {
  for (const field of ['provider', 'realm_id', 'binding_id', 'profile_id']) {
    if (hasOwn(record, field)) {
      throw new WebAuthContractError(`${path}.${field}: not allowed with an mcp target`);
    }
  }
}

export function parseLoginCompleteParams(params: LoginCompleteParams): LoginCompleteParams {
  const record = expectRecord(params, 'login_complete.params');
  expectString(record.code, 'login_complete.params.code');
  expectString(record.state, 'login_complete.params.state');
  expectString(record.redirect_uri, 'login_complete.params.redirect_uri');
  if (hasOwn(record, 'mcp')) {
    rejectProviderTargetFields(record, 'login_complete.params');
    parseWireMcpAuthTarget(record.mcp, 'login_complete.params.mcp');
    return params;
  }
  parseWireOAuthProvider(record.provider, 'login_complete.params.provider');
  expectString(record.code, 'login_complete.params.code');
  expectString(record.state, 'login_complete.params.state');
  expectString(record.redirect_uri, 'login_complete.params.redirect_uri');
  expectString(record.realm_id, 'login_complete.params.realm_id');
  expectString(record.binding_id, 'login_complete.params.binding_id');
  optionalString(record, 'profile_id', 'login_complete.params.profile_id');
  return params;
}

export function parseDeviceStartParams(params: DeviceStartParams): DeviceStartParams {
  const record = expectRecord(params, 'device_start.params');
  parseWireOAuthProvider(record.provider, 'device_start.params.provider');
  expectString(record.realm_id, 'device_start.params.realm_id');
  expectString(record.binding_id, 'device_start.params.binding_id');
  optionalString(record, 'profile_id', 'device_start.params.profile_id');
  return params;
}

export function parseDeviceCompleteParams(
  params: DeviceCompleteParams,
): DeviceCompleteParams {
  const record = expectRecord(params, 'device_complete.params');
  parseWireOAuthProvider(record.provider, 'device_complete.params.provider');
  expectString(record.device_code, 'device_complete.params.device_code');
  expectString(record.realm_id, 'device_complete.params.realm_id');
  expectString(record.binding_id, 'device_complete.params.binding_id');
  optionalString(record, 'profile_id', 'device_complete.params.profile_id');
  return params;
}

export function parseProvisionApiKeyParams(
  params: ProvisionApiKeyParams,
): ProvisionApiKeyParams {
  const record = expectRecord(params, 'provision_api_key.params');
  expectString(record.access_token, 'provision_api_key.params.access_token');
  optionalString(record, 'realm_id', 'provision_api_key.params.realm_id');
  optionalString(record, 'binding_id', 'provision_api_key.params.binding_id');
  optionalString(record, 'profile_id', 'provision_api_key.params.profile_id');
  return params;
}
