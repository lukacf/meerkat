# Source attribution

The macOS policy lowering in `src/seatbelt.rs` adapts selected hardening patterns
from OpenAI Codex revision `d6c3b448a41311ece3255c52ec3dbfd9ff36f154`:

- `codex-rs/sandboxing/src/seatbelt.rs`: explicit filesystem roots, mutable
  symlink rejection, protected ancestor unlink restrictions and mutating fcntls.
- `codex-rs/sandboxing/src/seatbelt_base_policy.sbpl`: deny-default process,
  same-sandbox signalling and explicit system-query vocabulary.
- `codex-rs/utils/pty/src/posix_child.rs`: reference for strict macOS descriptor
  inheritance through `POSIX_SPAWN_CLOEXEC_DEFAULT`.

OpenAI Codex is Apache-2.0. Its complete license and notice are retained under
`codex/`. Derived source is identified in its header. Meerkat uses a smaller
explicit baseline and a separate exec-only descriptor seal; it does not import
the Codex application, protocol, permission store, network proxy or supervisor.
The retained upstream notice mentions Ratatui; no Ratatui source is included.

These files record provenance, not proof that the adapted sandbox works.
Actual OS tests and each execution adapter's integration remain required.
