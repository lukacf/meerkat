import { createContext, useContext, type ReactNode } from "react";

import {
  EMPTY_AGENT_DIRECTORY,
  agentDisplayName,
  agentInitial,
  type AgentDirectory,
} from "./agents";
import { cx } from "./cx";
import { useConsoleNavigation, wantsAlternate } from "./navigation";

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

export type AgentAvatarSize = 18 | 22 | 28 | 36;

/** An agent's initial on the accent soft fill (18, 22, 28 or 36 px).
 * `live` (working now) adds the accent pulse ring; a static double ring
 * under reduced motion. Decorative: the name beside it carries the words. */
export function AgentAvatar({
  name,
  size = 18,
  live,
  className,
}: {
  name: string;
  size?: AgentAvatarSize;
  live?: boolean;
  className?: string;
}) {
  return (
    <span className={cx("cc-agent__avatar", live && "is-live", className)} data-size={size} aria-hidden="true">
      {agentInitial(name)}
    </span>
  );
}

/**
 * An agent's display name, resolved from the directory, with the raw key in
 * the tooltip. Unknown keys show without their kind prefix ("identity:",
 * "agent:"). Optional initial avatar (18, 22, 28 or 36 px).
 *
 * With `link` and a ConsoleNavigationProvider that can open agents, the
 * name is a button (A4.7): a click opens the agent's details beside the
 * current pane, a Ctrl or Cmd click its chat. It is opt-in, so a name
 * inside another button never nests controls.
 */
export function AgentName({
  id,
  avatar,
  live,
  link,
  directory,
  fallback,
  className,
  testId,
}: {
  id: string | null | undefined;
  avatar?: AgentAvatarSize;
  /** The avatar's live ring (working now). */
  live?: boolean;
  /** A link to the agent when the host can open agents. */
  link?: boolean;
  /** Overrides the provided directory (for hosts without the provider). */
  directory?: AgentDirectory;
  /** Shown when the key is empty. */
  fallback?: string;
  className?: string;
  testId?: string;
}) {
  const provided = useAgentDirectory();
  const navigation = useConsoleNavigation();
  const resolved = agentDisplayName(id, directory ?? provided) || fallback || "";
  const raw = typeof id === "string" ? id.trim() : "";
  if (link && raw && navigation?.openAgent) {
    const openAgent = navigation.openAgent;
    return (
      <button
        type="button"
        className={cx("cc-agent", "cc-agent--link", className)}
        title={raw}
        data-testid={testId}
        onClick={(event) => openAgent.call(navigation, raw, wantsAlternate(event) ? "chat" : "details")}
      >
        {avatar ? <AgentAvatar name={resolved} size={avatar} live={live} /> : null}
        <span className="cc-agent__name">{resolved}</span>
      </button>
    );
  }
  return (
    <span className={cx("cc-agent", className)} title={raw || undefined} data-testid={testId}>
      {avatar ? <AgentAvatar name={resolved} size={avatar} live={live} /> : null}
      <span className="cc-agent__name">{resolved}</span>
    </span>
  );
}
