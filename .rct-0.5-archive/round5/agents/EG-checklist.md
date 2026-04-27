# Lane EG Checklist

- [x] `EG-01` `DOGMA-20` Rebuild or replace the live `product_session` on mob-member websocket rotation. Done when post-rotation traffic uses the new bridge session rather than the retired one.
- [x] `EG-02` `DOGMA-20` Reset projection and attachment behavior coherently during rotation. Done when websocket status, input, and commit paths operate against the rotated session after the event.
- [x] `EG-03` `DOGMA-14` Remove silent `StandaloneEphemeral` fallback when runtime-backed authority exists. Done when runtime-backed surfaces recover canonical bindings or fail explicitly instead of degrading silently.
- [x] `EG-04` `DOGMA-14` Wire runtime-backed recovery through adjacent surfaces, not just the standard helper path. Done when nearby runtime adapters feed canonical bindings into recovery.
- [x] `EG-05` `DOGMA-23` Introduce a typed schedule-domain runtime delivery outcome model. Done when runtime admission and completion meaning is carried into schedule state without surface-local reinterpretation.
- [x] `EG-06` `DOGMA-23` Relegate legacy schedule failure classes to derived/read-model output only. Done when canonical schedule persistence stores typed runtime meaning rather than rewritten classes.
- [x] `EG-07` `DOGMA-18` Move default `project` and `user` skill source identities into canonical registry resolution. Done when default sources are not created from helper-local UUID fabrication.
- [x] `EG-08` `DOGMA-18` Update REST/RPC session skill canonicalizers to use the same registry path. Done when default-source skill refs resolve through one canonical registry path.
- [x] `EG-09` `DOGMA-18` Add or extend a defensive scan banning helper-local UUID fabrication outside the canonical registry path. Done when the scan is checked in and wired for lane/final gating.
- [x] `EG-10` Self static review. Done when `DOGMA-23` is clearly a full typed-outcome fix rather than a relabeled classification helper.
- [x] `EG-11` Handoff note. Done when handoff commit, changed files, and risks are recorded in [EG-plan.md](/Users/luka/.codex/worktrees/ec4d/meerkat/.rct/round5/agents/EG-plan.md).
- [x] `EG-12` Generated-output request. Done when any schema/regeneration need is recorded explicitly, or marked `none`.
