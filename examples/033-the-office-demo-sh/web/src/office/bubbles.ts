// ═══════════════════════════════════════════════════════════
// Bubbles — Speech bubbles + thought indicators
// ═══════════════════════════════════════════════════════════

import { AGENT_IDS } from "../types";
import type { AgentId } from "../types";
import { CANVAS_H, CANVAS_W, DESKS } from "./layout";
import { addRenderHook, addUpdateHook } from "./canvas";

// ── Speech Bubbles ──

interface ActiveBubble {
  agentId: AgentId;
  text: string;
  startTime: number;
  duration: number;
  elapsed: number;
}

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

type BubbleBias = "left" | "right" | "center";

interface SpeakerHint {
  dx: number;
  dy: number;
  bias: BubbleBias;
}

interface BubbleLayout extends Rect {
  lines: string[];
  speakerX: number;
  speakerY: number;
  tailBaseX: number;
  tailBaseY: number;
  tailTipX: number;
  tailTipY: number;
}

const speechBubbles = new Map<AgentId, ActiveBubble>();

const SPEAKER_HINTS: Record<AgentId, SpeakerHint> = {
  triage: { dx: 26, dy: -64, bias: "right" },
  archivist: { dx: -24, dy: -64, bias: "left" },
  "it-dept": { dx: -8, dy: -66, bias: "left" },
  "hr-dept": { dx: 8, dy: -66, bias: "right" },
  facilities: { dx: -10, dy: -66, bias: "left" },
  finance: { dx: 10, dy: -66, bias: "right" },
  "alex-pa": { dx: -8, dy: -66, bias: "left" },
  "sam-pa": { dx: 0, dy: -66, bias: "center" },
  "pat-pa": { dx: 8, dy: -66, bias: "right" },
  gate: { dx: 0, dy: -64, bias: "center" },
};

const BUBBLE_MARGIN = 8;
const BUBBLE_FONT = "700 20px 'Space Grotesk', sans-serif";
const BUBBLE_LINE_H = 25;
const BUBBLE_PAD_X = 12;
const BUBBLE_PAD_Y = 10;
const BUBBLE_MAX_TEXT_W = 300;

export function showSpeechBubble(agentId: AgentId, text: string, durationMs = 4000): void {
  const truncated = text.length > 80 ? text.slice(0, 77) + "..." : text;
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
  for (const [id, b] of speechBubbles) {
    b.elapsed += dt;
    if (b.elapsed >= b.duration) {
      speechBubbles.delete(id);
    }
  }
  thinkDotPhase += dt * 3;
}

// ── Render ──

function render(ctx: CanvasRenderingContext2D, _dt: number): void {
  const placed: BubbleLayout[] = [];
  const occupants = buildAgentBodyRects();
  const ordered = [...speechBubbles.values()].sort((a, b) => DESKS[a.agentId].y - DESKS[b.agentId].y);

  for (const bubble of ordered) {
    const layout = computeSpeechLayout(ctx, bubble, placed, occupants);
    drawSpeechBubble(ctx, bubble, layout);
    placed.push(layout);
  }

  for (const agentId of thinkingAgents) {
    if (speechBubbles.has(agentId)) continue;
    renderThinkBubble(ctx, agentId, placed);
  }
}

