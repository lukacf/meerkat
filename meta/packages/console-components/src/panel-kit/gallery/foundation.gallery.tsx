import type { CSSProperties } from "react";

import { KIT_GLYPH_NAMES, KitGlyph } from "../glyphs";
import { Item, Specimen, SpecimenRow } from "./specimen";
import type { GallerySection } from "./types";

const PALETTE: Array<[string, string]> = [
  ["Canvas", "--_cc-surface-canvas"],
  ["Card", "--_cc-surface-card"],
  ["Raised", "--_cc-surface-raised"],
  ["Soft", "--_cc-surface-soft"],
  ["Overlay", "--_cc-surface-overlay"],
  ["Ink strong", "--_cc-foreground-strong"],
  ["Ink", "--_cc-foreground"],
  ["Ink muted", "--_cc-foreground-muted"],
  ["Ink meta", "--_cc-foreground-meta"],
  ["Rule soft", "--_cc-border-soft"],
  ["Rule strong", "--_cc-border-strong"],
  ["Link / info", "--_cc-link"],
  ["Positive", "--_cc-positive"],
  ["Warning", "--_cc-warning"],
  ["Negative", "--_cc-negative"],
  ["Accent (now)", "--_cc-member-accent"],
  ["Accent soft", "--_cc-member-accent-soft"],
  ["Accent ink", "--_cc-accent-ink"],
];

const VIZ: Array<[string, string]> = [
  ["viz ink", "--_cc-viz-ink"],
  ["viz mid", "--_cc-viz-mid"],
  ["viz faint", "--_cc-viz-faint"],
  ["viz edge", "--_cc-viz-edge"],
  ["viz edge strong", "--_cc-viz-edge-strong"],
  ["viz grid", "--_cc-viz-grid"],
  ["viz live", "--_cc-viz-live"],
  ["viz live soft", "--_cc-viz-live-soft"],
];

const TYPE: Array<[string, string, CSSProperties, string]> = [
  ["--_cc-fs-hero", "28 / 600", { fontSize: "var(--_cc-fs-hero)", fontWeight: 600, letterSpacing: "-0.02em", lineHeight: 1.1 }, "1,284"],
  ["--_cc-fs-figure", "20 / 600", { fontSize: "var(--_cc-fs-figure)", fontWeight: 600, letterSpacing: "-0.01em" }, "48 messages"],
  ["--_cc-fs-entity", "18 / 600", { fontSize: "var(--_cc-fs-entity)", fontWeight: 600, letterSpacing: "-0.01em" }, "Support Triage"],
  ["--_cc-panel-fs-title", "15 / 600", { fontSize: "var(--_cc-panel-fs-title)", fontWeight: 600, letterSpacing: "-0.01em" }, "Topology"],
  ["--_cc-fs-card", "13 / 600", { fontSize: "var(--_cc-fs-card)", fontWeight: 600 }, "Ship the 2.4 release"],
  ["--_cc-panel-fs-body", "13 / 400-500", { fontSize: "var(--_cc-panel-fs-body)" }, "Replying to a support email about a refund"],
  ["--_cc-fs-control", "12 / 500", { fontSize: "var(--_cc-fs-control)", fontWeight: 500 }, "Open chat"],
  ["--_cc-panel-fs-meta", "11.5 / 400", { fontSize: "var(--_cc-panel-fs-meta)", color: "var(--_cc-foreground-meta)" }, "8 agents · Updated 12s ago"],
  ["--_cc-fs-graph", "11 / 500", { fontSize: "var(--_cc-fs-graph)", fontWeight: 500 }, "Billing"],
  ["--_cc-panel-fs-label", "10.5 / 600 caps", { fontSize: "var(--_cc-panel-fs-label)", fontWeight: 600, letterSpacing: "0.08em", textTransform: "uppercase", color: "var(--_cc-foreground-meta)" }, "Messages · last hour"],
  ["--_cc-fs-lane", "10 / 600 caps", { fontSize: "var(--_cc-fs-lane)", fontWeight: 600, letterSpacing: "0.1em", textTransform: "uppercase", color: "var(--_cc-foreground-meta)" }, "Coordinators"],
  ["--_cc-fs-tick", "9.5 mono", { fontSize: "var(--_cc-fs-tick)", fontFamily: "var(--_cc-font-mono)", color: "var(--_cc-foreground-meta)" }, "00   06   12   18   now"],
];

