/* A household: a family of four and 18 long-lived agents on one small computer at home,
   the outside systems they are connected to, and the knowledge base they share.
   Each agent carries its own memory (the arc on its node); the knowledge base is one
   shared store that every agent queries. Wiring follows the deployment's mob definition.
   Volumes follow its measured 7-day totals: turn inputs by hour, peer messages by flow,
   tool calls by kind, memory calls and injections. */
(() => {
  "use strict";
  const { COL, rgba, SANS } = window.MeerkatNet;

  // Turn inputs by local hour, 7-day totals, as reported by the deployment.
  const HOURLY = [49, 28, 19, 38, 21, 16, 100, 98, 37, 64, 32, 47, 78, 63, 46, 38, 48, 74, 42, 20, 14, 31, 25, 35];

  // Seven-day totals as reported. Peer messages exclude the triage -> specialist hops,
  // which are drawn as the onward step of a triaged signal.
  const WEEK = {
    securityIn: 120, personIn: 150, calendarIn: 80,
    peer: 756, kbSearch: 680, memoryTool: 407, injections: 605, distilledQuar: 28,
    calendarReads: 483, snapshots: 413, agentIngest: 60, connectorIngest: 60,
  };
  // What reached triage from outside in the same 7 days, about 450 inputs from nine sources:
  // household mail (polled every 5 minutes), the picture-schedule app, four phone apps (two
  // school apps, the cleaning service, waste collection; read every 15 minutes), calendar
  // changes, shared-drive documents and home automation events. The drawing shows the breadth
  // of the inflow, not its volumes: every source is drawn alike and sends about as often.
  // Triage searched the knowledge base about 265 times (is this known already?) and made
  // about 90 onward sends: about one input in five is handed on. It never messages people.
  const TRIAGE = { inputs: 450, kbChecks: 265, perSource: 5 };
  // About 1,750 turns a week are drawn; 605 of them start with injected memory records.
  const INJECT_P = 605 / 1750;

  // Screen time. Between moments the day runs fast; at each moment the clock slows to
  // HOLD_MIN simulated minutes, so an exchange that takes seconds does not read as hours.
  // The script and the clock use the same warp.
  const HOLD_MIN = 5;
  const FAST = 8; // screen seconds for everything outside the moments
  // [key, hour, minute, screen seconds, screen seconds in the hero (0: not played there)].
  // The hero plays six of the moments, each long enough to read while watching it.
  const MOMENTS = [
    ["night", 1, 10, 3.4, 3.4], ["audit", 5, 30, 5.2, 0], ["displays", 6, 45, 5.2, 5.8],
    ["digest", 7, 0, 3.2, 4.8], ["ask", 8, 20, 4.6, 5.0], ["school", 12, 30, 5.0, 5.4],
    ["voice", 17, 10, 4.6, 0], ["evening", 17, 45, 3.6, 0], ["gate", 20, 30, 5.8, 5.8],
    ["late", 22, 30, 3.0, 0],
  ];
  const DAMP = 0.35; // background density while a moment plays
  const DAMP_HERO = 0.15; // the hero keeps the background quieter still
  const DENSITY = 0.6; // share of the measured daily volume drawn per 45 s of screen time

  // Specialists, ordered so the ones with outside systems have room for them.
  // Security and home automation sit together: the one specialist pair that is wired.
  const SPECIALISTS = ["calendar", "home", "finance", "school", "health", "ha", "security", "network", "research"];
  const SPEC_LABEL = { calendar: "calendar", home: "home", finance: "finance", school: "school", health: "health", ha: ["home automation", "automation"], security: "security", network: "network", research: "research" };
  // Camera judgements usually lead nowhere, so security rarely joins the peer loops.
  const SPEC_WEIGHT = { calendar: 3, school: 2, home: 2, ha: 1.5, security: 0.2, network: 1, finance: 0.7, health: 0.7, research: 1 };
  const PERSON_AGENTS = ["pa", "pb", "ca", "cb"];
  const HUMAN = { pa: "h_pa", pb: "h_pb", ca: "h_ca", cb: "h_cb" };
  // Who searches the knowledge base: triage (is this known already?), calendar and school
  // (dates, obligations, what a letter said), person agents, and the auditor.
  const KB_USERS = { triage: 3, calendar: 2, school: 2, pa: 1.5, pb: 1.2, ca: 0.4, cb: 0.4, truth: 1 };
  const MEM_USERS = { triage: 3, pa: 2, pb: 1.5, security: 1.5, calendar: 1.5, school: 1, ca: 0.5, cb: 0.5, home: 0.5, ha: 0.5, network: 0.3, finance: 0.3, health: 0.3, research: 0.4, gate: 0.3, truth: 0.6, family: 0.4, dev: 0.2 };
  const TAINTED = { triage: 3, calendar: 1.5, school: 1.5 }; // sessions that read mail or calendar content

  let MC = null;
  const measurer = () => (MC || (MC = document.createElement("canvas").getContext("2d")));
  const clamp01 = (v) => Math.max(0, Math.min(1, v));
  const ease = (v) => { const x = clamp01(v); return x < 0.5 ? 2 * x * x : 1 - Math.pow(-2 * x + 2, 2) / 2; };

  function clockWarp(K, list) {
    const mom = list.map(([h, m, s]) => [h * 60 + m, s * K]);
    const rate = FAST / (1440 - mom.length * HOLD_MIN);
    const knots = [[0, 0]];
    mom.forEach(([x, s]) => {
      const [px, pt] = knots[knots.length - 1];
      const t = pt + (x - px) * rate;
      knots.push([x, t], [x + HOLD_MIN, t + s]);
    });
    const [lx, lt] = knots[knots.length - 1];
    const day = lt + (1440 - lx) * rate;
    knots.push([1440, day]);
    const warp = (v, i, o) => {
      for (let k = 1; k < knots.length; k++) {
        const a = knots[k - 1], b = knots[k];
        if (v <= b[i]) return b[i] === a[i] ? a[o] : a[o] + ((b[o] - a[o]) * (v - a[i])) / (b[i] - a[i]);
      }
      return knots[knots.length - 1][o];
    };
    const holds = mom.map((_, k) => [knots[1 + 2 * k][1], knots[2 + 2 * k][1]]);
    return {
      day,
      toSec: (min) => warp(min, 0, 1),
      toMin: (sec) => warp(sec, 1, 0),
      held: (t) => holds.some(([a, b]) => t >= a && t < b),
    };
  }

  const pickW = (R, w) => {
    const keys = Object.keys(w);
    let s = keys.reduce((a, k) => a + w[k], 0) * R();
    for (const k of keys) { s -= w[k]; if (s <= 0) return k; }
    return keys[0];
  };

  /* ================================================================== */
  /* The household network over one day.                                */
  /* ================================================================== */
  function household(W, H, S, opts, { rng, text }) {
    const R = rng(11);
    const labels = !!opts.labels;
    // In the hero, the moments tell the story alone: background traffic is drawn in grey and
    // lights nothing, so orange belongs to the moment that is playing.
    const hero = !!opts.hero;
    const narrow = W / H < 1.12;
    const K = labels ? 1 : 0.62; // an unlabelled network plays the moments faster
    const played = MOMENTS.filter((m) => !hero || m[4] > 0);
    const has = (key) => played.some((m) => m[0] === key);
    const { day: DAY, toSec, toMin, held } = clockWarp(K, played.map(([, h, m, s, hs]) => [h, m, hero ? hs : s]));
    const at = (h, m = 0) => toSec(h * 60 + m);
    const mc = measurer();
    const fsz = (size) => Math.max(9, size * S);
    const tw = (str, size = 9, w = 500, track = 0) => { mc.font = `${w} ${fsz(size)}px ${SANS}`; return mc.measureText(str).width + track * str.length; };

    /* ---------- Layout ---------- */
    const HEADERS = ["PEOPLE", "PERSON AGENTS", "COORDINATORS", "SPECIALISTS", "CONNECTORS", "SHARED"];
    const gutter = labels && !narrow;
    const hx = Math.round(W * 0.022);
    const L = gutter ? hx + Math.max(...HEADERS.map((s) => tw(s, 8, 600, 1))) + 14 * S : W * (narrow ? 0.05 : 0.06);
    const Rr = W * (narrow ? 0.955 : 0.975);
    const span = Rr - L;
    const X = (f) => L + span * f;
    const Y0 = narrow
      ? { P: 0.07, gap: 0.14, A: 0.225, C: 0.365, S: 0.525, cA: 0.65, cB: 0.72, K: 0.88 }
      : { P: 0.075, gap: 0.16, A: 0.245, C: 0.395, S: 0.545, cA: 0.665, cB: 0.735, K: 0.885 };
    const y = (k) => H * Y0[k];
    const col6 = (i) => X((i + 0.5) / 6);
    const col9 = (i) => X((i + 0.5) / 9);
    const c9 = span / 9;

    const nodes = [];
    const N = {};
    const add = (n) => { nodes.push(n); N[n.id] = n; return n; };
    const lab = { labelTone: "mid", labelSize: 9 };

    // People reach the system only through their own person agent.
    [["h_pa", "parent"], ["h_pb", "parent"], ["h_ca", "child"], ["h_cb", "child"]].forEach(([id, label], i) => {
      add({ id, kind: "person", x: col6(i), y: y("P"), r: 4.2 * S, label, labelPos: "above", ...lab });
    });
    add({ id: "robot", kind: "source", x: col6(4) - (narrow ? 0.16 * span / 6 : 0), y: y("P"), r: 3.6 * S, label: "desk robot", labelPos: "above", ...lab });
    add({ id: "chat", kind: "source", x: col6(5), y: y("P"), r: 3.6 * S, label: "family chat", labelPos: "above", ...lab });
    // On small canvases (the hero on a phone) the two sources take their short names where
    // the long ones would touch a neighbour's label or the canvas edge.
    {
      const clear = (a, b) => Math.abs(N[b].x - N[a].x) >= (tw(N[a].label) + tw(N[b].label)) / 2 + 6;
      if (!clear("h_cb", "robot")) N.robot.label = "robot";
      if (!clear("robot", "chat") || N.chat.x + tw(N.chat.label) / 2 > W - 2) N.chat.label = "chat";
    }
    add({ id: "stt", kind: "source", x: X(2 / 6), y: y("gap"), r: 3.4 * S, label: "speech-to-text", labelPos: "above", ...lab });

    // Every agent carries its own memory: the arc on the left of its node.
    PERSON_AGENTS.forEach((id, i) => add({ id, kind: "agent", x: col6(i), y: y("A"), r: 5.2 * S, mem: true }));
    add({ id: "dev", kind: "agent", x: col6(4), y: y("A"), r: 5.2 * S, mem: true, label: narrow ? "dev" : "developer", labelPos: "below", ...lab });
    add({ id: "family", kind: "coord", x: col6(5), y: y("A"), r: 5 * S, mem: true, label: "family group", labelPos: "below", labelSize: 9 });

    // Coordinators, on one row. Everything from outside that is not a person or a camera
    // reaches triage first, through one host endpoint: a column of nine sources left of
    // triage, each drawn alike, centred on triage's line. The phone apps (two school apps,
    // the cleaning service, waste collection) are read by one connector; where the column is
    // short they share a row. The column sits just left of the second person agent's drop,
    // its labels in the clear lane between the first two drops (on narrow canvases they may
    // run across the first drop).
    const INFLOW_ROWS = [
      [ // nine rows, one per source
        { ids: ["school1"], names: ["school app", "school"] },
        { ids: ["school2"], names: ["school platform", "platform"] },
        { ids: ["clean"], names: ["cleaning service", "cleaning app", "cleaning"] },
        { ids: ["waste"], names: ["waste collection", "waste pickup", "waste"] },
        { ids: ["hae"], names: ["home hub events", "home events", "home"] },
        { ids: ["drive"], names: ["shared drive", "drive"] },
        { ids: ["mail"], names: ["household mail", "mail"] },
        { ids: ["pics"], names: ["picture schedule", "schedule"] },
        { ids: ["cal"], names: ["calendars", "calendar"] },
      ],
      [ // eight rows
        { ids: ["school1", "school2"], names: ["school apps", "school"] },
        { ids: ["clean"], names: ["cleaning service", "cleaning app", "cleaning"] },
        { ids: ["waste"], names: ["waste collection", "waste pickup", "waste"] },
        { ids: ["hae"], names: ["home hub events", "home events", "home"] },
        { ids: ["drive"], names: ["shared drive", "drive"] },
        { ids: ["mail"], names: ["household mail", "mail"] },
        { ids: ["pics"], names: ["picture schedule", "schedule"] },
        { ids: ["cal"], names: ["calendars", "calendar"] },
      ],
      [ // seven rows
        { ids: ["school1", "school2"], names: ["school apps", "school"] },
        { ids: ["clean", "waste"], names: ["cleaning, waste apps", "cleaning, waste", "services"] },
        { ids: ["drive"], names: ["shared drive", "drive"] },
        { ids: ["hae"], names: ["home hub events", "home events", "home"] },
        { ids: ["mail"], names: ["mail"] },
        { ids: ["pics"], names: ["picture schedule", "schedule"] },
        { ids: ["cal"], names: ["calendars", "calendar"] },
      ],
      [ // six rows: 3 above, 2 below
        { ids: ["school1", "school2", "clean", "waste"], names: ["school, cleaning, waste apps", "phone apps", "apps"] },
        { ids: ["drive"], names: ["shared drive", "drive"] },
        { ids: ["hae"], names: ["home hub events", "home events", "home"] },
        { ids: ["mail"], names: ["mail"] },
        { ids: ["pics"], names: ["picture schedule", "schedule"] },
        { ids: ["cal"], names: ["calendars", "calendar"] },
      ],
    ];
    const INFLOW_LABEL = {}; // source -> the node that carries its row's label
    {
      const cy = y("C");
      const top = y("A") + 5.2 * S + Math.max(12 * S, 9); // clear of the person agents and their memory arcs
      const bottom = (y("C") + y("S")) / 2 - 6 * S; // clear of the bus
      const pitchFor = (rows) => Math.min(Math.max(13, 14 * S), (bottom - top) / (rows.length - 1));
      const minPitch = 1.22 * Math.max(9, 9 * S); // comfortable leading for the labels
      const rows = INFLOW_ROWS.find((r) => pitchFor(r) >= minPitch) || INFLOW_ROWS[INFLOW_ROWS.length - 1];
      const pitch = pitchFor(rows);
      // Centred on triage's line where the band allows; no source is set apart.
      const k0 = (rows.length - 1) / 2;
      const y0 = Math.max(top, Math.min(bottom - (rows.length - 1) * pitch, cy - k0 * pitch));
      const sr = Math.max(2.5, 3.2 * S);
      const q = 2 * sr + 2.5 * S; // squares side by side in one row
      const bx = col6(1) - Math.max(7, 9 * S); // the column's right edge
      const left = gutter ? col6(0) + 5 * S : 3;
      // Where the lane between the first two drops admits only a row's last-resort name (the
      // hero's smaller canvas), the label may run on across the first drop to the gutter,
      // as it does on narrow canvases; a halo keeps it legible over the line.
      const wideLeft = gutter ? L + 2 : 3;
      rows.forEach((row, i) => {
        const yy = y0 + i * pitch;
        const n = row.ids.length;
        // Four phone apps in one row sit as a small two-by-two grid. A row of several squares
        // hangs its label on an anchor, so the label lights with whichever square sends.
        const grid = n === 4;
        const g = 2 * sr + Math.max(1.6, 2 * S);
        const cols = grid ? 2 : n;
        const lx = bx - (cols - 1) * (grid ? g : q); // the row's leftmost column carries its label
        const room = lx - sr - 5 * S - left, roomWide = lx - sr - 5 * S - wideLeft;
        const last = row.names[row.names.length - 1];
        let name = row.names.find((s) => tw(s) <= room);
        if (!name || (name === last && row.names.length > 1)) name = row.names.find((s) => tw(s) <= roomWide) || last;
        const label = { label: name, labelPos: "left", ...lab };
        const carrier = n > 1 ? `lab_${row.ids[0]}` : row.ids[0];
        if (n > 1) add({ id: carrier, kind: "anchor", x: lx, y: yy, r: sr, ...label });
        row.ids.forEach((id, j) => {
          INFLOW_LABEL[id] = carrier;
          const x = grid ? bx - (1 - (j % 2)) * g : bx - (n - 1 - j) * q;
          const yj = grid ? yy + (j < 2 ? -g / 2 : g / 2) : yy;
          add({ id, kind: "source", x, y: yj, r: sr, ...(n === 1 ? label : {}) });
        });
      });
    }
    add({ id: "triage", kind: "coord", x: X(0.333), y: y("C"), r: 6 * S, mem: true, label: "triage", labelPos: "right", labelSize: 9.5, always: !narrow });
    // On narrow canvases the gate's label goes below, clear of the auditor's and of the drops beside it.
    add({ id: "gate", kind: "coord", x: X(0.667), y: y("C"), r: 6 * S, mem: true, label: "gate", labelPos: narrow ? "below" : "right", labelSize: 9.5, always: !narrow });
    // The auditor's label goes left: the family group's drop to the bus passes on its right.
    add({ id: "truth", kind: "coord", x: X(0.875), y: y("C"), r: 6 * S, mem: true, label: "auditor", labelPos: "left", labelSize: 9.5, always: !narrow });
    // The auditor's delegate helper, spawned for one bounded subtask of the morning sweep.
    add({ id: "helper", kind: "helper", x: N.truth.x - 24 * S, y: y("C") - 22 * S, r: 3.4 * S, hidden: true });

    // Specialists: one row. Labels go below if they fit, else alternate above and below.
    const specText = (id, long) => { const l = SPEC_LABEL[id]; return Array.isArray(l) ? l[long ? 0 : 1] : l; };
    const fitsRow = (strs, room) => strs.every((s, i) => i === 0 || (tw(strs[i - 1]) + tw(s)) / 2 + 7 <= room);
    const longL = SPECIALISTS.map((id) => specText(id, true)), shortL = SPECIALISTS.map((id) => specText(id, false));
    const fitsAlt = (arr) => fitsRow(arr.filter((_, i) => i % 2 === 0), 2 * c9) && fitsRow(arr.filter((_, i) => i % 2 === 1), 2 * c9);
    let specLabels = longL, alternate = false;
    if (!fitsRow(longL, c9)) {
      if (fitsRow(shortL, c9)) specLabels = shortL;
      else { alternate = true; specLabels = fitsAlt(longL) ? longL : shortL; }
    }
    SPECIALISTS.forEach((id, i) => {
      add({ id, kind: "agent", x: col9(i), y: y("S"), r: 4.4 * S, mem: true, label: specLabels[i], labelPos: alternate && i % 2 === 0 ? "above" : "below", ...lab });
    });

    // Connectors: the outside systems each specialist owns, in two staggered levels. (The
    // picture schedule, calendars and school apps are in triage's column above.)
    const conn = (id, col, lvl, names) => {
      const x = X((col + 0.5) / 9);
      add({ id, kind: "source", x, y: y(lvl), r: 3.6 * S, label: names[0], labelPos: "below", ...lab });
      return { id, x, lvl, names };
    };
    const CONN = [
      conn("hub", 5, "cA", ["home hub", "hub"]),
      conn("cams", 6, "cB", ["camera analysis", "cameras"]),
      conn("router", 7, "cA", ["router, parental controls", "router"]),
      conn("web", 8, "cB", ["web search, browser", "web, browser", "web"]),
    ];
    // Two e-ink displays, drawn as a pair with one label.
    const dx = X(0.5 / 9) + 3 * S;
    add({ id: "disp1", kind: "source", x: dx - 5.5 * S, y: y("cA"), r: 3.6 * S });
    add({ id: "disp2", kind: "source", x: dx + 5.5 * S, y: y("cA"), r: 3.6 * S, label: "displays", labelPos: "below", ...lab });
    CONN.push({ id: "disp2", x: dx, lvl: "cA", names: ["displays"] });
    // Start from the shortest names, then lengthen each (in this order) while it
    // still clears its neighbours and the canvas edge.
    const PRIORITY = ["hub", "web", "router", "cams", "disp2"];
    CONN.forEach((c) => { N[c.id].label = c.names[c.names.length - 1]; });
    PRIORITY.forEach((id) => {
      const c = CONN.find((k) => k.id === id);
      const row = CONN.filter((k) => k.lvl === c.lvl && k !== c);
      const fits = (name) => {
        const w = tw(name);
        if (c.x + w / 2 > W - 2 || c.x - w / 2 < 2) return false;
        return row.every((k) => Math.abs(k.x - c.x) >= (w + tw(N[k.id].label)) / 2 + 10);
      };
      N[c.id].label = c.names.find(fits) || c.names[c.names.length - 1];
    });

    // The household knowledge base: one store, shared by every agent, centre bottom.
    // Its label is drawn in the overlay, clear of the cylinder's width.
    add({ id: "kb", kind: "store", x: X(0.5), y: y("K"), r: 9.5 * S });

    const INFLOW = ["school1", "school2", "clean", "waste", "hae", "drive", "mail", "pics", "cal"];
    const AGENTS = [...PERSON_AGENTS, "dev", "family", "triage", "gate", "truth", ...SPECIALISTS];
    const IS_AGENT = new Set(AGENTS);

    /* ---------- Wiring ---------- */
    const PA = [...PERSON_AGENTS, "dev"];
    const SP = SPECIALISTS;
    const pairs = (as, bs) => as.flatMap((a) => bs.filter((b) => b !== a).map((b) => [a, b]));
    // Peer wiring as one bus between the coordinators and the specialists, Braun-style:
    // every agent drops a line to it (person agents, coordinators and specialists are wired
    // to one another). Messages still travel straight from sender to receiver.
    const BUS = [...PA, "family", "triage", "gate", "truth", ...SP];
    const busY = (y("C") + y("S")) / 2;
    const edges = [
      // Specialists do not talk to each other, with one exception: cameras and house state.
      { pairs: [["security", "ha"]], alpha: 0.35 },
      { pairs: [["h_pa", "pa"], ["h_pb", "pb"], ["h_ca", "ca"], ["h_cb", "cb"], ["chat", "family"], ["robot", "cb"]], alpha: 0.28, dash: [2, 3] },
      { pairs: pairs(["stt"], PERSON_AGENTS), alpha: 0.13, dash: [2, 3] },
      // Triage's inflow: nine sources, drawn alike.
      { pairs: INFLOW.map((id) => [id, "triage"]), alpha: 0.16 },
      // Calendar reads the family's calendars itself; camera events bypass triage.
      { pairs: [["cal", "calendar"], ["disp1", "calendar"], ["disp2", "calendar"], ["hub", "ha"], ["cams", "security"], ["router", "network"], ["web", "research"]], alpha: 0.2, dash: [2, 3] },
      // The knowledge base's heaviest users.
      { pairs: [...Object.keys(KB_USERS), "security"].map((a) => [a, "kb"]), alpha: 0.075 },
      { pairs: [["truth", "helper"]], alpha: 0.3 },
    ];

    /* ---------- Background: the measured volumes ---------- */
    const script = [];
    const flight = (a, b) => Math.max(0.45, Math.min(1.4, Math.hypot(N[b].x - N[a].x, N[b].y - N[a].y) / (260 * S)));
    const mem = (t, id, op) => script.push({ t, mem: id, op });
    const spec = () => pickW(R, SPEC_WEIGHT);
    const pa = () => (R() < 0.8 ? (R() < 0.5 ? "pa" : "pb") : (R() < 0.5 ? "ca" : "cb"));
    // A message; when it starts a turn, the agent may receive injected memory records.
    const bg = (t, a, b, c, land = true) => {
      script.push({ t, msg: [a, b], c: hero ? "mid" : c, land: land && !hero });
      if (c === "ink" && IS_AGENT.has(b) && R() < INJECT_P) mem(t + flight(a, b), b, "read");
    };
    // A knowledge-base search: the query goes down to the store and the result comes back.
    const search = (t, a, land = false) => {
      script.push({ t, msg: [a, "kb"], c: "mid", land });
      const back = t + flight(a, "kb") + 0.12;
      script.push({ t: back, msg: ["kb", a], c: "mid", land: false });
      return back + flight("kb", a);
    };

    // Sample times from the hourly rhythm over screen time; calmer while a moment plays.
    const STEP = 0.02;
    const cdf = [];
    let acc = 0;
    for (let t = 0; t < DAY; t += STEP) {
      const tm = t + STEP / 2;
      acc += HOURLY[Math.floor(toMin(tm) / 60) % 24] * (held(tm) ? (hero ? DAMP_HERO : DAMP) : 1);
      cdf.push(acc);
    }
    const nightAt = (t) => { const h = Math.floor(toMin(t) / 60) % 24; return h < 6 || h >= 22; };
    const when = (dayOnly, rr = R) => {
      for (let k = 0; k < 10; k++) {
        const u = rr() * acc;
        let lo = 0, hi = cdf.length - 1;
        while (lo < hi) { const mid = (lo + hi) >> 1; if (cdf[mid] < u) lo = mid + 1; else hi = mid; }
        const t = (lo + rr()) * STEP;
        if (!dayOnly || !nightAt(t)) return t;
      }
      return null;
    };
    const stream = (perWeek, dayOnly, fn) => {
      const n = (perWeek / 7) * DENSITY * (DAY / 45);
      const count = Math.floor(n) + (R() < n % 1 ? 1 : 0);
      for (let k = 0; k < count; k++) { const t = when(dayOnly); if (t != null) fn(t); }
    };

    // Triage's inflow. Each input may first be checked against the knowledge base (is it known
    // already?), then it is ignored, filed, or filed and handed on to the agent that owns it.
    // Triage decides where a signal goes; it never messages a person.
    const KB_CHECK_P = TRIAGE.kbChecks / TRIAGE.inputs;
    const HAND_TO = { school: 3, calendar: 3, home: 2, finance: 1, health: 1, pa: 0.4, pb: 0.4 };
    // out: "ignore", "file", or the agent it is handed on to.
    const intake = (t, src, out) => {
      bg(t, src, "triage", "ink");
      if (!hero) script.push({ t, on: [...new Set([src, INFLOW_LABEL[src]])], d: 0.8 }); // a source lights as it sends
      let u = t + flight(src, "triage") + 0.08;
      if (R() < KB_CHECK_P) u = search(u, "triage") + 0.06;
      if (out === "ignore") return;
      bg(u, "triage", "kb", "mid", false); // filed, with triage's decision and a note
      if (out !== "file") bg(u + 0.3, "triage", out, "ink");
    };
    // Every source sends about as often: the same number of inputs per day, each dealt the same
    // outcomes (one handed on, the rest filed or ignored), so one input in five is handed on.
    // Where each is handed on follows the deployment: school apps to school, the cleaning and
    // waste apps to home, the picture schedule and calendar changes to calendar, drive documents
    // to school, home automation events to home automation, mail to whichever agent owns it.
    // Mail, the drive and home automation also arrive at night; the phone apps, the picture
    // schedule and calendar changes in the day.
    const SOURCES = {
      school1: { to: () => "school", outs: ["file", "file", "file", "ignore"], day: true },
      school2: { to: () => "school", outs: ["file", "file", "file", "ignore"], day: true },
      clean: { to: () => "home", outs: ["file", "file", "file", "ignore"], day: true },
      waste: { to: () => "home", outs: ["file", "file", "file", "ignore"], day: true },
      hae: { to: () => "ha", outs: ["file", "ignore", "ignore", "ignore"], day: false },
      drive: { to: () => "school", outs: ["file", "file", "file", "file"], day: false },
      mail: { to: () => pickW(R, HAND_TO), outs: ["file", "file", "ignore", "ignore"], day: false },
      pics: { to: () => "calendar", outs: ["file", "file", "file", "ignore"], day: true },
      cal: { to: () => "calendar", outs: ["file", "file", "file", "ignore"], day: true },
    };
    {
      const R2 = rng(29);
      // Clear of the moment in which triage is the subject, and spaced apart.
      const busy = [at(12, 30) - 0.8, at(12, 30) + 5.6 * K];
      const gap = Math.min(0.9, DAY / (INFLOW.length * TRIAGE.perSource * 2.2));
      const placed = [];
      const place = (day) => {
        for (let k = 0; k < 80; k++) {
          const t = when(day, R2);
          if (t == null || t < 0.6 || t > DAY - 1.5) continue;
          if (t > busy[0] && t < busy[1]) continue;
          if (placed.some((p) => Math.abs(p - t) < gap)) continue;
          placed.push(t);
          return t;
        }
        return null;
      };
      // One hand-off per source, at a different turn for each.
      const decks = INFLOW.map((src, i) => { const d = [...SOURCES[src].outs]; d.splice((i * 2) % (d.length + 1), 0, "hand"); return d; });
      for (let round = 0; round < TRIAGE.perSource; round++) {
        INFLOW.forEach((src, i) => {
          const d = SOURCES[src];
          const out = decks[i][round];
          const t = place(d.day);
          if (t != null) intake(t, src, out === "hand" ? d.to() : out);
        });
      }
    }
    stream(WEEK.securityIn, false, (t) => {
      bg(t, "cams", "security", "ink");
      if (R() < 0.6) bg(t + 0.9, "security", "kb", "mid", false); // one observation note
    });
    stream(WEEK.personIn, true, (t) => { const p = pa(); bg(t, HUMAN[p], p, "ink"); bg(t + 1.1, p, HUMAN[p], "ink"); });
    stream(WEEK.calendarIn, true, (t) => { if (!hero) script.push({ t, on: "calendar", d: 0.6 }); if (R() < INJECT_P) mem(t, "calendar", "read"); });

    // Peer messages between agents, in the measured flow mix.
    const FLOWS = [
      [300, () => { const s = spec(), p = pa(); return R() < 0.62 ? [s, p] : [p, s]; }],
      [73, () => [spec(), "triage"]],
      [169, () => { const s = spec(); const r = R(); return r < 0.46 ? [s, "gate"] : r < 0.8 ? ["gate", s] : ["gate", pa()]; }],
      [68, () => { const s = spec(); const r = R(); return r < 0.5 ? ["truth", s] : r < 0.88 ? [s, "truth"] : ["truth", "triage"]; }],
      [29, () => (R() < 0.55 ? ["family", pa()] : [pa(), "family"])],
      [8, () => ["security", "triage"]],
      [11, () => (R() < 0.5 ? ["pa", "pb"] : ["pb", "pa"])],
    ];
    const flowTotal = FLOWS.reduce((a, f) => a + f[0], 0);
    const flow = () => { let s = flowTotal * R(); for (const [w, f] of FLOWS) { s -= w; if (s <= 0) return f(); } return FLOWS[0][1](); };
    stream(WEEK.peer, true, (t) => bg(t, ...flow(), "ink"));

    // Tool calls, drawn in the mid tone. Knowledge-base search is the most used tool in the house.
    // Triage's own searches are drawn with its inflow above; these are everyone else's.
    const KB_OTHERS = Object.fromEntries(Object.entries(KB_USERS).filter(([k]) => k !== "triage"));
    stream(WEEK.kbSearch - TRIAGE.kbChecks, true, (t) => search(t, pickW(R, KB_OTHERS)));
    stream(WEEK.agentIngest, true, (t) => { const a = pickW(R, { school: 2, triage: 1, calendar: 1, pa: 0.5 }); bg(t, a, "kb", "mid", false); });
    // The connectors index what they read as it arrives: mail, the phone apps, the shared drive.
    stream(WEEK.connectorIngest, false, (t) => bg(t, pickW(R, { mail: 1, school1: 1, school2: 1, clean: 1, waste: 1, drive: 1 }), "kb", "mid", false));
    stream(WEEK.calendarReads, true, (t) => bg(t, "cal", "calendar", "mid", false));
    stream(WEEK.snapshots, false, (t) => bg(t, "cams", "security", "mid", false));
    // Each agent's own memory: the memory tool (record, recall, update, forget), and copies
    // distilled in the background from sessions that read mail or calendar content, which land
    // quarantined.
    stream(WEEK.memoryTool, false, (t) => {
      const a = nightAt(t) ? (R() < 0.5 ? "triage" : "security") : pickW(R, MEM_USERS);
      mem(t, a, R() < 0.7 ? "write" : "read");
    });
    // (In the hero, a quarantined copy is shown only in the moments that speak of one.)
    stream(WEEK.distilledQuar, false, (t) => { if (!hero) mem(t, pickW(R, TAINTED), "quar"); });

    /* ---------- Moments the deployment described, at their times of day ---------- */
    const hopDur = (a, b) => Math.max(0.35, flight(a, b) * K);
    const hop = (t, a, b, c = "accent", land = true) => { script.push({ t, msg: [a, b], c, land, dur: hopDur(a, b) }); return t + hopDur(a, b); };
    const ask = (t, a, land = true) => { const back = hop(t, a, "kb", "mid", land) + 0.1 * K; return hop(back, "kb", a, "mid", false); };
    const on = (t, id, d) => script.push({ t, on: id, d: d * K });
    // Short notes drawn beside a node while something happens to it.
    const flashes = [];
    const flash = (t0, t1, id, str, c) => flashes.push({ t0, t1, id, str, c });
    const moment = (h, m, key, title, body) => {
      const t0 = at(h, m);
      script.push({ t: t0, cap: { key, title, body, time: `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}` } });
      return (dt) => t0 + dt * K;
    };

    script.push({ t: 0, cap: { key: "night", time: "00:00", title: "Night", body: "Camera events keep arriving. Repeated deliveries of one event become one judgement by the security agent, and routine activity waits for the morning digest." } });
    const burst = (T, n) => {
      for (let k = 0; k < n; k++) hop(T(k * 0.17), "cams", "security", "ink", false);
      on(T(0.95), "security", 1.1);
    };
    let T = (dt) => at(1, 10) + dt * K;
    burst(T, 4);
    hop(T(2.1), "security", "kb", "mid", false);
    burst((dt) => at(3, 20) + dt * 0.5, 3);
    burst((dt) => at(4, 5) + dt * 0.5, 3);

    if (has("audit")) {
      T = moment(5, 30, "audit", "Morning audit", "The read-only auditor checks what the family will read today, with a short-lived helper for one subtask. It asks calendar and school, and hands each finding to the agent that owns it.");
      on(T(0), "truth", 4.2);
      mem(T(0.05), "truth", "read");
      hop(T(0.1), "disp1", "truth", "mid", false);
      hop(T(0.2), "disp2", "truth", "mid", false);
      script.push({ t: T(0.3), show: "helper" });
      hop(T(0.35), "truth", "helper", "ink");
      on(T(0.8), "helper", 1.5);
      hop(T(0.75), "pics", "truth", "mid", false);
      ask(T(0.9), "truth");
      hop(T(1.4), "truth", "calendar");
      hop(T(1.5), "truth", "school");
      hop(T(2.3), "calendar", "truth", "ink");
      hop(T(2.4), "school", "truth", "ink");
      hop(T(2.5), "helper", "truth", "ink");
      script.push({ t: T(3.1), hide: "helper" });
      mem(T(3.0), "truth", "write");
      const fin = hop(T(3.4), "truth", "calendar");
      script.push({ t: fin, on: "calendar", d: 0.7 * K });
    }

    if (has("displays")) {
      T = moment(6, 45, "displays", "Hallway displays", "Calendar reads three calendars, the picture schedule and the knowledge base, and publishes one frame to both hallway displays. It keeps its own note in memory. A copy distilled in the background is quarantined: the session read calendar content.");
      on(T(0), "calendar", 3.9);
      mem(T(0.05), "calendar", "read");
      [0.1, 0.4, 0.7].forEach((d) => hop(T(d), "cal", "calendar", "mid", false));
      hop(T(1.0), "pics", "calendar", "mid", false);
      ask(T(1.2), "calendar");
      hop(T(2.1), "calendar", "disp1");
      hop(T(2.22), "calendar", "disp2");
      hop(T(2.95), "disp1", "calendar", "mid", false);
      hop(T(3.2), "calendar", "pa");
      mem(T(3.7), "calendar", "write");
      flash(T(3.7), T(4.5), "calendar", "memory written", COL.ink);
      mem(T(4.55), "calendar", "quar");
      flash(T(4.55), T(5.6), "calendar", "copy quarantined", COL.accent);
    }

    if (has("digest")) {
      T = moment(7, 0, "digest", "Morning digest", "The night's camera activity reaches the parents as one summary, through each parent's own agent.");
      on(T(0), "security", 1);
      hop(T(0.3), "security", "pa");
      hop(T(0.4), "security", "pb");
      hop(T(1.5), "pa", "h_pa");
      hop(T(1.6), "pb", "h_pb");
    }

    if (has("ask")) {
      T = moment(8, 20, "ask", "A parent asks", "A parent asks their own agent. It searches the household knowledge base, asks calendar, school and home in parallel, and decides what the parent needs to hear.");
      hop(T(0), "h_pa", "pa");
      on(T(0.5), "pa", 3.4);
      mem(T(0.5), "pa", "read");
      ask(T(0.55), "pa");
      ["calendar", "school", "home"].forEach((s, i) => {
        const land = hop(T(1.0 + i * 0.08), "pa", s);
        script.push({ t: land, on: s, d: 0.6 * K });
        hop(T(2.25 + i * 0.1), s, "pa", "ink");
      });
      hop(T(3.55), "pa", "h_pa");
    }

    if (has("school")) {
      T = moment(12, 30, "school", "School message", "An email from school reaches triage, which checks the knowledge base, files the email there and passes it to the school agent. School turns it into a dated to-do and hands it to a parent's agent.");
      script.push({ t: T(0), on: [...new Set(["mail", INFLOW_LABEL.mail])], d: 0.9 * K }); // the email arrives
      hop(T(0), "mail", "triage");
      on(T(0.5), "triage", 1.3);
      ask(T(0.45), "triage");
      hop(T(1.2), "triage", "kb", "mid");
      hop(T(1.3), "triage", "school");
      on(T(1.85), "school", 1.5);
      hop(T(2.05), "school", "kb", "mid");
      hop(T(2.75), "school", "pb");
      hop(T(3.55), "pb", "h_pb");
      mem(T(4.0), "triage", "quar");
      mem(T(4.15), "school", "quar");
    }

    if (has("voice")) {
      T = moment(17, 10, "voice", "Voice note", "A parent sends a voice note about next week's pickups. It is transcribed, and the parent's agent keeps it in its own memory and tells calendar. Nothing on the calendar changes.");
      hop(T(0), "h_pb", "stt");
      hop(T(0.7), "stt", "pb");
      on(T(1.15), "pb", 2.3);
      mem(T(1.4), "pb", "write");
      flash(T(1.4), T(2.3), "pb", "memory written", COL.ink);
      const told = hop(T(1.9), "pb", "calendar");
      script.push({ t: told, on: "calendar", d: 0.8 * K });
      hop(T(2.7), "pb", "h_pb", "ink");
    }

    if (has("evening")) {
      T = moment(17, 45, "evening", "Evening", "The evening frame goes up on the displays. The parents' agents coordinate on tomorrow, and the family group posts to the family chat.");
      on(T(0), "calendar", 1.4);
      ask(T(0.05), "calendar", false);
      hop(T(0.9), "calendar", "disp1");
      hop(T(1.02), "calendar", "disp2");
      hop(T(1.2), "pa", "pb");
      hop(T(1.9), "pb", "pa", "ink");
      hop(T(2.25), "family", "chat");
    }

    if (has("gate")) {
      // Held action: the gate reviews and asks through the parent's agent; the tool call
      // itself is held where it runs until the parent decides, then released there.
      T = moment(20, 30, "gate", "A held action", "A specialist wants to take a high-risk action. The gate reviews it and asks a parent through the parent's agent. The call is held until the parent approves or denies it on their phone, then released where it runs.");
      const heldFrom = T(0.2);
      hop(T(0), "ha", "gate");
      on(T(0.5), "gate", 1.2);
      hop(T(1.0), "gate", "pa");
      hop(T(1.75), "pa", "h_pa");
      on(T(2.6), "h_pa", 0.6);
      hop(T(3.1), "h_pa", "pa", "green");
      const heldTo = T(3.95);
      on(heldTo, "ha", 0.9);
      hop(T(4.15), "ha", "hub", "ink");
      flash(heldFrom, heldTo, "ha", "held", COL.accent);
      flash(heldTo, heldTo + 1.2 * K, "ha", "released", COL.green);
    }

    if (has("late")) {
      moment(22, 30, "late", "Late evening", "Mail and home events still reach triage through the night, and camera events go straight to security. Every agent keeps its full history for tomorrow.");
    }

    /* ---------- The peer bus ---------- */
    const decor = (ctx, net) => {
      const s = net.S;
      const ns = BUS.map((id) => net.node(id)).filter((n) => n && n.a > 0.05);
      if (!ns.length) return;
      const a = ns.reduce((v, n) => v + n.a, 0) / ns.length;
      const by = Math.round(busY) + 0.5;
      // Each drop starts clear of the node, its ring and a label on that side.
      const clear = (n, down) => {
        const sp = n.spec;
        const lab = sp.label && (labels || sp.always) ? Math.max(9, (sp.labelSize || 10) * s) + 2 * s : 0;
        const side = (sp.labelPos || "below") === (down ? "below" : "above") ? lab + 5 * s : 0;
        return n.r + (sp.kind === "coord" ? 4 : 2) * s + side;
      };
      ctx.save();
      ctx.lineWidth = 1;
      ctx.strokeStyle = rgba(COL.ink, 0.15 * a);
      ctx.beginPath();
      let x0 = Infinity, x1 = -Infinity;
      ns.forEach((n) => {
        const down = n.y < by;
        const y0 = down ? n.y + clear(n, true) : n.y - clear(n, false);
        if (down ? y0 >= by : y0 <= by) return;
        ctx.moveTo(Math.round(n.x) + 0.5, y0); ctx.lineTo(Math.round(n.x) + 0.5, by);
        x0 = Math.min(x0, n.x); x1 = Math.max(x1, n.x);
      });
      ctx.stroke();
      if (x1 > x0) {
        ctx.strokeStyle = rgba(COL.ink, 0.24 * a);
        ctx.lineWidth = 1.2;
        ctx.beginPath(); ctx.moveTo(x0 - 6 * s, by); ctx.lineTo(x1 + 6 * s, by); ctx.stroke();
        ctx.fillStyle = rgba(COL.ink, 0.4 * a);
        ns.forEach((n) => { ctx.beginPath(); ctx.arc(n.x, by, Math.max(1.2, 1.5 * s), 0, Math.PI * 2); ctx.fill(); });
      }
      ctx.restore();
    };

    /* ---------- Text: tier headers and notes ---------- */
    const head = (ctx, s, str, x, yy, o = {}) => text(ctx, s, str, x, yy, { size: 8, w: 600, c: COL.ink, a: 0.72, track: 1, halo: true, base: "middle", ...o });
    const note = (ctx, s, str, x, yy, o = {}) => text(ctx, s, str, x, yy, { size: 8.5, w: 500, c: COL.mid, halo: true, base: "middle", ...o });

    const overlay = (ctx, net) => {
      if (!labels) return;
      const s = net.S;
      if (gutter) {
        head(ctx, s, "PEOPLE", hx, y("P"));
        head(ctx, s, "PERSON AGENTS", hx, y("A"));
        head(ctx, s, "COORDINATORS", hx, y("C"));
        head(ctx, s, "SPECIALISTS", hx, y("S"));
        head(ctx, s, "CONNECTORS", hx, (y("cA") + y("cB")) / 2);
        head(ctx, s, "SHARED", hx, y("K"));
        note(ctx, s, "chat or voice", N.h_pa.x - 7 * s, (y("P") + y("A")) / 2, { align: "right" });
      } else {
        head(ctx, s, "PERSON AGENTS", N.pa.x - 5 * s, y("A") - N.pa.r - 9 * s);
      }
      const kb = net.node("kb");
      if (kb && kb.a > 0.3) {
        const hot = kb.flash > net.t || kb.active > net.t;
        text(ctx, s, "knowledge base", kb.x + kb.r * 1.2 + 7 * s, kb.y, { size: 9.5, w: 500, c: hot ? COL.accent : COL.ink, a: kb.a * (hot ? 1 : 0.85), base: "middle", halo: true });
      }
      // Notes beside a node, on the side its label does not use.
      // Where the specialists' labels alternate above and below (narrow canvases, and the
      // hero's), their notes use the clear lane under the row, kept inside the canvas.
      flashes.forEach((f) => {
        if (net.t < f.t0 || net.t >= f.t1) return;
        const n = net.node(f.id);
        if (!n || n.a < 0.3) return;
        const fade = Math.min(1, (net.t - f.t0) / 0.15, (f.t1 - net.t) / 0.25);
        const up = (n.spec.labelPos || "below") !== "above";
        const off = n.r + (n.spec.mem ? 9 : 8) * s;
        let x = n.x, yy = up ? n.y - off : n.y + off;
        if ((narrow || alternate) && SPECIALISTS.includes(f.id)) {
          const half = tw(f.str, 8.5, 600) / 2;
          x = Math.max(4 + half, Math.min(W - 4 - half, n.x));
          yy = y("S") + H * 0.068;
        } else if (narrow) yy = n.y + off;
        text(ctx, s, f.str, x, yy, { size: 8.5, w: 600, c: f.c, a: fade, align: "center", base: "middle", halo: true });
      });
    };

    return {
      nodes, edges, script, duration: DAY, decor, overlay,
      origin: [W / 2, H / 2],
      clock: toMin,
      staticAt: at(6, 47.5),
      captionReset: null,
    };
  }

  /* ================================================================== */
  /* Inside the calendar agent: one display refresh, as it ran.          */
  /* Order, counts and relative timing follow a production turn: 23 tool */
  /* calls (18 MCP, 5 native), 3 calendar reads, 6 knowledge-base calls, */
  /* 1 live mail search, 3 peer messages, 1 memory write, in about seven */
  /* minutes. Tool names are shown by their generic role.                */
  /* Directions carry meaning: context comes in from above, memory sits  */
  /* on the agent's left, peers on its right, and every call goes down   */
  /* through one of its tools to an outside system or to the knowledge   */
  /* base, which other agents share (bottom centre, as in the network).  */
  /* ================================================================== */
  function householdAgent(W, H, S, opts, helpers = {}) {
    const text = helpers.text || window.MeerkatScenes.text;
    const rng = helpers.rng || window.MeerkatScenes.rng;
    const mc = measurer();
    const fz = (size) => Math.max(9, size * S);
    const tw = (str, size = 8.5, w = 500, track = 0) => { mc.font = `${w} ${fz(size)}px ${SANS}`; return mc.measureText(str).width + track * str.length; };
    const fit = (list, room, size = 8.5, w = 500) => list.find((s) => tw(s, size, w) <= room) || null;

    const nodes = [];
    const N = {};
    const add = (n) => { nodes.push(n); N[n.id] = n; return n; };

    /* ---------- Geometry ---------- */
    const Ra = Math.max(19, 30 * S); // the calendar agent, drawn large
    const ax = W * 0.5, ay = H * 0.35;
    const tapeY = H * 0.08;
    const tape = { x0: W * 0.05, d1: W * 0.55, l0: W * 0.62, l1: W * 0.84, feed: W * 0.7 };
    const hMax = 11 * S;

    add({ id: "agent", kind: "agent", x: ax, y: ay, r: Ra, mem: true, label: "calendar agent", labelPos: "below", labelSize: 11, labelWeight: 600, always: true });
    add({ id: "tape0", kind: "anchor", x: tape.x0, y: tapeY, r: 1 });
    add({ id: "tape1", kind: "anchor", x: tape.l1, y: tapeY, r: 1 });

    // On the agent's right: the operator console the request came in through (relaying a
    // parent), the parent's own agent that receives the results, and the parent it speaks for.
    add({ id: "pa", kind: "agent", x: W * 0.8, y: ay + 4 * S, r: 6 * S, mem: true, label: "a parent's agent", labelPos: "below", labelSize: 9.5, always: true });
    add({ id: "parent", kind: "person", x: W * 0.93, y: ay - 30 * S, r: 4.4 * S, label: "a parent", labelPos: "above", labelTone: "mid", labelSize: 9, always: true });
    add({ id: "console", kind: "source", x: W * 0.78, y: ay - 40 * S, r: 3.6 * S, label: W < 420 ? "console" : "operator console", labelPos: "above", labelTone: "mid", labelSize: 9, always: true });

    // The outside systems the turn reads and writes, in one row below the tools.
    const rowY = H * 0.775;
    const DEST = [];
    {
      const bw = 20 * S, bh = 13 * S, gap = 5 * S, cx = W * 0.14;
      add({ id: "d1", kind: "box", x: cx - (bw + gap) / 2, y: rowY, r: bw / 2, w: bw / S, h: bh / S });
      add({ id: "d2", kind: "box", x: cx + (bw + gap) / 2, y: rowY, r: bw / 2, w: bw / S, h: bh / S });
      DEST.push({ label: "displays", parts: ["d1", "d2"], half: bh / 2 });
    }
    {
      const p = 10 * S, cx = W * 0.38;
      // The third calendar is the other parent's, which this agent may only read.
      ["cal1", "cal2", "cal3"].forEach((id, i) => add({ id, kind: "source", x: cx + (i - 1) * p, y: rowY, r: 3.6 * S, alpha: i === 2 ? 0.55 : 1 }));
      DEST.push({ label: "calendars", parts: ["cal1", "cal2", "cal3"], half: 3.6 * S });
    }
    add({ id: "pics", kind: "source", x: W * 0.62, y: rowY, r: 3.6 * S });
    DEST.push({ label: fit(["picture schedule", "schedule"], W * 0.22) || "schedule", parts: ["pics"], half: 3.6 * S });
    add({ id: "mail", kind: "source", x: W * 0.86, y: rowY, r: 3.6 * S });
    DEST.push({ label: "mail", parts: ["mail"], half: 3.6 * S });
    // The household knowledge base: shared, so other agents sit on it too.
    const kbR = 9.5 * S, kbY = H * 0.905;
    add({ id: "kb", kind: "store", x: ax, y: kbY, r: kbR });
    DEST.push({ label: "knowledge base", parts: ["kb"], half: kbR * 1.5 });
    const OTHERS = [-0.31, -0.17, 0.17, 0.31].map((f, i) => add({ id: `o${i}`, kind: "agent", x: ax + W * f, y: kbY, r: 3.4 * S, tone: "mid", alpha: 0.5, mem: true }).id);

    /* ---------- The agent's own memory: a ranked column attached to its arc ---------- */
    const KINDS = ["fact", "fact", "preference", "procedure", "open loop", "gotcha", "fact"];
    const slotH = Math.max(12, 13.5 * S);
    const colX = ax - Ra - 30 * S;
    const colY0 = ay - 3 * slotH; // slot 0: the highest ranked record
    const recS = Math.max(3.6, 4.6 * S);
    const spineR = Ra + 3 * S; // the engine draws the memory arc at this radius
    const spineAt = (sl) => { const a = Math.PI * (1.32 - (Math.min(sl, 6.6) / 6) * 0.64); return [Math.cos(a) * spineR, Math.sin(a) * spineR]; };
    const labelRoom = colX - recS / 2 - 6 * S - 4;

    /* ---------- Tools: 147 resolved for this agent (85 MCP, 62 native) ---------- */
    const pitch = Math.max(3.2, 4.6 * S);
    const dotR = Math.max(0.85, 1.25 * S);
    const MCPC = 17, NATC = 13, ROWS = 5, blockGap = 7 * S;
    const rackW = (MCPC + NATC - 1) * pitch + blockGap;
    const rackX0 = ax - rackW / 2, rackY0 = H * 0.6 - 2 * pitch;
    const natX0 = rackX0 + MCPC * pitch + blockGap;
    const RACK = [];
    for (let c = 0; c < MCPC; c++) for (let r = 0; r < ROWS; r++) RACK.push([rackX0 + c * pitch, rackY0 + r * pitch]);
    for (let c = 0; c < NATC; c++) for (let r = 0; r < ROWS; r++) if (c < NATC - 1 || r < 2) RACK.push([natX0 + c * pitch, rackY0 + r * pitch]);
    // The tools this turn calls, at their places in the rack, grouped by server and roughly
    // above the system each one reaches.
    const cell = (b, c, r) => (b ? MCPC * ROWS : 0) + c * ROWS + r;
    const TOOL = {
      dlist: cell(0, 1, 0), dschema: cell(0, 1, 2), dpub: cell(0, 2, 1), dframe: cell(0, 2, 3),
      cal: cell(0, 5, 2), date: cell(0, 8, 0),
      kbtl: cell(0, 11, 1), kbtop: cell(0, 11, 3), kbtodo: cell(0, 12, 0), kbsearch: cell(0, 12, 2),
      sched: cell(0, 15, 1), mail: cell(0, 16, 3),
      peers: cell(1, 3, 1), send: cell(1, 3, 3), memory: cell(1, 8, 2),
    };
    Object.entries(TOOL).forEach(([k, i]) => add({ id: `t_${k}`, kind: "anchor", x: RACK[i][0], y: RACK[i][1], r: 1 }));
    add({ id: "rack", kind: "anchor", x: rackX0, y: rackY0, r: 1 });

    const edges = [
      { pairs: [["parent", "pa"]], alpha: 0.3, dash: [2, 3] },
      { pairs: [["agent", "pa"]], alpha: 0.1 },
      { pairs: OTHERS.map((id) => [id, "kb"]), alpha: 0.14, dash: [2, 3] },
    ];

    /* ---------- The turn ---------- */
    const script = [];
    const hopDur = (a, b) => { const A = N[a], B = N[b]; return Math.max(0.22, Math.min(0.55, Math.hypot(B.x - A.x, B.y - A.y) / (400 * S))); };
    const hop = (t, a, b, c = "mid", land = true) => { script.push({ t, msg: [a, b], c, land, dur: hopDur(a, b) }); return t + hopDur(a, b); };
    // Tools by their generic names (vendor prefixes dropped).
    const NAME = {
      date: "date_info", dlist: "display.list", dschema: "display.get_schema", dpub: "display.publish", dframe: "display.get_frame",
      cal: "calendar.list", sched: "schedule.activities_by_date", kbtl: "kb.upcoming_timeline", kbtop: "kb.top_of_mind",
      kbtodo: "kb.list_todos", kbsearch: "kb.search_collection", mail: "mail.search", peers: "peers", send: "send_message", memory: "memory",
    };
    const log = (t, kind, str, tool) => script.push({ t, log: tool ? { kind, tool: NAME[tool] || tool, text: str } : { kind, text: str } });
    const USE = {}; // tool -> [[call, return]]
    const ROUTES = []; // each call's path: agent -> tool -> systems, lit while live
    const CALLS = []; // call times, for the counter
    const use = (tool, a, b) => { (USE[tool] = USE[tool] || []).push([a, b]); CALLS.push(a); };
    // One tool call: the line prints, the call goes to its tool and on to the system it
    // reaches, and the result comes back to the agent.
    const call = (t, tool, to, args, result, wait = 0.2) => {
      log(t, "tool", args, tool);
      const dot = `t_${tool}`;
      const reach = hop(t, "agent", dot, "mid", true);
      let back = reach;
      if (to) {
        [].concat(to).forEach((id, i) => {
          const land = hop(reach + i * 0.06, dot, id, "mid", true);
          back = Math.max(back, hop(land + wait, id, "agent", "mid", false));
        });
      } else back = hop(reach + wait, dot, "agent", "mid", false);
      use(tool, t, back);
      ROUTES.push({ t0: t, t1: back, via: dot, to: [].concat(to || []) });
      if (result) log(back, "result", result);
      return back;
    };
    const GAP = 0.34;

    script.push({ t: 0, cap: { key: "turn", title: "One display refresh", body: "23 tool calls, three messages to a parent's agent and one memory write, in about seven minutes." } });

    // Input, restored context, injected memory.
    const inAt = hop(0.3, "console", "agent", "accent");
    log(inAt, "input", "From the operator console, relaying a parent: the earlier blocker is cleared. Refresh both displays now and make the child's appointment prominent.");
    const ctxAt = inAt + 0.9;
    log(ctxAt + 0.5, "note", "A long-lived session: 8,640 durable rows since August, about 255 live messages after compaction. 147 tools resolved.");
    const injAt = ctxAt + 2.0;
    script.push({ t: injAt + 0.6, mem: "agent", op: "read" });
    log(injAt + 0.6, "memory", "2 records injected, picked by rank. Fact: the child's appointment waits for priority placement at the next full refresh. Fact: yesterday's evening frame was accepted on both displays; the picture schedule could not be verified.");

    let t = injAt + 2.6;
    t = call(t, "date", null, "", "Today and the next 14 days, with weekdays.") + GAP;
    script.push({ t, on: "agent", d: 1.2 });
    log(t, "think", "Refresh the calendars and the school information, replace both frames, and put the appointment and its pickup at the top.");
    t += 1.7;
    t = call(t, "dlist", ["d1", "d2"], "", "2 household e-ink displays, family audience.") + GAP;
    t = call(t, "dschema", "d1", "display 1", "Frame schema: headline, today, soon, actions (title, owner, due).") + GAP * 0.7;
    t = call(t, "dschema", "d2", "display 2", "Same schema.") + GAP;
    t = call(t, "cal", "cal1", "a parent's calendar, today to +4 days", "A handful of events: a work meeting, a family meeting. 4 KB.");
    const taintAt = t;
    log(t + 0.45, "note", "Calendar content is untrusted input. The session is now marked tainted.");
    t += 1.3;
    t = call(t, "cal", "cal2", "the children's calendar, same window", "School exceptions, an activity, the child's appointment next week. 9 KB.") + GAP * 0.8;
    t = call(t, "cal", "cal3", "the other parent's calendar, read-only", "A few events. 4 KB. Read, never written.") + GAP;
    t = call(t, "sched", "pics", "today to +4 days", "Picture-schedule activities: school days, pickups, an activity. 7 KB.") + GAP;
    t = call(t, "kbtl", "kb", "7 days, 15 items", "The household timeline, each item with its assertion references and confidence.") + GAP * 0.8;
    t = call(t, "kbtop", "kb", "6 items", "Upcoming items flagged as top of mind.") + GAP * 0.8;
    const todoFrom = t;
    t = call(t, "kbtodo", "kb", "open, 3 items", "Open household to-dos from source documents. The slowest step of the turn.", 2.0);
    const todoTo = t;
    t += GAP;
    t = call(t, "kbsearch", "kb", "school mail: this week's class letter", "Found: what to bring and prepare this week.") + GAP * 0.8;
    t = call(t, "kbsearch", "kb", "school mail: today's message", "Found: a replacement timetable for today.") + GAP;
    t = call(t, "mail", "mail", "recent school mail; reason: the index may miss a newer message", "4 recent messages. Nothing newer than the knowledge base already holds.") + GAP;
    t = call(t, "kbsearch", "kb", "school mail: one more message", "Found.") + GAP;
    t = call(t, "date", null, "", "Date checked again before composing.") + GAP * 0.7;
    t = call(t, "peers", null, "", "Wired peers: the person agents, the coordinators, the specialists.") + GAP;
    // Peer messages are tool calls too (send_message); they go sideways, to the parent's agent.
    const peer = (tt, str) => { log(tt, "peer", str, "send"); const land = hop(tt, "agent", "pa", "accent"); use("send", tt, land); ROUTES.push({ t0: tt, t1: land + 0.3, to: ["pa"], peer: true }); return land; };
    t = peer(t, "To a parent's agent: the blocker is cleared by a direct instruction. A full refresh is under way.") + 0.7;
    script.push({ t, on: "agent", d: 1.4 });
    log(t, "think", "One frame for both displays. Actions first: tell the school about the early pickup before the appointment. Then today and soon, display-safe items only.");
    t += 2.0;
    const pubAt = t;
    t = call(t, "dpub", ["d1", "d2"], "both displays: actions, today, soon", "Accepted on both displays, not truncated.", 0.35);
    const pubLand = pubAt + hopDur("agent", "t_dpub") + hopDur("t_dpub", "d1");
    t += GAP;
    t = call(t, "dframe", ["d1", "d2"], "all displays", "Stored frames read back: 7 seconds old, content matches.") + GAP;
    t = peer(t, "To a parent's agent: verified. This is what both displays show now. The school has not been told yet; that is an action for the parent.") + 0.9;
    t = peer(t, "To a parent's agent: the publish and read-back receipts, for your records. No reply needed.") + 1.0;
    // The memory write: the agent's own record, accepted; it supersedes the earlier one.
    const memAt = t;
    use("memory", t, t + 0.9);
    log(t, "tool", "record a fact: the appointment and its pickup are on top of both displays; the school is not told yet", "memory");
    const tW = memAt + 0.6; // the record lands in its slot
    script.push({ t: tW, mem: "agent", op: "write" });
    log(memAt + 1.1, "memory", "Accepted. The new record supersedes the earlier 'pending priority' record.");
    const quarAt = memAt + 3.1;
    script.push({ t: quarAt, mem: "agent", op: "quar" });
    log(quarAt, "note", "The agent wrote this record itself, so it is accepted. A copy distilled in the background from this session would be quarantined: the session read calendar and mail content.");
    const replyLand = hop(quarAt + 2.4, "agent", "console", "accent");
    log(replyLand, "result", "Final answer to the operator: both displays refreshed, the appointment and its pickup on top, telling the school still open.");
    const DUR = replyLand + 4.5;

    /* ---------- Drawing ---------- */
    const TR = rng(23);
    const durTicks = Array.from({ length: 480 }, () => 0.4 + 0.6 * TR());
    const liveTicks = Array.from({ length: 160 }, () => 0.5 + 0.5 * TR());
    const LOGS = script.filter((e) => e.log).map((e) => e.t).sort((a, b) => a - b);
    const head = (ctx, s, str, x, yy, o = {}) => text(ctx, s, str, x, yy, { size: 8, w: 600, c: COL.ink, a: 0.72, track: 1, halo: true, base: "middle", ...o });
    const small = (ctx, s, str, x, yy, o = {}) => text(ctx, s, str, x, yy, { size: 8.5, w: 500, c: COL.mid, halo: true, base: "middle", ...o });

    // Text that has to fit: the longest version that does.
    const ctxHead = "CONTEXT";
    const ctxRoom = W * 0.955 - tape.x0 - tw("HISTORY", 8, 600, 1) - 16 * S - tw(ctxHead, 8, 600, 1) - 8 * S;
    const ctxNote = fit(["about 255 messages", "about 255"], ctxRoom) || "";
    const durNote = fit(["8,640 rows since August", "8,640 rows"], tape.d1 - tape.x0) || "";
    const ghostNote = fit(["distilled copy: quarantined", "copy quarantined", "quarantined"], labelRoom, 8.5, 600) || "";
    const callsX = rackX0 + rackW + 8 * S;

    // The working set by rank. After the write everything shifts down one slot for the new
    // record; the superseded record then leaves the working set (it is kept, marked superseded)
    // and the column closes up again.
    const recs = KINDS.map((kind, i) => ({ kind, i }));
    const tLeave = tW + 2.0;
    const OUT = 7.2; // where the superseded record rests, below the working set
    const phase = (tt) => [ease((tt - tW) / 0.6), ease((tt - tLeave) / 0.7)];
    const slotOf = (rec, tt) => {
      const [push, close] = phase(tt);
      if (rec.i === 0) return tt < tLeave + 0.35 ? push : OUT;
      return rec.i + push - close;
    };
    // The superseded record fades out of its slot and reappears below the working set.
    const recAlpha = (rec, tt) => (rec.i !== 0 || tt < tLeave ? 1 : tt < tLeave + 0.35 ? 1 - (tt - tLeave) / 0.35 : clamp01((tt - tLeave - 0.5) / 0.4));
    const injected = (rec, tt) => rec.i < 2 && tt >= injAt && tt < injAt + 1.6;

    // Everything custom rides along with the agent during the zoom: same centre, same scale.
    const frameOf = (net) => {
      const g = net.node("agent");
      if (!g || g.a < 0.02) return null;
      const k = g.r / Ra;
      return { g, k, A: (x, yy) => [g.x + (x - ax) * k, g.y + (yy - ay) * k] };
    };

    const decor = (ctx, net) => {
      const F = frameOf(net);
      if (!F) return;
      const { g, k, A } = F;
      const s = net.S;
      ctx.globalAlpha = g.a;

      // History and context: every durable row since August, then the live context after
      // compaction (a summary and the recent messages), which grows with each line of this turn.
      const [X0, Y] = A(tape.x0, tapeY);
      const sx = (x) => X0 + (x - tape.x0) * k;
      const h = hMax * k;
      ctx.lineWidth = 1;
      const dStep = Math.max(1.5, (tape.d1 - tape.x0) / durTicks.length);
      ctx.strokeStyle = rgba(COL.ink, 0.2);
      ctx.beginPath();
      for (let i = 0, x = tape.x0; x <= tape.d1; i++, x += dStep) { const hh = durTicks[i % durTicks.length] * h; ctx.moveTo(sx(x) + 0.5, Y - hh / 2); ctx.lineTo(sx(x) + 0.5, Y + hh / 2); }
      ctx.stroke();
      ctx.strokeStyle = rgba(COL.ink, 0.28);
      ctx.beginPath();
      ctx.moveTo(sx(tape.d1) + 3 * s * k, Y - h / 2); ctx.lineTo(sx(tape.l0) - 3 * s * k, Y - h * 0.18);
      ctx.moveTo(sx(tape.d1) + 3 * s * k, Y + h / 2); ctx.lineTo(sx(tape.l0) - 3 * s * k, Y + h * 0.18);
      ctx.stroke();
      const sumW = 4 * S;
      ctx.fillStyle = rgba(COL.ink, 0.85);
      ctx.fillRect(sx(tape.l0), Y - h / 2, sumW * k, h);
      const nNew = LOGS.filter((v) => v <= net.t).length;
      const liveEnd = tape.l0 + (tape.l1 - tape.l0) * (255 + nNew) / 255;
      const lStep = (tape.l1 - tape.l0 - sumW - 2) / 60;
      ctx.strokeStyle = rgba(COL.ink, 0.6);
      ctx.beginPath();
      for (let i = 0, x = tape.l0 + sumW + 2; x <= liveEnd; i++, x += lStep) {
        const hh = liveTicks[i % liveTicks.length] * h;
        if (x > tape.l1) { ctx.stroke(); ctx.strokeStyle = rgba(COL.ink, 0.95); ctx.beginPath(); }
        ctx.moveTo(sx(x) + 0.5, Y - hh / 2); ctx.lineTo(sx(x) + 0.5, Y + hh / 2);
      }
      ctx.stroke();
      // The feed: the live context into the agent.
      const fx = sx(tape.feed), fy = Y + h / 2 + 3 * s * k;
      const ea = -0.36 * Math.PI;
      const ex = g.x + Math.cos(ea) * (g.r + 3 * s * k), ey = g.y + Math.sin(ea) * (g.r + 3 * s * k);
      const c1 = [fx, fy + (ey - fy) * 0.6], c2 = [ex + (fx - ex) * 0.15, ey - (ey - fy) * 0.4];
      ctx.strokeStyle = rgba(COL.ink, 0.2);
      ctx.beginPath(); ctx.moveTo(fx, fy); ctx.bezierCurveTo(c1[0], c1[1], c2[0], c2[1], ex, ey); ctx.stroke();
      const pf = (net.t - ctxAt) / 1.0;
      if (pf > 0 && pf < 1) {
        const u = ease(pf), v = 1 - u;
        const bx = v * v * v * fx + 3 * v * v * u * c1[0] + 3 * v * u * u * c2[0] + u * u * u * ex;
        const by = v * v * v * fy + 3 * v * v * u * c1[1] + 3 * v * u * u * c2[1] + u * u * u * ey;
        ctx.fillStyle = rgba(COL.ink, 1);
        ctx.beginPath(); ctx.arc(bx, by, 2.3 * s, 0, Math.PI * 2); ctx.fill();
      }

      // Routes: each call lights its path while it runs; the turn's used paths stay faintly drawn.
      ROUTES.forEach((r) => {
        if (net.t < r.t0) return;
        const live = net.t < r.t1 + 0.15;
        const fade = live ? Math.min(1, (net.t - r.t0) / 0.15) : 1;
        ctx.strokeStyle = rgba(live && r.peer ? COL.accent : COL.mid, live ? 0.55 * fade : 0.07);
        ctx.lineWidth = live ? 1.2 * s : 1;
        ctx.beginPath();
        if (r.via) {
          const v = net.node(r.via);
          ctx.moveTo(g.x, g.y); ctx.lineTo(v.x, v.y);
          r.to.forEach((id) => { const n = net.node(id); ctx.moveTo(v.x, v.y); ctx.lineTo(n.x, n.y); });
        } else r.to.forEach((id) => { const n = net.node(id); ctx.moveTo(g.x, g.y); ctx.lineTo(n.x, n.y); });
        ctx.stroke();
      });

      // Memory: a tie from the agent's arc to each record.
      const tie = (sl, c, a, w) => {
        const [sxv, syv] = spineAt(sl);
        const P = [g.x + sxv * k, g.y + syv * k];
        const [qx, qy] = A(colX + recS / 2 + 1.5 * s, colY0 + sl * slotH);
        ctx.strokeStyle = rgba(c, a); ctx.lineWidth = w;
        ctx.beginPath(); ctx.moveTo(qx, qy); ctx.quadraticCurveTo(qx + (P[0] - qx) * 0.62, qy, P[0], P[1]); ctx.stroke();
      };
      recs.forEach((rec) => {
        const a = recAlpha(rec, net.t);
        if (a <= 0) return;
        const inj = injected(rec, net.t);
        const out = rec.i === 0 && net.t >= tLeave ? 0 : 1; // a superseded record leaves the working set
        tie(slotOf(rec, net.t), inj ? COL.mid : COL.ink, (inj ? 0.85 : 0.17) * a * out, inj ? 1.5 * s : 1);
      });
      if (net.t >= tW) { const hot = net.t < tW + 1.3; tie(0, COL.ink, hot ? 0.75 : 0.17, hot ? 1.5 * s : 1); }
      // The long tail of records outside the working set.
      const [push, close] = phase(net.t);
      const tail = 7 + push - close + 0.85 * close;
      ctx.fillStyle = rgba(COL.ink, 0.22);
      for (let j = 0; j < 2; j++) { const [x, yy] = A(colX, colY0 + (tail + 0.1 + j * 0.34) * slotH); ctx.beginPath(); ctx.arc(x, yy, Math.max(0.9, 1.1 * s), 0, Math.PI * 2); ctx.fill(); }
      ctx.globalAlpha = 1;
    };

    const overlay = (ctx, net) => {
      const F = frameOf(net);
      if (!F) return;
      const { g, k, A } = F;
      const s = net.S;
      const ga = g.a;

      // History and context labels.
      {
        const [x0, yy] = A(tape.x0, tapeY);
        const [xr] = A(W * 0.955, tapeY);
        const up = yy - (hMax / 2 + 8 * S) * k, down = yy + (hMax / 2 + 8 * S) * k;
        head(ctx, s, "HISTORY", x0, up, { a: 0.72 * ga });
        if (durNote) small(ctx, s, durNote, x0, down, { a: ga });
        if (ctxNote) small(ctx, s, ctxNote, xr, up, { a: ga, align: "right", c: COL.ink });
        head(ctx, s, ctxHead, xr - (ctxNote ? tw(ctxNote) + 7 * S : 0), up, { a: 0.72 * ga, align: "right" });
      }

      // Memory records, by rank, each with its kind.
      const square = (x, yy, c, a) => { ctx.globalAlpha = a; ctx.fillStyle = c; ctx.fillRect(x - recS / 2, yy - recS / 2, recS, recS); ctx.globalAlpha = 1; };
      const slotXY = (sl) => A(colX, colY0 + sl * slotH);
      {
        const [hx, hy] = A(colX + recS / 2, colY0 - slotH * 1.15);
        head(ctx, s, "MEMORY", hx, hy, { align: "right", a: 0.72 * ga });
      }
      recs.forEach((rec) => {
        const a = ga * recAlpha(rec, net.t);
        if (a <= 0.01) return;
        const [x, yy] = slotXY(slotOf(rec, net.t));
        const inj = injected(rec, net.t);
        const superseded = rec.i === 0 && net.t >= tW + 0.7;
        square(x, yy, rgba(inj ? COL.mid : COL.ink, 1), a * (superseded ? 0.35 : 1));
        small(ctx, s, superseded ? "superseded" : rec.kind, x - recS / 2 - 6 * s, yy, { align: "right", a, c: inj ? COL.ink : COL.mid, w: inj ? 600 : 500 });
        if (superseded) {
          ctx.globalAlpha = a; ctx.strokeStyle = rgba(COL.ink, 0.6); ctx.lineWidth = 1;
          ctx.beginPath(); ctx.moveTo(x - recS, yy); ctx.lineTo(x + recS, yy); ctx.stroke(); ctx.globalAlpha = 1;
        }
      });
      // Injection: the two top records travel into the agent.
      const pi = (net.t - injAt) / 0.9;
      if (pi > 0 && pi < 1) {
        const u = ease(pi);
        [0, 1].forEach((i) => {
          const [x0, y0] = slotXY(i);
          const [sxv, syv] = spineAt(i);
          const x1 = g.x + sxv * 0.5 * k, y1 = g.y + syv * 0.5 * k;
          ctx.fillStyle = rgba(COL.mid, 1 - u * 0.5);
          ctx.beginPath(); ctx.arc(x0 + (x1 - x0) * u, y0 + (y1 - y0) * u, 2.3 * s, 0, Math.PI * 2); ctx.fill();
        });
      }
      // The write: a record leaves the agent and takes the top slot.
      if (net.t >= memAt && net.t < tW) {
        const u = ease((net.t - memAt) / (tW - memAt));
        const [x1, y1] = slotXY(0);
        ctx.fillStyle = rgba(COL.ink, 1);
        ctx.beginPath(); ctx.arc(g.x + (x1 - g.x) * u, g.y + (y1 - g.y) * u, 2.3 * s, 0, Math.PI * 2); ctx.fill();
      }
      if (net.t >= tW) {
        const [x, yy] = slotXY(0);
        const a = ga * clamp01((net.t - tW) / 0.3);
        square(x, yy, rgba(COL.ink, 1), a);
        small(ctx, s, "fact, new", x - recS / 2 - 6 * s, yy, { align: "right", a, c: COL.ink, w: net.t < tW + 2.8 ? 600 : 500 });
      }
      // A copy distilled in the background from this tainted session would land quarantined.
      if (net.t >= quarAt - 0.1 && ghostNote) {
        const a = ga * clamp01((net.t - quarAt + 0.1) / 0.4);
        const [x, yy] = slotXY(8.75);
        ctx.globalAlpha = a;
        ctx.strokeStyle = rgba(COL.accent, 1); ctx.lineWidth = 1.2;
        ctx.strokeRect(x - recS / 2 + 0.5, yy - recS / 2 + 0.5, recS - 1, recS - 1);
        ctx.beginPath(); ctx.moveTo(x - recS / 2 + 1, yy + recS / 2 - 1); ctx.lineTo(x + recS / 2 - 1, yy - recS / 2 + 1); ctx.stroke();
        ctx.globalAlpha = 1;
        small(ctx, s, ghostNote, x - recS / 2 - 6 * s, yy, { align: "right", a, c: COL.accent, w: 600 });
      }

      // The tool rack: 147 resolved tools. The ones this turn calls light while in use and stay marked.
      const rk = net.node("rack");
      if (rk && rk.a > 0.02) {
        const P = (x, yy) => [rk.x + (x - rackX0) * k, rk.y + (yy - rackY0) * k];
        const state = new Map();
        Object.entries(USE).forEach(([tool, spans]) => {
          if (spans.some(([a, b]) => net.t >= a && net.t < b + 0.12)) state.set(TOOL[tool], "live");
          else if (spans.some(([a]) => net.t >= a)) state.set(TOOL[tool], "done");
        });
        RACK.forEach(([x, yy], i) => {
          const st = state.get(i);
          const [px, py] = P(x, yy);
          ctx.fillStyle = st === "live" ? rgba(COL.accent, ga) : st === "done" ? rgba(COL.ink, 0.9 * ga) : rgba(COL.ink, 0.17 * ga);
          ctx.beginPath(); ctx.arc(px, py, (st ? dotR * 1.4 : dotR) * k, 0, Math.PI * 2); ctx.fill();
        });
        const [lx, my] = P(rackX0 - pitch / 2 - 7 * S, rackY0 + 2 * pitch);
        head(ctx, s, "147 TOOLS", lx, my, { align: "right", a: 0.72 * ga });
        const n = CALLS.filter((v) => v <= net.t).length;
        const [rx] = P(callsX, 0);
        small(ctx, s, n === 1 ? "1 call" : `${n} calls`, rx, my, { a: ga, c: n ? COL.ink : COL.mid });
        const [bx0, by] = P(rackX0 - pitch / 2, rackY0 + (ROWS - 1) * pitch + pitch / 2 + 8 * S);
        const [bx1] = P(natX0 - pitch / 2, 0);
        small(ctx, s, "85 MCP", bx0, by, { a: ga });
        small(ctx, s, "62 native", bx1, by, { a: ga });
      }

      // Displays: e-ink panels. Yesterday's frame until the publish lands, then a refresh
      // flash and the new frame: actions first, then today and soon.
      ["d1", "d2"].forEach((id, j) => {
        const n = net.node(id);
        if (!n || n.a < 0.05) return;
        const kk = n.spec.r ? n.r / n.spec.r : 1;
        const w = n.spec.w * s * kk, h = n.spec.h * s * kk;
        const x0 = n.x - w / 2 + 3 * s * kk, y0 = n.y - h / 2 + 3 * s * kk, iw = w - 6 * s * kk;
        const dt = net.t - (pubLand + j * 0.06);
        ctx.globalAlpha = n.a;
        if (dt >= 0 && dt < 0.22) { ctx.fillStyle = rgba(COL.ink, 0.9); ctx.fillRect(n.x - w / 2 + 1.5, n.y - h / 2 + 1.5, w - 2, h - 2); ctx.globalAlpha = 1; return; }
        const rows = dt >= 0.22 ? [[1, 0.9, 1.9], [0.8, 0.5, 1.1], [0.55, 0.5, 1.1], [0.7, 0.3, 1.1]] : [[0.7, 0.3, 1.1], [0.5, 0.3, 1.1], [0.62, 0.3, 1.1]];
        rows.forEach(([len, a, th], r) => { ctx.fillStyle = rgba(COL.ink, a); ctx.fillRect(x0, y0 + r * 2.4 * s * kk, iw * len, Math.max(1, th * s * kk)); });
        ctx.globalAlpha = 1;
      });

      // Labels of the outside systems, under each group; they light while in use.
      DEST.forEach((d) => {
        const ns = d.parts.map((id) => net.node(id)).filter(Boolean);
        if (!ns.length || ns[0].a < 0.3) return;
        const cx = (ns[0].x + ns[ns.length - 1].x) / 2;
        const hot = ns.some((m) => m.flash > net.t || m.active > net.t);
        small(ctx, s, d.label, cx, ns[0].y + (d.half + 9 * S) * k, { align: "center", a: Math.min(1, ns[0].a / (N[d.parts[0]].alpha ?? 1)), c: hot ? COL.accent : COL.mid });
      });

      // The knowledge base is slow on one call: the to-do list takes the longest.
      if (net.t >= todoFrom + hopDur("agent", "t_kbtodo") + hopDur("t_kbtodo", "kb") && net.t < todoTo - hopDur("kb", "agent")) {
        const n = net.node("kb");
        const ph = (net.t * 2.2) % 1;
        ctx.strokeStyle = rgba(COL.accent, 0.5 * (1 - ph));
        ctx.lineWidth = 1;
        ctx.beginPath(); ctx.arc(n.x, n.y, kbR * (1.6 + ph * 0.8) * k, 0, Math.PI * 2); ctx.stroke();
      }

      // Taint: from the first calendar read on, the session is marked.
      if (net.t >= taintAt) {
        const a = ga * clamp01((net.t - taintAt) / 0.3);
        const f11 = Math.max(9, 11 * s);
        const ly = g.y + g.r + 5 * s + f11 / 2 + f11 * 1.1;
        text(ctx, s, "session tainted", g.x, ly, { size: 8.5, w: 600, c: COL.accent, a, align: "center", base: "middle", halo: true });
      }
    };

    return {
      nodes, edges, script, duration: DUR, decor, overlay,
      origin: [W / 2, H / 2],
      staticAt: quarAt + 1.2,
      captionReset: null,
    };
  }

  window.MeerkatHousehold = household;
  window.MeerkatHouseholdAgent = (W, H, S, opts, helpers) => householdAgent(W, H, S, opts, helpers);
})();
