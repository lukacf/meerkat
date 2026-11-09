// The real SDK invokes this helper after decoding. Provider warnings are data,
// not permission to write arbitrary text to a shared host console.
export const logWarnings = <T>(value: T): T => value;
