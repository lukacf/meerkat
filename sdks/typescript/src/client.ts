/**
 * Meerkat client — spawns rkat-rpc and communicates via JSON-RPC.
 *
 * @example
 * ```ts
 * import { MeerkatClient } from "@rkat/sdk";
 *
 * const client = new MeerkatClient();
 * await client.connect();
 *
 * const session = await client.createSession("Hello!");
 * console.log(session.text);
 *
 * const result = await session.turn("Tell me more");
 * console.log(result.text);
 *
 * for await (const event of session.stream("Explain in detail")) {
 *   if (event.type === "text_delta") {
 *     process.stdout.write(event.delta);
 *   }
 * }
 *
 * await session.archive();
 * await client.close();
 * ```
 */

import { spawn, type ChildProcess } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { createInterface, type Interface as ReadlineInterface } from "node:readline";
import { MeerkatError, CapabilityUnavailableError } from "./generated/errors.js";
import { CONTRACT_VERSION } from "./generated/types.js";
import { Session } from "./session.js";
import { EventStream, AsyncQueue } from "./streaming.js";
import type { Capability, RunResult, SchemaWarning, SessionInfo, SessionOptions, SkillKey, SkillRef, Usage } from "./types.js";

const MEERKAT_REPO = "lukacf/meerkat";
const MEERKAT_RELEASE_BINARY = "rkat-rpc";
const MEERKAT_BINARY_CACHE_ROOT = path.join(
  os.homedir(),
  ".cache",
  "meerkat",
  "bin",
  MEERKAT_RELEASE_BINARY,
);

interface ResolvedBinary {
  command: string;
  useLegacySubcommand: boolean;
}

interface PlatformTarget {
  target: string;
  archiveExt: "tar.gz" | "zip";
  binaryName: string;
}

/** Options for connecting to the rkat-rpc runtime. */
export interface ConnectOptions {
  realmId?: string;
  instanceId?: string;
  realmBackend?: "jsonl" | "redb";
  isolated?: boolean;
  stateRoot?: string;
  contextRoot?: string;
  userConfigRoot?: string;
}

/**
 * Normalize a SkillRef to the wire format { source_uuid, skill_name }.
 *
 * SkillKey objects are converted from camelCase to snake_case.
 * Legacy strings are parsed and emit a console warning.
 */
function normalizeSkillRef(ref: SkillRef): { source_uuid: string; skill_name: string } {
  if (typeof ref !== "string") {
    return { source_uuid: ref.sourceUuid, skill_name: ref.skillName };
  }
  const value = ref.startsWith("/") ? ref.slice(1) : ref;
  const [sourceUuid, ...rest] = value.split("/");
  if (!sourceUuid || rest.length === 0) {
    throw new Error(
      `Invalid skill reference '${ref}'. Expected '<source_uuid>/<skill_name>'.`,
    );
  }
  console.warn(
    `[meerkat-sdk] legacy skill reference '${ref}' is deprecated; pass { sourceUuid, skillName } instead.`,
  );
  return { source_uuid: sourceUuid, skill_name: rest.join("/") };
}

function skillRefsToWire(refs: SkillRef[] | undefined): Array<{ source_uuid: string; skill_name: string }> | undefined {
  if (!refs) return undefined;
  return refs.map(normalizeSkillRef);
}

export class MeerkatClient {
  private process: ChildProcess | null = null;
  private requestId = 0;
  private _capabilities: Capability[] = [];
  private rkatPath: string;
  private pendingRequests = new Map<
    number,
    { resolve: (value: Record<string, unknown>) => void; reject: (reason: unknown) => void }
  >();
  private eventQueues = new Map<string, AsyncQueue<Record<string, unknown> | null>>();
  private rl: ReadlineInterface | null = null;

  constructor(rkatPath = "rkat-rpc") {
    this.rkatPath = rkatPath;
  }

  // -- Connection ---------------------------------------------------------

