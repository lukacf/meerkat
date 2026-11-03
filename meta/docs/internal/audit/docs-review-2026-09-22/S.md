# S: MobKit-specific guidance in the upstream architecture skill

[Audit index](README.md) | [Coverage](coverage.md)

Original documentation and initial evidence line ranges refer to baseline `af82b6b3ab34faed9bf3e962d148d55f10dcd1dc`, unless an external dependency or historical revision is explicitly identified. Final-review citations refer to the corrected files in this change. Source excerpts may be de-indented or omit intervening lines; cited ranges identify the complete context. Quoted defects are preserved as evidence, not current usage guidance.

## SKILL-001: External architecture gotcha incorrectly requires active restore to rotate its lease and run customization on every reconcile

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`.claude/skills/meerkat-architecture/references/gotchas.md:48`**

```text
`retire`/`respawn`/`reset` (and `restore_flow` on every reconcile) depend on the bump, and tests enforce it — `identity_first_runtime_retire_returns_advanced_fencing_token` asserts the returned token advances *and* that old-token writes are then rejected; `restore_flow_releases_active_lease_on_customizer_failure` asserts `restore_flow` refreshes (advances) the active lease before customizer work.
```

Agents following this loaded skill can incorrectly treat healthy active-lease reuse and skipped rebuild customization as defects, reintroducing unnecessary fencing rotation/rebuild work during reconcile and contradicting the current live-authority contract.

**`meerkat-mobkit/src/identity_first/orchestrator.rs:343-361`**

```text
.embody_identity(
```

Current restore_flow delegates eager restore to the shared IdentityRuntime embodiment path, supplying the spec and optional customizer.

**`meerkat-mobkit/src/identity_first/runtime.rs:5260-5270`**

```text
if state == IdentityLifecycleState::Active {
            // Converged eager restore still validates the time-sensitive
            // external lease. This is part of the shared embodiment door, not
            // a second restore implementation: healthy authority is reused,
            // due authority is renewed, and lost authority parks this member.
            let record = self.reuse_active_restore_state(&spec).await?;
            self.clear_materialization_backoff(identity).await;
            return Ok(EmbodimentOutcome {
```

An already Active identity returns through reuse before fresh acquisition or build customization. The quoted every-reconcile refencing claim is directly contradicted by the executable branch.

**`meerkat-mobkit/src/identity_first/runtime.rs:7183-7196`**

```text
Some(lease) if lease.is_healthy() => return Ok(lease.fencing_token),
```

ensure_active_lease returns the exact healthy fence; it does not call acquire_leases for healthy active authority.

**`meerkat-mobkit/src/identity_first/runtime.rs:7363-7372`**

```text
self.ensure_active_lease(&spec.identity).await?;
```

reuse_active_restore_state validates the existing authority. Due leases enter renewal and use its returned grant; healthy leases remain unchanged.

**`meerkat-mobkit/tests/identity_first_runtime.rs:9119-9187`**

```text
async fn identity_first_runtime_restore_flow_reuses_exact_active_lease_without_customizing()
```

The current regression test supplies a failing customizer yet expects successful active reuse, asserts the preserved fencing token equals the initial one, and asserts send uses that unchanged token. These are inspected test assertions, not a claim of an executed test run.

**`meerkat-mobkit/tests/identity_first_runtime.rs:9168-9181`**

```text
.expect("an already-active identity must bypass rebuild customization");
```

The exact current test contract is the opposite of invoking customization after every active restore refence.

**`meerkat-mobkit/tests/identity_first_runtime.rs:9351-9407`**

```text
async fn identity_first_runtime_restore_flow_renews_expired_active_lease_before_reuse()
```

The separate due-lease test uses a rotating external renewal provider. The correction must preserve that renewal can advance a fence, rather than incorrectly promising that every active restore always retains the same token.

**`meerkat-mobkit/src and meerkat-mobkit/tests:Exact-name search across Rust files`**

