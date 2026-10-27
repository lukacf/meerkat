/**
 * Meta builder chain — chainable configuration for the runtime.
 *
 * @example
 * ```ts
 * import { Meta } from "@rkat/meta-sdk";
 *
 * const rt = await Meta.builder()
 *   .mob("config/mob.toml")
 *   .sessionService(builder)
 *   .roster(myRoster)
 *   .build();
 *
 * const handle = rt.mobHandle();
 * const status = await handle.status();
 * ```
 */

import { existsSync, lstatSync } from "node:fs";
import type { SessionAgentBuilder, ErrorCallback } from "./agent-builder.js";
import { validateToolArtifactRef, type ToolArtifactRef } from "./models.js";
import type { MetaRuntime } from "./runtime.js";
import type { ExperimentalLiveGatewayConfig, OpenAiLiveGatewayConfig } from "./live.js";
import type { JobCredentialResolver } from "./jobs.js";
import type { GatewayTransportFactory } from "./transport.js";
import type {
  RosterProvider,
  AgentCustomizer,
  TopologyProvider,
} from "./types.js";

// -- agentMemory() key whitelists ------------------------------------------
// Mirror the gateway's supported-field lists (rpc_gateway.rs); the runtime
// checks exist so plain-JS callers fail loud on typos the way TS callers do
// at compile time.

const AGENT_MEMORY_KEYS = new Set([
  "enabled",
  "realm",
  "selection",
  "maxEntries",
  "recallTimeoutMs",
  "recallFailurePolicy",
  "instructionHeader",
  "perTurnInjection",
  "defangInbound",
  "store",
  "llmWrites",
  "recorderTool",
  "contentTrust",
  "selector",
  "operatorScope",
  "distiller",
  "steward",
  "hygienist",
]);

const CONTENT_TRUST_KEYS = new Set([
  "trustedMcpServers",
  "untrustedTools",
  "trustedTools",
]);

const DISTILLER_KEYS = new Set([
  "enabled",
  "runsPerHour",
  "minInteractions",
  "model",
]);

const STEWARD_KEYS = new Set([
  "enabled",
  "cadence",
  "model",
  "perMob",
  "runsPerDay",
  "minSignals",
  "maxOutputTokens",
  "max_output_tokens",
]);

const HYGIENIST_KEYS = new Set([
  "enabled",
  "runsPerDay",
  "model",
  "maxOutputTokens",
  "max_output_tokens",
]);

function rejectUnknownKeys(
  config: object,
  known: Set<string>,
  context: string,
): void {
  const unknown = Object.keys(config).filter((key) => !known.has(key)).sort();
  if (unknown.length > 0) {
    throw new Error(
      `${context} got unsupported option(s): ${unknown.join(", ")}`,
    );
  }
}

// -- Builder config -------------------------------------------------------

/** What a fork of a member with activated host instructions does with them. */
export type ForkInstructionPolicy =
  | "require_explicit_activation"
  | "reactivate_inherited"
  | "refuse";

export interface MetaBuilderConfig {
  mobConfigPath: string | null;
  /** Opaque prepared application bytes and their declared digest; transport only. */
  preparedApplication?: { bundle: Uint8Array; digest: string } | null;
  /**
   * Revision the stored mob definition must have for this boot's definition
   * to replace it (`runtime_options.declare_spec_update`); `null` declares
   * no update.
   */
  declareSpecUpdateExpectedRevision?: number | null;
  sessionBuilder: SessionAgentBuilder | null;
  errorCallback: ErrorCallback | null;
  eventLog: unknown;
  runtimeStore: unknown;
  consoleConfigPath: string | null;
  accessConfigPath: string | null;
  meerkatConfigPath: string | null;
  /** `runtime_options.provider_credentials_root`; absent or `null` sends nothing. */
  providerCredentialsRoot?: string | null;
  httpListen: string | null;
  httpPublicBaseUrl: string | null;
  allowRemote: boolean | null;
  consoleRequireAppAuth: boolean | null;
  consoleReadOnly: boolean | null;
  consoleFetchTimeoutMs: number | null;
  demoLlm: boolean;
  memberCommsAddress: string | null;
  gatingConfigPath: string | null;
  routingConfigPath: string | null;
  workgraphEnabled: boolean | string | null;
  /** `runtime_options.mob_announcements`, sent as given; absent or `null` sends nothing. */
  mobAnnouncements?: Record<string, unknown> | null;
  /** `runtime_options.fork_instruction_policy`; `null` sends nothing (the gateway default). */
  forkInstructionPolicy: ForkInstructionPolicy | null;
  memoryConfig: unknown;
  agentMemoryConfig: unknown;
  authConfig: unknown;
  implicitDelegateIdleRetireSecs: number | null | undefined;
  maxSessions: number | null;
  /** Artifacts this host installed; `null` selects none. */
  installedArtifacts?: ToolArtifactRef[] | null;
  gatewayTimeoutMs: number | null;
  gatewayBin: string | null;
  /** Builds the gateway transport on each connect; `null` uses the default. */
  gatewayTransportFactory?: GatewayTransportFactory | null;
  /**
   * Caller cap, in milliseconds, on waiting for an accepted `meta/init` to
   * settle. `null` waits until the gateway settles, exits, or the reader
   * fails. Running out throws `InitOutcomeUnknownError`.
   */
  initDeadlineMs: number | null;
  /** The `meta/init` parameter `shutdown_in_flight_drain_ms`; `null` sends nothing. */
  shutdownInFlightDrainMs: number | null;
  modules: unknown[];
  persistentState: string | null;
  rosterProvider: RosterProvider | null;
  agentCustomizer: AgentCustomizer | null;
  topologyProvider: TopologyProvider | null;
  jobCredentialResolver: JobCredentialResolver | null;
  experimentalLiveConfig: ExperimentalLiveGatewayConfig | null;
  openaiLiveConfig: OpenAiLiveGatewayConfig | null;
  consoleVoiceConfig?: OpenAiLiveGatewayConfig | null;
}

