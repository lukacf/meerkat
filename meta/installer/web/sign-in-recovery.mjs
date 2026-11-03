export function isSignInRequired(failure) {
  return failure?.status === 401 ||
    failure?.status === 403 && failure?.code === "sign_in_required";
}

export function hostSignInPath(document = globalThis.document) {
  const value = document?.querySelector('meta[name="forge-sign-in"]')?.getAttribute("content");
  if (typeof value !== "string" || !value.startsWith("/") || value.startsWith("//") ||
      /[\\\u0000-\u0020\u007f]/.test(value)) return null;
  try {
    const current = new URL(document.location.href);
    const target = new URL(value, current);
    return ["http:", "https:"].includes(current.protocol) && target.origin === current.origin &&
      !target.username && !target.password && !target.hash ? target.pathname + target.search : null;
  } catch { return null; }
}
