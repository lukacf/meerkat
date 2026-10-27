// SDKs still build their real requests. This projection separates URL queries
// into the existing owned transport contract without accepting duplicate keys.
export function sdkRequest(url: string | URL, method: string | undefined, headers: HeadersInit, body?: unknown) {
  const parsed = new URL(url);
  const queryParams = Object.fromEntries(parsed.searchParams);
  if (parsed.username || parsed.password || parsed.hash || Object.keys(queryParams).length !== [...parsed.searchParams].length || body !== undefined) {
    throw new Error('Unsupported SDK request');
  }
  return { method, url: parsed.origin + parsed.pathname, headers: Object.fromEntries(new Headers(headers)),
    ...(parsed.search ? { queryParams } : {}) };
}
