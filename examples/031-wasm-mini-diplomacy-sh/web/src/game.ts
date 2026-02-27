// ═══════════════════════════════════════════════════════════
// Game Engine
// ═══════════════════════════════════════════════════════════

import type { ArenaState, Team, TurnDecision } from "./types";
import { TEAMS } from "./types";

export function defaultState(): ArenaState {
  return {
    turn: 1, max_turns: 10,
    scores: { france: 0, prussia: 0, russia: 0 },
    regions: [
      { id: "iberia",    controller: "france",  defense: 45, value: 2 },
      { id: "paris",     controller: "france",  defense: 58, value: 3 },
      { id: "marseille", controller: "france",  defense: 42, value: 2 },
      { id: "burgundy",  controller: "france",  defense: 46, value: 3 },
      { id: "rhineland", controller: "prussia", defense: 44, value: 3 },
      { id: "berlin",    controller: "prussia", defense: 56, value: 3 },
      { id: "bavaria",   controller: "prussia", defense: 40, value: 2 },
      { id: "bohemia",   controller: "prussia", defense: 43, value: 2 },
      { id: "warsaw",    controller: "russia",  defense: 40, value: 3 },
      { id: "ukraine",   controller: "russia",  defense: 44, value: 2 },
      { id: "moscow",    controller: "russia",  defense: 55, value: 3 },
      { id: "crimea",    controller: "russia",  defense: 42, value: 2 },
    ],
  };
}

export function resolveOrders(state: ArenaState, orders: TurnDecision[]): ArenaState {
  const newRegions = state.regions.map(r => ({ ...r }));
  for (const decision of orders) {
    const { order } = decision;
    const target = newRegions.find(r => r.id === order.target_region);
    if (!target || target.controller === order.team) continue;
    const atkNoise = Math.floor(Math.random() * 12);
    const defNoise = Math.floor(Math.random() * 8);
    if (order.aggression + atkNoise > target.defense + defNoise) {
      target.controller = order.team;
      target.defense = Math.max(25, Math.floor(target.defense * 0.5) + Math.floor(order.fortify / 8));
    } else {
      target.defense = Math.min(100, target.defense + 2);
    }
    for (const r of newRegions) {
      if (r.controller === order.team) { r.defense = Math.min(100, r.defense + Math.floor(order.fortify / 15)); break; }
    }
  }
  const scores = { ...state.scores };
  for (const team of TEAMS) scores[team] += newRegions.filter(r => r.controller === team).reduce((s, r) => s + r.value, 0);
  const turn = state.turn + 1;
  let winner: Team | "draw" | undefined;
  if (turn > state.max_turns) {
    const sorted = TEAMS.slice().sort((a, b) => scores[b] - scores[a]);
    winner = scores[sorted[0]] > scores[sorted[1]] ? sorted[0] : "draw";
  }
  return { turn, max_turns: state.max_turns, regions: newRegions, scores, winner };
}
