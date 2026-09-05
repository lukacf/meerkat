---
name: six-feedback-prs-handover
description: "Sept 2026 handover of six external-feedback PRs (meerkat M1-M3, MobKit K1-K3) - goal all green and rebased on latest main, never merge (Luka reviews)"
metadata: 
  node_type: memory
  type: project
  originSessionId: 5df1b3d1-6021-4c86-b59d-623dd339f75b
  modified: 2026-09-04T11:14:22.358Z
---

On 2026-09-04 Luka also handed me the six "feedback PRs" fixing ~30 problems reported by two external candidates (Joris: Python SDK path; Romain: Rust library host path).

Original goal: all six implemented, adversarially reviewed, CI green, rebased onto latest main; do NOT merge. SUPERSEDED 2026-09-04 ~16:15 CEST by Luka: after Meerkat 0.8.33 is public, MERGE all six when green, then release Meerkat 0.8.34 and the paired MobKit Luka then confirmed (16:30 CEST) the 0.8.33 pairing stays: MobKit 0.8.31 = frozen b96552f3 + repin to 0.8.33 (no K content), released first; K1-K3 merge after that; then Meerkat 0.8.34 and MobKit 0.8.32 = K1-K3 + repin to 0.8.34. Meerkat convention: squash merges titled 'type(scope): subject (#N)'. MobKit convention: merge commits 'Merge pull request #N: title'.

Branches / PRs:
- meerkat M1 fix/provider-request-compat -> PR #1092
- meerkat M2 fix/mob-author-experience -> PR #1087 (excluded from the 0.8.33 emergency train)
- meerkat M3 fix/build-publish-infra -> no PR yet; one "handover snapshot" commit to be split into 4 units (docs.rs build.rs fallback; deploying guide; MobKit docs mirror pipeline; rkat-mcp stderr tracing)
- MobKit K1 feat/gateway-config-surface -> PR #393
- MobKit K2 fix/console-runtime-correctness -> PR #395
- MobKit K3 docs/feedback-hygiene -> PR #394

Conventions: hooks ON for pushes (the --no-verify pushes were one-time); conventional commits with trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`; no em/en dashes; CHANGELOG under [Unreleased]; meerkat semver gate needs public-API breaks under "### Breaking"; new Rust test files/bins need `node scripts/generate-bazel-rust-builds.mjs`.

CI facts: MobKit pull_request events produce no runs since 2026-09-03 22:24Z (cause unknown, reported to Luka); push to feat/** runs CI; for fix/ and docs/ branches dispatch `gh workflow run ci.yml --repo lukacf/meerkat-mobkit --ref <branch>` and judge from commit check-runs, not `gh pr checks`.

Work layout on the VM: worktrees under /home/luka/src/wt/ (meerkat-m1, meerkat-m2, meerkat-m3) so the release checkout at /home/luka/src/meerkat stays clean.

M3 Unit 1 ruling (2026-09-04): the handover snapshot 0efb0f1b6 actually carried the REJECTED design (meerkat-core publishing bridge suffixes as Cargo `links` metadata, DEP_MEERKAT_CORE_*, with two assertions of the security canary `authority_build_scripts_do_not_leak_factory_seal_metadata` removed). Luka's ruling: that shape is forbidden; keep a DOCS_RS fallback (warn + fixed `docsrs_unlinked` suffix only when DOCS_RS is set and no core checkout is visible; everything else fails closed). I implemented the fallback, restored the canary verbatim, and rewrote the four canary tests. If someone reopens the metadata approach, that is a security-canary change needing Luka.

Rebasing feedback branches over a release stamp: the branch changelog entries sit under the old [Unreleased]; resolve by rebuilding [Unreleased] from the branch's entries minus the bullets main stamped into the release (scripts in /tmp/rb on the VM were ad hoc).

**Why:** these PRs are Luka's to merge; my job is to make them reviewable and green, and to keep them off the emergency release train.

**How to apply:** finish M3 split first; rebase all six onto latest main after the 0.8.33/0.8.31 release commits land (their CHANGELOG entries will conflict with the release stamp); force-push with lease through hooks; report six URLs and SHAs. See [[release-train-0833-0831-roles]].
