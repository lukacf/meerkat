// Which agent started which helper. The sidebar's live snapshot drops the
// `spawned_by` label, so the console reads it from the topology query's node
// labels (`meta/topology/query`), when the console has read the topology.
// Pure module; the result is cached per query answer, so passing it on every
// render costs nothing.

const EMPTY: Readonly<Record<string, string>> = Object.freeze({});
const cache = new WeakMap<object, Readonly<Record<string, string>>>();

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

/** helper identity -> the identity that started it, from a topology query answer. */
export function topologyLineage(result: unknown): Readonly<Record<string, string>> {
  if (!result || typeof result !== "object") return EMPTY;
  const hit = cache.get(result);
  if (hit) return hit;
  const nodes = (result as { nodes?: unknown }).nodes;
  const lineage: Record<string, string> = {};
  if (Array.isArray(nodes)) {
    for (const node of nodes) {
      if (!node || typeof node !== "object") continue;
      const record = node as { endpoint?: { identity?: unknown }; identity?: unknown; labels?: Record<string, unknown> };
      const identity = text(record.endpoint?.identity) || text(record.identity);
      const spawnedBy = text(record.labels?.spawned_by);
      if (identity && spawnedBy && spawnedBy !== identity) lineage[identity] = spawnedBy;
    }
  }
  const frozen = Object.keys(lineage).length ? Object.freeze(lineage) : EMPTY;
  cache.set(result, frozen);
  return frozen;
}
