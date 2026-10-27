# I: Upstream asks and Meerkat documentation integration

[Audit index](README.md) | [Coverage](coverage.md)

Original documentation and initial evidence line ranges refer to baseline `af82b6b3ab34faed9bf3e962d148d55f10dcd1dc`, unless an external dependency or historical revision is explicitly identified. Final-review citations refer to the corrected files in this change. Source excerpts may be de-indented or omit intervening lines; cited ranges identify the complete context. Quoted defects are preserved as evidence, not current usage guidance.

## I-001: The current follow-up queue still calls the shipped console progress/health UI unimplemented

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/upstream-asks.md:769`**

```text
on the identity-inspect RPC. Console health affordance remains follow-up.
```

The file explicitly doubles as the current work queue, and both its introduction (45-46) and ask-14 disposition tell maintainers that this local adoption work remains. Operators are directed away from an existing progress/health surface and maintainers can unnecessarily schedule duplicate implementation.

**`console/src/lib/adapters.ts:224-228`**

```text
  const health = agent.progress?.health;
  if (!health || health === "healthy") return null;
  const tone: ConsoleSidebarMetaTone = health === "wedged" ? "negative" : health === "degraded" ? "warning" : "muted";
  return { id: "health", label: health, tone };
```

The actual sidebar adapter already exposes non-healthy machine-owned progress as a health chip, with distinct degraded/wedged tones.

**`console/src/panels/RosterPanel.tsx:160-178`**

```text
                    <dt>Run state</dt><dd className="mono">{active.progress.run_state}</dd>
                    <dt>In flight</dt><dd className="mono">{active.progress.in_flight_work}</dd>
