import { useState } from "react";

import { AgentAvatar, AgentDirectoryProvider, AgentName } from "../agent-name";
import { createAgentDirectory } from "../agents";
import { Button, IconButton } from "../button";
import { StatusPill } from "../chip";
import { Countdown } from "../countdown";
import { DetailDrawer, DrawerLayout, DrawerSection } from "../drawer";
import { EmptyIllustration, type IllustrationName } from "../empty-illustration";
import { EntityCard, EntityRow, TrustLadder } from "../entity-card";
import { FilterBar } from "../filter-bar";
import { KitGlyph } from "../glyphs";
import { NewItemsPill } from "../live";
import { ConsoleNavigationProvider, EntityLink } from "../navigation";
import { PanelBody, PanelHeader, PanelShell } from "../panel";
import { RelativeTime, RelativeTimeChip } from "../relative-time";
import { EmptyState, LoadingState, Notice } from "../states";
import { TechnicalDetails } from "../technical-details";
import { Item, Specimen, SpecimenRow } from "./specimen";
import type { GallerySection } from "./types";

const NOW = Date.now();
const MIN = 60_000;
const HOUR = 60 * MIN;

const DIRECTORY = createAgentDirectory([
  { id: "identity:router", label: "Router" },
  { id: "identity:support", label: "Support" },
  { id: "identity:billing", label: "Billing" },
  { id: "identity:triage", label: "Triage" },
  { id: "identity:release", label: "Release Manager" },
]);

function StatusSquare({ status }: { status: "open" | "ready" | "live" | "blocked" | "done" }) {
  // A stand-in for the graph kit's StatusSquare (K2), so the gallery shows a
  // task row without depending on it.
  return (
    <svg className="kg-square" data-status={status} viewBox="0 0 12 12" width={12} height={12} style={{ width: 12, height: 12 }} aria-hidden="true">
      <rect className="kg-square__box" x={0.75} y={0.75} width={10.5} height={10.5} rx={2} />
      {status === "blocked" ? <path className="kg-square__slash" d="M3 9 9 3" /> : null}
    </svg>
  );
}

const ILLUSTRATIONS: IllustrationName[] = ["agents", "connections", "work", "memory", "approvals-clear", "timeline", "routes", "search", "no-access", "not-configured", "error"];

function DrawerModes() {
  return (
    <>
      {[
        [760, "an overlay drawer (e3) over the right of the list"],
        [520, "the drawer replaces the list under a Back row; the list keeps its scroll"],
      ].map(([width, note]) => (
        <Specimen key={width} title={`${width} px pane`} note={note as string} bare>
          <div className="kg-pane" style={{ height: 360, width: width as number, maxWidth: "100%", margin: "0 auto 14px" }}>
            <PanelShell>
              <PanelHeader title="Roster" summary={[{ text: "8 agents" }, { text: "2 working", tone: "live" }]} />
              <DrawerLayout
                open
                onClose={() => {}}
                backLabel="All agents"
                persistKey="kit-gallery-modes"
                main={
                  <PanelBody>
                    {["Triage", "Router", "Billing", "Support", "Gate"].map((name) => (
                      <EntityRow key={name} kind="agent" title={name} subtitle="Domains" mark={<AgentAvatar name={name} size={22} />} time={<RelativeTime value={NOW - 7 * MIN} />} onActivate={() => {}} selected={name === "Billing"} />
                    ))}
                  </PanelBody>
                }
                drawer={
                  <DetailDrawer title="Billing" subtitle="Lead · Domains" mark={<AgentAvatar name="Billing" size={28} />} status={<StatusPill tone="ok">Healthy</StatusPill>} onClose={() => {}} autoFocus={false}>
                    <DrawerSection label="Now">
                      <div style={{ fontSize: 12.5 }}>Idle · last active 7m ago</div>
                    </DrawerSection>
                    <DrawerSection label="Connected to" count={3}>
                      <div className="kg-inline" style={{ gap: 6 }}>
                        <AgentName id="identity:router" avatar={18} />
                        <AgentName id="identity:support" avatar={18} />
                        <AgentName id="identity:triage" avatar={18} />
                      </div>
                    </DrawerSection>
                  </DetailDrawer>
                }
              />
            </PanelShell>
          </div>
        </Specimen>
      ))}
    </>
  );
}

