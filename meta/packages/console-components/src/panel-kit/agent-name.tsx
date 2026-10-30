import { createContext, useContext, type ReactNode } from "react";

import {
  EMPTY_AGENT_DIRECTORY,
  agentDisplayName,
  agentInitial,
  type AgentDirectory,
} from "./agents";
import { cx } from "./cx";

const AgentDirectoryContext = createContext<AgentDirectory>(EMPTY_AGENT_DIRECTORY);

/** Provides the agent directory to every AgentName below it. The host builds
 * the directory from the roster it already has; no panel requests names. */
export function AgentDirectoryProvider({
  directory,
  children,
}: {
  directory: AgentDirectory;
  children?: ReactNode;
}) {
  return <AgentDirectoryContext.Provider value={directory}>{children}</AgentDirectoryContext.Provider>;
}

export function useAgentDirectory(): AgentDirectory {
  return useContext(AgentDirectoryContext);
}

export type AgentAvatarSize = 18 | 22 | 28;

export function AgentAvatar({
  name,
  size = 18,
  className,
}: {
  name: string;
  size?: AgentAvatarSize;
  className?: string;
}) {
  return (
    <span className={cx("cc-agent__avatar", className)} data-size={size} aria-hidden="true">
      {agentInitial(name)}
    </span>
  );
}

/**
 * An agent's display name, resolved from the directory, with the raw key in
 * the tooltip. Unknown keys show without their kind prefix ("identity:",
 * "agent:"). Optional initial avatar (18, 22 or 28 px).
 */
export function AgentName({
  id,
  avatar,
  directory,
  fallback,
  className,
  testId,
}: {
  id: string | null | undefined;
  avatar?: AgentAvatarSize;
  /** Overrides the provided directory (for hosts without the provider). */
  directory?: AgentDirectory;
  /** Shown when the key is empty. */
  fallback?: string;
  className?: string;
  testId?: string;
}) {
  const provided = useAgentDirectory();
  const resolved = agentDisplayName(id, directory ?? provided) || fallback || "";
  const raw = typeof id === "string" ? id.trim() : "";
  return (
    <span className={cx("cc-agent", className)} title={raw || undefined} data-testid={testId}>
      {avatar ? <AgentAvatar name={resolved} size={avatar} /> : null}
      <span className="cc-agent__name">{resolved}</span>
    </span>
  );
}