  async connect(options?: ConnectOptions): Promise<this> {
    if (options?.realmId && options?.isolated) {
      throw new MeerkatError("INVALID_ARGS", "realmId and isolated cannot both be set");
    }

    const resolved = await MeerkatClient.resolveBinaryPath(this.rkatPath);
    this.rkatPath = resolved.command;

    const args = MeerkatClient.buildArgs(resolved.useLegacySubcommand, options);
    if (resolved.useLegacySubcommand && args.length > 1) {
      throw new MeerkatError(
        "LEGACY_BINARY_UNSUPPORTED",
        "Realm/context options require the standalone rkat-rpc binary. Install rkat-rpc and retry.",
      );
    }

    this.process = spawn(this.rkatPath, args, {
      stdio: ["pipe", "pipe", "pipe"],
    });

    this.rl = createInterface({ input: this.process.stdout! });
    this.rl.on("line", (line: string) => this.handleLine(line));

    // Handshake
    const initResult = await this.request("initialize", {});
    const serverVersion = String(initResult.contract_version ?? "");
    if (!MeerkatClient.checkVersionCompatible(serverVersion, CONTRACT_VERSION)) {
      throw new MeerkatError(
        "VERSION_MISMATCH",
        `Server version ${serverVersion} incompatible with SDK ${CONTRACT_VERSION}`,
      );
    }

    // Fetch capabilities
    const capsResult = await this.request("capabilities/get", {});
    const rawCaps = (capsResult.capabilities as Array<Record<string, unknown>>) ?? [];
    this._capabilities = rawCaps.map(
      (cap): Capability => ({
        id: String(cap.id ?? ""),
        description: String(cap.description ?? ""),
        status: MeerkatClient.normalizeStatus(cap.status),
      }),
    );

    return this;
  }

  async close(): Promise<void> {
    if (this.rl) {
      this.rl.close();
      this.rl = null;
    }
    if (this.process) {
      this.process.kill();
      this.process = null;
    }
    for (const [, pending] of this.pendingRequests) {
      pending.reject(new MeerkatError("CLIENT_CLOSED", "Client closed"));
    }
    this.pendingRequests.clear();
    for (const [, queue] of this.eventQueues) {
      queue.put(null);
    }
    this.eventQueues.clear();
  }

  // -- Session lifecycle --------------------------------------------------

  /**
   * Create a new session, run the first turn, and return a {@link Session}.
   *
   * @example
   * ```ts
   * const session = await client.createSession("Summarise this project", {
   *   model: "claude-sonnet-4-5",
   * });
   * console.log(session.text);
   * ```
   */
  async createSession(
    prompt: string,
    options?: SessionOptions,
  ): Promise<Session> {
    const params = MeerkatClient.buildCreateParams(prompt, options);
    const raw = await this.request("session/create", params);
    const result = MeerkatClient.parseRunResult(raw);
    return new Session(this, result);
  }

  /**
   * Create a new session and stream typed events from the first turn.
   *
   * Returns an {@link EventStream} `AsyncIterable<AgentEvent>`.
   */
  createSessionStreaming(
    prompt: string,
    options?: SessionOptions,
  ): EventStream {
    if (!this.process?.stdin) {
      throw new MeerkatError("NOT_CONNECTED", "Client not connected");
    }

    const params = MeerkatClient.buildCreateParams(prompt, options);
    this.requestId++;
    const requestId = this.requestId;

    const queue = new AsyncQueue<Record<string, unknown> | null>();
    const responsePromise = this.registerRequest(requestId);

    // When response arrives, bind the queue to the session_id
    const wrappedPromise = responsePromise.then((result) => {
      const sid = String(result.session_id ?? "");
      if (sid) {
        this.eventQueues.set(sid, queue);
      }
      return result;
    });

    const rpcRequest = { jsonrpc: "2.0", id: requestId, method: "session/create", params };
    this.process.stdin!.write(JSON.stringify(rpcRequest) + "\n");

    return new EventStream({
      sessionId: "",
      eventQueue: queue,
      responsePromise: wrappedPromise,
      parseResult: MeerkatClient.parseRunResult,
    });
  }

  // -- Session queries ----------------------------------------------------