function FilterDemo() {
  const [query, setQuery] = useState("bil");
  const [working, setWorking] = useState(true);
  const [blocked, setBlocked] = useState(false);
  const [mine, setMine] = useState(false);
  return (
    <div className="kg-pane">
      <PanelShell>
        <FilterBar
          search={{ label: "Search agents", value: query, onChange: setQuery }}
          chips={[
            { key: "working", label: "Working", count: 2, tone: "live", pressed: working, onToggle: setWorking },
            { key: "blocked", label: "Blocked", count: 2, tone: "warn", pressed: blocked, onToggle: setBlocked },
            { key: "mine", label: "Mine", pressed: mine, onToggle: setMine },
            { key: "facts", label: "Facts", count: 12, glyph: "k-fact", pressed: false, onToggle: () => {} },
          ]}
          onClear={() => {
            setQuery("");
            setWorking(false);
            setBlocked(false);
            setMine(false);
          }}
        />
        <EmptyState
          kind="filtered"
          illustration="search"
          title={`No agents match “${query}” and Working.`}
          body="Try another name or clear the filters."
          action={<Button size="sm" onClick={() => setQuery("")}>Clear filters</Button>}
        />
      </PanelShell>
    </div>
  );
}

function LiveDemo() {
  const [rows, setRows] = useState([{ id: 1, at: NOW - 2 * MIN }]);
  const [pending, setPending] = useState(3);
  return (
    <div className="kg-stack">
      <div className="kg-inline">
        <Button size="sm" onClick={() => setRows((current) => [{ id: current.length + 1, at: Date.now() }, ...current])}>
          Add a live row
        </Button>
        <NewItemsPill count={pending} onClick={() => setPending(0)} />
      </div>
      <div className="kg-pane">
        <PanelShell>
          {rows.slice(0, 4).map((row) => (
            <EntityRow
              key={row.id}
              kind="approval"
              className={row.id > 1 ? "cc-enter-live" : undefined}
              title={`Event ${row.id}: Support replied to a refund email`}
              time={<RelativeTime value={row.at} />}
            />
          ))}
        </PanelShell>
      </div>
    </div>
  );
}

