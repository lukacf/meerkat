# Lane D1 Checklist

- [x] `D1-01` `DOGMA-15` Add `connection_ref: ConnectionRef` to `ValidatedBinding`. Done when provider validation and registry resolution preserve canonical binding identity directly.
- [x] `D1-02` `DOGMA-15` Rekey token-store/auth-machine/runtime auth plumbing by binding identity. Done when persisted/runtime auth identity is keyed by binding, not `profile_id`.
- [x] `D1-03` `DOGMA-15` Normalize REST and RPC auth handlers to `ConnectionRef`. Done when auth CRUD/login/status/logout use binding identity as the storage and lookup key.
- [x] `D1-04` `DOGMA-15` Keep profile metadata informational only. Done when public auth responses may expose profile metadata but do not use it as the canonical runtime or persistence identity.
- [x] `D1-05` `DOGMA-17` Fix WASM external-auth binding key to `<realm_id>:<binding_id>`. Done when the host callback receives the canonical binding identity.
- [x] `D1-06` `DOGMA-15` Add or extend a defensive scan banning `profile_id` as runtime/persistence auth identity. Done when the scan is checked in and wired for lane/final gating.
- [x] `D1-07` Self static review. Done when no handler or provider binding path reconstructs identity from auth/backend profile ids.
- [x] `D1-08` Handoff note. Done when the `D2` checkpoint contract, handoff commit, and risks are recorded in [D1-plan.md](/Users/luka/.codex/worktrees/ec4d/meerkat/.rct/round5/agents/D1-plan.md).
- [x] `D1-09` Generated-output request. Done when any schema/regeneration need is recorded explicitly, or marked `none`.
