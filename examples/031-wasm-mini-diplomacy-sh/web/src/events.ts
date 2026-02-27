// ═══════════════════════════════════════════════════════════
// Event Streaming → DM Channels + Narrator Context
// ═══════════════════════════════════════════════════════════

import type { ArenaState, ChannelId, MatchSession, RuntimeModule, Team, TurnDecision } from "./types";
import { CHANNELS, TEAMS, TEAM_LABELS } from "./types";
import { pushMessage, pushStructuredSummary, dmChannel } from "./ui";

export interface DrainResult { events: number; errors: string[] }

function agentFallbackChannel(meerkatId: string, team: Team): ChannelId {
  if (meerkatId.includes("ambassador")) return `${team[0]}-plan-amb` as ChannelId;
  return `${team[0]}-plan-op` as ChannelId;
}

export function drainAllEvents(mod: RuntimeModule, session: MatchSession, turn: number): DrainResult {
  let events = 0;
  const errors: string[] = [];
  for (const sub of session.subs) {
    try {
      const raw = mod.poll_subscription(sub.handle);
      const parsed: any[] = JSON.parse(raw);
      for (const event of parsed) {
        events++;
        if (event.type === "run_failed" && event.error) {
          errors.push(`${sub.meerkatId}: ${event.error}`);
        }

        // Comms: agent used "send" tool → route raw message to DM channel
        if (event.type === "tool_call_requested" && event.name === "send") {
          const callId = event.id;
          if (callId && session.seenToolCallIds.has(callId)) continue;
          if (callId) session.seenToolCallIds.add(callId);
          try {
            const args = typeof event.args === "string" ? JSON.parse(event.args) : event.args;
            const to = args.to || "";
            const body = args.body || "";
            if (to && body) {
              const ch = dmChannel(sub.meerkatId, to);
              if (ch) {
                pushMessage({ channel: ch, role: sub.role, faction: sub.team, content: body, turn });
              }
            }
          } catch { /* skip parse errors */ }
        }

        // Structured output: agent completed a wake cycle.
        // The agent knows its full history — the extraction turn schema asks it to
        // produce dispatches ONLY for peers it messaged THIS cycle.
        // We trust the structured output and route each dispatch to its channel.
        if (event.type === "run_completed" && event.result) {
          try {
            const structured = JSON.parse(event.result);
            if (structured.headline) {
              const dispatches: { peer: string; summary: string }[] = structured.dispatches || [];
              const routed = new Set<ChannelId>();

              for (const dispatch of dispatches) {
                const ch = dmChannel(sub.meerkatId, dispatch.peer);
                if (!ch || routed.has(ch)) continue;
                routed.add(ch);
                pushStructuredSummary({
                  channel: ch, role: sub.role, faction: sub.team, turn,
                  content: dispatch.summary,
                  headline: dispatch.summary,
                  details: structured.headline,
                });
              }

              // Overall headline goes to agent's own channel if not already routed there
              const ownCh = agentFallbackChannel(sub.meerkatId, sub.team);
              if (!routed.has(ownCh)) {
                pushStructuredSummary({
                  channel: ownCh, role: sub.role, faction: sub.team, turn,
                  content: structured.headline,
                  headline: structured.headline,
                  details: dispatches.length > 0
                    ? dispatches.map((d: any) => `\u2192 ${d.summary}`).join("\n")
                    : undefined,
                });
              }
            }
          } catch { /* not JSON or missing fields — ignore */ }
        }
      }
    } catch { /* poll error */ }
  }
  return { events, errors };
}

export function buildNarratorSummary(
  sess: MatchSession, turn: number, decisions: TurnDecision[],
  captures: Set<string>, newState: ArenaState,
): string {
  const turnMsgs = sess.messages.filter(m => m.turn === turn);
  const sections: string[] = [];

  sections.push("=== BATTLE OUTCOMES ===");
  for (const d of decisions) {
    const hit = captures.has(d.order.target_region) ? "CAPTURED" : "REPELLED";
    sections.push(`${TEAM_LABELS[d.order.team]} attacked ${d.order.target_region.replace(/-/g, " ")} (aggression ${d.order.aggression}) \u2192 ${hit}`);
  }
  if (captures.size > 0) sections.push(`Territories changed hands: ${[...captures].map(id => id.replace(/-/g, " ")).join(", ")}`);
  sections.push(`Scores: ${TEAMS.map(t => `${TEAM_LABELS[t]}=${newState.scores[t]}`).join(", ")}. Turn ${turn} of ${newState.max_turns}.`);

  const channelGroups: [string, ChannelId[]][] = [
    ["INTERNAL STRATEGY (private war rooms \u2014 the public knows nothing)", ["f-plan-op", "p-plan-op", "r-plan-op"]],
    ["DIPLOMATIC BRIEFINGS (sovereigns instructing their ambassadors)", ["f-plan-amb", "p-plan-amb", "r-plan-amb"]],
    ["DIPLOMATIC NEGOTIATIONS (what the ambassadors said to each other)", ["f-p-diplo", "f-r-diplo", "p-r-diplo"]],
  ];

  for (const [heading, channels] of channelGroups) {
    const channelLines: string[] = [];
    for (const chId of channels) {
      const chMsgs = turnMsgs.filter(m => m.channel === chId);
      if (chMsgs.length === 0) continue;
      const label = CHANNELS.find(c => c.id === chId)?.label ?? chId;
      channelLines.push(`--- ${label} ---`);
      for (const m of chMsgs) {
        const who = `${m.role.toUpperCase()} [${TEAM_LABELS[m.faction as Team] ?? m.faction}]`;
        channelLines.push(`${who}: ${m.content.slice(0, 250)}`);
      }
    }
    if (channelLines.length > 0) {
      sections.push(`\n=== ${heading} ===`);
      sections.push(channelLines.join("\n"));
    }
  }

  return sections.join("\n");
}
