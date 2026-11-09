/* Network engine for the Meerkat candidate page.
   Draws agent topologies on a canvas and plays their dynamics from a script.
   Scenes are pure functions of the canvas size; nodes morph between scenes. */
(() => {
  "use strict";

  const TAU = Math.PI * 2;
  const lerp = (a, b, t) => a + (b - a) * t;
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  const COL = {
    ink: [28, 28, 26],
    mid: [112, 111, 106],
    faint: [196, 194, 188],
    accent: [224, 89, 42],
    green: [70, 132, 60],
    screen: [250, 250, 247],
  };
  const rgba = (c, a) => `rgba(${c[0] | 0},${c[1] | 0},${c[2] | 0},${a})`;
  const SANS = '"Inter", "Helvetica Neue", Helvetica, Arial, sans-serif';

  class Net {
    constructor(canvas, opts = {}) {
      this.canvas = canvas;
      this.ctx = canvas.getContext("2d");
      this.opts = Object.assign({ labels: false, max: 720, onCaption: null, speed: 1 }, opts);
      this.pool = Array.from({ length: this.opts.max }, () => ({
        x: 0, y: 0, r: 0, a: 0, spec: null, shown: true, active: 0, flash: 0, state: null,
      }));
      this.msgs = [];
      this.t = 0;
      this.cursor = 0;
      this.running = true;
      this.paused = false;
      this.visible = true;
      this.placed = false;
      this.last = performance.now();
      new ResizeObserver(() => this.resize()).observe(canvas);
      this.resize();
      const io = new IntersectionObserver(([e]) => {
        const was = this.visible;
        this.visible = e.isIntersecting;
        if (this.visible && !was) { this.last = performance.now(); requestAnimationFrame((n) => this.frame(n)); }
      });
      io.observe(canvas);
      document.addEventListener("visibilitychange", () => { this.running = !document.hidden; });
      requestAnimationFrame((n) => this.frame(n));
    }

    resize() {
      const r = this.canvas.getBoundingClientRect();
      if (!r.width || !r.height) return;
      this.dpr = Math.min(window.devicePixelRatio || 1, 2);
      this.W = r.width; this.H = r.height;
      this.S = Math.min(this.W, this.H) / 480;
      this.canvas.width = Math.round(this.W * this.dpr);
      this.canvas.height = Math.round(this.H * this.dpr);
      this.ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
      if (this.factory) this.build(false);
    }

    setScene(factory, o = {}) {
      this.factory = factory;
      this.build(true);
      if (o.burstFrom && !reduced) {
        const [bx, by] = typeof o.burstFrom === "function" ? o.burstFrom(this) : o.burstFrom;
        this.pool.forEach((p) => { p.x = bx; p.y = by; p.r = 0.5; p.a = 0; });
      }
    }

    build(reset) {
      const s = this.factory(this.W, this.H, this.S, this.opts);
      s.script = s.script.map((e, i) => [e, i]).sort((a, b) => a[0].t - b[0].t || a[1] - b[1]).map((x) => x[0]);
      this.scene = s;
      this.index = new Map();
      const ox = s.origin ? s.origin[0] : this.W / 2, oy = s.origin ? s.origin[1] : this.H / 2;
      const place = !this.placed && this.W > 0;
      this.pool.forEach((p, i) => {
        const spec = s.nodes[i];
        if (spec) {
          p.spec = spec;
          this.index.set(spec.id, i);
        } else {
          p.spec = { id: null, x: ox, y: oy, r: 0, kind: "none" };
        }
        if (place) { p.x = p.spec.x; p.y = p.spec.y; }
      });
      if (place) this.placed = true;
      if (reset) this.restart();
      if (reduced) this.snap();
    }

    restart() {
      if (this.opts.onLog) this.opts.onLog(null, this.scene);
      this.t = 0;
      this.cursor = 0;
      this.msgs = [];
      this.pool.forEach((p) => {
        p.active = 0; p.flash = 0; p.memAt = null;
        p.state = p.spec.state || null;
        p.shown = !p.spec.hidden;
        p.born = p.spec.born || 0;
      });
    }

    snap() {
      this.pool.forEach((p) => { p.x = p.spec.x; p.y = p.spec.y; p.r = p.spec.r; p.a = this.targetAlpha(p); });
    }

    node(id) { const i = this.index.get(id); return i === undefined ? null : this.pool[i]; }

    targetAlpha(p) {
      if (!p.spec.id) return 0;
      if (!p.shown) return 0;
      if (p.born && this.t < p.born) return 0;
      return p.spec.alpha ?? 1;
    }

    fire(ev) {
      const s = this.scene;
      if (ev.msg) {
        const [from, to] = ev.msg;
        const a = this.node(from), b = this.node(to);
        if (a && b) {
          const d = Math.hypot(b.spec.x - a.spec.x, b.spec.y - a.spec.y);
          const dur = ev.dur ?? Math.max(0.45, Math.min(1.4, d / (260 * this.S)));
          this.msgs.push({ a: from, b: to, t0: this.t, dur, c: ev.c || "accent", land: ev.land !== false });
        }
      }
      if (ev.on) [].concat(ev.on).forEach((id) => { const n = this.node(id); if (n) n.active = this.t + (ev.d ?? 1.2); });
      if (ev.show) [].concat(ev.show).forEach((id) => { const n = this.node(id); if (n) n.shown = true; });
      if (ev.hide) [].concat(ev.hide).forEach((id) => { const n = this.node(id); if (n) n.shown = false; });
      if (ev.mem) [].concat(ev.mem).forEach((id) => { const n = this.node(id); if (n) { n.memAt = this.t; n.memOp = ev.op || "write"; } });
      if (ev.state) {
        const [ids, st] = ev.state;
        [].concat(ids).forEach((id) => { const n = this.node(id); if (n) n.state = st; });
      }
      if (ev.cap && this.opts.onCaption) this.opts.onCaption(ev.cap, s);
      // Transcript lines, timed with the drawing (used by the zoomed-in views).
      if (ev.log && this.opts.onLog) this.opts.onLog(ev.log, s);
    }

    frame(now) {
      const dt = Math.min((now - this.last) / 1000, 0.05);
      this.last = now;
      if (this.scene && this.running && !this.paused) this.step(dt);
      this.draw();
      if (this.visible) requestAnimationFrame((n) => this.frame(n));
    }

    step(dt) {
      const s = this.scene;
      if (!reduced) {
        this.t += dt * this.opts.speed;
        const ev = s.script;
        while (this.cursor < ev.length && ev[this.cursor].t <= this.t) this.fire(ev[this.cursor++]);
        if (this.t >= s.duration) {
          if (this.opts.onLoop) this.opts.onLoop(this);
          this.restart();
          if (s.captionReset && this.opts.onCaption) this.opts.onCaption(s.captionReset, s);
        }
      } else if (this.cursor === 0) {
        // Reduced motion: show one settled moment of the script, no travel.
        const T = s.staticAt ?? s.duration;
        s.script.forEach((e) => { if (e.t <= T && (e.show || e.hide || e.state || e.log)) this.fire({ show: e.show, hide: e.hide, state: e.state, log: e.log }); });
        const last = [...s.script].reverse().find((e) => e.cap && e.t <= T);
        if (last && this.opts.onCaption) this.opts.onCaption(last.cap, s);
        if (s.staticAt != null) this.t = T;
        this.cursor = 1;
      }
      const k = reduced ? 1 : 1 - Math.pow(0.004, dt);
      this.pool.forEach((p) => {
        p.x = lerp(p.x, p.spec.x, k); p.y = lerp(p.y, p.spec.y, k);
        p.r = lerp(p.r, p.spec.r, k);
        p.a = lerp(p.a, this.targetAlpha(p), reduced ? 1 : 1 - Math.pow(0.02, dt));
      });
      this.msgs = this.msgs.filter((m) => {
        const done = this.t - m.t0 >= m.dur;
        if (done && m.land) { const n = this.node(m.b); if (n) n.flash = this.t + 0.5; }
        return !done;
      });
    }

    draw() {
      const { ctx, W, H, S } = this;
      const s = this.scene;
      ctx.clearRect(0, 0, W, H);
      if (!s) return;

      if (s.decor) s.decor(ctx, this);

      // Wiring
      if (s.edges) {
        s.edges.forEach((group) => {
          ctx.strokeStyle = rgba(COL.ink, group.alpha ?? 0.1);
          ctx.lineWidth = group.width ?? 1;
          if (group.dash) ctx.setLineDash(group.dash.map((v) => v * S)); else ctx.setLineDash([]);
          ctx.beginPath();
          group.pairs.forEach(([a, b]) => {
            const na = this.node(a), nb = this.node(b);
            if (!na || !nb) return;
            const al = Math.min(na.a, nb.a);
            if (al < 0.05) return;
            ctx.moveTo(na.x, na.y); ctx.lineTo(nb.x, nb.y);
          });
          ctx.stroke();
        });
        ctx.setLineDash([]);
      }

      // Messages
      this.msgs.forEach((m) => {
        const a = this.node(m.a), b = this.node(m.b);
        if (!a || !b) return;
        const p = Math.min(1, (this.t - m.t0) / m.dur);
        const e = p < 0.5 ? 2 * p * p : 1 - Math.pow(-2 * p + 2, 2) / 2;
        const x = lerp(a.x, b.x, e), y = lerp(a.y, b.y, e);
        const e2 = Math.max(0, e - 0.12);
        const c = m.c === "ink" ? COL.ink : m.c === "green" ? COL.green : m.c === "mid" ? COL.mid : COL.accent;
        ctx.strokeStyle = rgba(c, 0.35);
        ctx.lineWidth = 1.4 * S;
        ctx.beginPath(); ctx.moveTo(lerp(a.x, b.x, e2), lerp(a.y, b.y, e2)); ctx.lineTo(x, y); ctx.stroke();
        ctx.fillStyle = rgba(c, 1);
        ctx.beginPath(); ctx.arc(x, y, 2.3 * S, 0, TAU); ctx.fill();
      });

      // Nodes
      this.pool.forEach((p) => {
        if (p.a < 0.02 || p.r < 0.2) return;
        const sp = p.spec;
        const active = p.active > this.t;
        const flash = p.flash > this.t;
        const hot = active || flash;
        ctx.globalAlpha = p.a;
        switch (sp.kind) {
          case "agent":
          case "coord": {
            if (active) {
              ctx.strokeStyle = rgba(COL.accent, 0.35);
              ctx.lineWidth = 1;
              ctx.beginPath(); ctx.arc(p.x, p.y, p.r + 4 * S + 2 * S * Math.sin(this.t * 6), 0, TAU); ctx.stroke();
            }
            ctx.fillStyle = rgba(hot ? COL.accent : (sp.tone === "mid" ? COL.mid : COL.ink), 1);
            ctx.beginPath(); ctx.arc(p.x, p.y, p.r, 0, TAU); ctx.fill();
            if (sp.kind === "coord") {
              ctx.strokeStyle = rgba(hot ? COL.accent : COL.ink, 0.5);
              ctx.lineWidth = 1;
              ctx.beginPath(); ctx.arc(p.x, p.y, p.r + 3 * S, 0, TAU); ctx.stroke();
            }
            if (sp.mem) {
              // The agent's own memory: an arc that belongs to the node and lights on use.
              // Write is ink (with a tick: a new record), read is grey, quarantine is orange;
              // each use sends a short pulse outward.
              const age = p.memAt != null ? this.t - p.memAt : 99;
              const lit = age < 0.9;
              const op = p.memOp;
              const rr = p.r + (sp.kind === "coord" ? 5.5 : 3) * S;
              const a0 = Math.PI * 0.62, a1 = Math.PI * 1.38;
              const col = lit ? (op === "quar" ? COL.accent : op === "read" ? COL.mid : COL.ink) : COL.mid;
              ctx.lineCap = "round";
              ctx.strokeStyle = rgba(col, lit ? 1 : 0.55);
              ctx.lineWidth = lit ? Math.max(2, 2.4 * S) : Math.max(1.3, 1.5 * S);
              ctx.beginPath(); ctx.arc(p.x, p.y, rr, a0, a1); ctx.stroke();
              if (lit && age < 0.4) {
                const q = age / 0.4;
                ctx.strokeStyle = rgba(col, 0.55 * (1 - q));
                ctx.lineWidth = Math.max(1, 1.2 * S);
                ctx.beginPath(); ctx.arc(p.x, p.y, rr + (2.5 + 5 * q) * S, a0 + 0.06, a1 - 0.06); ctx.stroke();
              }
              if (lit && op === "write") {
                ctx.strokeStyle = rgba(COL.ink, 1);
                ctx.lineWidth = Math.max(1.2, 1.4 * S);
                ctx.beginPath(); ctx.moveTo(p.x - rr + 1.5 * S, p.y); ctx.lineTo(p.x - rr - 4.5 * S, p.y); ctx.stroke();
              }
              ctx.lineCap = "butt";
            }
            break;
          }
          case "store": {
            // A knowledge store: stacked discs, drawn as a cylinder.
            const w = p.r * 2.4, h = p.r * 2.1, e = p.r * 0.45;
            ctx.fillStyle = rgba(COL.screen, 1);
            ctx.strokeStyle = rgba(hot ? COL.accent : COL.ink, 1);
            ctx.lineWidth = 1.2 * S;
            ctx.beginPath();
            ctx.ellipse(p.x, p.y - h / 2, w / 2, e, 0, Math.PI, 0);
            ctx.lineTo(p.x + w / 2, p.y + h / 2);
            ctx.ellipse(p.x, p.y + h / 2, w / 2, e, 0, 0, Math.PI);
            ctx.closePath(); ctx.fill(); ctx.stroke();
            ctx.beginPath(); ctx.ellipse(p.x, p.y - h / 2, w / 2, e, 0, 0, Math.PI); ctx.stroke();
            ctx.beginPath(); ctx.ellipse(p.x, p.y, w / 2, e, 0, 0, Math.PI); ctx.strokeStyle = rgba(hot ? COL.accent : COL.ink, 0.35); ctx.stroke();
            break;
          }
          case "helper": {
            ctx.fillStyle = rgba(COL.screen, 1);
            ctx.strokeStyle = rgba(hot ? COL.accent : COL.ink, 1);
            ctx.lineWidth = 1.3 * S;
            ctx.beginPath(); ctx.arc(p.x, p.y, p.r, 0, TAU); ctx.fill(); ctx.stroke();
            break;
          }
          case "person": {
            ctx.strokeStyle = rgba(hot ? COL.accent : COL.mid, 1);
            ctx.lineWidth = 1.4 * S;
            ctx.beginPath(); ctx.arc(p.x, p.y, p.r, 0, TAU); ctx.stroke();
            ctx.fillStyle = rgba(hot ? COL.accent : COL.mid, 1);
            ctx.beginPath(); ctx.arc(p.x, p.y, p.r * 0.38, 0, TAU); ctx.fill();
            break;
          }
          case "source": {
            const w = p.r * 2;
            ctx.strokeStyle = rgba(hot ? COL.accent : COL.mid, 1);
            ctx.lineWidth = 1.2 * S;
            ctx.strokeRect(p.x - w / 2, p.y - w / 2, w, w);
            if (hot) { ctx.fillStyle = rgba(COL.accent, 0.18); ctx.fillRect(p.x - w / 2, p.y - w / 2, w, w); }
            break;
          }
          case "item": {
            const w = p.r * 2;
            const st = p.state || "open";
            if (st === "open") { ctx.strokeStyle = rgba(COL.ink, 0.6); ctx.lineWidth = 1; ctx.strokeRect(p.x - w / 2 + 0.5, p.y - w / 2 + 0.5, w - 1, w - 1); }
            else if (st === "claimed") { ctx.fillStyle = rgba(COL.accent, 1); ctx.fillRect(p.x - w / 2, p.y - w / 2, w, w); }
            else if (st === "done") { ctx.fillStyle = rgba(COL.ink, 1); ctx.fillRect(p.x - w / 2, p.y - w / 2, w, w); }
            else if (st === "skip") { ctx.fillStyle = rgba(COL.faint, 0.35); ctx.fillRect(p.x - w / 2, p.y - w / 2, w, w); }
            else if (st === "quar") {
              ctx.strokeStyle = rgba(COL.accent, 1); ctx.lineWidth = 1.2;
              ctx.strokeRect(p.x - w / 2 + 0.6, p.y - w / 2 + 0.6, w - 1.2, w - 1.2);
              ctx.beginPath(); ctx.moveTo(p.x - w / 2 + 1, p.y + w / 2 - 1); ctx.lineTo(p.x + w / 2 - 1, p.y - w / 2 + 1); ctx.stroke();
            }
            break;
          }
          case "box": {
            const w = sp.w * S, h = sp.h * S;
            ctx.strokeStyle = rgba(hot ? COL.accent : COL.ink, hot ? 1 : 0.55);
            ctx.lineWidth = 1;
            ctx.fillStyle = rgba(COL.screen, 1);
            ctx.fillRect(p.x - w / 2 + 0.5, p.y - h / 2 + 0.5, w, h);
            ctx.strokeRect(p.x - w / 2 + 0.5, p.y - h / 2 + 0.5, w, h);
            if (sp.text) {
              ctx.fillStyle = rgba(COL.ink, 0.85);
              ctx.font = `500 ${Math.max(8.5, (sp.textSize || 10.5) * S)}px ${SANS}`;
              ctx.textAlign = "center"; ctx.textBaseline = "middle";
              ctx.fillText(sp.text, p.x, p.y + 0.5);
            }
            break;
          }
          case "frame": {
            const w = sp.w, h = sp.h;
            ctx.strokeStyle = rgba(COL.ink, 0.28);
            ctx.lineWidth = 1;
            ctx.setLineDash([3 * S, 3 * S]);
            ctx.strokeRect(p.x - w / 2 + 0.5, p.y - h / 2 + 0.5, w, h);
            ctx.setLineDash([]);
            break;
          }
          default:
            break;
        }
        ctx.globalAlpha = 1;
      });

      // Labels: a halo keeps them legible over wiring; gaps clear coordinator rings.
      ctx.lineJoin = "round";
      this.pool.forEach((p) => {
        const sp = p.spec;
        if (!sp.label || p.a < 0.3) return;
        if (!(this.opts.labels || sp.always)) return;
        const hot = p.active > this.t || p.flash > this.t;
        const mid = sp.labelTone === "mid";
        const fs = Math.max(9, (sp.labelSize || 10) * S);
        ctx.globalAlpha = p.a;
        ctx.font = `${sp.labelWeight || 500} ${fs}px ${SANS}`;
        const g = p.r + (sp.kind === "coord" ? 8 : 5) * S;
        const pos = sp.labelPos || "below";
        let x = p.x, y = p.y;
        ctx.textBaseline = "middle";
        if (pos === "below") { ctx.textAlign = "center"; y = p.y + g + fs / 2; }
        else if (pos === "above") { ctx.textAlign = "center"; y = p.y - g - fs / 2; }
        else if (pos === "left") { ctx.textAlign = "right"; x = p.x - g; }
        else if (pos === "right") { ctx.textAlign = "left"; x = p.x + g; }
        else if (pos === "frame") { ctx.textAlign = "left"; ctx.textBaseline = "top"; x = p.x - sp.w / 2 + 10 * S; y = p.y - sp.h / 2 + 9 * S; }
        ctx.strokeStyle = rgba(COL.screen, 0.92);
        ctx.lineWidth = 3;
        ctx.strokeText(sp.label, x, y);
        ctx.fillStyle = rgba(hot ? COL.accent : (mid ? COL.mid : COL.ink), hot || mid ? 1 : 0.85);
        ctx.fillText(sp.label, x, y);
        ctx.globalAlpha = 1;
      });

      if (s.overlay) s.overlay(ctx, this);
    }
  }

  window.MeerkatNet = { Net, COL, rgba, SANS };
})();