  async listSessions(): Promise<SessionInfo[]> {
    const result = await this.request("session/list", {});
    const sessions = (result.sessions as Array<Record<string, unknown>>) ?? [];
    return sessions.map(
      (s): SessionInfo => ({
        sessionId: String(s.session_id ?? ""),
        sessionRef: s.session_ref != null ? String(s.session_ref) : undefined,
        createdAt: String(s.created_at ?? ""),
        updatedAt: String(s.updated_at ?? ""),
        messageCount: Number(s.message_count ?? 0),
        totalTokens: Number(s.total_tokens ?? 0),
        isActive: Boolean(s.is_active),
      }),
    );
  }

  async readSession(sessionId: string): Promise<Record<string, unknown>> {
    return this.request("session/read", { session_id: sessionId });
  }

  // -- Capabilities -------------------------------------------------------

  get capabilities(): readonly Capability[] {
    return this._capabilities;
  }

  hasCapability(capabilityId: string): boolean {
    return this._capabilities.some(
      (c) => c.id === capabilityId && c.status === "Available",
    );
  }

  requireCapability(capabilityId: string): void {
    if (!this.hasCapability(capabilityId)) {
      throw new CapabilityUnavailableError(
        "CAPABILITY_UNAVAILABLE",
        `Capability '${capabilityId}' is not available`,
      );
    }
  }

  // -- Config -------------------------------------------------------------

  async getConfig(): Promise<Record<string, unknown>> {
    return this.request("config/get", {});
  }

  async setConfig(config: Record<string, unknown>): Promise<void> {
    await this.request("config/set", config);
  }

  async patchConfig(patch: Record<string, unknown>): Promise<Record<string, unknown>> {
    return this.request("config/patch", patch);
  }

  // -- Internal methods used by Session -----------------------------------

  /** @internal */
  async _startTurn(
    sessionId: string,
    prompt: string,
    options?: { skillRefs?: SkillRef[]; skillReferences?: string[] },
  ): Promise<RunResult> {
    const params: Record<string, unknown> = { session_id: sessionId, prompt };
    const wireRefs = skillRefsToWire(options?.skillRefs);
    if (wireRefs) {
      params.skill_refs = wireRefs;
    }
    if (options?.skillReferences) {
      params.skill_references = options.skillReferences;
    }
    const raw = await this.request("turn/start", params);
    return MeerkatClient.parseRunResult(raw);
  }

  /** @internal */
  _startTurnStreaming(
    sessionId: string,
    prompt: string,
    options?: { skillRefs?: SkillRef[]; skillReferences?: string[] },
    session?: Session,
  ): EventStream {
    if (!this.process?.stdin) {
      throw new MeerkatError("NOT_CONNECTED", "Client not connected");
    }

    this.requestId++;
    const requestId = this.requestId;

    const queue = new AsyncQueue<Record<string, unknown> | null>();
    this.eventQueues.set(sessionId, queue);

    const responsePromise = this.registerRequest(requestId);
    const params: Record<string, unknown> = { session_id: sessionId, prompt };
    const wireRefs = skillRefsToWire(options?.skillRefs);
    if (wireRefs) {
      params.skill_refs = wireRefs;
    }
    if (options?.skillReferences) {
      params.skill_references = options.skillReferences;
    }

    const rpcRequest = { jsonrpc: "2.0", id: requestId, method: "turn/start", params };
    this.process.stdin!.write(JSON.stringify(rpcRequest) + "\n");

    return new EventStream({
      sessionId,
      eventQueue: queue,
      responsePromise,
      parseResult: MeerkatClient.parseRunResult,
      session,
    });
  }

  /** @internal */
  async _interrupt(sessionId: string): Promise<void> {
    await this.request("turn/interrupt", { session_id: sessionId });
  }

  /** @internal */
  async _archive(sessionId: string): Promise<void> {
    await this.request("session/archive", { session_id: sessionId });
  }

  /** @internal */
  async _send(
    sessionId: string,
    command: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.request("comms/send", { session_id: sessionId, ...command });
  }

  /** @internal */
  async _peers(
    sessionId: string,
  ): Promise<Record<string, unknown>> {
    return this.request("comms/peers", { session_id: sessionId });
  }

  // -- Transport ----------------------------------------------------------