```text
restore_flow_releases_active_lease_on_customizer_failure
```

A read-only exact-text scan returned zero matches for the old test name cited by the skill. The current test name and assertions are shown above.

### Independent adjudication

Independently traced restore_flow into embody_identity_locked, then the Active early return into reuse_active_restore_state and ensure_active_lease. The executable branch precedes both acquire_leases and customize_build, so the every-reconcile claim is false, not merely an obsolete test name. The actual healthy-reuse test installs a customizer that would fail and asserts successful restore with unchanged fencing and send tokens. The due-lease test deliberately rotates on renewal and requires the new token, so an unconditional 'active restores never rotate' replacement would also be false. The correction must distinguish healthy reuse from provider renewal and explicit lifecycle reacquisition. Source/tests were read from the actual MobKit checkout at af82b6b3ab34faed9bf3e962d148d55f10dcd1dc and verified unchanged against that baseline. These are inspected source/test assertions, not executed Rust test results. The old upstream audit pin and this Meerkat baseline have identical gotchas.md bytes, confirming the fix is not duplicated there.

**`lukacf/meerkat-mobkit@af82b6b3ab34faed9bf3e962d148d55f10dcd1dc:meerkat-mobkit/src/identity_first/orchestrator.rs:343-380`**

```text
spec: Some(&spec),
                                    customizer,
```

restore_flow passes both values into the shared embodiment path. Its error arm parks a failed member as Broken rather than failing unrelated members.

**`lukacf/meerkat-mobkit@af82b6b3ab34faed9bf3e962d148d55f10dcd1dc:meerkat-mobkit/src/identity_first/runtime.rs:5260-5282`**

```text
let record = self.reuse_active_restore_state(&spec).await?;
            self.clear_materialization_backoff(identity).await;
            return Ok(EmbodimentOutcome {
```

Already-Active embodiments return before the fresh acquisition and customization path.

**`lukacf/meerkat-mobkit@af82b6b3ab34faed9bf3e962d148d55f10dcd1dc:meerkat-mobkit/src/identity_first/runtime.rs:7183-7263`**

```text
Some(lease) if lease.is_healthy() => return Ok(lease.fencing_token),
```

Healthy authority is returned unchanged. Non-healthy authority calls renew_leases, publishes the returned grant, and reports LeaseLost on a Lost/missing response. No acquire_leases call occurs here.

**`lukacf/meerkat-mobkit@af82b6b3ab34faed9bf3e962d148d55f10dcd1dc:meerkat-mobkit/tests/identity_first_runtime.rs:9119-9203`**

```text
preserved_lease.fencing_token, initial_grant.fencing_token,
```

A deliberately failing customizer is bypassed; restore succeeds, the exact lease stays held, and subsequent send uses the initial token.

**`lukacf/meerkat-mobkit@af82b6b3ab34faed9bf3e962d148d55f10dcd1dc:meerkat-mobkit/tests/identity_first_runtime.rs:9351-9407`**

```text
renewed.fencing_token > initial_grant.fencing_token,
```

ControlledLeaseProvider is configured with RenewRotatedToken. The test requires one renewal and subsequent send using the renewed grant, not retention of the expired fence.

**`lukacf/meerkat-mobkit@af82b6b3ab34faed9bf3e962d148d55f10dcd1dc:meerkat-mobkit/tests/identity_first_runtime.rs:9410-9463`**

```text
assert_eq!(status.state, IdentityLifecycleState::Broken);
    assert!(status.lease.is_none());
```

Lost active authority yields a per-member Broken result and preserves the previous spec; 'fails closed' is not a claim that the fleet-level restore call must fail.

**`lukacf/meerkat-mobkit@af82b6b3ab34faed9bf3e962d148d55f10dcd1dc:meerkat-mobkit/src/identity_first/runtime.rs:5333-5341,5402-5404`**

