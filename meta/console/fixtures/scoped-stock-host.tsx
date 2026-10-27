import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { ConsoleApp, createConsoleApp } from "../src/index";
import "../src/console-host.css";
import "@console-components/styles";
import { acceptanceMarkdownUrlPolicy } from "./markdown-url-policy";

// The local test host owns this namespace. Production hosts must supply their
// authenticated authority/realm/principal namespace through the public API.
if (new URLSearchParams(location.search).get("runtime-scope-switch") === "1") {
  // This test-only host changes the public ConsoleApp authority props while
  // keeping the browser page mounted. Both proxy targets are real runtimes.
  function ScopeSwitchHost() {
    const [scope, setScope] = useState("scope-a");
    const baseUrl = `${location.origin}/${scope}`;
    return <div style={{ height: "100%", display: "flex", flexDirection: "column" }}>
      <header style={{ display: "flex", gap: 12, padding: 8 }}>
        <button type="button" onClick={() => setScope(value => value === "scope-a" ? "scope-b" : "scope-a")}>Change host scope</button>
        <span data-testid="host-scope">{scope}</span>
      </header>
      <div style={{ flex: 1, minHeight: 0 }}>
        <ConsoleApp baseUrl={baseUrl} storageNamespace={`${baseUrl}/acceptance-realm/operator-a`}
          markdownUrlPolicy={acceptanceMarkdownUrlPolicy()} />
      </div>
    </div>;
  }
  createRoot(document.getElementById("root")!).render(<ScopeSwitchHost />);
} else {
  createConsoleApp(document.getElementById("root"), {
    baseUrl: location.origin,
    storageNamespace: `${location.origin}/acceptance-realm/operator-a`,
    markdownUrlPolicy: acceptanceMarkdownUrlPolicy(),
  });
}
