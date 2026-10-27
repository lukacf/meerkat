/* Scenes: real topologies and the dynamics reported by the systems that run them.
   Each factory returns { nodes, edges, script, duration, decor, overlay, origin, staticAt }. */
(() => {
  "use strict";
  const { COL, rgba, SANS } = window.MeerkatNet;
  const TAU = Math.PI * 2;
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  // Deterministic pseudo-random numbers so layouts are stable across resizes.
  function rng(seed) {
    let a = seed >>> 0;
    return () => {
      a = (a + 0x6d2b79f5) >>> 0;
      let t = a;
      t = Math.imul(t ^ (t >>> 15), t | 1);
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }

  const text = (ctx, S, str, x, y, o = {}) => {
    ctx.font = `${o.w || 500} ${Math.max(o.min || 9, (o.size || 10) * S)}px ${SANS}`;
    ctx.fillStyle = rgba(o.c || COL.mid, o.a ?? 1);
    ctx.textAlign = o.align || "left";
    ctx.textBaseline = o.base || "alphabetic";
    if (o.track) ctx.letterSpacing = `${o.track}px`;
    if (o.halo) { ctx.lineJoin = "round"; ctx.lineWidth = 3; ctx.strokeStyle = rgba(COL.screen, 0.92); ctx.strokeText(str, x, y); }
    ctx.fillText(str, x, y);
    if (o.track) ctx.letterSpacing = "0px";
  };

  // Section header inside a canvas: small caps, drawn over the wiring.
  const header = (ctx, S, str, x, y, o = {}) => text(ctx, S, str, x, y, Object.assign({ size: 8, w: 600, c: COL.ink, a: 0.7, track: 1, halo: true }, o));

  /* Reduced motion: the engine shows one settled moment (scene.staticAt) with no travel.
     This draws what is in progress at that moment, so the still frame reads like a
     paused animation: active nodes in the accent colour and messages part-way along. */
  function frozen(nodes, script, T, S) {
    if (!reduced || T == null) return () => {};
    const byId = new Map(nodes.map((n) => [n.id, n]));
    const act = new Set(), msgs = [];
    script.forEach((e) => {
      if (e.t > T) return;
      if (e.on != null && e.t + (e.d ?? 1.2) > T) [].concat(e.on).forEach((id) => act.add(id));
      if (e.msg) {
        const a = byId.get(e.msg[0]), b = byId.get(e.msg[1]);
        if (!a || !b) return;
        const dur = e.dur ?? Math.max(0.45, Math.min(1.4, Math.hypot(b.x - a.x, b.y - a.y) / (260 * S)));
        if (e.t + dur > T) msgs.push([e.msg[0], e.msg[1], (T - e.t) / dur, e.c || "accent"]);
        else if (e.land !== false && e.t + dur + 0.5 > T) act.add(e.msg[1]);
      }
    });
    return (ctx, net) => {
      const s = net.S;
      act.forEach((id) => {
        const n = net.node(id);
        if (!n || n.a < 0.3) return;
        const k = n.spec.kind;
        ctx.globalAlpha = n.a;
        if (k === "agent" || k === "coord") {
          ctx.strokeStyle = rgba(COL.accent, 0.35); ctx.lineWidth = 1;
          ctx.beginPath(); ctx.arc(n.x, n.y, n.r + 5 * s, 0, TAU); ctx.stroke();
          ctx.fillStyle = rgba(COL.accent, 1);
          ctx.beginPath(); ctx.arc(n.x, n.y, n.r, 0, TAU); ctx.fill();
        } else if (k === "helper" || k === "person") {
          ctx.strokeStyle = rgba(COL.accent, 1); ctx.lineWidth = 1.4 * s;
          ctx.beginPath(); ctx.arc(n.x, n.y, n.r, 0, TAU); ctx.stroke();
        } else if (k === "source") {
          const w = n.r * 2;
          ctx.fillStyle = rgba(COL.accent, 0.18); ctx.fillRect(n.x - w / 2, n.y - w / 2, w, w);
          ctx.strokeStyle = rgba(COL.accent, 1); ctx.lineWidth = 1.2 * s; ctx.strokeRect(n.x - w / 2, n.y - w / 2, w, w);
        } else if (k === "box") {
          const w = n.spec.w * s, h = n.spec.h * s;
          ctx.strokeStyle = rgba(COL.accent, 1); ctx.lineWidth = 1;
          ctx.strokeRect(n.x - w / 2 + 0.5, n.y - h / 2 + 0.5, w, h);
        }
        ctx.globalAlpha = 1;
      });
      msgs.forEach(([ia, ib, p, c]) => {
        const a = net.node(ia), b = net.node(ib);
        if (!a || !b || a.a < 0.3 || b.a < 0.3) return;
        const e = p < 0.5 ? 2 * p * p : 1 - Math.pow(-2 * p + 2, 2) / 2;
        const e2 = Math.max(0, e - 0.12);
        const col = c === "ink" ? COL.ink : c === "green" ? COL.green : c === "mid" ? COL.mid : COL.accent;
        const x = a.x + (b.x - a.x) * e, y = a.y + (b.y - a.y) * e;
        ctx.strokeStyle = rgba(col, 0.35); ctx.lineWidth = 1.4 * s;
        ctx.beginPath(); ctx.moveTo(a.x + (b.x - a.x) * e2, a.y + (b.y - a.y) * e2); ctx.lineTo(x, y); ctx.stroke();
        ctx.fillStyle = rgba(col, 1);
        ctx.beginPath(); ctx.arc(x, y, 2.3 * s, 0, TAU); ctx.fill();
      });
    };
  }

  /* ------------------------------------------------------------------ */
  /* One command: an agent in a pipe. The pipe leads: lines leave app.log */
  /* and queue in order, the agent takes them in a turn, starts two       */
  /* background jobs, whose results come back along the same wires and    */
  /* queue as inputs, and sends its finding to stdout. Under it, a small  */
  /* transcript prints the same run in step: the command, the lines read, */
  /* a thought, the two shell commands and the finding. It is sized so    */
  /* that the thought is still in view when the finding prints.           */
  /* ------------------------------------------------------------------ */
  const MONO = 'ui-monospace, "SF Mono", Menlo, Consolas, monospace';
  const measureCtx = document.createElement("canvas").getContext("2d");
  const monoW = (fs) => { measureCtx.font = `500 ${fs}px ${MONO}`; return measureCtx.measureText("0").width; };
  // Word wrap to a number of columns; words longer than a row are broken.
  const wrapCols = (str, cols) => {
    const rows = [];
    let row = "";
    str.split(" ").forEach((w) => {
      while (w.length > cols) { if (row) { rows.push(row); row = ""; } rows.push(w.slice(0, cols)); w = w.slice(cols); }
      if (!row) row = w;
      else if (row.length + 1 + w.length <= cols) row += " " + w;
      else { rows.push(row); row = w; }
    });
    if (row || !rows.length) rows.push(row);
    return rows;
  };
  const roundRect = (ctx, x, y, w, h, r) => { if (ctx.roundRect) ctx.roundRect(x, y, w, h, r); else ctx.rect(x, y, w, h); };
  const easeIO = (p) => (p <= 0 ? 0 : p >= 1 ? 1 : p < 0.5 ? 2 * p * p : 1 - Math.pow(-2 * p + 2, 2) / 2);

  function command(W, H, S) {
    const tall = W / H < 1;
    const m = Math.round(Math.max(10, Math.min(16, W * 0.026)));

    /* ---------- The run, one timeline for the pipe and the transcript ---------- */
    const LINES = [2.3, 2.85, 3.4]; // three log lines leave app.log
    const TURN1 = 4.1, TOOL_A = 5.8, TOOL_B = 6.3; // the agent's turn, then two background jobs
    const RES_B = 8.2, RES_A = 8.8; // their results come back as inputs, in order
    const TURN2 = 10.2, FIND = 11.3; // a second turn once both are queued, then the finding goes to stdout
    const DUR = 16.6, staticAt = 14.0;

    /* ---------- The pipe ---------- */
    const Dh = H * 0.5; // the pipe's band; the transcript sits under it
    const yL = m + Dh * 0.74, yJ = m + Dh * 0.22;
    const X = (f) => f * W;
    const NQ = 7;
    const cell = Math.min(15 * S, X(tall ? 0.3 : 0.24) / NQ);
    const qx0 = X(tall ? 0.255 : 0.2);
    const ax = X(tall ? 0.64 : 0.58), jdx = X(tall ? 0.11 : 0.085);
    const nodes = [
      { id: "in", kind: "box", x: X(tall ? 0.125 : 0.1), y: yL, r: 1, w: 84, h: 28, text: "app.log" },
      { id: "agent", kind: "agent", x: ax, y: yL, r: 11 * S, label: "rkat run", always: true, labelPos: "below", labelSize: 11.5 },
      { id: "out", kind: "box", x: X(tall ? 0.875 : 0.9), y: yL, r: 1, w: 84, h: 28, text: "stdout" },
      { id: "ja", kind: "box", x: ax - jdx, y: yJ, r: 1, w: 62, h: 24, text: "zgrep", textSize: 10, alpha: 0.9 },
      { id: "jb", kind: "box", x: ax + jdx, y: yJ, r: 1, w: 62, h: 24, text: "git log", textSize: 10, alpha: 0.9 },
    ];
    const Q = [];
    for (let k = 0; k < NQ; k++) {
      Q.push(`q${k}`);
      nodes.push({ id: `q${k}`, kind: "item", x: qx0 + cell * (k + 0.5), y: yL, r: cell * 0.36, hidden: true, state: "open" });
    }
    const script = [];
    const msg = (t, a, b, c = "ink", o = {}) => script.push(Object.assign({ t, msg: [a, b], c }, o));
    const take = (t, ks) => ks.forEach((k, n) => {
      script.push({ t: t - 0.2 + n * 0.1, state: [`q${k}`, "claimed"] }, { t: t + 0.3 + n * 0.1, state: [`q${k}`, "done"] });
      msg(t - 0.1 + n * 0.1, `q${k}`, "agent", "ink", { dur: 0.45, land: false });
    });
    // Each line becomes an input with its own place in the queue.
    const lineIn = (t, k) => { msg(t, "in", `q${k}`, "ink", { dur: 0.55 }); script.push({ t: t + 0.55, show: `q${k}` }); };
    LINES.forEach((t, k) => lineIn(t, k));
    script.push({ t: TURN1, on: "agent", d: 2.6 });
    take(TURN1, [0, 1, 2]);
    msg(TOOL_A, "agent", "ja", "mid"); script.push({ t: TOOL_A + 0.4, on: "ja", d: RES_A - TOOL_A - 0.4 });
    msg(TOOL_B, "agent", "jb", "mid"); script.push({ t: TOOL_B + 0.4, on: "jb", d: RES_B - TOOL_B - 0.4 });
    // A result comes back along its job's wire, and the runtime queues it after the lines.
    const result = (t, job, k) => {
      msg(t, job, "agent", "ink", { dur: 0.5, land: false });
      msg(t + 0.5, "agent", `q${k}`, "ink", { dur: 0.45 });
      script.push({ t: t + 0.95, show: `q${k}` });
    };
    result(RES_B, "jb", 3);
    result(RES_A, "ja", 4);
    script.push({ t: TURN2, on: "agent", d: 1.7 });
    take(TURN2, [3, 4]);
    msg(FIND, "agent", "out", "accent", { dur: 0.7 });
    script.push({ t: FIND + 0.7, on: "out", d: 1.8 });
    script.push({ t: DUR - 0.6, hide: Q });
    const edges = [
      { pairs: [["agent", "out"]], alpha: 0.16 },
      { pairs: [["agent", "ja"], ["agent", "jb"]], alpha: 0.14, dash: [2, 3] },
    ];
    const still = frozen(nodes, script, staticAt, S);

    /* ---------- The transcript, under the pipe ---------- */
    const P = { x: m, y: Math.round(m + Dh + (tall ? 6 : 8)), w: W - 2 * m };
    P.h = H - m - P.y;
    const padX = tall ? 10 : 14, padY = tall ? 8 : 10;
    const fs = tall ? Math.max(8.5, Math.min(10, W * 0.028)) : Math.max(9.5, Math.min(11, W * 0.0168));
    const cw = monoW(fs), lh = Math.round(fs * 1.5);
    const gut = Math.round(fs * 3.6); // the gutter: a short tag per entry
    const tx = P.x + padX + gut, cols = Math.max(24, Math.floor((P.w - 2 * padX - gut) / cw));
    // t: when it starts; type: how long it takes to type or stream.
    // Whole rows only (one blank row before the finding), so the view never stops on half a row.
    const ENTRIES = [
      { t: 0.1, delay: 0.1, k: "cmd", tag: "$", s: 'tail -f app.log | rkat run -t workspace --stdin lines "Flag anything a person should act on."', type: 1.7 },
      ...LINES.map((t, k) => ({ t, k: "in", tag: "stdin", s: [
        "10:41:03 INFO  POST /checkout 201 212ms",
        "10:41:07 WARN  db pool 19/20 in use",
        "10:41:08 ERROR /checkout 503 pool exhausted",
      ][k] })),
      { t: TURN1 + 0.2, k: "think", tag: "think", s: "Checkout fails: the pool is full. Is this new? Check older logs and recent changes.", type: 1.2 },
      { t: TOOL_A, k: "tool", tag: "tool", pre: "shell ", s: "zgrep -c 'pool exhausted' app.log.*.gz", type: 0.35 },
      { t: TOOL_B, k: "tool", tag: "tool", pre: "shell ", s: "git log -S pool_size --since=7.days", type: 0.35 },
      { t: FIND + 0.2, k: "out", tag: "", s: "Checkout has returned 503 since 10:41: the database pool is exhausted. No pool errors in the previous 7 days. Commit 3f2a91c lowered pool_size from 40 to 20, 19 hours ago. Revert it or raise the pool size.", type: 2.2, gap: 1 },
    ];
    const t0 = (e) => e.t + (e.delay || 0);
    // Rows: log lines are clipped like a terminal; everything else wraps at word boundaries.
    const ROWS = [];
    ENTRIES.forEach((e) => {
      const full = (e.pre || "") + e.s;
      const parts = e.k === "in" ? [full.length > cols ? full.slice(0, cols - 1) + "…" : full] : wrapCols(full, cols);
      let c0 = 0;
      parts.forEach((str, i) => {
        const start = full.indexOf(str, c0);
        ROWS.push({ e, str, first: i === 0, c0: start, appear: i === 0 ? e.t : e.type ? t0(e) + e.type * (start / full.length) : e.t + i * 0.08, gap: i === 0 ? e.gap || 0 : 0 });
        c0 = start + str.length;
      });
      e.len = full.length;
    });
    const typed = (e, t) => (e.type ? Math.floor(e.len * Math.max(0, Math.min(1, (t - t0(e)) / e.type))) : e.len);
    // A light panel: the transcript accompanies the pipe and does not outweigh it.
    const PANEL = [241, 240, 235];

    const overlay = (ctx, net) => {
      const s = net.S, t = reduced ? staticAt : net.t;
      const inb = net.node("in"), ag = net.node("agent");
      ctx.save();
      // The queue: a thin rail from app.log through the cells to the agent, named once.
      ctx.globalAlpha = inb ? inb.a : 1;
      ctx.strokeStyle = rgba(COL.ink, 0.16); ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(inb.x + 42 * s, yL); ctx.lineTo(qx0, yL);
      ctx.moveTo(qx0 + cell * NQ, yL); ctx.lineTo(ag.x - ag.r, yL);
      ctx.stroke();
      ctx.strokeStyle = rgba(COL.ink, 0.28);
      ctx.strokeRect(qx0 + 0.5, yL - cell / 2 + 0.5, cell * NQ, cell);
      text(ctx, s, "inputs, in order", qx0, yL - cell / 2 - 6 * s, { size: 9, c: COL.mid, base: "bottom", halo: true });
      text(ctx, s, "background jobs", ax, yJ - 12 * s - 6, { size: 9, c: COL.mid, align: "center", base: "bottom", halo: true });
      ctx.globalAlpha = 1;
      still(ctx, net);

      // The transcript panel.
      const fadeIn = Math.min(1, ag ? ag.a : 1);
      ctx.globalAlpha = fadeIn;
      const rad = 6;
      ctx.fillStyle = rgba(PANEL, 1);
      ctx.beginPath(); roundRect(ctx, P.x + 0.5, P.y + 0.5, P.w - 1, P.h - 1, rad); ctx.fill();
      ctx.strokeStyle = rgba(COL.ink, 0.1); ctx.lineWidth = 1; ctx.stroke();
      ctx.save();
      // Rows that scroll out are cut at the top padding, so no half row lingers above the first.
      ctx.beginPath(); ctx.rect(P.x, P.y + Math.round(padY * 0.75), P.w, P.h - Math.round(padY * 0.75) - 3); ctx.clip();
      // Rows slide in; the view follows the newest one, keeping a line for the cursor.
      const vis = ROWS.filter((r) => r.appear <= t);
      const grow = (r) => (reduced ? 1 : easeIO((t - r.appear) / 0.16));
      let content = 0;
      vis.forEach((r) => { content += (r.gap + 1) * lh * grow(r); });
      const view = Math.max(1, Math.floor((P.h - 2 * padY) / lh) - 1) * lh;
      let y = P.y + padY - Math.max(0, content - view);
      const typing = ENTRIES.find((e) => e.type && t >= e.t && t < t0(e) + e.type);
      const lastOut = vis.filter((r) => r.e.k === "out").slice(-1)[0];
      let cur = null;
      vis.forEach((r) => {
        const g = grow(r);
        y += r.gap * lh * g;
        const e = r.e, base = y + lh * 0.72;
        if (y > P.y - lh && y < P.y + P.h) {
          if (r.first && e.tag) {
            ctx.font = `500 ${fs * 0.82}px ${MONO}`;
            ctx.fillStyle = rgba(COL.mid, 0.8); ctx.textAlign = "right"; ctx.textBaseline = "alphabetic";
            ctx.fillText(e.tag, tx - cw * 1.2, base);
          }
          if (e.k === "out") { ctx.fillStyle = rgba(COL.accent, 1); ctx.fillRect(tx - cw * 1.2 - 2, y + (r.first ? 3 : 0), 2, lh * g - (r.first ? 3 : 0) + (r === lastOut ? -3 : 0.5)); }
          const n = Math.max(0, Math.min(r.str.length, typed(e, t) - r.c0));
          let str = r.str.slice(0, n), x = tx;
          ctx.textAlign = "left";
          if (e.pre && r.first) {
            const pre = str.slice(0, e.pre.length);
            ctx.font = `italic 500 ${fs}px ${MONO}`; ctx.fillStyle = rgba(COL.accent, 1);
            ctx.fillText(pre, x, base);
            x += cw * pre.length; str = str.slice(e.pre.length);
          }
          const style = e.k === "think" ? "italic 400" : e.k === "out" || e.k === "cmd" ? "500" : "400";
          const strong = e.k === "cmd" || e.k === "out" || e.k === "tool";
          ctx.font = `${style} ${fs}px ${MONO}`; ctx.fillStyle = rgba(strong ? COL.ink : COL.mid, strong ? 0.9 : 1);
          ctx.fillText(str, x, base);
          if (e === typing && (n > 0 || r.first)) cur = [x + cw * str.length, y];
        }
        y += lh * g;
      });
      // The cursor: after the text being typed, or on the next line while the pipe runs.
      const settling = vis.some((r) => grow(r) < 1);
      if (!cur && !settling) cur = [tx, y];
      const blink = cur && (reduced || typing || (t % 1.05) < 0.62);
      if (blink) { ctx.fillStyle = rgba(COL.accent, 0.85); ctx.fillRect(cur[0] + 1, cur[1] + lh * 0.16, cw * 0.6, lh * 0.68); }
      ctx.restore();
      // Older lines fade out under the top edge.
      const fadeH = padY, grad = ctx.createLinearGradient(0, P.y, 0, P.y + fadeH);
      grad.addColorStop(0, rgba(PANEL, 1)); grad.addColorStop(0.65, rgba(PANEL, 1)); grad.addColorStop(1, rgba(PANEL, 0));
      ctx.save(); ctx.beginPath(); roundRect(ctx, P.x + 1, P.y + 1, P.w - 2, P.h - 2, rad); ctx.clip();
      ctx.fillStyle = grad; ctx.fillRect(P.x, P.y, P.w, fadeH);
      ctx.restore();
      ctx.restore();
    };
    return { nodes, script, duration: DUR, staticAt, edges, overlay, origin: [ax, yL] };
  }

  /* ------------------------------------------------------------------ */
  /* In a product: a library first. The diagram leads.                    */
  /* Beat 1, inside your service: a request reaches your code, and Meerkat */
  /* sits directly under it, a library called in-process. The builder     */
  /* lines configure what they name, one at a time (the agent, its model, */
  /* the output schema, your functions as its tools); agent.run calls the */
  /* model and both tools, and a typed result comes back to your code,    */
  /* which saves it and answers.                                          */
  /* Beat 2, from outside: the service dissolves and the same Meerkat box */
  /* stays where it is. A Python app connects over JSON-RPC and runs one  */
  /* session; then a TypeScript app, an HTTP client and an MCP client     */
  /* connect, and all four are served at once. The last moment of the     */
  /* loop is its first, so it repeats without a jump.                     */
  /* Under the diagram, the code that runs, one line at a time, in step   */
  /* with the dots; the typed result fills the shape the schema declared. */
  /* ------------------------------------------------------------------ */
  function product(W, H, S) {
    const tall = W / H < 1;
    const m = Math.round(Math.max(10, Math.min(16, W * 0.026)));
    const u = Math.max(0.72, Math.min(1.12, S));
    const tw = (str, font) => { measureCtx.font = font; return measureCtx.measureText(str).width; };

    /* ---------- What runs ---------- */
    const RUST = [
      "let mut agent = AgentBuilder::new()",
      '    .model("claude-opus-5-5")',
      "    .output_schema(OutputSchema::new(schema)?)",
      "    .build(llm, tools, store).await?;",
      "let out = agent.run(ticket.into()).await?",
      "    .structured_output.ok_or(NoOutput)?;",
      "db.save(&out).await?;",
      "Ok(Json(out))",
    ];
    const PY = [
      "from meerkat import MeerkatClient",
      "async with MeerkatClient() as client:",
      "    session = await client.create_session(",
      '        ticket, model="claude-opus-5-5",',
      "        output_schema=schema)",
      "    print(session.structured_output)",
    ];
    // The typed result: the keys are the schema's, the values arrive with the result.
    const OUT1 = [["action", '"refund"'], ["order", '"A-4471"'], ["amount", "49.0"]];
    const OUT2 = [["action", '"replace"'], ["order", '"A-4502"'], ["amount", "0.0"]];
    const jsonHead = (o, i) => `${i ? "  " : "{ "}"${o[i][0]}": `;
    const jsonTail = (o, i) => (i < o.length - 1 ? "," : " }");

    /* ---------- One timeline for the diagram and the code ---------- */
    const T0 = 0.5;
    const at1 = (a, b) => [T0 + a, T0 + b];
    // When each line runs: [from, to]. A builder line holds long enough to see what it configures.
    const RUN1 = [at1(0.9, 1.6), at1(1.6, 2.3), at1(2.3, 3.0), at1(3.0, 3.7), at1(3.7, 8.0), at1(8.0, 8.6), at1(8.6, 9.3), at1(9.3, 10.2)];
    const RUNT = T0 + 3.7; // agent.run
    const SW = T0 + 10.6; // the service dissolves
    const B2 = SW + 0.7; // the Python app starts
    const PYT = B2 + 0.9; // create_session
    const RUN2 = [[B2, B2 + 0.35], [B2 + 0.35, B2 + 0.9], [PYT, PYT + 2.2], [PYT, PYT + 2.2], [PYT, PYT + 2.2], [PYT + 2.2, PYT + 2.75]];
    const WAIT1 = [4, RUNT + 0.35, RUNT + 3.95], WAIT2 = [4, PYT + 0.5, PYT + 1.75]; // a line that waits on the agent
    const GOT1 = RUN1[5][0], GOT2 = PYT + 2.25; // the typed result lands in the code
    const P0 = SW + 4.4; // the other clients connect
    const R0 = P0 + 4.1; // back to the start
    const DUR = R0 + 1.2, staticAt = RUNT + 2.05;

    /* ---------- The code panel, under the diagram ---------- */
    const P = { x: m, w: W - 2 * m };
    const padX = tall ? 10 : 14, padY = tall ? 8 : 10;
    const codeCols = Math.max(...RUST.map((l) => l.length), ...PY.map((l) => l.length));
    const jsonCols = Math.max(...[OUT1, OUT2].flatMap((o) => o.map((_, i) => (jsonHead(o, i) + o[i][1] + jsonTail(o, i)).length)));
    const gutCols = 3, sepCols = 5.6;
    const need = tall ? gutCols + codeCols + 1 : gutCols + codeCols + sepCols + jsonCols;
    let fs = Math.min(tall ? 10 : Math.min(13, 11.5 * Math.max(1, S)), Math.max(7, (P.w - 2 * padX) / (need * 0.602)));
    let cw = monoW(fs);
    if (cw * need > P.w - 2 * padX) { fs *= (P.w - 2 * padX) / (cw * need); cw = monoW(fs); }
    const lh = Math.round(fs * (tall ? 1.45 : 1.55));
    const codeRows = Math.max(RUST.length, PY.length), jsonRows = OUT1.length;
    const rows = tall ? codeRows + 0.6 + jsonRows : codeRows;
    P.h = Math.round(2 * padY + rows * lh);
    P.y = Math.round(H - m - P.h);
    const gx = P.x + padX, cx = gx + gutCols * cw;
    const rowY = (i) => P.y + padY + i * lh + lh / 2;
    const jx = tall ? cx : cx + (codeCols + sepCols) * cw;
    // The result sits beside the code on wide screens, under it (after the last line) on narrow ones.
    const jRow = (i, n) => (tall ? rowY(n + 0.6 + i) : rowY(i + 1));
    const divX = cx + (codeCols + 4) * cw;

    /* ---------- The diagram: sizes ---------- */
    const bfs = Math.max(9, 10.5 * S), mfs = Math.max(9, 10 * S);
    const TS = Math.max(10.5, 9 / S); // the engine's box text, never under 9 px
    const boxPx = (str, mono) => tw(str, `500 ${mono ? mfs : bfs}px ${mono ? MONO : SANS}`) + 18 * u;
    const BH = Math.max(26, 20 / S), bh = BH * S;
    const modelName = tall ? "model" : "model provider";
    const wHandler = boxPx("POST /support", true), wT1 = boxPx("lookup_order", true), wT2 = boxPx("refund_policy", true);
    const wModel = boxPx(modelName);
    const modelX = W - m - wModel / 2 - 2, modelL = modelX - wModel / 2;
    const nameF = `600 ${Math.max(9, 10.5 * u)}px ${SANS}`, nameW = tw("Meerkat", nameF);
    const nameSize = Math.max(9, 10.5 * u);
    const boxW = Math.max(120 * u, 2 * (8 * u + nameW + 7 * u), boxPx("your code"));
    const boxH = Math.max(34, 50 * u);
    const agentR = Math.max(5, 7 * S);
    // The agent sits on the box's centre line, below its name (top left).
    const upH = boxH * 0.6, dnH = boxH * 0.4;
    const dbW = tw("database", `500 ${Math.max(9, 9.5 * S)}px ${SANS}`);

    // Beat 2's clients, in a column left of the box. Each wire is named by the surface it uses;
    // where there is no room for those names, the clients carry them.
    const wireF = `500 ${Math.max(9, 9 * S)}px ${SANS}`;
    let CLIENTS = [
      { id: "http", name: "HTTP client", wire: "REST" },
      { id: "py", name: "Python app", wire: "JSON-RPC" },
      { id: "ts", name: "TypeScript app", wire: "JSON-RPC" },
      { id: "mcp", name: "MCP client", wire: "MCP" },
    ];
    const clientGeo = (list, named) => {
      const wCl = Math.max(...list.map((c) => boxPx(c.name)));
      const clX = m + wCl / 2 + (tall ? 0 : 4 * u);
      const labW = named ? Math.max(...list.map((c) => tw(c.wire, wireF))) : 0;
      const busMin = clX + wCl / 2 + (named ? labW + 16 * u : 14 * u);
      return { wCl, clX, labW, busMin };
    };
    const busGap = 14 * u;

    /* ---------- The diagram: where things are ---------- */
    const fx0 = tall ? m : m + 24 * u;
    const fx1 = modelL - (tall ? 12 : 18) * u;
    // Where the box can sit: clear of the clients (beat 2), of the request row or the database
    // (beat 1), inside the frame and short of the model.
    const hi = Math.min(fx1 - 8 * u - boxW / 2, modelL - 22 * u - boxW / 2, tall ? Infinity : fx1 - (dbW + 18 * u) - boxW / 2);
    const lo1 = tall ? fx0 + dbW + 16 * u + boxW / 2 : fx0 + 10 * u + wHandler + 22 * u + boxW / 2;
    let named = true;
    let CG = clientGeo(CLIENTS, true);
    if (Math.max(lo1, CG.busMin + busGap + boxW / 2) > hi) {
      named = false;
      CLIENTS = [
        { id: "http", name: "REST client" }, { id: "py", name: "Python app" },
        { id: "ts", name: "TypeScript app" }, { id: "mcp", name: "MCP client" },
      ];
      CG = clientGeo(CLIENTS, false);
    }
    const lo = Math.max(lo1, CG.busMin + busGap + boxW / 2);
    const agentX = Math.max(lo, Math.min(hi, (CG.clX + CG.wCl / 2 + modelL) / 2));
    const boxL = agentX - boxW / 2, boxR = agentX + boxW / 2;
    const busX = boxL - busGap;

    // Rows. Wide: the request row (route, your code, database), the box, your functions.
    // Narrow: the route above your code, the database beside it.
    const bandT = m, bandB = P.y - (tall ? 8 : 12);
    const gapCB = 12 * u, forkGap = 20 * u, gapHC = 10 * u;
    const pitch = bh + 14 * u; // the clients' column
    const colHalf = 1.5 * pitch + bh / 2;
    // Offsets from the agent's line.
    const oy1 = -(upH + gapCB + bh / 2); // your code
    const oyH = tall ? oy1 - bh - gapHC : oy1; // the route
    const labRoom = tall ? 10 * u : 12 * u + nameSize + 6 * u; // wide: the frame's name sits above the route
    const oyFT = oyH - bh / 2 - labRoom; // frame top
    const oyStub = tall ? oyFT - 14 * u : oyH;
    const oy3 = dnH + forkGap + bh / 2; // your functions
    const oyFB = oy3 + bh / 2 + 10 * u; // frame bottom
    const above = Math.max(-(tall ? oyStub : oyFT), colHalf), below = Math.max(oyFB, colHalf);
    const agentY = bandT + above + Math.max(0, (bandB - bandT - above - below) / 2);
    const y1 = agentY + oy1, yH = agentY + oyH, y3 = agentY + oy3;
    const fyT = agentY + oyFT, fyB = agentY + oyFB;
    const handlerX = tall ? agentX : fx0 + 10 * u + wHandler / 2;
    const dbX = tall ? (fx0 + boxL) / 2 : (boxR + fx1) / 2;
    const stub = tall ? [agentX, agentY + oyStub] : [m, yH];
    const jOff = dnH + forkGap * 0.45; // the tools' fork, below the agent
    const tdx = Math.max(wT1, wT2) / 2 + 7 * u;
    const tLeft = agentX - tdx - wT1 / 2, tRight = agentX + tdx + wT2 / 2;
    const tShift = tRight > fx1 - 8 * u ? fx1 - 8 * u - tRight : tLeft < fx0 + 8 * u ? fx0 + 8 * u - tLeft : 0;
    const t1X = agentX - tdx + tShift, t2X = agentX + tdx + tShift;
    const FR = { x: (fx0 + fx1) / 2, y: (fyT + fyB) / 2, w: fx1 - fx0, h: fyB - fyT };
    const clY = (i) => agentY + (i - 1.5) * pitch;

    // The engine draws the boxes and keeps an anchor for the agent (and so the box around it).
    const box = (id, x, y, w, text, o = {}) => ({ id, kind: "box", x, y, r: 1, w: w / S, h: BH, text, textSize: TS, ...o });
    const nodes = [
      { id: "agent", kind: "anchor", x: agentX, y: agentY, r: agentR },
      box("code", agentX, y1, boxW, "your code"),
      box("model", modelX, agentY, wModel, modelName),
      box("handler", handlerX, yH, wHandler, ""),
      box("t1", t1X, y3, wT1, ""),
      box("t2", t2X, y3, wT2, ""),
      { id: "db", kind: "store", x: dbX, y: y1 - 3 * S, r: 8 * S, label: "database", always: true, labelTone: "mid", labelSize: 9.5 },
      { id: "frame", kind: "anchor", x: FR.x, y: FR.y, r: 1 },
      ...CLIENTS.map((c, i) => box(c.id, CG.clX, clY(i), CG.wCl, c.name, { hidden: true })),
    ];
    const BEAT1 = ["frame", "handler", "code", "db", "t1", "t2"];
    const OTHERS = CLIENTS.filter((c) => c.id !== "py").map((c) => c.id);

    /* ---------- The script: boxes shown, hidden and lit; dots drawn by the scene ---------- */
    const script = [];
    const on = (t, id, d) => script.push({ t, on: id, d });
    const DOTS = []; // { t, dur, path: (net) => points, c }
    const dot = (t, dur, path, c = "ink", wire = null) => { DOTS.push({ t, dur, path, c, wire }); return t + dur; };
    const AG = []; // the agent at work
    const BOX = []; // the box lit: an agent is built, a client connects
    const WIRE = { model: [], tools: [] }; // wires lit while a builder line configures them
    const CONNECT = {}; // client -> when its wire is drawn in
    const HOLD = {}; // client -> while its wire stays lit after its answer

    // Paths, from where things are now (they may still be arriving from the previous scene).
    const xy = (net, id) => { const n = net.node(id); return n ? [n.x, n.y] : [0, 0]; };
    const A = (net) => xy(net, "agent");
    const rev = (f) => (net) => f(net).slice().reverse();
    const toModel = (net) => [A(net), xy(net, "model")];
    const fork = (id) => (net) => {
      const [ax, ay] = A(net), [tx, ty] = xy(net, id);
      return [[ax, ay], [ax, ay + jOff], [tx, ay + jOff], [tx, ty]];
    };
    const client = (id) => (net) => {
      const [ax, ay] = A(net), [qx, qy] = xy(net, id);
      const bx = ax - boxW / 2 - busGap;
      return [[qx, qy], [bx, qy], [bx, ay], [ax, ay]];
    };

    // Beat 1: a request through your service.
    dot(T0, 0.45, (net) => [stub, xy(net, "handler")]);
    on(T0 + 0.42, "handler", 0.5);
    dot(T0 + 0.45, 0.4, (net) => [xy(net, "handler"), xy(net, "code")]);
    on(T0 + 0.82, "code", 0.5);
    BOX.push([RUN1[0][0] + 0.05, RUN1[0][1]]); // AgentBuilder::new()
    on(RUN1[1][0] + 0.05, "model", RUN1[1][1] - RUN1[1][0] - 0.05); // .model(...)
    WIRE.model.push([RUN1[1][0] + 0.05, RUN1[1][1]]);
    on(RUN1[3][0] + 0.05, ["t1", "t2"], RUN1[3][1] - RUN1[3][0] - 0.05); // .build(llm, tools, store)
    WIRE.tools.push([RUN1[3][0] + 0.05, RUN1[3][1]]);
    {
      const r = RUNT; // agent.run: straight down into the library, no port, no network
      dot(r, 0.35, (net) => [xy(net, "code"), A(net)]);
      AG.push([r + 0.35, r + 3.95]);
      dot(r + 0.4, 0.4, toModel, "mid"); on(r + 0.8, "model", 0.3);
      dot(r + 1.1, 0.4, rev(toModel));
      dot(r + 1.55, 0.42, fork("t1"), "mid"); dot(r + 1.61, 0.42, fork("t2"), "mid"); // one batch, two calls
      on(r + 1.97, ["t1", "t2"], 0.3);
      dot(r + 2.3, 0.42, rev(fork("t1"))); dot(r + 2.36, 0.42, rev(fork("t2")));
      dot(r + 2.85, 0.4, toModel, "mid"); on(r + 3.25, "model", 0.25);
      dot(r + 3.5, 0.4, rev(toModel));
      dot(r + 3.95, 0.35, (net) => [A(net), xy(net, "code")], "accent"); // the typed result
      on(r + 4.3, "code", 0.5);
    }
    dot(RUN1[6][0] + 0.05, 0.45, (net) => [xy(net, "code"), xy(net, "db")]); // db.save
    on(RUN1[6][0] + 0.5, "db", 0.5);
    dot(RUN1[7][0] + 0.05, 0.4, (net) => [xy(net, "code"), xy(net, "handler")], "accent"); // Ok(Json(out))
    dot(RUN1[7][0] + 0.45, 0.45, (net) => [xy(net, "handler"), stub], "accent");
    on(RUN1[7][0] + 0.42, "handler", 0.4);

    // Beat 2: the same box from outside.
    script.push({ t: SW, hide: BEAT1 }, { t: SW + 0.4, show: "py" });
    CONNECT.py = RUN2[1][0] + 0.05; // MeerkatClient(): the connection opens
    BOX.push([RUN2[1][0] + 0.2, RUN2[1][1]]);
    {
      const r = PYT;
      dot(r, 0.5, client("py"), "ink", "py");
      AG.push([r + 0.5, r + 1.75]);
      dot(r + 0.55, 0.4, toModel, "mid"); on(r + 0.95, "model", 0.3);
      dot(r + 1.3, 0.4, rev(toModel));
      dot(r + 1.75, 0.5, rev(client("py")), "accent", "py");
      on(r + 2.25, "py", 0.6);
    }
    // Then every client at once.
    script.push({ t: P0, show: OTHERS });
    OTHERS.forEach((id) => { CONNECT[id] = P0 + 0.15; });
    const ORDER = CLIENTS.map((c) => c.id);
    ORDER.forEach((id, i) => dot(P0 + 0.65 + i * 0.08, 0.5, client(id), "ink", id));
    AG.push([P0 + 1.2, P0 + 2.45]);
    dot(P0 + 1.25, 0.4, toModel, "mid"); on(P0 + 1.65, "model", 0.3);
    dot(P0 + 1.95, 0.4, rev(toModel));
    // Every answer lands, and all four stay lit for a moment: one box, every surface.
    ORDER.forEach((id, i) => {
      const land = dot(P0 + 2.4 + i * 0.08, 0.5, rev(client(id)), "accent", id);
      on(land, id, R0 - land);
      HOLD[id] = [land, R0];
    });
    // The loop ends where it starts: the clients leave first, then your service returns.
    script.push({ t: R0, hide: ORDER }, { t: R0 + 0.45, show: BEAT1 });
    const still = frozen(nodes, script, staticAt, S);
    const within = (list, t) => list.some(([a, b]) => t >= a && t < b);
    // A client's wire is named by its surface, lit while one of its dots is on it.
    const wireHot = (id, t) => (HOLD[id] && t >= HOLD[id][0] && t < HOLD[id][1]) || DOTS.some((d) => d.wire === id && t >= d.t && t < d.t + d.dur);

    /* ---------- Drawing ---------- */
    const line = (ctx, pts) => { ctx.beginPath(); ctx.moveTo(pts[0][0], pts[0][1]); for (let i = 1; i < pts.length; i++) ctx.lineTo(pts[i][0], pts[i][1]); ctx.stroke(); };
    // A polyline drawn in to a fraction of its length.
    const lineTo = (ctx, pts, f) => {
      if (f >= 1) return line(ctx, pts);
      const seg = []; let L = 0;
      for (let i = 1; i < pts.length; i++) { const d = Math.hypot(pts[i][0] - pts[i - 1][0], pts[i][1] - pts[i - 1][1]); seg.push(d); L += d; }
      let left = f * L;
      ctx.beginPath(); ctx.moveTo(pts[0][0], pts[0][1]);
      for (let i = 0; i < seg.length && left > 0; i++) {
        const k = Math.min(1, left / (seg[i] || 1));
        ctx.lineTo(pts[i][0] + (pts[i + 1][0] - pts[i][0]) * k, pts[i][1] + (pts[i + 1][1] - pts[i][1]) * k);
        left -= seg[i];
      }
      ctx.stroke();
    };
    // A dot on its way along a path, with a short trail (as the engine draws its messages).
    const drawDot = (ctx, s, pts, e, c) => {
      const seg = []; let L = 0;
      for (let i = 1; i < pts.length; i++) { const d = Math.hypot(pts[i][0] - pts[i - 1][0], pts[i][1] - pts[i - 1][1]); seg.push(d); L += d; }
      if (L < 1) return;
      const point = (d) => {
        let k = 0;
        while (k < seg.length - 1 && d > seg[k]) { d -= seg[k]; k++; }
        const f = seg[k] ? Math.max(0, Math.min(1, d / seg[k])) : 0;
        return [pts[k][0] + (pts[k + 1][0] - pts[k][0]) * f, pts[k][1] + (pts[k + 1][1] - pts[k][1]) * f];
      };
      const col = c === "accent" ? COL.accent : c === "mid" ? COL.mid : COL.ink;
      const d1 = e * L, d0 = Math.max(0, d1 - Math.max(12 * s, 0.14 * L));
      ctx.strokeStyle = rgba(col, 0.35); ctx.lineWidth = 1.5 * s;
      ctx.beginPath();
      const p0 = point(d0); ctx.moveTo(p0[0], p0[1]);
      let acc = 0;
      for (let i = 0; i < seg.length; i++) { acc += seg[i]; if (acc > d0 && acc < d1) ctx.lineTo(pts[i + 1][0], pts[i + 1][1]); }
      const p1 = point(d1); ctx.lineTo(p1[0], p1[1]); ctx.stroke();
      ctx.fillStyle = rgba(col, 1);
      ctx.beginPath(); ctx.arc(p1[0], p1[1], Math.max(2.2, 2.7 * s), 0, TAU); ctx.fill();
    };

    const decor = (ctx, net) => {
      const s = net.S, t = reduced ? staticAt : net.t;
      const ag = net.node("agent");
      if (!ag) return;
      const [ax, ay] = [ag.x, ag.y];
      const bL = ax - boxW / 2, bT = ay - upH, bB = ay + dnH;
      const alpha = (id) => { const n = net.node(id); return n ? n.a : 0; };
      ctx.save();
      ctx.lineWidth = 1;

      // Your service: a dashed frame, named at its top left.
      const fr = net.node("frame");
      if (fr && fr.a > 0.02) {
        ctx.globalAlpha = fr.a;
        ctx.strokeStyle = rgba(COL.ink, 0.28); ctx.setLineDash([3 * s, 3 * s]);
        ctx.strokeRect(Math.round(fr.x - FR.w / 2) + 0.5, Math.round(fr.y - FR.h / 2) + 0.5, Math.round(FR.w), Math.round(FR.h));
        ctx.setLineDash([]);
        text(ctx, s, "your service", fr.x - FR.w / 2 + 9 * u, fr.y - FR.h / 2 + 8 * u, { size: 9.5, c: COL.ink, a: 0.75, base: "top", halo: true });
      }

      // Wiring.
      const wire = (a, pts, hot) => {
        if (a < 0.03) return;
        ctx.globalAlpha = a;
        ctx.strokeStyle = hot ? rgba(COL.accent, 0.9) : rgba(COL.ink, 0.2);
        ctx.lineWidth = hot ? Math.max(1.2, 1.5 * s) : 1;
        line(ctx, pts);
        ctx.lineWidth = 1;
      };
      const hA = alpha("handler"), cA = alpha("code");
      wire(Math.min(hA, alpha("frame")), [stub, xy(net, "handler")]);
      wire(Math.min(hA, cA), [xy(net, "handler"), xy(net, "code")]);
      wire(Math.min(cA, alpha("db")), [xy(net, "code"), xy(net, "db")]);
      wire(Math.min(cA, ag.a), [xy(net, "code"), [ax, bT]]);
      wire(Math.min(ag.a, alpha("model")), [[ax + boxW / 2, ay], xy(net, "model")], within(WIRE.model, t));
      ["t1", "t2"].forEach((id) => wire(Math.min(ag.a, alpha(id)), fork(id)(net).slice(1), within(WIRE.tools, t)));
      // Clients: each wire is drawn in as it connects, and named by its surface.
      CLIENTS.forEach((c) => {
        const a = Math.min(ag.a, alpha(c.id));
        if (a < 0.03) return;
        const f = reduced ? 1 : easeIO((t - (CONNECT[c.id] ?? 0)) / 0.45);
        const pts = client(c.id)(net);
        pts[pts.length - 1] = [bL, ay];
        const hot = wireHot(c.id, t) || (f > 0 && f < 1);
        ctx.globalAlpha = a;
        ctx.strokeStyle = hot ? rgba(COL.accent, 0.9) : rgba(COL.ink, 0.2);
        ctx.lineWidth = hot ? Math.max(1.2, 1.5 * s) : 1;
        lineTo(ctx, pts, f);
        ctx.lineWidth = 1;
        if (named && c.wire && f > 0.3) {
          ctx.globalAlpha = a * Math.min(1, (f - 0.3) / 0.4);
          text(ctx, s, c.wire, pts[0][0] + CG.wCl / 2 + 8 * u, pts[0][1] - 4 * u, { size: 9, c: hot ? COL.accent : COL.mid, base: "bottom", halo: true });
        }
      });

      // Meerkat: a box with its name and the agent. The agent is drawn over the dots (overlay).
      ctx.globalAlpha = ag.a;
      const lit = !reduced && within(BOX, t);
      ctx.fillStyle = rgba(COL.screen, 1);
      ctx.strokeStyle = lit ? rgba(COL.accent, 1) : rgba(COL.ink, 0.6);
      ctx.lineWidth = lit ? Math.max(1.2, 1.5 * s) : 1;
      ctx.beginPath(); roundRect(ctx, Math.round(bL) + 0.5, Math.round(bT) + 0.5, Math.round(boxW), Math.round(boxH), 5 * u); ctx.fill(); ctx.stroke();
      ctx.lineWidth = 1;
      text(ctx, s, "Meerkat", bL + 8 * u, bT + 7 * u, { size: 10.5 * u / s, min: 9, w: 600, c: lit ? COL.accent : COL.ink, a: 0.9, base: "top" });
      // Inside the box, the wires reach the agent.
      ctx.strokeStyle = rgba(COL.ink, 0.2);
      const inner = (a, x0, y0, hot) => {
        if (a < 0.03) return;
        ctx.globalAlpha = a;
        ctx.strokeStyle = hot ? rgba(COL.accent, 0.9) : rgba(COL.ink, 0.2);
        ctx.lineWidth = hot ? Math.max(1.2, 1.5 * s) : 1;
        ctx.beginPath(); ctx.moveTo(x0, y0); ctx.lineTo(ax, ay); ctx.stroke();
        ctx.lineWidth = 1;
      };
      inner(Math.min(ag.a, cA), ax, bT);
      inner(Math.min(ag.a, alpha("model")), ax + boxW / 2, ay, within(WIRE.model, t));
      inner(Math.min(ag.a, alpha("t1")), ax, bB, within(WIRE.tools, t));
      inner(Math.min(ag.a, Math.max(...CLIENTS.map((c) => (reduced || t >= (CONNECT[c.id] ?? 0) + 0.4 ? alpha(c.id) : 0)))), bL, ay, CLIENTS.some((c) => wireHot(c.id, t)));

      // Dots, under the boxes they enter.
      ctx.globalAlpha = 1;
      DOTS.forEach((d) => {
        if (t < d.t || t >= d.t + d.dur) return;
        drawDot(ctx, s, d.path(net), easeIO((t - d.t) / d.dur), d.c);
      });
      ctx.restore();
    };

    // The agent itself, over the dots that reach it (as the engine draws its agents).
    const drawAgent = (ctx, net, t) => {
      const ag = net.node("agent");
      if (!ag || ag.a < 0.02) return;
      const s = net.S, active = within(AG, t);
      ctx.globalAlpha = ag.a;
      if (active) {
        ctx.strokeStyle = rgba(COL.accent, 0.35); ctx.lineWidth = 1;
        ctx.beginPath(); ctx.arc(ag.x, ag.y, agentR + 4 * s + (reduced ? 1 : 2 * s * Math.sin(net.t * 6)), 0, TAU); ctx.stroke();
      }
      ctx.fillStyle = rgba(active ? COL.accent : COL.ink, 1);
      ctx.beginPath(); ctx.arc(ag.x, ag.y, agentR, 0, TAU); ctx.fill();
      ctx.globalAlpha = 1;
    };

    // Code, drawn token by token: keywords in weight, strings and comments in grey.
    const KW = /^(let|mut|async|with|as|await|from|import)$/;
    const drawCode = (ctx, str, x, y, a) => {
      const cm = str.match(/^(\s*)(\/\/.*|#.*)$/);
      if (cm) {
        ctx.font = `italic 400 ${fs}px ${MONO}`; ctx.fillStyle = rgba(COL.mid, a);
        ctx.fillText(str, x, y);
        return x + cw * str.length;
      }
      let xx = x;
      str.split(/("[^"]*"|\b[a-z]+\b)/).forEach((tok) => {
        if (!tok) return;
        const quoted = tok[0] === '"', kw = KW.test(tok);
        ctx.font = `${kw ? 600 : 500} ${fs}px ${MONO}`;
        ctx.fillStyle = quoted ? rgba(COL.mid, Math.min(1, a + 0.08)) : rgba(COL.ink, a);
        ctx.fillText(tok, xx, y);
        xx += cw * tok.length;
      });
      return xx;
    };
    const PANEL = [241, 240, 235];
    let lastT = -1, looped = false;

    const overlay = (ctx, net) => {
      const s = net.S, t = reduced ? staticAt : net.t;
      if (t < lastT - 1) looped = true; // after the first loop the code is already there at t = 0
      lastT = t;
      ctx.save();
      // Names set in monospace: the route and your functions.
      [["handler", "POST /support"], ["t1", "lookup_order"], ["t2", "refund_policy"]].forEach(([id, str]) => {
        const n = net.node(id);
        if (!n || n.a < 0.05) return;
        const hot = n.active > net.t || n.flash > net.t;
        ctx.globalAlpha = n.a;
        ctx.font = `500 ${mfs}px ${MONO}`; ctx.fillStyle = rgba(hot ? COL.accent : COL.ink, hot ? 1 : 0.85);
        ctx.textAlign = "center"; ctx.textBaseline = "middle";
        ctx.fillText(str, n.x, n.y + 0.5);
      });
      ctx.globalAlpha = 1;
      drawAgent(ctx, net, t);
      still(ctx, net);

      // The code panel.
      const ag = net.node("agent");
      ctx.globalAlpha = ag ? Math.min(1, ag.a) : 1;
      const rad = 6;
      ctx.fillStyle = rgba(PANEL, 1);
      ctx.beginPath(); roundRect(ctx, P.x + 0.5, P.y + 0.5, P.w - 1, P.h - 1, rad); ctx.fill();
      ctx.strokeStyle = rgba(COL.ink, 0.1); ctx.lineWidth = 1; ctx.stroke();
      ctx.save();
      ctx.beginPath(); roundRect(ctx, P.x + 1, P.y + 1, P.w - 2, P.h - 2, rad); ctx.clip();
      const baseA = ctx.globalAlpha;
      // Rust until the switch, and again over the last moment (where the loop starts over);
      // Python in between.
      const endRust = t >= DUR - 0.45;
      const pages = [];
      if (reduced) pages.push([false, 1, t]);
      else {
        if (t < SW) pages.push([false, Math.min(looped ? 1 : t / 0.3, (SW - t) / 0.2, 1), t]);
        if (t >= SW && t < R0 + 0.4) pages.push([true, Math.min(1, Math.max(0, (t - SW - 0.15) / 0.3), (R0 + 0.4 - t) / 0.4), t]);
        if (endRust) pages.push([false, Math.min(1, (t - (DUR - 0.45)) / 0.4), 0]);
      }
      pages.forEach(([beat2, fade, tt]) => {
        if (fade <= 0) return;
        ctx.globalAlpha = baseA * Math.max(0, Math.min(1, fade));
        const code = beat2 ? PY : RUST, run = beat2 ? RUN2 : RUN1, wait = beat2 ? WAIT2 : WAIT1;
        const OUT = beat2 ? OUT2 : OUT1, got = beat2 ? GOT2 : GOT1;
        // The language, in the corner.
        text(ctx, s, beat2 ? "Python" : "Rust", P.x + P.w - padX, tall ? rowY(0) : rowY(codeRows - 1), { size: 9, c: COL.mid, align: "right", base: "middle" });
        code.forEach((str, i) => {
          const y = rowY(i);
          const r = run[i];
          const now = r && tt >= r[0] && tt < r[1];
          const done = r && tt >= r[1];
          if (now) {
            ctx.fillStyle = rgba(COL.ink, 0.05); ctx.fillRect(P.x + 1, y - lh / 2, (tall ? P.w : divX - P.x) - 2, lh);
            ctx.fillStyle = rgba(COL.accent, 1); ctx.fillRect(P.x + 1, y - lh / 2, 2.5, lh);
          }
          ctx.textAlign = "right"; ctx.textBaseline = "middle";
          ctx.font = `400 ${fs * 0.82}px ${MONO}`; ctx.fillStyle = rgba(COL.ink, now ? 0.55 : 0.25);
          ctx.fillText(String(i + 1), cx - cw * 1.2, y + 0.5);
          ctx.textAlign = "left";
          const end = drawCode(ctx, str, cx, y + 0.5, now || done ? 0.92 : 0.45);
          // A line that waits on the agent shows it.
          if (i === wait[0] && tt >= wait[1] && tt < wait[2]) {
            for (let d = 0; d < 3; d++) {
              const on3 = reduced ? d < 2 : Math.floor((tt - wait[1]) * 3) % 4 > d;
              ctx.fillStyle = rgba(COL.accent, on3 ? 1 : 0.25);
              ctx.beginPath(); ctx.arc(end + cw * (0.9 + d * 1.1), y + 0.5, Math.max(1.4, 1.7 * s), 0, TAU); ctx.fill();
            }
          }
        });
        // The typed result: the schema's shape once output_schema has run, the values as they land.
        const shapeAt = beat2 ? RUN2[2][0] : RUN1[2][0];
        const shape = reduced ? 1 : Math.max(0, Math.min(1, (tt - shapeAt - 0.05) / 0.3));
        if (shape <= 0) return;
        const pa = ctx.globalAlpha;
        ctx.globalAlpha = pa * shape;
        if (!tall) {
          ctx.strokeStyle = rgba(COL.ink, 0.1); ctx.beginPath(); ctx.moveTo(divX + 0.5, P.y + padY); ctx.lineTo(divX + 0.5, P.y + P.h - padY); ctx.stroke();
          text(ctx, s, "typed result", jx, rowY(0) + 0.5, { size: 9, c: COL.mid, base: "middle" });
        } else {
          ctx.strokeStyle = rgba(COL.ink, 0.1); ctx.beginPath(); const yy = rowY(code.length - 0.5) + lh * 0.3; ctx.moveTo(cx, yy + 0.5); ctx.lineTo(P.x + P.w - padX, yy + 0.5); ctx.stroke();
        }
        OUT.forEach((kv, i) => {
          const y = jRow(i, code.length) + 0.5;
          const head = jsonHead(OUT, i), val = kv[1], tail = jsonTail(OUT, i);
          const p = reduced ? (tt >= got ? 1 : 0) : Math.max(0, Math.min(1, (tt - got - i * 0.16) / 0.3));
          ctx.textAlign = "left"; ctx.textBaseline = "middle";
          ctx.font = `500 ${fs}px ${MONO}`;
          ctx.fillStyle = rgba(COL.ink, p > 0 ? 0.9 : 0.4);
          ctx.fillText(head, jx, y);
          const vx = jx + cw * head.length;
          if (p <= 0) {
            // Not there yet: a blank the width of the value.
            ctx.fillStyle = rgba(COL.ink, 0.14);
            ctx.fillRect(vx + 1, y + fs * 0.42, cw * val.length - 2, 1);
            ctx.fillStyle = rgba(COL.ink, 0.4);
            ctx.fillText(tail, vx + cw * val.length, y);
          } else {
            const n = Math.ceil(val.length * p);
            ctx.fillStyle = rgba(COL.ink, 0.95);
            ctx.fillText(val.slice(0, n), vx, y);
            if (p < 1) { ctx.fillStyle = rgba(COL.accent, 0.9); ctx.fillRect(vx + cw * n + 1, y - lh * 0.34, cw * 0.6, lh * 0.68); }
            else { ctx.fillStyle = rgba(COL.ink, 0.9); ctx.fillText(tail, vx + cw * val.length, y); }
          }
        });
        ctx.globalAlpha = pa;
      });
      ctx.restore();
      ctx.restore();
    };
    return { nodes, script, duration: DUR, staticAt, decor, overlay, origin: [agentX, agentY] };
  }

  /* ------------------------------------------------------------------ */
  /* An organisation at true scale.                                        */
  /* Development: repositories with long-lived domain agents that consult  */
  /* each other across repositories; people work through their personal   */
  /* agents, and progress reaches the initiative agents.                   */
  /* Initiatives and analysis: the review agent's hub with about 100       */
  /* initiative agents and the personal and channel agents that follow     */
  /* them; the investigation and delivery-analysis agents, which read and  */
  /* write through the initiative agents too; a reporting agent. Workers   */
  /* are spawned in waves and wired only to their parent. Between cycles   */
  /* the network murmurs on (people ask their agents, analysis agents      */
  /* check records, domain agents consult), so 50 to 100 agents are live   */
  /* at any moment, about 700 at peak. The meter counts what is drawn.     */
  /* Everything here is drawn in batches by the scene itself, so several   */
  /* hundred agents and messages stay smooth; the engine keeps only        */
  /* anchors (the zoom starts from "inv").                                 */
  /* ------------------------------------------------------------------ */
  function organisation(W, H, S, opts = {}) {
    const labels = !!opts.labels;
    const K = labels ? 1 : 0.66; // the unlabelled hero plays the same night faster
    const wide = W / H >= 1.12;
    const X = (f) => f * W, Y = (f) => f * H;
    const R = rng(23);
    const pick = (arr) => arr[Math.floor(R() * arr.length)];
    const start = performance.now();

    /* ---------- Layout ---------- */
    let G;
    if (wide) {
      const Rh = Math.min(Y(0.29), X(0.172)), Rd = Math.min(Y(0.2), X(0.118)), Rf = Math.min(Y(0.052), X(0.032));
      G = {
        dev: [X(0.025), Y(0.155), X(0.285), Y(0.78)], dcols: 3, drows: 6, pcols: 3, prows: 12, split: 0.6, more: true,
        hub: [X(0.49), Y(0.49), Rh], dan: [X(0.815), Y(0.5), Rd], fx: [X(0.352), Y(0.765), Rf],
        meter: [X(0.64), Y(0.04), X(0.975), Y(0.17)],
        sched: [X(0.405), Y(0.115)], chat: [X(0.575), Y(0.115)],
        summAng: -2.42, devArc: Math.PI, admin: [X(0.64), Y(0.285)], op: [X(0.64), Y(0.215)],
        board: [X(0.39), Y(0.925)], plans: [X(0.49), Y(0.925)], wh: [X(0.59), Y(0.925)],
        search: [X(0.66), Y(0.8), X(0.975), Y(0.975)],
        hdr: [[X(0.025), Y(0.07), "DEVELOPMENT"], [X(0.315), Y(0.07), "INITIATIVES AND ANALYSIS"]],
      };
    } else {
      const Rh = Math.min(X(0.335), Y(0.225)), Rd = Math.min(X(0.2), Y(0.135)), Rf = Math.min(X(0.055), Y(0.042));
      G = {
        dev: [X(0.56), Y(0.615), X(0.97), Y(0.84)], dcols: 3, drows: 2, pcols: 3, prows: 4, split: 0.58, more: false,
        hub: [X(0.5), Y(0.36), Rh], dan: [X(0.28), Y(0.72), Rd], fx: [X(0.1), Y(0.215), Rf],
        meter: [X(0.04), Y(0.02), X(0.96), Y(0.108)],
        sched: [X(0.9), Y(0.19)], chat: [X(0.9), Y(0.33)],
        summAng: 2.55, devArc: 0.75, admin: [X(0.9), Y(0.5)], op: [X(0.9), Y(0.44)],
        board: [X(0.6), Y(0.935)], plans: [X(0.745), Y(0.935)], wh: [X(0.89), Y(0.935)],
        search: [X(0.03), Y(0.875), X(0.5), Y(0.985)],
        hdr: [[X(0.56), Y(0.6), "DEVELOPMENT"]],
      };
    }
    const narrow = W < 560;

    /* ---------- Agents and connectors ---------- */
    const A = [];
    const agent = (o) => { o.i = A.length; A.push(o); return o.i; };
    const CN = {};
    const conn = (key, [x, y], label, short, kind = "src") => { CN[key] = { key, x, y, label: narrow && short ? short : label, kind, hot: false }; };
    const P = (e) => (typeof e === "number" ? A[e] : typeof e === "string" ? CN[e] : e);

    // Development: repositories of different sizes, each with three to eight long-lived
    // domain-expert agents (a lead in the middle of the larger ones).
    const [dx0, dy0, dx1, dy1] = G.dev;
    const rX1 = dx0 + (dx1 - dx0) * G.split;
    const cw = (rX1 - dx0) / G.dcols, chh = (dy1 - dy0) / G.drows;
    const fw = Math.min(cw, chh) * 0.8;
    const SIZES = [5, 3, 7, 4, 8, 3, 6, 4, 5, 3, 7, 4, 6, 5, 3, 8, 4, 5];
    const devHold = G.more ? 3.5 : 5.5; // the portrait layout draws a third of the repositories
    const repos = [];
    for (let r = 0; r < G.dcols * G.drows; r++) {
      const col = r % G.dcols, row = Math.floor(r / G.dcols);
      const cx = dx0 + cw * (col + 0.5), cy = dy0 + chh * (row + 0.5);
      const n = SIZES[r % SIZES.length];
      const f = fw * (0.62 + (0.38 * (n - 3)) / 5);
      const ids = [], lead = n >= 6;
      if (lead) ids.push(agent({ x: cx, y: cy, r: 1.7 * S, k: "long", sys: "dev", hold: devHold }));
      const ringN = lead ? n - 1 : n, rad = f * (lead ? 0.32 : 0.27);
      for (let k = 0; k < ringN; k++) {
        const a = -Math.PI / 2 + (k / ringN) * TAU + r * 0.5;
        ids.push(agent({ x: cx + Math.cos(a) * rad, y: cy + Math.sin(a) * rad, r: 1.6 * S, k: "long", sys: "dev", hold: devHold }));
      }
      repos.push({ cx, cy, ids, col, row, f, lead });
    }
    // Developers, each with their own agent.
    const devs = [];
    const px0 = rX1 + (dx1 - rX1) * 0.1;
    const pw = (dx1 - px0) / G.pcols, ph = (dy1 - dy0) / G.prows;
    for (let j = 0; j < G.pcols * G.prows; j++) {
      const col = j % G.pcols, row = Math.floor(j / G.pcols);
      const x = px0 + pw * (col + 0.5), y = dy0 + ph * (row + 0.5);
      const h = agent({ x, y: y - Math.min(ph * 0.2, 5 * S), r: 1.5 * S, k: "pers", sys: "dev" });
      const p = agent({ x, y: y + Math.min(ph * 0.18, 4.5 * S), r: 1.5 * S, k: "mid", sys: "dev", hold: devHold });
      devs.push({ h, p, row, col });
    }

    // Initiatives and analysis: the review agent's hub. Rings from the centre out: review workers,
    // the WorkGraph band (one item per initiative), initiative agents, subscribers.
    const [hx, hy, Rh] = G.hub;
    const on = (cx, cy, a, rad) => ({ x: cx + Math.cos(a) * rad, y: cy + Math.sin(a) * rad });
    const coord = agent({ x: hx, y: hy, r: 3.8 * S, k: "long", sys: "rev", ring: true, hold: 9 });
    const NI = 100, step = TAU / NI;
    const ang = (i) => -Math.PI / 2 + (i + 0.5) * step;
    const idxAt = (a) => ((Math.round((a + Math.PI / 2) / step - 0.5) % NI) + NI) % NI;
    const RI = 0.535, RN = 0.67; // band and initiative ring, as a share of the hub radius
    const init = [], items = [];
    for (let i = 0; i < NI; i++) {
      init.push(agent({ ...on(hx, hy, ang(i), Rh * RN), r: 1.45 * S, k: "long", sys: "rev", hold: 5 }));
      items.push({ ...on(hx, hy, ang(i), Rh * RI), seed: 1e9, claim: 1e9, done: 1e9 });
    }
    const itemSide = Math.max(2, Math.min(5.6 * S, Rh * RI * step * 0.8));

    // Subscribers: about 60 personal agents and a few channel agents. Most initiatives
    // have none to three; a few leaders follow many.
    const NP = 60, NC = 7;
    const follows = [];
    for (let p = 0; p < NP + NC; p++) {
      const chan = p >= NP, lead = p < 2;
      const base = chan ? idxAt(G.summAng + (R() - 0.5) * 0.5) : Math.floor(R() * NI);
      const n = lead ? 7 : chan ? 3 + Math.floor(R() * 3) : R() < 0.55 ? 1 : R() < 0.8 ? 2 : 3;
      const spread = lead ? 26 : chan ? 10 : 14;
      const list = [base];
      while (list.length < n) { const v = (base + Math.round((R() - 0.5) * spread) + NI) % NI; if (!list.includes(v)) list.push(v); }
      follows.push(list);
    }
    const meanAng = (list) => { let sx = 0, sy = 0; list.forEach((i) => { sx += Math.cos(ang(i)); sy += Math.sin(ang(i)); }); return Math.atan2(sy, sx); };
    // Subscribers sit evenly on an outer ring, in the order of the initiatives they follow.
    const NS = follows.length;
    const sa = follows.map((l, p) => ({ p, a: meanAng(l) })).sort((u, v) => u.a - v.a);
    { let sx = 0, sy = 0; sa.forEach((o, k) => { const d = o.a - (k * TAU) / NS; sx += Math.cos(d); sy += Math.sin(d); }); const off = Math.atan2(sy, sx); sa.forEach((o, k) => { o.a = off + (k * TAU) / NS; }); }
    const subs = [];
    sa.forEach((o) => { subs[o.p] = agent({ ...on(hx, hy, o.a, Rh * 0.9), r: 1.5 * S, k: o.p >= NP ? "long" : "mid", sys: "rev", hold: 6 }); });
    const subsOf = Array.from({ length: NI }, () => []);
    follows.forEach((l, p) => l.forEach((i) => subsOf[i].push(subs[p])));
    const chans = subs.slice(NP);

    // Singletons.
    const summ = agent({ ...on(hx, hy, G.summAng, Rh * 1.14), r: 2.5 * S, k: "long", sys: "rev", hold: 9 });
    const admin = agent({ x: G.admin[0], y: G.admin[1], r: 2.5 * S, k: "long", sys: "rev", hold: 9 });
    const op = agent({ x: G.op[0], y: G.op[1], r: 1.8 * S, k: "pers", sys: "ext" });
    const [ix, iy, Rd] = G.dan;
    const inv = agent({ x: ix, y: iy, r: 3.6 * S, k: "long", sys: "rev", ring: true, hold: 9 });
    const [fxx, fxy, Rf] = G.fx;
    const fx = agent({ x: fxx, y: fxy, r: 3 * S, k: "long", sys: "rev", ring: true, hold: 9 });

    // Connected systems. Enterprise search: one row, each label under its icon, so calls
    // arrive from above and never cross a label.
    conn("sched", G.sched, "schedule");
    conn("chat", G.chat, "chat platform", "chat");
    conn("board", G.board, "status board", "board");
    conn("plans", G.plans, "plan documents", "plans");
    conn("wh", G.wh, "reports store", "store", "store");
    const [sx0, sy0, sx1, sy1] = G.search;
    const SUB = [["issues", "issue tracker", "tickets"], ["git", "source control", "code"], ["wiki", "wiki", "wiki"], ["devdata", "developer data", "dev data"]];
    SUB.forEach(([key, label, short], n) => {
      conn(key, [sx0 + (sx1 - sx0) * (0.125 + n * 0.25), sy0 + (sy1 - sy0) * (wide ? 0.52 : 0.5)], label, short);
      CN[key].sub = true;
    });
    // Half the size of each connector's icon: calls stop at its edge.
    const iconR = (key) => (CN[key].kind === "store" ? 6.2 : CN[key].sub ? 3.4 : 3.8) * S + 1;

    /* ---------- Wiring ---------- */
    const E = { intra: [], cross: [], devRepo: [], personal: [], devInit: [], spokes: [], subs: [], core: [], invInit: [], fxInit: [] };
    repos.forEach((rp) => {
      const ids = rp.ids;
      if (rp.lead) { // the lead talks to everyone; the others to their neighbours
        for (let a = 1; a < ids.length; a++) { E.intra.push([ids[0], ids[a]]); E.intra.push([ids[a], ids[a + 1 < ids.length ? a + 1 : 1]]); }
      } else for (let a = 0; a < ids.length; a++) for (let b = a + 1; b < ids.length; b++) E.intra.push([ids[a], ids[b]]);
    });
    repos.forEach((rp, r) => {
      const nb = [];
      if (rp.col < G.dcols - 1) nb.push(r + 1);
      if (rp.row < G.drows - 1) nb.push(r + G.dcols);
      nb.forEach((q) => { if (R() < 0.75) E.cross.push([pick(rp.ids), pick(repos[q].ids)]); });
      if (R() < 0.35) { const q = Math.floor(R() * repos.length); if (q !== r) E.cross.push([pick(rp.ids), pick(repos[q].ids)]); }
    });
    const repoNear = (row) => { const rr = Math.min(G.drows - 1, Math.floor(((row + R()) * G.drows) / G.prows)); return repos[rr * G.dcols + Math.floor(R() * G.dcols)]; };
    const reach = new Map(); // a developer's agent -> the domain agents it consults
    devs.forEach((d) => {
      E.personal.push([d.h, d.p]);
      const list = [pick(repoNear(d.row).ids)];
      if (R() < 0.45) list.push(pick(repoNear(d.row).ids));
      list.forEach((b) => E.devRepo.push([d.p, b]));
      reach.set(d.p, list);
    });
    const crossOf = new Map();
    E.cross.forEach(([a, b]) => { (crossOf.get(a) || crossOf.set(a, []).get(a)).push(b); (crossOf.get(b) || crossOf.set(b, []).get(b)).push(a); });
    const arc0 = idxAt(G.devArc);
    devs.forEach((d, j) => { if (j % 3 === 1) E.devInit.push([d.p, init[(arc0 + Math.round((R() - 0.5) * 16) + NI) % NI]]); });
    init.forEach((id) => E.spokes.push([coord, id]));
    follows.forEach((l, p) => l.forEach((i) => E.subs.push([init[i], subs[p]])));
    E.core.push([summ, coord], [inv, coord]);
    chans.forEach((c) => E.core.push([summ, c]));
    // The investigation and delivery-analysis agents read and write through every initiative agent.
    init.forEach((id) => { E.invInit.push([inv, id]); E.fxInit.push([fx, id]); });

    /* ---------- The night ---------- */
    const M = [];
    // A message; bend > 0 draws it as an arc (a consult across repositories).
    const send = (t, a, b, c = "a", d, bend = 0) => {
      const u = P(a), v = P(b);
      const len = Math.hypot(v.x - u.x, v.y - u.y);
      const dur = d ?? Math.max(0.3, Math.min(1.25, (len * (bend ? 1.2 : 1)) / (300 * S)));
      M.push({ t, d: dur, a, b, c, bend });
      return t + dur;
    };
    const call = (t, a, key) => { const land = send(t, a, key, "m"); send(land + 0.12, key, a, "m"); };
    const DUR = 52;
    const cap = [];
    const caption = (t, key, title, body, hero = true) => cap.push({ t, key, title, body, hero });

    // Development never stops. A developer asks their own agent, which consults the domain
    // agents of the repositories involved; a domain agent may consult another repository's
    // (drawn as an arc) or read source control and the issue tracker; answers come back the
    // same way. Now and then a developer's agent reports progress to an initiative's agent.
    for (let t = 0.1; t < DUR - 1.2; t += 0.12 + R() * 0.17) {
      const r = R();
      if (r < 0.38) {
        const d = pick(devs), b = pick(reach.get(d.p));
        const l1 = send(t, d.h, d.p, "i");
        let back = send(l1 + 0.08, d.p, b) + 0.25 + R() * 0.5;
        const far = crossOf.get(b);
        if (far && R() < 0.4) { const c = pick(far); back = send(send(back, b, c, "a", undefined, 1), c, b, "i", undefined, 1) + 0.15; }
        else if (R() < 0.3) { call(back, b, R() < 0.6 ? "git" : "issues"); back += 0.9; }
        send(send(back, b, d.p, "i") + 0.08, d.p, d.h, "i");
      } else if (r < 0.58) { const [a, b] = pick(E.intra); send(send(t, a, b), b, a, "i"); }
      else if (r < 0.74) { const [a, b] = pick(E.cross); const [u, v] = R() < 0.5 ? [a, b] : [b, a]; send(send(t, u, v, "a", undefined, 1), v, u, "i", undefined, 1); }
      else if (r < 0.9) call(t, pick(pick(repos).ids), R() < 0.6 ? "git" : "issues");
      else { const [a, b] = pick(E.devInit); send(t, a, b); }
    }

    // The murmur: between cycles the network never stops. People ask their own agents, which
    // ask the initiative agents they follow (reading the saved record first, now and then);
    // team channels ask their channel agents; the investigation and delivery-analysis agents
    // check initiative records through the initiative agents; initiative agents refresh from
    // their sources and pass updates to their followers; an operator asks admin.
    const ask = (t, p, i) => {
      let tt = send(t, "chat", p, "i");
      tt = send(tt + 0.15, p, init[i]);
      if (R() < 0.45) { call(tt + 0.1, init[i], "wh"); tt += 0.95; }
      tt = send(tt + 0.25, init[i], p, "i");
      send(tt + 0.15, p, "chat", "m");
    };
    const check = (t, who, i) => {
      const land = send(t, who, init[i]);
      if (R() < 0.5) { call(land + 0.1, init[i], "wh"); send(land + 1.15, init[i], who, "i"); }
      else send(land + 0.3, init[i], who, "i");
    };
    // The portrait layout draws fewer repositories, so its review side murmurs a little more.
    const hum = wide ? 1 : 0.8;
    for (let t = 0.15; t < DUR - 0.8; t += (0.25 + R() * 0.28) * hum) {
      const r = R();
      if (r < 0.44) { const q = Math.floor(R() * NP); ask(t, subs[q], pick(follows[q])); }
      else if (r < 0.54) { const q = NP + Math.floor(R() * NC); ask(t, subs[q], pick(follows[q])); }
      else if (r < 0.69) check(t, inv, Math.floor(R() * NI));
      else if (r < 0.8) check(t, fx, Math.floor(R() * NI));
      else if (r < 0.93) call(t, init[Math.floor(R() * NI)], pick(["board", "plans", "wh"]));
      else {
        const i = Math.floor(R() * NI);
        if (subsOf[i].length) { const p = pick(subsOf[i]); send(send(t, init[i], p) + 0.4, p, "chat", "m"); }
      }
    }
    [[1.4, 3.0], [30.4, 32.3], [45.2, 46.9]].forEach(([a, b]) => { send(a, op, admin, "i"); send(b, admin, op, "i"); });

    // Nightly review cycle: one WorkGraph item per initiative, workers spawned five at a time.
    const CYCLE = 0.5;
    send(CYCLE, "sched", coord);
    const reviewWorker = (i, ts) => {
      const w = agent({ ...on(hx, hy, ang(i) + step / 2, Rh * (i % 2 ? 0.38 : 0.28)), r: 1.3 * S, k: "work", sys: "rev", parent: coord, born: ts, dies: 0 });
      send(ts, coord, w, "a", 0.42);
      const it = items[i];
      it.claim = send(ts + 0.5, w, it);
      call(ts + 1.0, w, "board"); call(ts + 1.12, w, "plans");
      call(ts + 2.7, w, "wh"); // save_review_result, bound to the fetched snapshot and plan
      it.done = send(ts + 3.6, w, it, "i"); // attaches the saved id as evidence, closes the item
      const done = send(ts + 4.1, w, coord); // one line: done, with the saved id
      A[w].dies = done + 13 + R() * 3;
      return done;
    };
    for (let b = 0; b < 4; b++) {
      const tc = CYCLE + 0.7 + b * 1.25;
      call(tc - 0.3, coord, "wh"); // claims durable report runs, 25 at a time
      for (let q = 0; q < 25; q++) items[b * 25 + q].seed = tc + 0.5 + q * 0.03;
      let last = 0;
      for (let k = 0; k < 5; k++) for (let m = 0; m < 5; m++) last = Math.max(last, reviewWorker(b * 25 + k * 5 + m, tc + 1.0 + k * 0.32 + m * 0.06));
      // Reconcile from the WorkGraph, then read each saved review and forward it.
      const tr = last + 0.4;
      for (let q = 0; q < 25; q++) send(tr + q * 0.03, items[b * 25 + q], coord, "i", 0.5);
      for (let q = 0; q < 25; q++) {
        const i = b * 25 + q, tq = tr + 0.9 + q * 0.07;
        call(tq, coord, "wh");
        const tf = tq + 1.1;
        send(tf, coord, init[i]);
        subsOf[i].forEach((p, m) => { const land = send(tf + 0.7 + m * 0.1, init[i], p); send(land + 0.6, p, "chat", "m"); });
      }
    }

    // Investigation: workers in waves, each spawned for one initiative, whose agent is asked
    // for its record; each worker spawns one helper per person. Findings go back through the
    // initiative's agent to the people who follow it. Between cycles, single investigations
    // run on request in the slots the waves have left.
    const INV0 = 1.4, NIW = 64;
    const invFree = new Array(NIW).fill(0);
    const invWorker = (j, ts, small = false) => {
      const ii = Math.floor((j * NI) / NIW);
      const a = -Math.PI / 2 + ((j + 0.5) * TAU) / NIW, da = TAU / NIW;
      const w = agent({ ...on(ix, iy, a, Rd * (j % 2 ? 0.35 : 0.26)), r: 1.3 * S, k: "work", sys: "rev", parent: inv, born: ts, dies: 0 });
      send(ts, inv, w, "a", 0.38);
      send(send(ts + 0.12, inv, init[ii]) + 0.2, init[ii], inv, "i"); // the initiative's record
      call(ts + 0.6, w, "wh"); call(ts + 0.7, w, "plans"); // initiative history, plan document
      call(ts + 1.5, w, "issues"); call(ts + 1.6, w, "wiki"); // enterprise search
      // Helpers keep the wave's layout, so a single investigation reuses its slots.
      const n = 3 + ((j * 7) % 4) + (j % 5 === 0 ? 1 : 0), nh = small ? 2 : n;
      let lastRep = 0, end = 0;
      for (let k = 0; k < nh; k++) {
        const tsw = ts + 2.4 + k * 0.1;
        const pos = on(ix, iy, a + (k % 2 ? 0.2 : -0.2) * da, Rd * (0.5 + (0.48 * (k + 0.5)) / n));
        const sw = agent({ ...pos, r: 1.15 * S, k: "work", sys: "rev", parent: w, born: tsw, dies: 0 });
        send(tsw, w, sw, "a", 0.3);
        call(tsw + 0.5, sw, "wh"); // the shared person-activity ledger
        const L = small ? 3.6 + R() * 1.0 : 17 + R() * 3.5;
        for (let tt = tsw + 1.8 + R(); tt < tsw + L - 2; tt += 3.6 + R() * 2.2) call(tt, sw, pick(["devdata", "devdata", "issues", "git"]));
        const rep = send(tsw + L, sw, w);
        A[sw].dies = tsw + L + 0.7;
        lastRep = Math.max(lastRep, rep);
        end = Math.max(end, A[sw].dies);
      }
      for (let tt = ts + 3.9 + R(); tt < lastRep - 1.5; tt += 3.4 + R() * 2.2) call(tt, w, pick(["issues", "git", "wiki", "devdata"]));
      call(lastRep + 0.8, w, "wh"); // save_investigation_result, once
      const done = send(lastRep + 2.0, w, inv); // one line with the saved id
      A[w].dies = done + (small ? 1.6 : 3.5) + R() * 2;
      invFree[j] = Math.max(end, A[w].dies);
      // Findings reach the initiative's agent; where the evidence and the declared status
      // disagree, its followers hear it from their own agents.
      const back = send(done + 0.7, inv, init[ii]);
      if (j % 3 === 0) subsOf[ii].forEach((p, q) => { const l2 = send(back + 0.3 + q * 0.1, init[ii], p); send(l2 + 0.5, p, "chat", "m"); });
    };
    send(INV0, "sched", inv);
    for (let j = 0; j < NIW; j++) invWorker(j, INV0 + 0.7 + Math.floor(j / 10) * 0.7 + (j % 10) * 0.05);
    for (let t = 27; t < DUR - 13.5; t += 0.9 + R() * 0.9) {
      const free = [];
      for (let j = 0; j < NIW; j++) if (invFree[j] + 0.8 < t) free.push(j);
      if (free.length) invWorker(pick(free), t, true);
    }

    // Delivery analysis: workers read the delivery history of one initiative each, asked of its
    // agent, and save their findings, which go back to that agent. Single analyses continue
    // between cycles in the slots the wave has left.
    const FX0 = 2.0, NA = 40;
    const fxFree = new Array(NA).fill(0);
    const fxWorker = (j, ts, small = false) => {
      const ii = (j * 41 + 7 + (small ? Math.floor(R() * NI) : 0)) % NI;
      const w = agent({ ...on(fxx, fxy, -Math.PI / 2 + ((j + 0.5) * TAU) / NA, Rf * (j % 2 ? 1 : 0.7)), r: 1.15 * S, k: "work", sys: "rev", parent: fx, born: ts, dies: 0 });
      send(ts, fx, w, "a", 0.3);
      send(send(ts + 0.1, fx, init[ii]) + 0.2, init[ii], fx, "i"); // the initiative's delivery history
      const reads = small ? 2 : 3;
      for (let q = 0; q < reads; q++) call(ts + 0.8 + q * 1.9, w, "wh");
      const done = send(ts + 1.9 * reads + 2.0, w, fx);
      A[w].dies = done + (small ? 2.5 : 6) + R() * 3;
      fxFree[j] = A[w].dies;
      send(done + 0.6, fx, init[ii]); // findings to the initiative's agent
    };
    send(FX0, "sched", fx);
    for (let j = 0; j < NA; j++) fxWorker(j, FX0 + 0.8 + Math.floor(j / 8) * 1.0 + (j % 8) * 0.07);
    for (let t = 21; t < DUR - 11; t += 1.1 + R() * 1.1) {
      const free = [];
      for (let j = 0; j < NA; j++) if (fxFree[j] + 0.8 < t) free.push(j);
      if (free.length) fxWorker(pick(free), t, true);
    }

    // The reporting agent reads the latest reviews through the review agent and posts through channel agents.
    send(21.0, summ, coord); send(22.0, coord, summ);
    chans.forEach((c, n) => { const land = send(22.8 + n * 0.12, summ, c); send(land + 0.5, c, "chat", "m"); });

    caption(0, "cycle", "The nightly cycle", "A schedule wakes the review agent. It opens one WorkGraph item per initiative and spawns a review worker for each, five at a time. The band around it fills as items are claimed and closed.");
    caption(2.2, "waves", "Investigations in waves", "The investigation agent asks each initiative's agent for its record and spawns investigation workers in waves. Each spawns one helper per person to gather activity evidence through enterprise search.");
    caption(5.6, "parent", "One parent each", "Every worker is wired only to the agent that spawned it, calls the tools its role allows and reports back to it. The delivery-analysis agent runs its workers the same way.", false);
    caption(9.2, "reconcile", "Reconciled from the WorkGraph", "The review agent counts closed items and reads each saved review by id. Initiative agents pass results to the personal agents that follow them, which deliver by chat.", false);
    caption(14.2, "peak", "About 700 at once", "At peak about 700 agents are active at once: the long-lived agents and several hundred workers, messaging and calling tools.");
    caption(24.6, "subside", "Subsiding", "Workers retire when idle. Findings go back through the initiative agents to the people who follow them.");
    caption(32.5, "dev", "Development", "Each repository has long-lived domain agents. People work through their personal agents, which consult them, and domain agents consult each other across repositories.", false);
    caption(41.5, "quiet", "Between cycles", "50 to 100 agents stay live. People ask their own agents, which ask the initiative agents; analysis agents check records and run single investigations; domain agents consult each other.");

    /* ---------- Timing, wake windows ---------- */
    M.forEach((m) => { m.t *= K; m.d *= K; });
    M.sort((u, v) => u.t - v.t);
    A.forEach((a) => { if (a.k === "work") { a.born *= K; a.dies *= K; } });
    items.forEach((it) => { it.seed *= K; it.claim *= K; it.done *= K; });
    // A wake window that runs past the end of the loop continues at its start, so the murmur
    // does not stop when the loop restarts.
    const iv = A.map(() => []);
    const LOOP = DUR * K;
    const wake = (i, s0) => { const e0 = s0 + A[i].hold * K; iv[i].push([s0, e0]); if (e0 > LOOP) iv[i].push([s0 - LOOP, e0 - LOOP]); };
    M.forEach((m) => {
      if (typeof m.a === "number" && A[m.a].hold) wake(m.a, m.t);
      if (typeof m.b === "number" && A[m.b].hold) wake(m.b, m.t + m.d);
    });
    A.forEach((a, i) => {
      const list = iv[i].sort((u, v) => u[0] - v[0]), out = [];
      list.forEach(([s0, e0]) => { if (out.length && s0 <= out[out.length - 1]) out[out.length - 1] = Math.max(out[out.length - 1], e0); else out.push(s0, e0); });
      a.wake = out;
    });
    const awake = (a, t) => { const w = a.wake; for (let q = 0; q < w.length; q += 2) if (t >= w[q] && t < w[q + 1]) return true; return false; };
    const flashT = 0.35 * Math.max(K, 0.8);
    const ARC_KEEP = 0.7 * K; // a consult's arc stays briefly after it lands
    const maxD = M.reduce((v, m) => Math.max(v, m.d), 0) + Math.max(flashT, ARC_KEEP);
    const first = (t) => { let lo = 0, hi = M.length; while (lo < hi) { const mid = (lo + hi) >> 1; if (M[mid].t < t) lo = mid + 1; else hi = mid; } return lo; };
    const SPAWN = 0.45 * K, GONE = 0.3 * K;
    const tick = 1.6 * K;
    const invKids = new Set(); // the investigation agent's workers and their helpers
    A.forEach((a, i) => { if (a.k === "work" && (a.parent === inv || (A[a.parent] && A[a.parent].parent === inv))) invKids.add(i); });

    const script = [];
    cap.forEach((c) => { if (labels || c.hero) script.push({ t: c.t * K, cap: { key: c.key, title: c.title, body: c.body } }); });
    // A settled moment for reduced motion: the peak.
    const staticAt = 18 * K;

    /* ---------- Static wiring as paths ---------- */
    const path = (pairs) => { const p = new Path2D(); pairs.forEach(([a, b]) => { const u = P(a), v = P(b); p.moveTo(u.x, u.y); p.lineTo(v.x, v.y); }); return p; };
    const W1 = {
      intra: path(E.intra), cross: path(E.cross), devRepo: path(E.devRepo), personal: path(E.personal), devInit: path(E.devInit),
      spokes: path(E.spokes), subs: path(E.subs), core: path(E.core), invInit: path(E.invInit), fxInit: path(E.fxInit),
      sched: path([["sched", coord], ["sched", inv], ["sched", fx]]), op: path([[op, admin]]),
    };
    const frames = new Path2D();
    repos.forEach((rp) => frames.rect(rp.cx - rp.f / 2 + 0.5, rp.cy - rp.f / 2 + 0.5, rp.f, rp.f));
    // The network does not end at the canvas: a fading row of further repositories.
    const moreRow = G.more ? repos.filter((rp) => rp.row === G.drows - 1).map((rp, k) => ({ cx: rp.cx, cy: rp.cy + chh, f: fw * [0.8, 0.66, 0.92][k % 3] })) : [];

    /* ---------- Drawing ---------- */
    const n = A.length;
    const px = new Float32Array(n), py = new Float32Array(n), pr = new Float32Array(n);
    const st = new Uint8Array(n); // 0 hidden, 1 dormant, 2 awake, 3 flash, 4 worker, 5 worker flash, 6 person, 7 person flash
    const fl = new Uint8Array(n);
    const inflight = [], bends = [];
    let liveLong = 0, liveWork = 0, peak = 0, lastT = -1, cache = null, invPop = 0, invR = 0;

    const pos = (a, t) => {
      if (a.k !== "work") return [a.x, a.y, a.r];
      const e = Math.min(1, (t - a.born) / SPAWN), q = 1 - Math.pow(1 - e, 3);
      const par = A[a.parent];
      const rr = t > a.dies - GONE ? a.r * Math.max(0, (a.dies - t) / GONE) : a.r;
      return [par.x + (a.x - par.x) * q, par.y + (a.y - par.y) * q, rr];
    };
    const ptOf = (e) => (typeof e === "number" ? [px[e] || A[e].x, py[e] || A[e].y] : [P(e).x, P(e).y]);
    // A message's two ends: calls stop at the edge of the connector's icon.
    const ends = (m) => {
      let [ax, ay] = ptOf(m.a), [bx, by] = ptOf(m.b);
      const len = Math.hypot(bx - ax, by - ay) || 1;
      const ka = typeof m.a === "string" ? Math.min(0.4, iconR(m.a) / len) : 0;
      const kb = typeof m.b === "string" ? Math.min(0.4, iconR(m.b) / len) : 0;
      const dx = bx - ax, dy = by - ay;
      return [ax + dx * ka, ay + dy * ka, bx - dx * kb, by - dy * kb];
    };
    // A consult's arc: a quadratic bend to one side of the straight line.
    const bendAt = (ax, ay, bx, by, e) => {
      const mx = (ax + bx) / 2, my = (ay + by) / 2, dx = bx - ax, dy = by - ay;
      const cx = mx - dy * 0.28, cy = my + dx * 0.28, q = 1 - e;
      return [q * q * ax + 2 * q * e * cx + e * e * bx, q * q * ay + 2 * q * e * cy + e * e * by];
    };

    const decor = (ctx, net) => {
      const t = net.t, s = net.S;
      if (t < lastT) peak = 0;
      lastT = t;
      const fade = reduced ? 1 : Math.min(1, (performance.now() - start) / 700);
      ctx.save();
      const ga = (a) => { ctx.globalAlpha = fade * a; };
      ctx.setLineDash([]);

      // Arrivals light their target; messages in flight are drawn last.
      fl.fill(0);
      for (const k in CN) CN[k].hot = false;
      inflight.length = 0; bends.length = 0;
      for (let q = first(t - maxD); q < M.length && M[q].t <= t; q++) {
        const m = M[q], e = t - m.t;
        if (e < m.d) inflight.push(m);
        else if (e < m.d + flashT) { if (typeof m.b === "number") fl[m.b] = 1; else if (typeof m.b === "string") CN[m.b].hot = true; }
        if (m.bend && e < m.d + ARC_KEEP) bends.push(m);
      }
      if ((t % tick) < 0.22 * K) CN.sched.hot = true;

      // Agent states for this frame.
      liveLong = 0; liveWork = 0; invPop = 0; invR = 0;
      for (let i = 0; i < n; i++) {
        const a = A[i];
        if (a.k === "work") {
          if (t < a.born || t >= a.dies) { st[i] = 0; continue; }
          const [x, y, r] = pos(a, t);
          px[i] = x; py[i] = y; pr[i] = r;
          st[i] = fl[i] ? 5 : 4;
          liveWork++;
          if (invKids.has(i)) { invPop++; invR = Math.max(invR, Math.hypot(x - ix, y - iy)); }
        } else {
          px[i] = a.x; py[i] = a.y; pr[i] = a.r;
          if (a.k === "pers") { st[i] = fl[i] ? 7 : 6; continue; }
          const w = awake(a, t);
          st[i] = fl[i] ? 3 : w ? 2 : 1;
          if (w) liveLong++;
        }
      }

      // Wiring that never changes is drawn once into a bitmap and blitted each frame. It
      // includes a faint ghost of every worker the night will spawn, so the full scale is
      // on screen from the first frame, and the empty slots of the WorkGraph band.
      if (!cache || cache.dpr !== net.dpr) {
        const cv = document.createElement("canvas");
        cv.width = Math.round(W * net.dpr); cv.height = Math.round(H * net.dpr);
        const c2 = cv.getContext("2d");
        c2.setTransform(net.dpr, 0, 0, net.dpr, 0, 0);
        c2.lineWidth = 1;
        const strokeP = (p, a, dash) => { c2.globalAlpha = a; c2.strokeStyle = rgba(COL.ink, 1); c2.setLineDash(dash ? dash.map((v) => v * s) : []); c2.stroke(p); };
        strokeP(W1.intra, 0.14); strokeP(W1.cross, 0.08); strokeP(W1.devRepo, 0.055); strokeP(W1.devInit, 0.04);
        strokeP(W1.personal, 0.25, [1.5, 2]);
        strokeP(W1.invInit, wide ? 0.026 : 0.016); strokeP(W1.fxInit, wide ? 0.026 : 0.016); strokeP(W1.spokes, 0.05); strokeP(W1.subs, 0.065); strokeP(W1.core, 0.2);
        strokeP(W1.sched, 0.12, [2, 3]); strokeP(W1.op, 0.25, [1.5, 2]);
        strokeP(frames, 0.26, [3, 3]);
        moreRow.forEach((m, k) => {
          const f = new Path2D(); f.rect(m.cx - m.f / 2 + 0.5, m.cy - m.f / 2 + 0.5, m.f, m.f);
          strokeP(f, 0.16 - k * 0.04, [3, 3]);
        });
        c2.globalAlpha = 0.3; c2.setLineDash([3 * s, 3 * s]); c2.strokeRect(sx0 + 0.5, sy0 + 0.5, sx1 - sx0, sy1 - sy0);
        c2.setLineDash([]);
        const ghost = new Path2D();
        A.forEach((a) => { if (a.k === "work") { ghost.moveTo(a.x + a.r, a.y); ghost.arc(a.x, a.y, a.r, 0, TAU); } });
        c2.globalAlpha = 0.13; c2.lineWidth = Math.max(0.8, 0.9 * s); c2.stroke(ghost);
        c2.lineWidth = 1;
        const hs = itemSide / 2, slots = new Path2D();
        items.forEach((it) => slots.rect(it.x - hs + 0.5, it.y - hs + 0.5, itemSide - 1, itemSide - 1));
        c2.globalAlpha = 0.16; c2.stroke(slots);
        cache = cv; cache.dpr = net.dpr;
      }
      ga(1);
      ctx.drawImage(cache, 0, 0, W, H);
      ctx.lineWidth = 1;
      // Workers hang off their parent only.
      ctx.beginPath();
      for (let i = 0; i < n; i++) if (st[i] === 4 || st[i] === 5) { const p = A[i].parent; ctx.moveTo(px[p], py[p]); ctx.lineTo(px[i], py[i]); }
      ga(0.17); ctx.strokeStyle = rgba(COL.ink, 1); ctx.stroke();

      // Consults across repositories: the arc stays drawn while it is in use.
      if (bends.length) {
        const pth = new Path2D();
        bends.forEach((m) => {
          const [ax, ay, bx, by] = ends(m);
          pth.moveTo(ax, ay);
          for (let k = 1; k <= 10; k++) { const [x, y] = bendAt(ax, ay, bx, by, k / 10); pth.lineTo(x, y); }
        });
        ga(0.26); ctx.strokeStyle = rgba(COL.ink, 1); ctx.lineWidth = 1; ctx.stroke(pth);
      }

      // Connected systems.
      for (const k in CN) {
        const c = CN[k], hot = c.hot;
        ga(1);
        if (c.kind === "store") {
          const r = 5 * s, w2 = r * 2.4, h2 = r * 2, e2 = r * 0.45;
          ctx.fillStyle = rgba(COL.screen, 1); ctx.strokeStyle = rgba(hot ? COL.accent : COL.ink, 1); ctx.lineWidth = 1.2 * s;
          ctx.beginPath(); ctx.ellipse(c.x, c.y - h2 / 2, w2 / 2, e2, 0, Math.PI, 0); ctx.lineTo(c.x + w2 / 2, c.y + h2 / 2);
          ctx.ellipse(c.x, c.y + h2 / 2, w2 / 2, e2, 0, 0, Math.PI); ctx.closePath(); ctx.fill(); ctx.stroke();
          ctx.beginPath(); ctx.ellipse(c.x, c.y - h2 / 2, w2 / 2, e2, 0, 0, Math.PI); ctx.stroke();
        } else {
          const w2 = (c.sub ? 6.4 : 7.2) * s;
          ctx.fillStyle = rgba(hot ? COL.accent : COL.screen, hot ? 0.18 : 1); ctx.fillRect(c.x - w2 / 2, c.y - w2 / 2, w2, w2);
          ctx.strokeStyle = rgba(hot ? COL.accent : COL.mid, 1); ctx.lineWidth = 1.2 * s;
          ctx.strokeRect(c.x - w2 / 2, c.y - w2 / 2, w2, w2);
        }
      }
      ctx.lineWidth = 1;

      // The WorkGraph band: seeded items open, claimed items orange, closed items ink.
      const hs = itemSide / 2;
      const itemsIn = (test) => { ctx.beginPath(); items.forEach((it) => { if (test(it)) ctx.rect(it.x - hs, it.y - hs, itemSide, itemSide); }); };
      const showItem = (it) => t >= it.seed && t < (DUR - 2) * K;
      itemsIn((it) => showItem(it) && t < it.claim); ga(1); ctx.fillStyle = rgba(COL.screen, 1); ctx.fill(); ga(0.75); ctx.strokeStyle = rgba(COL.ink, 1); ctx.stroke();
      itemsIn((it) => showItem(it) && t >= it.claim && t < it.done); ga(1); ctx.fillStyle = rgba(COL.accent, 1); ctx.fill();
      itemsIn((it) => showItem(it) && t >= it.done); ga(0.88); ctx.fillStyle = rgba(COL.ink, 1); ctx.fill();

      // Agents, one path per look.
      const disc = (test, col, a) => {
        ctx.beginPath();
        let any = false;
        for (let i = 0; i < n; i++) if (test(i)) { any = true; ctx.moveTo(px[i] + pr[i], py[i]); ctx.arc(px[i], py[i], pr[i], 0, TAU); }
        if (any) { ga(a); ctx.fillStyle = rgba(col, 1); ctx.fill(); }
      };
      disc((i) => st[i] === 1 && A[i].k === "long", COL.ink, 0.2);
      disc((i) => st[i] === 1 && A[i].k === "mid", COL.mid, 0.26);
      disc((i) => st[i] === 2 && A[i].k === "long", COL.ink, 1);
      disc((i) => st[i] === 2 && A[i].k === "mid", COL.mid, 1);
      disc((i) => st[i] === 3, COL.accent, 1);
      // Coordinators carry a ring.
      [coord, inv, fx].forEach((i) => {
        ga(st[i] === 1 ? 0.3 : 0.6); ctx.strokeStyle = rgba(st[i] === 3 ? COL.accent : COL.ink, 1);
        ctx.beginPath(); ctx.arc(A[i].x, A[i].y, A[i].r + 2.6 * s, 0, TAU); ctx.stroke();
      });
      // Workers: hollow.
      const ring = (test, col, lw) => {
        ctx.beginPath();
        let any = false;
        for (let i = 0; i < n; i++) if (test(i) && pr[i] > 0.2) { any = true; ctx.moveTo(px[i] + pr[i], py[i]); ctx.arc(px[i], py[i], pr[i], 0, TAU); }
        if (!any) return;
        ga(1); ctx.fillStyle = rgba(COL.screen, 1); ctx.fill();
        ctx.strokeStyle = rgba(col, 1); ctx.lineWidth = lw; ctx.stroke();
      };
      ring((i) => st[i] === 4, COL.ink, Math.max(0.9, 0.95 * s));
      ring((i) => st[i] === 5, COL.accent, Math.max(1, 1.1 * s));
      // People: a ring with a centre dot.
      ring((i) => st[i] === 6, COL.mid, Math.max(0.9, 1.05 * s));
      ring((i) => st[i] === 7, COL.accent, Math.max(0.9, 1.05 * s));
      ctx.beginPath();
      for (let i = 0; i < n; i++) if (st[i] >= 6) { ctx.moveTo(px[i] + pr[i] * 0.4, py[i]); ctx.arc(px[i], py[i], pr[i] * 0.4, 0, TAU); }
      ga(1); ctx.fillStyle = rgba(COL.mid, 1); ctx.fill();
      ctx.lineWidth = 1;

      // Messages in flight: one path per colour.
      const dotR = Math.max(1.1, 1.45 * s), trail = 11 * s;
      ["m", "i", "a"].forEach((c) => {
        const col = c === "m" ? COL.mid : c === "i" ? COL.ink : COL.accent;
        const trails = new Path2D(), dots = new Path2D();
        let any = false;
        for (const m of inflight) {
          if (m.c !== c) continue;
          any = true;
          const p = (t - m.t) / m.d;
          const e = p < 0.5 ? 2 * p * p : 1 - Math.pow(-2 * p + 2, 2) / 2;
          const [ax, ay, bx, by] = ends(m);
          const e2 = Math.max(0, e - trail / (Math.hypot(bx - ax, by - ay) + 1));
          let x, y, tx, ty;
          if (m.bend) { [x, y] = bendAt(ax, ay, bx, by, e); [tx, ty] = bendAt(ax, ay, bx, by, e2); }
          else { x = ax + (bx - ax) * e; y = ay + (by - ay) * e; tx = ax + (bx - ax) * e2; ty = ay + (by - ay) * e2; }
          trails.moveTo(tx, ty); trails.lineTo(x, y);
          dots.moveTo(x + dotR, y); dots.arc(x, y, dotR, 0, TAU);
        }
        if (!any) return;
        ga(0.32); ctx.strokeStyle = rgba(col, 1); ctx.lineWidth = 1.2 * s; ctx.stroke(trails);
        ga(c === "m" ? 0.85 : 1); ctx.fillStyle = rgba(col, 1); ctx.fill(dots);
      });
      ctx.restore();
    };

    /* ---------- Labels and the meter ---------- */
    const lab = (ctx, s, str, x, y, o = {}) => text(ctx, s, str, x, y, Object.assign({ size: 9, c: COL.mid, halo: true, base: "middle" }, o));
    // A label on a small plate of the screen colour: what passes under it stays under it.
    const plate = (ctx, s, str, x, y, o = {}) => {
      const size = o.size || 9, w = o.w || 500, track = o.track || 0;
      ctx.font = `${w} ${Math.max(9, size * s)}px ${SANS}`;
      if (track) ctx.letterSpacing = `${track}px`;
      const tw = ctx.measureText(str).width;
      if (track) ctx.letterSpacing = "0px";
      const fs = Math.max(9, size * s), al = o.align || "left";
      const x0 = al === "center" ? x - tw / 2 : al === "right" ? x - tw : x;
      ctx.fillStyle = rgba(COL.screen, 1);
      ctx.fillRect(x0 - 3, y - fs * 0.62, tw + 6, fs * 1.24);
      text(ctx, s, str, x, y, Object.assign({ c: COL.mid, base: "middle" }, o));
    };
    // Text set around the bottom of a ring, like the lettering on a dial.
    const ringText = (ctx, s, str, cx, cy, rad, o = {}) => {
      const size = Math.max(o.min || 8, (o.size || 8) * s), track = o.track || 0;
      ctx.font = `${o.w || 600} ${size}px ${SANS}`;
      const ws = [...str].map((ch) => ctx.measureText(ch).width + track);
      const total = ws.reduce((a, b) => a + b, 0) - track;
      let a = Math.PI / 2 + total / rad / 2;
      ctx.textAlign = "center"; ctx.textBaseline = "middle"; ctx.lineJoin = "round";
      [...str].forEach((ch, k) => {
        const ac = a - ws[k] / 2 / rad;
        ctx.save();
        ctx.translate(cx + Math.cos(ac) * rad, cy + Math.sin(ac) * rad);
        ctx.rotate(ac - Math.PI / 2);
        ctx.lineWidth = 3; ctx.strokeStyle = rgba(COL.screen, 0.92); ctx.strokeText(ch, 0, 0);
        ctx.fillStyle = rgba(o.c || COL.ink, o.a ?? 0.7); ctx.fillText(ch, 0, 0);
        ctx.restore();
        a -= ws[k] / rad;
      });
    };
    const meter = (ctx, s) => {
      const [mx0, my0, mx1, my1] = G.meter;
      const total = liveLong + liveWork;
      peak = Math.max(peak, total);
      const hgt = my1 - my0;
      header(ctx, s, "ACTIVE AGENTS", mx0, my0 + 7 * s);
      // Legend for the two kinds of segment, right-aligned on the title row.
      ctx.font = `500 ${Math.max(9, 8.5 * s)}px ${SANS}`;
      const l2 = "workers", l1 = "long-lived";
      const w2 = ctx.measureText(l2).width, w1 = ctx.measureText(l1).width;
      const sq = 5.5 * s, ly = my0 + 4.2 * s;
      let x = mx1 - w2;
      text(ctx, s, l2, x, ly, { size: 8.5, c: COL.mid, base: "middle" });
      x -= sq + 4 * s;
      ctx.strokeStyle = rgba(COL.ink, 1); ctx.lineWidth = 1; ctx.strokeRect(x + 0.5, ly - sq / 2 + 0.5, sq - 1, sq - 1);
      x -= 10 * s + w1;
      text(ctx, s, l1, x, ly, { size: 8.5, c: COL.mid, base: "middle" });
      x -= sq + 4 * s;
      ctx.fillStyle = rgba(COL.ink, 1); ctx.fillRect(x, ly - sq / 2, sq, sq);
      // The reading: fixed-width digits, so the number does not jitter.
      const fs = Math.max(17, Math.min(hgt * 0.42, 24 * s));
      ctx.font = `600 ${fs}px ${SANS}`;
      const dw = ctx.measureText("0").width;
      const str = String(total);
      const nx = mx0 + dw * 3;
      const baseY = my0 + hgt * 0.72;
      ctx.fillStyle = rgba(COL.ink, 1); ctx.textAlign = "center"; ctx.textBaseline = "alphabetic";
      for (let k = 0; k < str.length; k++) ctx.fillText(str[str.length - 1 - k], nx - (k + 0.5) * dw, baseY);
      // The ladder: 40 segments of 20 agents.
      const NSEG = 40, per = 20;
      const lx0 = nx + 12 * s, lx1 = mx1;
      const sw = (lx1 - lx0) / NSEG, segW = Math.max(1.5, sw * 0.6), segH = Math.max(7, Math.min(hgt * 0.3, 12 * s));
      const top = baseY - segH;
      const litN = Math.ceil(total / per), longN = Math.ceil(liveLong / per), peakN = Math.ceil(peak / per);
      for (let k = 0; k < NSEG; k++) {
        const sx = lx0 + k * sw;
        if (k < litN) {
          if (k === litN - 1 && total > 0 && !reduced) { ctx.fillStyle = rgba(COL.accent, 1); ctx.fillRect(sx, top, segW, segH); }
          else if (k < longN) { ctx.fillStyle = rgba(COL.ink, 1); ctx.fillRect(sx, top, segW, segH); }
          else { ctx.strokeStyle = rgba(COL.ink, 1); ctx.lineWidth = 1; ctx.strokeRect(sx + 0.5, top + 0.5, segW - 1, segH - 1); }
        } else if (k === peakN - 1) { ctx.fillStyle = rgba(COL.ink, 0.55); ctx.fillRect(sx + segW / 2 - 0.75, top, 1.5, segH); }
        else { ctx.fillStyle = rgba(COL.faint, 0.55); ctx.fillRect(sx, top, segW, segH); }
      }
      // Scale.
      for (let v = 0; v <= 800; v += 200) {
        const sx = v === 0 ? lx0 : v === 800 ? lx1 : lx0 + (v / per) * sw - (sw - segW) / 2;
        text(ctx, s, String(v), sx, baseY + 9 * s, { size: 8, c: COL.mid, align: v === 800 ? "right" : v ? "center" : "left", base: "middle" });
      }
    };

    const overlay = (ctx, net) => {
      const s = net.S;
      const fade = reduced ? 1 : Math.min(1, (performance.now() - start) / 700);
      ctx.save();
      ctx.globalAlpha = fade;
      G.hdr.forEach(([x, y, str]) => header(ctx, s, str, x, y));
      if (labels) {
        // Connected systems: labels under their icons (beside, for the two at the top).
        for (const k in CN) {
          const c = CN[k];
          if (k === "sched" || k === "chat") lab(ctx, s, c.label, c.x - 8 * s, c.y, { align: "right" });
          else plate(ctx, s, c.label, c.x, c.y + (c.kind === "store" ? 14 : 11) * s, { align: "center" });
        }
        plate(ctx, s, narrow ? "SEARCH" : "ENTERPRISE SEARCH", sx0 + 8 * s, sy0 + 10 * s, { size: 8, w: 600, c: COL.ink, a: 0.6, track: 1 });
        // Singletons: each label sits beside its own node, at a fixed gap.
        const I = A[inv], F = A[fx], Sm = A[summ], Ad = A[admin], Op = A[op];
        const beside = (node, str, o = {}) => lab(ctx, s, str, node.x + node.r + (node.ring ? 6 : 4) * s, node.y, Object.assign({ c: COL.ink }, o));
        lab(ctx, s, "review agent", hx, hy + A[coord].r + 10 * s, { align: "center", c: COL.ink });
        if (wide) {
          beside(I, "investigation agent");
          beside(F, "delivery analysis");
          lab(ctx, s, "reporting agent", Sm.x, Sm.y - 9 * s, { align: "center", c: COL.ink });
          beside(Ad, "admin");
          beside(Op, "operator", { c: COL.mid });
          lab(ctx, s, "initiative agents", hx, hy - Rh * 0.785, { align: "center" });
          plate(ctx, s, "personal and channel agents", hx, hy - Rh - 8 * s, { align: "center" });
          lab(ctx, s, "domain agents, by repository", dx0, dy0 - 10 * s);
          lab(ctx, s, "personal agents", px0 - 2 * s, dy0 - 10 * s);
          if (moreRow.length) lab(ctx, s, "+ more repositories", dx0, moreRow[0].cy + fw * 0.62, { a: 0.8 });
          // The investigation agent's workers and helpers: named while a wave is out, under the cluster.
          if (invPop > 24) {
            ctx.globalAlpha = fade * Math.min(1, (invPop - 24) / 30);
            lab(ctx, s, "investigation workers and their helpers", ix, iy + Math.max(invR, Rd * 0.3) + 10 * s, { align: "center" });
            ctx.globalAlpha = fade;
          }
          ringText(ctx, s, "WORKGRAPH · ONE ITEM PER INITIATIVE", hx, hy, Rh * ((RI + RN) / 2), { size: 7.5, min: 7.5, track: 0.8 });
        } else {
          beside(I, "investigation");
          beside(F, "delivery analysis");
          lab(ctx, s, "admin", Ad.x, Ad.y + 10 * s, { align: "center", c: COL.ink });
          ringText(ctx, s, "WORKGRAPH", hx, hy, Rh * ((RI + RN) / 2), { size: 7.5, min: 7.5, track: 0.8 });
        }
      }
      meter(ctx, s);
      ctx.restore();
    };

    // Engine nodes: anchors only. The zoom opens from the investigation agent.
    const nodes = [
      { id: "inv", kind: "anchor", x: ix, y: iy, r: A[inv].r },
      { id: "coord", kind: "anchor", x: hx, y: hy, r: A[coord].r },
    ];
    const countAt = (t) => {
      let l = 0, w = 0;
      A.forEach((a) => { if (a.k === "pers") return; if (a.k === "work") { if (t >= a.born && t < a.dies) w++; } else if (awake(a, t)) l++; });
      return [l, w];
    };
    return { nodes, edges: [], script, duration: DUR * K, decor, overlay, origin: [W / 2, H / 2], staticAt, countAt };
  }

  /* ------------------------------------------------------------------ */
  /* Zoom: inside one analysis worker, spawned by the investigation agent  */
  /* for one initiative. Its tool surface is drawn around it: one tick per */
  /* tool its role receives, named, and lit when called. There is no       */
  /* shell. Four helpers gather activity evidence, one per person. It      */
  /* saves once and sends the investigation agent one line with the saved  */
  /* id. The transcript pane prints the same steps, in time.               */
  /* ------------------------------------------------------------------ */
  function organisationAgent(W, H, S, opts = {}) {
    const X = (f) => f * W, Y = (f) => f * H;
    const callout = W >= 400 && H >= 380; // room to name every tool beside the dial
    const cx = X(0.5), cy = Y(callout ? 0.37 : 0.33);
    const wr = callout ? Math.min(80 * S, Math.min(W, H) * 0.14) : Math.min(W * 0.14, H * 0.14);
    const r0 = wr + 4 * S, r1 = wr + 10 * S;

    // Systems it reaches: its plans and the reports store on the left; enterprise search on the right.
    const bY = Y(callout ? 0.905 : 0.9);
    const [fx0, fy0, fx1, fy1] = [0.3, callout ? 0.8 : 0.79, 0.985, 0.985];
    const SRC = [["issues", "issue tracker", "tickets"], ["git", "source control", "code"], ["wiki", "wiki", "wiki"], ["devdata", "developer data", "dev data"], ["dir", "directory", "directory"]];
    const storeAt = callout ? [X(0.06), Y(0.885)] : [X(0.075), bY - 2 * S];
    const nodes = [
      { id: "inv", kind: "coord", x: cx, y: Y(0.06), r: 5.5 * S, label: "investigation agent", always: true, labelPos: "right", labelSize: 10.5 },
      { id: "worker", kind: "helper", x: cx, y: cy, r: wr, hidden: true },
      callout
        ? { id: "wh", kind: "store", x: storeAt[0], y: storeAt[1], r: 5 * S, label: "reports store", always: true, labelPos: "right", labelTone: "mid", labelSize: 9.5 }
        : { id: "wh", kind: "store", x: storeAt[0], y: storeAt[1], r: 5 * S, label: "store", always: true, labelPos: "below", labelTone: "mid", labelSize: 9.5 },
      callout
        ? { id: "plans", kind: "source", x: X(0.06), y: Y(0.795), r: 4.5 * S, label: "plan documents", always: true, labelPos: "right", labelTone: "mid", labelSize: 9.5 }
        : { id: "plans", kind: "source", x: X(0.2), y: bY, r: 4.5 * S, label: "plans", always: true, labelPos: "below", labelTone: "mid", labelSize: 9.5 },
      { id: "search", kind: "frame", x: X((fx0 + fx1) / 2), y: Y((fy0 + fy1) / 2), r: 1, w: X(fx1 - fx0), h: Y(fy1 - fy0), label: "enterprise search", labelPos: "frame", always: true, labelSize: 9 },
    ];
    SRC.forEach(([id, label, short], n) => {
      if (callout) {
        const col = n % 3, row = Math.floor(n / 3);
        nodes.push({ id, kind: "source", x: X(fx0) + 12 * S + (X(fx1 - fx0) - 12 * S) * (col / 3), y: Y(fy0) + (Y(fy1 - fy0)) * (row ? 0.78 : 0.5), r: 4 * S, label, always: true, labelPos: "right", labelTone: "mid", labelSize: 9.5 });
      } else {
        nodes.push({ id, kind: "source", x: X(fx0) + X(fx1 - fx0) * (0.1 + n * 0.2), y: Y(fy0) + Y(fy1 - fy0) * 0.52, r: 4 * S, label: short, always: true, labelPos: "below", labelTone: "mid", labelSize: 9 });
      }
    });
    const hy = Y(callout ? 0.705 : 0.665);
    const HELP = ["h1", "h2", "h3", "h4"];
    const hx0 = callout ? 0.4 : 0.29, hdx = callout ? 0.115 : 0.14;
    HELP.forEach((id, n) => nodes.push({ id, kind: "helper", x: X(hx0 + n * hdx), y: hy, r: 5.5 * S, hidden: true }));
    const hcx = X(hx0 + 1.5 * hdx);

    /* The tool surface: every tool it calls in this run, one tick each (23). Named callouts
       where there is room; grouped ticks otherwise. */
    const TICK = {}; // tool -> { a, lx, ly, align }
    const LEFT = ["get_initiative_history", "get_all_initiatives", "get_plan_document", "get_current_date_time", null, "mob_profile_list", "mob_profile_get", "mob_create", "mob_spawn_member", "spawn_member", "mob_list", "mob_list_members", "mob_check_member", "mob_destroy"];
    const RIGHT = ["send_message", "peers", "save_investigation_result", null, "enterprise_search", "directory_search", "issue_tracker_search", "issue_tracker_get", "source_control_repo_info", "source_control_get_file", "read_document"];
    const GROUPS = [
      ["message", -100, -80, ["send_message", "peers"]],
      ["enterprise search", -48, 40, ["enterprise_search", "directory_search", "issue_tracker_search", "issue_tracker_get", "source_control_repo_info", "source_control_get_file", "read_document"]],
      ["helpers", 56, 124, ["mob_profile_list", "mob_profile_get", "mob_create", "mob_spawn_member", "spawn_member", "mob_list", "mob_list_members", "mob_check_member", "mob_destroy"]],
      ["save", 142, 142, ["save_investigation_result"]],
      ["initiative record", 168, 222, ["get_initiative_history", "get_all_initiatives", "get_plan_document", "get_current_date_time"]],
    ];
    const rowsY0 = Y(0.2), rowsY1 = Y(0.63);
    let ngY = 0;
    if (callout) {
      const units = (list) => list.reduce((v, x) => v + (x ? 1 : 0.5), 0) - 1;
      const pitch = (rowsY1 - rowsY0) / Math.max(units(LEFT), units(RIGHT));
      const place = (list, lx, align) => {
        let u = 0;
        list.forEach((tn) => {
          if (!tn) { u += 0.5; return; }
          const ly = rowsY0 + pitch * u;
          TICK[tn] = { a: Math.atan2(ly - cy, lx - cx), lx, ly, align };
          u += 1;
        });
      };
      place(LEFT, X(0.335), "right");
      place(RIGHT, X(0.665), "left");
      ngY = rowsY0 + pitch * units(LEFT) + 26 * S;
    } else {
      GROUPS.forEach(([, a0, a1, tools]) => tools.forEach((tn, k) => { TICK[tn] = { a: ((tools.length === 1 ? a0 : a0 + ((a1 - a0) * k) / (tools.length - 1)) * Math.PI) / 180 }; }));
    }

    const script = [];
    const lit = []; // [t, tool]
    const CALLS = []; // the time of every tool call, for the gauge (69 in all)
    const cap = (t, key, title, body) => script.push({ t, cap: { key, title, body } });
    const log = (t, kind, text) => {
      script.push({ t, log: { kind, text } });
      // "a, b x4, c" is six calls.
      if (kind === "tool") text.split(":")[0].split(", ").forEach((part, k) => { const m = part.match(/ x(\d+)$/); const n = m ? +m[1] : 1; for (let q = 0; q < n; q++) CALLS.push(t + k * 0.08 + q * 0.05); });
    };
    const m = (t, a, b, c = "mid", o = {}) => script.push(Object.assign({ t, msg: [a, b], c }, o));
    const on = (t, id, d = 1.2) => script.push({ t, on: id, d });
    // A tool call: its tick lights, and calls go out to what it reaches and come back.
    const use = (t, tools, targets = []) => {
      tools.forEach((tn, k) => lit.push([t + k * 0.1, tn]));
      targets.forEach((to, k) => { m(t + 0.1 + k * 0.1, "worker", to); m(t + 1.0 + k * 0.1, to, "worker"); });
    };

    cap(0, "spawn", "Spawned by the investigation agent", "One analysis worker per initiative, wired only to the investigation agent that spawned it. Around it, the 23 tools it calls in this run; inside it, its work so far. There is no shell.");
    on(0.2, "inv", 1.4);
    m(0.5, "inv", "worker", "accent");
    script.push({ t: 1.0, show: "worker" });
    log(0.9, "input", "From the investigation agent: investigate initiative 42 over the last 14 days. Save findings once.");
    log(2.2, "note", "Tools: the initiative's record, enterprise search, helpers, one save and one message. No shell.");

    cap(3.4, "record", "Its own record first", "It reads the initiative's history and plan, then searches the issue tracker, source control, the wiki and developer data through enterprise search.");
    log(3.4, "think", "Start from the initiative's own record: its history, the portfolio and today's date.");
    log(3.9, "tool", "get_initiative_history, get_all_initiatives, get_current_date_time");
    use(3.9, ["get_initiative_history", "get_all_initiatives", "get_current_date_time"], ["wh"]);
    log(5.3, "result", "History for the window loaded. Declared status: green.");
    log(5.9, "tool", "get_plan_document");
    use(5.9, ["get_plan_document"], ["plans"]);
    log(7.0, "result", "No synced plan document. Look for the plan in the wiki.");
    log(7.7, "think", "Find the plan, the epics, the repositories and the people.");
    log(8.2, "tool", "enterprise_search x4, directory_search");
    use(8.2, ["enterprise_search", "directory_search"], ["issues", "git", "wiki", "devdata", "dir"]);
    on(8.4, "worker", 2.2);
    log(9.7, "result", "Plan found in the wiki. Epics and repositories located.");
    log(10.4, "tool", "issue_tracker_get, read_document, enterprise_search x3");
    use(10.4, ["issue_tracker_get", "read_document", "enterprise_search"], ["issues", "wiki", "devdata"]);
    log(11.7, "result", "The plan names four milestones.");
    log(12.4, "tool", "issue_tracker_search, issue_tracker_get, read_document, enterprise_search x2");
    use(12.4, ["issue_tracker_search", "issue_tracker_get", "read_document", "enterprise_search"], ["issues", "wiki", "git"]);
    log(13.6, "result", "issue_tracker_search: query rejected. Retry later with narrower fields.");
    log(14.4, "tool", "directory_search x3, enterprise_search x2");
    use(14.4, ["directory_search", "enterprise_search"], ["dir", "devdata"]);
    log(15.6, "result", "Six people linked to the initiative, four with recent activity.");
    log(16.4, "tool", "source_control_repo_info x4, source_control_get_file");
    use(16.4, ["source_control_repo_info", "source_control_get_file"], ["git", "git"]);
    log(17.6, "result", "Four repositories. Code surface mapped.");

    cap(18.4, "helpers", "One helper per person", "Activity evidence takes time, so it spawns four helpers, one per person. Each is wired only to this worker and searches for its person's activity.");
    log(18.4, "think", "Activity evidence per person takes time. Spawn one helper per person.");
    log(19.0, "tool", "mob_profile_list, mob_profile_get, mob_create x2");
    use(19.0, ["mob_profile_list", "mob_profile_get", "mob_create"]);
    log(19.6, "tool", "mob_spawn_member x8, spawn_member");
    use(19.6, ["mob_spawn_member", "spawn_member"]);
    HELP.forEach((h, n) => { m(19.8 + n * 0.15, "worker", h, "accent", { dur: 0.45 }); script.push({ t: 20.2 + n * 0.15, show: h }); });
    log(20.7, "result", "Four helpers running, one per person.");
    // The helpers work in parallel: activity in developer data, the issue tracker and source control.
    const HS = ["devdata", "issues", "git", "devdata", "git", "issues"];
    HELP.forEach((h, n) => {
      for (let k = 0, t = 21.0 + n * 0.45; t < 28; k++, t += 1.9 + ((n + k) % 3) * 0.35) {
        const to = HS[(n + k) % HS.length];
        m(t, h, to); m(t + 0.95, to, h); on(t, h, 0.6);
      }
    });
    log(21.3, "tool", "read_document, source_control_get_file x2, enterprise_search x2");
    use(21.3, ["read_document", "source_control_get_file", "enterprise_search"], ["wiki", "git", "devdata"]);
    log(22.8, "tool", "issue_tracker_search x3");
    use(22.8, ["issue_tracker_search"], ["issues"]);
    log(23.9, "result", "Two milestones have no linked delivery work.");
    log(24.6, "tool", "mob_list, mob_list_members x2");
    use(24.6, ["mob_list", "mob_list_members"]);
    log(25.2, "result", "Helpers still working.");
    log(25.9, "tool", "directory_search, issue_tracker_get x4");
    use(25.9, ["directory_search", "issue_tracker_get"], ["dir", "issues"]);
    log(27.3, "tool", "mob_check_member x4");
    use(27.3, ["mob_check_member"]);
    HELP.forEach((h, n) => m(28.4 + n * 0.3, h, "worker", "accent"));
    log(29.8, "result", "Activity evidence for four people returned.");
    log(30.3, "tool", "mob_destroy");
    use(30.3, ["mob_destroy"]);
    script.push({ t: 30.7, hide: HELP });
    log(30.9, "tool", "directory_search x3");
    use(30.9, ["directory_search"], ["dir"]);

    cap(31.8, "status", "Computed, not declared", "It compares the declared status with what the evidence supports, and records the limits of its evidence.");
    log(31.8, "think", "Declared green, computed amber: two milestones have no delivery evidence.");
    on(31.8, "worker", 2.4);
    const tStatus = 32.2;
    log(33.2, "note", "Evidence limits: CI is seen only through developer data, and calendars are not connected.");

    cap(34.4, "save", "Save once, send one line", "The findings go to the reports store once, with a source fingerprint and the run id. The investigation agent gets one line with the saved id and reads the findings from the store.");
    log(34.4, "tool", "save_investigation_result: declared and computed status, anchors, blockers, code surface, people, evidence limits, recommendations, report");
    lit.push([34.4, "save_investigation_result"]);
    m(34.5, "worker", "wh", "mid"); on(35.3, "wh", 0.8); m(35.7, "wh", "worker", "mid");
    const tFly = 34.6, tLand = 35.5; // the record travels into the store and stays there
    log(36.3, "result", "Saved inv-7f3a. Run preflighted, source fingerprint attached.");
    log(37.0, "tool", "peers");
    lit.push([37.0, "peers"]);
    log(37.5, "peer", "To the investigation agent: DONE initiative=42 computed=amber declared=green saved=inv-7f3a");
    lit.push([37.5, "send_message"]);
    m(37.6, "worker", "inv", "accent"); on(38.5, "inv", 1.6);
    log(39.0, "note", "One save and one line. The investigation agent reads saved findings one at a time.");

    cap(40.6, "retire", "Retired after idle", "Idle, the worker retires. The saved findings stay in the store for the investigation agent and the initiative's agent.");
    const tRetire = 42.6;
    script.push({ t: tRetire, hide: "worker" });
    const DUR = 47, staticAt = 37.2;

    // Its work so far: 21 reasoning steps (each batch of calls follows one, and one writes the
    // final message), 69 tool calls, about 3.16M input tokens as its context is sent each step.
    const LOGS = script.filter((e) => e.log);
    const toolAt = LOGS.filter((e) => e.log.kind === "tool").map((e) => e.t);
    const STEPS = [...toolAt, ...LOGS.filter((e) => e.log.kind === "think" && !toolAt.some((u) => u > e.t && u - e.t < 0.7)).map((e) => e.t), 37.5, 38.2].sort((a, b) => a - b);
    CALLS.push(37.5); // send_message
    CALLS.sort((a, b) => a - b);
    const countTo = (list, t) => { let k = 0; while (k < list.length && list[k] <= t) k++; return k; };
    const eased = (list, t) => { const k = countTo(list, t); if (!k) return 0; const dt = t - list[k - 1]; return k - 1 + Math.min(1, dt / 0.35); };
    const GAUGE = [
      { name: "STEPS", max: 21, val: (t) => eased(STEPS, t), fmt: (v) => `${Math.round(v)}/21`, last: (t) => STEPS[countTo(STEPS, t) - 1] },
      { name: "CALLS", max: 69, val: (t) => eased(CALLS, t), fmt: (v) => `${Math.round(v)}/69`, last: (t) => CALLS[countTo(CALLS, t) - 1] },
      { name: "TOKENS", max: 3.16e6, val: (t) => 3.16e6 * Math.pow(eased(STEPS, t) / 21, 1.7), fmt: (v) => (v >= 1e6 ? `${(v / 1e6).toFixed(2)}M` : `${Math.round(v / 1e3)}k`), last: (t) => STEPS[countTo(STEPS, t) - 1] },
    ];

    const edges = [{ pairs: [["inv", "worker"], ...HELP.map((h) => ["worker", h])], alpha: 0.22 }];

    const still = frozen(nodes, script, staticAt, S);
    const ease = (p) => (p <= 0 ? 0 : p >= 1 ? 1 : p < 0.5 ? 2 * p * p : 1 - Math.pow(-2 * p + 2, 2) / 2);
    const strike = (ctx, s, str, x, y, align) => {
      text(ctx, s, str, x, y, { size: 9, c: COL.mid, a: 0.75, align, base: "middle", halo: true });
      ctx.font = `500 ${Math.max(9, 9 * s)}px ${SANS}`;
      const tw = ctx.measureText(str).width, x0 = align === "right" ? x - tw : x;
      ctx.strokeStyle = rgba(COL.mid, 0.8); ctx.lineWidth = 1;
      ctx.beginPath(); ctx.moveTo(x0 - 1, y + 0.5); ctx.lineTo(x0 + tw + 1, y + 0.5); ctx.stroke();
    };
    // The gauges inside the worker: a label, a ten-segment ladder and a reading per row.
    const gauges = (ctx, s, x, y, rad, t, a) => {
      const fs = Math.max(9, 8.5 * s), rowH = Math.max(fs + 5, 14 * s);
      ctx.font = `600 ${fs}px ${SANS}`;
      const labW = Math.max(...GAUGE.map((g) => ctx.measureText(g.name).width)) + 5;
      const valW = ctx.measureText("3.16M").width;
      const chord = 2 * Math.sqrt(Math.max(0, rad * rad - rowH * rowH));
      const room = Math.min(58 * s, chord - labW - valW - 16);
      const bars = room >= 30;
      const barW = bars ? room : 0;
      const total = labW + (bars ? barW + 8 : 6) + valW;
      const x0 = x - total / 2;
      GAUGE.forEach((g, k) => {
        const yy = y + (k - 1) * rowH, v = g.val(t), lt = g.last(t);
        const live = lt != null && t - lt < 0.6 && !reduced;
        ctx.globalAlpha = a;
        text(ctx, s, g.name, x0, yy, { size: 7.5, w: 600, c: COL.mid, base: "middle", track: 0.6 });
        if (bars) {
          const bx = x0 + labW + 4, n = 10, sw = barW / n, lit = (v / g.max) * n;
          for (let q = 0; q < n; q++) {
            const f = Math.max(0, Math.min(1, lit - q));
            ctx.fillStyle = rgba(COL.faint, 0.5); ctx.fillRect(bx + q * sw, yy - 3.5 * s, sw - 1.5, 7 * s);
            if (f > 0) { ctx.fillStyle = rgba(live && q === Math.ceil(lit) - 1 ? COL.accent : COL.ink, 1); ctx.fillRect(bx + q * sw, yy - 3.5 * s, (sw - 1.5) * f, 7 * s); }
          }
        }
        ctx.font = `600 ${fs}px ${SANS}`;
        text(ctx, s, g.fmt(v), x0 + total, yy, { size: 8.5, w: 600, c: live ? COL.accent : COL.ink, align: "right", base: "middle" });
      });
      ctx.globalAlpha = 1;
    };
    // The saved record: the verdict pair appears under the worker, then travels with the
    // record into the store and stays there, beside it.
    const recordAt = (t) => {
      const under = [cx, cy + wr + 26 * S];
      const home = callout ? [storeAt[0] + 14 * S, storeAt[1] + 26 * S] : [X(0.05), Y(0.69)];
      const p = ease((t - tFly) / (tLand - tFly));
      return [under[0] + (home[0] - under[0]) * p, under[1] + (home[1] - under[1]) * p, p];
    };
    const record = (ctx, s, t) => {
      if (t < tStatus) return;
      const a = Math.min(1, (t - tStatus) / 0.4);
      const [x, y, p] = recordAt(t);
      const fs = Math.max(9, 9 * s);
      ctx.save();
      ctx.globalAlpha = a;
      ctx.font = `500 ${fs}px ${SANS}`;
      const pair = ["declared: green", "computed: amber"];
      const pw = Math.max(...pair.map((str) => ctx.measureText(str).width));
      const saved = t >= 36.3;
      if (p <= 0) {
        // Under the worker: the two statuses side by side.
        const gap = 10 * s;
        pair.forEach((str, k) => {
          const bx = x + (k ? gap / 2 : -gap / 2 - pw - 12 * s);
          ctx.fillStyle = rgba(COL.screen, 1); ctx.strokeStyle = rgba(k && t < tStatus + 1.6 ? COL.accent : COL.ink, k && t < tStatus + 1.6 ? 1 : 0.55); ctx.lineWidth = 1;
          ctx.fillRect(bx + 0.5, y - 10 * s, pw + 12 * s, 20 * s); ctx.strokeRect(bx + 0.5, y - 10 * s, pw + 12 * s, 20 * s);
          text(ctx, s, str, bx + 6 * s + pw / 2, y + 0.5, { size: 9, c: COL.ink, a: 0.85, align: "center", base: "middle" });
        });
      } else {
        // The record: its id, then the pair, as one card tied to the store.
        const idW = ctx.measureText("inv-7f3a").width + 12 * s, h = 18 * s;
        const hot = t < tLand + 1.2 && !reduced;
        ctx.fillStyle = rgba(COL.screen, 1); ctx.strokeStyle = rgba(hot ? COL.accent : COL.ink, hot ? 1 : 0.55); ctx.lineWidth = 1;
        ctx.fillRect(x + 0.5, y - h / 2, idW, h); ctx.strokeRect(x + 0.5, y - h / 2, idW, h);
        text(ctx, s, saved ? "inv-7f3a" : "saving", x + idW / 2, y + 0.5, { size: 9, c: COL.ink, a: 0.9, align: "center", base: "middle" });
        pair.forEach((str, k) => text(ctx, s, str, x + idW + 8 * s, y + (k ? 7 : -7) * s, { size: 8.5, c: k ? COL.ink : COL.mid, base: "middle" }));
        if (p >= 1) {
          // Tied to the store: below it on wide canvases, above it on small ones.
          const st = storeAt;
          ctx.strokeStyle = rgba(COL.ink, 0.35); ctx.lineWidth = 1;
          ctx.beginPath();
          if (callout) { ctx.moveTo(st[0], st[1] + 7 * s); ctx.lineTo(st[0], y); ctx.lineTo(x, y); }
          else { ctx.moveTo(st[0], st[1] - 8 * s); ctx.lineTo(st[0], y + h / 2); }
          ctx.stroke();
        }
      }
      ctx.restore();
    };

    const overlay = (ctx, net) => {
      const s = net.S, t = reduced ? staticAt : net.t;
      const w = net.node("worker");
      if (w && w.a > 0.05) {
        ctx.save();
        ctx.globalAlpha = w.a;
        const hot = (tn) => lit.some(([lt, n]) => n === tn && t >= lt && t < lt + 1.0);
        Object.keys(TICK).forEach((tn) => {
          const k = TICK[tn], h = hot(tn);
          const ca = Math.cos(k.a), sa = Math.sin(k.a);
          const ex = w.x + ca * r1, ey = w.y + sa * r1;
          ctx.strokeStyle = rgba(h ? COL.accent : COL.ink, h ? 1 : 0.6);
          ctx.lineWidth = (h ? 2.2 : 1.3) * s;
          ctx.beginPath(); ctx.moveTo(w.x + ca * r0, w.y + sa * r0); ctx.lineTo(ex, ey); ctx.stroke();
          if (callout) {
            const lx = k.lx + (k.align === "left" ? -3 : 3) * s;
            ctx.strokeStyle = rgba(h ? COL.accent : COL.ink, h ? 0.7 : 0.16); ctx.lineWidth = 1;
            ctx.beginPath(); ctx.moveTo(ex + ca * 2 * s, ey + sa * 2 * s); ctx.lineTo(lx, k.ly); ctx.stroke();
            text(ctx, s, tn, k.lx + (k.align === "left" ? 2 : -2) * s, k.ly, { size: 9, c: h ? COL.accent : COL.mid, align: k.align, base: "middle", halo: true, w: h ? 600 : 500 });
          }
        });
        if (callout) {
          // Tools this role does not receive: under the tool list, clear of the helpers.
          const x0 = X(0.335) - 2 * s;
          header(ctx, s, "NOT GRANTED", x0, ngY, { a: 0.5, align: "right" });
          ["shell", "chat delivery"].forEach((str, k) => strike(ctx, s, str, x0, ngY + (14 + k * 14) * s, "right"));
        } else {
          GROUPS.forEach(([name, a0, a1]) => {
            if (name === "message" || name === "helpers") return;
            const a = (((a0 + a1) / 2) * Math.PI) / 180, rr = r1 + 6 * s;
            const c = Math.cos(a), sn = Math.sin(a);
            text(ctx, s, name, w.x + c * rr, w.y + sn * rr, { size: 9, c: COL.mid, align: c > 0.3 ? "left" : c < -0.3 ? "right" : "center", base: sn > 0.5 ? "top" : sn < -0.5 ? "bottom" : "middle", halo: true });
          });
        }
        // Its name, above and to the right, clear of the investigation agent's line and the callouts.
        const k = w.r / wr;
        if (callout) text(ctx, s, "analysis worker", w.x + 8 * s, rowsY0 - 16 * s, { size: 10.5, w: 600, c: COL.ink, base: "middle", halo: true });
        else text(ctx, s, "analysis worker", w.x - (r1 + 4 * s) * 0.5, w.y - (r1 + 4 * s) * 0.87, { size: 10, w: 600, c: COL.ink, align: "right", base: "middle", halo: true });
        // Inside: its work so far.
        gauges(ctx, s, w.x, w.y, wr * k - 8 * s, t, w.a);
        ctx.restore();
      } else if (t >= tRetire) {
        // Retired: a dashed ring where it was, with what it did.
        const a = Math.min(1, (t - tRetire - 0.3) / 0.6);
        if (a > 0) {
          ctx.save();
          ctx.globalAlpha = a;
          ctx.setLineDash([3 * s, 3 * s]); ctx.strokeStyle = rgba(COL.ink, 0.45); ctx.lineWidth = 1;
          ctx.beginPath(); ctx.arc(cx, cy, wr, 0, TAU); ctx.stroke(); ctx.setLineDash([]);
          text(ctx, s, "retired after idle", cx, cy - 7 * s, { size: 9.5, w: 600, c: COL.ink, a: 0.75, align: "center", base: "middle" });
          text(ctx, s, "21 steps · 69 calls", cx, cy + 8 * s, { size: 8.5, c: COL.mid, align: "center", base: "middle" });
          ctx.restore();
        }
      }
      const h = net.node("h1");
      if (h && h.a > 0.05) {
        ctx.save(); ctx.globalAlpha = h.a;
        text(ctx, s, "helpers, one per person", hcx, hy + 15 * s, { size: 9.5, c: COL.mid, align: "center", base: "middle", halo: true });
        ctx.restore();
      }
      record(ctx, s, t);
      still(ctx, net);
    };

    return { nodes, edges, script, duration: DUR, overlay, origin: [W / 2, H / 2], staticAt };
  }

  /* ------------------------------------------------------------------ */
  /* A household: filled in by household() below.                        */
  /* ------------------------------------------------------------------ */
  function household(W, H, S, opts) {
    return window.MeerkatHousehold(W, H, S, opts, { rng, text });
  }
  // Zoom: inside one household agent (defined in household.js).
  function householdAgent(W, H, S, opts) {
    return window.MeerkatHouseholdAgent(W, H, S, opts, { rng, text });
  }

  window.MeerkatScenes = { command, product, household, householdAgent, organisation, organisationAgent, rng, text };
})();
