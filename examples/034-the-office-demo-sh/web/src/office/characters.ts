// ═══════════════════════════════════════════════════════════
// Characters — State machine + rendering for all 10 agents
// Each character animates independently with random offset.
// ═══════════════════════════════════════════════════════════

import { AGENT_IDS } from "../types";
import type { AgentId, CharacterState } from "../types";
import { DESKS } from "./layout";
import { drawCharacter } from "./sprites";
import { addRenderHook, addUpdateHook, getSelectedAgent } from "./canvas";

// ── Per-agent state ──

interface CharState {
  state: CharacterState;
  frame: number;
  frameTimer: number;
  /** Random offset so characters don't animate in sync */
  phaseOffset: number;
}

const agents = new Map<AgentId, CharState>();

// Init all agents as idle with random phase offsets
for (const id of AGENT_IDS) {
  agents.set(id, {
    state: "idle",
    frame: 0,
    frameTimer: Math.random() * 2, // random start offset
    phaseOffset: Math.random() * 2,
  });
}

// ── Public API ──

export function setAgentState(id: AgentId, state: CharacterState): void {
  const a = agents.get(id);
  if (a && a.state !== state) {
    a.state = state;
    a.frame = 0;
    a.frameTimer = a.phaseOffset; // reset to offset, not 0
  }
}

export function getAgentState(id: AgentId): CharacterState {
  return agents.get(id)?.state ?? "idle";
}

// ── Frame timing ──
// Different speeds per state for natural feel
const FRAME_DURATIONS: Record<CharacterState, number> = {
  idle: 0.6,      // slow, relaxed typing
  on_call: 0.4,   // faster, animated phone gestures
  thinking: 0.8,  // slow, contemplative
};

function update(dt: number): void {
  for (const [, a] of agents) {
    a.frameTimer += dt;
    const dur = FRAME_DURATIONS[a.state];
    if (a.frameTimer >= dur) {
      a.frameTimer -= dur;
      a.frame++;
    }
  }
}

// ── Render ──

function render(ctx: CanvasRenderingContext2D, _dt: number): void {
  const selected = getSelectedAgent();

  // Sort by Y for depth (agents further up = drawn first)
  const sorted = [...AGENT_IDS].sort((a, b) => DESKS[a].y - DESKS[b].y);

  for (const id of sorted) {
    const desk = DESKS[id];
    const a = agents.get(id)!;
    drawCharacter(ctx, id, desk.x, desk.y, a.state, a.frame, selected === id);
  }
}

// ── Register hooks ──

export function initCharacters(): void {
  addUpdateHook(update);
  addRenderHook(render);
}