function defaultConfig(): MetaBuilderConfig {
  return {
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
  };
}

// -- MetaBuilder --------------------------------------------------------

/**
 * Chainable builder for Meta runtime configuration.
 *
 * @example
 * ```ts
 * const rt = await Meta.builder()
 *   .mob("config/mob.toml")
 *   .gateway("./target/release/rpc_gateway")
 *   .build();
 * ```
 */
export class MetaBuilder {
  /** @internal */
  readonly _config: MetaBuilderConfig = defaultConfig();

  mob(configPath: string): this {
    this._config.mobConfigPath = configPath;
    return this;
  }

  /**
   * Start from an authenticated prepared application instead of mob TOML.
   *
   * `bundle` is the prepared application's bytes exactly as prepared and
   * `digest` the digest declared for them. Both are transport only: the
   * gateway's native host composition authenticates them before any
   * definition, storage, identity or roster step, and refuses the start when
   * it cannot. Mutually exclusive with mob().
   */
  preparedApplication(bundle: Uint8Array, digest: string): this {
    if (!(bundle instanceof Uint8Array)) {
      throw new TypeError("preparedApplication bundle must be a Uint8Array");
    }
    if (typeof digest !== "string" || digest.length === 0) {
      throw new Error("preparedApplication digest must be a non-empty string");
    }
    this._config.preparedApplication = { bundle: Uint8Array.from(bundle), digest };
    return this;
  }

  /**
   * Apply this boot's mob definition as a one-shot revision-pinned update.
   *
   * The definition supplied by mob() or preparedApplication() is compared
   * and swapped against the canonical definition in persistentState().
   * `expectedRevision` must be the revision observed before constructing the
   * replacement definition. Exact replay converges; a stale revision or
   * different content at the successor revision raises the typed
   * `StorageResolutionError` during `build()`.
   *
   * Use this only on the activation that is authorized to change the
   * definition. Later ordinary restarts should omit the declaration.
   */
  declareSpecUpdate(expectedRevision: number): this {
    if (typeof expectedRevision !== "number" || !Number.isSafeInteger(expectedRevision)) {
      throw new TypeError("expectedRevision must be a non-negative integer");
    }
    if (expectedRevision < 0) {
      throw new RangeError("expectedRevision must be a non-negative integer");
    }
    this._config.declareSpecUpdateExpectedRevision = expectedRevision;
    return this;
  }

  /**
   * Register the session agent builder. A second argument (the session store
   * of earlier releases) is refused: nothing ever read it, and Core's own
   * session store holds every session.
   */
  sessionService(builder: SessionAgentBuilder, ...unsupported: never[]): this {
    if (unsupported.length > 0) {
      throw new TypeError(
        "sessionService() takes only a session builder; it accepts no session store",
      );
    }
    this._config.sessionBuilder = builder;
    return this;
  }

  /**
   * Configure the operational event log. Preferred form — a typed
   * declaration from the `eventLog` config module:
   *
   * ```ts
   * import { eventLog } from "@rkat/meta-sdk";
   * Meta.builder().eventLog(eventLog.memory({ batchSize: 64 }));
   * Meta.builder().eventLog(eventLog.nullStore());
   * ```
   *
   * The gateway accepts only the declared ephemeral storage kinds
   * (`"memory"` — bounded queryable in-process store; `"null"` — events
   * dropped) and rejects anything else at startup. Without this call no
   * events are ingested at all. The legacy raw form
   * `eventLog({ storage: ..., batch_size: ... })` keeps working.
   */
  eventLog(
    options: { toDict(): Record<string, unknown> } | { storage: unknown; [key: string]: unknown },
  ): this {
    this._config.eventLog = "toDict" in options ? options : { ...options };
    return this;
  }

