import { act, fireEvent, render } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";

import {
  AgentDirectoryProvider,
  AgentName,
  Button,
  Chip,
  EmptyState,
  KeyValueList,
  ListRow,
  LoadingState,
  Meter,
  Notice,
  PanelBody,
  PanelHeader,
  PanelTabPanel,
  PanelTabs,
  RelativeTime,
  RowList,
  SearchField,
  SegmentedControl,
  SelectField,
  SplitDetail,
  StatTile,
  StatusDot,
  StatusRow,
  TechnicalDetails,
  TechnicalId,
  agentDisplayName,
  agentInitial,
  createAgentDirectory,
  formatDuration,
  formatRelativeTime,
  humanizeEnum,
  joinMeta,
  panelTabIds,
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

  it("rounds short durations down so a minute never reads as 60 s", () => {
    expect(formatDuration(59_600)).toBe("59 s");
    expect(formatDuration(60_000)).toBe("1 min");
    expect(formatDuration(4 * 60_000 + 20_000)).toBe("4 min");
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

  it("keys repeated labels apart, so React keeps every row", () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const view = render(
      <TechnicalDetails
        testId="details"
        items={[
          { label: "Attention binding ID", value: "binding-1" },
          { label: "Session ID", value: "session-1" },
          { label: "Attention binding ID", value: "binding-2" },
          { label: "Session ID", value: "session-2" },
        ]}
      />,
    );
    const duplicateKeyWarnings = errors.mock.calls.filter((call) => /same key/.test(String(call[0])));
    errors.mockRestore();
    expect(duplicateKeyWarnings.length).toBe(0);
    expect(view.getByTestId("details").querySelectorAll("dd").length).toBe(4);
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

  it("shares one timer among every relative time, and stops it when the last one goes", () => {
    // Later than any clock reading an earlier test left behind.
    const base = Date.now() + MINUTE;
    vi.useFakeTimers();
    try {
      vi.setSystemTime(base);
      const view = render(
        <div>
          {Array.from({ length: 50 }, (_, index) => (
            <RelativeTime key={index} value={base} testId={`time:${index}`} />
          ))}
        </div>,
      );
      const texts = () => Array.from(new Set(Array.from({ length: 50 }, (_, index) => view.getByTestId(`time:${index}`).textContent)));
      expect(vi.getTimerCount()).toBe(1);
      expect(texts()).toEqual(["just now"]);
      act(() => {
        vi.advanceTimersByTime(15 * SECOND);
      });
      expect(texts()).toEqual(["15s ago"]);
      act(() => {
        vi.advanceTimersByTime(45 * SECOND);
      });
      expect(texts()).toEqual(["1m ago"]);
      expect(vi.getTimerCount()).toBe(1);
      view.unmount();
      expect(vi.getTimerCount()).toBe(0);

      // After a quiet spell the next one reads a fresh clock and starts the
      // timer again.
      vi.setSystemTime(base + 10 * MINUTE);
      const again = render(<RelativeTime value={base + 9 * MINUTE} testId="again" />);
      expect(again.getByTestId("again").textContent).toBe("1m ago");
      expect(vi.getTimerCount()).toBe(1);
      again.unmount();
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
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

  it("ties each tab to the panel it controls", () => {
    function Tabs() {
      const [active, setActive] = useState<"holdings" | "records">("holdings");
      return (
        <>
          <PanelTabs
            ariaLabel="Memory views"
            idBase="memory"
            active={active}
            onSelect={setActive}
            tabs={[
              { id: "holdings", label: "Holdings", testId: "tab:holdings" },
              { id: "records", label: "Records", testId: "tab:records" },
            ]}
          />
          <PanelBody testId="body" tabPanel={{ idBase: "memory", tab: active }}>
            {active === "holdings" ? "Scopes and sizes" : "Every record"}
          </PanelBody>
        </>
      );
    }
    const view = render(<Tabs />);
    const holdings = view.getByTestId("tab:holdings");
    const records = view.getByTestId("tab:records");
    expect(panelTabIds("memory", "records")).toEqual({ tabId: "memory-tab-records", panelId: "memory-panel-records" });
    expect(holdings.getAttribute("id")).toBe("memory-tab-holdings");
    expect(holdings.getAttribute("aria-controls")).toBe("memory-panel-holdings");
    expect(records.getAttribute("aria-controls")).toBe("memory-panel-records");
    const body = view.getByTestId("body");
    expect(body.getAttribute("role")).toBe("tabpanel");
    expect(body.getAttribute("id")).toBe("memory-panel-holdings");
    expect(body.getAttribute("aria-labelledby")).toBe("memory-tab-holdings");
    expect(view.getByRole("tabpanel", { name: "Holdings" }).textContent).toBe("Scopes and sizes");

    fireEvent.click(records);
    expect(body.getAttribute("id")).toBe("memory-panel-records");
    expect(body.getAttribute("aria-labelledby")).toBe("memory-tab-records");
    expect(view.getByRole("tabpanel", { name: "Records" }).textContent).toBe("Every record");
  });

  it("makes content inside the body the panel with PanelTabPanel", () => {
    const view = render(
      <PanelTabPanel idBase="approvals" tab="audit" className="gating-content" data-testid="content">
        Recorded approvals
      </PanelTabPanel>,
    );
    const panel = view.getByTestId("content");
    expect(panel.getAttribute("role")).toBe("tabpanel");
    expect(panel.getAttribute("id")).toBe("approvals-panel-audit");
    expect(panel.getAttribute("aria-labelledby")).toBe("approvals-tab-audit");
    expect(panel.classList.contains("gating-content")).toBe(true);
    expect(panel.textContent).toBe("Recorded approvals");
  });

  it("leaves tabs and the body untied without an id base", () => {
    const view = render(
      <>
        <PanelTabs active="a" onSelect={() => {}} tabs={[{ id: "a", label: "A", testId: "tab:a" }]} />
        <PanelBody testId="body">Body</PanelBody>
      </>,
    );
    expect(view.getByTestId("tab:a").getAttribute("aria-controls") === null).toBe(true);
    expect(view.getByTestId("body").getAttribute("role") === null).toBe(true);
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

  it("leaves keys on a link inside to the link, and takes only Enter and Space itself", () => {
    const open = vi.fn();
    const view = render(
      <StatTile label="Trust rules" summary="1 record breaks the trust rules" tone="crit" onActivate={open} testId="tile">
        <button type="button">Release train cutoff</button>
      </StatTile>,
    );
    const tile = view.getByTestId("tile");
    const link = view.getByRole("button", { name: "Release train cutoff" });
    expect(link.parentElement?.classList.contains("cc-tile__links")).toBe(true);
    fireEvent.keyDown(link, { key: "Enter" });
    fireEvent.keyDown(link, { key: " " });
    fireEvent.keyDown(tile, { key: "a" });
    expect(open.mock.calls.length).toBe(0);
    // The tile keeps Enter and Space from scrolling or submitting.
    expect(fireEvent.keyDown(tile, { key: " " })).toBe(false);
    expect(open.mock.calls.length).toBe(1);
  });

  it("is a real button without links, with its tone, help and data attributes", () => {
    const open = vi.fn();
    const view = render(
      <StatTile
        label="Lattice"
        status={<Chip tone="ok">Holding</Chip>}
        summary="No record breaks a rule"
        tone="ok"
        help="Every record keeps its trust rules"
        onActivate={open}
        testId="tile"
        dataAttributes={{ "data-status": "holding" }}
      />,
    );
    const tile = view.getByTestId("tile");
    expect(tile.tagName).toBe("BUTTON");
    expect(tile.getAttribute("type")).toBe("button");
    expect(tile.getAttribute("data-tone")).toBe("ok");
    expect(tile.getAttribute("title")).toBe("Every record keeps its trust rules");
    expect(tile.getAttribute("data-status")).toBe("holding");
    expect(tile.querySelector(".cc-tile__label")?.textContent).toBe("Lattice");
    expect(tile.querySelector(".cc-tile__status")?.textContent).toBe("Holding");
    expect(tile.querySelector(".cc-tile__summary")?.textContent).toBe("No record breaks a rule");
    expect(tile.querySelector(".cc-tile__links") === null).toBe(true);
    fireEvent.click(tile);
    expect(open.mock.calls.length).toBe(1);
  });

  it("draws the quiet muted form for a check that cannot run", () => {
    const view = render(<StatTile label="Usefulness" summary="Not measured yet" muted testId="tile" />);
    const tile = view.getByTestId("tile");
    expect(tile.classList.contains("cc-tile--muted")).toBe(true);
    expect(tile.getAttribute("data-tone")).toBe("neutral");
    expect(tile.querySelector(".cc-tile__status") === null).toBe(true);
  });
});

describe("SplitDetail", () => {
  const AGENTS = [
    { id: "coordinator", name: "Coordinator" },
    { id: "analyst", name: "Research Analyst" },
  ];

  // A roster-like list and detail. `narrow` adds the narrow pane's rule that
  // hides the list under an open detail (jsdom runs no container queries).
  function Harness({
    narrow,
    heading = true,
    markSelection = true,
    rekeyOnBack = false,
  }: {
    narrow: boolean;
    heading?: boolean;
    markSelection?: boolean;
    rekeyOnBack?: boolean;
  }) {
    const [selected, setSelected] = useState<string | null>(null);
    const [open, setOpen] = useState(false);
    const [generation, setGeneration] = useState(0);
    const name = AGENTS.find((agent) => agent.id === selected)?.name ?? "";
    return (
      <div>
        {narrow ? <style>{".cc-split.has-detail .cc-split__list { visibility: hidden; }"}</style> : null}
        <button type="button" data-testid="outside" onClick={() => { setSelected("analyst"); setOpen(true); }}>
          Show the analyst
        </button>
        <SplitDetail
          testId="split"
          list={(
            <RowList>
              {AGENTS.map((agent) => (
                <ListRow
                  key={`${agent.id}:${generation}`}
                  title={agent.name}
                  testId={`row:${agent.id}`}
                  selected={markSelection && selected === agent.id}
                  onActivate={() => { setSelected(agent.id); setOpen(true); }}
                />
              ))}
            </RowList>
          )}
          detail={selected ? (
            <div>
              {heading ? <h3 data-testid="detail-heading">{name}</h3> : <p>{name}</p>}
              <button type="button" onClick={() => setSelected("coordinator")}>Show the coordinator</button>
            </div>
          ) : null}
          emptyDetail={<p data-testid="empty-detail">Select an agent.</p>}
          detailOpen={open}
          onBack={() => {
            setOpen(false);
            if (rekeyOnBack) setGeneration((value) => value + 1);
          }}
          backLabel="All agents"
        />
      </div>
    );
  }

  const activeTestId = () => document.activeElement?.getAttribute("data-testid") ?? null;

  it("shows the empty detail, then the detail with a Back row once opened", () => {
    const view = render(<Harness narrow={false} />);
    const split = view.getByTestId("split");
    expect(split.classList.contains("has-detail")).toBe(false);
    expect(view.queryByTestId("empty-detail") !== null).toBe(true);
    expect(view.queryByRole("button", { name: "All agents" }) === null).toBe(true);
    fireEvent.click(view.getByTestId("row:analyst"));
    expect(split.classList.contains("has-detail")).toBe(true);
    expect(view.getByTestId("detail-heading").textContent).toBe("Research Analyst");
    expect(view.queryByTestId("empty-detail") === null).toBe(true);
    expect(view.getByRole("button", { name: "All agents" }).classList.contains("cc-split__back")).toBe(true);
  });

  it("moves focus into a detail that replaces the list, and back to its row on Back", () => {
    const view = render(<Harness narrow />);
    const row = view.getByTestId("row:analyst");
    row.focus();
    fireEvent.click(row);
    expect(view.getByTestId("split").classList.contains("has-detail")).toBe(true);
    expect(activeTestId()).toBe("detail-heading");
    expect(view.getByTestId("detail-heading").getAttribute("tabindex")).toBe("-1");

    const back = view.getByRole("button", { name: "All agents" });
    back.focus();
    fireEvent.click(back);
    expect(view.getByTestId("split").classList.contains("has-detail")).toBe(false);
    expect(activeTestId()).toBe("row:analyst");
  });

  it("focuses the detail itself when it has no heading", () => {
    const view = render(<Harness narrow heading={false} />);
    const row = view.getByTestId("row:coordinator");
    row.focus();
    fireEvent.click(row);
    expect(document.activeElement?.classList.contains("cc-split__detail")).toBe(true);
    expect(document.activeElement?.getAttribute("tabindex")).toBe("-1");
  });

  it("returns focus to the row the detail shows after moving to another agent", () => {
    const view = render(<Harness narrow />);
    const row = view.getByTestId("row:analyst");
    row.focus();
    fireEvent.click(row);
    fireEvent.click(view.getByRole("button", { name: "Show the coordinator" }));
    expect(view.getByTestId("detail-heading").textContent).toBe("Coordinator");
    const back = view.getByRole("button", { name: "All agents" });
    back.focus();
    fireEvent.click(back);
    expect(activeTestId()).toBe("row:coordinator");
  });

  it("finds the opening row again by its test ID when the list re-rendered it", () => {
    const view = render(<Harness narrow markSelection={false} rekeyOnBack />);
    const row = view.getByTestId("row:analyst");
    row.focus();
    fireEvent.click(row);
    expect(activeTestId()).toBe("detail-heading");
    const back = view.getByRole("button", { name: "All agents" });
    back.focus();
    fireEvent.click(back);
    expect(row.isConnected).toBe(false);
    expect(activeTestId()).toBe("row:analyst");
    expect(document.activeElement?.tagName).toBe("BUTTON");
  });

  it("leaves focus alone side by side, and when it was outside the list", () => {
    const wide = render(<Harness narrow={false} />);
    const row = wide.getByTestId("row:analyst");
    row.focus();
    fireEvent.click(row);
    expect(wide.getByTestId("split").classList.contains("has-detail")).toBe(true);
    expect(activeTestId()).toBe("row:analyst");
    wide.unmount();

    const narrow = render(<Harness narrow />);
    const outside = narrow.getByTestId("outside");
    outside.focus();
    fireEvent.click(outside);
    expect(narrow.getByTestId("split").classList.contains("has-detail")).toBe(true);
    expect(activeTestId()).toBe("outside");
  });
});


describe("Meter", () => {
  it("draws each segment as its share of the total and names the bar in words", () => {
    const view = render(
      <Meter
        label="3 active, 1 quarantined"
        total={8}
        testId="meter"
        segments={[
          { value: 3, tone: "ok", label: "3 active" },
          { value: 1, tone: "warn", label: "1 quarantined" },
          { value: 0, tone: "neutral", label: "0 superseded" },
        ]}
      />,
    );
    const meter = view.getByTestId("meter");
    const segments = Array.from(meter.querySelectorAll<HTMLElement>(".cc-meter__seg"));
    expect(segments.map((segment) => segment.style.width)).toEqual(["37.5%", "12.5%"]);
    expect(segments.map((segment) => segment.getAttribute("data-tone"))).toEqual(["ok", "warn"]);
    expect(segments.map((segment) => segment.getAttribute("title"))).toEqual(["3 active", "1 quarantined"]);
    expect(view.getByRole("img", { name: "3 active, 1 quarantined" }).classList.contains("cc-meter__track")).toBe(true);
    expect(meter.querySelector(".cc-meter__label")?.textContent).toBe("3 active, 1 quarantined");
  });

  it("fills to the sum without a total, never past the whole, and skips empty and negative segments", () => {
    const view = render(
      <div>
        <Meter label="sum" testId="sum" segments={[{ value: 1 }, { value: 3 }]} />
        <Meter label="over" testId="over" total={2} segments={[{ value: 3 }, { value: 1 }]} />
        <Meter label="negative" testId="negative" segments={[{ value: -2 }, { value: 2 }]} />
        <Meter label="empty" testId="empty" showLabel={false} segments={[{ value: 0 }]} />
      </div>,
    );
    const widths = (testId: string) => Array.from(view.getByTestId(testId).querySelectorAll<HTMLElement>(".cc-meter__seg")).map((segment) => segment.style.width);
    expect(widths("sum")).toEqual(["25%", "75%"]);
    expect(widths("over")).toEqual(["75%", "25%"]);
    expect(widths("negative")).toEqual(["100%"]);
    expect(view.getByTestId("negative").querySelector(".cc-meter__seg")?.getAttribute("data-tone")).toBe("neutral");
    expect(widths("empty")).toEqual([]);
    expect(view.getByTestId("empty").querySelector(".cc-meter__label") === null).toBe(true);
    expect(view.getByRole("img", { name: "empty" }).getAttribute("aria-label")).toBe("empty");
  });
});

describe("KeyValueList", () => {
  it("lists labelled values in a description list and skips empty ones", () => {
    const view = render(
      <KeyValueList
        testId="facts"
        className="facts"
        items={[
          { label: "Owner", value: "Coordinator", testId: "fact:owner" },
          { label: "Priority", value: null },
          { label: "Due", value: undefined },
          { label: "Labels", value: "" },
          { label: "Pinned", value: false },
          { label: "Attempts", value: 0 },
          null,
          false,
          { label: "Updated", value: <time>12m ago</time> },
        ]}
      />,
    );
    const list = view.getByTestId("facts");
    expect(list.tagName).toBe("DL");
    expect(list.classList.contains("cc-kv")).toBe(true);
    expect(list.classList.contains("facts")).toBe(true);
    expect(Array.from(list.querySelectorAll("dt")).map((term) => term.textContent)).toEqual(["Owner", "Attempts", "Updated"]);
    expect(Array.from(list.querySelectorAll("dd")).map((value) => value.textContent)).toEqual(["Coordinator", "0", "12m ago"]);
    expect(view.getByTestId("fact:owner").classList.contains("cc-kv__row")).toBe(true);
  });

  it("keeps rows that share a label and renders nothing when every value is empty", () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const view = render(
      <div data-testid="host">
        <KeyValueList testId="facts" items={[{ label: "Parent", value: "Release" }, { label: "Parent", value: "Docs" }]} />
        <KeyValueList testId="nothing" items={[{ label: "Owner", value: null }, null]} />
      </div>,
    );
    const duplicateKeyWarnings = errors.mock.calls.filter((call) => /same key/.test(String(call[0])));
    errors.mockRestore();
    expect(duplicateKeyWarnings.length).toBe(0);
    expect(view.getByTestId("facts").querySelectorAll("dd").length).toBe(2);
    expect(view.queryByTestId("nothing") === null).toBe(true);
    expect(view.getByTestId("host").childElementCount).toBe(1);
  });
});

describe("LoadingState", () => {
  it("announces its label politely and holds the layout with hidden skeleton rows", () => {
    const view = render(<LoadingState label="Loading records" rows={3} testId="loading" />);
    const loading = view.getByTestId("loading");
    expect(loading.getAttribute("role")).toBe("status");
    expect(view.getByRole("status").textContent).toBe("Loading records");
    expect(loading.querySelector(".cc-visually-hidden")?.textContent).toBe("Loading records");
    const rows = Array.from(loading.querySelectorAll(".cc-skeleton__row"));
    expect(rows.length).toBe(3);
    expect(rows.every((row) => row.getAttribute("aria-hidden") === "true")).toBe(true);
    expect(loading.classList.contains("cc-skeleton--rows")).toBe(true);
  });

  it("says Loading by default, draws cards, and keeps between one and eight rows", () => {
    const view = render(
      <div>
        <LoadingState testId="default" />
        <LoadingState variant="cards" rows={0} testId="cards" />
        <LoadingState rows={20} testId="many" />
      </div>,
    );
    expect(view.getByTestId("default").textContent).toBe("Loading");
    expect(view.getByTestId("default").querySelectorAll(".cc-skeleton__row").length).toBe(4);
    expect(view.getByTestId("cards").classList.contains("cc-skeleton--cards")).toBe(true);
    expect(view.getByTestId("cards").querySelectorAll(".cc-skeleton__row").length).toBe(1);
    expect(view.getByTestId("many").querySelectorAll(".cc-skeleton__row").length).toBe(8);
  });
});

describe("rows and controls", () => {
  // A control inside another control: a button or link in a button, or in
  // an element that claims the button role.
  const NESTED_CONTROLS = [
    "button button", "button a", "button [role='button']",
    "[role='button'] button", "[role='button'] a", "[role='button'] [role='button']",
  ].join(", ");

  it("keeps a row's actions and extra content beside its button, never inside it", () => {
    const open = vi.fn();
    const claim = vi.fn();
    const view = render(
      <div>
        <ListRow title="Coordinator" subtitle="Leads" onActivate={open} testId="plain" />
        <ListRow
          title="Research Analyst"
          subtitle="Analyst"
          meta={<span>2m ago</span>}
          onActivate={open}
          selected
          ariaExpanded={false}
          titleAttr="identity:analyst"
          testId="with-actions"
          dataAttributes={{ "data-state": "working" }}
          actions={<Button size="sm" onClick={claim}>Claim</Button>}
        >
          <span>Waiting on a review</span>
        </ListRow>
      </div>,
    );
    const plain = view.getByTestId("plain");
    expect(plain.tagName).toBe("BUTTON");
    fireEvent.click(plain);
    expect(open.mock.calls.length).toBe(1);

    const row = view.getByTestId("with-actions");
    expect(row.getAttribute("role") === null).toBe(true);
    expect(row.getAttribute("tabindex") === null).toBe(true);
    expect(row.querySelectorAll(NESTED_CONTROLS).length).toBe(0);
    expect(row.classList.contains("is-selected")).toBe(true);
    expect(row.getAttribute("data-state")).toBe("working");
    // The title is the row's button: a real button, so Enter and Space work
    // natively, named by the title and described by the rest of the row.
    const hit = view.getByRole("button", { name: "Research Analyst" });
    expect(hit.tagName).toBe("BUTTON");
    expect(hit.getAttribute("type")).toBe("button");
    expect(hit.classList.contains("cc-row__hit")).toBe(true);
    expect(hit.getAttribute("aria-current")).toBe("true");
    expect(hit.getAttribute("aria-expanded")).toBe("false");
    expect(hit.querySelector(".cc-row__title")?.getAttribute("title")).toBe("identity:analyst");
    const described = (hit.getAttribute("aria-describedby") ?? "").split(" ").map((id) => document.getElementById(id)?.textContent);
    expect(described).toEqual(["Analyst", "2m ago", "Waiting on a review"]);
    const claimButton = view.getByRole("button", { name: "Claim" });
    expect(hit.contains(claimButton)).toBe(false);

    // The button, the row and its extra content open the row; an action
    // does not.
    fireEvent.click(hit);
    fireEvent.click(row);
    fireEvent.click(view.getByText("Waiting on a review"));
    expect(open.mock.calls.length).toBe(4);
    fireEvent.click(claimButton);
    expect(claim.mock.calls.length).toBe(1);
    expect(open.mock.calls.length).toBe(4);
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
