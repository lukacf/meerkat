// ═══════════════════════════════════════════════════════════
// Mob Definitions — autonomous agents with comms
// ═══════════════════════════════════════════════════════════

import type { ArenaState, Team } from "./types";
import { TEAMS, TEAM_LABELS } from "./types";

const GAME_RULES = `GAME: 3 great European powers — France, Prussia, and Russia — contest 12 territories across the continent.
COMBAT: aggression + rand(0-12) vs defense + rand(0-8). Capture if greater. Fortify = 100 - aggression.
SCORING: Each turn, powers earn sum of their territories' values. 10 turns total. Highest cumulative score wins.
DIPLOMACY: 2v1 coalitions are decisive. Alliances, betrayals, and secret treaties are critical.
TERRITORIES: Iberia, Paris, Marseille, Burgundy (France); Rhineland, Berlin, Bavaria, Bohemia (Prussia); Warsaw, Ukraine, Moscow, Crimea (Russia). Marseille connects to Crimea by sea.`;

const CYCLE_MODEL = `HOW YOU OPERATE: You run in cycles. Each cycle: you wake up with new messages in your inbox, you read them, you use send_message to reply, then you produce a structured output summarizing ONLY what you just did THIS cycle. You then go back to sleep until new messages arrive.
CRITICAL: Your structured output (headline + dispatches) must cover ONLY the actions you took THIS cycle — the messages you JUST sent. Not previous cycles. Not your overall history. Just what happened right now.`;

const HISTORICAL: Record<Team, { leader: string; style: string }> = {
  france:  { leader: "Talleyrand", style: "cunning and elegant, with Napoleonic grandeur" },
  prussia: { leader: "Bismarck",   style: "iron-willed and calculating, with Prussian precision" },
  russia:  { leader: "Gorchakov",  style: "patient and expansionist, with Imperial ambition" },
};

export function buildFactionDefinition(team: Team, model: string): object {
  const T = TEAM_LABELS[team];
  const H = HISTORICAL[team];
  const mobId = `diplomacy-${team}`;
  const others = TEAMS.filter(t => t !== team);
  const peerAddr = (role: string, t: Team = team) => `diplomacy-${t}/${role}/${t}-${role}`;

  const plannerSkill = `You are the strategic PLANNER for ${T}, channeling the cunning of ${H.leader}. You are ${H.style}.
${GAME_RULES}
${CYCLE_MODEL}

YOUR PEERS (use these exact addresses with send_message):
- Operator: ${peerAddr("operator")}
- Ambassador: ${peerAddr("ambassador")}

Do NOT act until you receive a turn message with game state.
When you receive game state:
1. Message your OPERATOR to debate strategy — target, aggression level, risk assessment.
2. After agreeing, brief your AMBASSADOR with diplomatic objectives:
   - What to PROPOSE to each foreign ambassador
   - What to CONCEAL (never reveal your actual attack target!)
   - What lies or misdirection to use (e.g. "we shall march on Warsaw" when you intend Bohemia)
3. Wait for diplomatic intelligence from your ambassador.
4. Adjust plan if needed, then tell your OPERATOR to finalize.
MESSAGE FORMAT: First line = one-sentence summary of the message. Then blank line. Then details (keep SHORT, 3-5 lines). Do not call peers().`;

  const operatorSkill = `You are the military OPERATOR for ${T}. Cold, analytical, focused on winning — a true ${T} field marshal.
${GAME_RULES}
${CYCLE_MODEL}

YOUR PEERS (use these exact addresses with send_message):
- Planner: ${peerAddr("planner")}

CRITICAL PROTOCOL — follow exactly:
- Do NOT act until your planner messages you.
- Do NOT issue FINAL ORDER until your planner explicitly says "finalize" or "execute".
- The planner will discuss strategy, then brief the ambassador, then wait for diplomatic intel, then tell you to finalize. You MUST wait through this cycle.

When the planner discusses strategy:
1. Run the combat math. Challenge weak plans. Push for high-probability attacks.
2. Agree on target and aggression — but DO NOT finalize yet.
3. Say "Standing by for diplomatic phase" and WAIT.
4. ONLY when the planner says to finalize, send a message containing:
   FINAL ORDER: target=<region-id> aggression=<0-100>

MESSAGE FORMAT: First line = one-sentence summary. Then blank line. Then details (3-5 lines max). Do not call peers().`;

  const ambassadorSkill = `You are the diplomatic AMBASSADOR for ${T}. A master of deception and persuasion at the Congress of Europe.
${GAME_RULES}
${CYCLE_MODEL}

YOUR PEERS (use these exact addresses with send_message):
- Planner (your sovereign): ${peerAddr("planner")}
- Foreign ambassadors: ${others.map(t => `${TEAM_LABELS[t]} \u2192 ${peerAddr("ambassador", t)}`).join(", ")}

Do NOT act until your planner briefs you.

CARDINAL RULES OF DIPLOMACY:
- NEVER reveal your nation's actual attack target to foreign ambassadors.
- NEVER share internal strategy discussions.
- LIE freely. Promise alliances you have no intention of keeping. Claim you march on one territory while your armies prepare elsewhere.
- Use misdirection: "We contemplate a defensive season" when you prepare all-out offense.
- Extract intelligence: discover the enemy's plans while revealing nothing genuine.
- Propose treaties that favor you. Make them believe it is mutual.

When briefed by your planner:
1. Contact foreign ambassadors. Pursue your planner's objectives through deception.
2. Extract enemy intentions — who do they intend to attack? What coalitions are forming?
3. Report back to your planner with intelligence and negotiation results.
Use markdown: **bold** for key intel, bullet lists for proposals. Be concise. Do not call peers().`;

  // Structured output schema — extraction turn produces this after each agent wake cycle.
  // The headline is what appears in compact UI bubbles, so it must be engaging and
  // in-character, like a telegram dispatch from the field.
  // The dispatches array provides per-recipient summaries for multi-channel routing.
  const turnSummarySchema = {
    type: "object",
    additionalProperties: false,
    properties: {
      headline: {
        type: "string",
        description: `Rewrite what you just communicated as a short SMS/text message (1-3 sentences). Address your peers directly. Use "you" not their name. BAD: "Informed operator about Bavaria strike plan." GOOD: "Bavaria, aggression 70 — run the numbers and confirm." BAD: "Proposed alliance to Prussia." GOOD: "Let's hit Russia together — their east is exposed. You in?"`,
      },
      dispatches: {
        type: "array",
        description: "ONLY peers you sent new messages to THIS cycle. Empty array if you sent nothing new.",
        items: {
          type: "object",
          additionalProperties: false,
          properties: {
            peer: { type: "string", description: "Full peer address, e.g. 'diplomacy-france/operator/france-operator'" },
            summary: { type: "string", description: `Rewrite what you told THIS peer as a short SMS. Address them with "you." BAD: "Proposed alliance against Russia." GOOD: "We should hit Russia together — you take Crimea, we take Warsaw. Deal?" BAD: "Reported intelligence to planner." GOOD: "Prussia's bluffing about Burgundy — they're really going east. We should strike now."` },
          },
          required: ["peer", "summary"],
        },
      },
    },
    required: ["headline", "dispatches"],
  };

  return {
    id: mobId,
    skills: {
      [`${team}-planner-role`]: { source: "inline", content: plannerSkill },
      [`${team}-operator-role`]: { source: "inline", content: operatorSkill },
      [`${team}-ambassador-role`]: { source: "inline", content: ambassadorSkill },
    },
    profiles: {
      planner: {
        model, runtime_mode: "autonomous_host",
        tools: { comms: true },
        skills: [`${team}-planner-role`],
        peer_description: `${T} strategic planner`,
        external_addressable: true,
        output_schema: turnSummarySchema,
      },
      operator: {
        model, runtime_mode: "autonomous_host",
        tools: { comms: true },
        skills: [`${team}-operator-role`],
        peer_description: `${T} military operator`,
        external_addressable: true,
        output_schema: turnSummarySchema,
      },
      ambassador: {
        model, runtime_mode: "autonomous_host",
        tools: { comms: true },
        skills: [`${team}-ambassador-role`],
        peer_description: `${T} diplomatic ambassador`,
        external_addressable: true,
        output_schema: turnSummarySchema,
      },
    },
    wiring: {},
    flows: {},
  };
}

