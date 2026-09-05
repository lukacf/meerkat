---
name: never-pkill-pre-commit-on-release-vm
description: "Killing `pre-commit run` processes by pattern on the release VM aborts any running pre-push gate mid-hook; kill by worktree-scoped pid instead"
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 5df1b3d1-6021-4c86-b59d-623dd339f75b
  modified: 2026-09-04T17:32:30.137Z
---

On 2026-09-04 I ran `pgrep -f 'pre-commit run' | xargs kill` (and a `pkill -f` with a similar pattern) to clear two MobKit commit hooks that had deadlocked on pre-commit's shared install lock. Every Meerkat pre-push gate also runs `pre-commit run --config .pre-commit-config.yaml --hook-stage pre-push`, so the kills aborted three running gates in a row ("No hook reported Failed, so the failure is in the gate harness"), costing three 40-minute re-pushes. Two of the kill commands also matched my own shell (exit 144).

**Why:** the pre-push dispatcher and ordinary commit hooks share the same `pre-commit` binary and argv shape; a pattern kill cannot tell them apart.

**How to apply:** never pattern-kill `pre-commit`, `cargo`, or `rustc` on this VM. Find the offending pid via its cwd (`ls -l /proc/<pid>/cwd`) or its parent chain and kill that pid only. Use `[p]attern` bracket tricks so pgrep never matches the calling shell. Avoid running commit hooks concurrently in two worktrees of the same repo (pre-commit's install lock serialises them and can hang); use `-c core.hooksPath=/dev/null` for mechanical changelog-only rewrites. See [[release-train-pr-inventory]].


Related (2026-09-05): never run /tmp/rb/restack-pick.sh (or any `reset --hard` / `checkout -B`) in a worktree a worker agent is still using. It reset the dflake2-meerkat worktree 55 s after the worker created a new branch there and silently switched its working tree back to the de-flake branch; the worker's later uncommitted edits landed on the wrong branch (repaired with `checkout -B` on the same HEAD). Give each branch its own worktree, or ask the worker to stop before re-stacking.


Also (2026-09-05): `pgrep -f '[p]attern'` still matches the CALLING shell when the same command line contains the literal path elsewhere (e.g. a sed or nohup argument naming the script); that killed my own shell again (exit 144). Kill helper daemons via `pgrep -x -f '<exact full command line>'` or a pidfile, never by substring.