  /**
   * Declare the runtime store's durability explicitly. The gateway's
   * runtime store (session resume/archive/retire) is persistent SQLite by
   * default; a failed open is a startup error, not a silent in-memory
   * fallback. The only declarable alternative is the explicit in-memory
   * form:
   *
   * ```ts
   * import { runtimeStore } from "@rkat/meta-sdk";
   * Meta.builder().runtimeStore(runtimeStore.memory());
   * ```
   *
   * Sessions then do not survive gateway restart, and the choice shows up
   * in the storage census (`meta/status` → `storage.slots`). Accepts the
   * typed config or the raw wire dict `{ storage: "memory" }`.
   */
  runtimeStore(
    config: { toDict(): Record<string, unknown> } | { storage: unknown },
  ): this {
    this._config.runtimeStore = config;
    return this;
  }

  consoleConfig(configPath: string): this {
    this._config.consoleConfigPath = configPath;
    return this;
  }

  /**
   * Enable ABAC access control backed by an existing TOML file
   * (conventionally `config/access.toml`). The gateway reads and validates
   * the file at startup: a missing path, a directory, or an unreadable or
   * invalid file refuses initialization, and an explicit empty path is
   * rejected here. Name the first admin in the file; a configuration without
   * admins grants administration to nobody. Console admin edits persist back
   * to the same path. Without this, access control stays off only when
   * nothing is at `config/access.toml` (a missing or dangling parent directory
   * counts as nothing there). Any other entry there (even a directory or a
   * dangling `access.toml` link) and any lookup error other than ENOENT is
   * forwarded to the gateway's strict loader, which refuses initialization
   * when it cannot open, read, parse or validate the file. The loader does
   * not check the file type, so a FIFO there blocks startup instead of
   * refusing it.
   */
  accessControl(configPath: string): this {
    if (configPath === "") {
      throw new Error(
        "accessControl requires a non-empty path: since Meta 0.9.0 an empty path no " +
          "longer switches access control off. To keep access control off, do not call " +
          "accessControl(), and keep nothing at config/access.toml under the working " +
          "directory, which the builder loads when present",
      );
    }
    this._config.accessConfigPath = configPath;
    return this;
  }

  /**
   * Load a meerkat host `config.toml` into every agent the gateway builds.
   *
   * Emitted as `runtime_options.meerkat_config_path`. This is the only door
   * for the tables meerkat keeps in its host config rather than in
   * `mob.toml`: `[self_hosted]` (servers and model aliases) and `[realm]`
   * (backend, auth and binding profiles). A host-config `[models]` table
   * reaches the factory too, but a profile's `model` must be catalogued,
   * provider-annotated or defined under `mob.toml`'s own `[models.<id>]` to
   * pass the init model check. The gateway reads the file once at init and
   * refuses a missing, malformed or non-validating file (meerkat's
   * `Config::validate`); it also
   * refuses a `mob.toml` that carries `[self_hosted]` or `[realm]` at top
   * level, so a table in the wrong file is an error rather than a silent
   * drop. Opt-in: without this call the gateway builds from meerkat's default
   * config as before. Mirrors the Python builder's `meerkat_config(path)`.
   */
  meerkatConfig(configPath: string): this {
    if (!configPath.trim()) {
      throw new Error("meerkatConfig path must not be empty");
    }
    this._config.meerkatConfigPath = configPath;
    return this;
  }

  /**
   * Keep every agent's provider credentials under `rootPath`.
   *
   * Emitted as `runtime_options.provider_credentials_root`. `rootPath` is the
   * credentials directory itself, an absolute path: the gateway opens it once
   * at init (creating it when missing), and every agent it builds (members,
   * the agents they create, live sessions and the agent memory engines)
   * resolves and persists provider logins and tokens there instead of the
   * process default `<config dir>/meerkat/credentials`, which every process
   * of the OS user shares. A relative path, or a root the gateway cannot
   * open, refuses init; it never falls back to the default. Runtimes that
   * share a root share provider logins, so give each user their own, and pass
   * the same root on every reopen of that user's state. A host that kept
   * users apart with a per-process `XDG_CONFIG_HOME=<C>` passes
   * `<C>/meerkat/credentials` to keep their existing logins. Opt-in: without
   * this call the gateway keeps the process default as before. Mirrors the
   * Python builder's `provider_credentials_root(path)`.
   */
  providerCredentialsRoot(rootPath: string): this {
    if (!rootPath.trim()) {
      throw new Error("providerCredentialsRoot path must not be empty");
    }
    this._config.providerCredentialsRoot = rootPath;
    return this;
  }

  /**
   * Install the public OpenAI Live (`gpt-live-1`) stdio registration
   * (`runtime_options.openai_live`).
   *
   * The gateway remains disabled when omitted. The selected binary must be
   * compiled with `openai-live`. Mutually exclusive with `experimentalLive`.
   */
  openaiLive(config: OpenAiLiveGatewayConfig): this {
    this._config.openaiLiveConfig = config;
    return this;
  }

