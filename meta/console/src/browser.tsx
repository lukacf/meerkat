import { createConsoleApp } from "./index";

function resolveBaseUrl(): string {
  const configured = document
    .querySelector('meta[name="meta-base-url"]')
    ?.getAttribute("content")
    ?.trim();
  if (configured) {
    return configured.replace(/\/$/, "");
  }
  return window.location.origin;
}

function boot(): void {
  const root = document.getElementById("root");
  if (!root) {
    throw new Error("missing #root mount element");
  }
  // Dev only: the console kit gallery (?kit_gallery=1) with the app's own sections (console/src/dev/app-gallery.ts).
  // The production build defines NODE_ENV, so this branch, the gallery and the app sections are dropped.
  if (process.env.NODE_ENV !== "production" && new URLSearchParams(window.location.search).get("kit_gallery") === "1") {
    void Promise.all([import("../../packages/console-components/src/panel-kit/gallery/entry"), import("./dev/app-gallery")]).then(([gallery, app]) =>
      import("./console-host.css").then(() => gallery.mountKitGallery(root, app.appGallerySections())),
    );
    return;
  }
  createConsoleApp(root, { baseUrl: resolveBaseUrl() });
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", boot, { once: true });
} else {
  boot();
}
