import React from "react";
import type { IdentityStatusRow } from "@console-core";
import {
  Chip,
  EmptyState,
  ListRow,
  PanelBody,
  PanelHeader,
  PanelShell,
  RelativeTime,
  RowList,
  StatusDot,
  joinMeta,
  plural,
  useAgentDirectory,
} from "../../../packages/console-components/src/panel-kit";
import { agentDisplayTitle } from "../../../packages/console-core/src/agent-title";
import { agentStatusChip, reachabilityWords } from "./AgentDetailsPanel";

/**
 * Health: one row per agent with its status in words and colour, whether it
 * can receive messages and when it last made progress. A row opens the
 * agent's details.
 */
export function HealthPanel({
  identities,
  stateLabel,
  onOpen,
  canOpen,
}: {
  identities: IdentityStatusRow[];
  stateLabel: (row: IdentityStatusRow) => string;
  onOpen?: (identity: string) => void;
  /** Whether a row's agent has details to open; rows that cannot open are
   * not buttons. */
  canOpen?: (identity: string) => boolean;
}): React.JSX.Element {
  const directory = useAgentDirectory();
  // Named the way every other surface names the agent; the identity is the tooltip.
  const titleOf = (row: IdentityStatusRow) => directory.lookup(row.identity)?.label
    || agentDisplayTitle({ label: row.display_name || "", identity: row.identity, role: row.role });
  const rows = identities.map((row) => ({ row, status: agentStatusChip(stateLabel(row), row.progress?.health) }));
  const count = (label: string) => rows.filter((entry) => entry.status.label === label).length;
  const healthy = count("Healthy");
  const degraded = count("Degraded");
  const stalled = count("Stalled");
  const repair = count("Needs repair");
  return (
    <PanelShell className="health" testId="health-panel">
      <PanelHeader
        title="Health"
        subtitle={joinMeta(
          plural(identities.length, "agent"),
          healthy ? `${healthy} healthy` : "",
          degraded ? `${degraded} degraded` : "",
          stalled ? `${stalled} stalled` : "",
          repair ? `${repair} need${repair === 1 ? "s" : ""} repair` : "",
        )}
      />
      <PanelBody>
        {rows.length === 0 ? (
          <EmptyState title="No agents to report on yet" body="Health appears here once a team is running." />
        ) : (
          <RowList>
            {rows.map(({ row, status }) => {
              const at = row.progress && row.progress.last_progress_at_ms > 0 ? row.progress.last_progress_at_ms : null;
              return (
                <ListRow
                  key={row.identity}
                  testId={`health-identity:${row.identity}`}
                  onActivate={onOpen && (canOpen ? canOpen(row.identity) : true) ? () => onOpen(row.identity) : undefined}
                  lead={<StatusDot tone={status.tone} />}
                  title={titleOf(row)}
                  titleAttr={row.identity}
                  subtitle={reachabilityWords(row.addressability)}
                  meta={
                    <>
                      <Chip tone={status.tone}>{status.label}</Chip>
                      {at ? <RelativeTime value={at} /> : null}
                    </>
                  }
                />
              );
            })}
          </RowList>
        )}
      </PanelBody>
    </PanelShell>
  );
}
