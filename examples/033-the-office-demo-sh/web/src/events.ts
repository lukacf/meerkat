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

// ── Incident callback (set by incidents module) ──

type IncidentCallback = (from: AgentId | "system", to: AgentId | null, content: string, headline: string, category: string) => void;
let onMessage: IncidentCallback | null = null;

export function setOnMessage(cb: IncidentCallback): void {
  onMessage = cb;
}

// ── Gate approval callback ──

type ApprovalCallback = (data: { action_description: string; risk_level: string; proposed_by: string }) => void;
let onApprovalNeeded: ApprovalCallback | null = null;

export function setOnApprovalNeeded(cb: ApprovalCallback): void {
  onApprovalNeeded = cb;
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

        // ── Tool call: agent used "send" ──
        if (payload.type === "tool_call_requested" && payload.name === "send") {
          const callId = payload.id;
          if (callId && seenToolCallIds.has(callId)) continue;
          if (callId) seenToolCallIds.add(callId);

          try {
            const args = typeof payload.args === "string" ? JSON.parse(payload.args) : payload.args;
            const toAddr = args.to || "";
            const body = args.body || "";
            const recipientId = addressToAgentId(toAddr);

            if (recipientId && body) {
              // Determine call color from context
              const color = recipientId === "gate" ? CALL_COLORS.approval
                : recipientId === "archivist" ? CALL_COLORS.knowledge
                : CALL_COLORS.routing;

              // Visual: phone call arc
              startCall(sub.agentId, recipientId, color);
              setAgentState(sub.agentId, "on_call");

              // Speech bubble on sender with truncated message
              const preview = body.length > 50 ? body.slice(0, 47) + "..." : body;
              showSpeechBubble(sub.agentId, preview, 5000);

              // Notify incident system
              onMessage?.(sub.agentId, recipientId, body, preview, "routing");

              // Check if gate is being asked for human approval
              if (recipientId === "gate") {
                try {
                  const parsed = JSON.parse(body);
                  if (parsed.require_human_approval) {
                    onApprovalNeeded?.(parsed);
                  }
                } catch { /* not JSON, that's fine */ }
              }
            }
          } catch { /* parse error */ }
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

              // Check if gate's structured output contains approval request
              if (sub.agentId === "gate" && result.headline) {
                try {
                  // Gate might embed JSON in its regular message content too
                  const match = payload.result.match(/\{[^}]*require_human_approval[^}]*\}/);
                  if (match) {
                    const data = JSON.parse(match[0]);
                    if (data.require_human_approval) {
                      onApprovalNeeded?.(data);
                    }
                  }
                } catch { /* not a gate approval */ }
              }
            }
          } catch { /* not JSON */ }
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
    } catch { /* poll error */ }
  }

  return { events, errors };
}
