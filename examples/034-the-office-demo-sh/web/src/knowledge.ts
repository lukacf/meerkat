// ═══════════════════════════════════════════════════════════
// Knowledge Graph — Force-directed graph from Archivist facts
// ═══════════════════════════════════════════════════════════

export interface KnowledgeNode {
  id: string;
  label: string;
  type: "person" | "company" | "system" | "date" | "amount" | "event";
  x: number;
  y: number;
  vx: number;
  vy: number;
  age: number; // seconds since creation, for glow animation
}

export interface KnowledgeEdge {
  from: string;
  to: string;
  label: string;
  age: number;
}

const NODE_COLORS: Record<KnowledgeNode["type"], string> = {
  person: "#4488CC",
  company: "#CC8844",
  system: "#44AA66",
  date: "#CCAA44",
  amount: "#CC4488",
  event: "#8866CC",
};

const nodes = new Map<string, KnowledgeNode>();
const edges: KnowledgeEdge[] = [];
let isVisible = false;
let canvas: HTMLCanvasElement | null = null;

// ── Public API ──

export function addFact(subject: string, predicate: string, object: string, subjectType: KnowledgeNode["type"] = "event", objectType: KnowledgeNode["type"] = "event"): void {
  ensureNode(subject, subjectType);
  ensureNode(object, objectType);

  // Avoid duplicate edges
  const exists = edges.some(e => e.from === subject && e.to === object && e.label === predicate);
  if (!exists) {
    edges.push({ from: subject, to: object, label: predicate, age: 0 });
  }
}

function ensureNode(id: string, type: KnowledgeNode["type"]): void {
  if (nodes.has(id)) return;
  const angle = Math.random() * Math.PI * 2;
  const r = 80 + Math.random() * 120;
  nodes.set(id, {
    id,
    label: id,
    type,
    x: 300 + Math.cos(angle) * r,
    y: 250 + Math.sin(angle) * r,
    vx: 0, vy: 0,
    age: 0,
  });
}

export function getNodeCount(): number { return nodes.size; }

/** Extract knowledge facts from archivist messages (heuristic parsing) */
export function extractFacts(content: string): void {
  // Simple heuristic: look for "X is Y" or "X: Y" patterns
  // Extract named entities
  const people = content.match(/(?:Alex|Sam|Pat|Casey|Jim|Max|Dev|Robin|Jordan|Morgan|Aria|Scout|Quinn|Bailey|Sage)\s*\w*/g) || [];
  const companies = content.match(/(?:Acme|CloudCorp|Corp)\s*\w*/gi) || [];

  for (const name of people) {
    ensureNode(name.trim(), "person");
  }
  for (const co of companies) {
    ensureNode(co.trim(), "company");
  }

  // Extract amounts
  const amounts = content.match(/\$[\d,]+(?:\.\d{2})?/g) || [];
  for (const amt of amounts) {
    ensureNode(amt, "amount");
  }

  // Extract dates
  const dates = content.match(/(?:Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday|today|tomorrow|Q[1-4])/gi) || [];
  for (const d of dates) {
    ensureNode(d, "date");
  }
}

// ── Toggle visibility ──

export function toggleKnowledgeGraph(panelContainer: HTMLElement): void {
  isVisible = !isVisible;
  if (isVisible) {
    showGraph(panelContainer);
  } else {
    hideGraph();
  }
}

export function isGraphVisible(): boolean { return isVisible; }

function showGraph(el: HTMLElement): void {
  el.innerHTML = `
    <div style="display:flex;flex-direction:column;height:100%">
      <div style="display:flex;align-items:center;justify-content:space-between;padding:4px 8px;flex-shrink:0">
        <span style="font-size:10px;font-weight:700;color:var(--text-dim);text-transform:uppercase;letter-spacing:0.1em">Knowledge Graph</span>
        <button id="kgClose" style="font-size:9px;background:none;border:1px solid var(--border);border-radius:3px;padding:2px 6px;color:var(--text-dim);cursor:pointer">\u2190 Incidents</button>
      </div>
      <canvas id="kgCanvas" style="flex:1;width:100%;cursor:grab"></canvas>
      <div style="font-size:8px;color:var(--text-muted);padding:4px 8px;text-align:center">${nodes.size} entities \u2022 ${edges.length} relationships</div>
    </div>
  `;
  canvas = document.getElementById("kgCanvas") as HTMLCanvasElement;
  document.getElementById("kgClose")!.addEventListener("click", () => toggleKnowledgeGraph(el));

  if (canvas) {
    const rect = canvas.parentElement!;
    canvas.width = rect.clientWidth;
    canvas.height = rect.clientHeight - 60;
    requestAnimationFrame(graphFrame);
  }
}

