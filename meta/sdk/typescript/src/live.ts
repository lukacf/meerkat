/** Versioned live channel contracts shared with the Meta wire boundary. */

export const LIVE_EXECUTION_IDENTITY_V1 = "live.execution_identity.v1" as const;
export const LIVE_EXECUTION_FUNCTION_BRIDGE_V1 =
  "live.execution.function_bridge.v1" as const;
export const LIVE_EXECUTION_CLIENT_CONTEXT_V1 =
  "live.execution.client_context.v1" as const;

export type LiveExecutionMode = "function_bridge" | "client_context";

const EXECUTION_MODE_CAPABILITIES: Readonly<Record<LiveExecutionMode, string>> = {
  function_bridge: LIVE_EXECUTION_FUNCTION_BRIDGE_V1,
  client_context: LIVE_EXECUTION_CLIENT_CONTEXT_V1,
};
/**
 * Public Live (`gpt-live-1`) client-context execution profile served by a
 * gateway registered through `runtime_options.openai_live`. Callers select it
 * through `live/open { execution_identity: { version: "v1", profile_id } }`.
 */
export const OPENAI_GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID =
  "openai.gpt-live-1.client-context.v1" as const;
const RESERVED_GPT_LIVE_PROFILE_IDS = new Set([
  "openai.gpt-live-1-codex.client-context.v1",
  "openai.gpt-live-1-codex.function-bridge.v1",
  OPENAI_GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID,
]);

export interface LiveAuthBindingRef {
  readonly realm: string;
  readonly binding: string;
  readonly profile?: string;
}

/**
 * Public OpenAI Live (`gpt-live-1`) host registration
 * (`runtime_options.openai_live` or `runtime_options.console_voice`, selected
 * by the builder method). The gateway fixes the execution identity
 * (provider OpenAI, model `gpt-live-1`, this configured API-key binding); the
 * host names only the principal, realm, binding, voice, and optional trusted
 * session instructions. Requires a gateway compiled with `openai-live`.
 */
export interface OpenAiLiveGatewayConfig {
  readonly principal: string;
  readonly realm: string;
  readonly authBinding: LiveAuthBindingRef;
  readonly voice: string;
  readonly sessionInstructions?: string;
  /** Console voice context summary bounds; every field optional (gateway defaults apply). */
  readonly summary?: OpenAiLiveSummaryConfig;
}

/**
 * Bounds for the console voice context summary (`console_voice.summary`).
 * `maxInputBytes` is the most recent slice of the serialized transcript handed
 * to the summariser (gateway default 65536), `maxOutputBytes` the UTF-8 cap on
 * the produced summary (gateway default 4096), and `model` the summary model on
 * the agent's provider and credentials (default: the agent's own model).
 */
export interface OpenAiLiveSummaryConfig {
  readonly model?: string;
  readonly maxInputBytes?: number;
  readonly maxOutputBytes?: number;
}

/** DEPRECATED private ChatGPT-brokered registration; prefer {@link OpenAiLiveGatewayConfig}. */
export interface ExperimentalLiveGatewayConfig {
  readonly principal: string;
  readonly realm: string;
  readonly factoryKind: string;
  readonly factoryVersion: string;
  readonly gate0Qualification: string;
  readonly authBinding: LiveAuthBindingRef;
  readonly voice: string;
  readonly executionProfiles?: readonly ExperimentalLiveExecutionProfileConfig[];
}

export interface ExperimentalLiveExecutionProfileConfig {
  readonly profileId: string;
  readonly sessionInstructions: string;
}

function liveAuthBindingToWire(
  realm: string,
  authBinding: LiveAuthBindingRef,
  context: string,
): Record<string, unknown> {
  const bindingRealm = requireString(authBinding.realm, `${context} authBinding.realm`);
  if (bindingRealm !== realm) {
    throw new TypeError(`${context} auth binding realm must equal realm`);
  }
  const wire: Record<string, unknown> = {
    realm: bindingRealm,
    binding: requireString(authBinding.binding, `${context} authBinding.binding`),
  };
  if (authBinding.profile !== undefined) {
    wire.profile = requireString(authBinding.profile, `${context} authBinding.profile`);
  }
  return wire;
}

