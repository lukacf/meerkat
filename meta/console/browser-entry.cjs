const { createConsoleApp } = require("./index.cjs");

function resolveBaseUrl() {
  const configured = document
    .querySelector('meta[name="meta-base-url"]')
    ?.getAttribute("content")
    ?.trim();
  if (configured) {
    return configured.replace(/\/$/, "");
  }
  return window.location.origin;
}

function boot() {
  const root = document.getElementById("root");
  if (!root) {
    throw new Error("missing #root mount element");
  }
  createConsoleApp(root, { baseUrl: resolveBaseUrl() });
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", boot, { once: true });
} else {
  boot();
}
