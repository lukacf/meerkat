/** Gutter navigation; live activity comes from the owning host. */
export function JumpToLatest({ onClick, working = false }: { onClick: () => void; working?: boolean }) {
  return <div className="cc-conversation-jump-anchor">
    <button type="button" className="cc-conversation-jump-latest" data-working={working}
      aria-label="Jump to latest" title={working ? "Jump to latest - agent is working" : "Jump to latest"}
      onClick={onClick}>
      <svg className="cc-conversation-jump-latest__activity" viewBox="0 0 36 36" aria-hidden="true"><circle cx="18" cy="18" r="16.5" /></svg>
      <svg viewBox="0 0 20 20" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M10 4v12m-5-5 5 5 5-5" /></svg>
    </button>
  </div>;
}
