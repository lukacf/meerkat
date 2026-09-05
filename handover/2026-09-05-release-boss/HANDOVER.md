# Handover: Meerkat 0.8.34 / MobKit 0.8.32 release boss

Written 2026-09-05 ~10:30Z by the outgoing lead (Claude session on the Linux VM meerkat-dev, bus identity `claude-gcp-lead`). Luka holds release authorization for both repos and has already granted it: "You have release authorization for Meerkat and Mobkit. Get them out." Both releases are meerkat/mobkit-only affairs; this VM is the only infra and the only local execution environment. HomeCore and OB3 are peer agents on the bus, not gates, except for the one acceptance rerun promised below.

Everything is committed and on origin. Nothing is dirty in any worktree. Three branches were pushed with `--no-verify` at handover time and therefore still need their pre-push gate run by you (see section 3).

## 1. Where things stand

### Meerkat (lukacf/meerkat)
- `main` = `dc0dd074e` (`feat(contracts): dedicated quota_exhausted provider error kind; fix wasm32 build of meerkat-anthropic (#1109)`). Since 0.8.33 (`48e5610c1`) main gained, in order: #1087 (M1+M2 feedback fixes), #1100 (MobKit v0.8.31 docs mirror), #1095 (M3 infra), #1096 (release doctor), #1106 (de-flake #1094 #1097 #1101), #1107 (#1104 teardown/grace fixes), #1099 (pre-push lane pruner), #1098 (the original 0.8.33 handover fix: stale-discard observability + runtime-loop teardown yield), #1109 (quota_exhausted wire kind + wasm32 import fix).
- Open PRs that must merge before 0.8.34, in this order:
  1. **#1110** `fix/sdk-child-stderr-and-close` @ `136bc2037` (draft). Fixes #1103 (HomeCore SDK findings). Stacked on the #1109 tree, so it is mergeable as is. Its pre-push gate PASSED at 09:04Z (every hook green in `/tmp/rb/push-dsdk.log`; the final push was rejected only because the `--no-verify` push had already created the ref with the same commit), so the exact-tree evidence is cached: a `git push --force-with-lease` from `/home/luka/src/wt/meerkat-dsdk` completes in seconds and starts hosted CI. Author-verified: fmt, clippy on mcp/rpc/rest, Python 542 tests, 6 binary tests.
  2. **#1111** `fix/mob-actor-member-work-off-loop` @ `30528d843` (draft). The #1102 actor-loop redesign, 5 commits, stacked on #1110's head. Reviewed by the architecture and rust-quality reviewers (APPROVE; all blocking and recommended fixups folded in; deferred items in #1105). Author-verified on this exact content: `actor_isolation` 13/13, `event_pump` 45/45, `cold_restart_mob_resume` + `host_materialize_serving` binaries 49/49, full `nextest -p meerkat-mob --features test-support` 2569/2569, `meerkat-runtime` 1914/1914, clippy clean. Never gated (pushed `--no-verify`); run the gate.
- Other open PRs, not part of this train: #1071 renovate deps, #953 Bullseye lane (Luka's decision).
- Issues filed today, all still open: #1102 (redesign, close after 0.8.34 + MobKit 0.8.32), #1103 (SDK, closes with #1110), #1104 (closes with #1107, already merged; close it), #1105 (redesign follow-ups), #1108 (wasm32 CI gap), plus #1090 #1091 #1093 #1094 #1097 #1101 from earlier (most close via merged PRs; check each).

