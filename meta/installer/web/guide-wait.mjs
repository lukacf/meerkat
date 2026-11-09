export function guideWaitBinding(state) {
  const operation = state?.operation;
  if (typeof state?.id !== "string" || !state.id || operation?.status !== "running" ||
      typeof operation.request_id !== "string" || !operation.request_id) return null;
  const startedAt = typeof operation.started_at === "string" ? operation.started_at : null;
  return {
    guideId: state.id,
    requestId: operation.request_id,
    startedAt,
    key: JSON.stringify([state.id, operation.request_id, startedAt]),
  };
}

export function guideWaitElapsed(startedAt, now) {
  const started = typeof startedAt === "string" && startedAt ? Date.parse(startedAt) : NaN;
  if (!Number.isFinite(started) || !Number.isFinite(now) || started > now) return null;
  const seconds = Math.floor((now - started) / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}
