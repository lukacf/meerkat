// ═══════════════════════════════════════════════════════════
// Sprites — Multi-frame animated character sprites
// Each character has typing (idle) and phone (on_call) animations
// with 4-6 frames each. Anchor: bottom-center.
// ═══════════════════════════════════════════════════════════

import { AGENTS, AGENT_IDS } from "../types";
import type { AgentId, CharacterState } from "../types";
import { CANVAS_W } from "./layout";

// ── Animation data ──

type AnimFrames = HTMLImageElement[];

interface CharacterAnims {
  typing: AnimFrames;
  phone: AnimFrames;
}

const animations = new Map<AgentId, CharacterAnims>();

function loadImage(src: string): Promise<HTMLImageElement | null> {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => resolve(null);
    img.src = src;
  });
}

export async function loadSprites(): Promise<void> {
  // Discover and load all sprite frames: {agentId}_{action}_{frameNum}.png
  for (const id of AGENT_IDS) {
    const anims: CharacterAnims = { typing: [], phone: [] };

    for (const action of ["typing", "phone"] as const) {
      // Try loading up to 6 frames per action (00–05)
      for (let i = 0; i < 6; i++) {
        const padded = String(i).padStart(2, "0");
        const img = await loadImage(`./sprites/${id}_${action}_${padded}.png`);
        if (img) {
          anims[action].push(img);
        }
      }
    }

    animations.set(id, anims);
  }

  let total = 0;
  for (const [, a] of animations) {
    total += a.typing.length + a.phone.length;
  }
  console.log(`Loaded ${total} sprite frames across ${animations.size} characters`);
}

/**
 * Draw a character at (x, y) = bottom-center anchor (floor level).
 * Selects animation based on state, advances frame based on frame counter.
 */
export function drawCharacter(
  ctx: CanvasRenderingContext2D,
  agentId: AgentId,
  x: number,
  y: number,
  state: CharacterState,
  frame: number,
  selected: boolean,
): void {
  const meta = AGENTS[agentId];
  const color = meta.color;
  const anims = animations.get(agentId);

  ctx.save();

  // Pick the right animation sequence
  let frames: AnimFrames = [];
  if (anims) {
    if (state === "on_call" && anims.phone.length > 0) {
      frames = anims.phone;
    } else if (anims.typing.length > 0) {
      frames = anims.typing;  // typing for both idle and thinking
    }
  }

  // Draw the current frame
  if (frames.length > 0) {
    const idx = frame % frames.length;
    const img = frames[idx];
    const drawX = Math.round(x - img.width / 2);
    const drawY = Math.round(y - img.height);
    ctx.drawImage(img, drawX, drawY);
  } else {
    // Fallback: colored dot
    ctx.beginPath();
    ctx.arc(x, y - 20, 8, 0, Math.PI * 2);
    ctx.fillStyle = color;
    ctx.fill();
  }

  // ── Selection ring ──
  if (selected) {
    ctx.beginPath();
    ctx.ellipse(x, y - 2, 24, 9, 0, 0, Math.PI * 2);
    ctx.strokeStyle = color;
    ctx.lineWidth = 2;
    ctx.globalAlpha = 0.6 + 0.2 * Math.sin(Date.now() / 300);
    ctx.stroke();
    ctx.globalAlpha = 1;
  }

  // ── Activity indicator ──
  const indicatorY = frames.length > 0 ? y - (frames[0]?.height ?? 50) - 6 : y - 50;
  if (state === "on_call") {
    const pulse = 0.6 + 0.4 * Math.sin(Date.now() / 200);
    ctx.beginPath();
    ctx.arc(x, indicatorY, 5, 0, Math.PI * 2);
    ctx.fillStyle = `rgba(68, 255, 100, ${pulse})`;
    ctx.fill();
    ctx.strokeStyle = "#2A8A3A";
    ctx.lineWidth = 1;
    ctx.stroke();
  } else if (state === "thinking") {
    const pulse = 0.5 + 0.5 * Math.sin(Date.now() / 250);
    ctx.beginPath();
    ctx.arc(x, indicatorY, 5, 0, Math.PI * 2);
    ctx.fillStyle = `rgba(255, 180, 60, ${pulse})`;
    ctx.fill();
    ctx.strokeStyle = "#AA7722";
    ctx.lineWidth = 1;
    ctx.stroke();
  }

  // ── Name label ──
  const title = meta.name;
  const subtitle = meta.role;
  ctx.textAlign = "center";

  ctx.font = "700 21px 'Chakra Petch', 'Space Grotesk', sans-serif";
  const titleW = ctx.measureText(title).width;
  ctx.font = "600 15px 'Space Grotesk', sans-serif";
  const subW = ctx.measureText(subtitle).width;

  const contentW = Math.max(titleW, selected ? subW : 0);
  const padX = 10;
  const pillW = Math.ceil(contentW + padX * 2);
  const pillH = selected ? 42 : 28;
  const unclampedX = x - pillW / 2;
  const pillX = Math.min(CANVAS_W - 6 - pillW, Math.max(6, unclampedX));
  const textX = pillX + pillW / 2;
  const pillY = y + 7;

  ctx.fillStyle = selected ? "rgba(12, 17, 26, 0.9)" : "rgba(12, 17, 26, 0.76)";
  roundRect(ctx, pillX, pillY, pillW, pillH, 5);
  ctx.fill();
  ctx.strokeStyle = selected ? "rgba(255,255,255,0.26)" : "rgba(255,255,255,0.16)";
  ctx.lineWidth = 1;
  roundRect(ctx, pillX, pillY, pillW, pillH, 5);
  ctx.stroke();

  ctx.fillStyle = color;
  ctx.fillRect(pillX + 2, pillY + 2, 3, pillH - 4);

  ctx.font = "700 21px 'Chakra Petch', 'Space Grotesk', sans-serif";
  ctx.strokeStyle = "rgba(0, 0, 0, 0.55)";
  ctx.lineWidth = 4;
  ctx.strokeText(title, textX, pillY + 20);
  ctx.fillStyle = "rgba(245, 250, 255, 0.98)";
  ctx.fillText(title, textX, pillY + 20);

  if (selected) {
    ctx.font = "600 15px 'Space Grotesk', sans-serif";
    ctx.fillStyle = "rgba(210, 220, 235, 0.92)";
    ctx.fillText(subtitle, textX, pillY + 35);
  }

  ctx.textAlign = "left";
  ctx.restore();
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
