import React from "react";
import { render, screen } from "@testing-library/react";

import type { BrowserDockTarget } from "@console-core";
import { BrowserDockTargetHost } from "@console-components";

describe("BrowserDockTargetHost", () => {
  test("forwards trusted host props while target identity remains controlled", () => {
    const target = {
      id: "browser-panel:panel-a",
      kind: "browser",
      title: "Browser",
      browserPanelId: "panel-a",
    } satisfies BrowserDockTarget;

    const { container } = render(
      <BrowserDockTargetHost
        aria-busy="true"
        className="host-layout"
        data-browser-dock-target-id="browser-panel:forged"
        data-browser-panel-id="forged"
        data-trusted-host-state="ready"
        target={target}
      >
        <div data-testid="host-browser-content">Trusted chrome and viewport marker</div>
      </BrowserDockTargetHost>,
    );

    const host = container.firstElementChild;
    expect(host).toHaveClass("host-layout");
    expect(host).toHaveAttribute("aria-busy", "true");
    expect(host).toHaveAttribute("data-trusted-host-state", "ready");
    expect(host).toHaveAttribute("data-browser-dock-target-id", "browser-panel:panel-a");
    expect(host).toHaveAttribute("data-browser-panel-id", "panel-a");
    expect(screen.getByTestId("host-browser-content")).toHaveTextContent(
      "Trusted chrome and viewport marker",
    );
    expect(container.querySelectorAll("button, input, iframe, webview")).toHaveLength(0);
  });
});