function computeSpeechLayout(
  ctx: CanvasRenderingContext2D,
  bubble: ActiveBubble,
  placed: BubbleLayout[],
  occupants: Rect[],
): BubbleLayout {
  const speaker = getSpeakerPoint(bubble.agentId);
  ctx.font = BUBBLE_FONT;
  const lines = wrapText(ctx, bubble.text, BUBBLE_MAX_TEXT_W, 3);
  const textW = Math.max(...lines.map((line) => ctx.measureText(line).width));
  const w = Math.ceil(textW + BUBBLE_PAD_X * 2);
  const h = Math.ceil(lines.length * BUBBLE_LINE_H + BUBBLE_PAD_Y * 2 - 4);

  const candidates = makeCandidates(speaker.x, speaker.y, w, h, speaker.bias);
  let bestRect = candidates[0];
  let bestScore = Number.POSITIVE_INFINITY;

  for (let i = 0; i < candidates.length; i++) {
    const score = scoreCandidateRect(candidates[i], i, speaker.x, speaker.y, placed, occupants);
    if (score < bestScore) {
      bestScore = score;
      bestRect = candidates[i];
    }
  }

  const clamped = clampRect(bestRect, BUBBLE_MARGIN);
  const tail = computeTail(clamped, speaker.x, speaker.y);
  return {
    ...clamped,
    lines,
    speakerX: speaker.x,
    speakerY: speaker.y,
    tailBaseX: tail.baseX,
    tailBaseY: tail.baseY,
    tailTipX: speaker.x,
    tailTipY: speaker.y,
  };
}

function drawSpeechBubble(ctx: CanvasRenderingContext2D, bubble: ActiveBubble, layout: BubbleLayout): void {
  const fadeStart = bubble.duration - 0.5;
  const alpha = bubble.elapsed > fadeStart ? 1 - (bubble.elapsed - fadeStart) / 0.5 : 1;
  ctx.save();
  ctx.globalAlpha = Math.max(0, alpha);

  ctx.shadowColor = "rgba(4, 8, 18, 0.3)";
  ctx.shadowBlur = 10;
  ctx.shadowOffsetY = 2;
  ctx.fillStyle = "rgba(255, 255, 255, 0.99)";
  roundRect(ctx, layout.x, layout.y, layout.w, layout.h, 7);
  ctx.fill();
  ctx.shadowBlur = 0;
  ctx.shadowOffsetY = 0;

  ctx.strokeStyle = "rgba(18, 28, 45, 0.38)";
  ctx.lineWidth = 1.35;
  roundRect(ctx, layout.x, layout.y, layout.w, layout.h, 7);
  ctx.stroke();

  drawTail(ctx, layout);

  ctx.fillStyle = "#111D31";
  ctx.font = BUBBLE_FONT;
  ctx.textAlign = "left";
  for (let i = 0; i < layout.lines.length; i++) {
    ctx.fillText(
      layout.lines[i],
      layout.x + BUBBLE_PAD_X,
      layout.y + BUBBLE_PAD_Y + (i + 1) * BUBBLE_LINE_H - 6,
    );
  }

  ctx.restore();
}

function drawTail(ctx: CanvasRenderingContext2D, layout: BubbleLayout): void {
  const vx = layout.tailTipX - layout.tailBaseX;
  const vy = layout.tailTipY - layout.tailBaseY;
  const len = Math.max(1, Math.hypot(vx, vy));
  const nx = -vy / len;
  const ny = vx / len;
  const half = 6;

  const p1x = layout.tailBaseX + nx * half;
  const p1y = layout.tailBaseY + ny * half;
  const p2x = layout.tailBaseX - nx * half;
  const p2y = layout.tailBaseY - ny * half;

  ctx.fillStyle = "rgba(255, 255, 255, 0.99)";
  ctx.beginPath();
  ctx.moveTo(p1x, p1y);
  ctx.lineTo(layout.tailTipX, layout.tailTipY);
  ctx.lineTo(p2x, p2y);
  ctx.closePath();
  ctx.fill();
  ctx.strokeStyle = "rgba(18, 28, 45, 0.38)";
  ctx.lineWidth = 1.1;
  ctx.stroke();
}

