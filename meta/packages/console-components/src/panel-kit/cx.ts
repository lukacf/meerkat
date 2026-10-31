/** Join class names, skipping empty parts. The kit carries no class-name
 * dependency, so any bundle that can resolve React can render it. */
export function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}
