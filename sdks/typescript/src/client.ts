/**
 * Meerkat client — spawns rkat rpc subprocess and communicates via JSON-RPC.
 */

import { spawn, ChildProcess } from "node:child_process";
import { createInterface } from "node:readline";
import { MeerkatError } from "./generated/errors";
import type {
  WireRunResult,
  WireUsage,
  CapabilitiesResponse,
  CapabilityEntry,
} from "./generated/types";
import { CONTRACT_VERSION } from "./generated/types";

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
    rl.on("line", (line) => {
      try {
        const response = JSON.parse(line);
        if ("id" in response && this.pendingRequests.has(response.id)) {
          const pending = this.pendingRequests.get(response.id)!;
          this.pendingRequests.delete(response.id);
          if (response.error) {
            pending.reject(
              new MeerkatError(
                String(response.error.code),
                response.error.message,
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
      capabilities: ((capsResult.capabilities as unknown[]) ?? []).map(
        (c: unknown) => {
          const cap = c as Record<string, unknown>;
          return {
            id: String(cap.id ?? ""),
            description: String(cap.description ?? ""),
            status: String(cap.status ?? "available"),
          };
        },
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
    system_prompt?: string;
    max_tokens?: number;
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
  ): Promise<WireRunResult> {
    const result = (await this.request("turn/start", {
      session_id: sessionId,
      prompt,
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
      (c) => c.id === capabilityId && c.status === "available",
    );
  }

  // --- Internal ---

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
