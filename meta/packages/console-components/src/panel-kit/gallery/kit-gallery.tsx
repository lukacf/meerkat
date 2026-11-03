import { useEffect, useMemo, useState } from "react";

import { cx } from "../cx";
import { SegmentedControl } from "../toolbar";
import type { GallerySection } from "./types";

// Every `gallerySections` export under packages/console-components/src,
// this folder's own sections included (Vite resolves the glob at dev time).
// @ts-ignore import.meta.glob is Vite's; nothing outside the dev server loads this file.
const MODULES = import.meta.glob("../../**/*.gallery.tsx", { eager: true }) as Record<string, GalleryModule>;

/** A module found by a gallery glob. */
export interface GalleryModule {
  gallerySections?: ReadonlyArray<GallerySection>;
}

/** The sections a glob found, flattened in path order. */
export function collectGallerySections(modules: Record<string, GalleryModule>): GallerySection[] {
  return Object.keys(modules)
    .sort()
    .flatMap((key) => modules[key].gallerySections ?? []);
}

/**
 * The kit's sections and any extra ones (the console app's, which cannot live
 * in this package) in one list: `only` keeps one id, then the list sorts by
 * `order` (100 without one) and title. A repeated id keeps its first section.
 */
export function mergeGallerySections(
  kit: ReadonlyArray<GallerySection>,
  extra: ReadonlyArray<GallerySection> = [],
  only: string | null = null,
): GallerySection[] {
  const seen = new Set<string>();
  const merged: GallerySection[] = [];
  for (const section of [...kit, ...extra]) {
    if (seen.has(section.id)) {
      console.warn(`kit gallery: a second section with the id "${section.id}" is left out`);
      continue;
    }
    seen.add(section.id);
    merged.push(section);
  }
  return merged
    .filter((section) => !only || section.id === only)
    .sort((a, b) => (a.order ?? 100) - (b.order ?? 100) || a.title.localeCompare(b.title));
}

type Theme = "light" | "dark";
type Variant = "rams" | "terminal" | "graphite";
type Motion = "system" | "reduced";

function readParams(): { theme: Theme; variant: Variant; motion: Motion; only: string | null } {
  const params = new URLSearchParams(window.location.search);
  const theme = params.get("theme") === "light" ? "light" : "dark";
  const variantParam = params.get("variant");
  const variant: Variant = variantParam === "terminal" || variantParam === "graphite" ? variantParam : "rams";
  const motion: Motion = params.get("motion") === "reduced" ? "reduced" : "system";
  return { theme, variant, motion, only: params.get("section") };
}

function writeParam(key: string, value: string | null) {
  const url = new URL(window.location.href);
  if (value === null) url.searchParams.delete(key);
  else url.searchParams.set(key, value);
  window.history.replaceState(null, "", url.toString());
}

/**
 * The dev-only kit gallery (`?kit_gallery=1`): every console v2 kit part in
 * one page, in the theme, variant and motion the URL names (`theme`,
 * `variant`, `motion=reduced`; `section=<id>` shows one section).
 * `extraSections` (the console app's own, from console/src/dev/app-gallery.ts)
 * join the kit's in the same sorted list, under the same filter.
 */
export function KitGallery({ extraSections }: { extraSections?: ReadonlyArray<GallerySection> } = {}) {
  const initial = useMemo(readParams, []);
  const [theme, setTheme] = useState<Theme>(initial.theme);
  const [variant, setVariant] = useState<Variant>(initial.variant);
  const [motion, setMotion] = useState<Motion>(initial.motion);
  const sections = useMemo(
    () => mergeGallerySections(collectGallerySections(MODULES), extraSections, initial.only),
    [extraSections, initial.only],
  );
  useEffect(() => {
    // The console's dev page puts a dark theme scope on <body>: follow the
    // gallery's choice so the page around it matches.
    document.body.setAttribute("data-cc-theme", theme);
    document.body.setAttribute("data-cc-variant", variant);
  }, [theme, variant]);
  return (
    <div
      className={cx("cc-theme-scope", "kg", motion === "reduced" && "reduced-motion")}
      data-cc-theme={theme}
      data-cc-variant={variant}
      data-testid="kit-gallery"
    >
      <header className="kg__head">
        <div className="kg__intro">
          <div className="kg__eyebrow">Meerkat Meta console · v2</div>
          <h1 className="kg__title">Console kit</h1>
          <p className="kg__lede">
            The parts every workbench panel is built from: tokens and marks, the panel frame, menus, figures, micro charts,
            entities, states, the drawer and the filter bar. Warm ink on paper; orange means now.
          </p>
        </div>
        <div className="kg__controls">
          <SegmentedControl
            ariaLabel="Theme"
            value={theme}
            onChange={(next) => {
              setTheme(next);
              writeParam("theme", next);
            }}
            options={[
              { id: "light", label: "Light", glyph: "sun", testId: "kit-gallery-theme:light" },
              { id: "dark", label: "Dark", glyph: "moon", testId: "kit-gallery-theme:dark" },
            ]}
          />
          <SegmentedControl
            ariaLabel="Variant"
            value={variant}
            onChange={(next) => {
              setVariant(next);
              writeParam("variant", next === "rams" ? null : next);
            }}
            options={[
              { id: "rams", label: "Rams", testId: "kit-gallery-variant:rams" },
              { id: "terminal", label: "Terminal", testId: "kit-gallery-variant:terminal" },
              { id: "graphite", label: "Graphite", testId: "kit-gallery-variant:graphite" },
            ]}
          />
          <SegmentedControl
            ariaLabel="Motion"
            value={motion}
            onChange={(next) => {
              setMotion(next);
              writeParam("motion", next === "reduced" ? "reduced" : null);
            }}
            options={[
              { id: "system", label: "Motion", testId: "kit-gallery-motion:system" },
              { id: "reduced", label: "Reduced", testId: "kit-gallery-motion:reduced" },
            ]}
          />
        </div>
      </header>
      <nav className="kg__toc" aria-label="Sections">
        {sections.map((section) => (
          <a key={section.id} className="kg__toc-link" href={`#kg-${section.id}`}>
            {section.title}
          </a>
        ))}
      </nav>
      <main className="kg__main">
        {sections.map((section) => (
          <section key={section.id} className="kg__section" id={`kg-${section.id}`} data-testid={`kit-gallery-section:${section.id}`}>
            <div className="kg__section-head">
              <h2 className="kg__section-title">{section.title}</h2>
              {section.description ? <p className="kg__section-note">{section.description}</p> : null}
            </div>
            <div className="kg__section-body">{section.render()}</div>
          </section>
        ))}
      </main>
    </div>
  );
}
