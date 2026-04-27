# Lane D2 Checklist

- [x] `D2-01` `DOGMA-25` Introduce one effective-capability computation path that honors compile-time registration, config status resolution, and per-build overrides. Done when the skill engine is no longer seeded from raw capability inventory.
- [x] `D2-02` `DOGMA-25` Update skill filtering to use the effective-capability result. Done when disabled builtins, shell, or memory remove corresponding skills from inventory and resolution.
- [x] `D2-03` `DOGMA-16` Change external-authorizer resolution to return typed auth material. Done: shared resolver now materializes the returned `ResolvedAuthEnvelope` into a real lease path instead of dropping it on the floor.
- [x] `D2-04` `DOGMA-16` Materialize a real `DynamicLease` for external-authorizer flows in provider runtimes. Done: placeholder empty leases are removed from owned provider runtimes; external static-header envelopes and in-process cloud authorizers now land in real `DynamicLease`s, and unsupported host-dynamic envelopes fail explicitly.
- [x] `D2-05` `DOGMA-24` Merge and enforce `constraints`, `metadata_defaults`, and binding policy in resolved runtime behavior. Done: metadata defaults are merged centrally, required account/workspace metadata is enforced, non-override workspace/org mismatches fail, and OAuth refresh/login behavior now respects binding constraints.
- [x] `D2-06` `DOGMA-16` `DOGMA-24` Consume `ValidatedBinding.connection_ref` directly in provider runtimes. Done: owned runtimes now key persisted-token resolution through the canonical `connection_ref`.
- [x] `D2-07` Self static review. Done for `D2-1` and `D2-2` scope: no placeholder empty-lease end state remains in owned provider runtimes, and the D1 `connection_ref` contract is consumed directly.
- [x] `D2-08` Handoff note. Done when `D2-1` completion and `D2-2` implementation/probe requests are recorded in [D2-plan.md](/Users/luka/.codex/worktrees/ec4d/meerkat/.rct/round5/agents/D2-plan.md).
- [x] `D2-09` Generated-output request. `none`.
