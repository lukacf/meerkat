export function formatCount(value: number): string {
  return new Intl.NumberFormat("en-US").format(Number(value) || 0);
}

export function formatRelativeTime(value?: string): string {
  if (!value) {
    return "";
  }
  const deltaMs = Date.now() - new Date(value).getTime();
  const minutes = Math.max(1, Math.round(deltaMs / 60_000));
  if (minutes < 60) {
    return `${minutes}m`;
  }
  const hours = Math.round(minutes / 60);
  if (hours < 24) {
    return `${hours}h`;
  }
  const days = Math.round(hours / 24);
  if (days < 7) {
    return `${days}d`;
  }
  const weeks = Math.round(days / 7);
  return `${weeks}w`;
}