export function openAiLiveGatewayConfigToWire(
  config: OpenAiLiveGatewayConfig,
): Record<string, unknown> {
  assertExactKeys(
    asRecord(config, "openai live config"),
    ["principal", "realm", "authBinding", "voice", "sessionInstructions", "summary"],
    "openai live config",
  );
  const principal = requireString(config.principal, "openai live principal");
  const realm = requireString(config.realm, "openai live realm");
  const voice = requireString(config.voice, "openai live voice");
  const result: Record<string, unknown> = {
    principal,
    realm,
    auth_binding: liveAuthBindingToWire(realm, config.authBinding, "openai live"),
    voice,
  };
  if (config.sessionInstructions !== undefined) {
    result.session_instructions = requireString(
      config.sessionInstructions,
      "openai live sessionInstructions",
    );
  }
  if (config.summary !== undefined) {
    result.summary = openAiLiveSummaryConfigToWire(config.summary);
  }
  return result;
}

function requirePositiveInteger(value: unknown, context: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value <= 0) {
    throw new TypeError(`${context} must be a positive integer`);
  }
  return value;
}

export function openAiLiveSummaryConfigToWire(
  summary: OpenAiLiveSummaryConfig,
): Record<string, unknown> {
  const record = asRecord(summary, "openai live summary");
  assertExactKeys(record, ["model", "maxInputBytes", "maxOutputBytes"], "openai live summary");
  const wire: Record<string, unknown> = {};
  if (record.model !== undefined) {
    wire.model = requireString(record.model, "openai live summary.model");
  }
  if (record.maxInputBytes !== undefined) {
    wire.max_input_bytes = requirePositiveInteger(
      record.maxInputBytes,
      "openai live summary.maxInputBytes",
    );
  }
  if (record.maxOutputBytes !== undefined) {
    wire.max_output_bytes = requirePositiveInteger(
      record.maxOutputBytes,
      "openai live summary.maxOutputBytes",
    );
  }
  return wire;
}

export function experimentalLiveGatewayConfigToWire(
  config: ExperimentalLiveGatewayConfig,
): Record<string, unknown> {
  assertExactKeys(
    asRecord(config, "experimental live config"),
    [
      "principal",
      "realm",
      "factoryKind",
      "factoryVersion",
      "gate0Qualification",
      "authBinding",
      "voice",
      "executionProfiles",
    ],
    "experimental live config",
  );
  const principal = requireString(config.principal, "experimental live principal");
  const realm = requireString(config.realm, "experimental live realm");
  const factoryKind = requireString(
    config.factoryKind,
    "experimental live factoryKind",
  );
  const factoryVersion = requireString(
    config.factoryVersion,
    "experimental live factoryVersion",
  );
  const gate0Qualification = requireString(
    config.gate0Qualification,
    "experimental live gate0Qualification",
  );
  const voice = requireString(config.voice, "experimental live voice");
  const authBinding = liveAuthBindingToWire(realm, config.authBinding, "experimental live");
  const result: Record<string, unknown> = {
    principal,
    realm,
    factory_kind: factoryKind,
    factory_version: factoryVersion,
    gate0_qualification: gate0Qualification,
    auth_binding: authBinding,
    voice,
  };
  if (config.executionProfiles !== undefined) {
    if (!Array.isArray(config.executionProfiles)) {
      throw new TypeError("experimental live executionProfiles must be an array");
    }
    const seen = new Set<string>();
    const executionProfiles = config.executionProfiles.map((profile, index) => {
      const profileRecord = asRecord(
        profile,
        `experimental live executionProfiles[${index}]`,
      );
      assertExactKeys(
        profileRecord,
        ["profileId", "sessionInstructions"],
        `experimental live executionProfiles[${index}]`,
      );
      const profileId = requireString(
        profileRecord.profileId,
        `experimental live executionProfiles[${index}].profileId`,
      ).trim();
      if (RESERVED_GPT_LIVE_PROFILE_IDS.has(profileId)) {
        throw new TypeError(
          `experimental live executionProfiles[${index}].profileId is reserved`,
        );
      }
      if (seen.has(profileId)) {
        throw new TypeError(
          `experimental live executionProfiles contains duplicate profileId ${profileId}`,
        );
      }
      seen.add(profileId);
      return {
        profile_id: profileId,
        session_instructions: requireString(
          profileRecord.sessionInstructions,
          `experimental live executionProfiles[${index}].sessionInstructions`,
        ).trim(),
      };
    });
    if (executionProfiles.length > 0) {
      result.execution_profiles = executionProfiles;
    }
  }
  return result;
}

