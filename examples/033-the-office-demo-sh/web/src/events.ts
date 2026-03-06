// ═══════════════════════════════════════════════════════════
// Event Streaming — Poll WASM subscriptions, drive visuals
// ═══════════════════════════════════════════════════════════

import type { AgentId, RuntimeModule, AgentSub } from "./types";
import { AGENT_IDS, CALL_COLORS } from "./types";
import { setAgentState } from "./office/characters";
import { showSpeechBubble, showThinkBubble, hideThinkBubble } from "./office/bubbles";
import { startCall, endCallsForAgent } from "./office/phonelines";

// ── Agent address → AgentId mapping ──

function addressToAgentId(addr: string): AgentId | null {
  // Address format: the-office/{profile}/{meerkat_id}
  const parts = addr.split("/");
  const meerkatId = parts[parts.length - 1] || addr;
  if (AGENT_IDS.includes(meerkatId as AgentId)) return meerkatId as AgentId;
  // Try matching by profile name
  for (const id of AGENT_IDS) {
    if (meerkatId === id) return id;
  }
  return null;
}

// ── Track seen tool call IDs to avoid duplicates ──

const seenToolCallIds = new Set<string>();
const seenApprovalDescs = new Set<string>();

// ── Incident callback (set by incidents module) ──

type IncidentCallback = (from: AgentId | "system", to: AgentId | null, content: string, headline: string, category: string) => void;
let onMessage: IncidentCallback | null = null;

export function setOnMessage(cb: IncidentCallback): void {
  onMessage = cb;
}

// ── Gate approval callback ──

type ApprovalCallback = (data: { short_summary?: string; action_description: string; risk_level: string; proposed_by: string }) => void;
let onApprovalNeeded: ApprovalCallback | null = null;

export function setOnApprovalNeeded(cb: ApprovalCallback): void {
  onApprovalNeeded = cb;
}

function fireApproval(data: { short_summary?: string; action_description: string; risk_level: string; proposed_by: string }): void {
  const key = data.action_description.slice(0, 60).toLowerCase();
  if (seenApprovalDescs.has(key)) return;
  seenApprovalDescs.add(key);
  console.log("[APPROVAL] Detected:", data.short_summary || data.action_description.slice(0, 50));
  onApprovalNeeded?.(data);
}

// ── Upsert record callback ──

type UpsertRecordCallback = (data: any) => void;
let onUpsertRecord: UpsertRecordCallback | null = null;

export function setOnUpsertRecord(cb: UpsertRecordCallback): void {
  onUpsertRecord = cb;
}

// ── Access control callback ──

type AccessControlCallback = (action: "revoke" | "restore", target: string, reason: string) => void;
let onAccessControl: AccessControlCallback | null = null;

export function setOnAccessControl(cb: AccessControlCallback): void {
  onAccessControl = cb;
}

// ── Poll all subscriptions ──

export function drainAllEvents(mod: RuntimeModule, subs: AgentSub[]): { events: number; errors: string[] } {
  let events = 0;
  const errors: string[] = [];

  for (const sub of subs) {
    try {
      const raw = mod.poll_subscription(sub.handle);
      const parsed: any[] = JSON.parse(raw);

      for (const event of parsed) {
        if (!event?.payload) continue;
        events++;
        const payload = event.payload;

        // ── Tool calls ──
        if (payload.type === "tool_call_requested") {
          const callId = payload.id;
          if (callId && seenToolCallIds.has(callId)) continue;
          if (callId) seenToolCallIds.add(callId);

          const toolName = payload.name;

          // ── Comms: agent used "send" ──
          if (toolName === "send") {
            try {
              const args = typeof payload.args === "string" ? JSON.parse(payload.args) : payload.args;
              const toAddr = args.to || "";
              const body = args.body || "";
              const recipientId = addressToAgentId(toAddr);

              if (recipientId && body) {
                const color = recipientId === "gate" ? CALL_COLORS.approval
                  : recipientId === "archivist" ? CALL_COLORS.knowledge
                  : CALL_COLORS.routing;

                startCall(sub.agentId, recipientId, color);
                setAgentState(sub.agentId, "on_call");

                const preview = body.length > 50 ? body.slice(0, 47) + "..." : body;
                showSpeechBubble(sub.agentId, preview, 5000);
                onMessage?.(sub.agentId, recipientId, body, preview, "routing");
              }
            } catch (e) { console.debug("[events] parse error:", e); }
          }

          // ── Fire-and-forget: request_human_approval ──
          if (toolName === "request_human_approval") {
            try {
              const args = typeof payload.args === "string" ? JSON.parse(payload.args) : payload.args;
              console.log("[APPROVAL] Tool call from", sub.agentId, args);
              fireApproval({
                short_summary: args.short_summary,
                action_description: args.action_description,
                risk_level: args.risk_level,
                proposed_by: args.proposed_by,
              });
            } catch (e) { console.debug("[events] parse error:", e); }
          }

          // ── Fire-and-forget: upsert_record ──
          if (toolName === "upsert_record") {
            try {
              const args = typeof payload.args === "string" ? JSON.parse(payload.args) : payload.args;
              console.log("[RECORD] Tool call from", sub.agentId, args);
              onUpsertRecord?.(args);
            } catch (e) { console.debug("[events] parse error:", e); }
          }

          // ── Fire-and-forget: revoke_access / restore_access ──
          if (toolName === "revoke_access") {
            try {
              const args = typeof payload.args === "string" ? JSON.parse(payload.args) : payload.args;
              console.log("[ACCESS CONTROL] revoke", args);
              onAccessControl?.("revoke", args.target, args.reason || "");
            } catch (e) { console.debug("[events] parse error:", e); }
          }
          if (toolName === "restore_access") {
            try {
              const args = typeof payload.args === "string" ? JSON.parse(payload.args) : payload.args;
              console.log("[ACCESS CONTROL] restore", args);
              onAccessControl?.("restore", args.target, args.reason || "");
            } catch (e) { console.debug("[events] parse error:", e); }
          }
        }

        // ── Run completed: structured output ──
        if (payload.type === "run_completed" && payload.result) {
          hideThinkBubble(sub.agentId);
          endCallsForAgent(sub.agentId);
          setAgentState(sub.agentId, "idle");

          try {
            const result = JSON.parse(payload.result);
            if (result.headline) {
              showSpeechBubble(sub.agentId, result.headline, 4000);
              onMessage?.(sub.agentId, null, result.headline, result.headline, result.category || "response");

            }
          } catch (e) { console.debug("[events] JSON parse:", e); }
        }

        // ── Run failed ──
        if (payload.type === "run_failed") {
          hideThinkBubble(sub.agentId);
          endCallsForAgent(sub.agentId);
          setAgentState(sub.agentId, "idle");
          const errMsg = payload.error || "Unknown error";
          errors.push(`${sub.agentId}: ${errMsg}`);
          showSpeechBubble(sub.agentId, `Error: ${errMsg.slice(0, 40)}`, 6000);
        }

        // ── Text delta (agent is generating) ──
        if (payload.type === "text_delta") {
          showThinkBubble(sub.agentId);
          setAgentState(sub.agentId, "thinking");

        }

        // ── Tool call start (agent called a tool) ──
        if (payload.type === "tool_call_start") {
          setAgentState(sub.agentId, "on_call");
        }
      }
    } catch (e) { console.debug("[events] poll error:", e); }
  }

  return { events, errors };
}