```text
let customize = customizer.customize_build(&build_context, &spec, &mut draft);
```

The fresh-materialization branch acquires its grant at 5337-5341, before invoking arbitrary customizer code at 5404; this ordering remains documented.

**`lukacf/meerkat-mobkit@af82b6b3ab34faed9bf3e962d148d55f10dcd1dc:meerkat-mobkit/src/identity_first/local_lease.rs:83-108,139-146`**

```text
let token = FencingToken::new(state.next_token);
            state.next_token += 1;
```

Explicit same-holder acquisition still issues a fresh token. Local renewal instead returns the existing matching token; neither behavior should be conflated with unconditional restore reacquisition.

**`lukacf/meerkat-mobkit@af82b6b3ab34faed9bf3e962d148d55f10dcd1dc:meerkat-mobkit/tests/identity_first_runtime.rs:3799-3835`**

```text
token > old_grant.fencing_token,
```

The retained retire regression explicitly requires an advanced fence and rejects snapshot writes under the old grant. Runtime lifecycle reacquisition sites at 9065, 9639, 9848, and 10072 are unchanged.

**`lukacf/meerkat-mobkit@af82b6b3ab34faed9bf3e962d148d55f10dcd1dc:meerkat-mobkit/src/unified_runtime/builder.rs:1229-1242`**

```text
Arc::new(LocalLeaseProvider::with_floor(high_water)),
```

The builder reads the persisted high-water from LocalContinuityStore::open_with_fencing_floor and seeds the provider; local_lease.rs:63-66 initializes next_token above that floor. Preserve the existing historical restart-floor caveat without changing runtime code.

**Required correction:** Edit only upstream gotchas item 37. Name LocalLeaseProvider for the explicit-acquire rule; retain retire/respawn/reset refencing and its current retire test. Replace every-reconcile rotation/customization claims and the removed test with the healthy Active reuse, due-provider-renewal, and lost-authority distinction plus the two current restore regression names. State that fresh materialization still acquires before customization. Remove the continuity-token-churn aside, retain the same-holder non-idempotence warning for explicit reacquisition, scope the existing lifecycle orphan-on-retry caveat to the bundled single-process provider, and preserve the existing restart-floor guidance.

### Changes and final verification

**Changed:** `.claude/skills/meerkat-architecture/references/gotchas.md`.

Corrected only item 37 (one physical line) in the isolated upstream Meerkat worktree. Explicit LocalLeaseProvider acquisition still refences, including lifecycle reacquisition. Already-Active restore instead keeps a healthy exact grant without build customization; due grants renew through the provider and may rotate, and lost authority fails closed. Fresh materialization acquires before customization. Replaced the removed test with the two current restore regression names, removed the every-reconcile churn claim, and scoped the existing lifecycle orphan caveat to the bundled provider. The historical restart-floor paragraph tail is byte-for-byte unchanged.

**Validation:** PASS: make SHELL=/bin/bash agent-gate AGENT_GATE_ARGS=--working-tree; git diff --check; read-only Python assertions prove only line 48 changed in exactly one tracked file, stale assertions are absent, all three cited tests exist, five MobKit source/test files exactly match af82b6b3ab34faed9bf3e962d148d55f10dcd1dc, audit-S preserves the original discovery object verbatim, and scope-S JSON parses. Fresh read-only review: PASS with no regressions (review-S.json). The parent additionally reports executing ./scripts/repo-cargo test -p meerkat-mobkit --test identity_first_runtime identity_first_runtime_restore_flow_ --locked: all 28 tests PASSED, with linker compact-unwind size warnings only. No duplicate Rust run was performed in this upstream worktree.

