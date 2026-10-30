import React from "react";
import type { IdentityInspectViewState } from "@console-core";
import type { ConsoleAgent } from "../types";
import { agentDisplayTitle } from "../lib/agents";
import { CopyButton } from "../../../packages/console-components/src/copy-button";
import {
  AgentAvatar,
  AgentName,
  Button,
  Chip,
  KeyValueList,
  LoadingState,
  Notice,
  PanelBody,
  PanelHeader,
  PanelSection,
  PanelShell,
  RelativeTime,
  TechnicalDetails,
  agentDisplayName,
  humanizeEnum,
  joinMeta,
  statusTone,
  useAgentDirectory,
  type Tone,
} from "../../../packages/console-components/src/panel-kit";

export interface AgentDetailsAction {
  label: string;
  onClick: () => void;
}

/** Status words and tone for an identity: the repair hold and health problems
 * outrank the lifecycle state. */
export function agentStatusChip(
  stateLabel: string,
  health: string | null | undefined,
): { label: string; tone: Tone } {
  const state = stateLabel.trim().toLowerCase();
  if (state === "needs repair") return { label: "Needs repair", tone: "warn" };
  if (state === "retired" || state === "retiring" || state === "stopped") {
    return { label: humanizeEnum(state), tone: "neutral" };
  }
  if (health === "wedged") return { label: "Stalled", tone: "crit" };
  if (health === "degraded") return { label: "Degraded", tone: "warn" };
  if (health === "healthy" && state === "active") return { label: "Healthy", tone: "ok" };
  return { label: humanizeEnum(state || "unknown"), tone: statusTone(state) };
}

export function reachabilityWords(addressability: string | null | undefined): string {
  if (addressability === "addressable") return "Can receive messages";
  if (addressability === "internal_only") return "Internal only";
  return humanizeEnum(addressability || "");
}

function CommandBlock({ command, testId, label }: { command: string; testId: string; label: string }) {
  return (
    <div className="agent-details__command">
      <code data-testid={testId}>{command}</code>
      <CopyButton text={command} label={`Copy the ${label} command`} className="agent-details__command-copy" />
    </div>
  );
}

/**
 * Agent details: who the agent is (avatar, display name, role, status), what
 * it is doing and whom it talks to, in words; the identity, session, runtime
 * and generation stay in the collapsed technical details.
 */
export function AgentDetailsPanel({
  identity,
  inspect,
  agent,
  stateLabel,
  actions,
  children,
}: {
  identity: string;
  inspect: IdentityInspectViewState | undefined;
  agent: ConsoleAgent | undefined;
  /** The state label the console shows (the repair hold reads "needs repair"). */
  stateLabel: string | null;
  actions: { respawn?: AgentDetailsAction; reset?: AgentDetailsAction; retire?: AgentDetailsAction };
  /** The schedules section, when offered. */
  children?: React.ReactNode;
}): React.JSX.Element {
  const directory = useAgentDirectory();
  // Named the way the sidebar names the agent.
  const name = agent
    ? agentDisplayTitle({ ...agent, label: agent.label || inspect?.display_name || "" })
    : inspect?.display_name || agentDisplayName(identity, directory) || identity;
  const role = inspect?.role || agent?.role;
  // The gateway's inspect payload carries no progress; the roster row does.
  const health = inspect?.progress?.health ?? agent?.progress?.health;
  const status = stateLabel !== null ? agentStatusChip(stateLabel, health) : null;
  const repair = inspect?.session_repair;
  const peers = inspect?.topology_peers || [];
  const progress = inspect?.progress ?? agent?.progress;
  const lastActivity = inspect?.last_activity_ms && inspect.last_activity_ms > 0
    ? inspect.last_activity_ms
    : progress && progress.last_progress_at_ms > 0 ? progress.last_progress_at_ms : null;
  return (
    <PanelShell className="agent-details" testId={`inspect-panel:${identity}`}>
      <PanelHeader
        lead={<AgentAvatar name={name} size={28} />}
        title={<span title={identity}>{name}</span>}
        subtitle={joinMeta(humanizeEnum(role || ""), agent?.group ? humanizeEnum(agent.group) : "") || undefined}
        actions={
          <>
            {status ? <Chip tone={status.tone}>{status.label}</Chip> : null}
            {actions.respawn ? (
              <Button data-testid={`inspect-action:${identity}:respawn`} onClick={actions.respawn.onClick}>
                {actions.respawn.label}
              </Button>
            ) : null}
            {actions.reset ? (
              <Button data-testid={`inspect-action:${identity}:reset`} onClick={actions.reset.onClick}>
                {actions.reset.label}
              </Button>
            ) : null}
            {actions.retire ? (
              <Button variant="danger" data-testid={`inspect-action:${identity}:retire`} onClick={actions.retire.onClick}>
                {actions.retire.label}
              </Button>
            ) : null}
          </>
        }
      />
      <PanelBody>
        {!inspect ? (
          <LoadingState label="Loading agent details" rows={4} />
        ) : (
          <>
            {repair ? (
              <div className="agent-details__repair" data-testid={`inspect-session-repair:${identity}`}>
                <Notice tone="warn">
                  <p>
                    The saved session is intact but refused until it is repaired. Run the diagnose
                    command, then the repair command, then reload the member
                    (<code>meta/reload_member</code>).
                  </p>
                  <CommandBlock command={repair.diagnose_command} testId="inspect-session-repair-diagnose" label="diagnose" />
                  <CommandBlock command={repair.apply_command} testId="inspect-session-repair-apply" label="repair" />
                </Notice>
              </div>
            ) : null}
            <PanelSection label="Overview" padded>
              <KeyValueList
                items={[
                  {
                    label: "Status",
                    value: (
                      <span data-testid={`inspect-state:${identity}`}>
                        {status ? status.label : humanizeEnum(stateLabel || "")}
                      </span>
                    ),
                  },
                  role ? { label: "Role", value: humanizeEnum(role) } : null,
                  { label: "Reachability", value: reachabilityWords(inspect.addressability) },
                  lastActivity ? { label: "Last activity", value: <RelativeTime value={lastActivity} /> } : null,
                  {
                    label: "Connected to",
                    value: peers.length ? (
                      <span className="cc-chips">
                        {peers.map((peer) => (
                          <span className="agent-details__peer" key={peer}>
                            <AgentName id={peer} avatar={18} />
                          </span>
                        ))}
                      </span>
                    ) : (
                      "Nobody yet"
                    ),
                  },
                ]}
              />
            </PanelSection>
            {inspect.output_preview ? (
              <PanelSection label="Latest output" padded>
                <blockquote className="agent-details__preview">{inspect.output_preview}</blockquote>
              </PanelSection>
            ) : null}
            <div className="agent-details__tech">
              <TechnicalDetails
                items={[
                  { label: "Identity", value: identity },
                  { label: "Session", value: inspect.continuity?.session_id },
                  { label: "Runtime", value: inspect.continuity?.agent_runtime_id },
                  typeof inspect.continuity?.generation === "number"
                    ? { label: "Generation", value: inspect.continuity.generation }
                    : null,
                ]}
              />
            </div>
          </>
        )}
        {children}
      </PanelBody>
    </PanelShell>
  );
}
