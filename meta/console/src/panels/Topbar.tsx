import React from "react";
import { Chip } from "../../../packages/console-components/src/panel-kit/chip";
import { Button, IconButton } from "../../../packages/console-components/src/panel-kit/button";
import { Popover } from "../../../packages/console-components/src/panel-kit/popover";
import { ViewOptions, type ViewOptionsControl } from "../../../packages/console-components/src/panel-kit/view-options";
import { Icon } from "../icon";
import type { ConsoleDisplayKey, ConsoleDisplaySettings, ConsoleTheme } from "./Tweaks";

/** Who the console acts as (`meta/console/whoami`), with the host's sign-out. */
export interface TopbarIdentity {
  /** The kind of caller (`person`, `token_principal`, `service`, `anonymous`). */
  kind: string;
  /** The caller's own principal, or words for a console nobody signed in to. */
  label: string;
  /** How the console knows the caller, in words. */
  detail: string;
  /** The host's sign-out (console configuration `[sign_out]`). */
  signOut?: { href: string; method: "get" | "post" };
  /** Runs before the browser leaves for the sign-out; a returned promise
   * holds the browser until it settled. */
  onSignOut?: () => Promise<void> | void;
}

/** Runs the sign-out first: when it returns a promise, the browser leaves
 * (`leave`) once that settled instead of at once. */
function signOutThen(event: React.SyntheticEvent, onSignOut: TopbarIdentity["onSignOut"], leave: () => void): void {
  const pending = onSignOut?.();
  if (!pending) return;
  event.preventDefault();
  void pending.then(leave, leave);
}

/** Posts to the host's sign-out from a form of its own, so the browser
 * leaves even when the popover closed in the meantime. */
function postSignOut(href: string): void {
  const form = document.createElement("form");
  form.method = "post";
  form.action = href;
  form.hidden = true;
  document.body.append(form);
  form.submit();
}

interface TopbarProps {
  mobName: string;
  connectionStatus?: React.ReactNode;
  /** Who the console acts as; without it the identity chip is absent. */
  identity?: TopbarIdentity;
  brandLabel?: string;
  brandLogoUrl?: string;
  brandLogoAlt?: string;
  mobStatus?: string;
  environment?: string;
  theme: ConsoleTheme;
  onToggleTheme: () => void;
  /** The Display settings (B21); without them the Display button is absent. */
  display?: ConsoleDisplaySettings;
  /** Whether the user has made any display choice of their own. */
  displayChosen?: boolean;
  onChooseDisplay?: <K extends ConsoleDisplayKey>(key: K, value: ConsoleDisplaySettings[K]) => void;
  onResetDisplay?: () => void;
  sidebarCollapsed: boolean;
  railCollapsed: boolean;
  railVisible?: boolean;
  onToggleSidebar: () => void;
  onToggleRail: () => void;
}

/// Lucide-style "panel" glyph: a rounded rectangle with an inner
/// divider on the side that owns the toggle, and a chevron pointing
/// inward when the panel is open (about to collapse) or outward when
/// the panel is collapsed (about to expand).
function PanelGlyph({ side, open }: { side: "left" | "right"; open: boolean }): React.JSX.Element {
  const dividerLeft = side === "left";
  const cx = dividerLeft ? 16.5 : 7.5;
  const point = open ? (dividerLeft ? 1 : -1) : (dividerLeft ? -1 : 1);
  const x1 = cx + point * 1.6;
  const x2 = cx - point * 1.6;
  return (
    <svg
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      <rect x="3" y="5" width="18" height="14" rx="1.5" />
      <path d={dividerLeft ? "M9 5 L9 19" : "M15 5 L15 19"} />
      <path d={`M${x1} 9.5 L${x2} 12 L${x1} 14.5`} />
    </svg>
  );
}

/** The team's state in words: "running", "stopped", "not observed". */
export function teamStateWords(status: string): string {
  switch (status) {
    case "unknown":
      return "not observed";
    default:
      return status.replace(/_/g, " ");
  }
}

/**
 * The Display popover (B21): theme, style, density (the default for panels
 * without their own choice) and motion, applied at once. Reset forgets the
 * user's choices, so the configured defaults apply again.
 */
