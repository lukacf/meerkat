---
name: release-train-0833-0831-roles
description: Sept 2026 paired release train - I am Meerkat release boss and MobKit lead on the Linux GCP VM; HomeCore run-9b is out of scope by Luka decision
metadata: 
  node_type: memory
  type: project
  originSessionId: 5df1b3d1-6021-4c86-b59d-623dd339f75b
  modified: 2026-09-04T07:00:04.142Z
---

On 2026-09-04 Luka handed me both roles for the paired release: Meerkat 0.8.33 release boss and MobKit 0.8.31 lead, with explicit release authorization for both.

Environment facts (verified 2026-09-04):
- This session runs on a Linux GCP VM (zsh), repos at /home/luka/src/meerkat and /home/luka/src/meerkat-mobkit.
- No Tailscale, no agentbus (~/.agentbus absent) on this VM. Home server 100.66.144.94 is unreachable from here.
- HomeCore is a separate agent on Luka's Mac (bus id "homecore"); reach it over the agent bus (see agentbus-on-meerkat-dev), not through Luka. It ran run-9b 2/2 PASS on the published 0.8.31 asset on 2026-09-04 and put activation 134 live.
- Owner-authenticated GitHub: `env -u GH_TOKEN -u GITHUB_API_TOKEN gh ...` (account lukacf).
- bb CLI at ~/.local/bin/bb, BUILDBUDDY_API_KEY set in env.

**Why:** the handover notes mix Mac-side paths with VM-side work; knowing which host owns which step avoids wasted attempts.

Luka's decision (2026-09-04 late morning): the HomeCore run-9b validation and HomeCore activation are OUT of scope for this train. Meerkat 0.8.33 and MobKit 0.8.31 are Meerkat/MobKit-only affairs; this VM is the only infra and only local execution environment.

ROLE CONSOLIDATION (2026-09-04 ~20:10Z, from Luka): the previous meerkat-boss (bus identity copilot-meerkat-boss-0831) and mobkit-lead roles are retired; claude-gcp-lead owns both repos, both release lines, and all release holds/clearances. Announced on the bus.

**How to apply:** do all Meerkat/MobKit code, CI, and release work here; post every merge, tag, publication and hold/clear decision on the agent bus. Do not plan around the home server, the bus, or HomeCore.
