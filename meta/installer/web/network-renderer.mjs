/* Network engine for the Meerkat candidate page.
   Draws agent topologies on a canvas and plays their dynamics from a script.
   Scenes are pure functions of the canvas size; nodes morph between scenes. */



  const TAU = Math.PI * 2;
  const lerp = (a, b, t) => a + (b - a) * t;


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
      this.alive = true; this.reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
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
      this.ro = new ResizeObserver(() => this.resize()); this.ro.observe(canvas);
      this.resize();
      this.io = new IntersectionObserver(([e]) => {
        const was = this.visible;
        this.visible = e.isIntersecting;
        if (this.visible && !was) { this.last = performance.now(); requestAnimationFrame((n) => this.frame(n)); }
      });
      this.io.observe(canvas);
      this.visibilityHandler = () => { this.running = !document.hidden; };
      document.addEventListener("visibilitychange", this.visibilityHandler);
      requestAnimationFrame((n) => this.frame(n));
    }

    destroy() { this.alive = false; this.ro.disconnect(); this.io.disconnect(); document.removeEventListener("visibilitychange", this.visibilityHandler); }

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
      if (o.burstFrom && !this.reducedMotion) {
        const [bx, by] = typeof o.burstFrom === "function" ? o.burstFrom(this) : o.burstFrom;
        this.pool.forEach((p) => { p.x = bx; p.y = by; p.r = 0.5; p.a = 0; });
      }
    }

    build(reset) {
      const s = this.factory(this.W, this.H, this.S, this.opts);
      while (this.pool.length < s.nodes.length) this.pool.push({x:0,y:0,r:0,a:0,spec:null,shown:true,active:0,flash:0,state:null});
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
        // A hidden first mount has no geometry. Replace every interpolated
        // value on its first measured layout so invalid radii cannot persist.
        if (place) { p.x = p.spec.x; p.y = p.spec.y; p.r = p.spec.r; p.a = this.targetAlpha(p); }
      });
      if (place) this.placed = true;
      if (reset) this.restart();
      if (this.reducedMotion || this.paused) this.snap();
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

    seek(time) {
      this.restart();
      this.t = Math.max(0, Math.min(time, this.scene.duration - .001));
      while (this.cursor < this.scene.script.length && this.scene.script[this.cursor].t <= this.t) this.fire(this.scene.script[this.cursor++]);
      this.msgs = this.msgs.filter(m => {
        if (this.t < m.t0 + m.dur) return true;
        const n = this.node(m.b); if (n) n.flash = m.t0 + m.dur + .5;
        return false;
      });
      this.snap(); this.draw();
      this.opts.onFrame?.(this.t, this.scene.duration);
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
          this.msgs.push({ a: from, b: to, edgeId:ev.edgeId, t0: ev.t ?? this.t, dur, c: ev.c || "accent", land: ev.land !== false });
        }
      }
      if (ev.on) [].concat(ev.on).forEach((id) => { const n = this.node(id); if (n) n.active = (ev.t ?? this.t) + (ev.d ?? 1.2); });
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
      if (!this.alive) return;
      const dt = Math.min((now - this.last) / 1000, 0.05);
      this.last = now;
      if (this.scene && this.running && !this.paused) this.step(dt);
      this.draw();
      if (this.scene && this.opts.onFrame) this.opts.onFrame(this.t, this.scene.duration);
      if (this.visible) requestAnimationFrame((n) => this.frame(n));
    }

    step(dt) {
      const s = this.scene;
      this.t += dt * this.opts.speed;
      const ev = s.script;
      while (this.cursor < ev.length && ev[this.cursor].t <= this.t) this.fire(ev[this.cursor++]);
      if (this.t >= s.duration) {
        if (this.opts.onLoop) this.opts.onLoop(this);
        this.restart();
      }
      const k = this.reducedMotion ? 1 : 1 - Math.pow(0.004, dt);
      this.pool.forEach((p) => {
        p.x = lerp(p.x, p.spec.x, k); p.y = lerp(p.y, p.spec.y, k);
        p.r = lerp(p.r, p.spec.r, k);
        p.a = lerp(p.a, this.targetAlpha(p), this.reducedMotion ? 1 : 1 - Math.pow(0.02, dt));
      });
      this.msgs = this.msgs.filter((m) => {
        const done = this.t - m.t0 >= m.dur;
        if (done && m.land) { const n = this.node(m.b); if (n) n.flash = m.t0 + m.dur + 0.5; }
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
          if(s.edgeVisible && !s.edgeVisible(group,this)) return;
          ctx.strokeStyle = rgba(COL.ink, s.edgeAlpha?.(group,this) ?? group.alpha ?? 0.1);
          ctx.lineWidth = group.width ?? 1;
          if (group.dash) ctx.setLineDash(group.dash.map((v) => v * S)); else ctx.setLineDash([]);
          ctx.beginPath();
          group.pairs.forEach(([a, b]) => {
            const na = this.node(a), nb = this.node(b);
            if (!na || !nb) return;
            const al = Math.min(na.a, nb.a);
            if (al < 0.05) return;
            const points=s.routePoints?.(group.edgeId,na,nb,this) || [na,nb];
            ctx.moveTo(points[0].x, points[0].y);
            points.slice(1).forEach(p=>ctx.lineTo(p.x,p.y));
          });
          ctx.stroke();
        });
        ctx.setLineDash([]);
      }

      // Messages
      this.msgs.forEach((m) => {
        const a = this.node(m.a), b = this.node(m.b);
        if (!a || !b) return;
        const p = this.reducedMotion ? .5 : Math.min(1, (this.t - m.t0) / m.dur);
        const e = p < 0.5 ? 2 * p * p : 1 - Math.pow(-2 * p + 2, 2) / 2;
        const points=s.routePoints?.(m.edgeId,a,b,this) || [a,b];
        const lengths=points.slice(1).map((point,i)=>Math.hypot(point.x-points[i].x,point.y-points[i].y));
        const total=lengths.reduce((sum,n)=>sum+n,0);
        const at=q=>{let distance=q*total;for(let i=0;i<lengths.length;i++){if(distance<=lengths[i] || i===lengths.length-1){const t=lengths[i]?distance/lengths[i]:0;return {x:lerp(points[i].x,points[i+1].x,t),y:lerp(points[i].y,points[i+1].y,t)};}distance-=lengths[i];}return points[0];};
        const {x,y}=at(e);
        const e2 = Math.max(0, e - 0.12);
        const c = m.c === "ink" ? COL.ink : m.c === "green" ? COL.green : m.c === "mid" ? COL.mid : COL.accent;
        ctx.strokeStyle = rgba(c, 0.35);
        ctx.lineWidth = 1.4 * S;
        const tail=at(e2);
        ctx.beginPath(); ctx.moveTo(tail.x,tail.y); ctx.lineTo(x, y); ctx.stroke();
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
              ctx.beginPath(); ctx.arc(p.x, p.y, p.r + 4 * S + 2 * S * (this.reducedMotion ? 0 : Math.sin(this.t * 6)), 0, TAU); ctx.stroke();
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

      // Reserve semantic headings first, then place wrapped identity labels.
      ctx.lineJoin = "round";
      const labelBoxes = [...(s.labelObstacles?.() || [])];
      const intersects=(a,b)=>a.x<b.x+b.w && a.x+a.w>b.x && a.y<b.y+b.h && a.y+a.h>b.y;
      this.pool.forEach((p) => {
        const sp=p.spec;
        if(!sp.label || p.a<.3 || !(this.opts.labels || sp.always))return;
        const hot=p.active>this.t || p.flash>this.t;
        const fs=sp.labelSize || Math.max(11,Math.min(14,12*S)), line=fs*1.22;
        ctx.font=`${hot?600:500} ${fs}px ${SANS}`;
        const maxWidth=sp.labelWidth || Math.min(148,Math.max(86,W*.19));
        const words=sp.label.split(/\s+/), lines=[];
        for(const word of words){
          if(!lines.length || ctx.measureText(`${lines.at(-1)} ${word}`).width>maxWidth)lines.push(word);
          else lines[lines.length-1]+=` ${word}`;
        }
        const tw=Math.max(...lines.map(str=>ctx.measureText(str).width)),height=lines.length*line;
        const candidates=[sp.labelPos || 'below','below','above','right','left'].filter((v,i,a)=>a.indexOf(v)===i);
        let chosen;
        for(let offset=0;offset<4&&!chosen;offset++)for(const side of candidates){
          const gap=p.r+7+offset*10;
          let x=p.x-tw/2,y=p.y+gap;
          if(side==='above')y=p.y-gap-height;
          if(side==='left'){x=p.x-gap-tw;y=p.y-height/2;}
          if(side==='right'){x=p.x+gap;y=p.y-height/2;}
          x=Math.max(6,Math.min(W-tw-6,x));
          const b={x:x-4,y:y-3,w:tw+8,h:height+6};
          if(y<5 || y+height>H-5 || labelBoxes.some(other=>intersects(b,other)))continue;
          chosen={x,y,b};break;
        }
        if(!chosen)return;
        labelBoxes.push(chosen.b);
        ctx.globalAlpha=p.a;ctx.textAlign='center';ctx.textBaseline='top';
        ctx.strokeStyle=rgba(COL.screen,.98);ctx.lineWidth=4;
        ctx.fillStyle=rgba(hot?COL.accent:COL.ink,hot?1:.83);
        lines.forEach((text,i)=>{ctx.strokeText(text,chosen.x+tw/2,chosen.y+i*line);ctx.fillText(text,chosen.x+tw/2,chosen.y+i*line);});
        ctx.globalAlpha=1;
      });

      if (s.overlay) s.overlay(ctx, this);
    }
  }

export { Net, COL, rgba, SANS };
