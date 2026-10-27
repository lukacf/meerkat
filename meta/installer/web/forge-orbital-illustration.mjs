// Meta Forge - first-run orbital illustration.
// Adapted from our Claude Design first-run exploration:
// https://claude.ai/design/p/e6b45339-dfc7-48ed-a94f-f36e6a79e5da
// Decorative only. It never represents real agents, counts or status, and it is
// not a schema for future topologies. Once a proposal exists, the agent-authored
// custom animation replaces it.
//
// App usage:
//   import React from "react";
//   import { makeOrbitalIllustration } from "./orbital-illustration.js";
//   const OrbitalIllustration = makeOrbitalIllustration(React);
//   <OrbitalIllustration topics={{ purpose: "done", people: "active" }} energy={0.6} />
//
// topics: { purpose|people|scale|responsibilities: "pending" | "active" | "done" }
//   These are conversation topics the guide covers before any services or APIs.
// energy: 0..1, how much of the idea exists so far (draft length, decisions made).
// mini: simplified, heavier marks for small sizes (phone band, under ~160px).

export const ORBITAL_TOPICS = [
  { id: "purpose", label: "PURPOSE", from: 0, to: 90 },
  { id: "people", label: "PEOPLE", from: 90, to: 180 },
  { id: "scale", label: "SCALE", from: 180, to: 270 },
  { id: "responsibilities", label: "RESPONSIBILITIES", from: 270, to: 360 },
];

const INK = "#1c1c1a", RUST = "#c4572a", RUST_TEXT = "#a8481f", SOFT = "#b9b5ab", MUTED = "#6f6a5f", PAPER = "#f1eee7";

const xy = (r, deg) => {
  const a = (deg - 90) * Math.PI / 180;
  return [+(r * Math.cos(a)).toFixed(2), +(r * Math.sin(a)).toFixed(2)];
};
const range = (n) => Array.from({ length: n }, (_, i) => i);
function arc(r, a, b, reverse) {
  const [x1, y1] = xy(r, reverse ? b : a), [x2, y2] = xy(r, reverse ? a : b);
  return `M ${x1} ${y1} A ${r} ${r} 0 0 ${reverse ? 0 : 1} ${x2} ${y2}`;
}
const clamp = (v) => Math.max(0, Math.min(1, v));