export function buildNarratorDefinition(model: string): object {
  return {
    id: "diplomacy-narrator",
    profiles: {
      narrator: {
        model, runtime_mode: "turn_driven",
        tools: { comms: true },
        peer_description: "War correspondent",
        external_addressable: false,
      },
    },
    flows: {
      narrate: {
        steps: {
          summarize: {
            role: "narrator",
            message: `You are a war correspondent for the London Times, writing from the great European conflict of 1870. You see EVERYTHING — the secret war rooms of Paris, the whispered lies between diplomats in Vienna, the private doubts of Prussian field marshals. You know who lied to whom, who plans betrayal, and what each power REALLY discussed behind closed doors.

Below is the COMPLETE intelligence from this turn: battle outcomes, private strategy discussions, diplomatic briefings, and the actual negotiations between ambassadors.

{{params.summary}}

Write 2-4 sentences of vivid, dramatic narrative in the style of a 19th century war correspondent. Reveal dramatic irony — e.g. "The French ambassador smiled and promised peace, even as Napoleon's marshals sharpened their swords for the Rhineland." Reference specific territories, betrayals, and secret plans. Make the reader feel the tension and treachery of the age.
CRITICAL: Output RAW JSON only. No markdown, no code fences, no backticks.
{"narrative": "your narrative here"}`,
            dispatch_mode: "one_to_one",
          },
        },
      },
    },
  };
}

export function serializeState(team: Team, state: ArenaState): string {
  const ours = state.regions.filter(r => r.controller === team);
  const enemies = state.regions.filter(r => r.controller !== team);
  return JSON.stringify({
    turn: state.turn, max_turns: state.max_turns, team: TEAM_LABELS[team],
    scores: state.scores,
    your_territories: ours.map(r => ({ id: r.id, defense: r.defense, value: r.value })),
    enemy_territories: enemies.map(r => ({ id: r.id, controller: r.controller, defense: r.defense, value: r.value })),
  });
}