function renderThinkBubble(ctx: CanvasRenderingContext2D, agentId: AgentId, placed: BubbleLayout[]): void {
  const speaker = getSpeakerPoint(agentId);
  let cx = speaker.x + (speaker.bias === "left" ? -22 : speaker.bias === "right" ? 22 : 0);
  let cy = speaker.y - 20;
  let rect: Rect = { x: cx - 22, y: cy - 12, w: 44, h: 24 };

  for (let i = 0; i < 4; i++) {
    const overlaps = placed.some((b) => rectsOverlap(rect, b));
    if (!overlaps) break;
    cy -= 14;
    rect = { x: cx - 22, y: cy - 12, w: 44, h: 24 };
  }

  cx = clamp(cx, 24, CANVAS_W - 24);
  cy = clamp(cy, 20, CANVAS_H - 20);

  const dotA = lerpPoint(speaker.x, speaker.y, cx, cy + 2, 0.35);
  const dotB = lerpPoint(speaker.x, speaker.y, cx, cy + 2, 0.7);

  ctx.save();

  for (const [idx, d] of [dotA, dotB].entries()) {
    ctx.beginPath();
    ctx.arc(d.x, d.y, idx === 0 ? 2.2 : 3, 0, Math.PI * 2);
    ctx.fillStyle = "rgba(255, 255, 255, 0.86)";
    ctx.fill();
    ctx.strokeStyle = "rgba(18, 28, 45, 0.24)";
    ctx.lineWidth = 0.8;
    ctx.stroke();
  }

  ctx.fillStyle = "rgba(255, 255, 255, 0.97)";
  roundRect(ctx, cx - 20, cy - 10, 40, 22, 10);
  ctx.fill();
  ctx.strokeStyle = "rgba(18, 28, 45, 0.28)";
  ctx.lineWidth = 1;
  roundRect(ctx, cx - 20, cy - 10, 40, 22, 10);
  ctx.stroke();

  const phase = thinkDotPhase % 3;
  for (let i = 0; i < 3; i++) {
    const alpha = i <= Math.floor(phase) ? 0.8 : 0.28;
    ctx.beginPath();
    ctx.arc(cx - 8 + i * 8, cy + 1, 2.4, 0, Math.PI * 2);
    ctx.fillStyle = `rgba(67, 80, 105, ${alpha})`;
    ctx.fill();
  }

  ctx.restore();
}

// ── Placement Helpers ──

function getSpeakerPoint(agentId: AgentId): { x: number; y: number; bias: BubbleBias } {
  const desk = DESKS[agentId];
  const hint = SPEAKER_HINTS[agentId];
  return { x: desk.x + hint.dx, y: desk.y + hint.dy, bias: hint.bias };
}

function makeCandidates(sx: number, sy: number, w: number, h: number, bias: BubbleBias): Rect[] {
  const top = { x: sx - w / 2, y: sy - h - 24, w, h };
  const leftTop = { x: sx - w - 20, y: sy - h - 16, w, h };
  const rightTop = { x: sx + 20, y: sy - h - 16, w, h };
  const leftMid = { x: sx - w - 24, y: sy - h * 0.62, w, h };
  const rightMid = { x: sx + 24, y: sy - h * 0.62, w, h };
  const leftHigh = { x: sx - w - 16, y: sy - h - 48, w, h };
  const rightHigh = { x: sx + 16, y: sy - h - 48, w, h };

  if (bias === "left") {
    return [leftTop, top, rightTop, leftMid, rightMid, leftHigh, rightHigh];
  }
  if (bias === "right") {
    return [rightTop, top, leftTop, rightMid, leftMid, rightHigh, leftHigh];
  }
  return [top, rightTop, leftTop, rightMid, leftMid, rightHigh, leftHigh];
}

function scoreCandidateRect(
  rect: Rect,
  orderIdx: number,
  sx: number,
  sy: number,
  placed: BubbleLayout[],
  occupants: Rect[],
): number {
  const clamped = clampRect(rect, BUBBLE_MARGIN);
  const displacement = Math.abs(clamped.x - rect.x) + Math.abs(clamped.y - rect.y);
  const overflow = overflowDistance(rect, BUBBLE_MARGIN);

  let overlapAgents = 0;
  for (const occ of occupants) overlapAgents += rectIntersectionArea(clamped, occ);

  let overlapBubbles = 0;
  for (const other of placed) overlapBubbles += rectIntersectionArea(clamped, other);

  const nearestX = clamp(sx, clamped.x, clamped.x + clamped.w);
  const nearestY = clamp(sy, clamped.y, clamped.y + clamped.h);
  const tailLen = Math.hypot(sx - nearestX, sy - nearestY);

  return (
    displacement * 20 +
    overflow * 120 +
    overlapAgents * 0.07 +
    overlapBubbles * 0.12 +
    tailLen * 0.8 +
    orderIdx * 12
  );
}

