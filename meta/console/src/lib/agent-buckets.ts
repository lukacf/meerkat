// The console's agent buckets: the sections the sidebar's Category view
// lists agents in, the roster's group filter and the topology's lanes. One
// rule, so every surface files an agent under the same words. Pure module
// (no React), so node test bundles can import it.

/** The fields that file an agent: its group label, else its role or kind. */
export interface AgentBucketFields {
  group?: string | null;
  role?: string | null;
  kind?: string | null;
}

/** The buckets an agent is filed under when no configured group names it. */
export type AgentBucket = "Personal" | "Coordinators" | "Domains" | "Internal" | "Other";

/** The order the buckets are listed in when the configuration names none. */
export const AGENT_BUCKET_ORDER: readonly AgentBucket[] = ["Personal", "Coordinators", "Domains", "Internal", "Other"];

/** An agent's section in the sidebar's Category view and the roster's group
 * filter. An explicit group label decides first; only an agent whose group
 * names no section is placed by its role (or kind). */
export function bucketOf(a: AgentBucketFields): AgentBucket {
  const g = (a.group || "").toLowerCase();
  const p = (a.role || a.kind || "").toLowerCase();
  if (g.includes("coordinator")) return "Coordinators";
  if (g.includes("personal")) return "Personal";
  if (g.includes("internal")) return "Internal";
  if (g.includes("domain") || g.includes("responder") || g.includes("communication") || g.includes("specialist")) return "Domains";
  if (p.includes("coord") || p.includes("triage") || p.includes("router") || p.includes("commander")) return "Coordinators";
  if (p.includes("personal") || p.includes("identity") || p.includes("lead")) return "Personal";
  if (p.includes("gate") || p.includes("monitor") || p.includes("scribe")) return "Internal";
  return "Domains";
}
