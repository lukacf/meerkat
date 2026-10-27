import React from "react";

interface TopbarProps {
  mobName: string;
  connectionStatus?: React.ReactNode;
  brandLabel?: string;
  brandLogoUrl?: string;
  brandLogoAlt?: string;
  mobStatus?: string;
  environment?: string;
  theme: "dark" | "light";
  onToggleTheme: () => void;
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

export function Topbar({
  mobName,
  connectionStatus,
  brandLabel = "Meta",
  brandLogoUrl,
  brandLogoAlt,
  mobStatus = "idle",
  environment = "dev",
  theme,
  onToggleTheme,
  sidebarCollapsed,
  railCollapsed,
  railVisible = true,
  onToggleSidebar,
  onToggleRail,
}: TopbarProps): React.JSX.Element {
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
      <div className="meta-topbar__mob">
        <span className="meta-topbar__mob-status" data-status={mobStatus} title={mobStatus} />
        <span>{mobName}</span>
        <span className="dim">· {mobStatus}</span>
      </div>
      <div className="meta-topbar__mob">
        <span>env:</span>
        <span>{environment}</span>
      </div>
      <div className="meta-topbar__spacer" />
      {connectionStatus}
      <div className="meta-topbar__util">
        <button
          type="button"
          onClick={onToggleTheme}
          data-testid="theme-toggle"
          title={`Switch to ${theme === "dark" ? "light" : "dark"} mode`}
        >
          {theme === "dark" ? "☾ dark" : "☀ light"}
        </button>
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
