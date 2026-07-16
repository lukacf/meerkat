// Generated error types for Meerkat SDK

export interface WireScopeDeniedDetail {
  presented: ("list" | "read_history" | "subscribe_events" | "send_command" | "cancel" | "retire" | "wire_topology" | "live" | "admin_host" | "admin_grants")[];
  required: "list" | "read_history" | "subscribe_events" | "send_command" | "cancel" | "retire" | "wire_topology" | "live" | "admin_host" | "admin_grants";
}

export interface WireHostUnavailableDetail {
  host?: string | null;
  timeout_ms?: number | null;
}

export interface WireStaleCursorDetail {
  generation?: number | null;
  requested?: number | null;
  watermark: number;
}

export interface WireStaleFenceDetail {
  actual?: number | null;
  expected?: number | null;
  runtime_id?: string | null;
}

export type MultiHostErrorCode = "SCOPE_DENIED" | "HOST_UNAVAILABLE" | "STALE_CURSOR" | "STALE_FENCE";

export const MULTI_HOST_JSON_RPC_ERROR_CODES: Readonly<
  Partial<Record<number, MultiHostErrorCode>>
> = {
  [-32025]: "SCOPE_DENIED",
  [-32026]: "HOST_UNAVAILABLE",
  [-32027]: "STALE_CURSOR",
  [-32028]: "STALE_FENCE",
};

function isErrorDetailRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isScopeDeniedDetail(value: unknown): value is WireScopeDeniedDetail {
  if (!isErrorDetailRecord(value)) return false;
  const allowed = ["presented", "required"] as readonly string[];
  if (!Object.keys(value).every((key) => allowed.includes(key))) return false;
  return (
    Object.prototype.hasOwnProperty.call(value, "presented") && (Array.isArray(value.presented) && value.presented.every((item) => (item === "list" || item === "read_history" || item === "subscribe_events" || item === "send_command" || item === "cancel" || item === "retire" || item === "wire_topology" || item === "live" || item === "admin_host" || item === "admin_grants"))) &&
    Object.prototype.hasOwnProperty.call(value, "required") && (value.required === "list" || value.required === "read_history" || value.required === "subscribe_events" || value.required === "send_command" || value.required === "cancel" || value.required === "retire" || value.required === "wire_topology" || value.required === "live" || value.required === "admin_host" || value.required === "admin_grants")
  );
}

function isHostUnavailableDetail(value: unknown): value is WireHostUnavailableDetail {
  if (!isErrorDetailRecord(value)) return false;
  const allowed = ["host", "timeout_ms"] as readonly string[];
  if (!Object.keys(value).every((key) => allowed.includes(key))) return false;
  return (
    (!Object.prototype.hasOwnProperty.call(value, "host") || (typeof value.host === "string" || value.host === null)) &&
    (!Object.prototype.hasOwnProperty.call(value, "timeout_ms") || ((typeof value.timeout_ms === "number" && Number.isFinite(value.timeout_ms) && Number.isSafeInteger(value.timeout_ms) && value.timeout_ms >= 0) || value.timeout_ms === null))
  );
}

function isStaleCursorDetail(value: unknown): value is WireStaleCursorDetail {
  if (!isErrorDetailRecord(value)) return false;
  const allowed = ["generation", "requested", "watermark"] as readonly string[];
  if (!Object.keys(value).every((key) => allowed.includes(key))) return false;
  return (
    (!Object.prototype.hasOwnProperty.call(value, "generation") || ((typeof value.generation === "number" && Number.isFinite(value.generation) && Number.isSafeInteger(value.generation) && value.generation >= 0) || value.generation === null)) &&
    (!Object.prototype.hasOwnProperty.call(value, "requested") || ((typeof value.requested === "number" && Number.isFinite(value.requested) && Number.isSafeInteger(value.requested) && value.requested >= 0) || value.requested === null)) &&
    Object.prototype.hasOwnProperty.call(value, "watermark") && (typeof value.watermark === "number" && Number.isFinite(value.watermark) && Number.isSafeInteger(value.watermark) && value.watermark >= 0)
  );
}

function isStaleFenceDetail(value: unknown): value is WireStaleFenceDetail {
  if (!isErrorDetailRecord(value)) return false;
  const allowed = ["actual", "expected", "runtime_id"] as readonly string[];
  if (!Object.keys(value).every((key) => allowed.includes(key))) return false;
  return (
    (!Object.prototype.hasOwnProperty.call(value, "actual") || ((typeof value.actual === "number" && Number.isFinite(value.actual) && Number.isSafeInteger(value.actual) && value.actual >= 0) || value.actual === null)) &&
    (!Object.prototype.hasOwnProperty.call(value, "expected") || ((typeof value.expected === "number" && Number.isFinite(value.expected) && Number.isSafeInteger(value.expected) && value.expected >= 0) || value.expected === null)) &&
    (!Object.prototype.hasOwnProperty.call(value, "runtime_id") || (typeof value.runtime_id === "string" || value.runtime_id === null))
  );
}

