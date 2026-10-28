/**
 * Typed data models for Meta SDK — input/config objects sent to the runtime.
 */

import { DetachedJobExecution } from "./jobs.js";

// -- DiscoverySpec --------------------------------------------------------

export interface DiscoverySpec {
  readonly role: string;
  readonly agentIdentity: string;
  readonly labels?: Readonly<Record<string, string>>;
  readonly appContext?: unknown;
  readonly additionalInstructions?: readonly string[];
  readonly resumeSessionId?: string;
}

export function discoverySpecToDict(
  spec: DiscoverySpec,
): Record<string, unknown> {
  const result: Record<string, unknown> = {
    role: spec.role,
    agent_identity: spec.agentIdentity,
  };
  if (spec.labels && Object.keys(spec.labels).length > 0) {
    result.labels = { ...spec.labels };
  }
  if (spec.appContext !== undefined) {
    result.app_context = spec.appContext;
  }
  if (spec.additionalInstructions && spec.additionalInstructions.length > 0) {
    result.additional_instructions = [...spec.additionalInstructions];
  }
  if (spec.resumeSessionId !== undefined) {
    result.resume_session_id = spec.resumeSessionId;
  }
  return result;
}

// -- PreSpawnData ---------------------------------------------------------

export interface PreSpawnData {
  readonly resumeMap?: Readonly<Record<string, string>>;
  readonly moduleId?: string;
  readonly env?: Readonly<Record<string, string>>;
}

export function preSpawnDataToDict(
  data: PreSpawnData,
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (data.resumeMap && Object.keys(data.resumeMap).length > 0) {
    result.resume_map = { ...data.resumeMap };
  }
  if (data.moduleId !== undefined) {
    result.module_id = data.moduleId;
  }
  if (data.env && Object.keys(data.env).length > 0) {
    result.env = Object.entries(data.env);
  }
  return result;
}

// -- SessionQuery ---------------------------------------------------------

export interface SessionQuery {
  readonly agentType?: string;
  readonly ownerId?: string;
  readonly labels?: Readonly<Record<string, string>>;
  readonly includeDeleted?: boolean;
  readonly limit?: number;
}

export function sessionQueryToDict(
  query: SessionQuery,
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (query.agentType !== undefined) result.agent_type = query.agentType;
  if (query.ownerId !== undefined) result.owner_id = query.ownerId;
  if (query.labels && Object.keys(query.labels).length > 0) {
    result.labels = { ...query.labels };
  }
  result.include_deleted = query.includeDeleted ?? false;
  result.limit = query.limit ?? 100;
  return result;
}

// -- Fork lineage -----------------------------------------------------------

/**
 * A mob member as meerkat names it: its mob, role and roster member id.
 *
 * In a Meta mob `member` is Meta's comms-safe roster encoding of the
 * member's durable identity (`mk--...`), not the identity itself.
 */
export interface MobMemberBinding {
  readonly mobId: string;
  readonly role: string;
  readonly member: string;
}

/**
 * The source a fork-derived member was forked from (meerkat 0.8.45+).
 *
 * Set by the mob runtime on the build that seats a durable fork (fork_off
 * children, fork_member children, local council participants) and on every
 * later rebuild of that member. Absent for every other build.
 */
export interface ForkBuildSource {
  readonly sourceMember: MobMemberBinding;
  readonly sourceSessionId: string;
}

function requiredString(
  data: Record<string, unknown>,
  key: string,
  owner: string,
): string {
  const value = data[key];
  if (typeof value !== "string") {
    throw new TypeError(`${owner}.${key} must be a string, got ${typeof value}`);
  }
  return value;
}

function asObject(raw: unknown, owner: string): Record<string, unknown> {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
    throw new TypeError(`${owner} must be an object`);
  }
  return raw as Record<string, unknown>;
}

/**
 * Decode the gateway's `fork_source` object. `null`/absent decodes to
 * `null`; fields this SDK does not know are ignored, so a newer gateway can
 * add fields without breaking older builders.
 */
export function parseForkBuildSource(raw: unknown): ForkBuildSource | null {
  if (raw === undefined || raw === null) return null;
  const data = asObject(raw, "ForkBuildSource");
  const member = asObject(data.source_member, "ForkBuildSource.source_member");
  return {
    sourceMember: {
      mobId: requiredString(member, "mob_id", "MobMemberBinding"),
      role: requiredString(member, "role", "MobMemberBinding"),
      member: requiredString(member, "member", "MobMemberBinding"),
    },
    sourceSessionId: requiredString(data, "source_session_id", "ForkBuildSource"),
  };
}

