import type {
  WireMemberReloadRefusal,
  WireMobDestroyMemberRefusal,
  WireMobDestroyReport,
} from "./generated/types.js";

// Checked against the generated report at compile time: a missing or extra
// field fails the build until it is parsed here.
const DESTROY_REPORT_FIELDS = {
  force_destroyed_members: "strings",
  orphaned_remote_members: "strings",
  remote_cleanup_deadline_exceeded: "flag",
  metadata_scrubbed: "flag",
  events_cleared: "flag",
  namespace_cleaned: "flag",
  errors: "strings",
  reloaded_members: "strings",
  member_reload_refusals: "refusals",
} as const satisfies Record<keyof WireMobDestroyReport, "strings" | "flag" | "refusals">;

// Checked against the generated union at compile time, like the fields.
const MEMBER_RELOAD_REFUSAL_KINDS = {
  repair_blocked: true,
  unknown: true,
} as const satisfies Record<WireMemberReloadRefusal["kind"], true>;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseMemberReloadRefusal(value: unknown): WireMemberReloadRefusal | undefined {
  if (!isRecord(value) || typeof value.kind !== "string") return undefined;
  if (!Object.prototype.hasOwnProperty.call(MEMBER_RELOAD_REFUSAL_KINDS, value.kind)) {
    // A kind this SDK does not know: the server's own `Unknown` form.
    return { kind: "unknown" };
  }
  const kind = value.kind as WireMemberReloadRefusal["kind"];
  switch (kind) {
    case "repair_blocked": {
      const reason = value.reason;
      const digest = value.evidence_digest;
      if (typeof reason !== "string") return undefined;
      if (digest === undefined || digest === null) return { kind, reason };
      return typeof digest === "string" ? { kind, reason, evidence_digest: digest } : undefined;
    }
    case "unknown":
      return { kind };
    default: {
      const unparsed: never = kind;
      return unparsed;
    }
  }
}

function parseMobDestroyMemberRefusal(value: unknown): WireMobDestroyMemberRefusal | undefined {
  if (!isRecord(value) || typeof value.member !== "string") return undefined;
  const refusal = parseMemberReloadRefusal(value.refusal);
  return refusal === undefined ? undefined : { member: value.member, refusal };
}

/**
 * Parse a mob destroy report off the wire: the known fields, each checked
 * (an absent one stays absent, a mistyped one fails the parse), with a
 * refusal `kind` this SDK does not know read as `{ kind: "unknown" }`.
 * `undefined` when the report does not parse.
 *
 * @internal
 */
export function parseMobDestroyReport(value: unknown): WireMobDestroyReport | undefined {
  if (!isRecord(value)) return undefined;
  const report: Record<string, unknown> = {};
  for (const [field, shape] of Object.entries(DESTROY_REPORT_FIELDS)) {
    if (!Object.prototype.hasOwnProperty.call(value, field)) continue;
    const raw = value[field];
    if (shape === "flag") {
      if (typeof raw !== "boolean") return undefined;
      report[field] = raw;
    } else if (shape === "strings") {
      if (!Array.isArray(raw) || !raw.every((item) => typeof item === "string")) return undefined;
      report[field] = [...raw];
    } else {
      if (!Array.isArray(raw)) return undefined;
      const refusals: WireMobDestroyMemberRefusal[] = [];
      for (const entry of raw) {
        const refusal = parseMobDestroyMemberRefusal(entry);
        if (refusal === undefined) return undefined;
        refusals.push(refusal);
      }
      report[field] = refusals;
    }
  }
  return report as WireMobDestroyReport;
}

/**
 * The destroy report of a mob destroy that did not finish.
 *
 * `mobDestroy`, and `mobLifecycle` with `"destroy"`, fail with `error.data`
 * `{code: "mob_destroy_incomplete", destroy_report, retryable: true}`, which
 * the SDK keeps as `details`. The report says what is left; for example,
 * `member_reload_refusals` names each member whose reload the runtime
 * refused, with its typed `refusal`: retry the destroy later. A refusal
 * `kind` this SDK does not know reads as `{ kind: "unknown" }`. `undefined`
 * for any other error, or when the report does not parse.
 */
export function mobDestroyIncompleteReport(error: unknown): WireMobDestroyReport | undefined {
  if (typeof error !== "object" || error === null) return undefined;
  const details = (error as { details?: unknown }).details;
  if (!isRecord(details) || details.code !== "mob_destroy_incomplete") return undefined;
  return parseMobDestroyReport(details.destroy_report);
}
