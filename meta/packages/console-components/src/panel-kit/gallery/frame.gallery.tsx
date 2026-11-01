import { useState } from "react";

import { AgentAvatar, AgentName } from "../agent-name";
import { Button, IconButton } from "../button";
import { StatusPill } from "../chip";
import { DetailDrawer, DrawerLayout, DrawerSection } from "../drawer";
import { EntityRow } from "../entity-card";
import { FigureTile } from "../figure-tile";
import { GlanceStrip } from "../glance-strip";
import { KitGlyph } from "../glyphs";
import { OverflowMenu, type MenuEntry } from "../menu";
import { PanelBody, PanelHeader, PanelShell, PanelTabs, StatusRow, type PanelDensity } from "../panel";
import { standardPanelMenu } from "../preferences";
import { RelativeTime } from "../relative-time";
import { SelectField } from "../toolbar";
import { ViewOptionsMenu } from "../view-options";
import { ActivityBars } from "../../viz/charts/activity-bars";
import { BarcodeLegend, BarcodeMeter } from "../../viz/charts/barcode-meter";
import { Specimen, SpecimenRow } from "./specimen";
import type { GallerySection } from "./types";

const NOW = Date.now();
const MIN = 60_000;

type View = "lanes" | "clusters" | "roles" | "list";

const AGENTS: Array<{ id: string; name: string; role: string; phase?: string; live?: boolean; tone: "live" | "ok" | "warn" | "crit" | "neutral"; word: string; trend: number[]; last: number }> = [
  { id: "triage", name: "Triage", role: "Coordinator · Leads", phase: "Replying to a refund email", live: true, tone: "live", word: "Working", trend: [1, 2, 4, 3, 5, 6, 4, 7], last: 1 },
  { id: "router", name: "Router", role: "Coordinator · Leads", tone: "neutral", word: "Idle", trend: [2, 1, 3, 2, 1, 2, 3, 1], last: 6 },
  { id: "billing", name: "Billing", role: "Lead · Domains", tone: "neutral", word: "Idle", trend: [0, 1, 0, 2, 3, 1, 0, 1], last: 14 },
  { id: "delivery", name: "Delivery", role: "Domains", tone: "warn", word: "Degraded", trend: [3, 2, 0, 0, 1, 0, 0, 0], last: 47 },
  { id: "support", name: "Support", role: "Domains", tone: "ok", word: "Healthy", trend: [2, 3, 2, 4, 3, 5, 4, 3], last: 3 },
  { id: "gate", name: "Gate", role: "Internal", tone: "crit", word: "Stalled", trend: [1, 0, 0, 0, 0, 0, 0, 0], last: 120 },
];

function topologyMenu(density: PanelDensity, setDensity: (d: PanelDensity) => void, ids: boolean, setIds: (on: boolean) => void): MenuEntry[] {
  return standardPanelMenu({
    viewOptions: [
      { kind: "radio", id: "window", label: "Window", value: "1h", options: [{ id: "15m", label: "15 minutes" }, { id: "1h", label: "1 hour" }, { id: "24h", label: "24 hours" }], onChange: () => {} },
      { kind: "checkbox", id: "helpers", label: "Show helpers", checked: true, onChange: () => {} },
      { kind: "checkbox", id: "retired", label: "Show retired", checked: false, onChange: () => {} },
    ],
    density,
    onDensityChange: setDensity,
    showTechnicalIds: ids,
    onShowTechnicalIdsChange: setIds,
    onOpenInSplit: () => {},
    getCopyData: () => ({ agents: AGENTS.length }),
    onRefresh: () => {},
    testIdPrefix: "kit-topology",
  });
}

