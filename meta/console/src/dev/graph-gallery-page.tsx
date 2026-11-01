// Dev-only page for the graph kit's gallery section, served by `npm run dev`
// at /graph-gallery.html (build.cjs never builds it). The same URL options as
// K1's kit gallery (?kit_gallery=1): &theme=light|dark,
// &variant=rams|terminal|graphite, &motion=reduced. Once that gallery is in
// the tree it shows this section too, and this page can go.

import { createRoot } from "react-dom/client";

// The console's own stylesheet (fonts, the icon reset, the kit), so the
// gallery draws exactly as the console does.
import "@console-components/styles";
import "../console-host.css";
import { gallerySections } from "../../../packages/console-components/src/viz/graph/graph.gallery";

function GalleryPage() {
  const params = new URLSearchParams(window.location.search);
  const theme = params.get("theme") === "dark" ? "dark" : "light";
  const variant = ["terminal", "graphite"].includes(params.get("variant") ?? "") ? params.get("variant")! : "rams";
  const reduced = params.get("motion") === "reduced";
  document.body.style.margin = "0";
  return (
    <div
      className={`cc-theme-scope${reduced ? " reduced-motion" : ""}`}
      data-cc-theme={theme}
      data-cc-variant={variant}
      style={{
        minHeight: "100vh",
        padding: "24px 32px 48px",
        boxSizing: "border-box",
        background: "var(--_cc-surface-canvas)",
        color: "var(--_cc-foreground)",
        fontFamily: "var(--_cc-font-display)",
      }}
    >
      <div style={{ maxWidth: 1100, margin: "0 auto" }}>
        {[...gallerySections].sort((a, b) => (a.order ?? 0) - (b.order ?? 0)).map((section) => (
          <section key={section.id} data-testid={`kit-gallery-section:${section.id}`}>
            <h2 style={{ fontSize: 15, fontWeight: 600, margin: "0 0 4px" }}>{section.title}</h2>
            {section.description ? <p style={{ margin: "0 0 16px", color: "var(--_cc-foreground-meta)", fontSize: 12 }}>{section.description}</p> : null}
            {section.render()}
          </section>
        ))}
      </div>
    </div>
  );
}

if (import.meta.env?.DEV) {
  const root = document.getElementById("root");
  if (root) createRoot(root).render(<GalleryPage />);
}