function DisplayOptions({
  display,
  chosen,
  onChoose,
  onReset,
}: {
  display: ConsoleDisplaySettings;
  chosen: boolean;
  onChoose: <K extends ConsoleDisplayKey>(key: K, value: ConsoleDisplaySettings[K]) => void;
  onReset: () => void;
}): React.JSX.Element {
  const [open, setOpen] = React.useState(false);
  const triggerRef = React.useRef<HTMLButtonElement | null>(null);
  const popoverId = `${React.useId()}-display`;
  const controls: ReadonlyArray<ViewOptionsControl<string>> = [
    {
      label: "Theme",
      testId: "display-theme",
      value: display.theme,
      options: [
        { id: "light", label: "Light", testId: "display-theme:light" },
        { id: "dark", label: "Dark", testId: "display-theme:dark" },
      ],
      onChange: (id: string) => onChoose("theme", id === "dark" ? "dark" : "light"),
    },
    {
      label: "Style",
      testId: "display-style",
      value: display.variant,
      options: [
        { id: "rams", label: "Rams", testId: "display-style:rams" },
        { id: "terminal", label: "Terminal", testId: "display-style:terminal" },
        { id: "graphite", label: "Graphite", testId: "display-style:graphite" },
      ],
      onChange: (id: string) => onChoose("variant", id === "terminal" || id === "graphite" ? id : "rams"),
    },
    {
      label: "Density",
      testId: "display-density",
      value: display.density,
      options: [
        { id: "comfortable", label: "Comfortable", testId: "display-density:comfortable" },
        { id: "compact", label: "Compact", testId: "display-density:compact" },
      ],
      onChange: (id: string) => onChoose("density", id === "compact" ? "compact" : "comfortable"),
      hint: "The roster, and panels without their own choice.",
    },
    {
      label: "Motion",
      testId: "display-motion",
      value: display.motion,
      options: [
        { id: "system", label: "System", testId: "display-motion:system" },
        { id: "reduced", label: "Reduced", testId: "display-motion:reduced" },
      ],
      onChange: (id: string) => onChoose("motion", id === "reduced" ? "reduced" : "system"),
      hint: display.motion === "reduced" ? "Pulses and movement stop." : "Follows your system setting.",
    },
  ];
  return (
    <>
      <IconButton
        ref={triggerRef}
        label="Display"
        title="Display settings"
        icon={<Icon name="i-sliders" />}
        badge={chosen}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? popoverId : undefined}
        className="meta-topbar__icon"
        data-testid="display-options"
        onClick={() => setOpen((value) => !value)}
      />
      <ViewOptions
        open={open}
        anchorRef={triggerRef}
        onClose={() => setOpen(false)}
        ariaLabel="Display"
        controls={controls}
        onReset={onReset}
        resetDisabled={!chosen}
        resetLabel="Reset display"
        resetTestId="display-reset"
        width={272}
        id={popoverId}
        testId="display-options-popover"
        className="meta-topbar__display"
      />
    </>
  );
}

/**
 * Who the console acts as, and the host's sign-out. The chip names the
 * caller; its popover says how the console knows them and, when the host
 * configures one, signs out: a POST form or a link to the host's page.
 */
function IdentityChip({ identity }: { identity: TopbarIdentity }): React.JSX.Element {
  const [open, setOpen] = React.useState(false);
  const triggerRef = React.useRef<HTMLButtonElement | null>(null);
  const popoverId = `${React.useId()}-identity`;
  const signOut = identity.signOut;
  return (
    <div className="meta-topbar__identity" data-testid="identity-chip" data-kind={identity.kind}>
      <button
        ref={triggerRef}
        type="button"
        className="meta-topbar__identity-trigger"
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? popoverId : undefined}
        aria-label={`${identity.label}, ${identity.detail}`}
        title={`${identity.label} (${identity.detail})`}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="meta-topbar__identity-mark" aria-hidden="true" />
        <span className="meta-topbar__identity-label">{identity.label}</span>
      </button>
      <Popover
        open={open}
        anchorRef={triggerRef}
        onClose={() => setOpen(false)}
        ariaLabel="Who this console acts as"
        id={popoverId}
        testId="identity-popover"
        className="meta-topbar__identity-popover"
        width={260}
      >
        <p className="meta-topbar__identity-name">{identity.label}</p>
        <p className="meta-topbar__identity-detail">{identity.detail}</p>
        {signOut?.method === "post" ? (
          <form
            method="post"
            action={signOut.href}
            className="meta-topbar__identity-sign-out"
            onSubmit={(event) => signOutThen(event, identity.onSignOut, () => postSignOut(signOut.href))}
          >
            <Button type="submit" size="sm">Sign out</Button>
          </form>
        ) : signOut ? (
          <a
            className="cc-btn cc-btn--sm meta-topbar__identity-sign-out"
            href={signOut.href}
            onClick={(event) => signOutThen(event, identity.onSignOut, () => window.location.assign(signOut.href))}
          >
            Sign out
          </a>
        ) : null}
      </Popover>
    </div>
  );
}