function TriageDrawer({ onClose }: { onClose?: () => void }) {
  return (
    <DetailDrawer
      title="Triage"
      subtitle="Coordinator · Operations team"
      mark={<AgentAvatar name="Triage" size={28} live />}
      status={<StatusPill tone="live">Working</StatusPill>}
      onClose={onClose}
      onOpenInPane={() => {}}
      autoFocus={false}
      actions={
        <>
          <Button variant="primary" icon={<KitGlyph name="chat" />}>
            Open chat
          </Button>
          <Button>Details</Button>
          <Button>Connect to...</Button>
        </>
      }
      technical={[
        { label: "Identity", value: "identity:triage-01JBX9A1C7E3G9" },
        { label: "Endpoint", value: "ep_7Q3S9U5W1Y7A3C9E" },
        { label: "Session", value: "session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D" },
      ]}
      testId="kit-drawer"
    >
      <DrawerSection label="Now">
        <div className="cc-entity__now">
          <span className="cc-mark" data-tone="live" aria-hidden="true" />
          <span>
            Replying to a support email: “Refund for order 4411”
            <span style={{ display: "block", fontSize: 11, color: "var(--_cc-foreground-meta)" }}>running a tool · refunds.lookup · 12 s</span>
          </span>
        </div>
      </DrawerSection>
      <DrawerSection label="Activity · 24h" action="46 turns">
        <ActivityBars values={[0, 0, 0, 0, 0, 1, 3, 4, 2, 3, 5, 4, 3, 2, 4, 5, 3, 2, 1, 2, 1, 0, 1, 2]} width={300} live label="46 turns in the last 24 hours, most around 10:00 and 15:00" unit="turns" />
      </DrawerSection>
      <DrawerSection label="Connections" count={4}>
        <div className="kg-conns">
          {[
            ["Support", "wired · 12 messages in the last hour"],
            ["Router", "wired"],
            ["Billing", "wired · 3 messages"],
          ].map(([name, rel]) => (
            <div key={name} className="kg-conn">
              <AgentName id={`identity:${name.toLowerCase()}`} avatar={18} fallback={name} directory={undefined} />
              <span className="kg-conn__rel">{rel}</span>
            </div>
          ))}
        </div>
      </DrawerSection>
      <DrawerSection label="Memory">
        <div style={{ fontSize: 12.5 }}>12 records · last given 4m ago</div>
      </DrawerSection>
    </DetailDrawer>
  );
}

function TopologyFrame({ narrow }: { narrow?: boolean }) {
  const [view, setView] = useState<View>("list");
  const [density, setDensity] = useState<PanelDensity>("comfortable");
  const [ids, setIds] = useState(false);
  const [tab, setTab] = useState("agents");
  const [selected, setSelected] = useState<string | null>(narrow ? null : "triage");
  const [refreshing, setRefreshing] = useState(false);
  return (
    <PanelShell density={density} showTechnicalIds={ids} className="topo" testId={narrow ? "kit-frame-narrow" : "kit-frame"}>
      <PanelHeader
        title="Topology"
        scope={
          <SelectField
            label="Topology team"
            showLabel={false}
            variant="chip"
            value="ops"
            options={[
              { value: "ops", label: "Operations team" },
              { value: "research", label: "Research team" },
            ]}
            onChange={() => {}}
          />
        }
        summary={[
          { key: "agents", text: "8 agents" },
          { key: "live", text: "1 working now", tone: "live" },
          { key: "degraded", text: "1 degraded", tone: "warn" },
        ]}
        freshness={{ at: NOW - 12_000, updating: refreshing }}
        refreshing={refreshing}
        view={{
          ariaLabel: "Topology view",
          value: view,
          onChange: (next) => setView(next as View),
          options: [
            { id: "lanes", label: "Lanes", glyph: "lanes" },
            { id: "clusters", label: "Clusters", glyph: "clusters" },
            { id: "roles", label: "Roles", glyph: "tree" },
            { id: "list", label: "List", glyph: "list" },
          ],
        }}
        tools={<IconButton label="Refresh topology" glyph="refresh" onClick={() => setRefreshing((on) => !on)} />}
        menu={{ label: "More actions for Topology", items: topologyMenu(density, setDensity, ids, setIds), triggerTestId: narrow ? undefined : "kit-frame-menu" }}
      />
      <GlanceStrip ariaLabel="Topology at a glance">
        <FigureTile
          hero
          label="Agents"
          value={8}
          meter={<BarcodeMeter filled={7} live={1} outlined={1} segments={24} width={150} label="8 agents: 7 long-lived, 1 helper, 1 working now" />}
          foot={<BarcodeLegend filled={7} outlined={1} live={1} labels={{ live: "working" }} />}
        />
        <FigureTile label="Connections" value={11} foot="7 wired · 3 routes · 1 helper" />
        <FigureTile
          label="Messages · last hour"
          value={48}
          trend={[2, 3, 1, 4, 6, 3, 2, 5, 4, 6, 5, 7]}
          trendLive
          trendLabel="48 messages in the last hour, rising"
          delta={{ value: 12, period: "the hour before" }}
        />
        <FigureTile label="Problems" value={1} tone="warn" foot="Delivery is degraded" />
      </GlanceStrip>
      <PanelTabs
        ariaLabel="Topology sections"
        active={tab}
        onSelect={setTab}
        tabs={[
          { id: "agents", label: "Agents", count: 8 },
          { id: "channels", label: "Channels", count: 3 },
          { id: "helpers", label: "Helpers", count: 1 },
        ]}
      />
      <StatusRow mode="toast" error={refreshing ? "Couldn't refresh the live topology." : null} updatedAt={NOW - 3 * MIN} onRetry={() => setRefreshing(false)} />
      <DrawerLayout
        open={Boolean(selected)}
        onClose={() => setSelected(null)}
        persistKey="kit-gallery"
        backLabel="All agents"
        main={
          <PanelBody>
            {AGENTS.map((agent) => (
              <EntityRow
                key={agent.id}
                kind="agent"
                title={agent.name}
                subtitle={agent.phase ? `${agent.role} · ${agent.phase}` : agent.role}
                mark={<AgentAvatar name={agent.name} size={22} live={agent.live} />}
                status={<StatusPill tone={agent.tone} variant={agent.tone === "neutral" ? "quiet" : "solid"}>{agent.word}</StatusPill>}
                trend={{ values: agent.trend, label: `${agent.name} turns in the last 8 hours`, live: agent.live }}
                time={<RelativeTime value={NOW - agent.last * MIN} />}
                actions={<IconButton label={`More actions for ${agent.name}`} glyph="ellipsis" size="sm" quiet />}
                selected={selected === agent.id}
                live={agent.live}
                onActivate={() => setSelected(agent.id)}
              />
            ))}
          </PanelBody>
        }
        drawer={selected ? <TriageDrawer onClose={() => setSelected(null)} /> : null}
      />
    </PanelShell>
  );
}