// -- SessionBuildOptions --------------------------------------------------

/** Callback tool handler: receives arguments dict, returns JSON-serializable result. */
/**
 * An installed artifact a callback tool's handler came from: an opaque
 * artifact id and the digest the host installed. The gateway verifies it
 * against the host's {@link MetaBuilder.installedArtifacts} before the build
 * publishes any tool, and carries a verified reference on the tool's
 * provenance. Attribution only: it never grants or refuses a call, and a
 * tool's name never implies one. It is not proof of the executable bytes or
 * the semantics of the handler that runs. The host's installed set is
 * trusted configuration from its own bootstrap.
 */
export interface ToolArtifactRef {
  readonly artifactId: string;
  readonly digest: string;
}

const TOOL_ARTIFACT_PART_MAX_BYTES = 256;

/** Validate a {@link ToolArtifactRef}; throws on a malformed part. */
export function validateToolArtifactRef(artifact: ToolArtifactRef): ToolArtifactRef {
  if (artifact === null || typeof artifact !== "object") {
    throw new TypeError("artifact must be a ToolArtifactRef object");
  }
  for (const [label, part] of [
    ["artifactId", artifact.artifactId],
    ["digest", artifact.digest],
  ] as const) {
    if (typeof part !== "string" || part.length === 0) {
      throw new Error(`tool artifact ${label} must be a non-empty string`);
    }
    if (new TextEncoder().encode(part).length > TOOL_ARTIFACT_PART_MAX_BYTES) {
      throw new Error(`tool artifact ${label} exceeds ${TOOL_ARTIFACT_PART_MAX_BYTES} bytes`);
    }
    if (/[\s\p{Cc}]/u.test(part)) {
      throw new Error(`tool artifact ${label} must not contain whitespace or control characters`);
    }
  }
  return { artifactId: artifact.artifactId, digest: artifact.digest };
}

export function toolArtifactRefToWire(artifact: ToolArtifactRef): Record<string, string> {
  return { artifact_id: artifact.artifactId, digest: artifact.digest };
}

export function parseToolArtifactRef(raw: unknown): ToolArtifactRef {
  const d = (raw ?? {}) as Record<string, unknown>;
  return validateToolArtifactRef({
    artifactId: d.artifact_id as string,
    digest: d.digest as string,
  });
}

/** The one origin a member auth binding may name. */
export const CONFIGURED_AUTH_BINDING_ORIGIN = "configured" as const;

/**
 * A configured auth binding of a realm: Meerkat's `AuthBindingRef`.
 *
 * `realm` and `binding` name the binding, and `profile` one of its auth
 * profiles when set. `origin` may be left out or be `"configured"`, the only
 * origin a member binding takes. The gateway resolves the binding in the
 * members' configuration at init; the SDK checks only its shape.
 */
export interface AuthBindingRef {
  readonly realm: string;
  readonly binding: string;
  readonly profile?: string;
  readonly origin?: typeof CONFIGURED_AUTH_BINDING_ORIGIN;
}

/**
 * A realm a native host keeps, whose configuration a gateway's members build
 * from (`runtime_options.native_realm`): `realmId` under `stateRoot`, the
 * realms root it lives under; `userConfigRoot`, the home-like root whose
 * `.rkat/config.toml` is the `global` realm's document; and `contextRoot`,
 * the native host's context root. The gateway requires each root to be an
 * absolute path and resolves the realm at init; the SDK checks only that
 * every field is a non-empty string.
 */
export interface NativeRealm {
  readonly realmId: string;
  readonly contextRoot: string;
  readonly stateRoot: string;
  readonly userConfigRoot: string;
}