export interface LiveExecutionIdentityV1 {
  readonly profileId: string;
}

export interface LiveExecutionIdentityWireV1 {
  readonly version: "v1";
  readonly profile_id: string;
}

export type LiveTransportBootstrap =
  | { readonly transport: "websocket"; readonly url: string; readonly token: string }
  | {
      readonly transport: "webrtc";
      readonly token: string;
      readonly answerMethod: string;
      readonly httpUrl?: string;
    }
  | { readonly transport: "unknown"; readonly debug: string };

export interface LiveChannelCapabilities {
  readonly audioIn: boolean;
  readonly audioOut: boolean;
  readonly textIn: boolean;
  readonly textOut: boolean;
  readonly imageIn: boolean;
  readonly videoIn: boolean;
  readonly transcriptSupported: boolean;
  readonly bargeInSupported: boolean;
  readonly providerNativeResume: boolean;
}

export type LiveContinuityMode =
  | { readonly mode: "fresh" | "transcript_only" | "degraded" }
  | { readonly mode: "provider_native_resume"; readonly providerSessionId: string }
  | { readonly mode: "unknown"; readonly debug: string };

export interface LiveChannelHandle {
  readonly channelId: string;
  readonly targetIdentity: string;
  readonly transport: LiveTransportBootstrap;
  readonly capabilities: LiveChannelCapabilities;
  readonly continuity: LiveContinuityMode;
  /**
   * What the open went ahead without, present only when the server reported
   * something: `"staged_prompt_refused"` means a run-start hook refused the
   * session's staged create-time prompt, which stays staged for its first
   * text turn. The vocabulary may grow; an unknown advisory is kept as the
   * string the server sent.
   */
  readonly advisories?: readonly string[];
}

export interface PendingLiveChannelHandle {
  readonly channelId: string;
  readonly targetIdentity: string;
  readonly executionMode: LiveExecutionMode;
  readonly pendingReceipt: string;
  readonly transport: LiveTransportBootstrap;
  readonly capabilities: LiveChannelCapabilities;
  readonly continuity: LiveContinuityMode;
  /** What the open went ahead without, as on {@link LiveChannelHandle}. */
  readonly advisories?: readonly string[];
}

export interface ActiveLiveChannelHandle {
  readonly channelId: string;
  readonly targetIdentity: string;
  readonly executionMode: LiveExecutionMode;
  readonly activationReceipt: string;
}

export interface ActiveLiveChannelConnection extends ActiveLiveChannelHandle {
  readonly pendingReceipt: string;
  readonly readinessReceipt: string;
  /** Revoke active authority after local media-owner loss. Idempotent locally. */
  ownerLost(): Promise<ExperimentalLiveChannelStatus>;
  /** Explicit lifecycle teardown, equivalent to ownerLost. */
  dispose(): Promise<ExperimentalLiveChannelStatus>;
}

export interface LivePlaybackOwnerReadiness {
  readonly channelId: string;
  readonly readinessReceipt: string;
}

export type ExperimentalLiveChannelStatus =
  | { readonly phase: "pending" }
  | { readonly phase: "active"; readonly handle: ActiveLiveChannelHandle }
  | { readonly phase: "revoked" }
  | { readonly phase: "closed" };

export interface LivePlaybackOwner {
  /** Install the output consumer and media gates before producing an offer. */
  prepare(pending: PendingLiveChannelHandle): Promise<string>;
  /** Apply the remote answer while microphone and remote audio remain gated. */
  acceptAnswer(answerSdp: string): Promise<void>;
  /** Release the media gates only after generated active authority exists. */
  activate(active: ActiveLiveChannelHandle): Promise<void>;
  /** Tear down the local peer and all gates after any failed activation. */
  abort(): Promise<void>;
  /** Optional transport-loss signal supervised by the returned connection. */
  waitForLoss?(): Promise<void>;
}

