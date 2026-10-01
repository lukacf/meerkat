# Native ownership tracer: publication companion

This records the bounded baseline behind the owner composition proposal. It
does not demonstrate governed authorization. The original source inspection
used Meerkat `b4ecf8a10beff0f769991ed238ca10de1eaa4851`; implementation must refresh
source coordinates and test the actual candidate.

## Existing owners to extend

| Boundary | Existing owner and integration obligation |
| --- | --- |
| Ingress and dedup | Native input admission owns InputId, exact dedup and durable acceptance. The immutable requester and authority association must survive payload retirement. |
| Work selection | Runtime queue and run owners select and merge contributors. A last-speaker field cannot replace their combined restrictions. |
| Model release | Each physical attempt, retry, fallback and compaction disclosure needs its own exact owner entry and witness acknowledgment before release. |
| Tool release | Native dispatch owns the concrete tool operation. Final admission must follow fallible preparation and queue waits. |
| Helpers and comms | Existing helper and peer owners retain causal identity. Routing identity alone is not a delegated requester mandate. |
| Projection and recipients | Stream taps, history reads, console delivery and later replay are separate protected releases with current audience checks. |
| Recovery | Existing durable input, operation and outcome owners retain uncertainty. An interrupted ticket or empty lookup does not prove non-entry. |

These are integration boundaries, not new parallel registries or wrapper-owned
lifecycles. The full ADR still requires every reachable protected operation to
participate or refuse explicitly.

## Executed causal baseline

On 2026-10-01, the diagnostic tracer ran against published Meerkat 0.8.49 and
MobKit `e2795b5dbbabe224e2e912256d7b99e480b5c9c9`. Its observer passed 23 Python
controls and seven Rust tests. The native trace passed in 6.398 seconds.

The trace deliberately terminated the gateway after durable input admission,
recovered the exact input without resending it, observed actual helper
delegation, resumed the parent and observed console delivery. The helper's
history came from the native session-service owner while that process lived.
The recovered process-local ticket remained Unknown.

This establishes causal baseline behavior only. It does not prove authenticated
requester propagation, delegated permission, helper history durability across
restart, model disclosure authority, audience authorization or any denied-sink
non-entry property. Duplicate console projections are not distinct native
operations or authorization evidence.

The accepted export allowlist contains 37 diagnostic artifacts and has SHA-256
`4e5376d8124f71c78083268b7ab02a97a70a080b293989970e297f42a9323dcb`.
Toolkit's bounded execution review has SHA-256
`070c985597500d9dbb23ea28381b7d5e633ce295eb59d15c59a3a4a525d847ba`,
with export-selection addendum
`80b42b6242d750f801cd5d23d82867c6363665ff78a4ee529eb99055e3d23695`.
That reviewer checked retained hashes, lengths and containment; it did not
independently rerun the gateway. Raw state, identity configuration and key files
are excluded from the accepted export.

The next acceptance trace must add authenticated ingress, real attenuation,
independently enforcing Elephant reads, exact model release and authorized
reply, then prove absence of denied effects under revocation, restart, missing
evidence and uncertain outcomes. The baseline does not close those cases.
