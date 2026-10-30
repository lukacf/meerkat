import { act, fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import {
  AgentDirectoryProvider,
  AgentName,
  Button,
  Chip,
  EmptyState,
  ListRow,
  Meter,
  Notice,
  PanelHeader,
  PanelTabs,
  RelativeTime,
  SearchField,
  SegmentedControl,
  SelectField,
  StatTile,
  StatusDot,
  StatusRow,
  TechnicalDetails,
  TechnicalId,
  agentDisplayName,
  agentInitial,
  createAgentDirectory,
  formatRelativeTime,
  humanizeEnum,
  joinMeta,
  plural,
  priorityTone,
  riskTone,
  splitTechnicalId,
  statusTone,
  stripAgentKeyPrefix,
  toEpochMs,
} from "./index";

// DOM assertions compare booleans and strings, never elements: a failing
// element comparison walks React internals and can exhaust memory.

const NOW = Date.UTC(2026, 9, 30, 12, 0, 0);
const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

describe("formatRelativeTime buckets", () => {
  it("reads compact past times", () => {
    expect(formatRelativeTime(NOW, NOW)).toBe("just now");
    expect(formatRelativeTime(NOW - 9 * SECOND, NOW)).toBe("just now");
    expect(formatRelativeTime(NOW - 40 * SECOND, NOW)).toBe("40s ago");
    expect(formatRelativeTime(NOW - 12 * MINUTE, NOW)).toBe("12m ago");
    expect(formatRelativeTime(NOW - 3 * HOUR - 5 * MINUTE, NOW)).toBe("3h ago");
    expect(formatRelativeTime(NOW - 2 * DAY, NOW)).toBe("2d ago");
    expect(formatRelativeTime(NOW - 6 * DAY - 23 * HOUR, NOW)).toBe("6d ago");
  });

  it("switches to a calendar date after a week, with the year only when it differs", () => {
    const lastWeek = formatRelativeTime(NOW - 8 * DAY, NOW);
    expect(lastWeek.includes("ago")).toBe(false);
    expect(lastWeek.includes("2026")).toBe(false);
    expect(/\d/.test(lastWeek)).toBe(true);
    const lastYear = formatRelativeTime(Date.UTC(2025, 5, 1, 12), NOW);
    expect(lastYear.includes("2025")).toBe(true);
  });

  it("reads future times and tolerates a little clock skew", () => {
    expect(formatRelativeTime(NOW + 5 * MINUTE, NOW)).toBe("in 5m");
    expect(formatRelativeTime(NOW + 3 * HOUR, NOW)).toBe("in 3h");
    expect(formatRelativeTime(NOW + 4 * SECOND, NOW)).toBe("just now");
  });

  it("accepts ISO strings and Dates and omits unreadable values", () => {
    expect(formatRelativeTime(new Date(NOW - 12 * MINUTE).toISOString(), NOW)).toBe("12m ago");
    expect(formatRelativeTime(new Date(NOW - 3 * HOUR), NOW)).toBe("3h ago");
    expect(formatRelativeTime("not a time", NOW)).toBe("");
    expect(formatRelativeTime(0, NOW)).toBe("");
    expect(formatRelativeTime(null, NOW)).toBe("");
    expect(toEpochMs(-5)).toBe(null);
    expect(toEpochMs("2026-10-30T12:00:00Z")).toBe(NOW);
  });
});

describe("copy helpers", () => {
  it("counts with real plurals", () => {
    expect(plural(1, "record")).toBe("1 record");
    expect(plural(2, "record")).toBe("2 records");
    expect(plural(0, "record")).toBe("0 records");
    expect(plural(3, "policy", "policies")).toBe("3 policies");
  });

  it("turns raw enums into sentence-case words", () => {
    expect(humanizeEnum("decision_recorded")).toBe("Decision recorded");
    expect(humanizeEnum("automatic_allowance")).toBe("Automatic allowance");
    expect(humanizeEnum("in-progress")).toBe("In progress");
    expect(humanizeEnum("toolCallRequested")).toBe("Tool call requested");
    expect(humanizeEnum("")).toBe("");
  });

  it("joins parts with the middle dot and skips empty ones", () => {
    expect(joinMeta("Coordinator", "", null, "Leads", 3)).toBe("Coordinator · Leads · 3");
  });
});

describe("tone map", () => {
  it("maps status words to one tone each", () => {
    expect(statusTone("in_progress")).toBe("info");
    expect(statusTone("In progress")).toBe("info");
    expect(statusTone("running")).toBe("info");
    expect(statusTone("completed")).toBe("ok");
    expect(statusTone("healthy")).toBe("ok");
    expect(statusTone("delivered")).toBe("ok");
    expect(statusTone("holding")).toBe("ok");
    expect(statusTone("degraded")).toBe("warn");
    expect(statusTone("blocked")).toBe("warn");
    expect(statusTone("quarantined")).toBe("warn");
    expect(statusTone("wedged")).toBe("crit");
    expect(statusTone("stalled")).toBe("crit");
    expect(statusTone("failed")).toBe("crit");
    expect(statusTone("violated")).toBe("crit");
    expect(statusTone("rejected")).toBe("crit");
    expect(statusTone("no-grant")).toBe("locked");
    expect(statusTone("unverifiable")).toBe("locked");
  });

  it("keeps quiet and unknown words neutral", () => {
    for (const word of ["open", "idle", "pending", "superseded", "retired", "cancelled", "something new", "", null, undefined]) {
      expect(statusTone(word)).toBe("neutral");
    }
  });

  it("shows priority only when it matters and tones risk tiers", () => {
    expect(priorityTone("high")).toBe("crit");
    expect(priorityTone("low")).toBe("neutral");
    expect(priorityTone("medium")).toBe(null);
    expect(priorityTone(undefined)).toBe(null);
    expect(riskTone("r3")).toBe("crit");
    expect(riskTone("r2")).toBe("warn");
    expect(riskTone("r1")).toBe("neutral");
    expect(riskTone("unknown")).toBe("neutral");
  });
});

describe("agent names", () => {
  const directory = createAgentDirectory([
    { id: "identity:coordinator", label: "Coordinator", aliases: ["member-7", "session-01JB"] },
    { id: "research", label: "Research Analyst" },
  ]);

  it("resolves canonical keys, aliases and prefixed or bare forms", () => {
    expect(agentDisplayName("identity:coordinator", directory)).toBe("Coordinator");
    expect(agentDisplayName("coordinator", directory)).toBe("Coordinator");
    expect(agentDisplayName("member-7", directory)).toBe("Coordinator");
    expect(agentDisplayName("session-01JB", directory)).toBe("Coordinator");
    expect(agentDisplayName("identity:research", directory)).toBe("Research Analyst");
  });

  it("falls back to the key without its kind prefix and never invents capitals", () => {
    expect(agentDisplayName("identity:support-triage", directory)).toBe("support-triage");
    expect(agentDisplayName("agent:nightly-monitor")).toBe("nightly-monitor");
    expect(agentDisplayName("household-agent")).toBe("household-agent");
    expect(agentDisplayName("", directory)).toBe("");
    expect(agentDisplayName(undefined, directory)).toBe("");
    expect(stripAgentKeyPrefix("identity:agent:x")).toBe("x");
    expect(stripAgentKeyPrefix("identity:")).toBe("identity:");
  });

  it("draws one initial for avatars", () => {
    expect(agentInitial("research analyst")).toBe("R");
    expect(agentInitial("  42 ops")).toBe("4");
    expect(agentInitial("")).toBe("?");
  });

  it("renders the display name with the raw key in the tooltip", () => {
    const view = render(
      <AgentDirectoryProvider directory={directory}>
        <AgentName id="identity:coordinator" avatar={18} testId="named" />
        <AgentName id="identity:unknown-agent" testId="fallback" />
      </AgentDirectoryProvider>,
    );
    const named = view.getByTestId("named");
    expect(named.textContent).toBe("CCoordinator");
    expect(named.getAttribute("title")).toBe("identity:coordinator");
    expect(named.querySelector(".cc-agent__avatar")?.getAttribute("aria-hidden")).toBe("true");
    const fallback = view.getByTestId("fallback");
    expect(fallback.textContent).toBe("unknown-agent");
    expect(fallback.getAttribute("title")).toBe("identity:unknown-agent");
  });
});

describe("TechnicalId", () => {
  const id = "mem_01JBX6Q5ZK3W8R2M8P4R6T0";

  it("splits long values into a shrinking head and a fixed six-character tail", () => {
    expect(id.length).toBe(27);
    expect(splitTechnicalId(id)).toEqual({ head: "mem_01JBX6Q5ZK3W8R2M8", tail: "P4R6T0" });
    expect(splitTechnicalId("short-id")).toEqual({ head: "short-id", tail: "" });
  });

  it("hides the truncated text from assistive tech and exposes the full value", () => {
    const view = render(<TechnicalId value={id} label="Memory ID" testId="tid" />);
    const root = view.getByTestId("tid");
    expect(root.getAttribute("title")).toBe(id);
    expect(root.querySelector(".cc-tech-id__text")?.getAttribute("aria-hidden")).toBe("true");
    expect(root.querySelector(".cc-tech-id__head")?.textContent).toBe("mem_01JBX6Q5ZK3W8R2M8");
    expect(root.querySelector(".cc-tech-id__tail")?.textContent).toBe("P4R6T0");
    expect(root.querySelector(".cc-visually-hidden")?.textContent).toBe(id);
    const copy = view.getByRole("button", { name: "Copy memory ID" });
    expect(copy.classList.contains("cc-tech-id__copy")).toBe(true);
  });

  it("copies the exact value and can render without a copy button", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    const view = render(<TechnicalId value={id} label="Memory ID" />);
    await act(async () => {
      fireEvent.click(view.getByRole("button", { name: "Copy memory ID" }));
    });
    expect(writeText.mock.calls.length).toBe(1);
    expect(writeText.mock.calls[0]?.[0]).toBe(id);
    vi.unstubAllGlobals();

    const bare = render(<TechnicalId value={id} copy={false} testId="bare" />);
    expect(bare.getByTestId("bare").querySelector("button") === null).toBe(true);
  });
});