  /**
   * Register authenticated console voice (`runtime_options.console_voice`).
   * Requires persistent sessions, console auth, and an `openai-live` gateway.
   * The principal must match the authenticated console subject.
   */
  consoleVoice(config: OpenAiLiveGatewayConfig): this {
    this._config.consoleVoiceConfig = config;
    return this;
  }

  /**
   * DEPRECATED: install the explicit pre-release GPT Live stdio registration
   * (private ChatGPT-brokered path). Prefer {@link openaiLive}.
   *
   * The gateway remains disabled when omitted. The selected binary must be
   * compiled with `experimental-gpt-live` and matching Gate0 evidence.
   */
  experimentalLive(config: ExperimentalLiveGatewayConfig): this {
    this._config.experimentalLiveConfig = config;
    return this;
  }

  consoleAuthRequired(required: boolean): this {
    this._config.consoleRequireAppAuth = required;
    return this;
  }

  consoleReadOnly(readOnly = true): this {
    this._config.consoleReadOnly = readOnly;
    return this;
  }

  /**
   * Bind the gateway HTTP listener to `HOST:PORT`.
   *
   * Emitted as `runtime_options.http_listen`. The default is `127.0.0.1:0`:
   * loopback, ephemeral port, the only bind every earlier release had. A
   * non-loopback address (`0.0.0.0:8080`) is refused at init unless the
   * console enforces app auth (`auth(...)`) or the launch acknowledges the
   * exposure with `allowRemote()`; the gateway logs a WARN line for every
   * non-loopback bind. IP literals only, no hostnames. The init result's
   * `http_base_url` keeps the same-host form (`127.0.0.1` for a wildcard
   * bind). Mirrors the Python builder's `http_listen`.
   */
  httpListen(listen: string): this {
    if (!listen.trim()) {
      throw new Error("httpListen address must not be empty");
    }
    this._config.httpListen = listen.trim();
    return this;
  }

  /**
   * Advertise the base URL clients reach the gateway at through a proxy.
   *
   * Emitted as `runtime_options.http_public_base_url` and reported back in
   * the init result; the gateway never binds it. Read it after `connect()`
   * as `runtime.rustHttpPublicBaseUrl`. Mirrors the Python builder's
   * `http_public_base_url`.
   */
  httpPublicBaseUrl(url: string): this {
    if (!url.trim()) {
      throw new Error("httpPublicBaseUrl must not be empty");
    }
    this._config.httpPublicBaseUrl = url.trim();
    return this;
  }

  /**
   * Acknowledge that `httpListen` exposes the listener beyond this host.
   *
   * Emitted as `runtime_options.allow_remote`. Same word and rule as
   * `--allow-remote` on `rkat-rpc --tcp`: an exposure acknowledgement, not an
   * auth mechanism. Pass it only with an authenticating proxy in front of the
   * listener. Mirrors the Python builder's `allow_remote`.
   */
  allowRemote(allow = true): this {
    this._config.allowRemote = allow;
    return this;
  }

  /**
   * Let in-flight member turns finish on a graceful stop, for up to
   * `windowMs`.
   *
   * Emitted as the `meta/init` parameter `shutdown_in_flight_drain_ms`. On
   * this SDK's shutdown handshake or a SIGTERM, the gateway stops the mob with
   * its members' current runs left running and waits for them, up to the
   * window, before it shuts down. A turn still running at the bound is
   * settled by the shutdown, as without a drain: a durable member's turn and
   * its eligible queued input are kept for the member's next start, and any
   * other turn is cancelled. The gateway's advertised shutdown horizon grows
   * by the window plus 10 seconds for the drain's own Stop, and this
   * transport waits that much longer on close. 0 (the default) drains
   * nothing; the most is 300000 (5 minutes). Mirrors the Python builder's
   * `shutdown_in_flight_drain_ms`.
   */
  shutdownInFlightDrainMs(windowMs: number): this {
    if (!Number.isInteger(windowMs) || windowMs < 0 || windowMs > 300_000) {
      throw new Error(
        "shutdownInFlightDrainMs must be an integer of milliseconds in 0..=300000",
      );
    }
    this._config.shutdownInFlightDrainMs = windowMs;
    return this;
  }

  consoleFetchTimeoutMs(timeoutMs: number): this {
    if (!Number.isInteger(timeoutMs) || timeoutMs <= 0) {
      throw new Error("consoleFetchTimeoutMs must be a positive integer");
    }
    this._config.consoleFetchTimeoutMs = timeoutMs;
    return this;
  }

  /**
   * Use the gateway's deterministic in-process LLM client.
   *
   * This is intended for local demos, smoke tests, and examples that need
   * autonomous agents to stay live without requiring provider credentials.
   */
  demoLlm(enabled = true): this {
    this._config.demoLlm = enabled;
    return this;
  }