function requireNonEmpty(value: unknown, context: string): string {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${context} must be a non-empty string`);
  }
  return value;
}

function requireClosedObject(
  raw: unknown,
  keys: readonly string[],
  context: string,
): Record<string, unknown> {
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
    throw new TypeError(`${context} must be an object`);
  }
  const extra = Object.keys(raw).filter((key) => !keys.includes(key)).sort();
  if (extra.length > 0) {
    throw new Error(`${context} has unknown fields: ${extra.join(", ")}`);
  }
  return raw as Record<string, unknown>;
}

/** Validate an {@link AuthBindingRef}; throws on a malformed field. */
export function validateAuthBindingRef(binding: AuthBindingRef): AuthBindingRef {
  const checked = requireClosedObject(
    binding,
    ["realm", "binding", "profile", "origin"],
    "auth binding",
  );
  const validated: {
    realm: string;
    binding: string;
    profile?: string;
    origin?: typeof CONFIGURED_AUTH_BINDING_ORIGIN;
  } = {
    realm: requireNonEmpty(checked.realm, "auth binding realm"),
    binding: requireNonEmpty(checked.binding, "auth binding binding"),
  };
  if (checked.profile !== undefined) {
    validated.profile = requireNonEmpty(checked.profile, "auth binding profile");
  }
  if (checked.origin !== undefined) {
    if (checked.origin !== CONFIGURED_AUTH_BINDING_ORIGIN) {
      throw new Error(
        `auth binding origin must be "${CONFIGURED_AUTH_BINDING_ORIGIN}" or absent, got ${JSON.stringify(checked.origin)}`,
      );
    }
    validated.origin = CONFIGURED_AUTH_BINDING_ORIGIN;
  }
  return validated;
}

/** The wire form of an {@link AuthBindingRef}: the fields set, as sent. */
export function authBindingRefToWire(binding: AuthBindingRef): Record<string, string> {
  const validated = validateAuthBindingRef(binding);
  const wire: Record<string, string> = {
    realm: validated.realm,
    binding: validated.binding,
  };
  if (validated.profile !== undefined) {
    wire.profile = validated.profile;
  }
  if (validated.origin !== undefined) {
    wire.origin = validated.origin;
  }
  return wire;
}

/** Read an {@link AuthBindingRef} from its wire form, which is closed. */
export function parseAuthBindingRef(raw: unknown): AuthBindingRef {
  return validateAuthBindingRef(raw as AuthBindingRef);
}

/** Validate a {@link NativeRealm}; throws on a malformed field. */
export function validateNativeRealm(realm: NativeRealm): NativeRealm {
  const checked = requireClosedObject(
    realm,
    ["realmId", "contextRoot", "stateRoot", "userConfigRoot"],
    "native realm",
  );
  return {
    realmId: requireNonEmpty(checked.realmId, "native realm realmId"),
    contextRoot: requireNonEmpty(checked.contextRoot, "native realm contextRoot"),
    stateRoot: requireNonEmpty(checked.stateRoot, "native realm stateRoot"),
    userConfigRoot: requireNonEmpty(checked.userConfigRoot, "native realm userConfigRoot"),
  };
}

/** The wire form of a {@link NativeRealm} (`runtime_options.native_realm`). */
export function nativeRealmToWire(realm: NativeRealm): Record<string, string> {
  const validated = validateNativeRealm(realm);
  return {
    realm_id: validated.realmId,
    context_root: validated.contextRoot,
    state_root: validated.stateRoot,
    user_config_root: validated.userConfigRoot,
  };
}

/** Read a {@link NativeRealm} from its wire form, which is closed. */
export function parseNativeRealm(raw: unknown): NativeRealm {
  const wire = requireClosedObject(
    raw,
    ["realm_id", "context_root", "state_root", "user_config_root"],
    "native realm",
  );
  return validateNativeRealm({
    realmId: wire.realm_id as string,
    contextRoot: wire.context_root as string,
    stateRoot: wire.state_root as string,
    userConfigRoot: wire.user_config_root as string,
  });
}

export type ToolHandler = (
  args: Record<string, unknown>,
) => unknown | Promise<unknown>;

/**
 * Mutable options passed to {@link SessionAgentBuilder.buildAgent}.
 *
 * The builder mutates fields during agent construction — sets profileName,
 * calls {@link addTools} or {@link registerTool}.
 *
 * @example
 * ```ts
 * const builder: SessionAgentBuilder = {
 *   async buildAgent(opts) {
 *     opts.profileName = "assistant";
 *     opts.registerTool("search", searchHandler);
 *   },
 * };
 * ```
 */
export class SessionBuildOptions {
  appContext: unknown = undefined;
  additionalInstructions: string[] = [];
  sessionId: string | null = null;
  labels: Record<string, string> = {};
  profileName: string | null = null;
  /**
   * The session this build resumes, when the gateway resumes one; a builder
   * may also set it to ask the gateway to resume that session (persistent
   * mode only).
   */
  resumeSessionId: string | null = null;
  /**
   * Receive-only fork lineage: which member this member was forked from, so
   * the builder can build the child as its source. Never sent back, and the
   * gateway ignores it in a build response: fork lineage is set by the mob
   * runtime alone. `forkSource.sourceMember.member` is Meta's encoded
   * roster id; {@link forkSourceIdentity} is the source's durable identity
   * (for example `domain:calendar`), the key to resolve grants by. The
   * child's own `labels` and `sessionId` still name the child.
   */
  readonly forkSource: ForkBuildSource | null;
  /** The fork source's durable identity; see {@link forkSource}. */
  readonly forkSourceIdentity: string | null;

  private _tools: string[] = [];
  private _toolHandlers: Map<string, ToolHandler> = new Map();
  private _jobExecutions: Map<string, DetachedJobExecution> = new Map();
  // Per-tool wire metadata from registerTool options; tools without an
  // entry cross the wire as bare name strings.
  private _toolDefs: Map<string, Record<string, unknown>> = new Map();

  constructor(init?: {
    forkSource?: ForkBuildSource | null;
    forkSourceIdentity?: string | null;
  }) {
    this.forkSource = init?.forkSource ?? null;
    this.forkSourceIdentity = init?.forkSourceIdentity ?? null;
  }

  /** Declare tool names the agent can use. */
  addTools(tools: string[]): void {
    for (const t of tools) {
      if (typeof t !== "string") {
        throw new TypeError(
          `tools must be strings, got ${typeof t}: ${String(t)}`,
        );
      }
    }
    this._tools.push(...tools);
  }

  /**
   * Register a callable tool with the agent.
   *
   * `options.inputSchema` is the JSON Schema for the tool arguments; when
   * omitted the gateway advertises the permissive `{"type": "object"}`.
   */
  registerTool(
    name: string,
    handler: ToolHandler,
    options?: {
      description?: string;
      inputSchema?: Record<string, unknown>;
      execution?: DetachedJobExecution;
      /** Installed artifact this handler came from; verified by the gateway. */
      artifact?: ToolArtifactRef;
    },
  ): void {
    if (typeof name !== "string") {
      throw new TypeError(
        `tool name must be a string, got ${typeof name}: ${String(name)}`,
      );
    }
    if (typeof handler !== "function") {
      throw new TypeError(
        `handler must be callable, got ${typeof handler}: ${String(handler)}`,
      );
    }
    // Validate everything before any mutation: a refused registration
    // leaves the previous handler and metadata untouched.
    const artifact =
      options?.artifact !== undefined ? validateToolArtifactRef(options.artifact) : undefined;
    if (
      options?.execution !== undefined &&
      !(options.execution instanceof DetachedJobExecution)
    ) {
      throw new TypeError("execution must be a DetachedJobExecution");
    }
    // Registering a name again replaces its handler AND all of its metadata:
    // a registration without an artifact never inherits the previous
    // handler's artifact (or description, schema, execution).
    if (!this._tools.includes(name)) this._tools.push(name);
    this._toolDefs.delete(name);
    this._jobExecutions.delete(name);
    this._toolHandlers.set(name, handler);
    if (
      options?.description !== undefined ||
      options?.inputSchema !== undefined ||
      options?.execution !== undefined ||
      artifact !== undefined
    ) {
      const def: Record<string, unknown> = { name };
      if (options?.description !== undefined) def.description = options.description;
      if (options?.inputSchema !== undefined) def.input_schema = options.inputSchema;
      if (options?.execution !== undefined) {
        def.execution = options.execution.toWire();
        this._jobExecutions.set(name, options.execution);
      }
      if (artifact !== undefined) def.artifact = toolArtifactRefToWire(artifact);
      this._toolDefs.set(name, def);
    }
  }

  get tools(): string[] {
    return [...this._tools];
  }

  get toolHandlers(): ReadonlyMap<string, ToolHandler> {
    return new Map(this._toolHandlers);
  }

  get jobExecutions(): ReadonlyMap<string, DetachedJobExecution> {
    return new Map(this._jobExecutions);
  }

  toDict(): Record<string, unknown> {
    const result: Record<string, unknown> = {};
    if (this.appContext !== undefined) result.app_context = this.appContext;
    if (this.additionalInstructions.length > 0) {
      result.additional_instructions = [...this.additionalInstructions];
    }
    if (this.sessionId !== null) result.session_id = this.sessionId;
    if (Object.keys(this.labels).length > 0) {
      result.labels = { ...this.labels };
    }
    if (this.profileName !== null) result.profile_name = this.profileName;
    if (this.resumeSessionId !== null) {
      result.resume_session_id = this.resumeSessionId;
    }
    if (this._tools.length > 0) {
      // Names with registered metadata cross as {name, description?,
      // input_schema?} objects; everything else stays a bare string
      // (backward-compatible with pre-0.7.30 gateways).
      result.tools = this._tools.map(
        (name) => this._toolDefs.get(name) ?? name,
      );
    }
    return result;
  }
}