  private handleLine(line: string): void {
    let data: Record<string, unknown>;
    try {
      data = JSON.parse(line);
    } catch {
      return;
    }

    if ("id" in data && typeof data.id === "number") {
      const pending = this.pendingRequests.get(data.id);
      if (pending) {
        this.pendingRequests.delete(data.id);
        const error = data.error as Record<string, unknown> | null | undefined;
        if (error) {
          pending.reject(
            new MeerkatError(
              String(error.code ?? "UNKNOWN"),
              String(error.message ?? "Unknown error"),
            ),
          );
        } else {
          pending.resolve((data.result ?? {}) as Record<string, unknown>);
        }
      }
    } else if ("method" in data) {
      const params = (data.params ?? {}) as Record<string, unknown>;
      const sessionId = String(params.session_id ?? "");
      const event = params.event as Record<string, unknown> | undefined;
      if (event) {
        const queue = this.eventQueues.get(sessionId);
        if (queue) {
          queue.put(event);
        }
      }
    }
  }

  private request(method: string, params: unknown): Promise<Record<string, unknown>> {
    if (!this.process?.stdin) {
      throw new MeerkatError("NOT_CONNECTED", "Client not connected");
    }

    this.requestId++;
    const id = this.requestId;
    const rpcRequest = { jsonrpc: "2.0", id, method, params };
    const promise = this.registerRequest(id);
    this.process.stdin!.write(JSON.stringify(rpcRequest) + "\n");
    return promise;
  }

  private registerRequest(id: number): Promise<Record<string, unknown>> {
    return new Promise((resolve, reject) => {
      this.pendingRequests.set(id, { resolve, reject });
    });
  }

  // -- Static helpers -----------------------------------------------------

  private static normalizeStatus(raw: unknown): string {
    if (typeof raw === "string") return raw;
    if (typeof raw === "object" && raw !== null) {
      return Object.keys(raw)[0] ?? "Unknown";
    }
    return String(raw);
  }

  private static checkVersionCompatible(server: string, client: string): boolean {
    try {
      const s = server.split(".").map(Number);
      const c = client.split(".").map(Number);
      if (s[0] === 0 && c[0] === 0) return s[1] === c[1];
      return s[0] === c[0];
    } catch {
      return false;
    }
  }

  static parseRunResult(data: Record<string, unknown>): RunResult {
    const usageRaw = data.usage as Record<string, unknown> | undefined;
    const usage: Usage = {
      inputTokens: Number(usageRaw?.input_tokens ?? 0),
      outputTokens: Number(usageRaw?.output_tokens ?? 0),
      cacheCreationTokens: usageRaw?.cache_creation_tokens != null
        ? Number(usageRaw.cache_creation_tokens)
        : undefined,
      cacheReadTokens: usageRaw?.cache_read_tokens != null
        ? Number(usageRaw.cache_read_tokens)
        : undefined,
    };

    const rawWarnings = data.schema_warnings as Array<Record<string, unknown>> | undefined;
    const schemaWarnings: SchemaWarning[] | undefined = rawWarnings?.map(
      (w): SchemaWarning => ({
        provider: String(w.provider ?? ""),
        path: String(w.path ?? ""),
        message: String(w.message ?? ""),
      }),
    );

    return {
      sessionId: String(data.session_id ?? ""),
      sessionRef: data.session_ref != null ? String(data.session_ref) : undefined,
      text: String(data.text ?? ""),
      turns: Number(data.turns ?? 0),
      toolCalls: Number(data.tool_calls ?? 0),
      usage,
      structuredOutput: data.structured_output,
      schemaWarnings,
      skillDiagnostics: data.skill_diagnostics,
    };
  }

