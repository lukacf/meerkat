#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..", "..");
const outputDir = path.join(
  repoRoot,
  "docs",
  "architecture",
  "two-kernel-research",
  "posters",
);

const MACHINE_SPECS = [
  {
    id: "meerkat_machine",
    title: "MeerkatMachine",
    subtitle: "Runtime control plane kernel",
    tlaPath: path.join(
      repoRoot,
      "specs",
      "machines",
      "meerkat_machine",
      "model.tla",
    ),
    contractPath: path.join(
      repoRoot,
      "specs",
      "machines",
      "meerkat_machine",
      "contract.md",
    ),
    accent: "#c3513c",
  },
  {
    id: "mob_machine",
    title: "MobMachine",
    subtitle: "Identity-first orchestration kernel",
    tlaPath: path.join(
      repoRoot,
      "specs",
      "machines",
      "mob_machine",
      "model.tla",
    ),
    contractPath: path.join(
      repoRoot,
      "specs",
      "machines",
      "mob_machine",
      "contract.md",
    ),
    accent: "#355f73",
  },
];

const THEME = {
  paper: "#f4f0e7",
  panel: "#fbfaf6",
  card: "#fffdf8",
  border: "#cfc8bb",
  ink: "#181716",
  muted: "#6d6a63",
  rule: "#d7d0c3",
  input: "#2c5668",
  signal: "#b3791b",
  effect: "#4f7259",
  phase: "#ece6da",
  shadow: "rgba(24, 23, 22, 0.08)",
};

main();

function main() {
  fs.mkdirSync(outputDir, { recursive: true });

  const posters = MACHINE_SPECS.map((spec) => buildPoster(spec));
  for (const poster of posters) {
    fs.writeFileSync(
      path.join(outputDir, `${poster.id}.html`),
      renderPosterHtml(poster),
      "utf8",
    );
  }
  fs.writeFileSync(
    path.join(outputDir, "index.html"),
    renderIndexHtml(posters),
    "utf8",
  );

  for (const poster of posters) {
    console.log(`generated ${path.relative(repoRoot, path.join(outputDir, `${poster.id}.html`))}`);
  }
  console.log(`generated ${path.relative(repoRoot, path.join(outputDir, "index.html"))}`);
}

function buildPoster(spec) {
  const contract = parseContract(fs.readFileSync(spec.contractPath, "utf8"));
  const tla = parseTla(fs.readFileSync(spec.tlaPath, "utf8"));

  const transitionRecords = contract.transitions.map((transition, index) => {
    const tlaDef = tla.definitions.get(transition.name);
    const writes = tlaDef ? extractWrites(tlaDef.body) : [];
    const clauses = tlaDef ? extractClauses(tlaDef.body) : [];
    const phaseShift =
      transition.from.length === 1 && transition.from[0] === transition.to
        ? "loop"
        : "handoff";
    return {
      ...transition,
      key: `${transition.name}-${index}`,
      kind: contract.inputNames.has(transition.onName) ? "input" : "signal",
      writes,
      clauses,
      body: tlaDef?.body ?? "",
      phaseShift,
    };
  });

  const transitionsByPhase = new Map(
    contract.phases.map((phase) => [phase, []]),
  );
  for (const record of transitionRecords) {
    for (const phase of record.from) {
      if (!transitionsByPhase.has(phase)) {
        transitionsByPhase.set(phase, []);
      }
      transitionsByPhase.get(phase).push(record);
    }
  }
  for (const [phase, records] of transitionsByPhase) {
    records.sort((a, b) => {
      if (a.kind !== b.kind) {
        return a.kind.localeCompare(b.kind);
      }
      return a.name.localeCompare(b.name);
    });
  }

  const invariants = contract.invariants.map((name) => ({
    name,
    formula: tla.invariants.get(name) ?? "",
  }));

  return {
    ...spec,
    generatedAt: new Date().toISOString(),
    phases: contract.phases,
    stateFields: contract.stateFields,
    inputs: contract.inputs,
    signals: contract.signals,
    effects: contract.effects,
    invariants,
    transitions: transitionRecords,
    transitionsByPhase,
    overviewSvg: renderOverviewSvg(contract.phases, transitionRecords, spec.accent),
  };
}

