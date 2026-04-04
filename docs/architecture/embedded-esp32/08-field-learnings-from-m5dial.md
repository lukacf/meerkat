# Field Learnings From Adjacent M5Dial Delivery

This file records external empirical input from a separate ESP32-S3 project delivered autonomously on an M5Dial. It is not a repo-grounded statement about the current Meerkat codebase. Its role is to shape execution strategy, risk handling, and Phase 0 behavior.

## Why this matters

The adjacent project demonstrates that an autonomous agent can close a real ESP32-S3 loop very quickly when it treats the environment as a full pipeline instead of “just firmware”:

- board identification
- browser and service automation
- backend API validation from the host
- network and credential discovery
- vendor-library source verification
- firmware build and flash
- real-world result verification

That delivery reportedly reached a first-flash success with zero debug cycles after the hardware blocker was resolved. The main bottleneck was not firmware complexity but physical-world access, especially USB connectivity.

## Reported takeaways translated for this dossier

### 1. Separate probe stack from production stack

The adjacent project succeeded using a fast vendor-native toolchain. That does not mean the production Meerkat runtime must use the same stack. It does mean Phase 0 should be allowed to use the fastest sacrificial board-native path that closes the external contracts.

Applied in this dossier:

- [02-external-contract-matrix.md](./02-external-contract-matrix.md) `DEC-007`
- [04-implementation-phase-plan.md](./04-implementation-phase-plan.md) Phase 0 operational notes

### 2. Test service APIs from the host before flashing

The adjacent project validated the Home Assistant endpoints from the Mac before firmware ever touched the board. That reduced first-flash uncertainty dramatically.

Applied in this dossier:

- Phase 0 now requires host-side backend validation before the first target flash that depends on those services.
- Phase 0 packets in [05-autonomous-execution-pack.md](./05-autonomous-execution-pack.md) explicitly include host API validation.

### 3. Verify board APIs against actual library source

The adjacent project avoided unnecessary complexity by checking the real library headers and finding built-in touch helpers that were better than the initial hand-rolled plan.

Applied in this dossier:

- Phase 0 and Phase 2 execution heuristics now require checking installed headers or library source for board APIs before designing abstractions around them.

### 4. Use tool chaining aggressively

The reported success depended on composing webcam capture, browser automation, HTTP clients, credential access, flash tools, and network inspection into one pipeline.

Applied in this dossier:

- [05-autonomous-execution-pack.md](./05-autonomous-execution-pack.md) now treats multi-tool chaining as a deliberate execution heuristic, not an accident.

### 5. Build previews while blocked on hardware

The adjacent project used blocked time productively by building an interactive UI preview before the USB issue was resolved.

Applied in this dossier:

- Host-sim and preview work are treated as productive fallback tasks whenever hardware is temporarily unavailable.
- This especially matters for the single-node example UI or operator workflow and for the swarm topology visualizer.

### 6. Escalate physical blockers early

The adjacent project identified that the biggest inefficiency was waiting too quietly on a charge-only cable. This is directly relevant to autonomous ESP work.

Applied in this dossier:

- Phase 0 and Phase 3 now explicitly treat unresolved physical blockers as escalation-worthy instead of allowing busy polling to continue indefinitely.

### 7. Establish OTA early when possible

Once the adjacent project had a Wi-Fi capable build, OTA removed the need for repeated USB interaction.

Applied in this dossier:

- Phase 0 and Phase 3 execution packets now instruct the agent to establish OTA or another low-friction redeploy path as soon as the chosen stack makes that possible.

## Relevance to Example 036 and Example 037

### Example 036

The field report reinforces that a single-node example should be a genuinely useful operational pattern. A persistent event agent with live provider access, restartable state, local tool integration, and a documented redeploy workflow fits that requirement better than a thin direct-provider demo.

### Example 037

The field report also reinforces that real-world value comes from orchestrating the whole system, not from toy firmware. A multi-node triangulation and self-organization pattern is ambitious, but it is user-meaningful if and only if Phase 0 proves that peer messaging and telemetry are actually stable enough and the example produces an operator-facing topology artifact rather than only serial smoke output.

## What this file does not claim

- It does not claim that the current Meerkat embedded stack already supports these behaviors.
- It does not claim that Arduino or PlatformIO is the final Meerkat implementation stack.
- It does not close any `HYPOTHESIS` row in the external contract matrix by itself.

Those closures still require the real probes and artifact bundles defined elsewhere in this dossier.
