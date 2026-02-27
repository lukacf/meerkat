// ═══════════════════════════════════════════════════════════
// Map Rendering — SVG-only with period aesthetic
// ═══════════════════════════════════════════════════════════

import type { ArenaState, Team } from "./types";
import { TERRITORIES } from "./territories";

// Warm, muted faction colors — parchment-map style
const TEAM_FILL: Record<Team, string> = {
  france: "#2e5a8a",
  prussia: "#8a7a28",
  russia: "#8a3535",
};
const TEAM_FILL_LT: Record<Team, string> = {
  france: "#3a72aa",
  prussia: "#a89430",
  russia: "#aa4545",
};
const TEAM_STROKE: Record<Team, string> = {
  france: "#5a90cc",
  prussia: "#c4b040",
  russia: "#cc6060",
};

// Adjacency for frontline rendering
export const EDGES: [string, string][] = [
  ["iberia", "paris"], ["iberia", "marseille"],
  ["paris", "marseille"], ["paris", "burgundy"],
  ["marseille", "burgundy"], ["marseille", "bavaria"],
  ["burgundy", "rhineland"], ["burgundy", "bavaria"],
  ["rhineland", "berlin"], ["rhineland", "bavaria"], ["rhineland", "bohemia"],
  ["berlin", "bohemia"], ["berlin", "warsaw"],
  ["bavaria", "bohemia"],
  ["bohemia", "warsaw"],
  ["warsaw", "ukraine"], ["warsaw", "moscow"],
  ["ukraine", "moscow"], ["ukraine", "crimea"],
  ["moscow", "crimea"],
  ["marseille", "crimea"], // sea route
];

export function renderMap(state: ArenaState, targets?: Record<Team, string>, captures?: Set<string>): void {
  const ctrl = new Map(state.regions.map(r => [r.id, r.controller]));

  let svg = `<defs>
    <filter id="glow"><feGaussianBlur stdDeviation="4" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>
    <filter id="parchment">
      <feTurbulence type="fractalNoise" baseFrequency="0.03" numOctaves="4" seed="2" result="noise"/>
      <feColorMatrix type="saturate" values="0" in="noise" result="bw"/>
      <feBlend in="SourceGraphic" in2="bw" mode="multiply" result="textured"/>
      <feComposite in="textured" in2="SourceGraphic" operator="in"/>
    </filter>`;

  // Per-team gradient fills
  for (const team of ["france", "prussia", "russia"] as Team[]) {
    svg += `<linearGradient id="fill-${team}" x1="0" y1="0" x2="0.3" y2="1">
      <stop offset="0%" stop-color="${TEAM_FILL_LT[team]}"/>
      <stop offset="100%" stop-color="${TEAM_FILL[team]}"/>
    </linearGradient>`;
  }

  svg += `</defs>`;

  // Ocean background
  svg += `<rect width="960" height="540" fill="#0c1520"/>`;

  // Territory fills with parchment texture
  for (const region of state.regions) {
    const t = TERRITORIES[region.id];
    if (!t) continue;
    const team = region.controller;
    svg += `<path d="${t.path}" fill="url(#fill-${team})" filter="url(#parchment)" class="territory-path" data-region="${region.id}"/>`;
    svg += `<path d="${t.path}" fill="none" stroke="${TEAM_STROKE[team]}" stroke-width="0.8" stroke-opacity="0.5"/>`;
  }

  // Contested frontlines
  for (const [a, b] of EDGES) {
    if (ctrl.get(a) !== ctrl.get(b)) {
      const ta = TERRITORIES[a], tb = TERRITORIES[b];
      if (ta && tb) {
        const isSea = (a === "marseille" && b === "crimea") || (a === "crimea" && b === "marseille");
        const cls = isSea ? "edge-sea" : "edge-front";
        svg += `<line x1="${ta.cx}" y1="${ta.cy}" x2="${tb.cx}" y2="${tb.cy}" class="${cls}"/>`;
      }
    }
  }

  // Labels
  for (const region of state.regions) {
    const t = TERRITORIES[region.id];
    if (!t) continue;
    svg += `<text class="region-label" x="${t.cx}" y="${t.cy - 5}">${t.label}</text>`;
    svg += `<text class="region-stats" x="${t.cx}" y="${t.cy + 9}">\u2694${region.defense} \u2605${region.value}</text>`;
  }

  // Capture flash
  if (captures) {
    for (const id of captures) {
      const t = TERRITORIES[id];
      const team = ctrl.get(id);
      if (t && team) {
        svg += `<path d="${t.path}" fill="${TEAM_STROKE[team]}" fill-opacity="0" stroke="${TEAM_STROKE[team]}" stroke-width="2" class="captured-flash" filter="url(#glow)"/>`;
      }
    }
  }

  // Target rings
  if (targets) {
    for (const [team, rid] of Object.entries(targets)) {
      const t = TERRITORIES[rid];
      if (t) svg += `<circle cx="${t.cx}" cy="${t.cy}" r="24" class="target-ring ${team}"/>`;
    }
  }

  (document.getElementById("mapSvg") as unknown as SVGSVGElement).innerHTML = svg;
}
