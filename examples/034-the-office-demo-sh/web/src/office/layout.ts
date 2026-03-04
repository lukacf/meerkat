// ═══════════════════════════════════════════════════════════
// Office Layout — Precise positions from 3x-zoom grid analysis
// Anchor = bottom-center (chair wheels on floor below desk)
// All coordinates in 1376x768 canvas space
// ═══════════════════════════════════════════════════════════

import type { AgentId } from "../types";

export const CANVAS_W = 1376;
export const CANVAS_H = 768;

export interface DeskPos {
  x: number;
  y: number;
  phoneX: number;
  phoneY: number;
  label: string;
  zone: string;
}

export const DESKS: Record<AgentId, DeskPos> = {
  // Mail Room — right desk, chair on floor below desk drawers (front edge y≈150)
  triage:     { x: 335, y: 195, phoneX: 320, phoneY: 110, label: "Max",    zone: "Mail Room" },

  // Archive — desk at top-right with lamp (front edge y≈150)
  archivist:  { x: 720, y: 195, phoneX: 740, phoneY: 110, label: "Sage",   zone: "Archive" },

  // Dept Row TL — chair below desk+monitor area (keyboard at y≈355, floor at y≈365)
  "it-dept":  { x: 95,  y: 400, phoneX: 110, phoneY: 320, label: "Dev",    zone: "Dept Row" },

  // Dept Row TR — mirror of TL
  "hr-dept":  { x: 290, y: 400, phoneX: 260, phoneY: 320, label: "Robin",  zone: "Dept Row" },

  // Dept Row BL — below lower desks (keyboard at y≈455)
  facilities: { x: 65,  y: 490, phoneX: 85,  phoneY: 430, label: "Jordan", zone: "Dept Row" },

  // Dept Row BR — mirror of BL
  finance:    { x: 260, y: 490, phoneX: 230, phoneY: 430, label: "Morgan", zone: "Dept Row" },

  // PA Bullpen — 3 of 4 desks, desk front at y≈385, keyboard at y≈390
  "alex-pa":  { x: 470, y: 415, phoneX: 490, phoneY: 355, label: "Aria",   zone: "PA Bullpen" },
  "sam-pa":   { x: 625, y: 415, phoneX: 640, phoneY: 355, label: "Scout",  zone: "PA Bullpen" },
  "pat-pa":   { x: 775, y: 415, phoneX: 790, phoneY: 355, label: "Quinn",  zone: "PA Bullpen" },

  // Compliance — desk surface at y≈535, floor below at y≈580
  gate:       { x: 590, y: 590, phoneX: 570, phoneY: 540, label: "Bailey", zone: "Compliance" },
};

export const MAIL_SLOT = { x: 45, y: 55 };
export const FILING_CABINET = { x: 920, y: 40, w: 180, h: 120 };

export const ZONES: { label: string; x: number; y: number; w: number; h: number; color: string }[] = [
  { label: "Mail Room",   x: 10,  y: 10,  w: 430, h: 225, color: "rgba(68,136,204,0.08)" },
  { label: "Archive",     x: 560, y: 10,  w: 540, h: 225, color: "rgba(102,170,136,0.08)" },
  { label: "Dept Row",    x: 10,  y: 245, w: 480, h: 250, color: "rgba(170,136,68,0.08)" },
  { label: "PA Bullpen",  x: 420, y: 245, w: 620, h: 180, color: "rgba(204,68,102,0.08)" },
  { label: "Compliance",  x: 450, y: 460, w: 400, h: 240, color: "rgba(204,68,68,0.08)" },
  { label: "Work Area",   x: 10,  y: 500, w: 440, h: 260, color: "rgba(100,100,100,0.06)" },
  { label: "rkat.ai",     x: 860, y: 460, w: 500, h: 300, color: "rgba(100,100,100,0.06)" },
];
