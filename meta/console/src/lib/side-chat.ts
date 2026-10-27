import type { ConsoleAgent } from "../types";

/**
 * Side chats: a human's private branch of an agent's conversation. The
 * console forks the agent (`meta/fork_member`) into a new member that starts
 * from the agent's committed transcript, opens it beside the agent, lists it
 * under the agent, and closes it with the ordinary retire. The source agent
 * is never touched. These helpers keep the console's decisions pure.
 */

/** The JSON-RPC code of a typed `meta/fork_member` refusal: nothing forked. */
export const SIDE_CHAT_FORK_REFUSED_CODE = -32055;
/** Meerkat's typed fork instruction refusal (the gateway's `refuse` policy). */
export const FORK_INSTRUCTION_REFUSED_CODE = -32037;

const ALIAS_BASE_MAX_CHARS = 40;
const ALIAS_NONCE_MAX_CHARS = 12;

/** What the fork did with one instruction the side chat inherited. */
export interface SideChatInstruction {
  namespace: string;
  key: string;
  state: "inherited" | "inactive";
  reason?: string;
}

/** The `meta/fork_member` result the console acts on. */
export interface SideChatForkResult {
  agentIdentity: string;
  sideChatOf: string;
  displayName: string;
  sessionId?: string;
  instructions: SideChatInstruction[];
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function nonEmptyString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

/** The identity a side chat names as its source, else null. */
export function sideChatSource(agent: ConsoleAgent | null | undefined): string | null {
  return nonEmptyString(agent?.side_chat_of);
}

/**
 * A fresh member alias for a side chat of `sourceIdentity`: the source's
 * identity, `-side-` and a nonce, kept out of the runtime-owned namespaces
 * (`rt:` generated aliases, `mk--` encoded roster ids).
 */
export function sideChatAlias(sourceIdentity: string, nonce: string): string {
  let base = sourceIdentity.trim();
  while (base.startsWith("rt:") || base.startsWith("mk--")) {
    base = base.startsWith("rt:") ? base.slice(3) : base.slice(4);
  }
  base = base
    .replace(/[^A-Za-z0-9:_-]+/g, "-")
    .replace(/^[-:_]+/, "")
    .slice(0, ALIAS_BASE_MAX_CHARS)
    .replace(/[-:_]+$/, "");
  const suffix = nonce.toLowerCase().replace(/[^a-z0-9]+/g, "").slice(0, ALIAS_NONCE_MAX_CHARS);
  return `${base || "agent"}-side-${suffix || "0"}`;
}

/** The `source_member_id` a fork of `agent` names: its identity, else its member id. */
export function sideChatSourceMemberId(agent: ConsoleAgent): string {
  return (agent.identity || agent.member_id).trim();
}

/**
 * Why "Fork to side chat" is unavailable right now, else null. A source that
 * is running is refused by the runtime, so the action waits for the agent to
 * be idle: no local or server phase, no busy fold, nothing queued.
 */
export function sideChatForkBlockedReason(input: {
  busy: boolean;
  phase: string | null;
  responsePhase: string | null | undefined;
  queued: number;
}): string | null {
  if (input.busy || input.phase !== null || (input.responsePhase ?? null) !== null || input.queued > 0) {
    return "Available when the agent is idle";
  }
  return null;
}

/** Validate the `meta/fork_member` result; fail closed on anything else. */
export function parseSideChatForkResult(result: unknown): SideChatForkResult {
  const body = record(result);
  const agentIdentity = nonEmptyString(body?.agent_identity);
  const sideChatOf = nonEmptyString(body?.side_chat_of);
  if (!body || !agentIdentity || !sideChatOf) {
    throw new Error("invalid meta/fork_member result: missing agent_identity or side_chat_of");
  }
  const instructions: SideChatInstruction[] = [];
  for (const entry of Array.isArray(body.instructions) ? body.instructions : []) {
    const row = record(entry);
    const namespace = nonEmptyString(row?.namespace);
    const key = nonEmptyString(row?.key);
    if (!row || !namespace || !key) continue;
    const state = row.state === "inherited" ? "inherited" : "inactive";
    const reasonKind = nonEmptyString(record(row.reason)?.kind);
    instructions.push({
      namespace,
      key,
      state,
      ...(state === "inactive" && reasonKind ? { reason: reasonKind } : {}),
    });
  }
  return {
    agentIdentity,
    sideChatOf,
    displayName: nonEmptyString(body.display_name) ?? agentIdentity,
    ...(nonEmptyString(body.session_id) ? { sessionId: nonEmptyString(body.session_id) as string } : {}),
    instructions,
  };
}

const INSTRUCTION_GAP_TEXT: Record<string, string> = {
  not_effective_in_source: "not active in the source",
  body_unreadable: "its text could not be read",
  activation_refused: "the runtime refused it",
};

/**
 * The notice a side chat shows for the instructions its fork could not
 * inherit, else null: until a host activates them, the side chat refuses to
 * answer and names them.
 */
export function inactiveInstructionNotice(instructions: readonly SideChatInstruction[]): string | null {
  const inactive = instructions.filter((instruction) => instruction.state === "inactive");
  if (inactive.length === 0) return null;
  const names = inactive.map((instruction) => {
    const why = instruction.reason ? INSTRUCTION_GAP_TEXT[instruction.reason] : undefined;
    return `${instruction.namespace}/${instruction.key}${why ? ` (${why})` : ""}`;
  });
  return `Not inherited: ${names.join(", ")}. This side chat cannot answer until ${inactive.length === 1 ? "it is" : "they are"} activated.`;
}

/** The operator-facing message for a failed fork. */
export function sideChatForkErrorMessage(error: unknown): string {
  const rpc = (error as { rpcError?: { code?: unknown; message?: unknown; data?: unknown } } | null)?.rpcError;
  const data = record(rpc?.data);
  if (rpc?.code === SIDE_CHAT_FORK_REFUSED_CODE && data) {
    if (data.kind === "fork_source_unavailable") {
      switch (data.cause) {
        case "running":
          return "The agent is working. Fork to a side chat when it is idle.";
        case "no_session":
          return "This agent has no conversation to fork.";
        case "remote_read_unavailable":
          return "This agent runs on another host; its conversation cannot be forked here.";
        default:
          return "This agent cannot be forked right now. Try again shortly.";
      }
    }
    if (data.kind === "unwired_seat_unavailable") {
      const wiring = record(data.wiring);
      const peer = nonEmptyString(wiring?.orchestrator_role) ?? nonEmptyString(wiring?.peer_role);
      return peer
        ? `Side chats are not available for this agent: the mob wires its role to ${peer} peers, and a side chat must stay private.`
        : "Side chats are not available for this agent: the mob wires its role to peers, and a side chat must stay private.";
    }
  }
  if (rpc?.code === FORK_INSTRUCTION_REFUSED_CODE) {
    return "This agent's activated instructions may not be copied into a side chat on this gateway.";
  }
  if (error instanceof Error && error.message) return error.message;
  return String(error);
}

/** The agent a fork seated as `alias`, once the console lists it. */
export function findSideChatAgent(agents: readonly ConsoleAgent[], alias: string): ConsoleAgent | null {
  const wanted = alias.trim().toLowerCase();
  if (!wanted) return null;
  return (
    agents.find((agent) =>
      [agent.identity, agent.member_id, agent.agent_id].some((key) => key?.trim().toLowerCase() === wanted),
    ) ?? null
  );
}

/** The side chats among `agents` that branched from `source`. */
export function sideChatsOf(agents: readonly ConsoleAgent[], source: ConsoleAgent): ConsoleAgent[] {
  const keys = new Set(
    [source.identity, source.member_id, source.agent_id]
      .filter((key): key is string => Boolean(key))
      .map((key) => key.toLowerCase()),
  );
  return agents.filter((agent) => {
    const of = sideChatSource(agent);
    return of !== null && agent.member_id !== source.member_id && keys.has(of.toLowerCase());
  });
}
