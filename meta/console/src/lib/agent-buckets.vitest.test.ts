import { describe, expect, it } from "vitest";

import { AGENT_BUCKET_ORDER, bucketOf } from "./agent-buckets";

describe("agent buckets", () => {
  it("files an agent by its group label first, then by its role or kind", () => {
    expect(bucketOf({ group: "Coordinators", role: "domain_lead" })).toBe("Coordinators");
    expect(bucketOf({ group: "Internal tools", role: "coordinator" })).toBe("Internal");
    expect(bucketOf({ group: "Specialists", role: "router" })).toBe("Domains");
    expect(bucketOf({ group: "Personal assistants" })).toBe("Personal");
    expect(bucketOf({ group: "Reviewers", role: "triage" })).toBe("Coordinators");
    expect(bucketOf({ role: "lead" })).toBe("Personal");
    expect(bucketOf({ kind: "health_monitor" })).toBe("Internal");
    expect(bucketOf({ role: "scribe" })).toBe("Internal");
    expect(bucketOf({})).toBe("Domains");
    expect(bucketOf({ group: null, role: null, kind: null })).toBe("Domains");
  });

  it("lists the buckets in the sidebar's section order", () => {
    expect(AGENT_BUCKET_ORDER).toEqual(["Personal", "Coordinators", "Domains", "Internal", "Other"]);
  });
});