  private static buildCreateParams(
    prompt: string,
    options?: SessionOptions,
  ): Record<string, unknown> {
    const params: Record<string, unknown> = { prompt };
    if (!options) return params;

    if (options.model) params.model = options.model;
    if (options.provider) params.provider = options.provider;
    if (options.systemPrompt) params.system_prompt = options.systemPrompt;
    if (options.maxTokens) params.max_tokens = options.maxTokens;
    if (options.outputSchema) params.output_schema = options.outputSchema;
    if (options.structuredOutputRetries != null && options.structuredOutputRetries !== 2) {
      params.structured_output_retries = options.structuredOutputRetries;
    }
    if (options.hooksOverride) params.hooks_override = options.hooksOverride;
    if (options.enableBuiltins) params.enable_builtins = true;
    if (options.enableShell) params.enable_shell = true;
    if (options.enableSubagents) params.enable_subagents = true;
    if (options.enableMemory) params.enable_memory = true;
    if (options.hostMode) params.host_mode = true;
    if (options.commsName) params.comms_name = options.commsName;
    if (options.peerMeta != null) params.peer_meta = options.peerMeta;
    if (options.providerParams) params.provider_params = options.providerParams;
    if (options.preloadSkills != null) params.preload_skills = options.preloadSkills;
    const wireRefs = skillRefsToWire(options.skillRefs);
    if (wireRefs) params.skill_refs = wireRefs;
    if (options.skillReferences != null) params.skill_references = options.skillReferences;
    return params;
  }

  // -- Binary resolution --------------------------------------------------

  private static commandPath(command: string): string | null {
    if (path.isAbsolute(command)) {
      return existsSync(command) ? command : null;
    }
    const pathEnv = process.env.PATH ?? "";
    if (!pathEnv) return null;
    const exts =
      process.platform === "win32"
        ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT;.COM")
            .split(";")
            .filter((ext: string) => ext.length > 0)
        : [""];
    for (const dir of pathEnv.split(path.delimiter)) {
      if (!dir) continue;
      for (const ext of exts) {
        const candidate =
          process.platform === "win32" &&
          ext &&
          !command.toLowerCase().endsWith(ext.toLowerCase())
            ? `${command}${ext}`
            : command;
        const fullPath = path.join(dir, candidate);
        if (existsSync(fullPath)) return fullPath;
      }
    }
    return null;
  }

  private static resolveCandidatePath(commandOrPath: string): string | null {
    if (commandOrPath.includes(path.sep) || commandOrPath.includes("/")) {
      const candidate = path.resolve(commandOrPath);
      return existsSync(candidate) ? candidate : null;
    }
    return MeerkatClient.commandPath(commandOrPath);
  }

  private static platformTarget(): PlatformTarget {
    const architecture = os.arch();
    if (process.platform === "darwin") {
      if (architecture === "arm64") {
        return { target: "aarch64-apple-darwin", archiveExt: "tar.gz", binaryName: "rkat-rpc" };
      }
      throw new MeerkatError("UNSUPPORTED_PLATFORM", `Unsupported macOS architecture '${architecture}'.`);
    }
    if (process.platform === "linux") {
      if (architecture === "x64") return { target: "x86_64-unknown-linux-gnu", archiveExt: "tar.gz", binaryName: "rkat-rpc" };
      if (architecture === "arm64") return { target: "aarch64-unknown-linux-gnu", archiveExt: "tar.gz", binaryName: "rkat-rpc" };
      throw new MeerkatError("UNSUPPORTED_PLATFORM", `Unsupported Linux architecture '${architecture}'.`);
    }
    if (process.platform === "win32") {
      if (architecture === "x64") return { target: "x86_64-pc-windows-msvc", archiveExt: "zip", binaryName: "rkat-rpc.exe" };
      throw new MeerkatError("UNSUPPORTED_PLATFORM", `Unsupported Windows architecture '${architecture}'.`);
    }
    throw new MeerkatError("UNSUPPORTED_PLATFORM", `Unsupported platform '${process.platform}'.`);
  }

  private static async runCommand(command: string, args: string[]): Promise<void> {
    return new Promise((resolve, reject) => {
      const proc = spawn(command, args, { stdio: ["ignore", "inherit", "pipe"] });
      let stderr = "";
      proc.stderr?.on("data", (chunk) => { stderr += String(chunk); });
      proc.on("error", (error) => { reject(error); });
      proc.on("close", (code) => {
        if (code === 0) { resolve(); return; }
        reject(new MeerkatError("ARCHIVE_EXTRACT_FAILED", `${command} failed with code ${code}: ${stderr.trim()}`));
      });
    });
  }

  private static async extractZip(archivePath: string, destinationDir: string): Promise<void> {
    try {
      await MeerkatClient.runCommand("tar", ["-xf", archivePath, "-C", destinationDir]);
    } catch {
      await MeerkatClient.runCommand("powershell", [
        "-NoProfile", "-Command",
        `Expand-Archive -LiteralPath '${archivePath.replaceAll("'", "''")}' -DestinationPath '${destinationDir.replaceAll("'", "''")}' -Force`,
      ]);
    }
  }