function hideGraph(): void {
  isVisible = false;
  canvas = null;
}

// ── Force simulation + render ──

let lastGraphTime = 0;

function graphFrame(time: number): void {
  if (!isVisible || !canvas) return;
  const dt = lastGraphTime === 0 ? 0 : Math.min((time - lastGraphTime) / 1000, 0.05);
  lastGraphTime = time;

  // Age nodes/edges
  for (const [, n] of nodes) n.age += dt;
  for (const e of edges) e.age += dt;

  // Force simulation
  simulate(dt);

  // Render
  const ctx = canvas.getContext("2d")!;
  renderGraph(ctx, canvas.width, canvas.height);

  requestAnimationFrame(graphFrame);
}

function simulate(dt: number): void {
  const nodeList = [...nodes.values()];
  const cx = 300;
  const cy = 200;

  // Repulsion between all nodes
  for (let i = 0; i < nodeList.length; i++) {
    for (let j = i + 1; j < nodeList.length; j++) {
      const a = nodeList[i], b = nodeList[j];
      let dx = b.x - a.x, dy = b.y - a.y;
      let dist = Math.sqrt(dx * dx + dy * dy);
      if (dist < 1) { dx = 1; dy = 0; dist = 1; }
      const force = 2000 / (dist * dist);
      const fx = (dx / dist) * force;
      const fy = (dy / dist) * force;
      a.vx -= fx; a.vy -= fy;
      b.vx += fx; b.vy += fy;
    }
  }

  // Spring attraction along edges
  for (const e of edges) {
    const a = nodes.get(e.from);
    const b = nodes.get(e.to);
    if (!a || !b) continue;
    const dx = b.x - a.x, dy = b.y - a.y;
    const dist = Math.sqrt(dx * dx + dy * dy);
    if (dist < 1) continue;
    const force = (dist - 80) * 0.03;
    const fx = (dx / dist) * force;
    const fy = (dy / dist) * force;
    a.vx += fx; a.vy += fy;
    b.vx -= fx; b.vy -= fy;
  }

  // Gravity toward center
  for (const n of nodeList) {
    n.vx += (cx - n.x) * 0.005;
    n.vy += (cy - n.y) * 0.005;
  }

  // Apply velocity with damping
  for (const n of nodeList) {
    n.vx *= 0.85;
    n.vy *= 0.85;
    n.x += n.vx * dt * 60;
    n.y += n.vy * dt * 60;
    // Clamp to bounds
    n.x = Math.max(30, Math.min(570, n.x));
    n.y = Math.max(30, Math.min(370, n.y));
  }
}

function renderGraph(ctx: CanvasRenderingContext2D, w: number, h: number): void {
  ctx.clearRect(0, 0, w, h);

  // Background
  ctx.fillStyle = "rgba(26,26,36,0.95)";
  ctx.fillRect(0, 0, w, h);

  // Edges
  for (const e of edges) {
    const a = nodes.get(e.from);
    const b = nodes.get(e.to);
    if (!a || !b) continue;

    const alpha = Math.min(1, e.age * 2);
    ctx.strokeStyle = `rgba(255,255,255,${0.15 * alpha})`;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(a.x, a.y);
    ctx.lineTo(b.x, b.y);
    ctx.stroke();

    // Edge label
    if (e.label) {
      const mx = (a.x + b.x) / 2;
      const my = (a.y + b.y) / 2;
      ctx.fillStyle = `rgba(255,255,255,${0.3 * alpha})`;
      ctx.font = "8px 'Space Grotesk', sans-serif";
      ctx.textAlign = "center";
      ctx.fillText(e.label, mx, my - 3);
    }
  }

  // Nodes
  for (const [, n] of nodes) {
    const color = NODE_COLORS[n.type] || "#888";
    const alpha = Math.min(1, n.age * 2);
    const glowRadius = n.age < 1 ? 12 + (1 - n.age) * 8 : 12;

    // Glow
    ctx.beginPath();
    ctx.arc(n.x, n.y, glowRadius, 0, Math.PI * 2);
    ctx.fillStyle = color + "20";
    ctx.fill();

    // Node circle
    ctx.beginPath();
    ctx.arc(n.x, n.y, 8, 0, Math.PI * 2);
    ctx.fillStyle = color;
    ctx.globalAlpha = alpha;
    ctx.fill();
    ctx.globalAlpha = 1;

    // Label
    ctx.fillStyle = `rgba(255,255,255,${0.8 * alpha})`;
    ctx.font = "9px 'Space Grotesk', sans-serif";
    ctx.textAlign = "center";
    ctx.fillText(n.label, n.x, n.y + 18);
  }

  ctx.textAlign = "left";
}