describe("TechnicalDetails", () => {
  it("is collapsed by default, skips empty values and shows IDs as TechnicalIds", () => {
    const view = render(
      <TechnicalDetails
        testId="details"
        items={[
          { label: "Identity", value: "identity:release-manager" },
          { label: "Session", value: "" },
          { label: "Generation", value: 7 },
          null,
        ]}
      />,
    );
    const details = view.getByTestId("details") as HTMLDetailsElement;
    expect(details.tagName).toBe("DETAILS");
    expect(details.open).toBe(false);
    expect(details.querySelector("summary")?.textContent).toBe("Technical details");
    const labels = Array.from(details.querySelectorAll("dt")).map((node) => node.textContent);
    expect(labels).toEqual(["Identity", "Generation"]);
    expect(details.querySelectorAll(".cc-tech-id").length).toBe(1);
    expect(details.textContent?.includes("identity:release-manager")).toBe(true);
    expect(details.querySelectorAll("dd")[1]?.textContent).toBe("7");
  });

  it("renders nothing when every value is empty", () => {
    const view = render(<div data-testid="host"><TechnicalDetails items={[{ label: "Session", value: null }]} /></div>);
    expect(view.getByTestId("host").childElementCount).toBe(0);
  });
});

describe("RelativeTime", () => {
  it("renders a time element with the absolute time in the tooltip", () => {
    const view = render(<RelativeTime value={NOW - 12 * MINUTE} now={NOW} prefix="Updated" testId="time" />);
    const time = view.getByTestId("time");
    expect(time.tagName).toBe("TIME");
    expect(time.textContent).toBe("Updated 12m ago");
    expect(time.getAttribute("datetime")).toBe(new Date(NOW - 12 * MINUTE).toISOString());
    expect((time.getAttribute("title") ?? "").includes("2026")).toBe(true);
  });

  it("keeps a caller's tooltip and renders nothing for unreadable values", () => {
    const view = render(
      <div data-testid="host">
        <RelativeTime value="2026-07-08T18:15:30Z" now={NOW} title="as of 2026-07-08 18:15:30" testId="as-of" />
        <RelativeTime value="garbage" now={NOW} />
      </div>,
    );
    expect(view.getByTestId("as-of").getAttribute("title")).toBe("as of 2026-07-08 18:15:30");
    expect(view.getByTestId("host").querySelectorAll("time").length).toBe(1);
  });
});