export type LiveReplacementRequired =
  | { readonly required: false }
  | {
      readonly required: true;
      readonly reason: "canonical_context" | "delegation_result";
      readonly replacement: LiveChannelHandle;
      readonly canonicalSeedCursor: number;
    };

/** Opaque channel-scoped address published before assistant playback. */
export interface LiveAssistantOutputAddress {
  readonly channelId: string;
  readonly outputId: string;
  readonly contentIndex: number;
}

export interface LivePlaybackCompleteResult {
  readonly status: "completed";
}

function asRecord(value: unknown, context: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TypeError(`${context} must be an object`);
  }
  return value as Record<string, unknown>;
}

function assertExactKeys(
  value: Record<string, unknown>,
  allowed: readonly string[],
  context: string,
): void {
  const unknown = Object.keys(value).find((key) => !allowed.includes(key));
  if (unknown !== undefined) {
    throw new TypeError(`${context} contains unknown field ${unknown}`);
  }
}

function requireString(value: unknown, context: string): string {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new TypeError(`${context} must be a non-empty string`);
  }
  return value;
}

function requireBoolean(value: unknown, context: string): boolean {
  if (typeof value !== "boolean") {
    throw new TypeError(`${context} must be a boolean`);
  }
  return value;
}

/**
 * What a successful open went ahead without. The server omits the field when
 * there is nothing to report. The vocabulary may grow: an advisory this SDK
 * does not know is kept as the string the server sent, so a newer server never
 * fails the open; a field that is not an array of non-empty strings fails
 * closed.
 */
function parseAdvisories(
  raw: unknown,
  context: string,
): { readonly advisories?: readonly string[] } {
  if (raw === undefined) return {};
  if (!Array.isArray(raw)) {
    throw new TypeError(`${context}.advisories must be an array`);
  }
  if (raw.length === 0) return {};
  return {
    advisories: raw.map((value) => requireString(value, `${context}.advisories`)),
  };
}

/** Serialize the strict v1 envelope to canonical snake-case wire JSON. */
export function liveExecutionIdentityV1ToWire(
  input: LiveExecutionIdentityV1,
): LiveExecutionIdentityWireV1 {
  const raw = asRecord(input, "execution identity");
  assertExactKeys(raw, ["profileId"], "execution identity");
  return {
    version: "v1",
    profile_id: requireString(raw.profileId, "profileId"),
  };
}

/** Reject ambiguous mixing of the new envelope with legacy top-level fields. */
export function liveOpenExecutionIdentityParams(
  executionIdentity: LiveExecutionIdentityV1,
  legacy?: {
    readonly model?: unknown;
    readonly provider?: unknown;
    readonly auth_binding?: unknown;
    readonly self_hosted_server_id?: unknown;
    readonly provider_params?: unknown;
  },
): { readonly execution_identity: LiveExecutionIdentityWireV1 } {
  for (const field of [
    "model",
    "provider",
    "auth_binding",
    "self_hosted_server_id",
    "provider_params",
  ] as const) {
    if (legacy !== undefined && Object.prototype.hasOwnProperty.call(legacy, field)) {
      throw new TypeError(
        `executionIdentity conflicts with legacy top-level ${field}`,
      );
    }
  }
  return { execution_identity: liveExecutionIdentityV1ToWire(executionIdentity) };
}

export function supportsLiveExecutionIdentityV1(capabilities: {
  readonly featureCapabilities: readonly string[];
}): boolean {
  return capabilities.featureCapabilities.includes(LIVE_EXECUTION_IDENTITY_V1);
}

export function liveExecutionModeCapability(mode: LiveExecutionMode): string {
  const capability = EXECUTION_MODE_CAPABILITIES[mode];
  if (capability === undefined) {
    throw new TypeError("unknown provider-neutral live execution mode");
  }
  return capability;
}

export function supportsLiveExecutionMode(
  capabilities: { readonly featureCapabilities: readonly string[] },
  mode: LiveExecutionMode,
): boolean {
  return capabilities.featureCapabilities.includes(
    liveExecutionModeCapability(mode),
  );
}

