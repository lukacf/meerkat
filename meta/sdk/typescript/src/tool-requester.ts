/**
 * The original requester of the run a callback serves.
 *
 * The gateway names it on `callback/call_tool` and on `callback/job/start` as
 * `requester`, typed: the principal, and `carried` when the run's work reached
 * its agent through another actor (a peer hop or an approval). The runtime sets
 * it from the run's admitted inputs; tool arguments never do. A console send
 * from a person on a console that requires sign-in names that person; a run
 * without a requester names none.
 *
 * A tool handler receives it as its optional second argument, so a tool that
 * acts in one person's name can refuse every other requester:
 *
 * ```ts
 * opts.registerTool("read_mail", async (args, requester) => {
 *   if (requester === undefined || requester.carried ||
 *       requester.principal.kind !== "human" ||
 *       requester.principal.id !== OWNER_SUB ||
 *       requester.principal.trustDomainId !== OWNER_DOMAIN) {
 *     throw new Error("this mailbox serves its owner only");
 *   }
 *   // ...
 * });
 * ```
 */

/**
 * The principal a requester names: its category (`"human"` for a person), its
 * id, and its trust domain when it is qualified (for a person signed in
 * through an identity provider, `"oidc-issuer:"` and the issuer).
 */
export interface RequesterPrincipal {
  readonly kind: string;
  readonly id: string;
  readonly trustDomainId?: string;
}

/**
 * The original requester of the run a callback serves. A tool that acts in a
 * person's name serves only a requester that is not carried.
 */
export interface ToolCallRequester {
  readonly principal: RequesterPrincipal;
  readonly carried: boolean;
}

/** The principal kinds the runtime's requester grammar names. */
const PRINCIPAL_KINDS = new Set<string>([
  "human",
  "personal_agent",
  "shared_agent",
  "runtime_host",
  "service_account",
]);

/** A control character, as the runtime's identity check reads one. */
const CONTROL_CHARACTER = /[\u0000-\u001f\u007f-\u009f]/;

/**
 * Whether `value` is an id the runtime accepts for a qualified principal or a
 * trust domain: a string that is not blank and holds no control character.
 */
function isIdentity(value: unknown): value is string {
  return typeof value === "string" && value.trim() !== "" && !CONTROL_CHARACTER.test(value);
}

/**
 * Parse a wire `requester` object as the runtime's grammar reads it. An absent
 * or malformed value is `undefined`: no requester, which a person-scoped tool
 * refuses. Malformed is anything the runtime refuses: a principal kind outside
 * `human`, `personal_agent`, `shared_agent`, `runtime_host` and
 * `service_account`; a qualification that is null, has an unknown kind, or
 * carries a field its kind does not have; a blank trust domain or one with a
 * control character; and a blank qualified id or one with a control character.
 */
export function parseToolCallRequester(raw: unknown): ToolCallRequester | undefined {
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
    return undefined;
  }
  const { principal, carried } = raw as Record<string, unknown>;
  if (principal === null || typeof principal !== "object" || Array.isArray(principal)) {
    return undefined;
  }
  const wirePrincipal = principal as Record<string, unknown>;
  const { kind, id } = wirePrincipal;
  if (
    typeof kind !== "string" ||
    !PRINCIPAL_KINDS.has(kind) ||
    typeof id !== "string" ||
    id.trim() === ""
  ) {
    return undefined;
  }
  let trustDomainId: string | undefined;
  if (Object.prototype.hasOwnProperty.call(wirePrincipal, "qualification")) {
    const qualification = wirePrincipal.qualification;
    if (qualification === null || typeof qualification !== "object" || Array.isArray(qualification)) {
      return undefined;
    }
    const fields = qualification as Record<string, unknown>;
    const keys = Object.keys(fields).sort().join(",");
    if (fields.kind === "qualified") {
      const domain = fields.trust_domain_id;
      if (keys !== "kind,trust_domain_id" || !isIdentity(domain) || !isIdentity(id)) {
        return undefined;
      }
      trustDomainId = domain;
    } else if (fields.kind !== "unqualified" || keys !== "kind") {
      return undefined;
    }
  }
  if (carried !== undefined && typeof carried !== "boolean") {
    return undefined;
  }
  return {
    principal: trustDomainId === undefined ? { kind, id } : { kind, id, trustDomainId },
    carried: carried === true,
  };
}