**Final review: pass.** The correction accurately distinguishes healthy Active restore reuse from due-lease renewal, lost authority, and explicit lifecycle reacquisition. Independent inspection of executable source confirms that the Active branch returns before acquisition and customization; healthy authority preserves its exact fence, due authority invokes provider renewal, and lost authority becomes a member-local Broken outcome before publishing the requested spec. The inspected regression tests assert these distinctions, including bypassing a deliberately failing customizer. Explicit LocalLeaseProvider acquisitions still advance fencing, lifecycle operations still reacquire, and fresh materialization still acquires before customization. Persisted restart-floor guidance remains valid and its historical paragraph tail is byte-for-byte preserved. Exactly item 37 at line 48 changed; the obsolete claims and nonexistent test citation are removed, and all three current test citations exist. No significant issues found in the reviewed changes.

**`(operator-retained path)/.claude/skills/meerkat-architecture/references/gotchas.md:48`**

```text
For an already `Active` identity, `restore_flow` instead reuses the exact healthy live lease and skips build customization; due authority is renewed through the provider and may return a newer fence, while lost authority fails closed.
```

The replacement states the implementation's conditional behavior rather than promising either unconditional rotation or unconditional token preservation.

**`(operator-retained path)/meerkat-mobkit/src/identity_first/runtime.rs:5260-5282`**

```text
let record = self.reuse_active_restore_state(&spec).await?;
            self.clear_materialization_backoff(identity).await;
            return Ok(EmbodimentOutcome {
```

External MobKit implementation, verified byte-identical to af82b6b3ab34faed9bf3e962d148d55f10dcd1dc: the Active branch returns before fresh acquisition at 5337-5341 and customize_build at 5404.

**`(operator-retained path)/meerkat-mobkit/src/identity_first/runtime.rs:7183-7263`**

```text
Some(lease) if lease.is_healthy() => return Ok(lease.fencing_token),
```

Healthy authority returns the existing token directly. The remaining branch calls renew_leases and publishes its returned grant; Lost or missing renewal results call mark_lease_lost and return LeaseLost. The inspected health predicate at 957-960 uses remaining TTL, so 'due' is not restricted to already-expired grants.

**`(operator-retained path)/meerkat-mobkit/src/identity_first/orchestrator.rs:343-380`**

```text
let failure = runtime
                                    .park_embodiment_failure(&identity, &error)
                                    .await;
```

restore_flow delegates to embody_identity with the requested spec and customizer. Its error arm produces RestoreOutcome::Broken for that member instead of propagating the member failure as a fleet-level error.

**`(operator-retained path)/meerkat-mobkit/tests/identity_first_runtime.rs:9119-9203`**

```text
preserved_lease.fencing_token, initial_grant.fencing_token,
```

Inspected, not executed: identity_first_runtime_restore_flow_reuses_exact_active_lease_without_customizing supplies a customizer whose body returns BuildFailed, requires successful Resumed restore, asserts exact fence equality, asserts subsequent send uses that same fence, and checks another holder cannot acquire the preserved lease.

**`(operator-retained path)/meerkat-mobkit/tests/identity_first_runtime.rs:9351-9407`**

```text
renewed.fencing_token > initial_grant.fencing_token,
```

Inspected, not executed: the cited renewal test selects RenewRotatedToken, asserts one renewal, and requires subsequent send to use the renewed fence. The provider's actual implementation at 650-655 allocates and installs the newer token, supporting 'may return a newer fence' rather than mandatory rotation for all providers.

**`(operator-retained path)/meerkat-mobkit/tests/identity_first_runtime.rs:9410-9463`**

```text
assert_eq!(status.state, IdentityLifecycleState::Broken);
    assert!(status.lease.is_none());
```

Inspected, not executed: lost renewal must produce a member-local Broken outcome while the fleet pass succeeds, remove the lease, and retain the old spec label. Actual mark_lease_lost source likewise sets Broken and clears the lease; reuse_active_restore_state validates authority before updating entry.spec.

**`(operator-retained path)/meerkat-mobkit/src/identity_first/local_lease.rs:83-146`**

```text
let token = FencingToken::new(state.next_token);
            state.next_token += 1;
```