function parseContract(text) {
  const lines = text.split(/\r?\n/);
  let section = "";
  let index = 0;
  const stateFields = [];
  const inputs = [];
  const signals = [];
  const effects = [];
  const invariants = [];
  const transitions = [];
  let phases = [];

  while (index < lines.length) {
    const line = lines[index];
    if (line.startsWith("## ")) {
      section = line.slice(3).trim();
      index += 1;
      continue;
    }

    if (section === "State") {
      if (line.startsWith("- Phase enum:")) {
        phases = extractBackticked(line)[0].split("|").map((item) => item.trim());
      } else if (line.startsWith("- `")) {
        const match = line.match(/^- `([^`]+)`: `([^`]+)`/);
        if (match) {
          stateFields.push({ name: match[1], type: match[2] });
        }
      }
    } else if (section === "Inputs") {
      const item = parseSignatureBullet(line);
      if (item) {
        inputs.push(item);
      }
    } else if (section === "Signals") {
      const item = parseSignatureBullet(line);
      if (item) {
        signals.push(item);
      }
    } else if (section === "Effects") {
      const item = parseSignatureBullet(line);
      if (item) {
        effects.push(item);
      }
    } else if (section === "Invariants") {
      const item = parseSignatureBullet(line);
      if (item) {
        invariants.push(item.name);
      }
    } else if (section === "Transitions" && line.startsWith("### `")) {
      const [transition, nextIndex] = parseTransitionBlock(lines, index);
      transitions.push(transition);
      index = nextIndex;
      continue;
    }

    index += 1;
  }

  return {
    phases,
    stateFields,
    inputs,
    signals,
    effects,
    invariants,
    transitions,
    inputNames: new Set(inputs.map((item) => item.name)),
    signalNames: new Set(signals.map((item) => item.name)),
  };
}

function parseTransitionBlock(lines, start) {
  const name = lines[start].match(/^### `([^`]+)`$/)?.[1] ?? "Unknown";
  const transition = {
    name,
    from: [],
    onName: "",
    onSignature: "",
    onArgs: [],
    to: "",
    emits: [],
    guards: [],
  };
  let index = start + 1;
  while (index < lines.length) {
    const line = lines[index];
    if (line.startsWith("### `") || line.startsWith("## ")) {
      break;
    }
    if (line.startsWith("- From:")) {
      transition.from = extractBackticked(line);
    } else if (line.startsWith("- On:")) {
      const match = line.match(/^- On: `([^`]+)`(?:\((.*)\))?/);
      if (match) {
        transition.onName = match[1];
        transition.onSignature = `${match[1]}(${match[2] ?? ""})`;
        transition.onArgs = match[2]
          ? match[2]
              .split(",")
              .map((part) => part.trim())
              .filter(Boolean)
          : [];
      }
    } else if (line.startsWith("- Emits:")) {
      transition.emits = extractBackticked(line);
    } else if (line.startsWith("- To:")) {
      transition.to = extractBackticked(line)[0] ?? "";
    } else if (line.startsWith("- Guards:")) {
      index += 1;
      while (index < lines.length && lines[index].startsWith("  - ")) {
        const guardMatch = lines[index].match(/^  - `([^`]+)`$/);
        if (guardMatch) {
          transition.guards.push(guardMatch[1]);
        } else {
          transition.guards.push(lines[index].replace(/^  - /, "").trim());
        }
        index += 1;
      }
      continue;
    }
    index += 1;
  }
  return [transition, index];
}

