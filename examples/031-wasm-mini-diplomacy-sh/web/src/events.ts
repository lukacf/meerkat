// ═══════════════════════════════════════════════════════════
// Event Streaming → DM Channels + Narrator Context
// ═══════════════════════════════════════════════════════════

import type { ArenaState, ChannelId, MatchSession, RuntimeModule, Team, OrderOutcome } from "./types";
import { CHANNELS, TEAMS, TEAM_LABELS } from "./types";
import { pushMessage, pushStructuredSummary, dmChannel } from "./ui";

export interface DrainResult { events: number; errors: string[] }

export function sourceSortKey(source: unknown): string | null {
  if (!source || typeof source !== "object" || Array.isArray(source)) return null;
  const record = source as Record<string, unknown>;
  const field = {
    session: "session_id", runtime: "runtime_id", interaction: "interaction_id", external: "source_id",
  }[record.type as string];
  if (record.type === "callback") return Object.keys(record).length === 1 ? "callback" : null;
  if (!field || Object.keys(record).some(key => key !== "type" && key !== field)) return null;
  const id = record[field];
  return typeof id === "string" && id.length > 0 ? `${record.type}:${id}` : null;
}

function agentFallbackChannel(agentIdentity: string, team: Team): ChannelId {
  if (agentIdentity.includes("ambassador")) return `${team[0]}-plan-amb` as ChannelId;
  return `${team[0]}-plan-op` as ChannelId;
}

export function drainAllEvents(mod: RuntimeModule, session: MatchSession, turn: number): DrainResult {
  let events = 0;
  const errors: string[] = [];
  const buffered: Array<{ sub: (typeof session.subs)[number]; env: any }> = [];
  let warnedMalformedEnvelope = false;
  for (const sub of session.subs) {
    try {
      const raw = mod.poll_subscription(sub.handle);
      const parsed: any[] = JSON.parse(raw);
      for (const event of parsed) {
        if (
          event == null
          || typeof event.timestamp_ms !== "number"
          || sourceSortKey(event.source) === null
          || typeof event.seq !== "number"
          || typeof event.event_id !== "string"
          || event.payload == null
        ) {
          if (!warnedMalformedEnvelope) {
            warnedMalformedEnvelope = true;
            console.warn("Skipping malformed event envelope from poll_subscription", event);
          }
          continue;
        }
        buffered.push({ sub, env: event });
      }
    } catch { /* poll error */ }
  }
  buffered.sort((a, b) => {
    const ta = a.env.timestamp_ms;
    const tb = b.env.timestamp_ms;
    if (ta !== tb) return ta - tb;
    const sa = sourceSortKey(a.env.source)!;
    const sb = sourceSortKey(b.env.source)!;
    if (sa !== sb) return sa < sb ? -1 : 1;
    const qa = a.env.seq;
    const qb = b.env.seq;
    if (qa !== qb) return qa - qb;
    const ia = a.env.event_id;
    const ib = b.env.event_id;
    return ia < ib ? -1 : ia > ib ? 1 : 0;
  });
  for (const { sub, env } of buffered) {
    if (session.seenEventIds.has(env.event_id)) continue;
    session.seenEventIds.add(env.event_id);
    const event = env.payload;
    events++;
    if (event.type === "run_failed") {
      if (event.error_report) session.failures.push({ agentIdentity: sub.agentIdentity, turn, report: event.error_report });
      errors.push(`${sub.agentIdentity}: ${event.error_report?.message ?? "Run failed without a diagnostic"}`);
    }
    if (event.type === "extraction_failed") {
      errors.push(`${sub.agentIdentity}: Extraction failed: ${event.reason}`);
    }
    if (event.type === "run_started") {
      session.summarizedRuns.delete(sub.agentIdentity);
    }

    // Comms: agent used send_message/send tool → route raw message to DM channel
    if (event.type === "tool_call_requested" && (event.name === "send_message" || event.name === "send")) {
      try {
        const args = typeof event.args === "string" ? JSON.parse(event.args) : event.args;
        const to = session.peerMembers.get(args.peer_id);
        const body = args.body || "";
        if (to && body) {
          const ch = dmChannel(sub.agentIdentity, to);
          if (ch) {
            pushMessage({ channel: ch, role: sub.role, faction: sub.team, content: body, turn });
          }
        }
      } catch { /* skip parse errors */ }
    }

    // Structured output: agent completed a wake cycle.
    if ((event.type === "run_completed" || event.type === "extraction_succeeded") && event.structured_output) {
      if (session.summarizedRuns.has(sub.agentIdentity)) continue;
      try {
        const structured = event.structured_output;
        if (structured.headline) {
          session.summarizedRuns.add(sub.agentIdentity);
          const dispatches: { peer: string; summary: string }[] = structured.dispatches || [];
          const routed = new Set<ChannelId>();

          for (const dispatch of dispatches) {
            const peer = session.peerMembers.get(dispatch.peer);
            const ch = peer ? dmChannel(sub.agentIdentity, peer) : null;
            if (!ch || routed.has(ch)) continue;
            routed.add(ch);
            pushStructuredSummary({
              channel: ch, role: sub.role, faction: sub.team, turn,
              content: dispatch.summary,
              headline: dispatch.summary,
              details: structured.headline,
            });
          }

          const ownCh = agentFallbackChannel(sub.agentIdentity, sub.team);
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
  return { events, errors };
}

export function buildNarratorSummary(
  sess: MatchSession, turn: number, outcomes: OrderOutcome[],
  captures: Set<string>, newState: ArenaState,
): string {
  const turnMsgs = sess.messages.filter(m => m.turn === turn);
  const sections: string[] = [];

  sections.push("=== BATTLE OUTCOMES ===");
  for (const { order, result } of outcomes) {
    sections.push(`${TEAM_LABELS[order.team]} attacked ${order.target_region.replace(/-/g, " ")} (aggression ${order.aggression}) \u2192 ${result.toUpperCase()}`);
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
