/** Display a wire instant on the same local clock as the conversation. */
export function formatWorkGraphTimestamp(
  value: string | null | undefined,
  options: { date?: boolean; seconds?: boolean } = {},
): string {
  if (!value) return "";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  const pad = (part: number) => String(part).padStart(2, "0");
  const day = options.date
    ? `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} `
    : "";
  const clock = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  return `${day}${clock}${options.seconds ? `:${pad(date.getSeconds())}` : ""}`;
}