export function Topbar({
  mobName,
  connectionStatus,
  identity,
  brandLabel = "Meta",
  brandLogoUrl,
  brandLogoAlt,
  mobStatus = "idle",
  environment = "dev",
  theme,
  onToggleTheme,
  display,
  displayChosen = false,
  onChooseDisplay,
  onResetDisplay,
  sidebarCollapsed,
  railCollapsed,
  railVisible = true,
  onToggleSidebar,
  onToggleRail,
}: TopbarProps): React.JSX.Element {
  const stateWords = teamStateWords(mobStatus);
  return (
    <div className="meta-topbar" data-testid="meta-topbar">
      <button
        type="button"
        className="meta-topbar__toggle meta-topbar__toggle--left"
        onClick={onToggleSidebar}
        aria-pressed={!sidebarCollapsed}
        aria-label={sidebarCollapsed ? "Expand sidebar" : "Collapse sidebar"}
        title={sidebarCollapsed ? "Expand sidebar" : "Collapse sidebar"}
        data-testid="sidebar-collapse-toggle"
      >
        <PanelGlyph side="left" open={!sidebarCollapsed} />
      </button>
      <div className="meta-topbar__brand">
        {brandLogoUrl
          ? <img className="meta-topbar__brand-logo" src={brandLogoUrl} alt={brandLogoAlt || brandLabel} />
          : <span className="meta-topbar__brand-mark" />}
        <span>{brandLabel}</span>
      </div>
      {/* The team and its state: the mark's shape and colour, and the words. */}
      <div className="meta-topbar__mob meta-topbar__team" data-testid="topbar-team">
        <span className="meta-topbar__mob-status" data-status={mobStatus} title={stateWords} aria-hidden="true" />
        <span className="meta-topbar__team-name">{mobName}</span>
        <span className="meta-topbar__team-state">{stateWords}</span>
      </div>
      {environment ? (
        <div className="meta-topbar__env" data-testid="topbar-environment">
          <Chip tone="neutral" title={`Environment: ${environment}`}>{environment}</Chip>
        </div>
      ) : null}
      <div className="meta-topbar__spacer" />
      {connectionStatus}
      {identity ? <IdentityChip identity={identity} /> : null}
      <div className="meta-topbar__util">
        {display && onChooseDisplay && onResetDisplay ? (
          <DisplayOptions display={display} chosen={displayChosen} onChoose={onChooseDisplay} onReset={onResetDisplay} />
        ) : null}
        <IconButton
          label={`Switch to ${theme === "dark" ? "light" : "dark"} mode`}
          icon={<Icon name={theme === "dark" ? "i-sun" : "i-moon"} />}
          className="meta-topbar__icon"
          data-testid="theme-toggle"
          onClick={onToggleTheme}
        />
      </div>
      {railVisible ? (
        <button
          type="button"
          className="meta-topbar__toggle meta-topbar__toggle--right"
          onClick={onToggleRail}
          aria-pressed={!railCollapsed}
          aria-label={railCollapsed ? "Expand signals rail" : "Collapse signals rail"}
          title={railCollapsed ? "Expand signals rail" : "Collapse signals rail"}
          data-testid="signals-rail-collapse-toggle"
        >
          <PanelGlyph side="right" open={!railCollapsed} />
        </button>
      ) : null}
    </div>
  );
}
