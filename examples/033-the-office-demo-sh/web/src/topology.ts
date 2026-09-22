import type { AgentId, RuntimeModule } from "./types";
import { WIRING_PAIRS } from "./agents";

export interface TopologyResult {
  state: "complete" | "partial" | "failed";
  errors: string[];
  blocked: AgentId[];
}

/** Acknowledged edges are a projection, never inferred from a rejected operation. */
export class OfficeTopology {
  readonly blocked = new Set<AgentId>();
  readonly edges = new Map<string, boolean | null>();
  private tail: Promise<unknown> = Promise.resolve();

  constructor(private mod: RuntimeModule, private mobId: string) {
    for (const [a, b] of WIRING_PAIRS) this.edges.set(`${a}|${b}`, true);
  }

  settled(): Promise<unknown> { return this.tail; }

  change(action: "revoke" | "restore", target: AgentId): Promise<TopologyResult> {
    const next = this.tail.then(async () => {
      if (action === "revoke") this.blocked.add(target);
      else this.blocked.delete(target);
      const errors: string[] = [];
      let completed = 0;
      for (const [a, b] of WIRING_PAIRS) {
        const wanted = !this.blocked.has(a) && !this.blocked.has(b);
        const key = `${a}|${b}`;
        if (this.edges.get(key) === wanted) continue;
        try {
          if (wanted) await this.mod.mob_wire(this.mobId, a, b);
          else await this.mod.mob_unwire(this.mobId, a, b);
          this.edges.set(key, wanted);
          completed++;
        } catch (error) {
          this.edges.set(key, null);
          errors.push(`${a} ↔ ${b}: ${String(error)}`);
        }
      }
      return {
        state: errors.length === 0 ? "complete" : completed ? "partial" : "failed",
        errors, blocked: [...this.blocked],
      } satisfies TopologyResult;
    });
    this.tail = next;
    return next;
  }
}