The acquisition body rejects a different holder but reaches this allocation for the same holder, preserving explicit reacquisition fencing. In contrast, local renewal returns the existing matching record token. The correction correctly scopes the acquisition rule to LocalLeaseProvider.

**`(operator-retained path)/meerkat-mobkit/src/identity_first/runtime.rs:7266-7281`**

```text
entry.state = state;
        entry.lease = None;
```

The retained lifecycle caveat describes actual state clearing. Inspected retire, respawn, live-respawn rebind, and reset bodies at 9045-9067, 9633-9641, 9843-9850, and 10066-10074 call this lifecycle transition before explicit acquire_leases. The wording remains limited to the bundled single-process provider.

**`(operator-retained path)/meerkat-mobkit/tests/identity_first_runtime.rs:3799-3835`**

```text
token > old_grant.fencing_token,
```

Inspected, not executed: the retained retire test requires a higher returned token and calls assert_old_token_snapshot_write_rejected. That helper's actual body at 2023-2046 attempts a snapshot write under the old token and requires ContinuityStoreError::StaleFencingToken.

**`(operator-retained path)/meerkat-mobkit/src/unified_runtime/builder.rs:1229-1242`**

```text
Arc::new(LocalLeaseProvider::with_floor(high_water)),
```

The builder obtains the persisted high-water through open_with_fencing_floor and supplies it to LocalLeaseProvider. The inspected provider constructor initializes next_token with floor.saturating_add(1), supporting the retained distinction between restart-floor recovery and same-process reacquisition.

## Upstream ownership and publication

This finding belongs to `lukacf/meerkat`, not the MobKit-owned symlink. Its correction was made in a separate upstream worktree; the shared Meerkat checkout and the personal alias targets were not edited.

```json
{
  "checks": [
    "Pinned MobKit HEAD and exact source/test byte comparisons: passed. No MobKit file or shared Meerkat checkout was edited.",
    "Exact paragraph-scope, cited-symbol, stale-reference, historical-tail, and discovery-object preservation assertions: passed.",
    "make SHELL=/bin/bash agent-gate AGENT_GATE_ARGS=--working-tree: exit 0. Rust lane doctor: 24 pass, one expected dirty-worktree warning, zero failures. docs-check: 116 public pages and six Python test suites passed (one existing skipped test). Cargo changed-path gate: no Rust build-relevant changes.",
    "git diff --check: passed.",
    "Fresh independent read-only code-review agent: SKILL-001 PASS, no regressions, exact one-paragraph scope verified against actual MobKit implementation/test bodies. Full returned review persisted in review-S.json.",
    "Parent-executed downstream proof (reported by session c5d99d5b-1af8-42ef-8569-503cf51d138a): ./scripts/repo-cargo test -p meerkat-mobkit --test identity_first_runtime identity_first_runtime_restore_flow_ --locked PASSED all 28 tests, including healthy Active reuse without customization, renewal, lost authority, and fresh materialization. Only linker compact-unwind size warnings.",
    "Normal git commit succeeded as 4b97c131d8c8213f7b53c0d5d3753da70016f797 with the required co-author trailer. Pre-commit hooks correctly skipped unrelated Rust/Bazel/dogma paths. Worktree clean afterward.",
    "Verified the existing gh keyring account lukacf has push permission for lukacf/meerkat without exposing credential values or changing global auth. Push used per-command env -u GH_TOKEN -u GITHUB_TOKEN git -c credential.helper= -c 'credential.helper=!gh auth git-credential' push -u origin HEAD, with normal hooks enabled.",
    "Normal pre-push hooks passed dogma mirror verification, secret detection, whitespace/EOF/YAML/TOML/conflict/size checks, CI nextest archive contracts, cargo fmt, and changed-crate Clippy. machine-codegen-verify then FAILED (exit 2): TLC rejected 18446744073709551615 in meerkat_machine (ci.cfg); make machine-verify exited 1 and the dispatcher rejected the push. No failed hook was bypassed.",
    "Post-failure authenticated git ls-remote --heads origin refs/heads/mobkit-lease-skill-correction: exit 0, empty result (branch not published). git status --porcelain: empty. Commit still contains exactly one insertion/one deletion in item 37.",
    "Exported upstream-skill-correction.patch directly from git diff HEAD^ HEAD -- .claude/skills/meerkat-architecture/references/gotchas.md. git apply --check --reverse against the clean committed worktree passed without modifying files. SHA-256: 4de1cbf3e10de0f6e2b393f33f3fdd227da14631fdc1ce64927e6170d246b0b7."
  ],
  "caveats": [
    "Initial make agent-gate AGENT_GATE_ARGS=--working-tree returned 0 but emitted a pre-existing /bin/sh parse error from scripts/build-backend-env:73 (Bash process substitution). The complete target was rerun cleanly with SHELL=/bin/bash; no build infrastructure was changed and no gate or hook was bypassed.",
    "Upstream implementer and independent reviewer inspected runtime test bodies, while the parent separately executed the 28-test downstream proof; execution is attributed to that parent rather than this worktree. The only repository edit here is documentation.",
    "The parent MobKit HEAD advanced to e53206a1fad55092b29420f3b647bee6af12079a during independent review. The reviewer independently verified all five principal source/test evidence files remain byte-identical to the requested baseline and the new HEAD.",
    "Upstream publication is blocked by the exact machine-codegen-verify/TLC error recorded above. The correction is durably committed locally and independently reviewed, but is neither pushed nor in a companion PR. The unpushed branch prevents PR creation without bypassing the failed gate. Per the requested scope, no unrelated machine/runtime fix or hook bypass was attempted.",
    "The parent may include the exact exported patch in its MobKit audit ledger for a durable reviewable handoff. Applying or publishing the upstream correction remains separate work; exporting the patch did not modify either repository or bypass the upstream push gate."
  ]
}
```