function Swatch({ name, token }: { name: string; token: string }) {
  return (
    <div className="kg-swatch">
      <span className="kg-swatch__chip" style={{ background: `var(${token})` }} />
      <span className="kg-swatch__text">
        <span className="kg-swatch__name">{name}</span>
        <span className="kg-swatch__token">{token}</span>
      </span>
    </div>
  );
}

// The magnitude ramp, light to dark, named in full (the stylesheet guard
// reads every var() name in the source).
const SEQ = [
  "var(--_cc-viz-seq-1)",
  "var(--_cc-viz-seq-2)",
  "var(--_cc-viz-seq-3)",
  "var(--_cc-viz-seq-4)",
  "var(--_cc-viz-seq-5)",
  "var(--_cc-viz-seq-6)",
  "var(--_cc-viz-seq-7)",
];

export const gallerySections: GallerySection[] = [
  {
    id: "tokens",
    title: "Tokens",
    order: 10,
    description: "A2: one palette with semantic roles, derived viz inks, the type scale, a 4 px grid, four elevations.",
    render: () => (
      <>
        <SpecimenRow>
          <Specimen title="Palette" note="surfaces, inks, rules, tones and the accent (hue 40)">
            <div className="kg-grid">
              {PALETTE.map(([name, token]) => (
                <Swatch key={token} name={name} token={token} />
              ))}
            </div>
          </Specimen>
        </SpecimenRow>
        <SpecimenRow>
          <Specimen title="Visualisation" note="foreground mixed toward the card; one hue">
            <div className="kg-grid">
              {VIZ.map(([name, token]) => (
                <Swatch key={token} name={name} token={token} />
              ))}
            </div>
          </Specimen>
          <Specimen title="Magnitude ramp" note="viz-seq-1 to 7, light to dark">
            <div className="kg-inline">
              {SEQ.map((token, index) => (
                <Item key={token} label={`seq-${index + 1}`}>
                  <span className="kg-swatch__chip" style={{ background: token }} />
                </Item>
              ))}
            </div>
          </Specimen>
        </SpecimenRow>
        <Specimen title="Type scale" note="Inter Tight; IBM Plex Mono for machine values">
          <div className="kg-type">
            {TYPE.map(([token, spec, style, sample]) => (
              <div key={token} style={{ display: "contents" }}>
                <span className="kg-type__token">
                  {token} · {spec}
                </span>
                <span style={style}>{sample}</span>
              </div>
            ))}
          </div>
        </Specimen>
        <SpecimenRow>
          <Specimen title="Spacing" note="--_cc-sp-1 to 11: 2 4 6 8 12 16 20 24 32 40 48">
            <div className="kg-space">
              {[2, 4, 6, 8, 12, 16, 20, 24, 32, 40, 48].map((size, index) => (
                <Item key={size} label={`${index + 1}`}>
                  <span className="kg-space__bar" style={{ height: size }} />
                </Item>
              ))}
            </div>
          </Specimen>
          <Specimen title="Elevation" note="e0 flat, e1 rule, e2 overlay, e3 drawer">
            <div className="kg-elev">
              {["e0 panel body", "e1 card", "e2 popover", "e3 drawer"].map((name, level) => (
                <div key={name} className="kg-elev__box" data-level={level}>
                  {name}
                </div>
              ))}
            </div>
          </Specimen>
        </SpecimenRow>
      </>
    ),
  },
  {
    id: "icons",
    title: "Icons",
    order: 15,
    description: "A2.10: KitGlyph, the same 24 px stroke paths as the console sprite (i-<name>).",
    render: () => (
      <Specimen>
        <div className="kg-grid" style={{ gridTemplateColumns: "repeat(auto-fill, minmax(112px, 1fr))" }}>
          {KIT_GLYPH_NAMES.map((name) => (
            <Item key={name} label={name}>
              <KitGlyph name={name} className="kg-icon" />
            </Item>
          ))}
        </div>
      </Specimen>
    ),
  },
];