export function makeOrbitalIllustration(React) {
  const h = React.createElement;
  const spin = (calm, seconds, reverse) => calm ? null : h("animateTransform", {
    attributeName: "transform", type: "rotate", dur: `${seconds}s`, repeatCount: "indefinite",
    from: reverse ? "360" : "0", to: reverse ? "0" : "360",
  });
  const prefersStill = () => globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;

  function Ticks({ topics, mini }) {
    const r = mini ? 236 : 246, step = mini ? 7.5 : 3.4, size = mini ? 15 : 5.2;
    return ORBITAL_TOPICS.map((topic) => {
      const status = topics[topic.id] || "pending";
      const marks = [];
      for (let d = topic.from + 5; d <= topic.to - 5 + 0.01; d += step) {
        marks.push(h("rect", {
          key: d.toFixed(1), x: -size / 2, y: -r - size / 2, width: size, height: size,
          transform: `rotate(${d.toFixed(2)})`,
          fill: status === "pending" ? PAPER : status === "active" ? RUST : INK,
          stroke: status === "pending" ? SOFT : "none", strokeWidth: mini ? 2.6 : 1,
          style: { transition: "fill .5s, stroke .5s" },
        }));
      }
      return h("g", { key: topic.id }, marks);
    });
  }

  function Labels({ topics, uid }) {
    return ORBITAL_TOPICS.map((topic) => {
      const status = topics[topic.id] || "pending";
      const lower = topic.from >= 90 && topic.to <= 270;
      const id = `${uid}-${topic.id}`;
      return h("g", { key: topic.id },
        h("path", { id, d: arc(lower ? 275 : 262, topic.from + 3, topic.to - 3, lower), fill: "none" }),
        h("text", {
          fontFamily: "'Geist Mono', ui-monospace, monospace", fontSize: 14, letterSpacing: 2.6,
          fill: status === "active" ? RUST_TEXT : status === "done" ? INK : MUTED,
          fontWeight: status === "active" ? 500 : 400, style: { transition: "fill .5s" },
        }, h("textPath", { href: `#${id}`, startOffset: "50%", textAnchor: "middle" }, topic.label)));
    });
  }

  function Satellite({ x, y, ring, count, calm, seconds, open = true, core = 5 }) {
    const dist = Math.hypot(x, y), ux = x / dist, uy = y / dist;
    return h("g", null,
      h("line", {
        x1: +(ux * 214).toFixed(1), y1: +(uy * 214).toFixed(1),
        x2: +(x - ux * (ring + 8)).toFixed(1), y2: +(y - uy * (ring + 8)).toFixed(1),
        stroke: INK, strokeOpacity: 0.32, strokeWidth: 1, strokeDasharray: "2 5",
      }),
      h("g", { transform: `translate(${x} ${y})` },
        h("g", null, spin(calm, seconds, !open), range(count).map((i) => {
          const [cx, cy] = xy(ring, (360 / count) * i);
          return h("circle", open
            ? { key: i, cx, cy, r: 1.9, fill: PAPER, stroke: INK, strokeWidth: 0.9 }
            : { key: i, cx, cy, r: 1.5, fill: INK });
        })),
        h("circle", { r: core * 2.2, fill: PAPER, stroke: INK, strokeWidth: 0.9 }),
        h("circle", { r: core, fill: INK })));
  }

  return function OrbitalIllustration({
    topics = {}, energy = 0.4, still = false, mini = false, labels = !mini,
    title = "Illustration of a system taking shape. Not a proposal.", className, style,
  }) {
    const uid = React.useId();
    const [reducedMotion, setReducedMotion] = React.useState(prefersStill);
    React.useEffect(() => {
      const media = globalThis.matchMedia?.("(prefers-reduced-motion: reduce)");
      if (!media) return;
      const update = () => setReducedMotion(media.matches === true);
      media.addEventListener?.("change", update);
      update();
      return () => media.removeEventListener?.("change", update);
    }, []);
    const calm = still || reducedMotion;
    const e = clamp(energy);
    const svg = (viewBox, ...children) => h("svg", {
      viewBox, role: "img", "aria-label": title, className, focusable: "false",
      style: { display: "block", width: "100%", height: "100%", overflow: "visible", ...style },
    }, ...children);

    if (mini) {
      return svg("-262 -262 524 524",
        h(Ticks, { topics, mini: true }),
        h("g", null, spin(calm, 160), range(22).map((i) => {
          const [cx, cy] = xy(156, (360 / 22) * i);
          return h("circle", { key: i, cx, cy, r: 7, fill: PAPER, stroke: INK, strokeWidth: 2.6 });
        })),
        h("circle", { r: 96, fill: "none", stroke: INK, strokeWidth: 2.6, strokeDasharray: "3 13", strokeOpacity: 0.7 }),
        !calm && h("circle", { r: 44, fill: "none", stroke: RUST, strokeWidth: 3 },
          h("animate", { attributeName: "r", values: "44;100", dur: "4.8s", repeatCount: "indefinite" }),
          h("animate", { attributeName: "stroke-opacity", values: "0.55;0", dur: "4.8s", repeatCount: "indefinite" })),
        h("circle", { r: 44, fill: PAPER, stroke: RUST, strokeWidth: 3 }),
        h("circle", { r: 20, fill: RUST }));
    }

    return svg("-340 -340 680 680",
      h("circle", { r: 294, fill: "none", stroke: "#d6d2c8", strokeWidth: 1 }),
      h("g", { style: { opacity: clamp((e - 0.25) / 0.5), transition: "opacity .9s" } },
        h(Satellite, { x: -258, y: 236, ring: 30, count: 14, calm, seconds: 90 }),
        h(Satellite, { x: 262, y: -244, ring: 20, count: 10, calm, seconds: 70, open: false, core: 3.6 }),
        h(Satellite, { x: -306, y: 24, ring: 13, count: 8, calm, seconds: 60, core: 2.6 })),
      h(Ticks, { topics }),
      labels && h(Labels, { topics, uid }),
      h("g", { style: { opacity: 0.3 + 0.7 * e, transition: "opacity .9s" } }, spin(calm, 260),
        range(96).map((i) => {
          const [cx, cy] = xy(208, 3.75 * i);
          return h("circle", { key: i, cx, cy, r: 2.3, fill: PAPER, stroke: INK, strokeWidth: 0.9 });
        })),
      h("g", { style: { opacity: 0.55 + 0.45 * e, transition: "opacity .9s" } }, spin(calm, 340, true),
        range(64).map((i) => {
          const [cx, cy] = xy(174, 5.625 * i);
          return h("circle", { key: i, cx, cy, r: 1.7, fill: INK, fillOpacity: 0.8 });
        })),
      h("g", null, spin(calm, 200),
        range(44).map((i) => {
          const [x1, y1] = xy(30, (360 / 44) * i), [x2, y2] = xy(126, (360 / 44) * i);
          return h("line", { key: `l${i}`, x1, y1, x2, y2, stroke: INK, strokeOpacity: 0.09, strokeWidth: 0.8 });
        }),
        range(44).map((i) => {
          const [cx, cy] = xy(132, (360 / 44) * i);
          return h("circle", { key: `c${i}`, cx, cy, r: 3.1, fill: PAPER, stroke: i % 11 === 3 ? RUST : INK, strokeWidth: 1 });
        })),
      h("circle", { r: 92, fill: "none", stroke: INK, strokeOpacity: 0.45, strokeWidth: 1, strokeDasharray: "1 5.5" }),
      h("g", null, spin(calm, 150, true),
        range(18).map((i) => {
          const [cx, cy] = xy(60, 20 * i);
          return h("circle", { key: i, cx, cy, r: 1.6, fill: INK });
        })),
      !calm && h("circle", { r: 24, fill: "none", stroke: RUST, strokeWidth: 1 },
        h("animate", { attributeName: "r", values: "24;58", dur: "4.8s", repeatCount: "indefinite" }),
        h("animate", { attributeName: "stroke-opacity", values: "0.55;0", dur: "4.8s", repeatCount: "indefinite" })),
      h("circle", { r: 24, fill: PAPER, stroke: RUST, strokeWidth: 1 }),
      h("circle", { r: 10, fill: RUST }));
  };
}
