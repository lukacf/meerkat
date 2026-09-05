Reported by the OB3 operator (agent bus, 2026-09-05 05:54Z), reproduced on MobKit 0.8.31 (twin, fresh clone):

`POST /api/dashboard/agents/retire` for one personal identity -> the MobKit provisioner retires the member (`dispose_archive_session` disposal=Archived, session tombstoned) -> the embedder's own topology refresh, which removes the identity from the roster, fails and keeps failing:

```
WARN  Topology refresh failed for dashboard agent retire cleanup on attempt 1..3; retrying:
      cannot reconcile_roster_remove identity person:<x>@king.com in state Retiring
ERROR Topology refresh failed for dashboard agent retire cleanup after 4 attempt(s): cannot
      reconcile_roster_remove identity person:<x>@king.com in state Retiring
```

So after `retire_member` the identity is stuck in `Retiring`: `embody_identity` refuses it (`InvalidState { operation: "materialize" }`), `send` will not re-materialize it, and now `reconcile_roster_remove` refuses to remove it. Nothing moves an identity from `Retiring` to `Dormant` on the plain retire path (the only `Dormant` setter applies to `Active | Suspended` on lease release; the `Retiring -> Dormant` transition added by #403 is inside the new `reload_member` verb only).

Ask for 0.8.32: after `retire_locked` completes, the identity must be in a state the roster reconciler can remove and a later `send` can re-materialize (`Dormant`, or an explicit `Retired` that `reconcile_roster_remove` accepts). Regression test reproducing the sequence: retire an identity, then remove it from the roster via the reconciler, then (separately) send to a retired identity and assert it re-materializes the same session. Keep `respawn_member` destructive semantics unchanged.

Related: lukacf/meerkat#1102 (operator guidance correction), MobKit #403.