function parseTla(text) {
  const lines = text.split(/\r?\n/);
  const variableLine = lines.find((line) => line.startsWith("VARIABLES ")) ?? "";
  const variables = variableLine
    .replace(/^VARIABLES /, "")
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);

  const definitions = new Map();
  const defIndices = [];
  for (let index = 0; index < lines.length; index += 1) {
    const match = lines[index].match(/^([A-Za-z][A-Za-z0-9_]*)(?:\((.*?)\))? ==$/);
    if (match) {
      defIndices.push({
        index,
        name: match[1],
        params: match[2] ? match[2].split(",").map((part) => part.trim()).filter(Boolean) : [],
      });
    }
  }
  for (let i = 0; i < defIndices.length; i += 1) {
    const current = defIndices[i];
    const end = i + 1 < defIndices.length ? defIndices[i + 1].index : lines.length;
    const body = lines.slice(current.index + 1, end).join("\n").trimEnd();
    definitions.set(current.name, { params: current.params, body });
  }

  const nextStart = lines.findIndex((line) => line === "Next ==");
  const nextLines = [];
  if (nextStart >= 0) {
    for (let index = nextStart + 1; index < lines.length; index += 1) {
      const line = lines[index];
      if (/^[a-z_][A-Za-z0-9_]* ==/.test(line) || line.startsWith("CiStateConstraint ==")) {
        break;
      }
      nextLines.push(line);
    }
  }

  const invariants = new Map();
  const theoremStart = lines.findIndex((line) => line.startsWith("THEOREM "));
  const invariantZone = theoremStart >= 0 ? lines.slice(0, theoremStart) : lines;
  for (const line of invariantZone) {
    const match = line.match(/^([a-z_][A-Za-z0-9_]*) == (.*)$/);
    if (match) {
      invariants.set(match[1], match[2]);
    }
  }

  return {
    variables,
    definitions,
    next: nextLines,
    invariants,
  };
}