```

The roster detail panel renders Health, Run state, In flight and Last progress from active.progress; this is the precise ask-14 projection affordance named as still outstanding, not an unrelated generic health panel.

**`meerkat-mobkit/src/runtime/console_ingress.rs:1109-1110`**

```text
            if let Some(progress) = agent.get("progress").filter(|value| !value.is_null()) {
                row["progress"] = progress.clone();
```

The console ingress projection carries upstream progress into the visible roster rows, so the UI consumer is backed by production data.

**`console/src/lib/adapters.test.ts:113-115`**

```text
  const healthChip = wedgedItem?.meta?.find((chip) => chip.id === "health");
  assert.equal(healthChip?.label, "wedged");
  assert.equal(healthChip?.tone, "negative");
```

An inspected regression explicitly asserts the wedged chip. The same test covers degraded and healthy cases. This audit did not execute the test.

### Independent adjudication

Confirmed narrowly as an outdated ongoing follow-up, not as an inaccurate July 12 historical statement. I independently read the complete 1,869-line ledger, traced the present console data path, and checked the introducing commit. The header is dated 2026-07-12, and git blame attributes the follow-up statements to commits preceding the console implementation. The affordance really was outstanding at that checkpoint; rewriting it as though it already existed in MobKit 0.7.35 would falsify history. However, lines 5-7 explicitly designate this file as the current work queue, and the unqualified 'remains follow-up' is inside ask 14's authoritative status paragraph. MobKit PR #279, commit f3db01834548ede68faf56bfd2d8cba8a8fea3c3, added the exact requested console affordance on 2026-07-13 and is an ancestor of the audited baseline af82b6b3ab34faed9bf3e962d148d55f10dcd1dc. It also updated the embedded console bundle. Present source supplies progress from member_status through the experience/identity-row projections into sidebar chips and roster details. Correct this with an explicitly later follow-up update while preserving the original pending-at-checkpoint fact. Health is best-effort: healthy chips are suppressed, final members are skipped, unavailable snapshots omit progress, and rosters above the configurable cap skip projection. Neither this UI nor its documentation correction removes application watchdog responsibilities.

**`docs/design/upstream-asks.md:3-7`**

```text
## Current status (2026-07-12)

This file is both the historical evidence record and the current upstream work
queue. Historical problem statements remain below, but their status is
authoritative only through the explicit status line under each ask.
```

The dated checkpoint must be preserved, but the document also expressly carries an ongoing queue. This supports a later status annotation rather than modernization of every historical paragraph.

**`docs/design/upstream-asks.md:45-49;763-769`**

```text
Non-ask follow-ups remain: a MobKit console-health affordance from ask 14's
progress data; an optional MobKit gateway projection for M4's already-shipped
terminal-status query; and refresh of Meerkat's downstream compatibility
table after M3. These are local projection/documentation maintenance, not
open upstream runtime asks.

on the identity-inspect RPC. Console health affordance remains follow-up.
```

The same completed local task remains listed in both the queue summary and authoritative ask disposition. Both locations need a consistent later update.

**`https://github.com/lukacf/meerkat-mobkit/commit/f3db01834548ede68faf56bfd2d8cba8a8fea3c3:commit metadata and diff`**

```text
CommitDate: Mon Jul 13 12:07:15 2026 +0200
feat: console health affordance from member progress (ask 14 UI) (#279)
```

Verified with git show, git log -S for the health adapter and roster fields, and git merge-base --is-ancestor (exit 0). The commit modifies the production Rust projection, console source/tests, console/dist, and meerkat-mobkit/console-dist. It follows the dated checkpoint. Its workspace version was 0.7.37, but this adjudication deliberately does not infer the first published release containing the change.

**`meerkat-mobkit/src/http_console.rs:10508-10552`**

```text
const CONSOLE_PROGRESS_MEMBER_CAP: usize = 64;

    if entries.len() > cap {

        if entry.is_final {
            continue;
        }
        let Ok(snapshot) = handle.member_status(&entry.agent_identity).await else {
            continue;
        };
        member.progress = snapshot
            .progress
            .as_ref()
            .and_then(|progress| serde_json::to_value(progress).ok());
```

The real live snapshot attaches upstream machine progress, not an unrelated host-health proxy. The cap, final-member check, and fallible lookup delimit the correction; progress is not promised for every roster row. MOBKIT_CONSOLE_PROGRESS_MEMBER_CAP overrides the default and 0 disables it.

**`meerkat-mobkit/src/runtime/console_ingress.rs:1108-1111`**

```text
            // Machine-owned liveness projection (meerkat 0.7.29+, ask 14).
            if let Some(progress) = agent.get("progress").filter(|value| !value.is_null()) {
                row["progress"] = progress.clone();
            }
```

Identity status rows preserve the machine-owned progress. I additionally inspected console/src/lib/agents.ts:109,149,185 and packages/console-core/src/control-plane.ts:28-39,338-341 to verify normalization and agent-row propagation.

**`console/src/lib/adapters.ts:224-229;304-308`**

```text
export function agentHealthMeta(agent: ConsoleAgent): { id: string; label: string; tone: ConsoleSidebarMetaTone } | null {
  const health = agent.progress?.health;
  if (!health || health === "healthy") return null;
  const tone: ConsoleSidebarMetaTone = health === "wedged" ? "negative" : health === "degraded" ? "warning" : "muted";
  return { id: "health", label: health, tone };
}

            const health = agentHealthMeta(agent);
            return health ? [health] : [];
```

The helper is actually invoked while building sidebar metadata. Only non-healthy supplied values produce a chip; wedged and degraded have distinct tones.

**`console/src/panels/RosterPanel.tsx:160-181`**

```text
                {active.progress && (
                  <>
                    <dt>Health</dt>

                    <dt>Run state</dt><dd className="mono">{active.progress.run_state}</dd>
                    <dt>In flight</dt><dd className="mono">{active.progress.in_flight_work}</dd>
                    <dt>Last progress</dt>
```

Roster details render the exact requested machine health/progress fields conditionally on progress availability.

**`console/src/lib/adapters.test.ts:91-150`**

```text
test("buildSidebarViewState surfaces an unhealthy liveness chip and stays silent when healthy", () => {

  assert.equal(healthChip?.label, "wedged");
  assert.equal(healthChip?.tone, "negative");

  assert.equal(degradedChip?.label, "degraded");
  assert.equal(degradedChip?.tone, "warning");

  assert.equal(healthyItem?.meta?.some((chip) => chip.id === "health"), false);
```

Read-only inspection of the targeted regression corroborates the present rendering contract. Tests were not executed in this documentation adjudication.

**Required correction:** Update only the two current-follow-up formulations, using an explicit later annotation to preserve chronology. At lines 45-49, replace the paragraph with: 'At the 2026-07-12 checkpoint, the non-ask follow-ups were a MobKit console-health affordance from ask 14’s progress data, an optional MobKit gateway projection for M4’s already-shipped terminal-status query, and refresh of Meerkat’s downstream compatibility table after M3. **Follow-up update (2026-09-22):** the console-health affordance subsequently landed in MobKit PR #279 (`f3db01834548ede68faf56bfd2d8cba8a8fea3c3`, 2026-07-13) and is no longer outstanding. The M4 projection and compatibility-table refresh are separate local projection/documentation follow-ups, not open upstream runtime asks.' At line 769, retain 'on the identity-inspect RPC.' and replace only 'Console health affordance remains follow-up.' with: 'The console-health affordance remained a follow-up at that checkpoint; it subsequently landed in MobKit PR #279 (`f3db0183`, 2026-07-13). When progress is available, sidebar chips expose non-healthy machine health and roster details show Health, Run state, In flight, and Last progress.' Preserve the July 12 header, the 0.7.35 adoption chronology, the original problem/interim/watchdog discussion, and ask 14’s shipped upstream disposition. Do not assert the UI existed in 0.7.35, name an unverified first release, promise progress for every member, or reopen an upstream ask.

### Changes and final verification

**Changed:** `docs/design/upstream-asks.md`.

Updated only the introduction's non-ask follow-up paragraph and ask 14's console-health follow-up sentence. The introduction preserves the 2026-07-12 pending checkpoint and adds the explicit 2026-09-22 follow-up annotation citing PR #279, commit f3db01834548ede68faf56bfd2d8cba8a8fea3c3, and its 2026-07-13 landing date. Ask 14 distinguishes that subsequent UI delivery from the original MobKit 0.7.35 adoption and describes non-healthy sidebar chips and roster progress fields only when progress is available. M4 and compatibility-table follow-ups remain separate; upstream ask closure, original problem/interim paragraphs, and watchdog discussion are unchanged.

**Validation:** git show -s verified PR #279's exact commit, date, and title; git merge-base --is-ancestor exited 0. Read console/src/lib/adapters.ts:219-230, console/src/panels/RosterPanel.tsx:155-184, and meerkat-mobkit/src/http_console.rs:10504-10554 to confirm chip suppression, conditional roster fields, and best-effort/capped progress projection. Scoped diff and ledger-preservation assertions passed.

**Final review: pass.** Both stale follow-up formulations are corrected with the adjudicated chronological boundary intact. The introduction explicitly preserves the July 12 checkpoint and labels September 22 as the later annotation; ask 14 separates MobKit 0.7.35's API/SDK adoption from the July 13 console implementation. I independently verified PR #279's merge metadata, its local Git commit and ancestry, its production-source/embedded-bundle changes, and the current producer-to-UI data path. The final wording promises non-healthy chips and the named roster fields only when progress is available, matching healthy-chip suppression and the fallible, capped, non-final-member projection. It does not invent a first published release, remove watchdog responsibilities, reopen the shipped upstream ask, or conflate the independent M4 and compatibility-table follow-ups.

**`docs/design/upstream-asks.md:3-7;45-53`**

```text
At the 2026-07-12 checkpoint, the non-ask follow-ups were a MobKit
console-health affordance from ask 14's progress data, an optional MobKit
gateway projection for M4's already-shipped terminal-status query, and refresh
of Meerkat's downstream compatibility table after M3. **Follow-up update
(2026-09-22):** the console-health affordance subsequently landed in MobKit
PR #279 (`f3db01834548ede68faf56bfd2d8cba8a8fea3c3`, 2026-07-13) and is no
longer outstanding. The M4 projection and compatibility-table refresh are
separate local projection/documentation follow-ups, not open upstream runtime
asks.
```

The outstanding work is corrected without silently moving the original checkpoint or treating local surface work as unshipped runtime authority. The dated heading and historical/current-status interpretation remain unchanged.

**`docs/design/upstream-asks.md:772-787`**

```text
on the identity-inspect RPC. The console-health affordance remained a follow-up
at that checkpoint; it subsequently landed in MobKit PR #279 (`f3db0183`,
2026-07-13). When progress is available, sidebar chips expose non-healthy
machine health and roster details show Health, Run state, In flight, and Last
progress.
```

The shipped Meerkat 0.7.29 disposition and MobKit 0.7.35 API/SDK chronology precede this later UI annotation. The original paragraph explicitly identifying the historical pre-0.7.29 state follows it unchanged.

**`https://api.github.com/repos/lukacf/meerkat-mobkit/pulls/279:number,title,merged_at,merge_commit_sha`**

```text
{"merge_commit_sha":"f3db01834548ede68faf56bfd2d8cba8a8fea3c3","merged_at":"2026-07-13T10:07:16Z","number":279,"title":"feat: console health affordance from member progress (ask 14 UI)"}
```

Fresh read-only gh api retrieval independently establishes the exact PR and landing date. git show reports the matching commit date 2026-07-13T12:07:15+02:00; git merge-base --is-ancestor against af82b6b3ab34faed9bf3e962d148d55f10dcd1dc succeeds. The commit changes the Rust projection, console consumers/tests, and both generated console bundles.

**`git blame af82b6b3ab34faed9bf3e962d148d55f10dcd1dc -- docs/design/upstream-asks.md:baseline 763-774`**

```text
8376b0c8a (Luka Crnković-Friis 2026-07-12 13:51:52 +0200 769) on the identity-inspect RPC. Console health affordance remains follow-up.
```

The original pending assertion predates PR #279. The fix correctly annotates subsequent completion rather than pretending that the UI already shipped at the historical checkpoint.

**`meerkat-mobkit/src/http_console.rs:10434-10504;10508-10554`**

```text
    attach_member_progress(handle, &entries, &mut members).await;
```

The production roster projection invokes the progress attachment, rather than leaving an unused helper. That helper queries handle.member_status, serializes snapshot.progress, skips final members and failed lookups, and skips the entire projection above the default 64-member cap. MOBKIT_CONSOLE_PROGRESS_MEMBER_CAP overrides the cap; zero disables it. These omissions are covered by the final 'When progress is available' qualification.

**`meerkat-mobkit/src/runtime/console_ingress.rs:1108-1111`**

```text
            // Machine-owned liveness projection (meerkat 0.7.29+, ask 14).
            if let Some(progress) = agent.get("progress").filter(|value| !value.is_null()) {
                row["progress"] = progress.clone();
            }
```

The console identity-row projection carries the actual progress. I also read packages/console-core/src/control-plane.ts:28-39,338-341 and console/src/lib/agents.ts:90-185: normalization and each relevant agent-row construction preserve supplied progress without requiring it.

**`console/src/lib/adapters.ts:222-229;304-308`**

```text
  const health = agent.progress?.health;
  if (!health || health === "healthy") return null;
  const tone: ConsoleSidebarMetaTone = health === "wedged" ? "negative" : health === "degraded" ? "warning" : "muted";
  return { id: "health", label: health, tone };
```

The sidebar adapter calls this helper while building metadata, so healthy/absent health is silent and non-healthy supplied values receive chips. console/src/ConsoleApp.tsx:3869-3873 invokes that view-state construction in the actual app. The existing regression at adapters.test.ts:91-150 explicitly checks wedged, degraded and healthy cases; I inspected it but did not execute it.

**`console/src/panels/RosterPanel.tsx:160-181`**

```text
                    <dt>Run state</dt><dd className="mono">{active.progress.run_state}</dd>
                    <dt>In flight</dt><dd className="mono">{active.progress.in_flight_work}</dd>
                    <dt>Last progress</dt>
```

The surrounding active.progress guard conditionally renders Health as well as these fields. The corrected prose names the exact UI labels and does not confuse lifecycle or lease health with member progress.

**`https://github.com/lukacf/meerkat/blob/0fa68f3b5771d87c703aa09192be6363c9154746/meerkat-mob/src/runtime/handle.rs:962-998`**

```text
pub struct MemberProgressSnapshot {
    pub run_state: MemberRunState,
    pub in_flight_work: u64,
    pub last_progress_at_ms: u64,
    pub last_progress_event: MemberProgressEvent,
    pub health: MemberHealthClass,
}
```

A fresh exact-tag lookup resolved v0.8.40 to this SHA, matching the local manifest/lock version. Current source retains the machine-owned progress contract and Healthy/Degraded/Wedged/Unknown vocabulary. This is corroboration of current behavior, not evidence that the console existed at an earlier MobKit release.

## I-002: Ask 9's historical evidence falsely says Meerkat 0.7.15 lacked ToolDef.provenance

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/upstream-asks.md:595-599`**

```text
**Evidence.** Both gaps are documented as upstream-gated in
`meerkat-mobkit/src/memory/taint.rs` module docs ("Honest gaps that remain
(upstream asks, §13)"): the race, and "no `ToolDef.provenance`". Verified
still absent in meerkat-core 0.7.15 (`types.rs` has no tool provenance
field; no dispatch-time host taint seam).
```

The work-order ledger presents this as code-verified evidence, incorrectly assigning a downstream integration limitation to an absent upstream API in a named release. Readers diagnosing older deployments or reconstructing the fix chronology cannot rely on the claimed verification, despite the correct closed status.

**`https://github.com/lukacf/meerkat/blob/84ecb083e982929d06217185acdcfc2f0182fffb/meerkat-core/src/types.rs:2935-2942`**

```text
pub struct ToolDef {
    pub name: ToolName,
    pub description: String,
    pub input_schema: Value,
    /// Optional provenance metadata tracking which subsystem materialized this tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ToolProvenance>,
}
```

Read through gh API, first at v0.7.15 and then at the exact tag-resolved commit 84ecb083e982929d06217185acdcfc2f0182fffb. The exact historical version named by the evidence already has the field. This is not a comparison of a historically valid claim with a newer API.

**`https://github.com/lukacf/meerkat/blob/84ecb083e982929d06217185acdcfc2f0182fffb/meerkat-core/src/hooks.rs:238-243`**

```text
    /// Typed provenance of the dispatched tool definition, when the active
    /// tool catalog carries one. Projection of the `ToolDef.provenance` owner
    /// (never re-derived from the tool name string): dispatch-time policy
    /// hooks steer on this typed field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ToolProvenance>,
```

The same 0.7.15 source carries typed provenance into HookToolCall. It also has HookToolResult.provenance at 269-273. The tracker must distinguish upstream capability availability from a specific downstream observer-only integration.

**`(operator-retained path)/meerkat-core-0.8.40/src/types.rs:3728-3735`**

```text
pub struct ToolDef {
    pub name: ToolName,
    pub description: String,
    pub input_schema: Value,
    /// Optional provenance metadata tracking which subsystem materialized this tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ToolProvenance>,
}
```

The current exact-pinned dependency retains the same field; the explicit closed status is compatible with source, while the historical 'verified absent' explanation is not.

### Independent adjudication

Confirmed only for the exact historical assertion that meerkat-core 0.7.15 types.rs lacked ToolDef.provenance and upstream had no provenance-bearing tool-hook seam. This is not a comparison of old prose to a later API: I independently resolved v0.7.15 to 84ecb083e982929d06217185acdcfc2f0182fffb, checked that commit's Cargo.toml version, and read its actual ToolDef and hook definitions via gh API. The field and hook payloads already exist in that very release. The source-history challenge is important: the filing-time MobKit taint.rs comment says its observe-stream tool EVENTS lack provenance, not that ToolDef itself lacks the field. Historical Meerkat AgentEvent definitions confirm that narrower statement was valid. Upstream capability availability, asynchronous event payloads, and the ability of a particular MobKit composition to install synchronous policy are different claims. Preserve the original observer-only incident and integration limitation; qualify the faulty upstream-absence diagnosis rather than deleting the ask or representing all historical/custom compositions as already protected. The current exact =0.8.40 pin still has the field, but current source is corroboration rather than the basis for rejecting the 0.7.15 assertion. The upstream audit's A17-020/A17-021 address another document and explicitly retain custom-composition/name-only-observer caveats; they do not authorize rewriting this historical report wholesale.

**`docs/design/upstream-asks.md:595-599`**

```text
**Evidence.** Both gaps are documented as upstream-gated in
`meerkat-mobkit/src/memory/taint.rs` module docs ("Honest gaps that remain
(upstream asks, §13)"): the race, and "no `ToolDef.provenance`". Verified
still absent in meerkat-core 0.7.15 (`types.rs` has no tool provenance
field; no dispatch-time host taint seam).
```

The defect is the claimed verification against a named historical version. git blame attributes this unchanged assertion to a534d6fec65245d0c5345c5ba9e8d5730f05f0f2, rather than a present-day API guide.

**`https://api.github.com/repos/lukacf/meerkat/git/refs/tags/v0.7.15:object.sha`**

```text
"sha":"84ecb083e982929d06217185acdcfc2f0182fffb","type":"commit"
```

Resolved independently using gh api. All following historical source reads use this immutable SHA, not the moving documentation-audit branch.

**`https://github.com/lukacf/meerkat/blob/84ecb083e982929d06217185acdcfc2f0182fffb/Cargo.toml:51-52`**

```text
[workspace.package]
version = "0.7.15"
```

Confirms the inspected source belongs to the exact version named by the ledger.

**`https://github.com/lukacf/meerkat/blob/84ecb083e982929d06217185acdcfc2f0182fffb/meerkat-core/src/types.rs:2935-2942`**

```text
pub struct ToolDef {
    pub name: ToolName,
    pub description: String,
    pub input_schema: Value,
    /// Optional provenance metadata tracking which subsystem materialized this tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ToolProvenance>,
}
```

Decisive historical implementation proof: the allegedly absent field is present. It is optional, so this does not prove every tool definition supplies metadata.

**`https://github.com/lukacf/meerkat/blob/84ecb083e982929d06217185acdcfc2f0182fffb/meerkat-core/src/hooks.rs:234-243;269-273`**

```text
pub struct HookToolCall {
    pub tool_use_id: String,
    pub name: String,
    pub args: ToolCallArguments,
    /// Typed provenance of the dispatched tool definition, when the active
    /// tool catalog carries one. Projection of the `ToolDef.provenance` owner
    /// (never re-derived from the tool name string): dispatch-time policy
    /// hooks steer on this typed field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ToolProvenance>,

    /// Typed provenance of the dispatched tool definition, when the active
    /// tool catalog carries one. Projection of the `ToolDef.provenance` owner
    /// (never re-derived from the tool name string).
    #[serde(default)]
    pub provenance: Option<ToolProvenance>,
```

Both HookToolCall and HookToolResult already carry typed provenance. This disproves blanket payload/API absence without asserting that the old MobKit member builder installed those hooks.

**`https://github.com/lukacf/meerkat-mobkit/blob/a534d6fec65245d0c5345c5ba9e8d5730f05f0f2/meerkat-mobkit/src/memory/taint.rs:31-51`**

```text
//! - **The first-ingestion race** (ask: taint visibility at tool-dispatch
//!   time): taint is derived from the observe-only agent-event stream,
//!   which is asynchronous.

//! - **Name-based classification**: meerkat tool events carry only the tool
//!   NAME — no `ToolDef.provenance` — so MCP tools cannot be attributed to
//!   a server unless their names are server-qualified
```

Read directly with git show at the filing commit. The original comment describes a real observer integration limitation. The ledger incorrectly generalized absence on these events to absence on ToolDef itself; retain that distinction in the fix.

**`https://github.com/lukacf/meerkat/blob/84ecb083e982929d06217185acdcfc2f0182fffb/meerkat-core/src/event.rs:1707-1719;1729-1741`**

```text
    ToolCallRequested {
        id: String,
        name: String,
        args: ToolCallArguments,
    },

    ToolExecutionCompleted {
        id: String,
        name: String,
        /// Canonical typed tool-result content. Display text is derived from
        /// these blocks at the consumer edge, never carried beside them.
        content: Vec<ContentBlock>,
        is_error: bool,
        duration_ms: u64,
    },
```

Independent counter-evidence against an overbroad correction: asynchronous AgentEvent payloads lack provenance in the same release. The historical observer warning is not false merely because ToolDef and hook payloads contain richer information.

**`https://github.com/lukacf/meerkat/blob/0fa68f3b5771d87c703aa09192be6363c9154746/meerkat-core/src/types.rs:3728-3735`**

```text
pub struct ToolDef {
    pub name: ToolName,
    pub description: String,
    pub input_schema: Value,
    /// Optional provenance metadata tracking which subsystem materialized this tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ToolProvenance>,
}
```

The local manifest pins meerkat-core =0.8.40; independently resolving the v0.8.40 tag identifies this SHA. Current source retains the field, consistent with the ask remaining closed. No dependency installation or execution was used.

**`https://github.com/lukacf/meerkat/blob/bdb53a24f7a1ac048fe91a328f82e161e00a2b95/audit/docs-review-2026-09-21/A17.md:2926-2928;3107-3109`**

```text
The asynchronous observer still has a name-only path, so the subclaim that those events lack provenance need not be replaced by a false claim that every event is richer.

An unfilled custom slot remains a pass-through, and this evidence does not establish universal race freedom or prompt-injection safety.
```

Read the relevant A17-020/A17-021 sections independently at the supplied audit SHA. Their caveats agree with the source distinction, but their imported memory-guide findings are not additional scope-I findings. The same audit README:24-30 says the imported MobKit snapshot remains release-pinned; it is not authority over this checkout.

**Required correction:** Replace only ask 9's evidence paragraph at lines 595-599 with: '**Historical evidence and correction.** The filing-time MobKit `meerkat-mobkit/src/memory/taint.rs` module docs recorded the first-ingestion race and that its asynchronous tool events carried names without `ToolDef.provenance`. The original report incorrectly generalized this to missing upstream APIs in Meerkat 0.7.15: `ToolDef.provenance` and provenance-bearing `HookToolCall`/`HookToolResult` payloads were already present in that release (`84ecb083e982929d06217185acdcfc2f0182fffb`). This corrects the reported upstream diagnosis, not the historical MobKit observer-only integration limitation; upstream capability availability and a host composition’s use of those capabilities are separate adoption questions.' Preserve the historical problem/proposal/interim paragraphs and the closed disposition. Do not claim every observe event carried provenance, every custom member composition had synchronous protection, or the first-ingestion incident never existed. Do not reopen ask 9 or modify the separately owned memory guide.

### Changes and final verification

**Changed:** `docs/design/upstream-asks.md`.

Replaced only ask 9's erroneous evidence paragraph with the independently adjudicated historical correction. It records that Meerkat 0.7.15 already supplied optional ToolDef.provenance and provenance-bearing HookToolCall/HookToolResult payloads at commit 84ecb083e982929d06217185acdcfc2f0182fffb, while explicitly preserving the historical MobKit observer-only integration limitation and distinguishing API availability from host adoption. Ask 9's closed disposition, historical incident, proposal, and interim behavior remain intact.

**Validation:** Read-only gh api source checks at 84ecb083e982929d06217185acdcfc2f0182fffb confirmed Cargo.toml:51-52 identifies version 0.7.15, types.rs:2935-2942 contains optional ToolDef.provenance, hooks.rs:234-274 contains provenance in both hook payloads, and event.rs:1707-1741 lacks provenance on the asynchronous tool events. git show a534d6fec65245d0c5345c5ba9e8d5730f05f0f2:meerkat-mobkit/src/memory/taint.rs confirmed the filing-time first-ingestion race and name-only event caveats. Scoped diff and ledger-preservation assertions passed.

**Final review: pass.** The replacement corrects exactly the provably false named-release API-absence diagnosis, not the genuine historical observer-only integration limitation. I independently resolved Meerkat v0.7.15, checked its manifest, and read its ToolDef, HookToolCall, HookToolResult, and asynchronous AgentEvent definitions at the immutable release SHA. The field and hook payloads exist, while the relevant asynchronous tool events lack provenance. I also inspected the filing-time MobKit tracker implementation, not just its comments: its tool-result observer classifies by name. The new paragraph explicitly separates capability availability from host adoption, and the original problem, proposal, interim mitigation and closed status are preserved byte-for-byte. Current local production wiring and the pinned upstream audit independently support retaining the custom-composition and name-only-observer caveats rather than claiming universal synchronous protection.

**`docs/design/upstream-asks.md:581-621`**

```text
**Historical evidence and correction.** The filing-time MobKit
`meerkat-mobkit/src/memory/taint.rs` module docs recorded the first-ingestion
race and that its asynchronous tool events carried names without
`ToolDef.provenance`. The original report incorrectly generalized this to
missing upstream APIs in Meerkat 0.7.15: `ToolDef.provenance` and
provenance-bearing `HookToolCall`/`HookToolResult` payloads were already present
in that release (`84ecb083e982929d06217185acdcfc2f0182fffb`). This corrects
the reported upstream diagnosis, not the historical MobKit observer-only
integration limitation; upstream capability availability and a host
composition's use of those capabilities are separate adoption questions.
```

Final lines 599-608 implement the narrowed correction. Surrounding history still records the first-ingestion race, name-based classification, proposed dispatch seam, quarantined posture and original adoption intent. It neither erases the incident nor reopens ask 9.

**`https://api.github.com/repos/lukacf/meerkat/git/refs/tags/v0.7.15:object.sha;object.type`**

```text
"sha": "84ecb083e982929d06217185acdcfc2f0182fffb"
```

Fresh gh api lookup returned a commit object with this SHA. At the same immutable SHA, Cargo.toml:51-52 reads [workspace.package] and version = "0.7.15". The historical claim is tested against its own named release, not modern source or the moving upstream audit branch.

**`https://github.com/lukacf/meerkat/blob/84ecb083e982929d06217185acdcfc2f0182fffb/meerkat-core/src/types.rs:2935-2942`**

```text
pub struct ToolDef {
    pub name: ToolName,
    pub description: String,
    pub input_schema: Value,
    /// Optional provenance metadata tracking which subsystem materialized this tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ToolProvenance>,
}
```

Decisive exact-release source proof. Optional metadata proves API availability, not that every tool or host supplied it; the replacement makes no universal-presence promise.

**`https://github.com/lukacf/meerkat/blob/84ecb083e982929d06217185acdcfc2f0182fffb/meerkat-core/src/hooks.rs:234-244;262-274`**

```text
    /// Typed provenance of the dispatched tool definition, when the active
    /// tool catalog carries one. Projection of the `ToolDef.provenance` owner
    /// (never re-derived from the tool name string): dispatch-time policy
    /// hooks steer on this typed field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ToolProvenance>,
```

HookToolCall already has the quoted optional provenance field, and HookToolResult independently has provenance at lines 269-273. These are real historical hook payload definitions, not assertions copied from the adjudication.

**`https://github.com/lukacf/meerkat/blob/84ecb083e982929d06217185acdcfc2f0182fffb/meerkat-core/src/event.rs:1707-1720;1729-1741`**

```text
    ToolCallRequested {
        id: String,
        name: String,
        args: ToolCallArguments,
    },
```

The complete relevant ToolCallRequested, ToolResultReceived, ToolExecutionStarted and ToolExecutionCompleted variants were inspected. They carry names/content but no provenance field. A correction claiming every observe event was already provenance-bearing would be false; this fix deliberately avoids that regression.

**`https://github.com/lukacf/meerkat-mobkit/blob/a534d6fec65245d0c5345c5ba9e8d5730f05f0f2/meerkat-mobkit/src/memory/taint.rs:31-51;305-359`**

```text
            AgentEvent::ToolResultReceived { name, .. }
            | AgentEvent::ToolExecutionCompleted { name, .. } => {
                if let ToolContentTrust::Untrusted { source } = self.config.classify_tool(name) {
                    self.mark_identity_tainted(identity, source);
                }
            }
```

Fresh git show at the ledger's filing commit confirms the name-only observer in executable implementation. Its module documentation at lines 33-39 records the asynchronous first-ingestion race, and lines 45-51 identify missing provenance on tool events, not missing ToolDef metadata. git blame independently identifies this commit as the source of the faulty ledger diagnosis.

**`meerkat-mobkit/src/memory/dispatch_taint.rs:73-99;130-152;217-229;253-276`**

```text
        if let Some(tracker) = self.slot.tracker() {
            self.mark_request_ingestions(&tracker, messages, tools);
        }
```

Current code marks request ingestions before the inner LLM call and marks server-tool blocks before returning the response. mark_request_ingestions joins tools[*].provenance to results. The tracker slot is Option-backed; an unfilled slot does not run these marks. This supports the new adoption distinction without retroactively granting the historical or arbitrary custom composition this behavior.

**`meerkat-mobkit/src/memory/taint.rs:193-220;382-389;408-419`**

```text
            AgentEvent::ToolResultReceived { name, .. }
            | AgentEvent::ToolExecutionCompleted { name, .. } => {
                if let ToolContentTrust::Untrusted { source } = self.config.classify_tool(name) {
                    self.mark_identity_tainted(identity, source);
                }
            }
```

The name-only asynchronous path still exists alongside the richer dispatch path. classify_tool_with_provenance recognizes ToolSourceKind::Mcp/source_id, while the dispatcher feed accepts Option<&ToolProvenance>. Searches also verified actual member-build attachment at mob_handle_runtime.rs:725 and slot filling at unified_runtime/builder.rs:1475 and bin/rpc_gateway.rs:12908.

**`https://github.com/lukacf/meerkat/blob/bdb53a24f7a1ac048fe91a328f82e161e00a2b95/audit/docs-review-2026-09-21/A17.md:2926-2928;3107-3109`**

```text
An unfilled custom slot remains a pass-through, and this evidence does not establish universal race freedom or prompt-injection safety.
```

Independently retrieved and read A17-020 and A17-021. They concern a different memory guide and preserve exactly the distinctions corroborated here by historical and local source. They are context, not authority to erase the original ask or treat imported release documentation as current MobKit source.

## Independent scope checks

> [
>   "Read audit-brief.md, audit-scopes.json, audit-I.json, and all 1,869 lines of docs/design/upstream-asks.md.",
>   "Baseline git rev-parse HEAD: af82b6b3ab34faed9bf3e962d148d55f10dcd1dc. Worktree was clean before and after read-only investigation.",
>   "Independently read current console producer, normalization/propagation, consumers, and targeted test bodies; traced introducing commit and verified its ancestry.",
>   "Independently retrieved historical Meerkat 0.7.15 source, current Meerkat 0.8.40 source, and relevant upstream audit sections with read-only gh API calls at immutable revisions.",
>   "git show 01435e7ccda5e925fc2cf9f462327bdae9584d07 -- docs/design/upstream-asks.md produced no owned-file diff; prior correction identity was not used to exclude either finding.",
>   "No tests, builds, live deployments, provider calls, dependency changes, repository edits, commits, pushes, or delegation were performed."
> ]

## Final scope checks

> [
>   "Read the coordination brief, audit-scopes.json, audit-I.json, adjudication-I.json and fixes-I.json. The scope has exactly two candidates, both confirmed and marked fixed, and no rejected or supplemental items.",
>   "Read all 1,882 final lines of docs/design/upstream-asks.md, including the long historical addenda and final acceptance section. Read its entire scoped git diff. No other scope's documentation was audited or edited.",
>   "Independently compared the final document to baseline af82b6b3ab34faed9bf3e962d148d55f10dcd1dc using difflib.SequenceMatcher with autojunk disabled. Exactly three replacement spans exist: baseline 45-49 -> final 45-53; baseline 595-599 -> final 599-608; baseline 769 -> final 778-782. Every other byte is preserved.",
>   "Python assertions passed for all 40 unchanged ask headings, the unchanged July 12 checkpoint/adoption introduction, and the unchanged original problem, proposed-shape and interim-behavior paragraphs for asks 9 and 14. The diff proves the closed/shipped status lines and watchdog discussion were not removed or broadened.",
>   "git diff --check -- docs/design/upstream-asks.md passed. Both Markdown relative targets still exist and are not symlinks; the ledger is not a symlink. The Markdown-link inventory is unchanged and no new link or runnable example was introduced.",
>   "Fresh gh api PR #279 metadata, local git show/blame and git merge-base --is-ancestor established the exact console UI introduction and its chronological relationship to the prior pending statement. No first published MobKit release containing PR #279 was inferred.",
>   "Traced current member_status progress through the capped/fallible backend projection, identity rows, TypeScript normalization, agent-row mapping, sidebar adapter, live app call site and conditional roster details. Inspected the targeted existing sidebar regression; no runtime or browser test was executed.",
>   "Fresh read-only gh api calls resolved Meerkat v0.7.15 to 84ecb083e982929d06217185acdcfc2f0182fffb and retrieved its version manifest, ToolDef, both hook payloads and complete relevant asynchronous tool-event definitions. Fresh git show at filing commit a534d6fec65245d0c5345c5ba9e8d5730f05f0f2 verified the historical MobKit observer implementation and its documented race.",
>   "Read local manifests and parsed Cargo.lock: MobKit workspace 0.8.39, relevant Meerkat dependencies exactly 0.8.40. Fresh tag lookup resolved Meerkat v0.8.40 to 0fa68f3b5771d87c703aa09192be6363c9154746; retrieved its Cargo version, current provenance payloads and MemberProgressSnapshot as corroborating source.",
>   "Independently retrieved the supplied upstream audit README, relevant A17-020/A17-021 evidence/challenge sections and docs/mobkit/_source.json at bdb53a24f7a1ac048fe91a328f82e161e00a2b95. That imported snapshot is v0.8.35 at 426ebdfc4101e69abb4c6749f14d15dc14bbd524, not this checkout. Publication boundaries and custom-composition caveats were preserved.",
>   "Inspected prior canonical correction commit 01435e7ccda5e925fc2cf9f462327bdae9584d07 for this owned document: no diff. Neither finding was excluded on prior-commit identity.",
>   "Reviewed document SHA256: 3c3ea3ce7d6e4bd01378e0c0c43f43f164fce6a0d6b4a407b2f52a4a057197a2. Baseline document SHA256: 4f1f07814ef965068c25fb932c7b1ad124cccdfc6f60dc0e98258b4a88cc0b03. HEAD remained af82b6b3ab34faed9bf3e962d148d55f10dcd1dc.",
>   "No repository edits, delegation, dependency operations, builds, runtime/provider calls, commits, pushes, publication or git-state mutation were performed by this reviewer. Only this requested session artifact was written. This is independently sourced documentation/history verification, not live-runtime acceptance or a renewed PR-by-PR proof of all 40 original shipments."
> ]