/** Parse a canonical wire handle and reject drift in the typed top-level shape. */
export function parseLiveChannelHandle(raw: unknown): LiveChannelHandle {
  const d = asRecord(raw, "live channel handle");
  assertExactKeys(
    d,
    [
      "channel_id",
      "target_identity",
      "transport",
      "capabilities",
      "continuity",
      "advisories",
    ],
    "live channel handle",
  );
  const transport = parseTransport(d.transport);
  const capabilities = parseCapabilities(d.capabilities);
  const continuity = parseContinuity(d.continuity);
  return {
    channelId: requireString(d.channel_id, "channel_id"),
    targetIdentity: requireString(d.target_identity, "target_identity"),
    transport,
    capabilities,
    continuity,
    ...parseAdvisories(d.advisories, "live channel handle"),
  };
}

function parseExecutionMode(raw: unknown, context: string): LiveExecutionMode {
  const mode = requireString(raw, `${context}.execution_mode`);
  if (mode !== "function_bridge" && mode !== "client_context") {
    throw new TypeError(`${context}.execution_mode is unknown`);
  }
  return mode;
}

export function parsePendingLiveChannelHandle(
  raw: unknown,
): PendingLiveChannelHandle {
  const d = asRecord(raw, "pending live channel handle");
  assertExactKeys(
    d,
    [
      "channel_id",
      "target_identity",
      "execution_mode",
      "pending_receipt",
      "transport",
      "capabilities",
      "continuity",
      "advisories",
    ],
    "pending live channel handle",
  );
  return {
    channelId: requireString(d.channel_id, "pending live channel handle.channel_id"),
    targetIdentity: requireString(
      d.target_identity,
      "pending live channel handle.target_identity",
    ),
    executionMode: parseExecutionMode(d.execution_mode, "pending live channel handle"),
    pendingReceipt: requireString(
      d.pending_receipt,
      "pending live channel handle.pending_receipt",
    ),
    transport: parseTransport(d.transport),
    capabilities: parseCapabilities(d.capabilities),
    continuity: parseContinuity(d.continuity),
    ...parseAdvisories(d.advisories, "pending live channel handle"),
  };
}

export function parseActiveLiveChannelHandle(
  raw: unknown,
): ActiveLiveChannelHandle {
  const d = asRecord(raw, "active live channel handle");
  assertExactKeys(
    d,
    ["channel_id", "target_identity", "execution_mode", "activation_receipt"],
    "active live channel handle",
  );
  return {
    channelId: requireString(d.channel_id, "active live channel handle.channel_id"),
    targetIdentity: requireString(
      d.target_identity,
      "active live channel handle.target_identity",
    ),
    executionMode: parseExecutionMode(d.execution_mode, "active live channel handle"),
    activationReceipt: requireString(
      d.activation_receipt,
      "active live channel handle.activation_receipt",
    ),
  };
}

export function parseLivePlaybackOwnerReadiness(
  raw: unknown,
): LivePlaybackOwnerReadiness {
  const d = asRecord(raw, "playback owner readiness");
  assertExactKeys(d, ["channel_id", "readiness_receipt"], "playback owner readiness");
  return {
    channelId: requireString(d.channel_id, "playback owner readiness.channel_id"),
    readinessReceipt: requireString(
      d.readiness_receipt,
      "playback owner readiness.readiness_receipt",
    ),
  };
}

export function parseExperimentalLiveChannelStatus(
  raw: unknown,
): ExperimentalLiveChannelStatus {
  const d = asRecord(raw, "experimental live channel status");
  const phase = requireString(d.phase, "experimental live channel status.phase");
  if (phase === "pending" || phase === "revoked" || phase === "closed") {
    assertExactKeys(d, ["phase"], "experimental live channel status");
    return { phase };
  }
  if (phase === "active") {
    assertExactKeys(d, ["phase", "handle"], "experimental live channel status");
    return { phase, handle: parseActiveLiveChannelHandle(d.handle) };
  }
  throw new TypeError("experimental live channel status.phase is unknown");
}

