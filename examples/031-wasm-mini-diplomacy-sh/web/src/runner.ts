/** Owns all turn scheduling. Pause/replace take effect at a completed-turn boundary. */
export class CampaignRunner<T> {
  private current: T | null = null;
  private active: Promise<void> | null = null;
  private starting: Promise<void> | null = null;
  private continuous = false;

  constructor(
    private turn: (session: T) => Promise<void>,
    private canRun: (session: T) => boolean,
    private dispose: (session: T) => Promise<void>,
  ) {}

  get paused(): boolean { return !this.continuous; }

  replace(create: () => Promise<T | null>): Promise<void> {
    if (this.starting) return this.starting;
    this.continuous = false;
    this.starting = (async () => {
      await this.active;
      if (this.current) await this.dispose(this.current);
      this.current = null;
      this.current = await create();
    })().finally(() => { this.starting = null; });
    return this.starting;
  }

  pause(): void { this.continuous = false; }

  resume(): Promise<void> {
    if (this.starting) return this.starting;
    this.continuous = true;
    return this.run();
  }

  step(): Promise<void> {
    if (this.starting) return this.starting;
    this.continuous = false;
    return this.run();
  }

  private run(): Promise<void> {
    if (this.active) return this.active;
    const session = this.current;
    if (!session || !this.canRun(session)) return Promise.resolve();
    this.active = (async () => {
      do {
        await this.turn(session);
      } while (this.continuous && this.current === session && this.canRun(session));
    })().finally(() => { this.active = null; });
    return this.active;
  }
}

/** Poll newly queued extraction work for its own full, bounded observation window. */
export async function waitForOrders(
  poll: () => boolean,
  active: () => boolean,
  sleep: (ms: number) => Promise<void>,
  now: () => number = Date.now,
): Promise<void> {
  const deadline = now() + 20_000;
  while (active() && now() < deadline) {
    await sleep(300);
    if (poll()) return;
  }
}

export async function pollNarrative(
  status: () => Promise<string>,
  sleep: (ms: number) => Promise<void>,
): Promise<string | null> {
  for (let i = 0; i < 40; i++) {
    await sleep(500);
    const { run } = JSON.parse(await status()) ?? {};
    if (!run) continue;
    if (run.status === "completed") {
      const step = run.step_ledger?.find((s: { step_id: string; status: string }) => s.step_id === "summarize" && s.status === "completed");
      return typeof step?.output?.narrative === "string" ? step.output.narrative
        : typeof step?.output === "string" ? step.output : null;
    }
    if (run.status === "failed" || run.status === "canceled") return null;
  }
  return null;
}