export class MeerkatError extends Error {
  constructor(
    public readonly code: string,
    message: string,
    public readonly details?: unknown,
    public readonly capabilityHint?: { capability_id: string; message: string },
  ) {
    super(message);
    this.name = 'MeerkatError';
  }
}

export class CapabilityUnavailableError extends MeerkatError {
  constructor(code: string, message: string, details?: unknown, capabilityHint?: { capability_id: string; message: string }) {
    super(code, message, details, capabilityHint);
    this.name = 'CapabilityUnavailableError';
  }
}

export class SessionNotFoundError extends MeerkatError {
  constructor(code: string, message: string) {
    super(code, message);
    this.name = 'SessionNotFoundError';
  }
}

export class SkillNotFoundError extends MeerkatError {
  constructor(code: string, message: string) {
    super(code, message);
    this.name = 'SkillNotFoundError';
  }
}

export class ScopeDeniedError extends MeerkatError {
  declare public readonly details: WireScopeDeniedDetail;
  constructor(
    message: string,
    details: WireScopeDeniedDetail,
    capabilityHint?: { capability_id: string; message: string },
  ) {
    super("SCOPE_DENIED", message, details, capabilityHint);
    this.name = "ScopeDeniedError";
  }
}

export class HostUnavailableError extends MeerkatError {
  declare public readonly details: WireHostUnavailableDetail;
  constructor(
    message: string,
    details: WireHostUnavailableDetail,
    capabilityHint?: { capability_id: string; message: string },
  ) {
    super("HOST_UNAVAILABLE", message, details, capabilityHint);
    this.name = "HostUnavailableError";
  }
}

export class StaleCursorError extends MeerkatError {
  declare public readonly details: WireStaleCursorDetail;
  constructor(
    message: string,
    details: WireStaleCursorDetail,
    capabilityHint?: { capability_id: string; message: string },
  ) {
    super("STALE_CURSOR", message, details, capabilityHint);
    this.name = "StaleCursorError";
  }
}

export class StaleFenceError extends MeerkatError {
  declare public readonly details: WireStaleFenceDetail;
  constructor(
    message: string,
    details: WireStaleFenceDetail,
    capabilityHint?: { capability_id: string; message: string },
  ) {
    super("STALE_FENCE", message, details, capabilityHint);
    this.name = "StaleFenceError";
  }
}

export function meerkatErrorFromSemanticCode(
  code: string,
  message: string,
  details?: unknown,
  capabilityHint?: { capability_id: string; message: string },
): MeerkatError {
  if (code === "SCOPE_DENIED" && isScopeDeniedDetail(details)) {
    return new ScopeDeniedError(message, details, capabilityHint);
  }
  if (code === "HOST_UNAVAILABLE" && isHostUnavailableDetail(details)) {
    return new HostUnavailableError(message, details, capabilityHint);
  }
  if (code === "STALE_CURSOR" && isStaleCursorDetail(details)) {
    return new StaleCursorError(message, details, capabilityHint);
  }
  if (code === "STALE_FENCE" && isStaleFenceDetail(details)) {
    return new StaleFenceError(message, details, capabilityHint);
  }
  return new MeerkatError(code, message, details, capabilityHint);
}

function normalizeJsonRpcCode(code: number | string): number | undefined {
  if (typeof code === "number") {
    return Number.isInteger(code) ? code : undefined;
  }
  if (!/^-?\d+$/.test(code)) return undefined;
  const parsed = Number(code);
  return Number.isSafeInteger(parsed) && String(parsed) === code ? parsed : undefined;
}

export function meerkatErrorFromJsonRpcCode(
  rpcCode: number | string,
  semanticCode: string | undefined,
  message: string,
  details?: unknown,
  capabilityHint?: { capability_id: string; message: string },
): MeerkatError {
  const normalizedRpcCode = normalizeJsonRpcCode(rpcCode);
  const mappedSemantic =
    normalizedRpcCode === undefined
      ? undefined
      : MULTI_HOST_JSON_RPC_ERROR_CODES[normalizedRpcCode];
  if (semanticCode !== undefined) {
    if (mappedSemantic !== undefined && mappedSemantic !== semanticCode) {
      return new MeerkatError(semanticCode, message, details, capabilityHint);
    }
    return meerkatErrorFromSemanticCode(
      semanticCode,
      message,
      details,
      capabilityHint,
    );
  }
  if (mappedSemantic !== undefined) {
    return meerkatErrorFromSemanticCode(
      mappedSemantic,
      message,
      details,
      capabilityHint,
    );
  }
  return new MeerkatError(String(rpcCode), message, details, capabilityHint);
}
