/**
 * A complete `MetaBuilderConfig` for tests that construct `MetaRuntime`
 * directly: every field at `MetaBuilder`'s default, then the test's own
 * values. A config field added to the SDK fails the tests type-check here
 * once, instead of in every test that builds a runtime by hand.
 */
import type { MetaBuilderConfig } from "../src/builder.js";

// Plain default values, checked complete against the SDK's config type.
// Without overrides they also fit the same type compiled into dist, whose
// classes are separate declarations.
const DEFAULTS = {
  mobConfigPath: null,
  preparedApplication: null,
  declareSpecUpdateExpectedRevision: null,
  sessionBuilder: null,
  errorCallback: null,
  eventLog: null,
  runtimeStore: null,
  consoleConfigPath: null,
  accessConfigPath: null,
  meerkatConfigPath: null,
  providerCredentialsRoot: null,
  httpListen: null,
  httpPublicBaseUrl: null,
  allowRemote: null,
  consoleRequireAppAuth: null,
  consoleReadOnly: null,
  consoleFetchTimeoutMs: null,
  demoLlm: false,
  memberCommsAddress: null,
  gatingConfigPath: null,
  routingConfigPath: null,
  workgraphEnabled: null,
  forkInstructionPolicy: null,
  memoryConfig: null,
  agentMemoryConfig: null,
  authConfig: null,
  implicitDelegateIdleRetireSecs: undefined,
  maxSessions: null,
  gatewayTimeoutMs: null,
  gatewayBin: null,
  initDeadlineMs: null,
  shutdownInFlightDrainMs: null,
  modules: [],
  persistentState: null,
  rosterProvider: null,
  agentCustomizer: null,
  topologyProvider: null,
  jobCredentialResolver: null,
  experimentalLiveConfig: null,
  openaiLiveConfig: null,
  consoleVoiceConfig: null,
} satisfies MetaBuilderConfig;

export function runtimeConfig(): typeof DEFAULTS;
export function runtimeConfig(overrides: Partial<MetaBuilderConfig>): MetaBuilderConfig;
export function runtimeConfig(overrides: Partial<MetaBuilderConfig> = {}): MetaBuilderConfig {
  return { ...DEFAULTS, ...overrides };
}