export function pendingLiveChannelHandleToWire(
  handle: PendingLiveChannelHandle,
): Record<string, unknown> {
  liveExecutionModeCapability(handle.executionMode);
  return {
    ...liveChannelHandleToWire({
      channelId: handle.channelId,
      targetIdentity: handle.targetIdentity,
      transport: handle.transport,
      capabilities: handle.capabilities,
      continuity: handle.continuity,
      advisories: handle.advisories,
    }),
    execution_mode: handle.executionMode,
    pending_receipt: requireString(handle.pendingReceipt, "pendingReceipt"),
  };
}

export function activeLiveChannelHandleToWire(
  handle: ActiveLiveChannelHandle,
): Record<string, string> {
  liveExecutionModeCapability(handle.executionMode);
  return {
    channel_id: requireString(handle.channelId, "channelId"),
    target_identity: requireString(handle.targetIdentity, "targetIdentity"),
    execution_mode: handle.executionMode,
    activation_receipt: requireString(handle.activationReceipt, "activationReceipt"),
  };
}

/** Parse the strict one-shot replacement signaling result. */
export function parseLiveReplacementRequired(
  raw: unknown,
): LiveReplacementRequired {
  const d = asRecord(raw, "live replacement result");
  const required = requireBoolean(d.required, "live replacement result.required");
  if (!required) {
    assertExactKeys(d, ["required"], "live replacement result");
    return { required: false };
  }
  assertExactKeys(
    d,
    ["required", "reason", "replacement", "canonical_seed_cursor"],
    "live replacement result",
  );
  const reason = requireString(d.reason, "live replacement result.reason");
  if (reason !== "canonical_context" && reason !== "delegation_result") {
    throw new TypeError("live replacement result.reason is unknown");
  }
  if (
    typeof d.canonical_seed_cursor !== "number" ||
    !Number.isSafeInteger(d.canonical_seed_cursor) ||
    d.canonical_seed_cursor < 0
  ) {
    throw new TypeError(
      "live replacement result.canonical_seed_cursor must be a non-negative integer",
    );
  }
  return {
    required: true,
    reason,
    replacement: parseLiveChannelHandle(d.replacement),
    canonicalSeedCursor: d.canonical_seed_cursor,
  };
}

export function parseLivePlaybackCompleteResult(
  raw: unknown,
): LivePlaybackCompleteResult {
  const d = asRecord(raw, "live playback complete result");
  assertExactKeys(d, ["status"], "live playback complete result");
  if (d.status !== "completed") {
    throw new TypeError("live playback complete result.status must be completed");
  }
  return { status: "completed" };
}

export function parseLiveAssistantOutputAddress(
  raw: unknown,
): LiveAssistantOutputAddress {
  const d = asRecord(raw, "live assistant output address");
  assertExactKeys(
    d,
    ["channel_id", "output_id", "content_index"],
    "live assistant output address",
  );
  if (
    typeof d.content_index !== "number" ||
    !Number.isSafeInteger(d.content_index) ||
    d.content_index < 0
  ) {
    throw new TypeError(
      "live assistant output address.content_index must be a non-negative integer",
    );
  }
  return {
    channelId: requireString(d.channel_id, "channel_id"),
    outputId: requireString(d.output_id, "output_id"),
    contentIndex: d.content_index,
  };
}

