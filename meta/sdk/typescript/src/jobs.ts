/**
 * Detached callback-job host contracts.
 *
 * Meerkat's generated job machine owns lifecycle and authority. These types
 * only run host work outside the callback and echo the exact committed
 * attempt authority on ordinary gateway RPC.
 */

export type JobRestartClass =
  | "adoptable"
  | "checkpoint_resumable"
  | "replayable"
  | "non_resumable";

export type JobIdempotencyScope =
  | "tool_call"
  | "interaction_and_arguments"
  | "host_semantic_key";

export interface DetachedJobAuthority {
  readonly jobId: string;
  readonly attemptId: string;
  readonly fence: number;
}

export interface DetachedJobRunner {
  run(context: DetachedJobContext): unknown | Promise<unknown>;
  reconcile?(attempt: Record<string, unknown>): boolean | Promise<boolean>;
  cancel?(authority: Record<string, unknown>): void | Promise<void>;
}

export type DetachedJobHandler =
  | ((context: DetachedJobContext) => unknown | Promise<unknown>)
  | DetachedJobRunner;

export type DetachedJobRpc = (
  method: string,
  params: Record<string, unknown>,
) => Promise<unknown>;

export type JobCredentialResolver = (
  profileName: string | null,
  scopes: readonly string[],
) => Readonly<Record<string, unknown>> | Promise<Readonly<Record<string, unknown>>>;

export class DetachedJobResult {
  constructor(readonly resultRef: string | null = null) {}
}

export interface DetachedJobExecutionOptions {
  readonly runner: string;
  readonly version: string;
  readonly restartClass: JobRestartClass;
  readonly idempotencyScope: JobIdempotencyScope;
  readonly submissionTimeoutMs: number;
  readonly credentialScopes?: readonly string[];
  readonly handler: DetachedJobHandler;
}

export class DetachedJobExecution {
  readonly runner: string;
  readonly version: string;
  readonly restartClass: JobRestartClass;
  readonly idempotencyScope: JobIdempotencyScope;
  readonly submissionTimeoutMs: number;
  readonly credentialScopes: readonly string[];
  readonly handler: DetachedJobHandler;

  constructor(options: DetachedJobExecutionOptions) {
    if (typeof options.runner !== "string" || options.runner.trim() === "") {
      throw new TypeError("detached runner must be a non-empty string");
    }
    if (typeof options.version !== "string" || options.version.trim() === "") {
      throw new TypeError("detached runner version must be a non-empty string");
    }
    if (
      !["adoptable", "checkpoint_resumable", "replayable", "non_resumable"]
        .includes(options.restartClass)
    ) {
      throw new TypeError(`unsupported restartClass: ${String(options.restartClass)}`);
    }
    if (
      !["tool_call", "interaction_and_arguments", "host_semantic_key"]
        .includes(options.idempotencyScope)
    ) {
      throw new TypeError(
        `unsupported idempotencyScope: ${String(options.idempotencyScope)}`,
      );
    }
    if (
      !Number.isSafeInteger(options.submissionTimeoutMs) ||
      options.submissionTimeoutMs < 1 ||
      options.submissionTimeoutMs > 120_000
    ) {
      throw new TypeError(
        "submissionTimeoutMs must be an integer in the public 1..=120000ms callback window",
      );
    }
    if (
      typeof options.handler !== "function" &&
      (typeof options.handler !== "object" ||
        options.handler === null ||
        typeof options.handler.run !== "function")
    ) {
      throw new TypeError("detached execution requires a function or runner.run");
    }
    const scopes = [...(options.credentialScopes ?? [])];
    if (scopes.some((scope) => typeof scope !== "string" || scope.trim() === "")) {
      throw new TypeError("credential scopes must be non-empty strings");
    }

    this.runner = options.runner;
    this.version = options.version;
    this.restartClass = options.restartClass;
    this.idempotencyScope = options.idempotencyScope;
    this.submissionTimeoutMs = options.submissionTimeoutMs;
    this.credentialScopes = scopes;
    this.handler = options.handler;
  }

  get runnerKey(): string {
    return `${this.runner}\u0000${this.version}`;
  }

  toWire(): Record<string, unknown> {
    const result: Record<string, unknown> = {
      mode: "detached",
      runner: { name: this.runner, version: this.version },
      restart_class: this.restartClass,
      idempotency_scope: this.idempotencyScope,
      submission_timeout_ms: this.submissionTimeoutMs,
    };
    if (this.credentialScopes.length > 0) {
      result.credential_scopes = [...this.credentialScopes];
    }
    return result;
  }
}

