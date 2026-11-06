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

/**
 * Parse a wire `requester` object. An absent or malformed value is
 * `undefined`: no requester, which a person-scoped tool refuses.
 */
export function parseToolCallRequester(raw: unknown): ToolCallRequester | undefined {
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
    return undefined;
  }
  const { principal, carried } = raw as Record<string, unknown>;
  if (principal === null || typeof principal !== "object" || Array.isArray(principal)) {
    return undefined;
  }
  const { kind, id, qualification } = principal as Record<string, unknown>;
  if (typeof kind !== "string" || kind.trim() === "" || typeof id !== "string" || id.trim() === "") {
    return undefined;
  }
  let trustDomainId: string | undefined;
  if (qualification !== undefined && qualification !== null) {
    if (typeof qualification !== "object" || Array.isArray(qualification)) {
      return undefined;
    }
    const { kind: qualificationKind, trust_domain_id: domain } = qualification as Record<
      string,
      unknown
    >;
    if (qualificationKind === "qualified") {
      if (typeof domain !== "string" || domain.trim() === "") {
        return undefined;
      }
      trustDomainId = domain;
    } else if (qualificationKind !== "unqualified") {
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