  /**
   * Give locally hosted mob members signed TCP comms endpoints.
   *
   * External members run in another process and cannot reply to the default
   * in-process endpoints. Each local member binds an ephemeral TCP port. Pass
   * a concrete externally reachable interface address for cross-host peers.
   */
  memberCommsTcp(address = "127.0.0.1:0"): this {
    if (!address.trim()) {
      throw new Error("memberCommsTcp address must not be empty");
    }
    this._config.memberCommsAddress = address;
    return this;
  }

  onError(callback: ErrorCallback): this {
    this._config.errorCallback = callback;
    return this;
  }

  /**
   * Load gating tiers and escalation from an existing config file
   * (conventionally `config/gating.toml`). The gateway reads and validates
   * the file at startup: a missing path, a directory, or an unreadable or
   * invalid file refuses initialization, and an explicit empty path is
   * rejected here. Without this, gating stays off only when nothing is at
   * `config/gating.toml` (a missing or dangling parent directory counts as
   * nothing there). Any other entry there (even a directory or a dangling
   * `gating.toml` link) and any lookup error other than ENOENT is forwarded
   * to the gateway's strict loader, which refuses initialization when it
   * cannot open or read the file or rejects its contents. The loader does
   * not check the file type, so a FIFO there blocks startup instead of
   * refusing it.
   */
  gating(configPath: string): this {
    if (configPath === "") {
      throw new Error(
        "gating requires a non-empty path: since Meta 0.9.0 an empty path no longer " +
          "switches gating off. To keep gating off, do not call gating(), and keep nothing " +
          "at config/gating.toml under the working directory, which the builder loads " +
          "when present",
      );
    }
    this._config.gatingConfigPath = configPath;
    return this;
  }

  routing(configPath: string): this {
    this._config.routingConfigPath = configPath;
    return this;
  }

  /**
   * Enable or disable WorkGraph service construction (goals, work items,
   * attention bindings). Defaults to enabled on the gateway; pass `false`
   * to opt out of construction entirely.
   */
  /**
   * Enable/disable WorkGraph, or pass a string DIRECTORY for an explicit
   * durable store location (`workgraph.sqlite3` created inside) — for
   * launches without a state dir that would otherwise be memory-backed.
   */
  workgraph(enabled: boolean | string = true): this {
    this._config.workgraphEnabled = enabled;
    return this;
  }

  /**
   * Enable mob announcements (`runtime_options.mob_announcements`). `true`
   * sends `{enabled: true}` and `false` sends `{enabled: false}`. An object
   * is sent unchanged, in wire form, for the batch, retention and limit
   * options (`{enabled: true, batch: {...}, retention_ms: ...}`; see the
   * configuration reference). The gateway enables them only on a
   * persistent, identity-first launch, and refuses the init otherwise.
   * Omitted entirely when never called.
   */
  mobAnnouncements(config: Readonly<Record<string, unknown>> | boolean = true): this {
    this._config.mobAnnouncements =
      typeof config === "boolean" ? { enabled: config } : { ...config };
    return this;
  }

  /**
   * Set what a fork of a member with activated host instructions does
   * (`runtime_options.fork_instruction_policy`): `"require_explicit_activation"`
   * (the gateway default when unset), `"reactivate_inherited"` or `"refuse"`.
   */
  forkInstructionPolicy(policy: ForkInstructionPolicy): this {
    if (
      policy !== "require_explicit_activation" &&
      policy !== "reactivate_inherited" &&
      policy !== "refuse"
    ) {
      throw new RangeError(
        "forkInstructionPolicy must be 'require_explicit_activation', 'reactivate_inherited' or 'refuse'",
      );
    }
    this._config.forkInstructionPolicy = policy;
    return this;
  }

  memory(config?: unknown, options?: { stores?: string[] }): this {
    if (config === undefined && options?.stores !== undefined) {
      throw new Error(
        "memory(stores=...) is not supported by the Rust gateway; pass memory.localJson()",
      );
    }
    this._config.memoryConfig = config ?? null;
    return this;
  }