function NarrowHeader({ width }: { width: number }) {
  const [view, setView] = useState("graph");
  return (
    <div className="kg-pane" style={{ width, flex: "none" }} data-testid={`kit-narrow-pane:${width}`}>
      <PanelShell>
        <PanelHeader
          title="WorkGraph"
          scope={<SelectField label="Work Graph team" showLabel={false} variant="chip" value="ops" options={[{ value: "ops", label: "Operations team" }]} onChange={() => {}} />}
          summary={[{ text: "9 items" }, { text: "3 in progress", tone: "live" }, { text: "1 blocked", tone: "warn" }]}
          freshness={{ at: NOW - MIN }}
          view={{
            ariaLabel: "WorkGraph view",
            value: view,
            onChange: setView,
            options: [
              { id: "graph", label: "Graph", glyph: "graph", testId: `kit-narrow-view:${width}:graph` },
              { id: "tree", label: "Outline", glyph: "tree", testId: `kit-narrow-view:${width}:tree` },
            ],
          }}
          tools={<IconButton label="Refresh work items" glyph="refresh" data-testid={`kit-narrow-refresh:${width}`} />}
          menu={{ label: "More actions for WorkGraph", items: [{ kind: "item", id: "refresh", label: "Refresh now", onSelect: () => {} }], triggerTestId: `kit-narrow-menu:${width}` }}
        />
      </PanelShell>
    </div>
  );
}

export const gallerySections: GallerySection[] = [
  {
    id: "frame",
    title: "Panel frame",
    order: 30,
    description: "A3.1: header v2 (scope, live summary, view switch, tools, overflow), glance strip, tabs, rows, the docked drawer.",
    render: () => (
      <>
        <Specimen bare>
          <div className="kg-pane" style={{ height: 640 }}>
            <TopologyFrame />
          </div>
        </Specimen>
        <SpecimenRow>
          <Specimen title="Narrow pane" note="420 px: the actions take a row of their own, the view switch shows its glyphs, the strip is 2 x 2" bare width={460}>
            <div className="kg-pane" style={{ height: 520, width: 420, margin: "0 auto 16px" }}>
              <TopologyFrame narrow />
            </div>
          </Specimen>
          <Specimen title="Status toast" note="A3.1: over the body, no reserved space; Retry">
            <div className="kg-pane" style={{ height: 220 }}>
              <PanelShell>
                <PanelHeader title="Approvals" summary={[{ text: "3 pending" }, { text: "next deadline in 4m", tone: "warn" }]} freshness={{ at: NOW - 3 * MIN }} tools={<IconButton label="Refresh approvals" glyph="refresh" />} menu={{ label: "More actions for Approvals", items: [] }} />
                <StatusRow mode="toast" error="Couldn't refresh." updatedAt={NOW - 3 * MIN} onRetry={() => {}} />
                <PanelBody>
                  <div style={{ padding: "56px 20px 0", fontSize: 12.5, color: "var(--_cc-foreground-muted)" }}>The data from 3 minutes ago stays in place under the toast.</div>
                </PanelBody>
              </PanelShell>
            </div>
          </Specimen>
        </SpecimenRow>
        <Specimen title="Narrow headers" note="180, 330 and 500 px panes: the actions wrap to a row of their own; the view switch stays, glyphs only under 480 px">
          <div className="kg-inline" style={{ alignItems: "flex-start", gap: 14 }}>
            {[180, 330, 500].map((width) => (
              <NarrowHeader key={width} width={width} />
            ))}
          </div>
        </Specimen>
      </>
    ),
  },
  {
    id: "menus",
    title: "Menus and view options",
    order: 35,
    description: "A3.2, A5: the overflow menu (commands, checkboxes, radio groups) and the view-options dialog; both live in the top layer.",
    render: () => (
      <>
        <MenusDemo />
        <HeaderActions />
      </>
    ),
  },
];