  private static async ensureDownloadedBinary(): Promise<string> {
    const { target, archiveExt, binaryName } = MeerkatClient.platformTarget();
    const version = CONTRACT_VERSION;
    const asset = `${MEERKAT_RELEASE_BINARY}-v${version}-${target}.${archiveExt}`;
    const url = `https://github.com/${MEERKAT_REPO}/releases/download/v${version}/${asset}`;
    const baseDir = path.join(MEERKAT_BINARY_CACHE_ROOT, `v${version}`, target);
    mkdirSync(baseDir, { recursive: true });
    const cached = path.join(baseDir, binaryName);
    if (existsSync(cached)) return cached;

    const response = await fetch(url);
    if (!response.ok) {
      throw new MeerkatError("BINARY_DOWNLOAD_FAILED", `Failed to download binary from ${url} (HTTP ${response.status})`);
    }
    const arrayBuffer = await response.arrayBuffer();
    const archivePath = path.join(baseDir, asset);
    writeFileSync(archivePath, Buffer.from(arrayBuffer));

    try {
      if (archiveExt === "tar.gz") {
        await MeerkatClient.runCommand("tar", ["-xzf", archivePath, "-C", baseDir]);
      } else {
        await MeerkatClient.extractZip(archivePath, baseDir);
      }
    } catch (error) {
      throw new MeerkatError("BINARY_DOWNLOAD_EXTRACT_FAILED", `Failed to extract ${archivePath}: ${String(error)}`);
    } finally {
      try { unlinkSync(archivePath); } catch { /* best-effort cleanup */ }
    }

    if (process.platform !== "win32") chmodSync(cached, 0o755);
    return cached;
  }

  private static async resolveBinaryPath(requestedPath: string): Promise<ResolvedBinary> {
    const overridden = process.env.MEERKAT_BIN_PATH?.trim();
    if (overridden) {
      const candidate = MeerkatClient.resolveCandidatePath(overridden);
      if (!candidate) {
        throw new MeerkatError("BINARY_NOT_FOUND", `Binary not found at MEERKAT_BIN_PATH='${overridden}'.`);
      }
      return { command: candidate, useLegacySubcommand: path.basename(candidate) === "rkat" };
    }

    if (requestedPath !== "rkat-rpc") {
      const candidate = MeerkatClient.resolveCandidatePath(requestedPath);
      if (!candidate) {
        throw new MeerkatError("BINARY_NOT_FOUND", `Binary not found at '${requestedPath}'.`);
      }
      return { command: candidate, useLegacySubcommand: path.basename(candidate) === "rkat" };
    }

    const defaultBinary = MeerkatClient.commandPath("rkat-rpc");
    if (defaultBinary) return { command: defaultBinary, useLegacySubcommand: false };

    try {
      const downloaded = await MeerkatClient.ensureDownloadedBinary();
      return { command: downloaded, useLegacySubcommand: false };
    } catch (error) {
      const legacy = MeerkatClient.commandPath("rkat");
      if (legacy) return { command: legacy, useLegacySubcommand: true };
      if (error instanceof MeerkatError) throw error;
      throw new MeerkatError("BINARY_NOT_FOUND", `Could not find '${MEERKAT_RELEASE_BINARY}' and auto-download failed.`);
    }
  }

  private static buildArgs(legacy: boolean, options?: ConnectOptions): string[] {
    if (legacy) return ["rpc"];
    if (!options) return [];
    const args: string[] = [];
    if (options.isolated) args.push("--isolated");
    if (options.realmId) args.push("--realm", options.realmId);
    if (options.instanceId) args.push("--instance", options.instanceId);
    if (options.realmBackend) args.push("--realm-backend", options.realmBackend);
    if (options.stateRoot) args.push("--state-root", options.stateRoot);
    if (options.contextRoot) args.push("--context-root", options.contextRoot);
    if (options.userConfigRoot) args.push("--user-config-root", options.userConfigRoot);
    return args;
  }
}
