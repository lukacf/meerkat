import type { ConsoleDockTarget } from "./dock";

export type MetaWorkbenchTarget =
  | MetaIdentityChatTarget
  | MetaIdentityInspectTarget
  | MetaControlWorkbenchTarget;

export interface MetaIdentityChatTarget extends ConsoleDockTarget {
  kind: "meta/identity-chat";
  identity: string;
  memberId?: string;
  addressingMode: "identity";
}

export interface MetaIdentityInspectTarget extends ConsoleDockTarget {
  kind: "meta/identity-inspect";
  identity: string;
  memberId?: string;
}

export type MetaControlTargetKind =
  | "meta/topology"
  | "meta/activity"
  | "meta/roster"
  | "meta/routing"
  | "meta/gating"
  | "meta/access"
  | "meta/memory"
  | "meta/workgraph"
  | "meta/logs";

export interface MetaControlWorkbenchTarget extends ConsoleDockTarget {
  kind: MetaControlTargetKind;
}

export interface HostWorkbenchTarget<TPayload = unknown> extends ConsoleDockTarget {
  kind: `${string}/${string}`;
  title: string;
  payloadVersion: number;
  payload?: TPayload;
  provenance: "host";
}

export type ConsoleWorkbenchTarget<THost = unknown> =
  | MetaWorkbenchTarget
  | HostWorkbenchTarget<THost>;

type UnknownRecord = Record<string, unknown>;

const LEGACY_CONTROL_TARGETS: Record<string, MetaControlTargetKind> = {
  topology: "meta/topology",
  health: "meta/activity",
  timeline: "meta/activity",
  roster: "meta/roster",
  routing: "meta/routing",
  gating: "meta/gating",
  gates: "meta/gating",
  access: "meta/access",
  memory: "meta/memory",
  workgraph: "meta/workgraph",
  logs: "meta/logs",
};

export function migrateConsoleWorkbenchTarget(
  input: unknown,
): ConsoleWorkbenchTarget | null {
  if (!isRecord(input)) {
    return null;
  }

  const id = stringValue(input.id);
  const kind = stringValue(input.kind);
  const title = stringValue(input.title);
  if (!id || !kind || !title) {
    return null;
  }

  if (kind === "agent-chat") {
    const identity = stringValue(input.identity) || stringValue(input.memberId) || id;
    return {
      ...baseTarget(input, id, "meta/identity-chat", title),
      identity,
      memberId: stringValue(input.memberId),
      addressingMode: "identity",
    };
  }

  if (kind === "identity-inspect") {
    const identity = stringValue(input.identity) || stringValue(input.memberId) || id.replace(/^inspect:/, "");
    return {
      ...baseTarget(input, id, "meta/identity-inspect", title),
      identity,
      memberId: stringValue(input.memberId),
    };
  }

  const controlKind = LEGACY_CONTROL_TARGETS[kind];
  if (controlKind) {
    return baseTarget(input, id, controlKind, title);
  }

  if (kind.startsWith("meta/")) {
    return normalizeMetaWorkbenchTarget(input, id, kind, title);
  }

  if (isNamespacedKind(kind) && kind !== "meta/unknown") {
    const payloadVersion = typeof input.payloadVersion === "number" && Number.isSafeInteger(input.payloadVersion)
      ? input.payloadVersion
      : 1;
    return {
      ...baseTarget(input, id, kind as `${string}/${string}`, title),
      payloadVersion,
      payload: input.payload,
      provenance: "host",
    };
  }

  return null;
}

function normalizeMetaWorkbenchTarget(
  input: UnknownRecord,
  id: string,
  kind: string,
  title: string,
): ConsoleWorkbenchTarget | null {
  if (kind === "meta/identity-chat") {
    const identity = stringValue(input.identity) || stringValue(input.memberId);
    if (!identity) return null;
    return {
      ...baseTarget(input, id, kind, title),
      identity,
      memberId: stringValue(input.memberId),
      addressingMode: "identity",
    };
  }

  if (kind === "meta/identity-inspect") {
    const identity = stringValue(input.identity) || stringValue(input.memberId);
    if (!identity) return null;
    return {
      ...baseTarget(input, id, kind, title),
      identity,
      memberId: stringValue(input.memberId),
    };
  }

  if (Object.values(LEGACY_CONTROL_TARGETS).includes(kind as MetaControlTargetKind)) {
    return baseTarget(input, id, kind as MetaControlTargetKind, title);
  }

  return null;
}

function baseTarget<TKind extends string>(
  input: UnknownRecord,
  id: string,
  kind: TKind,
  title: string,
): ConsoleDockTarget & { kind: TKind } {
  return {
    id,
    kind,
    title,
    subtitle: stringValue(input.subtitle),
    iconName: stringValue(input.iconName),
    badgeLabel: stringValue(input.badgeLabel),
  };
}

function isRecord(value: unknown): value is UnknownRecord {
  return Boolean(value && typeof value === "object" && !Array.isArray(value));
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function isNamespacedKind(kind: string): kind is `${string}/${string}` {
  const [namespace, name, ...rest] = kind.split("/");
  return Boolean(namespace && name && rest.length === 0);
}
