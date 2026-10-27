export interface ConsoleReadOnlyOverrideInput {
  search?: string;
  hostOverride?: unknown;
}

const READ_ONLY_QUERY_KEYS = [
  "console_read_only",
  "meta_console_read_only",
  "view_only",
];

function parseBooleanFlag(value: unknown): boolean | null {
  if (typeof value === "boolean") return value;
  if (typeof value !== "string") return null;
  switch (value.trim().toLowerCase()) {
    case "1":
    case "true":
    case "yes":
    case "on":
      return true;
    case "0":
    case "false":
    case "no":
    case "off":
      return false;
    default:
      return null;
  }
}

function browserSearch(): string {
  if (typeof window === "undefined") return "";
  return window.location.search;
}

function browserHostOverride(): unknown {
  if (typeof window === "undefined") return undefined;
  return (window as Window & { __META_CONSOLE_READ_ONLY__?: unknown })
    .__META_CONSOLE_READ_ONLY__;
}

export function resolveConsoleReadOnlyOverride(
  input: ConsoleReadOnlyOverrideInput = {},
): boolean {
  const search = input.search ?? browserSearch();
  const params = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
  const hostOverride = parseBooleanFlag(input.hostOverride ?? browserHostOverride());
  if (hostOverride === true) return true;
  for (const key of READ_ONLY_QUERY_KEYS) {
    const parsed = parseBooleanFlag(params.get(key));
    if (parsed === true) return true;
  }
  return false;
}

export const __readOnlyOverrideTest = {
  parseBooleanFlag,
};