describe("panel frame", () => {
  it("gives the panel one heading, a subtitle and actions", () => {
    const view = render(
      <PanelHeader title="Memory" subtitle="12 records" actions={<Button>Refresh</Button>} className="memory-head" />,
    );
    expect(view.getAllByRole("heading").length).toBe(1);
    expect(view.getByRole("heading", { name: "Memory" }).tagName).toBe("H2");
    expect(view.getByText("12 records").className).toBe("cc-panel-head__sub");
    expect(view.container.querySelector("header")?.classList.contains("memory-head")).toBe(true);
  });

  it("renders tabs with selection state, counts and arrow-key focus", () => {
    const onSelect = vi.fn();
    const view = render(
      <PanelTabs
        ariaLabel="Memory views"
        active="records"
        onSelect={onSelect}
        tabs={[
          { id: "holdings", label: "Holdings", testId: "tab:holdings" },
          { id: "records", label: "Records", testId: "tab:records" },
          { id: "pipeline", label: "Pipeline", count: 2, testId: "tab:pipeline" },
        ]}
      />,
    );
    expect(view.getByRole("tablist", { name: "Memory views" }).classList.contains("cc-tabs")).toBe(true);
    const records = view.getByTestId("tab:records");
    expect(records.getAttribute("aria-selected")).toBe("true");
    expect(records.classList.contains("is-active")).toBe(true);
    expect(records.getAttribute("tabindex")).toBe("0");
    expect(view.getByTestId("tab:holdings").getAttribute("tabindex")).toBe("-1");
    expect(view.getByRole("tab", { name: "Pipeline 2" }).getAttribute("data-testid")).toBe("tab:pipeline");

    records.focus();
    fireEvent.keyDown(records, { key: "ArrowRight" });
    expect(document.activeElement === view.getByTestId("tab:pipeline")).toBe(true);
    fireEvent.keyDown(view.getByTestId("tab:pipeline"), { key: "ArrowRight" });
    expect(document.activeElement === view.getByTestId("tab:holdings")).toBe(true);
    fireEvent.keyDown(view.getByTestId("tab:holdings"), { key: "End" });
    expect(document.activeElement === view.getByTestId("tab:pipeline")).toBe(true);
    expect(onSelect.mock.calls.length).toBe(0);

    fireEvent.click(view.getByTestId("tab:pipeline"));
    expect(onSelect.mock.calls[0]?.[0]).toBe("pipeline");
  });

  it("reserves the status row and shows either the error or the freshness", () => {
    const view = render(
      <div>
        <StatusRow error="Record refresh unavailable" errorTestId="memory-error" />
        <StatusRow updatedAt={Date.now() - 2 * MINUTE} />
      </div>,
    );
    const alert = view.getByRole("alert");
    expect(alert.getAttribute("data-testid")).toBe("memory-error");
    expect(alert.getAttribute("title")).toBe("Record refresh unavailable");
    expect(view.getAllByRole("alert").length).toBe(1);
    expect(view.getByText("Updated 2m ago").tagName).toBe("TIME");
  });
});

