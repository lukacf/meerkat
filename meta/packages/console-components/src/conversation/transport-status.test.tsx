import { render, screen, fireEvent } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ConsoleTransportStatus, consoleTransportLabel } from "./transport-status";

describe("connection status", () => {
  it("requires a completed snapshot before claiming live freshness", () => {
    expect(consoleTransportLabel({ phase: "connected-replaying", stale: true, freshness: "replaying" })).toBe("Connected, checking history");
    expect(consoleTransportLabel({ phase: "live", stale: false, freshness: "unknown" })).toBe("Connected, freshness unknown");
    expect(consoleTransportLabel({ phase: "live", stale: false, freshness: "current" })).toBe("Live");
  });
  it("offers explicit retry for stopped updates without turning an access denial into a retry loop", () => {
    const retry = vi.fn();
    const view = render(<ConsoleTransportStatus state={{ phase: "stopped", stale: true, freshness: "unknown" }} onRetry={retry} />);
    fireEvent.click(screen.getByRole("button", { name: "Reconnect" }));
    expect(retry).toHaveBeenCalledTimes(1);
    view.rerender(<ConsoleTransportStatus state={{ phase: "forbidden", stale: true, freshness: "unknown" }} onRetry={retry} />);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByRole("status").textContent).toBe("Timeline access denied");
  });
});
