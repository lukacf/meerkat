// The console's changes of long-term agents, as one object the panels call:
// it sends each change under its request id, reads the answer, keeps the
// requests of this page that wait for a person's decision, and tells the
// console when the roster may have changed.

import React from "react";

import { createConsoleId } from "./id";
import {
  agentRefRequest,
  answeredRefusalOf,
  identityRefusalOf,
  parseAgentPage,
  parseAgentRecord,
  parseChangePlan,
  parseIdentityDirectory,
  parseIdentityOutcome,
  readAllPages,
  type AgentRecord,
  type AgentSummary,
  type ChangePlan,
  type DirectoryPeer,
  type DirectoryProfile,
  type IdentityChangeFlags,
  type IdentityChangeKind,
  type IdentityOutcome,
  type IdentityRefusal,
} from "./identity-changes";
import { errorMessage } from "./errors";

/** The console commands of the family (lib/headless.ts). */
export type IdentityCommand =
  | "createAgent"
  | "updateAgent"
  | "archiveAgent"
  | "unarchiveAgent"
  | "removeAgent"
  | "withdrawAgentChange"
  | "getAgentRecord"
  | "listAgentRecords"
  | "getAgentChange"
  | "listAgentDirectory"
  | "planAgentChange";

/** Runs one command of the family and answers its result; a refusal is the
 * transport's error with the family's JSON-RPC error on it. */
export type IdentityCommandRunner = (command: IdentityCommand, params: Record<string, unknown>) => Promise<unknown>;

const CHANGE_COMMANDS: Readonly<Record<IdentityChangeKind, IdentityCommand>> = {
  create: "createAgent",
  update: "updateAgent",
  archive: "archiveAgent",
  unarchive: "unarchiveAgent",
  remove: "removeAgent",
};

/** What sending a change came to. */
export type IdentityChangeResult =
  | { kind: "outcome"; outcome: IdentityOutcome }
  | { kind: "refused"; refusal: IdentityRefusal }
  /** No answer the console can read: the change may or may not have been
   * made. Sending the same request again under the same id is safe. */
  | { kind: "failed"; message: string };

/** What asking the gateway about a change came to. */
export type IdentityPlanResult =
  | { kind: "plan"; plan: ChangePlan }
  /** The gateway refuses the change as planned. */
  | { kind: "refused"; refusal: IdentityRefusal }
  /** The gateway said nothing the console can use: no answer came, it
   * could not be read, or the gateway plans no change for the console. The
   * change is judged when it is sent. */
  | { kind: "unknown" };

/** A request of this page that waits for a person's decision. */
export interface PendingIdentityRequest {
  requestId: string;
  /** The decision the gateway answered the request with. */
  decisionId: string;
  change: IdentityChangeKind;
  /** The agent as people know it. */
  name: string;
  deadlineMs: number;
}

export interface IdentityDirectory {
  profiles: DirectoryProfile[];
  peers: DirectoryPeer[];
}

export interface IdentityChangesApi {
  flags: IdentityChangeFlags;
  /** Counts up whenever a change of this page may have changed an agent:
   * what a panel reads again on. */
  revision: number;
  /** A new request id, for one action. */
  mintRequestId(): string;
  /** What a create or an update may name: every page. */
  loadDirectory(): Promise<IdentityDirectory>;
  /** One agent's record, read now: its spec digest is what a change of it
   * names. */
  loadRecord(name: string): Promise<AgentRecord>;
  /** The archived agents. */
  loadArchived(): Promise<AgentSummary[]>;
  /** Send `request` as the change `change` under `requestId`. `name` is
   * the agent as people know it, kept with a request that waits for a
   * decision. */
  change(change: IdentityChangeKind, request: Record<string, unknown>, requestId: string, name: string): Promise<IdentityChangeResult>;
  /** Ask the gateway what `request` as the change `change` would get.
   * Nothing is held and nothing is written, and the question has a request
   * id of its own: the change, when it is sent, is another request. */
  plan(change: IdentityChangeKind, request: Record<string, unknown>): Promise<IdentityPlanResult>;
  /** Archive, unarchive or remove the agent `name`: reads its record for
   * the spec digest, then sends the change. */
  changeWhole(change: "archive" | "unarchive" | "remove", name: string, requestId: string, displayName: string): Promise<IdentityChangeResult>;
  /** Withdraw this page's own request that waits for a decision. */
  withdraw(requestId: string): Promise<IdentityChangeResult | { kind: "withdrawn" }>;
  /** This page's requests that wait for a decision. */
  pending: PendingIdentityRequest[];
  /** Read each waiting request again: a decided or ended one leaves the
   * list. */
  refreshPending(): Promise<void>;
}

/** A failed call as a result: a refusal when the call was answered (or
 * never sent), "no answer" only when none came. */
function failure(error: unknown): IdentityChangeResult {
  const refusal = answeredRefusalOf(error);
  return refusal ? { kind: "refused", refusal } : { kind: "failed", message: errorMessage(error) };
}

/**
 * The family for the panels. `run` carries a command out; `onChanged` is
 * called after a change that committed or may have. `readable` says that
 * the gateway serves the family's reads: the archived agents are then
 * listed even where no change is offered, and the flags alone decide the
 * controls. Null while the gateway serves neither.
 */