export function parseDetachedJobAuthority(
  raw: unknown,
): DetachedJobAuthority {
  if (typeof raw !== "object" || raw === null) {
    throw new Error("job callback requires authority");
  }
  const value = raw as Record<string, unknown>;
  const jobId = value.job_id;
  const attemptId = value.attempt_id;
  const fence = value.fence;
  if (typeof jobId !== "string" || jobId === "") {
    throw new Error("job authority requires a non-empty job_id");
  }
  if (typeof attemptId !== "string" || attemptId === "") {
    throw new Error("job authority requires a non-empty attempt_id");
  }
  if (!Number.isSafeInteger(fence) || Number(fence) < 1) {
    throw new Error("job authority requires a positive integer fence");
  }
  return { jobId, attemptId, fence: Number(fence) };
}

export function jobAuthorityToDict(
  authority: DetachedJobAuthority,
): Record<string, unknown> {
  return {
    job_id: authority.jobId,
    attempt_id: authority.attemptId,
    fence: authority.fence,
  };
}

export function sameJobAuthority(
  left: DetachedJobAuthority,
  right: DetachedJobAuthority,
): boolean {
  return (
    left.jobId === right.jobId &&
    left.attemptId === right.attemptId &&
    left.fence === right.fence
  );
}

/** @internal */
export class DetachedJobReporter {
  private terminal = false;

  constructor(
    private readonly authority: DetachedJobAuthority,
    private readonly rpc: DetachedJobRpc,
  ) {}

  private async send(
    method: string,
    params: Record<string, unknown>,
  ): Promise<unknown> {
    return this.rpc(method, {
      authority: jobAuthorityToDict(this.authority),
      ...params,
    });
  }

  async heartbeat(options: {
    leaseExpiresAtMs: number;
    heartbeatAtMs?: number;
  }): Promise<unknown> {
    return this.send("meta/jobs/heartbeat", {
      heartbeat_at_ms: options.heartbeatAtMs ?? Date.now(),
      lease_expires_at_ms: options.leaseExpiresAtMs,
    });
  }

  async progress(
    cursor: number,
    detail: string,
    options?: { observedAtMs?: number },
  ): Promise<unknown> {
    return this.send("meta/jobs/progress", {
      cursor,
      detail,
      observed_at_ms: options?.observedAtMs ?? Date.now(),
    });
  }

  async checkpoint(
    checkpointRef: string,
    options?: { observedAtMs?: number },
  ): Promise<unknown> {
    return this.send("meta/jobs/checkpoint", {
      checkpoint_ref: checkpointRef,
      observed_at_ms: options?.observedAtMs ?? Date.now(),
    });
  }

  async complete(resultRef: string | null): Promise<unknown> {
    if (this.terminal) return null;
    const params: Record<string, unknown> = { completed_at_ms: Date.now() };
    if (resultRef !== null) params.result_ref = resultRef;
    const result = await this.send("meta/jobs/complete", params);
    this.terminal = true;
    return result;
  }

  async fail(code: string, detailRef?: string): Promise<unknown> {
    if (this.terminal) return null;
    const params: Record<string, unknown> = {
      failed_at_ms: Date.now(),
      code,
    };
    if (detailRef !== undefined) params.detail_ref = detailRef;
    const result = await this.send("meta/jobs/fail", params);
    this.terminal = true;
    return result;
  }

  async cancelAck(): Promise<unknown> {
    if (this.terminal) return null;
    const result = await this.send("meta/jobs/cancel_ack", {
      acknowledged_at_ms: Date.now(),
    });
    this.terminal = true;
    return result;
  }
}

export class DetachedJobContext {
  readonly credentials: Readonly<Record<string, unknown>>;

  constructor(
    readonly authority: DetachedJobAuthority,
    readonly runnerHandle: string,
    private readonly _arguments: unknown,
    credentials: Readonly<Record<string, unknown>>,
    readonly resumeCheckpoint: string | null,
    readonly signal: AbortSignal,
    private readonly reporter: DetachedJobReporter,
  ) {
    this.credentials = { ...credentials };
  }

  get arguments(): unknown {
    return this._arguments;
  }

  heartbeat(options: {
    leaseExpiresAtMs: number;
    heartbeatAtMs?: number;
  }): Promise<unknown> {
    return this.reporter.heartbeat(options);
  }

  progress(
    cursor: number,
    detail: string,
    options?: { observedAtMs?: number },
  ): Promise<unknown> {
    return this.reporter.progress(cursor, detail, options);
  }

  checkpoint(
    checkpointRef: string,
    options?: { observedAtMs?: number },
  ): Promise<unknown> {
    return this.reporter.checkpoint(checkpointRef, options);
  }
}