describe("StatTile", () => {
  it("stays a keyboard-reachable button when it also holds links", () => {
    const open = vi.fn();
    const link = vi.fn();
    const view = render(
      <StatTile label="Trust rules" summary="2 records break the trust rules" tone="crit" onActivate={open} testId="tile">
        <button type="button" onClick={link}>Release train cutoff</button>
      </StatTile>,
    );
    const tile = view.getByTestId("tile");
    expect(tile.getAttribute("role")).toBe("button");
    expect(tile.getAttribute("tabindex")).toBe("0");
    fireEvent.click(tile);
    fireEvent.keyDown(tile, { key: "Enter" });
    fireEvent.keyDown(tile, { key: " " });
    expect(open.mock.calls.length).toBe(3);
    // A link inside the tile opens its record, not the tile's tab.
    fireEvent.click(view.getByRole("button", { name: "Release train cutoff" }));
    expect(link.mock.calls.length).toBe(1);
    expect(open.mock.calls.length).toBe(3);
  });

  it("is a plain region without an action", () => {
    const view = render(<StatTile label="Usefulness" summary="Fine" testId="tile" />);
    const tile = view.getByTestId("tile");
    expect(tile.getAttribute("role") === null).toBe(true);
    expect(tile.classList.contains("is-interactive")).toBe(false);
  });
});

