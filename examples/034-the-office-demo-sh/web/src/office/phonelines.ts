// ═══════════════════════════════════════════════════════════
// Phone Lines — Glowing arc connections between agent desks
// ═══════════════════════════════════════════════════════════

import type { AgentId } from "../types";
import { DESKS } from "./layout";
import { addRenderHook, addUpdateHook } from "./canvas";

interface ActiveCall {
  id: string;
  from: AgentId;
  to: AgentId;
  color: string;
  elapsed: number;
  fadeStart: number | null;
  opacity: number;
}

const calls = new Map<string, ActiveCall>();
let nextCallId = 0;

// ── Public API ──

export function startCall(from: AgentId, to: AgentId, color = "#4488CC"): string {
  const id = `call-${nextCallId++}`;
  calls.set(id, { id, from, to, color, elapsed: 0, fadeStart: null, opacity: 1 });
  return id;
}

export function endCall(callId: string): void {
  const c = calls.get(callId);
  if (c && !c.fadeStart) {
    c.fadeStart = c.elapsed;
  }
}

export function endCallsForAgent(agentId: AgentId): void {
  for (const [, c] of calls) {
    if ((c.from === agentId || c.to === agentId) && !c.fadeStart) {
      c.fadeStart = c.elapsed;
    }
  }
}

export function getActiveCallCount(): number { return calls.size; }

// ── Envelope arrival animation ──

interface EnvelopeAnim {
  startX: number; startY: number;
  endX: number; endY: number;
  elapsed: number;
  duration: number;
}

const envelopes: EnvelopeAnim[] = [];

export function triggerEnvelopeArrival(toAgent: AgentId): void {
  const desk = DESKS[toAgent];
  envelopes.push({
    startX: 40, startY: 40, // mail slot area
    endX: desk.x, endY: desk.y - 10,
    elapsed: 0,
    duration: 0.8,
  });
}

// ── Update ──

const FADE_DURATION = 1.0;

function update(dt: number): void {
  for (const [id, c] of calls) {
    c.elapsed += dt;
    if (c.fadeStart !== null) {
      const fadeElapsed = c.elapsed - c.fadeStart;
      c.opacity = Math.max(0, 1 - fadeElapsed / FADE_DURATION);
      if (c.opacity <= 0) calls.delete(id);
    } else {
      // Gentle pulse
      c.opacity = 0.7 + 0.3 * Math.sin(c.elapsed * 3);
    }
  }

  // Tick envelopes
  for (let i = envelopes.length - 1; i >= 0; i--) {
    envelopes[i].elapsed += dt;
    if (envelopes[i].elapsed >= envelopes[i].duration) {
      envelopes.splice(i, 1);
    }
  }
}

// ── Render ──

function render(ctx: CanvasRenderingContext2D, _dt: number): void {
  // Phone call arcs
  for (const [, c] of calls) {
    renderCallArc(ctx, c);
  }

  // Envelope animations
  for (const env of envelopes) {
    renderEnvelope(ctx, env);
  }
}

function renderCallArc(ctx: CanvasRenderingContext2D, c: ActiveCall): void {
  const fromDesk = DESKS[c.from];
  const toDesk = DESKS[c.to];

  const x1 = fromDesk.phoneX;
  const y1 = fromDesk.phoneY;
  const x2 = toDesk.phoneX;
  const y2 = toDesk.phoneY;

  // Control point: midpoint raised upward
  const mx = (x1 + x2) / 2;
  const my = Math.min(y1, y2) - 50 - Math.abs(x2 - x1) * 0.08;

  ctx.save();
  ctx.globalAlpha = c.opacity;

  // Outer glow (very thick, very transparent)
  ctx.beginPath();
  ctx.moveTo(x1, y1);
  ctx.quadraticCurveTo(mx, my, x2, y2);
  ctx.strokeStyle = c.color;
  ctx.lineWidth = 14;
  ctx.globalAlpha = c.opacity * 0.12;
  ctx.stroke();

  // Mid glow
  ctx.beginPath();
  ctx.moveTo(x1, y1);
  ctx.quadraticCurveTo(mx, my, x2, y2);
  ctx.lineWidth = 6;
  ctx.globalAlpha = c.opacity * 0.3;
  ctx.stroke();

  // Core line
  ctx.beginPath();
  ctx.moveTo(x1, y1);
  ctx.quadraticCurveTo(mx, my, x2, y2);
  ctx.strokeStyle = "#fff";
  ctx.lineWidth = 2;
  ctx.globalAlpha = c.opacity * 0.6;
  ctx.stroke();

  // Colored core on top
  ctx.beginPath();
  ctx.moveTo(x1, y1);
  ctx.quadraticCurveTo(mx, my, x2, y2);
  ctx.strokeStyle = c.color;
  ctx.lineWidth = 3;
  ctx.globalAlpha = c.opacity * 0.9;
  ctx.stroke();

  // Endpoint glows
  for (const [px, py] of [[x1, y1], [x2, y2]]) {
    ctx.beginPath();
    ctx.arc(px, py, 8, 0, Math.PI * 2);
    ctx.fillStyle = c.color;
    ctx.globalAlpha = c.opacity * 0.25;
    ctx.fill();
    ctx.beginPath();
    ctx.arc(px, py, 4, 0, Math.PI * 2);
    ctx.globalAlpha = c.opacity * 0.7;
    ctx.fill();
  }

  ctx.restore();
}

function renderEnvelope(ctx: CanvasRenderingContext2D, env: EnvelopeAnim): void {
  const t = easeOutCubic(Math.min(1, env.elapsed / env.duration));
  const x = env.startX + (env.endX - env.startX) * t;
  const y = env.startY + (env.endY - env.startY) * t - Math.sin(t * Math.PI) * 30;

  ctx.save();
  ctx.globalAlpha = 1 - t * 0.2;

  // Envelope body
  ctx.fillStyle = "#F5E6C8";
  ctx.fillRect(x - 10, y - 6, 20, 14);
  ctx.strokeStyle = "#AA8855";
  ctx.lineWidth = 1;
  ctx.strokeRect(x - 10, y - 6, 20, 14);

  // Envelope flap (V shape)
  ctx.beginPath();
  ctx.moveTo(x - 10, y - 6);
  ctx.lineTo(x, y + 2);
  ctx.lineTo(x + 10, y - 6);
  ctx.strokeStyle = "#AA8855";
  ctx.lineWidth = 1;
  ctx.stroke();

  ctx.restore();
}

function easeOutCubic(t: number): number {
  return 1 - Math.pow(1 - t, 3);
}

// ── Register ──

export function initPhoneLines(): void {
  addUpdateHook(update);
  addRenderHook(render);
}