  agentMemory(
    config: true | {
      enabled?: boolean;
      realm?: string;
      selection?: "always" | "contextual";
      maxEntries?: number;
      recallTimeoutMs?: number;
      recallFailurePolicy?: "skip" | "fail";
      instructionHeader?: string;
      perTurnInjection?: "off" | "budgeted";
      defangInbound?: boolean;
      /**
       * `"markdown"` is deliberately absent: it is retired as a live store and
       * the gateway now refuses it at init (-32014). A compile error here beats
       * a boot failure. Existing markdown files still migrate - point the
       * default sqlite store at the same agent-memory directory and it imports
       * them on first open.
       */
      store?: "sqlite";
      llmWrites?: "observed" | "quarantined";
      recorderTool?: boolean;
      contentTrust?: {
        trustedMcpServers?: string[];
        untrustedTools?: string[];
        trustedTools?: string[];
      };
      /**
       * RETIRED, and narrowed to the one value that still means anything.
       * The §8.3 LLM recall-selector stage was removed unactivated, so recall
       * is the deterministic lexical path on every turn. The gateway accepts
       * absent or `"off"` for migration compatibility and refuses every
       * activation-shaped value at init.
       */
      selector?: "off";
      operatorScope?: "off" | "provisional";
      distiller?: boolean | {
        enabled?: boolean;
        runsPerHour?: number;
        minInteractions?: number;
        model?: string;
      };
      steward?: boolean | {
        enabled?: boolean;
        cadence?: string;
        model?: string;
        perMob?: boolean;
        runsPerDay?: number;
        minSignals?: number;
        /** Output-token ceiling for the dream call. Unset keeps the profile default. */
        maxOutputTokens?: number;
      };
      /**
       * PARKED. Only disabled compatibility forms are accepted. The internal
       * Rust engine remains available for validation, but the public gateway
       * has no supported activation seam or provider proof in this release.
       */
      hygienist?: false | {
        enabled: false;
        runsPerDay?: number;
        model?: string;
        maxOutputTokens?: number;
      };
    } = true,
  ): this {
    // Runtime unknown-key rejection for plain-JS callers (the type already
    // catches typos for TS callers): a typo'd option would otherwise be
    // silently dropped here, before the gateway's own fail-loud check.
    if (config !== true) {
      rejectUnknownKeys(config, AGENT_MEMORY_KEYS, "agentMemory");
    }
    if (config !== true && config.enabled === false) {
      this._config.agentMemoryConfig = { enabled: false };
      return this;
    }
    if (config === true) {
      this._config.agentMemoryConfig = true;
      return this;
    }
    const wire: Record<string, unknown> = {};
    if (config.enabled !== undefined) wire.enabled = config.enabled;
    if (config.realm !== undefined) wire.realm = config.realm;
    if (config.selection !== undefined) wire.selection = config.selection;
    if (config.maxEntries !== undefined) wire.max_entries = config.maxEntries;
    if (config.recallTimeoutMs !== undefined) {
      wire.recall_timeout_ms = config.recallTimeoutMs;
    }
    if (config.recallFailurePolicy !== undefined) {
      wire.recall_failure_policy = config.recallFailurePolicy;
    }
    if (config.instructionHeader !== undefined) {
      wire.instruction_header = config.instructionHeader;
    }
    if (config.perTurnInjection !== undefined) {
      wire.per_turn_injection = config.perTurnInjection;
    }
    if (config.defangInbound !== undefined) {
      wire.defang_inbound = config.defangInbound;
    }
    if (config.store !== undefined) wire.store = config.store;
    if (config.llmWrites !== undefined) wire.llm_writes = config.llmWrites;
    if (config.recorderTool !== undefined) {
      wire.recorder_tool = config.recorderTool;
    }
    if (config.contentTrust !== undefined) {
      rejectUnknownKeys(
        config.contentTrust,
        CONTENT_TRUST_KEYS,
        "agentMemory contentTrust",
      );
      const contentTrust: Record<string, unknown> = {};
      if (config.contentTrust.trustedMcpServers !== undefined) {
        contentTrust.trusted_mcp_servers = config.contentTrust.trustedMcpServers;
      }
      if (config.contentTrust.untrustedTools !== undefined) {
        contentTrust.untrusted_tools = config.contentTrust.untrustedTools;
      }
      if (config.contentTrust.trustedTools !== undefined) {
        contentTrust.trusted_tools = config.contentTrust.trustedTools;
      }
      wire.content_trust = contentTrust;
    }
    if (config.selector !== undefined) {
      if (config.selector !== "off") {
        throw new Error(
          "agentMemory selector is RETIRED; remove it or set selector to 'off'",
        );
      }
      wire.selector = config.selector;
    }
    if (config.operatorScope !== undefined) {
      wire.operator_scope = config.operatorScope;
    }
    if (config.distiller !== undefined) {
      if (typeof config.distiller === "boolean") {
        wire.distiller = config.distiller;
      } else {
        rejectUnknownKeys(
          config.distiller,
          DISTILLER_KEYS,
          "agentMemory distiller",
        );
        const distiller: Record<string, unknown> = {};
        if (config.distiller.enabled !== undefined) {
          distiller.enabled = config.distiller.enabled;
        }
        if (config.distiller.runsPerHour !== undefined) {
          distiller.runs_per_hour = config.distiller.runsPerHour;
        }
        if (config.distiller.minInteractions !== undefined) {
          distiller.min_interactions = config.distiller.minInteractions;
        }
        if (config.distiller.model !== undefined) {
          distiller.model = config.distiller.model;
        }
        wire.distiller = distiller;
      }
    }
    if (config.steward !== undefined) {
      if (typeof config.steward === "boolean") {
        wire.steward = config.steward;
      } else {
        rejectUnknownKeys(config.steward, STEWARD_KEYS, "agentMemory steward");
        const steward: Record<string, unknown> = {};
        if (config.steward.enabled !== undefined) {
          steward.enabled = config.steward.enabled;
        }
        if (config.steward.cadence !== undefined) {
          steward.cadence = config.steward.cadence;
        }
        if (config.steward.model !== undefined) {
          steward.model = config.steward.model;
        }
        if (config.steward.perMob !== undefined) {
          steward.per_mob = config.steward.perMob;
        }
        if (config.steward.maxOutputTokens !== undefined) {
          steward.max_output_tokens = config.steward.maxOutputTokens;
        }
        if (config.steward.runsPerDay !== undefined) {
          steward.runs_per_day = config.steward.runsPerDay;
        }
        if (config.steward.minSignals !== undefined) {
          steward.min_signals = config.steward.minSignals;
        }
        wire.steward = steward;
      }
    }
    if (config.hygienist !== undefined) {
      if (
        config.hygienist !== false &&
        (typeof config.hygienist !== "object" ||
          config.hygienist.enabled !== false)
      ) {
        throw new Error(
          "agentMemory hygienist is PARKED and cannot be enabled; remove it, use false, or use { enabled: false }",
        );
      }
      if (typeof config.hygienist === "boolean") {
        wire.hygienist = config.hygienist;
      } else {
        rejectUnknownKeys(
          config.hygienist,
          HYGIENIST_KEYS,
          "agentMemory hygienist",
        );
        const hygienist: Record<string, unknown> = {};
        if (config.hygienist.enabled !== undefined) {
          hygienist.enabled = config.hygienist.enabled;
        }
        if (config.hygienist.runsPerDay !== undefined) {
          hygienist.runs_per_day = config.hygienist.runsPerDay;
        }
        if (config.hygienist.model !== undefined) {
          hygienist.model = config.hygienist.model;
        }
        if (config.hygienist.maxOutputTokens !== undefined) {
          hygienist.max_output_tokens = config.hygienist.maxOutputTokens;
        }
        wire.hygienist = hygienist;
      }
    }
    this._config.agentMemoryConfig = wire;
    return this;
  }