/** Serialize a typed channel handle to canonical snake-case wire JSON. */
export function liveChannelHandleToWire(
  handle: LiveChannelHandle,
): Record<string, unknown> {
  const transport: Record<string, unknown> = { transport: handle.transport.transport };
  if (handle.transport.transport === "websocket") {
    transport.url = handle.transport.url;
    transport.token = handle.transport.token;
  } else if (handle.transport.transport === "webrtc") {
    transport.token = handle.transport.token;
    transport.answer_method = handle.transport.answerMethod;
    if (handle.transport.httpUrl !== undefined) transport.http_url = handle.transport.httpUrl;
  } else {
    transport.debug = handle.transport.debug;
  }

  const continuity: Record<string, unknown> = { mode: handle.continuity.mode };
  if (handle.continuity.mode === "provider_native_resume") {
    continuity.provider_session_id = handle.continuity.providerSessionId;
  } else if (handle.continuity.mode === "unknown") {
    continuity.debug = handle.continuity.debug;
  }

  return {
    channel_id: handle.channelId,
    target_identity: handle.targetIdentity,
    transport,
    capabilities: {
      audio_in: handle.capabilities.audioIn,
      audio_out: handle.capabilities.audioOut,
      text_in: handle.capabilities.textIn,
      text_out: handle.capabilities.textOut,
      image_in: handle.capabilities.imageIn,
      video_in: handle.capabilities.videoIn,
      transcript_supported: handle.capabilities.transcriptSupported,
      barge_in_supported: handle.capabilities.bargeInSupported,
      provider_native_resume: handle.capabilities.providerNativeResume,
    },
    continuity,
    ...(handle.advisories !== undefined && handle.advisories.length > 0
      ? { advisories: [...handle.advisories] }
      : {}),
  };
}

function parseTransport(raw: unknown): LiveTransportBootstrap {
  const d = asRecord(raw, "transport");
  const kind = requireString(d.transport, "transport.transport");
  if (kind === "websocket") {
    assertExactKeys(d, ["transport", "url", "token"], "websocket transport");
    return {
      transport: "websocket",
      url: requireString(d.url, "transport.url"),
      token: requireString(d.token, "transport.token"),
    };
  }
  if (kind === "webrtc") {
    assertExactKeys(d, ["transport", "token", "answer_method", "http_url"], "webrtc transport");
    const result: {
      transport: "webrtc";
      token: string;
      answerMethod: string;
      httpUrl?: string;
    } = {
      transport: "webrtc",
      token: requireString(d.token, "transport.token"),
      answerMethod: requireString(d.answer_method, "transport.answer_method"),
    };
    if (d.http_url !== undefined) result.httpUrl = requireString(d.http_url, "transport.http_url");
    return result;
  }
  if (kind === "unknown") {
    assertExactKeys(d, ["transport", "debug"], "unknown transport");
    return { transport: "unknown", debug: requireString(d.debug, "transport.debug") };
  }
  throw new TypeError(`unknown live transport ${kind}`);
}

function parseCapabilities(raw: unknown): LiveChannelCapabilities {
  const d = asRecord(raw, "capabilities");
  const keys = [
    "audio_in", "audio_out", "text_in", "text_out", "image_in", "video_in",
    "transcript_supported", "barge_in_supported", "provider_native_resume",
  ] as const;
  assertExactKeys(d, keys, "capabilities");
  return {
    audioIn: requireBoolean(d.audio_in, "capabilities.audio_in"),
    audioOut: requireBoolean(d.audio_out, "capabilities.audio_out"),
    textIn: requireBoolean(d.text_in, "capabilities.text_in"),
    textOut: requireBoolean(d.text_out, "capabilities.text_out"),
    imageIn: requireBoolean(d.image_in, "capabilities.image_in"),
    videoIn: requireBoolean(d.video_in, "capabilities.video_in"),
    transcriptSupported: requireBoolean(d.transcript_supported, "capabilities.transcript_supported"),
    bargeInSupported: requireBoolean(d.barge_in_supported, "capabilities.barge_in_supported"),
    providerNativeResume: requireBoolean(d.provider_native_resume, "capabilities.provider_native_resume"),
  };
}

function parseContinuity(raw: unknown): LiveContinuityMode {
  const d = asRecord(raw, "continuity");
  const mode = requireString(d.mode, "continuity.mode");
  if (mode === "fresh" || mode === "transcript_only" || mode === "degraded") {
    assertExactKeys(d, ["mode"], "continuity");
    return { mode };
  }
  if (mode === "provider_native_resume") {
    assertExactKeys(d, ["mode", "provider_session_id"], "continuity");
    return {
      mode,
      providerSessionId: requireString(d.provider_session_id, "continuity.provider_session_id"),
    };
  }
  if (mode === "unknown") {
    assertExactKeys(d, ["mode", "debug"], "continuity");
    return { mode, debug: requireString(d.debug, "continuity.debug") };
  }
  throw new TypeError(`unknown live continuity mode ${mode}`);
}