export function useIdentityChanges({
  flags,
  readable,
  run,
  onChanged,
}: {
  flags: IdentityChangeFlags;
  readable: boolean;
  run: IdentityCommandRunner;
  onChanged: () => void;
}): IdentityChangesApi | null {
  const [pending, setPending] = React.useState<PendingIdentityRequest[]>([]);
  const [revision, setRevision] = React.useState(0);
  const runRef = React.useRef(run);
  runRef.current = run;
  const changedRef = React.useRef(onChanged);
  changedRef.current = onChanged;
  const pendingRef = React.useRef(pending);
  pendingRef.current = pending;

  const api = React.useMemo(() => {
    const changed = () => {
      setRevision((current) => current + 1);
      changedRef.current();
    };
    const loadRecord = async (name: string): Promise<AgentRecord> => {
      const record = parseAgentRecord(await runRef.current("getAgentRecord", { name }));
      if (!record) throw new Error("The gateway's answer was not an agent's record.");
      return record;
    };
    const change: IdentityChangesApi["change"] = async (kind, request, requestId, name) => {
      let answer: unknown;
      try {
        answer = await runRef.current(CHANGE_COMMANDS[kind], { ...request, request_id: requestId });
      } catch (error) {
        const result = failure(error);
        // A lost answer may be a change that was made.
        if (result.kind === "failed") changed();
        return result;
      }
      const outcome = parseIdentityOutcome(answer);
      if (!outcome) {
        changed();
        return { kind: "failed", message: "The gateway's answer could not be read." };
      }
      if (outcome.outcome === "refused") return { kind: "refused", refusal: outcome.refusal };
      if (outcome.outcome === "pending_decision") {
        setPending((current) => [
          ...current.filter((entry) => entry.requestId !== requestId),
          { requestId, decisionId: outcome.decisionId, change: kind, name, deadlineMs: outcome.deadlineMs },
        ]);
      } else {
        setPending((current) => current.filter((entry) => entry.requestId !== requestId));
        changed();
      }
      return { kind: "outcome", outcome };
    };
    return {
      mintRequestId: () => createConsoleId("agent-change"),
      loadDirectory: async (): Promise<IdentityDirectory> => {
        const profiles: DirectoryProfile[] = [];
        const peers = await readAllPages(async (after) => {
          const page = parseIdentityDirectory(await runRef.current("listAgentDirectory", after ? { after } : {}));
          for (const profile of page.profiles) {
            if (!profiles.some((known) => known.profile === profile.profile)) profiles.push(profile);
          }
          return { items: page.peers, next: page.next };
        });
        return { profiles, peers };
      },
      loadRecord,
      loadArchived: async (): Promise<AgentSummary[]> => {
        const agents = await readAllPages(async (after) => {
          const page = parseAgentPage(await runRef.current("listAgentRecords", { include_archived: true, ...(after ? { after } : {}) }));
          return { items: page.agents, next: page.next };
        });
        return agents.filter((agent) => agent.archived);
      },
      change,
      plan: async (kind, request): Promise<IdentityPlanResult> => {
        let answer: unknown;
        try {
          answer = await runRef.current("planAgentChange", { ...request, change: kind, request_id: createConsoleId("agent-plan") });
        } catch (error) {
          // Only the family's own refusal is the gateway's judgment of the
          // change. A plan that was not answered, not asked or refused as a
          // read says nothing about it.
          const refusal = identityRefusalOf(error);
          return refusal ? { kind: "refused", refusal } : { kind: "unknown" };
        }
        const plan = parseChangePlan(answer);
        return plan ? { kind: "plan", plan } : { kind: "unknown" };
      },
      changeWhole: async (kind, name, requestId, displayName): Promise<IdentityChangeResult> => {
        let record: AgentRecord;
        try {
          record = await loadRecord(name);
        } catch (error) {
          return failure(error);
        }
        return change(kind, agentRefRequest(record), requestId, displayName);
      },
      withdraw: async (requestId: string): Promise<IdentityChangeResult | { kind: "withdrawn" }> => {
        try {
          await runRef.current("withdrawAgentChange", { request_id: requestId });
        } catch (error) {
          return failure(error);
        }
        setPending((current) => current.filter((entry) => entry.requestId !== requestId));
        return { kind: "withdrawn" };
      },
      refreshPending: async (): Promise<void> => {
        const settled: string[] = [];
        let committed = false;
        for (const entry of pendingRef.current) {
          let outcome: IdentityOutcome | null;
          try {
            outcome = parseIdentityOutcome(await runRef.current("getAgentChange", { request_id: entry.requestId }));
          } catch {
            continue;
          }
          if (!outcome || outcome.outcome === "pending_decision") continue;
          settled.push(entry.requestId);
          if (outcome.outcome === "done" || outcome.outcome === "pending_preparation") committed = true;
        }
        if (settled.length === 0) return;
        setPending((current) => current.filter((entry) => !settled.includes(entry.requestId)));
        if (committed) changed();
      },
    };
  }, []);

  const { available, canCreate, canChange, canRemove } = flags;
  return React.useMemo(
    () => (available || readable ? { ...api, flags: { available, canCreate, canChange, canRemove }, pending, revision } : null),
    [api, available, readable, canCreate, canChange, canRemove, pending, revision],
  );
}
