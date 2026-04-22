# Lane B Checklist

- [x] `B-01` `DOGMA-11` Rename RPC session-control catalog entries and handlers to the fixed public nouns. Done when the public RPC surface uses only `status`, `submit`, `retire`, `reset`, `submission`, and `submissions`.
- [x] `B-02` `DOGMA-11` Rename REST session-control routes and handlers to the fixed public nouns. Done when the REST route table and route tests use the renamed paths only.
- [x] `B-03` `DOGMA-27` Add `connection_ref` to REST, RPC, MCP-facing create-session helpers, and SDK create-session options. Done when all listed surfaces accept and forward `connection_ref`.
- [x] `B-04` `DOGMA-21` Remove raw runtime incarnation handles from public mob MCP results. Done when public spawn/member-send results expose only `member_ref`.
- [x] `B-05` `DOGMA-10` Finish late-cancel parity on public surfaces. Done when the request-executor result is the only late-cancel decision source.
- [x] `B-06` `DOGMA-11` `DOGMA-27` Update SDK public wrappers and parity tests to the renamed session-control surface and `connection_ref`. Done when TS, Python, and Web wrappers align with the renamed public surface.
- [x] `B-07` `DOGMA-26` Unify realtime status semantics after `EG1a`. Done when RPC and websocket projection expose the same `attempt_count` semantics.
- [x] `B-08` `DOGMA-11` Add or extend a whole-tree scan banning the old session-control names. Done when the scan is checked in and wired for lane/final gating.
- [x] `B-09` Self static review. Done when no compatibility shim remains in any reviewable commit.
- [x] `B-10` Handoff note. Done when milestone boundaries, handoff commit, and risks are recorded in [B-plan.md](/Users/luka/.codex/worktrees/ec4d/meerkat/.rct/round5/agents/B-plan.md).
- [x] `B-11` Generated-output request. Done when schema/catalog/SDK regen needs are recorded explicitly, or marked `none`.