function extractWrites(body) {
  const matches = [...body.matchAll(/\b([A-Za-z_][A-Za-z0-9_]*)'\s*=/g)].map((match) => match[1]);
  return [...new Set(matches)];
}

function extractClauses(body) {
  const clauses = [];
  for (const rawLine of body.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line.startsWith("/\\")) {
      continue;
    }
    const clause = line.replace(/^\/\\\s*/, "").trim();
    if (!clause || clause.startsWith("UNCHANGED")) {
      continue;
    }
    if (/\bmodel_step_count'\s*=/.test(clause)) {
      continue;
    }
    if (clause.includes("' =")) {
      continue;
    }
    clauses.push(clause);
  }
  return clauses;
}

function renderOverviewSvg(phases, transitions, accent) {
  const width = 1800;
  const height = 280;
  const marginX = 120;
  const nodeWidth = 170;
  const nodeHeight = 56;
  const laneY = 156;
  const step = phases.length > 1 ? (width - marginX * 2 - nodeWidth) / (phases.length - 1) : 0;
  const nodes = phases.map((phase, index) => ({
    phase,
    x: marginX + index * step,
    y: laneY,
  }));

  const edgeMap = new Map();
  for (const transition of transitions) {
    for (const from of transition.from) {
      const key = `${from}->${transition.to}`;
      if (!edgeMap.has(key)) {
        edgeMap.set(key, {
          from,
          to: transition.to,
          count: 0,
          inputCount: 0,
          signalCount: 0,
        });
      }
      const edge = edgeMap.get(key);
      edge.count += 1;
      if (transition.kind === "input") {
        edge.inputCount += 1;
      } else {
        edge.signalCount += 1;
      }
    }
  }

  const svg = [];
  svg.push(
    `<svg viewBox="0 0 ${width} ${height}" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="Phase wiring overview">`,
  );
  svg.push(
    `<rect x="0" y="0" width="${width}" height="${height}" rx="28" fill="${THEME.panel}" stroke="${THEME.border}" />`,
  );
  svg.push(
    `<text x="64" y="46" fill="${THEME.ink}" font-family="system-ui, -apple-system, sans-serif" font-size="28" font-weight="700">Phase transfer schematic</text>`,
  );
  svg.push(
    `<text x="64" y="76" fill="${THEME.muted}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="16">Aggregated transition count per from → to edge. Solid = mostly inputs, dashed = mostly signals.</text>`,
  );

  for (const edge of edgeMap.values()) {
    const source = nodes.find((node) => node.phase === edge.from);
    const target = nodes.find((node) => node.phase === edge.to);
    if (!source || !target) {
      continue;
    }
    const mostlySignal = edge.signalCount > edge.inputCount;
    const stroke = mostlySignal ? THEME.signal : accent;
    const dash = mostlySignal ? `stroke-dasharray="10 8"` : "";
    if (edge.from === edge.to) {
      const x = source.x + nodeWidth / 2;
      const y = source.y - 12;
      const loopPath = [
        `M ${x - 36} ${y}`,
        `C ${x - 60} ${y - 72}, ${x + 60} ${y - 72}, ${x + 36} ${y}`,
      ].join(" ");
      svg.push(
        `<path d="${loopPath}" fill="none" stroke="${stroke}" stroke-width="3.5" ${dash} />`,
      );
      svg.push(
        `<text x="${x}" y="${y - 70}" fill="${stroke}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="15" text-anchor="middle">${edge.count}</text>`,
      );
    } else {
      const startX = source.x + nodeWidth / 2;
      const endX = target.x + nodeWidth / 2;
      const direction = endX > startX ? 1 : -1;
      const curvature = Math.max(36, Math.abs(endX - startX) * 0.18);
      const baseY = source.y - 24;
      const controlY = baseY - (direction > 0 ? 48 : 72);
      const path = [
        `M ${startX} ${baseY}`,
        `C ${startX + curvature * direction} ${controlY}, ${endX - curvature * direction} ${controlY}, ${endX} ${baseY}`,
      ].join(" ");
      svg.push(
        `<path d="${path}" fill="none" stroke="${stroke}" stroke-width="3.25" ${dash} marker-end="url(#arrow-${mostlySignal ? "signal" : "input"})" />`,
      );
      svg.push(
        `<text x="${(startX + endX) / 2}" y="${controlY - 8}" fill="${stroke}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="15" text-anchor="middle">${edge.count}</text>`,
      );
    }
  }

  svg.push(
    `<defs>
      <marker id="arrow-input" markerWidth="12" markerHeight="12" refX="10" refY="6" orient="auto">
        <path d="M0,0 L12,6 L0,12 z" fill="${accent}" />
      </marker>
      <marker id="arrow-signal" markerWidth="12" markerHeight="12" refX="10" refY="6" orient="auto">
        <path d="M0,0 L12,6 L0,12 z" fill="${THEME.signal}" />
      </marker>
    </defs>`,
  );

  for (const node of nodes) {
    svg.push(
      `<rect x="${node.x}" y="${node.y}" width="${nodeWidth}" height="${nodeHeight}" rx="18" fill="${THEME.phase}" stroke="${THEME.border}" />`,
    );
    svg.push(
      `<text x="${node.x + nodeWidth / 2}" y="${node.y + 24}" fill="${THEME.ink}" font-family="system-ui, -apple-system, sans-serif" font-size="18" font-weight="700" text-anchor="middle">${escapeHtml(
        node.phase,
      )}</text>`,
    );
    const count = transitions.filter((transition) => transition.from.includes(node.phase)).length;
    svg.push(
      `<text x="${node.x + nodeWidth / 2}" y="${node.y + 42}" fill="${THEME.muted}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="12" text-anchor="middle">${count} transitions</text>`,
    );
  }

  svg.push(`</svg>`);
  return svg.join("");
}

function renderPosterHtml(poster) {
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>${escapeHtml(poster.title)} poster</title>
    <style>
      ${renderStyles(poster.accent, poster.phases.length)}
    </style>
  </head>
  <body>
    <main class="poster-shell" style="--phase-count:${poster.phases.length}">
      <header class="masthead">
        <div class="masthead__eyebrow">Generated machine poster</div>
        <div class="masthead__row">
          <div>
            <a class="back-link" href="./index.html">← Poster index</a>
            <h1>${escapeHtml(poster.title)}</h1>
            <p class="subtitle">${escapeHtml(poster.subtitle)}</p>
          </div>
          <div class="meta-stack">
            <div class="meta-chip"><span>Source</span><strong>${escapeHtml(
              path.relative(repoRoot, poster.tlaPath),
            )}</strong></div>
            <div class="meta-chip"><span>Contract</span><strong>${escapeHtml(
              path.relative(repoRoot, poster.contractPath),
            )}</strong></div>
            <div class="meta-chip"><span>Generated</span><strong>${escapeHtml(
              poster.generatedAt.replace("T", " ").replace("Z", " UTC"),
            )}</strong></div>
          </div>
        </div>
        <div class="stat-ribbon">
          ${renderStat("phases", poster.phases.length)}
          ${renderStat("transitions", poster.transitions.length)}
          ${renderStat("inputs", poster.inputs.length)}
          ${renderStat("signals", poster.signals.length)}
          ${renderStat("effects", poster.effects.length)}
          ${renderStat("invariants", poster.invariants.length)}
          ${renderStat("state vars", poster.stateFields.length)}
        </div>
      </header>

      <section class="overview-panel">
        ${poster.overviewSvg}
      </section>

      <section class="summary-grid">
        <section class="panel">
          <div class="panel__header">State vector</div>
          <div class="field-list">
            ${poster.stateFields
              .map(
                (field) => `<div class="field-pill"><strong>${escapeHtml(field.name)}</strong><span>${escapeHtml(
                  field.type,
                )}</span></div>`,
              )
              .join("")}
          </div>
        </section>
        <section class="panel">
          <div class="panel__header">Invariants</div>
          <div class="invariant-list">
            ${poster.invariants
              .map(
                (invariant) => `<article class="invariant-card"><h3>${escapeHtml(
                  invariant.name,
                )}</h3><pre>${escapeHtml(invariant.formula || "—")}</pre></article>`,
              )
              .join("")}
          </div>
        </section>
        <section class="panel">
          <div class="panel__header panel__header--input">Inputs</div>
          <div class="alphabet-list">
            ${poster.inputs
              .map((item) => renderAlphabetItem(item, "input"))
              .join("")}
          </div>
        </section>
        <section class="panel">
          <div class="panel__header panel__header--signal">Signals</div>
          <div class="alphabet-list">
            ${poster.signals
              .map((item) => renderAlphabetItem(item, "signal"))
              .join("")}
          </div>
        </section>
      </section>

      <section class="board">
        ${poster.phases
          .map((phase) => {
            const cards = poster.transitionsByPhase.get(phase) ?? [];
            return `<section class="phase-column">
              <header class="phase-column__header">
                <div class="phase-name">${escapeHtml(phase)}</div>
                <div class="phase-count">${cards.length} legal transitions</div>
              </header>
              <div class="phase-column__cards">
                ${cards.map((card) => renderTransitionCard(card)).join("")}
              </div>
            </section>`;
          })
          .join("")}
      </section>

      <footer class="footer-note">
        <div>Poster design: generated from canonical TLA transition bodies with normalized contract metadata.</div>
        <div>Regenerate with <code>node scripts/machine-posters/generate-machine-posters.mjs</code>.</div>
      </footer>
    </main>
  </body>
</html>`;
}

function renderIndexHtml(posters) {
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Machine posters</title>
    <style>
      body {
        margin: 0;
        min-height: 100vh;
        background: ${THEME.paper};
        color: ${THEME.ink};
        font-family: "Avenir Next", "Helvetica Neue", Arial, sans-serif;
      }
      main {
        max-width: 1080px;
        margin: 0 auto;
        padding: 48px 32px 72px;
      }
      h1 {
        margin: 0 0 12px;
        font-size: 48px;
        letter-spacing: -0.04em;
      }
      p {
        margin: 0 0 28px;
        color: ${THEME.muted};
        max-width: 48rem;
        line-height: 1.5;
      }
      .grid {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
        gap: 20px;
      }
      a.card {
        display: block;
        padding: 28px;
        border-radius: 22px;
        border: 1px solid ${THEME.border};
        background: ${THEME.panel};
        color: inherit;
        text-decoration: none;
        box-shadow: 0 20px 36px rgba(24, 23, 22, 0.06);
      }
      a.card:hover {
        transform: translateY(-2px);
        box-shadow: 0 26px 40px rgba(24, 23, 22, 0.08);
      }
      .eyebrow {
        color: ${THEME.muted};
        font-size: 12px;
        letter-spacing: 0.18em;
        text-transform: uppercase;
      }
      .card h2 {
        margin: 12px 0 10px;
        font-size: 30px;
        letter-spacing: -0.03em;
      }
      .card small {
        display: block;
        margin-top: 18px;
        color: ${THEME.muted};
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
      }
    </style>
  </head>
  <body>
    <main>
      <div class="eyebrow">Generated posters</div>
      <h1>Canonical machine posters</h1>
      <p>
        Large-format, Dieter Rams-inspired machine posters generated from the
        canonical formal models. Each view combines the normalized input/signal
        alphabet with raw TLA-derived transition writes, guards, and phase
        transfers.
      </p>
      <div class="grid">
        ${posters
          .map(
            (poster) => `<a class="card" href="./${poster.id}.html">
              <div class="eyebrow">${escapeHtml(poster.subtitle)}</div>
              <h2>${escapeHtml(poster.title)}</h2>
              <p>${poster.phases.length} phases · ${poster.transitions.length} transitions · ${poster.inputs.length} inputs · ${poster.signals.length} signals</p>
              <small>${escapeHtml(path.relative(repoRoot, poster.tlaPath))}</small>
            </a>`,
          )
          .join("")}
      </div>
    </main>
  </body>
</html>`;
}

function renderTransitionCard(card) {
  const kindLabel = card.kind === "input" ? "INPUT" : "SIGNAL";
  const writes = card.writes.filter((name) => name !== "model_step_count");
  const guardMarkup =
    card.guards.length > 0
      ? `<ul class="guard-list">${card.guards
          .map((guard) => `<li>${escapeHtml(guard)}</li>`)
          .join("")}</ul>`
      : `<div class="guard-empty">No named guard beyond phase legality.</div>`;
  const clauseMarkup =
    card.clauses.length > 0
      ? `<pre class="clause-block">${escapeHtml(card.clauses.slice(0, 3).join("\n"))}</pre>`
      : `<pre class="clause-block clause-block--empty">No additional raw guard clauses.</pre>`;
  return `<article class="transition-card transition-card--${card.kind}">
    <header class="transition-card__header">
      <span class="kind-pill kind-pill--${card.kind}">${kindLabel}</span>
      <span class="phase-shift phase-shift--${card.phaseShift}">${escapeHtml(card.to)}</span>
    </header>
    <h3>${escapeHtml(card.name)}</h3>
    <div class="trigger-line">${escapeHtml(card.onSignature || `${card.onName}()`)} </div>
    <div class="detail-grid">
      <div>
        <div class="detail-label">Guards</div>
        ${guardMarkup}
      </div>
      <div>
        <div class="detail-label">Writes</div>
        <div class="write-list">${writes.length ? writes.map((name) => `<span>${escapeHtml(name)}</span>`).join("") : `<span>phase only</span>`}</div>
      </div>
    </div>
    ${card.emits.length ? `<div class="effect-row"><span class="detail-label">Effects</span>${card.emits.map((effect) => `<span class="effect-pill">${escapeHtml(effect)}</span>`).join("")}</div>` : ""}
    <div class="detail-label">TLA clauses</div>
    ${clauseMarkup}
  </article>`;
}

function renderAlphabetItem(item, kind) {
  return `<div class="alphabet-item alphabet-item--${kind}">
    <strong>${escapeHtml(item.name)}</strong>
    <span>${escapeHtml(item.signature || `${item.name}()` )}</span>
  </div>`;
}

function renderStat(label, value) {
  return `<div class="stat-chip"><span>${escapeHtml(label)}</span><strong>${value}</strong></div>`;
}

function renderStyles(accent, phaseCount) {
  return `
      :root {
        --paper: ${THEME.paper};
        --panel: ${THEME.panel};
        --card: ${THEME.card};
        --border: ${THEME.border};
        --ink: ${THEME.ink};
        --muted: ${THEME.muted};
        --rule: ${THEME.rule};
        --input: ${THEME.input};
        --signal: ${THEME.signal};
        --effect: ${THEME.effect};
        --phase: ${THEME.phase};
        --accent: ${accent};
      }
      * { box-sizing: border-box; }
      body {
        margin: 0;
        min-height: 100vh;
        background:
          radial-gradient(circle at top left, rgba(255,255,255,0.6), transparent 42%),
          linear-gradient(180deg, #d8d2c6 0%, #e7e2d8 18%, var(--paper) 54%, #ebe6dd 100%);
        color: var(--ink);
        font-family: "Avenir Next", "Helvetica Neue", Arial, sans-serif;
      }
      code {
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
      }
      .poster-shell {
        width: max(calc(${phaseCount} * 295px + 520px), 96vw);
        max-width: 3400px;
        margin: 18px auto 40px;
        padding: 28px 30px 40px;
        background: var(--paper);
        border: 1px solid rgba(24, 23, 22, 0.16);
        box-shadow:
          0 24px 50px rgba(24, 23, 22, 0.08),
          inset 0 1px 0 rgba(255,255,255,0.75);
      }
      .masthead {
        border-bottom: 1px solid var(--rule);
        padding-bottom: 18px;
      }
      .masthead__eyebrow {
        color: var(--muted);
        font-size: 12px;
        letter-spacing: 0.22em;
        text-transform: uppercase;
        margin-bottom: 12px;
      }
      .back-link {
        display: inline-block;
        margin-bottom: 18px;
        color: var(--accent);
        text-decoration: none;
        font-size: 12px;
        letter-spacing: 0.18em;
        text-transform: uppercase;
      }
      .masthead__row {
        display: flex;
        justify-content: space-between;
        gap: 28px;
        align-items: flex-start;
      }
      h1 {
        margin: 0;
        font-size: clamp(56px, 5vw, 92px);
        font-weight: 700;
        letter-spacing: -0.06em;
        line-height: 0.96;
      }
      .subtitle {
        margin: 10px 0 0;
        color: var(--muted);
        font-size: 19px;
        line-height: 1.4;
      }
      .meta-stack {
        display: grid;
        grid-template-columns: repeat(3, minmax(180px, 1fr));
        gap: 12px;
        min-width: min(48vw, 760px);
      }
      .meta-chip, .stat-chip {
        background: var(--panel);
        border: 1px solid var(--border);
        border-radius: 16px;
        padding: 12px 14px;
      }
      .meta-chip span,
      .stat-chip span,
      .panel__header,
      .detail-label {
        display: block;
        color: var(--muted);
        text-transform: uppercase;
        letter-spacing: 0.15em;
        font-size: 11px;
      }
      .meta-chip strong,
      .stat-chip strong {
        display: block;
        margin-top: 6px;
        font-size: 16px;
        line-height: 1.35;
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
      }
      .stat-ribbon {
        display: grid;
        grid-template-columns: repeat(7, minmax(140px, 1fr));
        gap: 12px;
        margin-top: 22px;
      }
      .overview-panel {
        margin-top: 20px;
      }
      .summary-grid {
        display: grid;
        grid-template-columns: 1.25fr 1.4fr 1fr 1.25fr;
        gap: 18px;
        margin-top: 18px;
      }
      .panel {
        background: var(--panel);
        border: 1px solid var(--border);
        border-radius: 22px;
        padding: 18px;
        min-height: 240px;
      }
      .panel__header {
        margin-bottom: 14px;
      }
      .panel__header--input { color: var(--input); }
      .panel__header--signal { color: var(--signal); }
      .field-list,
      .alphabet-list {
        display: flex;
        flex-wrap: wrap;
        gap: 10px;
      }
      .field-pill,
      .alphabet-item {
        display: flex;
        flex-direction: column;
        gap: 4px;
        min-width: 120px;
        padding: 10px 12px;
        border-radius: 16px;
        border: 1px solid var(--border);
        background: var(--card);
      }
      .field-pill strong,
      .alphabet-item strong {
        font-size: 14px;
        line-height: 1.25;
      }
      .field-pill span,
      .alphabet-item span {
        color: var(--muted);
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
        font-size: 11px;
        line-height: 1.35;
      }
      .alphabet-item--input {
        border-color: rgba(44, 86, 104, 0.25);
      }
      .alphabet-item--signal {
        border-color: rgba(179, 121, 27, 0.28);
      }
      .invariant-list {
        display: grid;
        gap: 10px;
      }
      .invariant-card {
        border: 1px solid var(--border);
        border-radius: 16px;
        background: var(--card);
        padding: 12px 14px;
      }
      .invariant-card h3 {
        margin: 0 0 8px;
        font-size: 15px;
      }
      .invariant-card pre,
      .clause-block {
        margin: 0;
        white-space: pre-wrap;
        word-break: break-word;
        color: var(--muted);
        font-size: 12px;
        line-height: 1.5;
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
      }
      .board {
        display: grid;
        margin-top: 20px;
        gap: 18px;
        overflow-x: auto;
        padding-bottom: 8px;
        grid-template-columns: repeat(${phaseCount}, minmax(280px, 1fr));
      }
      .phase-column {
        background: linear-gradient(180deg, rgba(255,255,255,0.55), rgba(255,255,255,0.18));
        border: 1px solid var(--border);
        border-radius: 24px;
        padding: 16px;
        box-shadow: inset 0 1px 0 rgba(255,255,255,0.6);
      }
      .phase-column__header {
        display: flex;
        justify-content: space-between;
        align-items: baseline;
        gap: 12px;
        border-bottom: 1px solid var(--rule);
        padding-bottom: 10px;
        margin-bottom: 14px;
      }
      .phase-name {
        font-size: 30px;
        font-weight: 700;
        letter-spacing: -0.04em;
      }
      .phase-count {
        color: var(--muted);
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
        font-size: 12px;
      }
      .phase-column__cards {
        display: grid;
        gap: 12px;
      }
      .transition-card {
        padding: 14px;
        border-radius: 18px;
        border: 1px solid var(--border);
        background: var(--card);
        box-shadow: 0 8px 18px rgba(24, 23, 22, 0.05);
      }
      .transition-card--input {
        border-top: 4px solid var(--input);
      }
      .transition-card--signal {
        border-top: 4px solid var(--signal);
      }
      .transition-card__header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        gap: 12px;
      }
      .kind-pill,
      .effect-pill,
      .phase-shift {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        padding: 5px 9px;
        border-radius: 999px;
        font-size: 11px;
        font-weight: 700;
        letter-spacing: 0.12em;
        text-transform: uppercase;
      }
      .kind-pill--input {
        color: white;
        background: var(--input);
      }
      .kind-pill--signal {
        color: white;
        background: var(--signal);
      }
      .phase-shift {
        color: var(--ink);
        background: var(--phase);
      }
      .transition-card h3 {
        margin: 10px 0 4px;
        font-size: 22px;
        line-height: 1.1;
        letter-spacing: -0.03em;
      }
      .trigger-line {
        color: var(--muted);
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
        font-size: 12px;
        line-height: 1.45;
      }
      .detail-grid {
        display: grid;
        grid-template-columns: 1.4fr 1fr;
        gap: 12px;
        margin-top: 12px;
      }
      .guard-list {
        margin: 8px 0 0;
        padding-left: 18px;
        color: var(--ink);
        font-size: 13px;
        line-height: 1.5;
      }
      .guard-empty {
        margin-top: 8px;
        color: var(--muted);
        font-size: 12px;
      }
      .write-list {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
        margin-top: 8px;
      }
      .write-list span,
      .effect-pill {
        padding: 4px 8px;
        border-radius: 999px;
        background: rgba(53, 95, 115, 0.1);
        color: var(--ink);
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
        font-size: 11px;
      }
      .effect-row {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 6px;
        margin-top: 12px;
      }
      .effect-pill {
        background: rgba(79, 114, 89, 0.12);
      }
      .clause-block {
        margin-top: 8px;
        padding: 10px 12px;
        border-radius: 14px;
        border: 1px solid rgba(24, 23, 22, 0.08);
        background: rgba(255,255,255,0.75);
      }
      .clause-block--empty {
        color: var(--muted);
      }
      .footer-note {
        display: flex;
        justify-content: space-between;
        gap: 16px;
        margin-top: 18px;
        padding-top: 16px;
        border-top: 1px solid var(--rule);
        color: var(--muted);
        font-size: 12px;
        line-height: 1.5;
      }
      @media (max-width: 1600px) {
        .summary-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
        .meta-stack { grid-template-columns: 1fr; min-width: 260px; }
      }
      @media (max-width: 1080px) {
        .masthead__row { flex-direction: column; }
        .summary-grid { grid-template-columns: 1fr; }
        .stat-ribbon { grid-template-columns: repeat(2, minmax(0, 1fr)); }
      }`;
}

function parseSignatureBullet(line) {
  const match = line.match(/^- `([^`]+)`(?:\((.*)\))?/);
  if (!match) {
    return null;
  }
  return {
    name: match[1],
    signature: `${match[1]}(${match[2] ?? ""})`,
  };
}

function extractBackticked(line) {
  return [...line.matchAll(/`([^`]+)`/g)].map((match) => match[1]);
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}
