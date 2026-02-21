/**
 * Meerkat client — spawns rkat-rpc subprocess and communicates via JSON-RPC.
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
import { createInterface } from "node:readline";
import { MeerkatError } from "./generated/errors.js";
import type {
  WireRunResult,
  CapabilitiesResponse,
  CapabilityEntry,
} from "./generated/types.js";
import { CONTRACT_VERSION } from "./generated/types.js";

const MEERKAT_REPO = "lukacf/meerkat";
const MEERKAT_RELEASE_BINARY = "rkat-rpc";
const MEERKAT_BINARY_CACHE_ROOT = path.join(
  os.homedir(),
  ".cache",
  "meerkat",
  "bin",
  MEERKAT_RELEASE_BINARY,
);

/** Supplementary discovery metadata for a comms peer. */
export interface PeerMeta {
  description?: string;
  labels?: Record<string, string>;
}

interface ResolvedBinary {
  command: string;
  useLegacySubcommand: boolean;
}

interface PlatformTarget {
  target: string;
  archiveExt: "tar.gz" | "zip";
  binaryName: string;
}

export class MeerkatClient {
  private process: ChildProcess | null = null;
  private requestId = 0;
  private capabilities: CapabilitiesResponse | null = null;
  private rkatPath: string;
  private pendingRequests = new Map<
    number,
    { resolve: (value: unknown) => void; reject: (reason: unknown) => void }
  >();

