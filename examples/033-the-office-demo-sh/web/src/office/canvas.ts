// ═══════════════════════════════════════════════════════════
// Office Canvas — Game loop + rendering
// ═══════════════════════════════════════════════════════════

import { AGENT_IDS } from "../types";
import type { AgentId } from "../types";
import { CANVAS_W, CANVAS_H, DESKS, ZONES, MAIL_SLOT, FILING_CABINET } from "./layout";

// ── State ──

let canvas: HTMLCanvasElement | null = null;
let ctx: CanvasRenderingContext2D | null = null;
let bgImage: HTMLImageElement | null = null;
let lastTime = 0;
let stopped = false;
let selectedAgent: AgentId | null = null;
let onSelectAgent: ((id: AgentId | null) => void) | null = null;

// External render hooks (set by other modules in later phases)
export type RenderHook = (ctx: CanvasRenderingContext2D, dt: number) => void;
const renderHooks: RenderHook[] = [];
export function addRenderHook(hook: RenderHook): void { renderHooks.push(hook); }

export type UpdateHook = (dt: number) => void;
const updateHooks: UpdateHook[] = [];
export function addUpdateHook(hook: UpdateHook): void { updateHooks.push(hook); }

// ── Init ──

export function initCanvas(el: HTMLCanvasElement): void {
  canvas = el;
  ctx = el.getContext("2d")!;
  ctx.imageSmoothingEnabled = false;
  resizeCanvas();
  window.addEventListener("resize", resizeCanvas);
  canvas.addEventListener("click", handleClick);
  lastTime = 0;
  stopped = false;
  requestAnimationFrame(frame);
}

export function setOnSelectAgent(cb: (id: AgentId | null) => void): void {
  onSelectAgent = cb;
}

export function stopCanvas(): void { stopped = true; }

// ── Resize (maintain aspect ratio) ──

function resizeCanvas(): void {
  if (!canvas) return;
  const container = canvas.parentElement;
  if (!container) return;
  const cw = container.clientWidth;
  const ch = container.clientHeight;
  const scale = Math.min(cw / CANVAS_W, ch / CANVAS_H);
  const w = Math.floor(CANVAS_W * scale);
  const h = Math.floor(CANVAS_H * scale);
  canvas.style.width = `${w}px`;
  canvas.style.height = `${h}px`;
  canvas.width = CANVAS_W;
  canvas.height = CANVAS_H;
  if (ctx) ctx.imageSmoothingEnabled = false;
}

// ── Click handling ──

let onFilingCabinetClick: (() => void) | null = null;
export function setOnFilingCabinetClick(cb: () => void): void { onFilingCabinetClick = cb; }

function handleClick(e: MouseEvent): void {
  if (!canvas) return;
  const rect = canvas.getBoundingClientRect();
  const scaleX = CANVAS_W / rect.width;
  const scaleY = CANVAS_H / rect.height;
  const mx = (e.clientX - rect.left) * scaleX;
  const my = (e.clientY - rect.top) * scaleY;

  // Hit-test filing cabinet
  if (mx >= FILING_CABINET.x && mx <= FILING_CABINET.x + FILING_CABINET.w &&
      my >= FILING_CABINET.y && my <= FILING_CABINET.y + FILING_CABINET.h) {
    onFilingCabinetClick?.();
    return;
  }

  // Hit-test agents: bounding box from chair bottom up to head (~55px sprite)
  for (const id of AGENT_IDS) {
    const d = DESKS[id];
    if (mx >= d.x - 25 && mx <= d.x + 25 && my >= d.y - 60 && my <= d.y + 5) {
      selectedAgent = selectedAgent === id ? null : id;
      onSelectAgent?.(selectedAgent);
      return;
    }
  }
  selectedAgent = null;
  onSelectAgent?.(selectedAgent);
}

// ── Game loop ──

function frame(time: number): void {
  if (stopped) return;
  const dt = lastTime === 0 ? 0 : Math.min((time - lastTime) / 1000, 0.1);
  lastTime = time;

  // Update hooks
  for (const hook of updateHooks) hook(dt);

  // Render
  if (ctx) render(ctx, dt);

  requestAnimationFrame(frame);
}

// ── Render ──

function render(c: CanvasRenderingContext2D, dt: number): void {
  c.clearRect(0, 0, CANVAS_W, CANVAS_H);

  // Background
  if (bgImage) {
    c.drawImage(bgImage, 0, 0, CANVAS_W, CANVAS_H);
  } else {
    renderPlaceholderBg(c);
  }

  // Render hooks (characters, phone lines, bubbles)
  for (const hook of renderHooks) hook(c, dt);
}

function renderPlaceholderBg(c: CanvasRenderingContext2D): void {
  // Dark office floor
  c.fillStyle = "#2a2a35";
  c.fillRect(0, 0, CANVAS_W, CANVAS_H);

  // Zone rectangles
  for (const z of ZONES) {
    c.fillStyle = z.color;
    c.fillRect(z.x, z.y, z.w, z.h);
    c.strokeStyle = "rgba(255,255,255,0.08)";
    c.lineWidth = 1;
    c.strokeRect(z.x, z.y, z.w, z.h);
    c.fillStyle = "rgba(255,255,255,0.2)";
    c.font = "bold 10px 'Space Grotesk', sans-serif";
    c.fillText(z.label, z.x + 6, z.y + 14);
  }

  // Mail slot
  c.fillStyle = "rgba(68,136,204,0.3)";
  c.fillRect(MAIL_SLOT.x - 10, MAIL_SLOT.y - 10, 30, 20);
  c.fillStyle = "rgba(255,255,255,0.3)";
  c.font = "8px 'Space Grotesk', sans-serif";
  c.fillText("MAIL", MAIL_SLOT.x - 6, MAIL_SLOT.y + 3);

  // Filing cabinet
  c.fillStyle = "rgba(102,170,136,0.2)";
  c.fillRect(FILING_CABINET.x, FILING_CABINET.y, FILING_CABINET.w, FILING_CABINET.h);
  c.fillStyle = "rgba(255,255,255,0.25)";
  c.font = "9px 'Space Grotesk', sans-serif";
  c.fillText("FILING CABINET", FILING_CABINET.x + 4, FILING_CABINET.y + 14);
}

export function loadBackground(url: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => { bgImage = img; resolve(); };
    img.onerror = reject;
    img.src = url;
  });
}

export function getSelectedAgent(): AgentId | null { return selectedAgent; }