## Final scope checks

> [
>   "Read audit-brief.md, audit-S.json, adjudication-S.json, and fixes-S.json from the specified session artifact directory. Independently checked their claims against actual documentation, source, and test bodies. JSON parsing and assertions confirmed scope S and the sole item SKILL-001; there were no rejected adjudications.",
>   "Ran read-only git status, rev-parse HEAD, branch --show-current, staged diff, paragraph diff, and diff --stat in the isolated upstream worktree. Confirmed HEAD 02dc6732c87ba2105eb4f84a1f499869965159a7, the requested branch, an empty staged diff, and exactly one modified tracked file.",
>   "Ran read-only Python byte comparisons and diff assertions: only physical line 48 changed; the historical restart-floor tail is byte-identical; obsolete every-reconcile wording, token-churn wording, and the old test citation are absent. Verified the documentation and five principal evidence files use regular paths without symlink components.",
>   "Verified all three cited test definitions exist in the actual external MobKit test file. Read their bodies, the lost-lease regression, the controlled renewal provider, and the stale-write assertion helper. These tests were inspected, not executed.",
>   "Ran pinned git grep for restore_flow_releases_active_lease_on_customizer_failure across MobKit source and tests: exit 1 with no matches.",
>   "Observed external MobKit HEAD advance during review from af82b6b3ab34faed9bf3e962d148d55f10dcd1dc to e53206a1fad55092b29420f3b647bee6af12079a. The initial HEAD-stability assertion therefore failed. Follow-up comparisons passed: orchestrator.rs, runtime.rs, local_lease.rs, builder.rs, and identity_first_runtime.rs are byte-identical to both the requested baseline and the new HEAD.",
>   "Ran git diff --check successfully, including a final repeat after the scope assertions. No Rust builds, runtime tests, or Make gates were executed by this reviewer; no files, git state, commits, or publication state were modified."
> ]