function computeTail(rect: Rect, sx: number, sy: number): { baseX: number; baseY: number } {
  const left = rect.x;
  const right = rect.x + rect.w;
  const top = rect.y;
  const bottom = rect.y + rect.h;

  const toLeft = Math.abs(sx - left);
  const toRight = Math.abs(sx - right);
  const toTop = Math.abs(sy - top);
  const toBottom = Math.abs(sy - bottom);
  const min = Math.min(toLeft, toRight, toTop, toBottom);

  if (min === toBottom) {
    return { baseX: clamp(sx, left + 14, right - 14), baseY: bottom };
  }
  if (min === toTop) {
    return { baseX: clamp(sx, left + 14, right - 14), baseY: top };
  }
  if (min === toLeft) {
    return { baseX: left, baseY: clamp(sy, top + 14, bottom - 14) };
  }
  return { baseX: right, baseY: clamp(sy, top + 14, bottom - 14) };
}

function buildAgentBodyRects(): Rect[] {
  const rects: Rect[] = [];
  for (const id of AGENT_IDS) {
    const d = DESKS[id];
    rects.push({ x: d.x - 34, y: d.y - 84, w: 68, h: 96 });
  }
  return rects;
}

function clampRect(rect: Rect, margin: number): Rect {
  return {
    x: clamp(rect.x, margin, CANVAS_W - margin - rect.w),
    y: clamp(rect.y, margin, CANVAS_H - margin - rect.h),
    w: rect.w,
    h: rect.h,
  };
}

function overflowDistance(rect: Rect, margin: number): number {
  const left = Math.max(0, margin - rect.x);
  const top = Math.max(0, margin - rect.y);
  const right = Math.max(0, rect.x + rect.w - (CANVAS_W - margin));
  const bottom = Math.max(0, rect.y + rect.h - (CANVAS_H - margin));
  return left + top + right + bottom;
}

function rectIntersectionArea(a: Rect, b: Rect): number {
  const x1 = Math.max(a.x, b.x);
  const y1 = Math.max(a.y, b.y);
  const x2 = Math.min(a.x + a.w, b.x + b.w);
  const y2 = Math.min(a.y + a.h, b.y + b.h);
  const w = x2 - x1;
  const h = y2 - y1;
  if (w <= 0 || h <= 0) return 0;
  return w * h;
}

function rectsOverlap(a: Rect, b: Rect): boolean {
  return !(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y);
}

// ── Text / Geometry Helpers ──

function wrapText(
  ctx: CanvasRenderingContext2D,
  text: string,
  maxWidth: number,
  maxLines = 3,
): string[] {
  const words = text.split(" ");
  const lines: string[] = [];
  let current = "";

  for (const word of words) {
    const next = current ? `${current} ${word}` : word;
    if (ctx.measureText(next).width > maxWidth && current) {
      lines.push(current);
      current = word;
    } else {
      current = next;
    }
  }
  if (current) lines.push(current);

  if (lines.length > maxLines) {
    lines.length = maxLines;
    let tail = lines[maxLines - 1].trimEnd();
    while (tail.length > 1 && ctx.measureText(`${tail}...`).width > maxWidth) {
      tail = tail.slice(0, -1);
    }
    lines[maxLines - 1] = `${tail}...`;
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

function lerpPoint(x1: number, y1: number, x2: number, y2: number, t: number): { x: number; y: number } {
  return { x: x1 + (x2 - x1) * t, y: y1 + (y2 - y1) * t };
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, v));
}

// ── Register ──

export function initBubbles(): void {
  addUpdateHook(update);
  addRenderHook(render);
}