  auth(config: unknown): this {
    this._config.authConfig = config;
    return this;
  }

  implicitDelegateIdleRetirement(seconds: number | null): this {
    if (seconds !== null && (!Number.isFinite(seconds) || seconds < 0)) {
      throw new Error(
        "implicit delegate idle retirement seconds must be non-negative or null",
      );
    }
    this._config.implicitDelegateIdleRetireSecs = seconds;
    return this;
  }

  /**
   * Name the artifacts this host installed (extension packages, plugins).
   *
   * Emitted as `runtime_options.installed_artifacts` at the trusted
   * bootstrap. With a set, every callback tool must name a
   * {@link ToolArtifactRef} from it (`registerTool(..., { artifact })`) whose
   * digest matches; an unknown, mismatched or missing reference refuses the
   * build before any tool is published. Without a set, callback tools stay
   * unassociated. Attribution only, never authority.
   */
  installedArtifacts(artifacts: Iterable<ToolArtifactRef>): this {
    const list = [...artifacts].map(validateToolArtifactRef);
    const seen = new Map<string, string>();
    for (const artifact of list) {
      const digest = seen.get(artifact.artifactId);
      if (digest !== undefined && digest !== artifact.digest) {
        throw new Error(
          `installed artifact ${artifact.artifactId} is listed with two different digests`,
        );
      }
      seen.set(artifact.artifactId, artifact.digest);
    }
    this._config.installedArtifacts = list;
    return this;
  }

  maxSessions(maxSessions: number): this {
    if (!Number.isInteger(maxSessions) || maxSessions <= 0) {
      throw new Error("maxSessions must be a positive integer");
    }
    this._config.maxSessions = maxSessions;
    return this;
  }

  gatewayTimeoutMs(timeoutMs: number): this {
    if (!Number.isInteger(timeoutMs) || timeoutMs <= 0) {
      throw new Error("gatewayTimeoutMs must be a positive integer");
    }
    this._config.gatewayTimeoutMs = timeoutMs;
    return this;
  }

  gateway(binPath: string): this {
    this._config.gatewayBin = binPath;
    return this;
  }

  /**
   * Compose the local gateway process. Every `connect()` calls `factory`
   * with the `gateway()` binary to build the transport it starts, in place
   * of the default `PersistentTransport`, so a host can pass extra arguments,
   * stdio slots or its own startup step. Host-local only: nothing about it is
   * sent in RPC. `shutdown()` and a failed connect stop the returned
   * transport. Requires `gateway()`.
   */
  gatewayTransport(factory: GatewayTransportFactory): this {
    if (typeof factory !== "function") {
      throw new TypeError("gateway transport factory must be callable");
    }
    this._config.gatewayTransportFactory = factory;
    return this;
  }

