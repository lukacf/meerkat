// ═══════════════════════════════════════════════════════════
// Bubbles — Speech bubbles + thought indicators
// ═══════════════════════════════════════════════════════════

import type { AgentId } from "../types";
import { DESKS, CANVAS_W } from "./layout";
import { addRenderHook, addUpdateHook } from "./canvas";

// ── Speech Bubbles ──

interface ActiveBubble {
  agentId: AgentId;
  text: string;
  startTime: number;
  duration: number;
  elapsed: number;
}

const speechBubbles = new Map<AgentId, ActiveBubble>();

export function showSpeechBubble(agentId: AgentId, text: string, durationMs = 4000): void {
  // Truncate to ~60 chars
  const truncated = text.length > 60 ? text.slice(0, 57) + "..." : text;
  speechBubbles.set(agentId, {
    agentId,
    text: truncated,
    startTime: performance.now(),
    duration: durationMs / 1000,
    elapsed: 0,
  });
}

export function hideSpeechBubble(agentId: AgentId): void {
  speechBubbles.delete(agentId);
}

// ── Thought Bubbles ──

const thinkingAgents = new Set<AgentId>();
let thinkDotPhase = 0;

export function showThinkBubble(agentId: AgentId): void {
  thinkingAgents.add(agentId);
}

export function hideThinkBubble(agentId: AgentId): void {
  thinkingAgents.delete(agentId);
}

export function isThinking(agentId: AgentId): boolean {
  return thinkingAgents.has(agentId);
}

// ── Update ──

function update(dt: number): void {
  // Tick speech bubbles
  for (const [id, b] of speechBubbles) {
    b.elapsed += dt;
    if (b.elapsed >= b.duration) {
      speechBubbles.delete(id);
    }
  }
  // Tick thought dot phase
  thinkDotPhase += dt * 3; // ~3 cycles per second
}

// ── Render ──

function render(ctx: CanvasRenderingContext2D, _dt: number): void {
  // Speech bubbles
  for (const [, b] of speechBubbles) {
    renderSpeechBubble(ctx, b);
  }

  // Think bubbles
  for (const agentId of thinkingAgents) {
    // Don't show think bubble if speech bubble is active
    if (speechBubbles.has(agentId)) continue;
    renderThinkBubble(ctx, agentId);
  }
}

function renderSpeechBubble(ctx: CanvasRenderingContext2D, b: ActiveBubble): void {
  const desk = DESKS[b.agentId];
  const x = desk.x;
  const y = desk.y - 62; // above person's head (desk.y = chair bottom, sprite is ~55px tall)

  // Fade out in last 0.5s
  const fadeStart = b.duration - 0.5;
  const alpha = b.elapsed > fadeStart ? 1 - (b.elapsed - fadeStart) / 0.5 : 1;

  ctx.save();
  ctx.globalAlpha = Math.max(0, alpha);

  // Measure text for bubble sizing
  ctx.font = "13px 'Space Grotesk', sans-serif";
  const lines = wrapText(ctx, b.text, 200);
  const lineH = 17;
  const padX = 10;
  const padY = 6;
  const w = Math.min(220, Math.max(...lines.map(l => ctx.measureText(l).width)) + padX * 2);
  const h = lines.length * lineH + padY * 2;

  // Clamp to canvas bounds
  let bx = x - w / 2;
  if (bx < 4) bx = 4;
  if (bx + w > CANVAS_W - 4) bx = CANVAS_W - 4 - w;
  const by = y - h;

  // Background
  ctx.fillStyle = "rgba(255,255,255,0.95)";
  roundRect(ctx, bx, by, w, h, 6);
  ctx.fill();

  // Border
  ctx.strokeStyle = "rgba(0,0,0,0.15)";
  ctx.lineWidth = 1;
  roundRect(ctx, bx, by, w, h, 6);
  ctx.stroke();

  // Pointer triangle
  ctx.fillStyle = "rgba(255,255,255,0.95)";
  ctx.beginPath();
  ctx.moveTo(x - 5, by + h);
  ctx.lineTo(x, by + h + 6);
  ctx.lineTo(x + 5, by + h);
  ctx.closePath();
  ctx.fill();
  ctx.strokeStyle = "rgba(0,0,0,0.15)";
  ctx.beginPath();
  ctx.moveTo(x - 5, by + h);
  ctx.lineTo(x, by + h + 6);
  ctx.lineTo(x + 5, by + h);
  ctx.stroke();

  // Text
  ctx.fillStyle = "#222";
  ctx.font = "13px 'Space Grotesk', sans-serif";
  ctx.textAlign = "left";
  for (let i = 0; i < lines.length; i++) {
    ctx.fillText(lines[i], bx + padX, by + padY + (i + 1) * lineH - 2);
  }

  ctx.restore();
}

function renderThinkBubble(ctx: CanvasRenderingContext2D, agentId: AgentId): void {
  const desk = DESKS[agentId];
  const x = desk.x + 14;
  const y = desk.y - 62;

  ctx.save();

  // Small cloud dots leading to bubble
  const dotSizes = [2, 3];
  for (let i = 0; i < dotSizes.length; i++) {
    ctx.beginPath();
    ctx.arc(x - 6 + i * 5, y + 12 + i * 4, dotSizes[i], 0, Math.PI * 2);
    ctx.fillStyle = "rgba(255,255,255,0.7)";
    ctx.fill();
  }

  // Main thought cloud
  ctx.fillStyle = "rgba(255,255,255,0.9)";
  roundRect(ctx, x - 16, y - 8, 36, 20, 10);
  ctx.fill();
  ctx.strokeStyle = "rgba(0,0,0,0.1)";
  ctx.lineWidth = 1;
  roundRect(ctx, x - 16, y - 8, 36, 20, 10);
  ctx.stroke();

  // Animated dots
  const phase = thinkDotPhase % 3;
  for (let i = 0; i < 3; i++) {
    const dotAlpha = i <= Math.floor(phase) ? 0.8 : 0.2;
    ctx.beginPath();
    ctx.arc(x - 6 + i * 10, y + 2, 3, 0, Math.PI * 2);
    ctx.fillStyle = `rgba(80,80,120,${dotAlpha})`;
    ctx.fill();
  }

  ctx.restore();
}

// ── Helpers ──

function wrapText(ctx: CanvasRenderingContext2D, text: string, maxWidth: number): string[] {
  const words = text.split(" ");
  const lines: string[] = [];
  let current = "";
  for (const word of words) {
    const test = current ? current + " " + word : word;
    if (ctx.measureText(test).width > maxWidth && current) {
      lines.push(current);
      current = word;
    } else {
      current = test;
    }
  }
  if (current) lines.push(current);
  // Max 3 lines
  if (lines.length > 3) {
    lines.length = 3;
    lines[2] = lines[2].slice(0, -3) + "...";
  }
  return lines;
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number): void {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.lineTo(x + w - r, y);
  ctx.quadraticCurveTo(x + w, y, x + w, y + r);
  ctx.lineTo(x + w, y + h - r);
  ctx.quadraticCurveTo(x + w, y + h, x + w - r, y + h);
  ctx.lineTo(x + r, y + h);
  ctx.quadraticCurveTo(x, y + h, x, y + h - r);
  ctx.lineTo(x, y + r);
  ctx.quadraticCurveTo(x, y, x + r, y);
  ctx.closePath();
}

// ── Register ──

export function initBubbles(): void {
  addUpdateHook(update);
  addRenderHook(render);
}
