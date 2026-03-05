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
  // Mail Room (top-left office)
  triage:     { x: 404,  y: 198, phoneX: 506,  phoneY: 106, label: "Max",    zone: "Mail Room" },

  // Archive (top-right office)
  archivist:  { x: 923,  y: 198, phoneX: 976,  phoneY: 106, label: "Sage",   zone: "Archive" },

  // Dept Row (middle-left offices)
  "it-dept":  { x: 96,   y: 435, phoneX: 166,  phoneY: 343, label: "Dev",    zone: "Dept Row" },
  "hr-dept":  { x: 435,  y: 435, phoneX: 488,  phoneY: 343, label: "Robin",  zone: "Dept Row" },

  // Dept Row (bottom-left offices)
  facilities: { x: 96,   y: 666, phoneX: 168,  phoneY: 611, label: "Jordan", zone: "Dept Row" },
  finance:    { x: 264,  y: 666, phoneX: 334,  phoneY: 611, label: "Morgan", zone: "Dept Row" },

  // PA Bullpen (middle-right row of desks)
  "alex-pa":  { x: 962,  y: 435, phoneX: 1015, phoneY: 354, label: "Aria",   zone: "PA Bullpen" },
  "sam-pa":   { x: 1088, y: 435, phoneX: 1115, phoneY: 354, label: "Scout",  zone: "PA Bullpen" },
  "pat-pa":   { x: 1217, y: 435, phoneX: 1245, phoneY: 354, label: "Quinn",  zone: "PA Bullpen" },

  // Compliance desk (center)
  gate:       { x: 688,  y: 622, phoneX: 762,  phoneY: 541, label: "Bailey", zone: "Compliance" },
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