export const gallerySections: GallerySection[] = [
  {
    id: "entities",
    title: "Entities",
    order: 50,
    description: "A3.4: one part, two layouts. The title is the button; actions reveal on hover; live draws the accent rule.",
    render: () => (
      <AgentDirectoryProvider directory={DIRECTORY}>
        <SpecimenRow>
          <Specimen title="EntityCard" note="agent">
            <div className="kg-cards">
              <EntityCard
                kind="agent"
                title="Triage"
                subtitle="Coordinator · Leads"
                mark={<AgentAvatar name="Triage" size={28} live />}
                status={<StatusPill tone="live">Working</StatusPill>}
                now={{ text: "Replying to a refund email", live: true }}
                trend={{ values: [1, 2, 4, 3, 5, 6, 4, 7, 5, 6], label: "12 turns today", live: true }}
                figures={["12 turns today"]}
                time={<RelativeTimeChip value={NOW - MIN} />}
                relations={
                  <>
                    <KitGlyph name="link" className="kg-rel-glyph" />
                    <AgentName id="identity:router" avatar={18} />
                    <AgentName id="identity:support" avatar={18} />
                    <span>+1</span>
                  </>
                }
                actions={
                  <>
                    <IconButton label="Open chat" glyph="chat" size="sm" />
                    <IconButton label="More actions for Triage" glyph="ellipsis" size="sm" />
                  </>
                }
                technical={[{ label: "Identity", value: "identity:triage" }, { label: "Session", value: "session-01JBX4N0Q3T9" }]}
                live
                onActivate={() => {}}
              />
              <EntityCard
                kind="agent"
                title="Release Manager"
                subtitle="Release · Specialists"
                mark={<AgentAvatar name="Release Manager" size={28} />}
                status={<StatusPill tone="warn">Degraded</StatusPill>}
                now={{ text: "Degraded · idle 18m" }}
                trend={{ values: [0, 1, 1, 0, 2, 1, 0, 0, 1, 0], label: "2 turns today" }}
                figures={["2 turns today"]}
                time={<RelativeTimeChip value={NOW - 18 * MIN} />}
                relations={<AgentName id="identity:triage" avatar={18} />}
                selected
                onActivate={() => {}}
              />
            </div>
          </Specimen>
        </SpecimenRow>
        <Specimen title="EntityRow" note="agent, task and memory kinds" bare>
          <div className="kg-pane" style={{ border: 0, borderRadius: 0 }}>
            <PanelShell>
              <EntityRow
                kind="agent"
                title="Billing"
                subtitle="Lead · Domains"
                mark={<AgentAvatar name="Billing" size={22} />}
                trend={{ values: [1, 0, 2, 3, 1, 0, 1, 2], label: "Turns over 8 hours" }}
                status={<StatusPill tone="neutral" variant="quiet">Idle</StatusPill>}
                time={<RelativeTime value={NOW - 4 * MIN} />}
                actions={<IconButton label="More actions for Billing" glyph="ellipsis" size="sm" />}
                onActivate={() => {}}
              />
              <EntityRow
                kind="task"
                title="Publish to all regions"
                subtitle="Ship the 2.4 release · Research Analyst"
                mark={<StatusSquare status="blocked" />}
                figures={[<StatusPill key="p" tone="crit" variant="outline">High</StatusPill>, <StatusPill key="b" tone="warn">Blocked by 1</StatusPill>]}
                time={<span>3d</span>}
                onActivate={() => {}}
              />
              <EntityRow
                kind="task"
                title="Run the regression suite"
                subtitle="Ship the 2.4 release · Release Manager"
                mark={<StatusSquare status="live" />}
                now={{ text: "Release Manager is working on it", live: true }}
                time={<Countdown to={NOW + 25 * MIN} />}
                live
                onActivate={() => {}}
              />
              <EntityRow
                kind="memory"
                title="Refunds over 500 need a second approver"
                subtitle="Billing's memory"
                mark={<KitGlyph name="k-procedure" className="kg-kind-glyph" />}
                figures={[<TrustLadder key="t" level={4} />, <span key="u">given 4 times · useful 2</span>]}
                time={<RelativeTime value={NOW - 5 * HOUR} />}
                onActivate={() => {}}
              />
            </PanelShell>
          </div>
        </Specimen>
      </AgentDirectoryProvider>
    ),
  },
  {
    id: "time",
    title: "Time",
    order: 55,
    description: "A3.5: relative in rows, clock time only on axes; countdowns tone by distance and tick under two minutes.",
    render: () => (
      <Specimen>
        <div className="kg-inline" style={{ gap: "16px 28px" }}>
          <Item label="RelativeTime">
            <RelativeTime value={NOW - 4 * MIN} />
          </Item>
          <Item label="RelativeTimeChip">
            <RelativeTimeChip value={NOW - 2 * HOUR} />
          </Item>
          <Item label="more than 1 h">
            <Countdown to={NOW + (2 * 24 + 4) * HOUR} chip />
          </Item>
          <Item label="10 min to 1 h">
            <Countdown to={NOW + 25 * MIN} chip />
          </Item>
          <Item label="under 10 min">
            <Countdown to={NOW + 4 * MIN + 12_000} chip />
          </Item>
          <Item label="under 2 min, ticking">
            <Countdown to={NOW + 95_000} chip />
          </Item>
          <Item label="past due">
            <Countdown to={NOW - 5 * MIN} chip />
          </Item>
          <Item label="inline">
            <Countdown to={NOW + 25 * MIN} />
          </Item>
        </div>
      </Specimen>
    ),
  },
  {
    id: "technical",
    title: "Technical details",
    order: 58,
    description: "A3.6: ids at the last level of disclosure, counted, each copyable, Copy all; inline when Show technical IDs is on.",
    render: () => (
      <SpecimenRow>
        <Specimen title="Disclosure">
          <TechnicalDetails
            open
            items={[
              { label: "Identity", value: "identity:billing-01JBX9A1C7E3G9J5L1N7Q3S9U5" },
              { label: "Endpoint", value: "ep_7Q3S9U5W1Y7A3C9E" },
              { label: "Session", value: "session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D" },
              { label: "Generation", value: 7 },
            ]}
          />
        </Specimen>
        <Specimen title="Inline" note="Show technical IDs on">
          <TechnicalDetails
            inline
            items={[
              { label: "Identity", value: "identity:billing-01JBX9A1C7E3G9J5L1N7Q3S9U5" },
              { label: "Session", value: "session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D" },
            ]}
          />
        </Specimen>
      </SpecimenRow>
    ),
  },
  {
    id: "states",
    title: "States",
    order: 60,
    description: "A3.7: every state is designed. Skeletons of the real layout; empty states that teach; illustrations in the graph grammar.",
    render: () => (
      <>
        <Specimen title="Illustrations" note="120 x 72; at most one accent element">
          <div className="kg-grid" style={{ gridTemplateColumns: "repeat(auto-fill, minmax(150px, 1fr))" }}>
            {ILLUSTRATIONS.map((name) => (
              <Item key={name} label={name}>
                <EmptyIllustration name={name} />
              </Item>
            ))}
          </div>
        </Specimen>
        <SpecimenRow>
          <Specimen title="Empty" bare>
            <EmptyState
              illustration="agents"
              title="No agents in this team yet"
              body="Agents appear here when the team starts them."
              action={<Button size="sm" variant="primary">Create an agent</Button>}
            />
          </Specimen>
          <Specimen title="Error, no data" bare>
            <EmptyState
              kind="error"
              illustration="error"
              title="Couldn't load approvals"
              body="The gateway refused the request."
              action={<Button size="sm">Retry</Button>}
              details={[{ label: "Code", value: "-32602", plain: true }, { label: "Message", value: "invalid params", plain: true }]}
            />
          </Specimen>
          <Specimen title="Not set up" bare>
            <EmptyState
              kind="not-configured"
              illustration="not-configured"
              title="Memory isn't set up on this runtime."
              body="Agents remember facts, preferences and procedures here once a memory store is configured."
            />
          </Specimen>
          <Specimen title="No access" bare>
            <EmptyState kind="no-access" illustration="no-access" title="Routing needs runtime admin" body="Ask an admin for the routing grant." />
          </Specimen>
        </SpecimenRow>
        <SpecimenRow>
          <Specimen title="Loading: graph" bare>
            <LoadingState variant="graph" label="Loading the team map" />
          </Specimen>
          <Specimen title="Loading: timeline" bare>
            <LoadingState variant="timeline" label="Loading today" />
          </Specimen>
          <Specimen title="Loading: rows and tiles" bare>
            <LoadingState variant="tiles" rows={4} label="Loading figures" />
            <LoadingState rows={4} label="Loading records" />
          </Specimen>
        </SpecimenRow>
        <Specimen title="Notices" note="tone icons carry their shape">
          <div className="kg-stack">
            <Notice tone="info">Showing activity since 14:02, when this console connected.</Notice>
            <Notice tone="ok">Saved. The rule applies to new requests.</Notice>
            <Notice tone="warn" action={<Button size="sm">Review</Button>}>
              2 records wait for approval before they reach team memory.
            </Notice>
            <Notice tone="crit">The delivery to support.inbound.email failed 3 times.</Notice>
            <Notice tone="live">Triage is replying now.</Notice>
          </div>
        </Specimen>
        <Specimen title="Inline empty section" bare>
          <EmptyState inline illustration="routes" title="No routes feed this agent." body="Routes appear when a channel delivers here." />
        </Specimen>
      </>
    ),
  },
  {
    id: "drawer",
    title: "Detail drawer",
    order: 70,
    description: "A3.10: docked from 900 px (see the panel frame), an overlay from 600 to 900 px, the whole view below 600 px.",
    render: () => (
      <AgentDirectoryProvider directory={DIRECTORY}>
        <DrawerModes />
      </AgentDirectoryProvider>
    ),
  },
  {
    id: "filters",
    title: "Filter bar and links",
    order: 75,
    description: "A3.11, A4.7: search ('/' focuses it), toggle chips with counts, Clear when not default; entity names link across panels.",
    render: () => (
      <SpecimenRow>
        <Specimen title="FilterBar" bare>
          <FilterDemo />
        </Specimen>
        <Specimen title="Links" note="with a navigation provider; Ctrl or Cmd opens the chat">
          <ConsoleNavigationProvider value={{ openAgent: () => {}, openWorkItem: () => {}, openApproval: () => {} }}>
            <AgentDirectoryProvider directory={DIRECTORY}>
              <div className="kg-stack" style={{ fontSize: 12.5 }}>
                <span>
                  <AgentName id="identity:support" avatar={18} link /> asked <AgentName id="identity:billing" avatar={18} link /> about refund 4411.
                </span>
                <span>
                  Blocked by <EntityLink kind="work-item" id="item-7">Run the regression suite</EntityLink>; waiting for{" "}
                  <EntityLink kind="approval" id="ap-3">Deploy release 2.4</EntityLink>.
                </span>
              </div>
            </AgentDirectoryProvider>
          </ConsoleNavigationProvider>
        </Specimen>
      </SpecimenRow>
    ),
  },
  {
    id: "interaction",
    title: "Interaction and density",
    order: 80,
    description: "A4: controls reveal on hover or focus; live rows enter and hold still; compact density from the overflow menu.",
    render: () => (
      <>
        <SpecimenRow>
          <Specimen title="Hover reveal" note="tab into the row: the actions stay in the tab order">
            <div className="kg-reveal-row cc-reveal-host">
              <span className="kg-reveal-row__text">Support Triage</span>
              <span className="cc-reveal kg-inline" style={{ gap: 2 }}>
                <IconButton label="Pin" glyph="flag" size="sm" />
                <IconButton label="Copy identity" glyph="copy" size="sm" />
              </span>
              <IconButton label="More actions for Support Triage" glyph="ellipsis" size="sm" quiet />
            </div>
          </Specimen>
          <Specimen title="Live rows" note="200 ms entrance, the accent rule fades over 1.2 s">
            <LiveDemo />
          </Specimen>
        </SpecimenRow>
        <Specimen title="Density" note="comfortable 40 px rows (48 with a subtitle); compact 30 px (38 with a subtitle), 8 x 10 card padding, 16 px figures">
          <div className="kg-pair">
            {(["comfortable", "compact"] as const).map((density) => (
              <div key={density}>
                <div className="kg-pair__label">{density}</div>
                <div className="kg-pane">
                  <PanelShell density={density}>
                    {["Triage", "Router"].map((name) => (
                      <EntityRow key={name} kind="agent" title={name} subtitle="Coordinator · Leads" mark={<AgentAvatar name={name} size={density === "compact" ? 18 : 22} />} time={<RelativeTime value={NOW - 9 * MIN} />} />
                    ))}
                    {["Billing", "Support"].map((name) => (
                      <EntityRow key={name} kind="agent" title={name} mark={<AgentAvatar name={name} size={density === "compact" ? 18 : 22} />} status={<StatusPill tone="neutral" variant="quiet">Idle</StatusPill>} time={<RelativeTime value={NOW - 14 * MIN} />} />
                    ))}
                  </PanelShell>
                </div>
              </div>
            ))}
          </div>
        </Specimen>
      </>
    ),
  },
];