function MenusDemo() {
  const [density, setDensity] = useState<PanelDensity>("comfortable");
  const [ids, setIds] = useState(false);
  const [show, setShow] = useState("all");
  const [group, setGroup] = useState("category");
  const [sort, setSort] = useState("manual");
  const isDefault = show === "all" && group === "category" && sort === "manual";
  return (
    <SpecimenRow>
      <Specimen title="OverflowMenu" note="click the trigger; arrows, Home, End, typeahead, Escape">
        <div className="kg-inline" style={{ justifyContent: "space-between", minHeight: 360, alignItems: "flex-start" }}>
          <span style={{ fontSize: 12.5, color: "var(--_cc-foreground-muted)" }}>More actions for Topology</span>
          <OverflowMenu label="More actions for Topology" items={topologyMenu(density, setDensity, ids, setIds)} quiet triggerTestId="kit-menu-trigger" testId="kit-menu" />
        </div>
      </Specimen>
      <Specimen title="ViewOptionsMenu" note="the sidebar's view menu: the dot shows a non-default view">
        <div className="kg-inline" style={{ justifyContent: "space-between", minHeight: 360, alignItems: "flex-start" }}>
          <span style={{ display: "flex", flexDirection: "column", gap: 2 }}>
            <span style={{ fontSize: 15, fontWeight: 650 }}>Roster</span>
            <span style={{ fontSize: 11.5, color: "var(--_cc-foreground-meta)" }}>{isDefault ? "7 agents" : `7 agents · ${[show !== "all" ? show : null, sort !== "manual" ? `By ${sort}` : null, group === "none" ? "Ungrouped" : null].filter(Boolean).join(" · ")}`}</span>
          </span>
          <ViewOptionsMenu
            label="Roster view options"
            ariaLabel="Roster view"
            triggerTestId="kit-view-trigger"
            testId="kit-view"
            isDefault={isDefault}
            onReset={() => {
              setShow("all");
              setGroup("category");
              setSort("manual");
            }}
            hint={sort === "manual" ? "Manual order: drag agents to reorder" : "Switch to Manual to drag agents"}
            controls={[
              { label: "Show", ariaLabel: "Agent activity", value: show, onChange: setShow, options: [{ id: "all", label: "All" }, { id: "Working", label: "Working" }, { id: "Quiet", label: "Quiet" }, { id: "Unknown", label: "Unknown" }] },
              { label: "Group by", value: group, onChange: setGroup, options: [{ id: "category", label: "Category" }, { id: "none", label: "None" }] },
              { label: "Sort by", value: sort, onChange: setSort, options: [{ id: "manual", label: "Manual" }, { id: "name", label: "Name" }, { id: "activity", label: "Last activity" }] },
            ]}
          />
        </div>
      </Specimen>
    </SpecimenRow>
  );
}

function MemoryHeader() {
  return (
    <div className="kg-pane">
      <PanelShell>
        <PanelHeader
          title="Memory"
          scope={<SelectField label="Memory team" showLabel={false} variant="chip" value="ops" options={[{ value: "ops", label: "Operations team" }]} onChange={() => {}} />}
          summary={[{ text: "74 records" }, { text: "1 quarantined", tone: "warn" }, { text: "2 waiting", tone: "info" }]}
          freshness={{ at: NOW - MIN }}
          tools={
            <>
              <IconButton label="Search records" glyph="search" />
              <IconButton label="Refresh memory" glyph="refresh" />
            </>
          }
          menu={{ label: "More actions for Memory", items: [{ kind: "item", id: "refresh", label: "Refresh now", onSelect: () => {} }] }}
        />
      </PanelShell>
    </div>
  );
}

function HeaderActions() {
  return (
    <Specimen title="Header actions" note="the tools reveal on hover or focus (always shown under 480 px); the overflow trigger stays, quiet">
      <div className="kg-pair">
        <div>
          <div className="kg-pair__label">at rest</div>
          <MemoryHeader />
        </div>
        <div className="kg-force-reveal">
          <div className="kg-pair__label">on hover or focus</div>
          <MemoryHeader />
        </div>
      </div>
    </Specimen>
  );
}
