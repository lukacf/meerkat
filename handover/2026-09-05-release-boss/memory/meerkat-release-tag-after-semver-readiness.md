---
name: meerkat-release-tag-after-semver-readiness
description: "Meerkat release procedure gotcha - push the release commit to main, wait for the \"Release semver readiness\" run to succeed, then push the tag; cargo-release's combined push is refused by the pre-push gate"
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 5df1b3d1-6021-4c86-b59d-623dd339f75b
  modified: 2026-09-04T13:51:42.300Z
---

When releasing Meerkat with `./scripts/repo-cargo release patch --execute`:

1. cargo-release pushes `main` and the tag in ONE `git push` with two refs. The pre-push dispatcher refuses multi-ref pushes ("requires exactly one ref update; received 2"), so the local commit and tag exist but nothing is pushed. Push `main` first, then the tag, as two separate pushes (the tag push reuses the exact-tree gate evidence and is fast).
2. Do NOT push the tag right after main. The tag-triggered release workflow's `release_semver_gate` step "Verify exact-tree pre-tag semver evidence" requires an unexpired `meerkat-semver-attestation-main-<tree>` artifact produced by the separate "Release semver readiness" workflow run on that main commit. If that run has not finished, the gate fails, `publish_registries` is skipped, and you must wait for the run to finish and `gh run rerun <release-run> --failed`.

3. If you do end up in the recovery path, expect the tag run's rerun to end with `publish_registries: failure` on step "Verify all Rust crates are public within 30 minutes" even though every crate was skipped as already published: the SLO is measured from the tag time. Nothing downstream depends on that job (assets + Homebrew still publish). Verify registries directly and move on.

**Why:** on 2026-09-04 (v0.8.33) I hit both: the combined push was refused, and the tag pushed minutes after main raced the semver-readiness run (release run 33878977142 failed the gate, needing a rerun).

**How to apply:** sequence = release execute (local) -> `git push origin main` -> wait for both `CI` and `Release semver readiness` runs on that commit to be green -> `git push origin vX.Y.Z`. Related: [[release-train-0833-0831-roles]].