  /**
   * Cap how long `connect()` waits for an accepted `meta/init`.
   *
   * A gateway that accepted init keeps working through its startup phases
   * (provider callbacks, restore, prewarm) and then settles ready or failed.
   * By default the SDK waits for that settlement, the gateway exiting, or the
   * reader failing. Set a deadline only when the host has its own startup
   * budget: running out throws `InitOutcomeUnknownError`, because the gateway
   * may already have changed native state, and the gateway is asked to shut
   * down.
   */
  initDeadline(ms: number | null): this {
    if (ms !== null && (typeof ms !== "number" || !Number.isFinite(ms) || ms <= 0)) {
      throw new TypeError("initDeadline must be a positive finite number of milliseconds or null");
    }
    this._config.initDeadlineMs = ms;
    return this;
  }

  modules(moduleSpecs: unknown[]): this {
    this._config.modules = moduleSpecs;
    return this;
  }

  /**
   * Enable persistent state at the given path.
   *
   * When set, the gateway creates SQLite-backed session/runtime state,
   * Meta metadata, console logs, and binary blob storage under this
   * directory. Mob storage remains in-memory.
   */
  persistentState(path: string): this {
    this._config.persistentState = path;
    return this;
  }

  /** Set a RosterProvider. */
  rosterProvider(provider: RosterProvider): this {
    this._config.rosterProvider = provider;
    return this;
  }

  /** Set an AgentCustomizer. */
  agentCustomizer(customizer: AgentCustomizer): this {
    this._config.agentCustomizer = customizer;
    return this;
  }

  /** Set a TopologyProvider. */
  topologyProvider(provider: TopologyProvider): this {
    this._config.topologyProvider = provider;
    return this;
  }

  /**
   * Resolve scoped credentials immediately before each committed detached
   * attempt. Resolved values remain host-local and never cross job RPC.
   */
  jobCredentials(resolver: JobCredentialResolver): this {
    if (typeof resolver !== "function") {
      throw new TypeError("job credential resolver must be callable");
    }
    this._config.jobCredentialResolver = resolver;
    return this;
  }

  async build(): Promise<MetaRuntime> {
    this._validateConfig();
    this._applyConventionDefaults();
    // Dynamic import to break circular dep (runtime imports from builder config type)
    const { MetaRuntime } = await import("./runtime.js");
    return MetaRuntime._create(this._config);
  }

  private _validateConfig(): void {
    if (this._config.preparedApplication && this._config.mobConfigPath) {
      throw new Error("preparedApplication() and mob() are mutually exclusive");
    }
    if (this._config.gatewayTransportFactory && !this._config.gatewayBin) {
      throw new Error("gatewayTransport() requires gateway()");
    }
    if (
      this._config.declareSpecUpdateExpectedRevision !== undefined &&
      this._config.declareSpecUpdateExpectedRevision !== null
    ) {
      if (!this._config.mobConfigPath && !this._config.preparedApplication) {
        throw new Error(
          "declareSpecUpdate() requires mob() or preparedApplication() to supply " +
            "the replacement definition",
        );
      }
      if (!this._config.persistentState) {
        throw new Error(
          "declareSpecUpdate() requires persistentState() because ephemeral mob " +
            "storage has no durable definition to update",
        );
      }
    }
  }

  private _applyConventionDefaults(): void {
    if (this._config.consoleConfigPath === null) {
      const candidate = "config/console.toml";
      if (existsSync(candidate)) {
        this._config.consoleConfigPath = candidate;
      }
    }

    if (this._config.accessConfigPath === null) {
      const candidate = "config/access.toml";
      if (entryPresent(candidate)) {
        this._config.accessConfigPath = candidate;
      }
    }

    if (this._config.gatingConfigPath === null) {
      const candidate = "config/gating.toml";
      if (entryPresent(candidate)) {
        this._config.gatingConfigPath = candidate;
      }
    }

    if (this._config.routingConfigPath === null) {
      const candidate = "deployment/routing.toml";
      if (existsSync(candidate)) {
        this._config.routingConfigPath = candidate;
      }
    }

  }
}

/**
 * Whether the lookup of `path` finds an entry or fails other than with
 * ENOENT. Discovery for a file whose presence opts into a security control:
 * only ENOENT (nothing there, including a missing or dangling parent) means
 * "off". A directory, a dangling final link, ENOTDIR or any other lookup
 * error is reported so the strict loader handles it.
 */
function entryPresent(path: string): boolean {
  try {
    lstatSync(path);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code !== "ENOENT";
  }
}

// -- Meta static factory ------------------------------------------------

export class Meta {
  static builder(): MetaBuilder {
    return new MetaBuilder();
  }
}
