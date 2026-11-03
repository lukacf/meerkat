import React from "react";
import "./native-bundle-selection.css";

export function NativeBundleSelection({ tools }) {
  const bundles = tools?.rust_bundles || [];
  if (!bundles.length) return null;
  return (
    <div className="native-bundle-selection">
      <strong>Native bundles (selected)</strong>
      <ul>{bundles.map((name) => <li key={name}><code>{name}</code></li>)}</ul>
      <p>The host must register these bundles and tool permissions must allow their use. Selecting a bundle does not make it ready to run.</p>
    </div>
  );
}