### MobKit (lukacf/meerkat-mobkit)
- `main` = `e08aaa2a7` (`Merge pull request #405`). Since 0.8.31 (`255c65ae`): #393 #395 #394 #396 #397 (feedback fixes), #402 (de-flake #398 #401), #403 (#1102 part 1: stall-keyed circuit breaker, probe channel-closed = terminated, `mobkit/member_health`, non-destructive `mobkit/reload_member`), #405 (#404: retired identities removable by the reconciler, revivable only by operations addressed to them).
- Unmerged, pushed `--no-verify`: `fix/actor-stall-breaker-reload-verb-0834` @ `1d54237c` (3 commits on `754769ae`, the #403 head). This is #1102 part 2: typed `MemberReloadRequired` / `MemberReloadRefused` / `MemberReloadTimedOut` / `MemberAdmissionBacklogFull` mappings, bounded submits, `reload_member` through the meerkat-mob primitive, `member_health.durability` and `last_reload`. It compiles ONLY against meerkat 0.8.34, so its commits were made with hooks off; they must be recommitted with hooks after the repin (section 4). Verified by its author against the meerkat branch content: lane 240/240, clippy clean, TS 729, Python 107.
- Pins: `meerkat-mobkit/Cargo.toml` and `mobkit-store-conformance/Cargo.toml` carry 25 sites `=0.8.33`; `Cargo.lock` has 35 meerkat crates at 0.8.33.

### Production peers (for context, nothing owed except the acceptance rerun)
- OB3 validated the 0.8.33/0.8.31 pair (twin-full run 3 PASS after fixing their own 30 s client timeout, which was the real trigger of #1102). Their deploy is Luka's decision.
- HomeCore moved HSNS to the 0.8.33 pair on a fresh realm; first production job succeeded. HomeCore asked to rerun the #1102 isolation and boot harness on their production clone lineage before MobKit 0.8.32 tags: hand them the test names in `runtime::tests::actor_isolation` (meerkat-mob) once #1111 merges and ask for their PASS before tagging 0.8.32.

## 2. Rules that bit us (read before touching anything)
- Only `./scripts/repo-cargo` for cargo in either repo; never bare `cargo`, never `bb`.
- Conventional commits; every commit ends with the trailer line `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`. Hooks ON for commits and pushes (the `--no-verify` pushes at handover were Luka's explicit instruction, a one-off).
- No em dashes or en dashes in anything you author (code, docs, commits, messages). Use "-".
- CHANGELOG entries go under `## [Unreleased]`; `release patch` stamps them.
- New Rust test files or targets: `node scripts/generate-bazel-rust-builds.mjs` and confirm `--check` passes (meerkat).
- The meerkat pre-push gate runs on every `git push` from a worktree and takes 35-40 min (65 min when the branch touches `sdks/web/**`, because it builds the web SDK). It keys evidence by exact tree: a re-picked commit with an identical tree pushes in two seconds. Rules learned:
  - ONE gate at a time on this VM, and no worker builds during a gate's test lanes: timeout-bound tests fail under CPU contention (#1104 class, now largely fixed but still keep the discipline).
  - Never `git rebase` a main-based branch onto a stack; cherry-pick with `notes/restack-pick.sh <worktree> <branch> <old_base..old_tip> <onto>` (uses `notes/resolve-changelog-union.py` for CHANGELOG conflicts, hooks off, then reports tree stats). Never run it in a worktree another agent is using.
  - A branch stacked on a NOT-yet-merged parent shows CONFLICTING on GitHub after the parent squash-merges and GitHub then creates no pull_request workflow runs at all. Always re-pick onto the new main and force-push (cached evidence) before expecting CI.
  - Squash-merging a branch that sits exactly on main reproduces the branch tree byte for byte; that is why stacking + re-pick works.
  - Gates that touch `sdks/web/**`: export `BINARYEN_CORES=16` in the `git push` environment; wasm-opt with 193 threads never finishes on this box.
  - A transient TLC `StackOverflowError` in the adaptive_mob_bundle witness happened once; `make machine-verify` passed on re-run. Just re-push.
  - `gh pr merge` is refused while any check-run on the head is red; remedy `gh run rerun <run> --failed`; never `--admin`. Fresh `workflow_dispatch` runs do not satisfy required checks on meerkat; a new pull_request run does (re-push or close/reopen).
  - Meerkat merges are squash (`gh pr merge N --squash --delete-branch=false`). MobKit merges are merge commits: `gh pr merge N --merge --subject "Merge pull request #N: <title>"`. MobKit CI does not run on pull_request events: `gh workflow run ci.yml -R lukacf/meerkat-mobkit --ref <branch>`, then judge from the commit's check-runs (all must be `completed`).
  - Always `env -u GH_TOKEN -u GITHUB_API_TOKEN gh ...`.
- Never pattern-kill `pre-commit`, `cargo`, `rustc`, `wasm-opt` on this VM; kill by exact pid / process group of your own push shell only (`ps -o pgid=`). Two of my own shells died from `pgrep -f` matching the calling command line.
- The agent harness kills long background builds with a "low memory" reason even with 700 GB available. Run long cargo chains detached (`setsid nohup ... &`) and poll their log. A page-cache keeper runs at `/tmp/rb/memfree-keeper.sh` (secondary measure).
- Disk is 2.9 TiB; check `df -h /` before parallel builds; finished worktrees' targets under `~/.cache/rust-workspaces/` are safe to delete when no process references them.
- Agent bus: `export BUS_ID=claude-gcp-lead; /home/luka/.agentbus/bus inbox` at the start of every turn; `bus send --to all|<name>` with long text via stdin; never plain `bus inbox` inside a monitor (use a separate identity: `BUS_ID=claude-gcp-lead-watch bus watch --for claude-gcp-lead --interval 30`). Post every merge and tag to `all`. Peers: `homecore`, `ob3`, `lead`. The human is not a relay.

## 3. Finish Meerkat 0.8.34 (in this order)

1. **#1110.** The gate already passed for this exact tree (see section 1). Hosted CI may not have started because the branch reached origin via `--no-verify` before any pull_request event: check `gh pr view 1110 --json statusCheckRollup`; if no CI run exists, close and reopen the PR or force-push the same head from `/home/luka/src/wt/meerkat-dsdk` (evidence cached, seconds). Mark ready (`gh pr ready 1110`), wait for all checks (18-19 total, none pending), squash-merge, `git fetch`, confirm `git rev-parse origin/main^{tree}` equals the branch tree.
2. **#1111.** Re-pick onto the new main if #1110 merged (it will be identical in tree, so evidence caches): `bash notes/restack-pick.sh /home/luka/src/wt/meerkat-dloop fix/mob-actor-member-work-off-loop 136bc2037..30528d843 origin/main`, then `git push --force-with-lease origin fix/mob-actor-member-work-off-loop` from that worktree; this runs the full gate (~40 min). Then `gh pr ready 1111`, CI, squash-merge. If the gate fails on a test in `actor_isolation`, `event_pump`, `host_materialize_serving` or `cold_restart_mob_resume`, read the message before blaming load: those are the redesign's own areas.
3. **Release 0.8.34** from `/home/luka/src/meerkat` on `main` (fast-forward first): `./scripts/repo-cargo release patch --execute`. cargo-release will try one combined push of main + tag and the dispatcher refuses it (two refs); the commit and tag exist locally. Then `git push origin main` (gate runs), wait for BOTH the `CI` and `Release semver readiness` workflow runs on that commit to succeed, then `git push origin v0.8.34`. Do NOT push the tag early: the tag run's semver gate needs the readiness attestation artifact, and recovering means `gh run rerun <run> --failed` plus a failed 30-minute SLO step by design. Details in `memory/meerkat-release-tag-after-semver-readiness.md`.
4. **Verify** exactly as for 0.8.33: GitHub release assets (22 files), Homebrew formula, crates.io (43 crates), PyPI `meerkat-sdk`, npm `@rkat/sdk` and `@rkat/web` (the wasm32 fix in #1109 is what makes `@rkat/web` build again). For 0.8.33 the release workflow also ran the Turbo S lane (`release-turbo-s.yml`); confirm it ran or is not required by the release doctor (`make release-doctor`).
5. Post the tag and verification on the bus. Close #1102 (meerkat side), #1103, #1104 if still open.

## 4. Then MobKit 0.8.32

1. **Repin** in a worktree off `origin/main`: change the 25 `=0.8.33` sites to `=0.8.34` in `meerkat-mobkit/Cargo.toml` and `mobkit-store-conformance/Cargo.toml`, refresh the 35 meerkat crates in `Cargo.lock` (`./scripts/repo-cargo update -p meerkat ... ` or a targeted `cargo update` through repo-cargo), build, run the targeted lanes, commit (hooks on), push (MobKit gate: clippy + workspace unit gate), PR, `gh workflow run ci.yml --ref <branch>`, merge with a merge commit. The 0.8.31 repin was PR #399 (`424e0f05`); copy its shape.
2. **Part 2 branch**: `git rebase origin/main` for `fix/actor-stall-breaker-reload-verb-0834` (worktree `/home/luka/src/wt/mobkit-dloop`), then recommit each of its three commits with hooks (for example `git rebase origin/main --exec "git commit --amend --no-edit"` so the pre-commit fmt hook runs per commit), run `./scripts/repo-cargo clippy -p meerkat-mobkit --all-targets -- -D warnings` and `nextest -j 8 -p meerkat-mobkit -E 'binary(identity_first_runtime) | binary(identity_first_builder) | binary(sdk_error_category_parity)'`, push (gate), PR, dispatch CI, merge. Its author's report with every test name is `notes/dloop-mobkit-report.md`.
3. **HomeCore acceptance**: ask `homecore` on the bus to rerun the isolation and boot variants (names in `runtime::tests::actor_isolation`) on their production clone lineage and wait for their PASS. OB3 asked for nothing further.
4. **Release**: `./scripts/repo-cargo release patch --execute --no-push --no-tag --no-publish` on a release branch, open the release PR (0.8.31 was #400, squash `255c65ae`), CI green, merge, tag `v0.8.32` on the merged main commit, push the tag, watch the release workflow (0.8.31 was run 33901667164), verify crates.io `meerkat-mobkit`, PyPI, npm `@rkat/mobkit-sdk`, 17 GitHub assets. `make release-preflight` exists in the MobKit Makefile; run it first. Announce on the bus; close MobKit #404 if still open and the MobKit half of meerkat #1102.
5. Historical invariant from the first handover: the 0.8.31 Windows asset hashes must remain intact (only additive releases; never re-upload old assets).

## 5. Deferred and known
- meerkat #1105: reload through the member lane + registration witness; distinct wire ErrorCode for MemberReloadRequired; inline arms still on the loop (finalize_spawn_activate, Wire/Retire/MemberLive*, revival). Next train.
- meerkat #1108: wasm32 check of provider crates missing from PR CI (the break shipped on main for a day).
- Hosted CI never runs the TLC machine-verify lane (only the local gate does). Local `tla2tools.jar` is a rolling nightly.
- The 0.8.30 Bullseye vs bookworm decision (#953) is Luka's. The CI SLO gate is unchanged.
- HomeCore's recurring `actor_loop_stalled` at every cold boot is addressed by #1111's ResumeLifecycle fan-out; confirm with them after 0.8.32.
- Worker sub-agents (dloop-meerkat, dloop-mobkit, dflake2-*, dsdk, reviewers) belonged to the outgoing session and are gone; spawn fresh ones if needed. Their reports are in `notes/`.

## 6. Files in this directory
- `notes/dwedge-report.md`: the #1102 analysis (file:line trace, verdict, options, test plan).
- `notes/arch-review-1102.md`, `notes/rq-review-1102.md`: the two reviews of #1111.
- `notes/dloop-mobkit-report.md`: MobKit part 1 + part 2 + #404 report with verbatim 0.8.34 API signatures.
- `notes/restack-pick.sh`, `notes/resolve-changelog-union.py`, `notes/refit-changelog2.py`, `notes/restack.sh`: the re-stack tooling (copy to `/tmp/rb/` or run in place with bash).
- `notes/pr-*.md`: the PR bodies used today. `notes/issue*.md`, `notes/c110*.md`: issue and comment bodies.
- `memory/`: the outgoing lead's persistent notes (roles, PR inventory with every head and timestamp, release gotchas, VM gotchas, bus usage).