describe("rows and controls", () => {
  it("activates button rows and role=button rows with Enter and Space, not from their actions", () => {
    const open = vi.fn();
    const act1 = vi.fn();
    const view = render(
      <div>
        <ListRow title="Coordinator" subtitle="Leads" onActivate={open} testId="plain" />
        <ListRow
          title="Research Analyst"
          onActivate={open}
          selected
          testId="with-actions"
          actions={<Button size="sm" onClick={act1}>Claim</Button>}
        />
      </div>,
    );
    const plain = view.getByTestId("plain");
    expect(plain.tagName).toBe("BUTTON");
    fireEvent.click(plain);
    expect(open.mock.calls.length).toBe(1);

    const row = view.getByTestId("with-actions");
    expect(row.getAttribute("role")).toBe("button");
    expect(row.getAttribute("tabindex")).toBe("0");
    expect(row.getAttribute("aria-current")).toBe("true");
    expect(row.classList.contains("is-selected")).toBe(true);
    fireEvent.keyDown(row, { key: "Enter" });
    fireEvent.keyDown(row, { key: " " });
    expect(open.mock.calls.length).toBe(3);
    fireEvent.click(view.getByRole("button", { name: "Claim" }));
    expect(act1.mock.calls.length).toBe(1);
    expect(open.mock.calls.length).toBe(3);
  });

  it("keeps the button label as the button's own text and hides icons", () => {
    const view = render(
      <Button icon={<svg data-icon="refresh" />} variant="primary">
        Confirm
      </Button>,
    );
    const button = view.getByText("Confirm");
    expect(button.tagName).toBe("BUTTON");
    expect(button.textContent).toBe("Confirm");
    expect(button.getAttribute("type")).toBe("button");
    expect(button.classList.contains("cc-btn--primary")).toBe(true);
    expect(button.querySelector(".cc-btn__icon")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("marks pressed segments and labels search and select fields", () => {
    const onChange = vi.fn();
    const view = render(
      <div>
        <SegmentedControl
          ariaLabel="View"
          value="tree"
          onChange={onChange}
          options={[
            { id: "tree", label: "Tree", testId: "view:tree" },
            { id: "graph", label: "Graph", testId: "view:graph" },
          ]}
        />
        <SearchField label="Search agents" value="" onChange={() => {}} />
        <SelectField label="Mob" ariaLabel="Memory mob" value="a" onChange={() => {}} options={[{ value: "a", label: "Operations" }]} />
      </div>,
    );
    expect(view.getByTestId("view:tree").getAttribute("aria-pressed")).toBe("true");
    expect(view.getByTestId("view:graph").getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(view.getByTestId("view:graph"));
    expect(onChange.mock.calls[0]?.[0]).toBe("graph");
    expect(view.getByRole("searchbox", { name: "Search agents" }).tagName).toBe("INPUT");
    expect(view.getByRole("combobox", { name: "Memory mob" }).tagName).toBe("SELECT");
  });

  it("pairs tone attributes with words and keeps dots decorative", () => {
    const view = render(
      <div>
        <Chip tone="warn" testId="chip">Degraded</Chip>
        <StatusDot tone="ok" live />
        <StatusDot tone="crit" label="Stalled" />
        <Meter label="1 of 5 done" segments={[{ value: 1, tone: "ok" }]} total={5} testId="meter" />
      </div>,
    );
    expect(view.getByTestId("chip").getAttribute("data-tone")).toBe("warn");
    expect(view.getByTestId("chip").textContent).toBe("Degraded");
    const dots = view.container.querySelectorAll(".cc-dot");
    expect(dots[0]?.getAttribute("aria-hidden")).toBe("true");
    expect(dots[0]?.classList.contains("is-live")).toBe(true);
    expect(view.getByRole("img", { name: "Stalled" }).classList.contains("cc-dot")).toBe(true);
    expect(view.getByRole("img", { name: "1 of 5 done" }).classList.contains("cc-meter__track")).toBe(true);
    expect((view.getByTestId("meter").querySelector(".cc-meter__seg") as HTMLElement | null)?.style.width).toBe("20%");
  });

  it("designs empty, no-access and notice states", () => {
    const view = render(
      <div>
        <EmptyState title="No agents yet" body="Agents appear here once a team starts." testId="empty" />
        <EmptyState kind="no-access" title="You don't have access to work items" testId="denied" />
        <Notice tone="warn" testId="notice">This connection is read-only.</Notice>
      </div>,
    );
    expect(view.getByTestId("empty").getAttribute("data-kind")).toBe("empty");
    expect(view.getByText("No agents yet").className).toBe("cc-panel-empty__title");
    expect(view.getByTestId("denied").querySelector('[data-glyph="shield"]') !== null).toBe(true);
    expect(view.getByTestId("notice").getAttribute("data-tone")).toBe("warn");
    expect(view.getByText("This connection is read-only.").className).toBe("cc-notice__text");
  });
});