  constructor(rkatPath = "rkat-rpc") {
    this.rkatPath = rkatPath;
  }

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
        return {
          target: "aarch64-apple-darwin",
          archiveExt: "tar.gz",
          binaryName: "rkat-rpc",
        };
      }
      throw new MeerkatError(
        "UNSUPPORTED_PLATFORM",
        `Unsupported macOS architecture '${architecture}'. Only Apple Silicon (arm64) is supported.`,
      );
    }
    if (process.platform === "linux") {
      if (architecture === "x64") {
        return {
          target: "x86_64-unknown-linux-gnu",
          archiveExt: "tar.gz",
          binaryName: "rkat-rpc",
        };
      }
      if (architecture === "arm64") {
        return {
          target: "aarch64-unknown-linux-gnu",
          archiveExt: "tar.gz",
          binaryName: "rkat-rpc",
        };
      }
      throw new MeerkatError(
        "UNSUPPORTED_PLATFORM",
        `Unsupported Linux architecture '${architecture}'.`,
      );
    }
    if (process.platform === "win32") {
      if (architecture === "x64") {
        return {
          target: "x86_64-pc-windows-msvc",
          archiveExt: "zip",
          binaryName: "rkat-rpc.exe",
        };
      }
      throw new MeerkatError(
        "UNSUPPORTED_PLATFORM",
        `Unsupported Windows architecture '${architecture}'.`,
      );
    }
    throw new MeerkatError(
      "UNSUPPORTED_PLATFORM",
      `Unsupported platform '${process.platform}'.`,
    );
  }

  private static async runCommand(
    command: string,
    args: string[],
  ): Promise<void> {
    return new Promise((resolve, reject) => {
      const proc = spawn(command, args, { stdio: ["ignore", "inherit", "pipe"] });
      let stderr = "";
      proc.stderr?.on("data", (chunk) => {
        stderr += String(chunk);
      });
      proc.on("error", (error) => {
        reject(error);
      });
      proc.on("close", (code) => {
        if (code === 0) {
          resolve();
          return;
        }
        reject(
          new MeerkatError(
            "ARCHIVE_EXTRACT_FAILED",
            `${command} failed with code ${code}: ${stderr.trim()}`,
          ),
        );
      });
    });
  }

  private static async extractZip(archivePath: string, destinationDir: string): Promise<void> {
    try {
      await MeerkatClient.runCommand("tar", ["-xf", archivePath, "-C", destinationDir]);
      return;
    } catch (error) {
      await MeerkatClient.runCommand("powershell", [
        "-NoProfile",
        "-Command",
        `Expand-Archive -LiteralPath '${archivePath.replaceAll("'", "''")}' -DestinationPath '${destinationDir.replaceAll(
          "'",
          "''",
        )}' -Force`,
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
    if (existsSync(cached)) {
      return cached;
    }

    const response = await fetch(url);
    if (!response.ok) {
      throw new MeerkatError(
        "BINARY_DOWNLOAD_FAILED",
        `Failed to download binary from ${url} (HTTP ${response.status})`,
      );
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
      throw new MeerkatError(
        "BINARY_DOWNLOAD_EXTRACT_FAILED",
        `Failed to extract ${archivePath}: ${String(error)}`,
      );
    } finally {
      try { unlinkSync(archivePath); } catch { /* best-effort cleanup */ }
    }

    if (process.platform !== "win32") {
      chmodSync(cached, 0o755);
    }
    return cached;
  }

  private static async resolveBinaryPath(requestedPath: string): Promise<ResolvedBinary> {
    const overridden = process.env.MEERKAT_BIN_PATH?.trim();
    if (overridden) {
      const candidate = MeerkatClient.resolveCandidatePath(overridden);
      if (!candidate) {
        throw new MeerkatError(
          "BINARY_NOT_FOUND",
          `Binary not found at MEERKAT_BIN_PATH='${overridden}'. Set it to a valid executable.`,
        );
      }
      return {
        command: candidate,
        useLegacySubcommand: path.basename(candidate) === "rkat",
      };
    }

    if (requestedPath !== "rkat-rpc") {
      const candidate = MeerkatClient.resolveCandidatePath(requestedPath);
      if (!candidate) {
        throw new MeerkatError(
          "BINARY_NOT_FOUND",
          `Binary not found at '${requestedPath}'. Set rkatPath to a valid path or use MEERKAT_BIN_PATH.`,
        );
      }
      return {
        command: candidate,
        useLegacySubcommand: path.basename(candidate) === "rkat",
      };
    }

    const defaultBinary = MeerkatClient.commandPath("rkat-rpc");
    if (defaultBinary) {
      return {
        command: defaultBinary,
        useLegacySubcommand: false,
      };
    }

    try {
      const downloaded = await MeerkatClient.ensureDownloadedBinary();
      return { command: downloaded, useLegacySubcommand: false };
    } catch (error) {
      const legacy = MeerkatClient.commandPath("rkat");
      if (legacy) {
        return { command: legacy, useLegacySubcommand: true };
      }
      if (error instanceof MeerkatError) throw error;
      throw new MeerkatError(
        "BINARY_NOT_FOUND",
        `Could not find '${MEERKAT_RELEASE_BINARY}' and auto-download failed.`,
      );
    }
  }

  async connect(options?: {
    realmId?: string;
    instanceId?: string;
    realmBackend?: "jsonl" | "redb";
    isolated?: boolean;
    stateRoot?: string;
    contextRoot?: string;
    userConfigRoot?: string;
  }): Promise<this> {
    if (options?.realmId && options?.isolated) {
      throw new MeerkatError(
        "INVALID_ARGS",
        "realmId and isolated cannot both be set",
      );
    }
    const resolved = await MeerkatClient.resolveBinaryPath(this.rkatPath);
    this.rkatPath = resolved.command;
    const args: string[] = [];
    const legacyRequiresNewBinary = Boolean(
      options?.isolated ||
        options?.realmId ||
        options?.instanceId ||
        options?.realmBackend ||
        options?.stateRoot ||
        options?.contextRoot ||
        options?.userConfigRoot,
    );
    if (resolved.useLegacySubcommand) {
      if (legacyRequiresNewBinary) {
        throw new MeerkatError(
          "LEGACY_BINARY_UNSUPPORTED",
          "Realm/context options require the standalone rkat-rpc binary. Install rkat-rpc and retry.",
        );
      }
      args.push("rpc");
    } else {
      if (options?.isolated) {
        args.push("--isolated");
      }
      if (options?.realmId) {
        args.push("--realm", options.realmId);
      }
      if (options?.instanceId) {
        args.push("--instance", options.instanceId);
      }
      if (options?.realmBackend) {
        args.push("--realm-backend", options.realmBackend);
      }
      if (options?.stateRoot) {
        args.push("--state-root", options.stateRoot);
      }
      if (options?.contextRoot) {
        args.push("--context-root", options.contextRoot);
      }
      if (options?.userConfigRoot) {
        args.push("--user-config-root", options.userConfigRoot);
      }
    }
    this.process = spawn(this.rkatPath, args, {
      stdio: ["pipe", "pipe", "pipe"],
    });

    const rl = createInterface({ input: this.process.stdout! });
    rl.on("line", (line: string) => {
      try {
        const response = JSON.parse(line) as Record<string, unknown>;
        if (
          "id" in response &&
          typeof response.id === "number" &&
          this.pendingRequests.has(response.id)
        ) {
          const pending = this.pendingRequests.get(response.id)!;
          this.pendingRequests.delete(response.id);
          const error = response.error as
            | Record<string, unknown>
            | null
            | undefined;
          if (error) {
            pending.reject(
              new MeerkatError(
                String(error.code ?? "UNKNOWN"),
                String(error.message ?? "Unknown error"),
              ),
            );
          } else {
            pending.resolve(response.result ?? {});
          }
        }
      } catch {
        // Ignore non-JSON lines
      }
    });

    // Initialize
    const initResult = (await this.request("initialize", {})) as Record<
      string,
      unknown
    >;
    const serverVersion = String(initResult.contract_version ?? "");
    if (!this.checkVersionCompatible(serverVersion, CONTRACT_VERSION)) {
      throw new MeerkatError(
        "VERSION_MISMATCH",
        `Server version ${serverVersion} incompatible with SDK ${CONTRACT_VERSION}`,
      );
    }

    // Fetch capabilities
    const capsResult = (await this.request("capabilities/get", {})) as Record<
      string,
      unknown
    >;
    this.capabilities = {
      contract_version: String(capsResult.contract_version ?? ""),
      capabilities: (
        (capsResult.capabilities as Array<Record<string, unknown>>) ?? []
      ).map(
        (cap): CapabilityEntry => ({
          id: String(cap.id ?? ""),
          description: String(cap.description ?? ""),
          status: MeerkatClient.normalizeStatus(cap.status),
        }),
      ),
    };

    return this;
  }

  async close(): Promise<void> {
    if (this.process) {
      this.process.kill();
      this.process = null;
    }
  }

  async createSession(params: {
    prompt: string;
    model?: string;
    provider?: string;
    system_prompt?: string;
    max_tokens?: number;
    output_schema?: Record<string, unknown>;
    structured_output_retries?: number;
    hooks_override?: Record<string, unknown>;
    enable_builtins?: boolean;
    enable_shell?: boolean;
    enable_subagents?: boolean;
    enable_memory?: boolean;
    host_mode?: boolean;
    comms_name?: string;
    peer_meta?: PeerMeta;
    provider_params?: Record<string, unknown>;
    preload_skills?: string[];
    skill_refs?: Array<Record<string, unknown> | string>;
    skill_references?: string[];
  }): Promise<WireRunResult> {
    const result = (await this.request("session/create", params)) as Record<
      string,
      unknown
    >;
    return this.parseRunResult(result);
  }

  async startTurn(
    sessionId: string,
    prompt: string,
    options?: {
      skill_refs?: Array<Record<string, unknown> | string>;
      skill_references?: string[];
    },
  ): Promise<WireRunResult> {
    const result = (await this.request("turn/start", {
      session_id: sessionId,
      prompt,
      ...options,
    })) as Record<string, unknown>;
    return this.parseRunResult(result);
  }

  async interrupt(sessionId: string): Promise<void> {
    await this.request("turn/interrupt", { session_id: sessionId });
  }

  async listSessions(): Promise<unknown[]> {
    const result = (await this.request("session/list", {})) as Record<
      string,
      unknown
    >;
    return (result.sessions as unknown[]) ?? [];
  }

  async readSession(sessionId: string): Promise<Record<string, unknown>> {
    return (await this.request("session/read", {
      session_id: sessionId,
    })) as Record<string, unknown>;
  }

  async archiveSession(sessionId: string): Promise<void> {
    await this.request("session/archive", { session_id: sessionId });
  }

  async getCapabilities(): Promise<CapabilitiesResponse> {
    if (this.capabilities) return this.capabilities;
    const result = (await this.request("capabilities/get", {})) as Record<
      string,
      unknown
    >;
    return {
      contract_version: String(result.contract_version ?? ""),
      capabilities: [],
    };
  }

  hasCapability(capabilityId: string): boolean {
    if (!this.capabilities) return false;
    return this.capabilities.capabilities.some(
      (c) => c.id === capabilityId && c.status === "Available",
    );
  }

  requireCapability(capabilityId: string): void {
    if (!this.hasCapability(capabilityId)) {
      throw new MeerkatError(
        "CAPABILITY_UNAVAILABLE",
        `Capability '${capabilityId}' is not available`,
      );
    }
  }

  // --- Config ---

  async getConfig(): Promise<Record<string, unknown>> {
    return (await this.request("config/get", {})) as Record<string, unknown>;
  }

  async setConfig(config: Record<string, unknown>): Promise<void> {
    await this.request("config/set", config);
  }

  async patchConfig(patch: Record<string, unknown>): Promise<Record<string, unknown>> {
    return (await this.request("config/patch", patch)) as Record<
      string,
      unknown
    >;
  }

  /**
   * Send a canonical comms command to a session.
   *
   * @param sessionId - Target session ID
   * @param command - Command fields (kind, to, body, intent, params, etc.)
   * @returns Receipt info on success
   */
  async send(
    sessionId: string,
    command: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return (await this.request("comms/send", {
      session_id: sessionId,
      ...command,
    })) as Record<string, unknown>;
  }

  /**
   * Send a command and open a stream in one call.
   *
   * @param sessionId - Target session ID
   * @param command - Command fields (kind, to, body, etc.)
   * @returns Receipt and stream info
   */
  async sendAndStream(
    sessionId: string,
    command: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return (await this.request("comms/send", {
      session_id: sessionId,
      stream: "reserve_interaction",
      ...command,
    })) as Record<string, unknown>;
  }

  /**
   * List peers visible to a session's comms runtime.
   *
   * @param sessionId - Target session ID
   * @returns Object with `{ peers: [...] }`
   */
  async peers(
    sessionId: string,
  ): Promise<{ peers: Array<Record<string, unknown>> }> {
    return (await this.request("comms/peers", {
      session_id: sessionId,
    })) as { peers: Array<Record<string, unknown>> };
  }

  // --- Internal ---

  /**
   * Normalize a CapabilityStatus from the wire.
   * Available is the string "Available", but other variants are
   * externally-tagged objects like {"DisabledByPolicy": {"description": "..."}}.
   * We normalize to the variant name string.
   */
  private static normalizeStatus(raw: unknown): string {
    if (typeof raw === "string") return raw;
    if (typeof raw === "object" && raw !== null) {
      const keys = Object.keys(raw);
      return keys[0] ?? "Unknown";
    }
    return String(raw);
  }

  private request(method: string, params: unknown): Promise<unknown> {
    if (!this.process?.stdin) {
      throw new MeerkatError("NOT_CONNECTED", "Client not connected");
    }

    this.requestId++;
    const id = this.requestId;
    const request = {
      jsonrpc: "2.0",
      id,
      method,
      params,
    };

    return new Promise((resolve, reject) => {
      this.pendingRequests.set(id, { resolve, reject });
      this.process!.stdin!.write(JSON.stringify(request) + "\n");
    });
  }

  private checkVersionCompatible(server: string, client: string): boolean {
    try {
      const s = server.split(".").map(Number);
      const c = client.split(".").map(Number);
      if (s[0] === 0 && c[0] === 0) return s[1] === c[1];
      return s[0] === c[0];
    } catch {
      return false;
    }
  }

  private parseRunResult(data: Record<string, unknown>): WireRunResult {
    const usage = data.usage as Record<string, unknown> | undefined;
    return {
      session_id: String(data.session_id ?? ""),
      session_ref:
        data.session_ref == null ? undefined : String(data.session_ref),
      text: String(data.text ?? ""),
      turns: Number(data.turns ?? 0),
      tool_calls: Number(data.tool_calls ?? 0),
      usage: {
        input_tokens: Number(usage?.input_tokens ?? 0),
        output_tokens: Number(usage?.output_tokens ?? 0),
        total_tokens: Number(usage?.total_tokens ?? 0),
        cache_creation_tokens: usage?.cache_creation_tokens as
          | number
          | undefined,
        cache_read_tokens: usage?.cache_read_tokens as number | undefined,
      },
      structured_output: data.structured_output,
      schema_warnings: data.schema_warnings as
        | Array<{ provider: string; path: string; message: string }>
        | undefined,
    };
  }
}
