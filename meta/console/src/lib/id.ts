type CryptoLike = {
  randomUUID?: () => string;
  getRandomValues?: <T extends ArrayBufferView>(array: T) => T;
};

function randomUuidFromValues(cryptoSource: CryptoLike): string | null {
  if (typeof cryptoSource.getRandomValues !== "function") {
    return null;
  }
  try {
    const bytes = cryptoSource.getRandomValues(new Uint8Array(16));
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0"));
    return [
      hex.slice(0, 4).join(""),
      hex.slice(4, 6).join(""),
      hex.slice(6, 8).join(""),
      hex.slice(8, 10).join(""),
      hex.slice(10, 16).join(""),
    ].join("-");
  } catch {
    return null;
  }
}

export function createConsoleId(
  prefix = "console",
  cryptoSource: CryptoLike | undefined =
    typeof globalThis.crypto !== "undefined" ? globalThis.crypto : undefined,
): string {
  if (cryptoSource && typeof cryptoSource.randomUUID === "function") {
    try {
      return `${prefix}-${cryptoSource.randomUUID()}`;
    } catch {
      // Fall through to getRandomValues/Math fallback.
    }
  }
  const generated = cryptoSource ? randomUuidFromValues(cryptoSource) : null;
  if (generated) {
    return `${prefix}-${generated}`;
  }
  return `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}
