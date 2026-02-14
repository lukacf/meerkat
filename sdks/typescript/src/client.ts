/**
 * Meerkat client — spawns rkat rpc subprocess and communicates via JSON-RPC.
 */

import { spawn, type ChildProcess } from "node:child_process";
import { createInterface } from "node:readline";
import { MeerkatError } from "./generated/errors.js";
import type {
  WireRunResult,
  CapabilitiesResponse,
  CapabilityEntry,
} from "./generated/types.js";
import { CONTRACT_VERSION } from "./generated/types.js";

/** Supplementary discovery metadata for a comms peer. */
export interface PeerMeta {
  description?: string;
  labels?: Record<string, string>;
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

  constructor(rkatPath = "rkat") {
    this.rkatPath = rkatPath;
  }

  async connect(): Promise<this> {
    this.process = spawn(this.rkatPath, ["rpc"], {
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
    options?: { skill_references?: string[] },
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
