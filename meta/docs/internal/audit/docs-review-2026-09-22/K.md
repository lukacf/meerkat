# K: Recovered corrections from the prior unmerged audit

[Audit index](README.md) | [Coverage](coverage.md)

Original documentation and initial evidence line ranges refer to baseline `af82b6b3ab34faed9bf3e962d148d55f10dcd1dc`, unless an external dependency or historical revision is explicitly identified. Final-review citations refer to the corrected files in this change. Source excerpts may be de-indented or omit intervening lines; cited ranges identify the complete context. Quoted defects are preserved as evidence, not current usage guidance.

## K-001: The access-control TypeScript example calls nonexistent builder methods

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/access-control.mdx:116-119`**

```text
const mobkit = await MobKit.builder()
  .mobConfig("config/mob.toml")
  .accessControl("config/access.toml") // auto-discovered if the file exists
  .start();
```

The copyable access-control setup fails TypeScript checking or throws at mobConfig before any runtime or access controller starts.

**`sdk/typescript/src/builder.ts:195-211`**

```text
 * const rt = await MobKit.builder()
 *   .mob("config/mob.toml")
 *   .gateway("./target/release/rpc_gateway")
 *   .build();
```

The public builder example and actual mob(configPath: string) declaration use mob, not mobConfig. A direct inventory of method declarations finds mob/accessControl/gateway/build and no mobConfig/start.

**`sdk/typescript/src/builder.ts:729-732`**

```text
  gateway(binPath: string): this {
    this._config.gatewayBin = binPath;
    return this;
  }
```

The SDK host explicitly chooses its gateway executable; the replacement must retain this setup rather than only renaming the nonexistent methods.

**`sdk/typescript/src/builder.ts:799-805`**

```text
  async build(): Promise<MobKitRuntime> {
    this._validateConfig();
    this._applyConventionDefaults();
    // Dynamic import to break circular dep (runtime imports from builder config type)
    const { MobKitRuntime } = await import("./runtime.js");
    return MobKitRuntime._create(this._config);
  }
```

build is the implemented asynchronous construction entry point.

### Independent adjudication

The current public MobKitBuilder class declares mob(configPath), accessControl(configPath), gateway(binPath), and async build(); it neither inherits nor declares mobConfig() or start(). The documentation therefore fails before access-control configuration can run. Merely renaming those methods would leave this particular example without a gateway process: runtime bootstrap is conditional on gatewayBin. No audit-A through audit-J finding claims these nonexistent methods; C-018 through C-022 concern configuration validation, action vocabulary, authentication, persistence, and cross-mob grants instead. D-005 concerns different storage declaration examples, not this builder-method defect.

**`docs/concepts/access-control.mdx:116-119`**

```text
const mobkit = await MobKit.builder()
  .mobConfig("config/mob.toml")
  .accessControl("config/access.toml") // auto-discovered if the file exists
  .start();
```

The invalid method chain remains in the current checkout.

**`sdk/typescript/src/builder.ts:204-211`**

```text
export class MobKitBuilder {
  /** @internal */
  readonly _config: MobKitBuilderConfig = defaultConfig();

  mob(configPath: string): this {
    this._config.mobConfigPath = configPath;
    return this;
  }
```

The actual class has no superclass providing alternate method names, and mob is the configuration setter.

**`sdk/typescript/src/builder.ts:276-279`**

```text
  accessControl(configPath: string): this {
    this._config.accessConfigPath = configPath;
    return this;
  }
```

The accessControl portion is already correct and should be preserved.

**`sdk/typescript/src/builder.ts:729-732`**

```text
  gateway(binPath: string): this {
    this._config.gatewayBin = binPath;
    return this;
  }
```

Gateway selection belongs in this builder chain.

**`sdk/typescript/src/builder.ts:799-805`**

```text
  async build(): Promise<MobKitRuntime> {
    this._validateConfig();
    this._applyConventionDefaults();
    // Dynamic import to break circular dep (runtime imports from builder config type)
    const { MobKitRuntime } = await import("./runtime.js");
    return MobKitRuntime._create(this._config);
  }
```

build is the asynchronous runtime construction method.

**`sdk/typescript/src/runtime.ts:649-653`**

```text
  private async _bootstrap(): Promise<void> {
    if (this._config.gatewayBin) {
      this._transport = new PersistentTransport(this._config.gatewayBin, {
        timeout: this._config.gatewayTimeoutMs ?? undefined,
      });
```

The runtime does not start a gateway automatically when the binary path is absent.

**`sdk/typescript/src/index.ts:30`**

```text
export { MobKit, MobKitBuilder } from "./builder.js";
```

The proposed package-root import is supported.

**Required correction:** Replace the TypeScript block with: import { MobKit } from "@rkat/mobkit-sdk"; followed by const mobkit = await MobKit.builder().mob("config/mob.toml").accessControl("config/access.toml").gateway("/path/to/rpc_gateway").build(); format the chain on separate lines. Preserve the accessControl auto-discovery comment. Link the binary-path prerequisite to /quickstart#install. This is a documentation correction, not a request to introduce SDK aliases.

### Changes and final verification

**Changed:** `docs/concepts/access-control.mdx`.

Replaced nonexistent TypeScript mobConfig/start methods with package-root MobKit import and mob/accessControl/gateway/build; preserved the discovery comment and linked the gateway-install prerequisite.

**Validation:** Compared builder.ts public methods, runtime.ts gatewayBin bootstrap condition and index.ts export. The quickstart install anchor resolves; no SDK aliases or code were introduced.

**Final review: pass.** The C-owned TypeScript example now uses the actual package export and mob/accessControl/gateway/build methods, with an explicit executable prerequisite and a resolving quickstart link. No SDK aliases or runtime behavior were introduced by this fix.

**`docs/concepts/access-control.mdx:125-136`**

```text
.gateway("/path/to/rpc_gateway")
```

The chain imports MobKit, preserves the discovery comment and ends in build rather than nonexistent start.

**`sdk/typescript/src/builder.ts:799-805`**

```text
async build(): Promise<MobKitRuntime> {
```

Actual mob/accessControl/gateway declarations were read at 208-210,276-279,729-732. runtime.ts:649-653 only starts the process when gatewayBin is configured.

**Final review: pass.** The access-control TypeScript block now uses the exported MobKit builder, all four implemented methods, an explicit gateway executable, and a working installation link. I checked the class declarations and the runtime's conditional transport bootstrap rather than accepting the fix report. The neighboring unchanged Python sample was not used as evidence for this TypeScript finding.

**`docs/concepts/access-control.mdx:125-137`**

```text
const mobkit = await MobKit.builder()
  .mob("config/mob.toml")
  .accessControl("config/access.toml") // auto-discovered if the file exists
  .gateway("/path/to/rpc_gateway")
  .build();
```

The corrected chain retains the discovery comment and includes the previously missing gateway selection.

**`sdk/typescript/src/builder.ts:204-211`**

```text
mob(configPath: string): this
```

The actual builder declares mob; accessControl, gateway and build were independently checked at 276-279, 729-732 and 799-805.

**`sdk/typescript/src/runtime.ts:649-653`**

```text
if (this._config.gatewayBin) {
      this._transport = new PersistentTransport(this._config.gatewayBin, {
```

Providing the binary is material to starting the transport.

**`sdk/typescript/src/index.ts:30`**

```text
export { MobKit, MobKitBuilder } from "./builder.js";
```

The documented package-root import is real. The new /quickstart#install link also resolves.

## K-002: Both memory Rust value examples assign borrowed string literals to owned String fields

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/memory.mdx:50-53`**

```text
LocalJsonMemoryBackendConfig {
    state_path: "/var/mobkit/state/memory-ledger-state.json",
    health_check_endpoint: Some("http://localhost:3000"),
}
```

Readers copying either Rust memory example encounter mismatched-type compilation errors even with the correct exported types imported.

**`meerkat-mobkit/src/runtime.rs:566-570`**

```text
pub struct LocalJsonMemoryBackendConfig {
    pub state_path: String,
    #[serde(default)]
    pub health_check_endpoint: Option<String>,
}
```

Rust struct literals do not implicitly convert &str to String, including within Some. Both documented assignments have the wrong type.

**`docs/concepts/memory.mdx:231-234`**

```text
MemoryStoreInfo {
    store: "knowledge_graph",
    record_count: 42,
}
```

The second prior-corrected value example has the same borrowed-versus-owned mismatch and is grouped into this finding rather than counted separately.

**`meerkat-mobkit/src/runtime.rs:778-781`**

```text
pub struct MemoryStoreInfo {
    pub store: String,
    pub record_count: usize,
}
```

MemoryStoreInfo.store also requires String.

### Independent adjudication

Both fences are Rust value literals, not schematic field-type declarations. Their string expressions are &str, whereas the actual public fields require String and Option<String>; Rust does not perform these owned conversions implicitly. The nearby field table even states the owned types, so there is no reasonable pseudocode interpretation that makes the copyable literals correct. G-001 through G-004 address ledger persistence, normalization, conflict semantics, and panel methods; none includes these literal type errors. The two occurrences are one finding, not separate counts.

**`docs/concepts/memory.mdx:50-53`**

```text
LocalJsonMemoryBackendConfig {
    state_path: "/var/mobkit/state/memory-ledger-state.json",
    health_check_endpoint: Some("http://localhost:3000"),
}
```

Both strings in this literal lack an owned conversion.

**`meerkat-mobkit/src/runtime.rs:566-570`**

```text
pub struct LocalJsonMemoryBackendConfig {
    pub state_path: String,
    #[serde(default)]
    pub health_check_endpoint: Option<String>,
}
```

The two required target types are unambiguously owned.

**`docs/concepts/memory.mdx:231-234`**

```text
MemoryStoreInfo {
    store: "knowledge_graph",
    record_count: 42,
}
```

The later memory-store literal repeats the same kind of error.

**`meerkat-mobkit/src/runtime.rs:778-781`**

```text
pub struct MemoryStoreInfo {
    pub store: String,
    pub record_count: usize,
}
```

record_count is already valid; only store needs conversion.

**Required correction:** Use state_path: "/var/mobkit/state/memory-ledger-state.json".into(), health_check_endpoint: Some("http://localhost:3000".into()), and store: "knowledge_graph".into() in the two existing Rust value literals. Leave the paths, endpoint, record_count, and surrounding memory behavior unchanged.

### Changes and final verification

**Changed:** `docs/concepts/memory.mdx`.

Added .into() to LocalJsonMemoryBackendConfig.state_path, the endpoint within Some, and MemoryStoreInfo.store, preserving their values and record_count.

**Validation:** Verified the actual String/Option<String> field declarations in runtime.rs and the three converted literal expressions. Source-contract validation only; no Rust build or live snippet execution was needed for these documentary edits.

**Final review: pass.** All three borrowed-string assignments now convert to owned Strings at the existing literal sites, including inside Some. The example values and record_count remain unchanged; no API or storage semantics were altered.

**`docs/concepts/memory.mdx:50-53`**

```text
state_path: "/var/mobkit/state/memory-ledger-state.json".into(),
    health_check_endpoint: Some("http://localhost:3000".into()),
```

Both LocalJsonMemoryBackendConfig assignments now target their declared owned types.

**`docs/concepts/memory.mdx:250-253`**

```text
store: "knowledge_graph".into(),
    record_count: 42,
```

The second literal is corrected without changing the numerical example.

**`meerkat-mobkit/src/runtime.rs:566-570`**

```text
pub state_path: String,
    #[serde(default)]
    pub health_check_endpoint: Option<String>,
```

The actual public config fields require these conversions.

**`meerkat-mobkit/src/runtime.rs:778-781`**

```text
pub store: String,
    pub record_count: usize,
```

MemoryStoreInfo's actual field types match the corrected literal.

**Final review: pass.** All three owned-string expressions, across both Rust value literals, now have .into(). The values and record count are preserved. The concrete source fields remain String and Option<String>; no runtime or field-type change was used to accommodate the snippets.

**`docs/concepts/memory.mdx:49-54`**

```text
state_path: "/var/mobkit/state/memory-ledger-state.json".into(),
    health_check_endpoint: Some("http://localhost:3000".into()),
```

Both formerly borrowed backend configuration values now convert to owned strings.

**`docs/concepts/memory.mdx:248-254`**

```text
store: "knowledge_graph".into(),
    record_count: 42,
```

The second affected literal is corrected too.

**`meerkat-mobkit/src/runtime.rs:566-570`**

```text
pub state_path: String,
    #[serde(default)]
    pub health_check_endpoint: Option<String>,
```

These are the exact destination types for the backend configuration expressions.

**`meerkat-mobkit/src/runtime.rs:778-781`**

```text
pub store: String,
    pub record_count: usize,
```

The store-info conversion is likewise required by the public type.

## K-003: Current memory trust classification is incorrectly described as name-only

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/memory.mdx:184`**

```text
In this release classification is by tool *name* (tool events do not carry provenance yet), so MCP tools are attributable to a server only when their names are qualified as `mcp__<server>__<tool>`; list unqualified MCP tool names in `untrusted_tools`.
```

Operators are told the server allowlist cannot classify unqualified MCP tools and may add explicit untrusted overrides that defeat their intended server-level trust policy.

**`meerkat-mobkit/src/memory/taint.rs:209-220`**

```text
        if let Some(provenance) = provenance
            && provenance.kind == ToolSourceKind::Mcp
        {
            return self.classify_mcp_server(provenance.source_id.as_str(), name);
        }
        if let Some(rest) = name.strip_prefix(MCP_QUALIFIED_PREFIX) {
            let server = rest.split("__").next().unwrap_or(rest);
            return self.classify_mcp_server(server, name);
        }
        ToolContentTrust::Trusted
```

The actual classifier attributes MCP tools using typed source_id, independent of their name shape, and only falls back to qualified names when applicable provenance is absent.

**`meerkat-mobkit/src/memory/dispatch_taint.rs:226-230`**

```text
                        let provenance = tools
                            .iter()
                            .find(|tool| tool.name.as_ref() == *name)
                            .and_then(|tool| tool.provenance.as_ref());
                        tracker.observe_dispatched_tool_result(&self.identity, name, provenance);
```

The dispatch-time client joins tool-result names to the request's ToolDef catalog and actually supplies typed provenance, so this is implemented behavior rather than merely an unused upstream capability.

**`meerkat-mobkit/src/memory/dispatch_taint.rs:432-456`**

```text
        let tools = vec![mcp_tool("scrape_page", "scraper")];
        drive(&client, &messages, &tools).await;
```

The regression marks unqualified scrape_page as coming from MCP server scraper and asserts the resulting taint source. It does not rely on a qualified mcp__ name.

**`meerkat-mobkit/src/memory/taint.rs:198-208`**

```text
        if ALWAYS_UNTRUSTED_TOOL_NAMES.contains(&name) {
            return ToolContentTrust::Untrusted {
                source: format!("web tool '{name}'"),
            };
        }
        if self.untrusted_tools.iter().any(|tool| tool == name) {
            return ToolContentTrust::Untrusted {
                source: format!("configured untrusted tool '{name}'"),
            };
        }
        if self.trusted_tools.iter().any(|tool| tool == name) {
```

Explicit tool rules have ordered precedence before server attribution; builtin web names cannot be made trusted by an override.

### Independent adjudication

The current dispatch wrapper does not merely possess an unused provenance-aware classifier: it joins a recognized tool result's tool-use name against the current request's ToolDef catalog and passes that provenance into the live tracker. The classifier uses typed MCP source_id before qualified-name fallback, after explicit tool rules. In contrast, the asynchronous event consumer genuinely invokes name-only classify_tool; that narrower claim must remain rather than asserting that all event payloads now carry provenance. I-002 corrects a historical claim about Meerkat 0.7.15 API availability and explicitly excludes this guide; it does not correct current MobKit adoption or policy instructions. K-004 is distinct ordering/composition behavior, not a duplicate classification claim.

**`docs/concepts/memory.mdx:184`**

```text
In this release classification is by tool *name* (tool events do not carry provenance yet), so MCP tools are attributable to a server only when their names are qualified as `mcp__<server>__<tool>`; list unqualified MCP tool names in `untrusted_tools`.
```

This unqualified current-release claim denies behavior implemented at the dispatch boundary.

**`meerkat-mobkit/src/memory/taint.rs:198-220`**

```text
        if ALWAYS_UNTRUSTED_TOOL_NAMES.contains(&name) {
            return ToolContentTrust::Untrusted {
                source: format!("web tool '{name}'"),
            };
        }
        if self.untrusted_tools.iter().any(|tool| tool == name) {
            return ToolContentTrust::Untrusted {
                source: format!("configured untrusted tool '{name}'"),
            };
        }
        if self.trusted_tools.iter().any(|tool| tool == name) {
            return ToolContentTrust::Trusted;
        }
        if let Some(provenance) = provenance
            && provenance.kind == ToolSourceKind::Mcp
        {
            return self.classify_mcp_server(provenance.source_id.as_str(), name);
        }
        if let Some(rest) = name.strip_prefix(MCP_QUALIFIED_PREFIX) {
            let server = rest.split("__").next().unwrap_or(rest);
            return self.classify_mcp_server(server, name);
        }
        ToolContentTrust::Trusted
```

Direct branch inspection establishes precedence, typed MCP attribution, non-MCP/absent-provenance fallback, and the final trusted default.

**`meerkat-mobkit/src/memory/taint.rs:223-235`**

```text
    fn classify_mcp_server(&self, server: &str, name: &str) -> ToolContentTrust {
        if self
            .trusted_mcp_servers
            .iter()
            .any(|trusted| trusted == server)
        {
            return ToolContentTrust::Trusted;
        }
        ToolContentTrust::Untrusted {
            source: format!("MCP server '{server}' (tool '{name}')"),
        }
    }
```

The typed server identifier is compared directly with trusted_mcp_servers; unlisted MCP servers are untrusted.

**`meerkat-mobkit/src/memory/dispatch_taint.rs:223-230`**

```text
                        let Some(name) = names.get(result.tool_use_id.as_str()) else {
                            continue;
                        };
                        let provenance = tools
                            .iter()
                            .find(|tool| tool.name.as_ref() == *name)
                            .and_then(|tool| tool.provenance.as_ref());
                        tracker.observe_dispatched_tool_result(&self.identity, name, provenance);
```

This production call supplies the typed catalog provenance. The guarantee is for recognized results; a missing matching call/name is not invented.

**`meerkat-mobkit/src/memory/taint.rs:408-419`**

```text
    pub fn observe_dispatched_tool_result(
        &self,
        identity: &str,
        name: &str,
        provenance: Option<&ToolProvenance>,
    ) {
        if let ToolContentTrust::Untrusted { source } =
            self.config.classify_tool_with_provenance(name, provenance)
        {
            self.mark_identity_tainted(identity, source);
        }
    }
```

The supplied provenance affects the session-tracker mark, not just an informational projection.

**`meerkat-mobkit/src/memory/taint.rs:384-389`**

```text
            AgentEvent::ToolResultReceived { name, .. }
            | AgentEvent::ToolExecutionCompleted { name, .. } => {
                if let ToolContentTrust::Untrusted { source } = self.config.classify_tool(name) {
                    self.mark_identity_tainted(identity, source);
                }
            }
```

The asynchronous fallback remains name-based, so replacing the sentence with an all-events-provenance claim would be incorrect.

**Required correction:** Replace the name-only current-release sentence with: 'Dispatch-time classification joins recognized tool results to the request tool catalog's ToolDef.provenance. MCP provenance uses source_id as the server name even when the tool name is unqualified. The asynchronous observe-stream fallback remains name-based.' Document precedence as always-untrusted builtin names (web_search, web_fetch, fetch, http_request), untrusted_tools, trusted_tools, typed MCP server attribution/allowlist, qualified mcp__<server>__<tool> fallback when provenance is absent or non-MCP, then trusted. State that provider-native ServerToolContent is always untrusted and that explicit tool rules precede MCP server rules but cannot override the always-untrusted builtin names. Do not require listing all unqualified MCP tools in untrusted_tools; reserve such overrides for intentional policy or tools without usable attribution.

### Changes and final verification

**Changed:** `docs/concepts/memory.mdx`.

Documented recognized-result ToolDef.provenance joins, typed MCP source_id attribution for unqualified names, and the still-name-based asynchronous fallback. Listed exact classifier precedence, the always-untrusted builtin names, MCP allowlist behavior, and provider-native ServerToolContent treatment. Removed the blanket instruction to override all unqualified MCP tools.

**Validation:** Read classify_tool_with_provenance and dispatch_taint's tool-catalog join; verified the asynchronous name-only classifier evidence. All precedence/attribution caveat assertions passed.

**Final review: pass.** The new trust paragraph is provenance-aware without overclaiming event payloads. It correctly limits the catalog join to recognized results, attributes unqualified MCP names using source_id, preserves the asynchronous name-only fallback, and spells out the exact precedence including non-overridable builtin names and always-untrusted provider-native blocks.

**`docs/concepts/memory.mdx:188`**

```text
Typed MCP provenance uses `source_id` as the server name even when the tool name is unqualified. The asynchronous observe-stream fallback remains name-based.
```

The same paragraph lists always-untrusted names, untrusted_tools, trusted_tools, typed server rules, qualified-name fallback, then trusted, and removes the blanket unqualified-tool override instruction.

**`meerkat-mobkit/src/memory/dispatch_taint.rs:223-230`**

```text
.and_then(|tool| tool.provenance.as_ref());
                        tracker.observe_dispatched_tool_result(&self.identity, name, provenance);
```

The live wrapper first resolves a paired call name, joins the request ToolDef catalog, and actually supplies typed provenance.

**`meerkat-mobkit/src/memory/taint.rs:198-220`**

```text
return self.classify_mcp_server(provenance.source_id.as_str(), name);
```

Direct branch inspection confirmed exact precedence. The always-untrusted constant at line 83 has precisely web_search, web_fetch, fetch, and http_request.

**`meerkat-mobkit/src/memory/taint.rs:384-419`**

```text
self.config.classify_tool(name)
```

The asynchronous ToolResultReceived/ToolExecutionCompleted branches remain name-only. Provider-native ServerToolContent is unconditionally marked in this block and the synchronous method at 425-434.

**Final review: pass.** The final memory guide names the dispatch-time ToolDef provenance join, unqualified MCP attribution through source_id, the still-name-based observer, all four always-untrusted names, explicit rule ordering, MCP allowlist, absent/non-MCP qualified-name fallback, trusted default, and provider-native content treatment. Each clause matches executable branches; it does not claim all observed events carry provenance.

**`docs/concepts/memory.mdx:188`**

```text
The asynchronous observe-stream fallback remains name-based. Classification precedence is: always-untrusted builtin names (`web_search`, `web_fetch`, `fetch`, `http_request`), `untrusted_tools`, `trusted_tools`, typed MCP server attribution against `trusted_mcp_servers`, qualified `mcp__<server>__<tool>` fallback when provenance is absent or non-MCP, then trusted.
```

The corrected prose preserves the exact precedence and fallback qualifications.

**`meerkat-mobkit/src/memory/taint.rs:198-220`**

```text
if let Some(provenance) = provenance
            && provenance.kind == ToolSourceKind::Mcp
        {
            return self.classify_mcp_server(provenance.source_id.as_str(), name);
        }
```

Typed MCP attribution is active after builtin/untrusted/trusted tool rules, followed by qualified-name fallback and trusted default.

**`meerkat-mobkit/src/memory/dispatch_taint.rs:223-230`**

```text
let provenance = tools
                            .iter()
                            .find(|tool| tool.name.as_ref() == *name)
                            .and_then(|tool| tool.provenance.as_ref());
                        tracker.observe_dispatched_tool_result(&self.identity, name, provenance);
```

Recognized results really pass catalog provenance into the tracker.

**`meerkat-mobkit/src/memory/taint.rs:384-397`**

```text
if let ToolContentTrust::Untrusted { source } = self.config.classify_tool(name)
```

The observer uses the name-only classifier; the immediately following ServerToolContent arm unconditionally marks taint. The builtin list at line 83 also matches the guide.

## K-004: The memory guide presents already-shipped dispatch-time taint ordering as future work

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/memory.mdx:186`**

```text
Choose `"quarantined"` if you cannot accept the known race: a write issued in the same turn as the session's *first* untrusted ingestion can beat the observe-stream taint signal (closed in a later phase by dispatch-time taint visibility).
```

Users choosing memory write policy are told a shipped ordering fix is absent, and are directed to quarantine every write to compensate for an observer-only limitation that does not describe the stock composed path.

**`meerkat-mobkit/src/memory/dispatch_taint.rs:261-275`**

```text
        if let Some(tracker) = self.slot.tracker() {
            self.mark_request_ingestions(&tracker, messages, tools);
        }
        let result = self
            .inner
            .stream_response(messages, tools, max_tokens, temperature, provider_params)
            .await?;
        if let Some(tracker) = self.slot.tracker() {
            for block in result.blocks() {
                if let AssistantBlock::ServerToolContent { kind, .. } = block {
                    tracker.observe_dispatched_server_tool(&self.identity, kind);
                }
            }
        }
        Ok(result)
```

The wrapper marks classified input ingestions before invoking the next model call, and typed provider-native server-tool content before returning the response. The relevant ordering does not wait for the asynchronous observer.

**`meerkat-mobkit/src/mob_handle_runtime.rs:722-726`**

```text
        // After the user hook (composes over any decorator it set) and after
        // sanitize (which only touches the raw llm_client_override).
        if let Some(slot) = self.dispatch_taint.as_ref() {
            crate::memory::dispatch_taint::attach_member_taint_decorator(&mut req, slot);
        }
```

The member build path installs the decorator rather than leaving the class unused.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12904-12908`**

```text
                    // §10.1 dispatch-time taint join: bind the tracker into
                    // the member pre-build seam so every member's LLM client
                    // marks untrusted ingestion synchronously - ahead of the
                    // async observer spawned below (first-ingestion race).
                    dispatch_taint_slot.fill(tracker.clone());
```

The SDK gateway fills the live tracker slot for the composed memory stack.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1471-1475`**

```text
            // §10.1 dispatch-time taint join: bind the stack's tracker into
            // the member pre-build seam so every member's LLM client marks
            // untrusted ingestion synchronously - ahead of the async
            // observer spawned below, closing the first-ingestion race.
            dispatch_taint_slot.fill(stack.taint.clone());
```

The library's full-stack composition supplies the same tracker; the implementation remains conditional on a filled slot, so an arbitrary custom composition must not be given a universal guarantee.

### Independent adjudication

The alleged future dispatch-order fix is active code at the current baseline. stream_response marks recognized request ingestions before awaiting the model and typed server-tool response content before returning it. I independently followed the member-build decorator, the gateway/full-stack tracker fills, and the write gate sharing the same tracker, rather than relying on module comments or the old patch. Narrow the result: this is a filled-slot, decorated-member guarantee for classified tool results and typed provider-native blocks, not universal prompt-injection safety, all possible content carriers, or arbitrary custom providers. The classic UnifiedRuntime provider path can install a trackerless gate and is not equivalent to the full-stack path. No A-J finding owns this current memory-guide ordering warning; I-002 is a historical attribution correction.

**`docs/concepts/memory.mdx:186`**

```text
Choose `"quarantined"` if you cannot accept the known race: a write issued in the same turn as the session's *first* untrusted ingestion can beat the observe-stream taint signal (closed in a later phase by dispatch-time taint visibility).
```

The guide tells current users to compensate for an observer-only limitation without acknowledging the shipped join.

**`meerkat-mobkit/src/memory/dispatch_taint.rs:261-275`**

```text
        if let Some(tracker) = self.slot.tracker() {
            self.mark_request_ingestions(&tracker, messages, tools);
        }
        let result = self
            .inner
            .stream_response(messages, tools, max_tokens, temperature, provider_params)
            .await?;
        if let Some(tracker) = self.slot.tracker() {
            for block in result.blocks() {
                if let AssistantBlock::ServerToolContent { kind, .. } = block {
                    tracker.observe_dispatched_server_tool(&self.identity, kind);
                }
            }
        }
        Ok(result)
```

The marks happen synchronously on the model-call path, before the relevant derived or same-response tool calls can be dispatched.

**`meerkat-mobkit/src/mob_handle_runtime.rs:6611-6615`**

```text
        let dispatch_taint_slot = crate::memory::dispatch_taint::DispatchTaintSlot::default();
        let session_service = Arc::new(PreBuildMobSessionService {
            inner: session_service,
            hook: no_op_pre_build_hook(),
            dispatch_taint: Some(dispatch_taint_slot.clone()),
```

RealMobRuntimeSpec construction installs the shared late-bound slot in the session-build adapter.

**`meerkat-mobkit/src/mob_handle_runtime.rs:724-726`**

```text
        if let Some(slot) = self.dispatch_taint.as_ref() {
            crate::memory::dispatch_taint::attach_member_taint_decorator(&mut req, slot);
        }
```

The build adapter actually attaches the decorator to member session requests.

**`meerkat-mobkit/src/memory_wiring.rs:184-188`**

```text
    let taint = SessionTaintTracker::new(config.content_trust.clone());
    taintable.set_llm_write_gate(Arc::new(TaintLlmWriteGate::new(
        Some(taint.clone()),
        config.llm_writes,
    )));
```

The composed memory store's write gate receives this tracker; marking is relevant to actual writes.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12908-12915`**

```text
                    dispatch_taint_slot.fill(tracker.clone());
                    // Observe-stream feed lives for the gateway process;
                    // forgetting the guard keeps the task running.
                    std::mem::forget(meerkat_mobkit::spawn_member_event_observer(
                        runtime.mob_handle(),
                        sinks,
                    ));
                    agent_memory_taint = Some(tracker);
```

The rpc_gateway agent-memory stack fills the slot and retains the asynchronous fallback rather than using that fallback alone.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1415-1419`**

```text
        // Full-stack path (persistent_agent_memory_stack): firewall + engines
        // + observer over the pre-opened stack provider.
        if let (Some(provider), Some(engines)) =
            (stack_provider, self.agent_memory_engines.as_ref())
        {
```

The library guarantee is conditional on the full-stack composition.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1475`**

```text
            dispatch_taint_slot.fill(stack.taint.clone());
```

The full-stack builder fills the same kind of slot using the attached store gate's tracker.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1405-1407`**

```text
                taintable.set_llm_write_gate_if_absent(Arc::new(
                    crate::memory::taint::TaintLlmWriteGate::new(None, llm_writes),
                ));
```

The separate classic-provider path has no tracker in this default gate; it must not inherit an unconditional full-stack assurance.

**`meerkat-mobkit/src/memory/taint.rs:792-797`**

```text
        let tracker = self.tracker.as_ref()?;
        if let MemoryAuthor::Agent { identity } = author
            && let Some(state) = tracker.identity_taint(identity)
        {
            return Some(format!("session tainted by {}", state.source));
        }
```

The gate quarantines agent writes based on the marked identity; a missing tracker cannot provide observed-taint enforcement.

**Required correction:** Retain the observed/quarantined policy distinction, but remove 'closed in a later phase'. Explain that the bundled rpc_gateway with its SQLite agent-memory stack and UnifiedRuntime's full-stack persistent_agent_memory_stack composition fill DispatchTaintSlot and decorate member LLM clients. Recognized untrusted tool results are marked before the LLM request consuming them, and typed provider-native server-tool blocks are marked before the response returns for tool dispatch; these paths do not wait for the asynchronous observer before a derived LLM memory write. Keep the observer as fallback. Explicitly qualify that an unfilled slot is pass-through, custom/classic-provider compositions need equivalent wiring, and this ordering is not a guarantee for unclassified content or general prompt-injection immunity. Describe quarantined as the stricter policy choice, not a workaround for a still-unshipped stock ordering fix.

### Changes and final verification

**Changed:** `docs/concepts/memory.mdx`.

Replaced the future-work first-ingestion warning with the shipped, scoped filled-slot/decorated-member behavior of rpc_gateway's SQLite stack and UnifiedRuntime's persistent_agent_memory_stack. Explained request-ingestion and provider-native-response marking order, observer fallback, shared tracker-backed gate, and unfilled-slot/custom/classic-provider limitations. Kept quarantined as the stricter policy, not an unshipped-fix workaround.

**Validation:** Read stream_response ordering and both stock tracker-fill sites; verified member decorator, shared write-gate, and trackerless classic-provider evidence. Assertions confirm no general prompt-injection or unclassified-carrier guarantee.

**Final review: pass.** The page now describes the shipped dispatch-order join only for the filled-slot/decorated-member stock compositions and classified carriers. It retains observed versus stricter quarantined posture without suggesting quarantine is a workaround for an unshipped fix. Unfilled-slot, custom/classic-provider, observer-fallback, and non-universal-safety caveats are all explicit.

**`docs/concepts/memory.mdx:192-205`**

```text
This is a composed-path ordering guarantee, not general prompt-injection
immunity or a guarantee for unclassified content. An unfilled slot is
pass-through; custom or classic-provider compositions need equivalent tracker,
decorator, and write-gate wiring.
```

The preceding paragraph names rpc_gateway's SQLite stack and persistent_agent_memory_stack, request-before-call marking, response-before-dispatch marking, and the shared gate.

**`meerkat-mobkit/src/memory/dispatch_taint.rs:261-276`**

```text
self.mark_request_ingestions(&tracker, messages, tools);
```

This runs before inner.stream_response; after await, ServerToolContent is marked before Ok(result). Both are conditional on the slot's tracker. The member pre-build path actually installs the decorator at mob_handle_runtime.rs:724-725.

**`meerkat-mobkit/src/memory_wiring.rs:184-188`**

```text
taintable.set_llm_write_gate(Arc::new(TaintLlmWriteGate::new(
        Some(taint.clone()),
        config.llm_writes,
    )));
```

The composed store's write gate uses the same tracker, so dispatch marking reaches enforcement rather than only logging.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12904-12915`**

```text
dispatch_taint_slot.fill(tracker.clone());
```

The stock gateway fills the slot and separately retains spawn_member_event_observer as fallback.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1471-1475`**

```text
dispatch_taint_slot.fill(stack.taint.clone());
```

The library full-stack branch installs the same mechanism. The distinct classic-provider branch at 1405-1407 can install TaintLlmWriteGate::new(None, llm_writes), proving why the page's composition caveat matters.

**Final review: pass.** The obsolete future-fix warning is gone. The guarantee is expressly limited to recognized classified content, decorated members and a shared tracker-backed gate in rpc_gateway's SQLite stack or UnifiedRuntime's full-stack composition. Both tracker fills, member decorator installation and the actual call ordering were checked. The custom/classic-provider and unfilled-slot exclusions remain explicit, as does the distinction from general prompt-injection immunity.

**`docs/concepts/memory.mdx:192-205`**

```text
This is a composed-path ordering guarantee, not general prompt-injection
immunity or a guarantee for unclassified content. An unfilled slot is
pass-through; custom or classic-provider compositions need equivalent tracker,
decorator, and write-gate wiring.
```

The corrected current-status claim retains the adjudicated scope limitations.

**`meerkat-mobkit/src/memory/dispatch_taint.rs:261-275`**

```text
if let Some(tracker) = self.slot.tracker() {
            self.mark_request_ingestions(&tracker, messages, tools);
        }
```

This precedes inner.stream_response; the same function marks ServerToolContent before returning the result.

**`meerkat-mobkit/src/mob_handle_runtime.rs:722-726`**

```text
crate::memory::dispatch_taint::attach_member_taint_decorator(&mut req, slot);
```

The member build path actually installs the decorator.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12904-12915`**

```text
dispatch_taint_slot.fill(tracker.clone());
```

The stock gateway fills the slot and retains the observer as fallback.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1471-1482`**

```text
dispatch_taint_slot.fill(stack.taint.clone());
```

The full-stack library composition uses its stack tracker. The separate classic-provider branch at 1398-1407 installs a trackerless gate, validating the explicit exclusion.

**`meerkat-mobkit/src/memory_wiring.rs:184-188`**

```text
taintable.set_llm_write_gate(Arc::new(TaintLlmWriteGate::new(
        Some(taint.clone()),
        config.llm_writes,
    )));
```

The taint mark and actual LLM write gate share the same tracker.

## K-005: RosterContext.previous_identities is not a complete current-membership snapshot on every callback

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/roster.mdx:457`**

```text
| `previous_identities` | Identities the identity runtime has registered at the time of the call. Empty on the bootstrap resolves (nothing is registered yet), populated on later re-derivations such as `mobkit/topology/query` or an edge reconcile. |
```

A roster provider may interpret an empty list during refresh or reset as proof of an empty runtime and make incorrect allocation or roster decisions. Fixing the separate nested-envelope defect does not fix this field's documented meaning.

**`meerkat-mobkit/src/identity_first/runtime.rs:1047-1056`**

```text
        let previous_identities = self.runtime.registered_identities().await;
        let roster = self
            .roster_provider
            .roster(&RosterContext {
                mob_definition: self.mob_definition.clone(),
                previous_identities,
            })
```

Topology snapshots genuinely provide the registered identities, explaining why the examples in the table can appear correct while the unqualified definition is not.

**`meerkat-mobkit/src/identity_first/runtime.rs:1265-1285`**

```text
        let roster = match self
            .roster_provider
            .roster(&RosterContext {
                mob_definition: self.mob_definition.clone(),
                previous_identities: Vec::new(),
            })
```

refresh_desired_topology passes an empty vector on a later full roster refresh, without constructing a snapshot from registered identities.

**`meerkat-mobkit/src/identity_first/runtime.rs:6663-6673`**

```text
        match roster_provider
            .roster(&RosterContext {
                mob_definition,
                previous_identities: Vec::new(),
            })
```

Reset-time adopt_roster_spec_with_context also passes an empty vector while operating on an existing identity.

### Independent adjudication

The table correctly describes the topology-snapshot call site but incorrectly promotes that behavior to every roster callback. refresh_desired_topology sends Vec::new() before applying a new roster even on an established runtime, and reset-time spec adoption likewise sends an empty vector for an existing identity. The gateway serializes exactly the caller-supplied context rather than replacing its previous_identities field, so this difference reaches SDK providers. B-001/E-001 repair the callback envelope and typed-context access; neither changes or qualifies the current table's independent promise of a complete registered-identity snapshot. Retain K-005 separately while coordinating its wording with those fixes.

**`docs/concepts/roster.mdx:457`**

```text
| `previous_identities` | Identities the identity runtime has registered at the time of the call. Empty on the bootstrap resolves (nothing is registered yet), populated on later re-derivations such as `mobkit/topology/query` or an edge reconcile. |
```

The universal field meaning is contradicted by later non-snapshot callback sites.

**`meerkat-mobkit/src/identity_first/runtime.rs:1050-1056`**

```text
        let previous_identities = self.runtime.registered_identities().await;
        let roster = self
            .roster_provider
            .roster(&RosterContext {
                mob_definition: self.mob_definition.clone(),
                previous_identities,
            })
```

topology_snapshot_inputs intentionally provides the registered identity list.

**`meerkat-mobkit/src/topology_control.rs:1283-1287`**

```text
    async fn reconcile_identity_first_with_controller(
        controller: &TopologyController,
        context: &crate::identity_first::IdentityFirstRuntimeContext,
    ) -> UnifiedRuntimeReconcileEdgesReport {
        let (_, provider_edges) = match context.topology_snapshot_inputs().await {
```

The edge-reconciliation case uses the snapshot-producing call site.

**`meerkat-mobkit/src/topology_control.rs:1500-1507`**

```text
    async fn query_identity_first(
        &self,
        context: &crate::identity_first::IdentityFirstRuntimeContext,
    ) -> Result<TopologySnapshot, TopologyControlError> {
        let authority = self.authority();
        let (roster, provider_edges) = context
            .topology_snapshot_inputs()
            .await
```

The topology-query case likewise supplies a snapshot, without making it a universal roster-provider contract.

**`meerkat-mobkit/src/identity_first/runtime.rs:1280-1285`**

```text
        let roster = match self
            .roster_provider
            .roster(&RosterContext {
                mob_definition: self.mob_definition.clone(),
                previous_identities: Vec::new(),
            })
```

The full refresh callback uses an empty list regardless of current registration.

**`meerkat-mobkit/src/identity_first/runtime.rs:6663-6674`**

```text
    async fn adopt_roster_spec_with_context(
        &self,
        roster_provider: &Arc<dyn RosterProvider>,
        identity: &AgentIdentity,
        mob_definition: Option<meerkat_mob::MobDefinition>,
    ) {
        match roster_provider
            .roster(&RosterContext {
                mob_definition,
                previous_identities: Vec::new(),
            })
            .await
```

The reset-time adoption helper also omits the registered-identity snapshot.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1297-1301`**

```text
            let roster_specs = roster_provider
                .roster(&RosterContext {
                    mob_definition: Some(runtime.mob_runtime.handle().definition().clone()),
                    previous_identities: Vec::new(),
                })
```

Bootstrap is another empty-list caller, but is not the only one.

**`meerkat-mobkit/src/identity_first/gateway_bridges.rs:392-398`**

```text
    async fn roster(&self, context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        let context = serde_json::to_value(context)
            .map_err(|e| RosterError::Io(format!("serialize roster context: {e}")))?;
        let params = json!({ "context": context });
        let result = self
            .bridge
            .call("callback/roster_provider/roster", params)
```

The callback transport forwards these differing values unchanged inside the correct nested envelope.

**Required correction:** Replace the previous_identities table row with: 'Call-site-dependent prior-identity context, not an authoritative membership snapshot on every callback. Topology-query and edge-reconciliation snapshots supply currently registered identities. Bootstrap, full roster refresh/reconcile, and reset-time spec adoption currently pass an empty list; an empty list does not establish that no identities are registered.' Include the same qualification or a direct link to it in docs/api/rpc.mdx's callback explanation and any typed-context wording restored in docs/concepts/roster.mdx or docs/quickstart.mdx under E-001/B-001. Preserve the separate envelope/type fixes; do not change runtime callback behavior.

### Changes and final verification

**Changed:** `docs/quickstart.mdx`.

Qualified the restored typed roster context: topology-query/edge-reconciliation snapshots supply registered identities, while bootstrap/full refresh/reconcile/reset-time adoption currently pass empty lists. Explicitly prohibited treating an empty list as proof of zero registered identities.

**Validation:** Read identity_first/runtime.rs:1038-1070,1270-1295,6660-6680, confirming the registered-identity snapshot versus Vec::new call sites. Python dispatcher validation confirms context is forwarded rather than synthesized. C/E-owned explanations remain their owners' assignments.

**Changed:** `docs/api/rpc.mdx`.

Restored the actual params.context callback envelope and typed Python/TypeScript context field names, with optional custom-host definition and call-site-dependent prior identity context. Explicitly stated that an empty bootstrap/full-refresh/reset list does not prove empty membership.

**Validation:** Checked GatewayRosterProvider serialization, both SDK context models, topology_snapshot_inputs, refresh_desired_topology and reset-time adoption. Correction is limited to C's owned RPC occurrence; B/E own their guide occurrences.

**Changed:** `docs/concepts/roster.mdx`.

Qualified previous_identities in the field table and restored typed-context explanation. Topology-query/edge snapshots supply registered identities, whereas bootstrap, full roster refresh/reconcile, and reset-time adoption pass an empty list; empty is not proof of an empty membership set.

**Validation:** Read identity_first/runtime.rs:1047-1062,1265-1288,6663-6674 and gateway_bridges.rs:391-401. Exact evidence checks included topology query/reconcile and builder bootstrap call sites. Other document owners and SDK source-docstring follow-up are recorded in caveats.

**Changed:** `sdk/python/meerkat_mobkit/identity_first_models.py`, `sdk/typescript/src/types.ts`.

Aligned Python docstring and TypeScript API comment with the confirmed call-site-dependent previous-identities contract.

**Validation:** Comment/docstring-only changes; matches adjudication-K and roster/API/quickstart corrections. Final SDK and K reviewers must check these additional documentary surfaces.

**Changed:** `docs/sdks/python.mdx`, `docs/sdks/typescript.mdx`.

Updated the Python previous_identities table row and TypeScript RosterContext reference comment to the call-site-dependent contract. Both now distinguish registered-identity snapshots for topology queries/edge reconciliation from empty bootstrap/full roster refresh/reconcile/reset-time adoption inputs, and explicitly state that empty does not imply no registered identities. Preserved the typed callback envelope and optional mob-definition semantics.

**Validation:** Read meerkat-mobkit/src/identity_first/runtime.rs:1047-1060,1265-1296,6663-6675, src/topology_control.rs:1283-1288,1500-1508, src/bin/rpc_gateway.rs:11533-11536,13036-13043, src/unified_runtime/builder.rs:1297-1303 and src/identity_first/gateway_bridges.rs:392-403. Compared the already-correct source documentation at sdk/python/meerkat_mobkit/identity_first_models.py:665-669 and sdk/typescript/src/types.ts:2652-2656 without modifying it. Source/document assertions and both MDX compilations passed.

**Final review: pass.** The B-owned quickstart retains the correct call-site-dependent qualification. On residual re-review, additionally checked both SDK reference pages: Python's previous_identities row and TypeScript's RosterContext comment now explicitly separate registered topology/edge snapshots from empty bootstrap/full-refresh/reset calls, with empty not implying no registered identities. RPC, roster guide and both SDK source comments remain consistent. No callback behavior changed.

**`docs/quickstart.mdx:208-212`**

```text
The prior-identity list is call-site-dependent, not an authoritative membership
snapshot on every callback.
```

The B-owned restored typed-context prose carries the qualification.

**`meerkat-mobkit/src/identity_first/runtime.rs:1050-1056`**

```text
let previous_identities = self.runtime.registered_identities().await;
```

Topology snapshots supply registered identities; topology query and edge reconciliation use this helper.

**`meerkat-mobkit/src/identity_first/runtime.rs:1280-1285`**

```text
previous_identities: Vec::new(),
```

Full refresh does not supply a membership snapshot.

**`meerkat-mobkit/src/identity_first/runtime.rs:6669-6673`**

```text
previous_identities: Vec::new(),
```

Reset-time roster adoption is also empty.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1297-1301`**

```text
previous_identities: Vec::new(),
```

Bootstrap is empty as described.

**`docs/api/rpc.mdx:330-334`**

```text
An empty list does
not establish that no identities are registered.
```

The C-owned explanation preserves the same caveat.

**`docs/concepts/roster.mdx:459-459`**

```text
Call-site-dependent prior-identity context, not an authoritative membership snapshot on every callback.
```

The E-owned table is corrected.

**`sdk/python/meerkat_mobkit/identity_first_models.py:665-669`**

```text
currently pass an empty list, even if identities are already registered.
```

The coordination docstring patch agrees.

**`sdk/typescript/src/types.ts:2652-2656`**

```text
empty list, even if identities are already registered.
```

The TypeScript API comment agrees.

**`docs/sdks/python.mdx:231-231`**

```text
an empty list does not imply that no identities are registered.
```

The newly corrected Python reference row now carries the missing qualification.

**`docs/sdks/typescript.mdx:146-153`**

```text
and reset-time spec adoption currently pass an empty list; an empty list
   * does not imply that no identities are registered.
```

The newly corrected TypeScript reference interface comment agrees.

**Final review: pass.** The C-owned callback explanation now has the real params.context envelope, typed SDK naming, optional custom-host definition and call-site-dependent prior identities. It explicitly forbids treating an empty refresh/reset list as proof of empty membership.

**`docs/api/rpc.mdx:324-337`**

```text
`previous_identities` is call-site-dependent, not an authoritative membership
```

The text distinguishes topology/edge snapshots from bootstrap/full refresh/reset adoption and names both SDK field styles.

**`meerkat-mobkit/src/identity_first/gateway_bridges.rs:392-403`**

```text
let params = json!({ "context": context });
```

The bridge serializes context unchanged. SDK decoding was independently read at agent_builder.py:753-758 and agent-builder.ts:517-525.

**`meerkat-mobkit/src/identity_first/runtime.rs:1280-1285`**

```text
previous_identities: Vec::new(),
```

Full refresh passes empty; reset adoption does likewise at 6669-6673. The topology snapshot at 1050-1056 instead reads registered_identities, and topology_control.rs calls it for query/reconcile.

**Final review: pass.** Final PASS now covers both source docstring/comment corrections AND both SDK reference pages. The Python table and TypeScript RosterContext snippet now distinguish snapshot-bearing topology/edge callbacks from empty bootstrap/full-refresh/reset inputs and explicitly forbid inferring an empty runtime from an empty list. Independently re-followed all five call-site categories and the unchanged bridge forwarding behavior. The existing typed callback envelope and optional mob-definition qualifications are preserved. The original source-only pass and missed-page regression R-D-001 are retained in review_history with their resolution.

**`docs/sdks/python.mdx:231-231`**

```text
Call-site-dependent prior-identity context, not an authoritative membership snapshot on every callback. Topology queries and edge reconciliation supply currently registered identities. Bootstrap, full roster refresh/reconcile, and reset-time spec adoption currently pass an empty list; an empty list does not imply that no identities are registered.
```

The Python SDK reference no longer promises a universal current-membership snapshot.

**`docs/sdks/typescript.mdx:147-151`**

```text
   * Call-site-dependent prior-identity context, not an authoritative membership
   * snapshot on every callback. Topology queries and edge reconciliation supply
   * currently registered identities. Bootstrap, full roster refresh/reconcile,
   * and reset-time spec adoption currently pass an empty list; an empty list
   * does not imply that no identities are registered.
```

The TypeScript copied interface comment now carries the same complete qualification as its source.

**`sdk/python/meerkat_mobkit/identity_first_models.py:665-669`**

```text
    ``previous_identities`` is call-site-dependent prior-identity context,
    not an authoritative membership snapshot on every callback. Topology
    queries and edge reconciliation supply currently registered identities.
    Bootstrap, full roster refresh/reconcile, and reset-time spec adoption
    currently pass an empty list, even if identities are already registered.
```

The Python source docstring now distinguishes all adjudicated call sites.

**`sdk/typescript/src/types.ts:2652-2656`**

```text
 * `previousIdentities` is call-site-dependent prior-identity context, not an
 * authoritative membership snapshot on every callback. Topology queries and
 * edge reconciliation supply currently registered identities. Bootstrap, full
 * roster refresh/reconcile, and reset-time spec adoption currently pass an
 * empty list, even if identities are already registered.
```

The TypeScript public API comment expresses the same contract without modifying the interface or parser.

**`meerkat-mobkit/src/identity_first/runtime.rs:1050-1056`**

```text
        let previous_identities = self.runtime.registered_identities().await;
```

Topology snapshot callbacks get current registered identities; topology_control.rs:1283-1287 and 1500-1507 route edge/query calls here.

**`meerkat-mobkit/src/identity_first/runtime.rs:1280-1285`**

```text
                previous_identities: Vec::new(),
```

Full refresh does not obtain a current-membership snapshot, even on an established runtime.

**`meerkat-mobkit/src/identity_first/runtime.rs:6663-6674`**

```text
                previous_identities: Vec::new(),
```

Reset-time adoption also passes an empty list for an existing identity.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1297-1301`**

```text
                    previous_identities: Vec::new(),
```

Bootstrap is another empty-list caller, but is not the only one.

**Final review: pass.** The E-owned roster table and restored typed-context explanation now make previous identities call-site-dependent. Snapshot callers are distinguished from bootstrap, full refresh/reconcile, and reset-time adoption, and empty is explicitly not proof of no registered identities. The coordinator's Python docstring and TypeScript API comment agree and were independently verified to be documentation-only changes. This verdict covers E's assigned portion plus those coordinated repeats, not uninspected B/C-owned text.

**`docs/concepts/roster.mdx:459-466`**

```text
Bootstrap, full roster refresh/reconcile, and reset-time spec adoption currently pass an empty list; an empty list does not establish that no identities are registered.
```

The table precisely corrects the universal-snapshot claim and the typed SDK paragraph links the same qualification.

**`meerkat-mobkit/src/identity_first/runtime.rs:1047-1056`**

```text
let previous_identities = self.runtime.registered_identities().await;
```

Topology query and edge reconciliation call topology_snapshot_inputs, which actually supplies a snapshot.

**`meerkat-mobkit/src/identity_first/runtime.rs:1280-1285`**

```text
previous_identities: Vec::new(),
```

Full refresh uses an empty vector irrespective of already registered identities; reset-time adoption at 6663-6674 does the same.

**`sdk/python/meerkat_mobkit/identity_first_models.py:665-669`**

```text
not an authoritative membership snapshot on every callback. Topology
    queries and edge reconciliation supply currently registered identities.
```

The coordinated SDK docstring no longer repeats the misleading universal guarantee.

**`sdk/typescript/src/types.ts:2652-2656`**

```text
empty list, even if identities are already registered.
```

The corresponding TypeScript comment preserves the same call-site limitation.

**Final review: pass.** Independent residual re-review confirms that both public SDK reference mirrors now match the roster guide, RPC guide, quickstart and both source-documentation comments. All seven surfaces distinguish registered-identity snapshots for topology query/edge reconciliation from empty bootstrap/full-refresh/reconcile/reset-time contexts, and explicitly reject empty-as-no-membership inference. Re-read the current topology, refresh, reset, bootstrap and gateway serialization implementation; no runtime change was used to make the documentation true.

**`docs/sdks/python.mdx:231`**

```text
Call-site-dependent prior-identity context, not an authoritative membership snapshot on every callback. Topology queries and edge reconciliation supply currently registered identities. Bootstrap, full roster refresh/reconcile, and reset-time spec adoption currently pass an empty list; an empty list does not imply that no identities are registered.
```

The Python reference now preserves all call-site qualifications and the empty-list warning.

**`docs/sdks/typescript.mdx:147-151`**

```text
Call-site-dependent prior-identity context, not an authoritative membership
   * snapshot on every callback. Topology queries and edge reconciliation supply
   * currently registered identities. Bootstrap, full roster refresh/reconcile,
   * and reset-time spec adoption currently pass an empty list; an empty list
   * does not imply that no identities are registered.
```

The copied TypeScript interface now matches the actual source comment and the current runtime.

**`docs/concepts/roster.mdx:459`**

```text
Bootstrap, full roster refresh/reconcile, and reset-time spec adoption currently pass an empty list; an empty list does not establish that no identities are registered.
```

The primary field table is correctly fixed, and the early typed-context introduction links here.

**`docs/api/rpc.mdx:330-334`**

```text
`previous_identities` is call-site-dependent, not an authoritative membership
snapshot on every callback:
```

The RPC callback explanation correctly qualifies the restored envelope/type wording.

**`docs/quickstart.mdx:208-212`**

```text
reset-time spec adoption currently pass an empty list. An empty list does not
establish that no identities are registered.
```

The quickstart correction includes the reset/full-refresh limitation.

**`sdk/python/meerkat_mobkit/identity_first_models.py:665-669`**

```text
Bootstrap, full roster refresh/reconcile, and reset-time spec adoption
    currently pass an empty list, even if identities are already registered.
```

The specifically requested coordination docstring correction is present.

**`sdk/typescript/src/types.ts:2652-2656`**

```text
roster refresh/reconcile, and reset-time spec adoption currently pass an
 * empty list, even if identities are already registered.
```

The specifically requested TypeScript source-comment correction is also present.

**`meerkat-mobkit/src/identity_first/runtime.rs:1050-1056`**

```text
let previous_identities = self.runtime.registered_identities().await;
```

topology_snapshot_inputs supplies the registered list; topology_control.rs:1287 and 1505-1507 connect edge reconciliation and topology query to it.

**`meerkat-mobkit/src/identity_first/runtime.rs:1280-1285`**

```text
previous_identities: Vec::new(),
```

Full roster refresh omits that snapshot even on an established runtime.

**`meerkat-mobkit/src/identity_first/runtime.rs:6663-6674`**

```text
previous_identities: Vec::new(),
```

Reset-time spec adoption also passes an empty list. Bootstrap does the same at unified_runtime/builder.rs:1297-1301.

**`meerkat-mobkit/src/identity_first/gateway_bridges.rs:392-399`**

```text
let params = json!({ "context": context });
```

The gateway serializes the caller-supplied context unchanged, so the difference reaches SDK callbacks. Python and TypeScript dispatchers decode this envelope at agent_builder.py:753-758 and agent-builder.ts:517-525.

## K-006: The console module-list endpoint is mislabeled as a loaded-runtime inventory

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/console.mdx:46`**

```text
| `GET /console/modules` | Loaded module summary. |
```

An operator can mistake configured IDs for evidence that modules successfully started or remain loaded, using a configuration inventory as a readiness check.

**`meerkat-mobkit/src/runtime/console_ingress.rs:277-281`**

```text
    let modules: Vec<String> = decisions
        .modules
        .iter()
        .map(|module| module.id.clone())
        .collect();
```

The returned IDs come from the configured decision state, not live process startup, loaded-runtime membership or health.

**`meerkat-mobkit/src/runtime/console_ingress.rs:291-299`**

```text
    let mut body = if base_path == CONSOLE_EXPERIENCE_ROUTE {
        build_console_experience_contract(&modules, &live_snapshot, &decisions.console)
    } else {
        serde_json::json!({
            "contract_version": MOBKIT_CONTRACT_VERSION,
            "modules": modules
        })
    };
```

The non-experience module response uses that configured list directly, with no health/startup filtering.

**`docs/guides/console.mdx:522`**

```text
| `/console/modules` | GET | Loaded modules list (JSON) |
```

A second occurrence in the same guide requires the same correction, not a separate finding.

**`docs/guides/unified-runtime.mdx:172`**

```text
| `/console/modules` | GET | Loaded modules JSON |
```

The prior commit also corrected this duplicate description of the same endpoint.

### Independent adjudication

The endpoint directly returns IDs from RuntimeDecisionState.modules and does not consult live_snapshot for that response. I also checked that actual loaded_modules is a separate runtime set updated only on successful module-start events, so 'loaded' is not merely a harmless alternate name for this list. F-004/F-005 mention /console/modules only as context for unrelated URL/dev-proxy problems; E-017 concerns bootstrap ordering. None corrects these three route descriptions. Treat the repeated endpoint claim as one defect.

**`docs/guides/console.mdx:46`**

```text
| `GET /console/modules` | Loaded module summary. |
```

The first guide table assigns runtime-loading semantics to the configured inventory.

**`docs/guides/console.mdx:522`**

```text
| `/console/modules` | GET | Loaded modules list (JSON) |
```

The route table repeats the same incorrect claim.

**`docs/guides/unified-runtime.mdx:172`**

```text
| `/console/modules` | GET | Loaded modules JSON |
```

The unified-runtime guide is a third occurrence requiring consistent wording.

**`meerkat-mobkit/src/runtime/console_ingress.rs:277-281`**

```text
    let modules: Vec<String> = decisions
        .modules
        .iter()
        .map(|module| module.id.clone())
        .collect();
```

The IDs are derived from configured decision-state modules.

**`meerkat-mobkit/src/runtime/console_ingress.rs:291-299`**

```text
    let mut body = if base_path == CONSOLE_EXPERIENCE_ROUTE {
        build_console_experience_contract(&modules, &live_snapshot, &decisions.console)
    } else {
        serde_json::json!({
            "contract_version": MOBKIT_CONTRACT_VERSION,
            "modules": modules
        })
    };
```

The /console/modules response does not filter or annotate those IDs using live state.

**`meerkat-mobkit/src/runtime/bootstrap.rs:72-78`**

```text
        if let Some(event) = start_result.event {
            loaded_modules.insert(module_id.clone());
            if let Some(child) = start_result.child {
                live_children.insert(module_id.clone(), child);
            }
            module_events.push(event);
        }
```

Actual successful-load tracking is a separate runtime fact, not the source of the endpoint's IDs.

**Required correction:** In both docs/guides/console.mdx tables and docs/guides/unified-runtime.mdx's route table, replace 'Loaded modules' wording with 'Configured module IDs (JSON)' or 'Configured module-ID summary'. State that these IDs come from RuntimeDecisionState.modules and do not confirm successful startup, current liveness, or health. Keep the response shape and runtime implementation unchanged.

### Changes and final verification

**Changed:** `docs/guides/unified-runtime.mdx`.

Changed /console/modules to configured module IDs and explained that RuntimeDecisionState.modules is configuration inventory, not startup/liveness/health confirmation. Pointed only to loaded_modules/module_health_transitions for separate startup results, not continuous liveness assurance.

**Validation:** Read runtime/console_ingress.rs:277-299 and checked successful-load bookkeeping at runtime/bootstrap.rs:72-78. Corrected route wording and configured-state source assertions passed. Scope F owns the console-guide occurrences.

**Changed:** `docs/guides/console.mdx`.

Corrected both console-guide /console/modules table entries to configured module IDs and explained their RuntimeDecisionState.modules source, explicitly excluding successful startup, current liveness or health confirmation. This entry covers only F-owned occurrences.

**Validation:** Read runtime/console_ingress.rs:277-298. Assertions confirmed both corrected table rows, the source qualification and unchanged configured-ID response implementation.

**Final review: pass.** The E-owned /console/modules route description now says configured IDs and explicitly disclaims startup, liveness, and health confirmation. The separate loaded_modules and module_health_transitions references are correctly presented as startup results, not continuous health checks. This verdict does not substitute for F's review of its console-guide occurrences.

**`docs/guides/unified-runtime.mdx:181-198`**

```text
`GET /console/modules` lists IDs from `RuntimeDecisionState.modules`.
It is a configuration inventory, not confirmation of successful startup,
current liveness, or health.
```

The route table and adjacent qualification consistently describe configuration inventory.

**`meerkat-mobkit/src/runtime/console_ingress.rs:277-299`**

```text
let modules: Vec<String> = decisions
        .modules
        .iter()
        .map(|module| module.id.clone())
        .collect();
```

The response uses decision-state IDs directly rather than the runtime's loaded set or live snapshot.

**`meerkat-mobkit/src/runtime/bootstrap.rs:72-78`**

```text
loaded_modules.insert(module_id.clone());
```

Actual successful-start tracking is a separate fact recorded only when a start result contains an event.

**Final review: pass.** Both F-owned console tables now say configured IDs and explicitly distinguish configuration inventory from startup/liveness/health confirmation. Independently checked the response builder and separate successful-load bookkeeping. Also checked the E-owned unified-runtime route/qualification for consistent closure of the third occurrence, without reviewing unrelated E fixes.

**`docs/guides/console.mdx:55`**

```text
`/console/modules` returns IDs from `RuntimeDecisionState.modules`. It is a configuration inventory, not confirmation of successful startup, current liveness, or health.
```

This qualifies the configured-summary table at line 46; the second table at 532 repeats the non-health semantics.

**`docs/guides/unified-runtime.mdx:195-198`**

```text
`GET /console/modules` lists IDs from `RuntimeDecisionState.modules`.
It is a configuration inventory, not confirmation of successful startup,
current liveness, or health.
```

The shared E-owned occurrence also has the same correction and its route row at 181 says configured IDs.

**`meerkat-mobkit/src/runtime/console_ingress.rs:277-281`**

```text
let modules: Vec<String> = decisions
        .modules
        .iter()
        .map(|module| module.id.clone())
        .collect();
```

The non-experience response at 293-298 uses this list, whereas bootstrap.rs:72-78 separately tracks successful starts in loaded_modules.

**Final review: pass.** Both console-guide tables and the unified-runtime route table now describe configured IDs. Each guide explicitly excludes startup, current liveness and health confirmation and identifies RuntimeDecisionState.modules. The REST reference is consistent too. I independently checked the configured-ID response and separate successful-load bookkeeping.

**`docs/guides/console.mdx:46-55`**

```text
`/console/modules` returns IDs from `RuntimeDecisionState.modules`. It is a configuration inventory, not confirmation of successful startup, current liveness, or health.
```

The first table has configured-module wording and the immediate qualification.

**`docs/guides/console.mdx:532`**

```text
| `/console/modules` | GET | Configured module IDs (JSON), not startup/liveness/health confirmation |
```

The second console table is independently corrected.

**`docs/guides/unified-runtime.mdx:181-198`**

```text
`GET /console/modules` lists IDs from `RuntimeDecisionState.modules`.
It is a configuration inventory, not confirmation of successful startup,
current liveness, or health.
```

The unified guide corrects both its table and explanatory scope.

**`docs/api/rest.mdx:90-93`**

```text
Returns the configured module IDs from `RuntimeDecisionState`. This response
does not contain per-module health or startup state.
```

The corresponding REST contract has no residual loaded-module claim.

**`meerkat-mobkit/src/runtime/console_ingress.rs:277-298`**

```text
let modules: Vec<String> = decisions
        .modules
        .iter()
        .map(|module| module.id.clone())
        .collect();
```

The non-experience response returns this configured vector directly. runtime/bootstrap.rs:72-78 tracks successful loads separately.

## K-007: The Python SDK quickstart sends to agent-1 without creating that member

**Severity:** high. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/sdks/python.mdx:34-39`**

```text
async def main():
    async with await MobKit.builder().mob("config/mob.toml").gateway("/path/to/rpc_gateway").build() as rt:
        handle = rt.mob_handle()

        # Send a message (comms)
        await handle.send("agent-1", "Hello")
```

On a fresh runtime the first advertised message operation fails because agent-1 has never been provisioned, independently of the separately reported event-attribute and SSE-auth problems.

**`sdk/python/meerkat_mobkit/runtime.py:1768-1779`**

```text
    async def ensure_member(
        self, member_id: str, role: str, **kwargs: Any
    ) -> MemberSnapshot:
        """Ensure a mob member exists, spawning it if missing.

        Idempotent — returns the member snapshot whether it was just spawned
        or already existed. Use before ``send()`` when handling first contact
        from an unknown user (e.g. new Slack DM).
```

The SDK exposes a separate provisioning operation precisely because send does not create an unknown member.

**`meerkat-mobkit/src/rpc/mob_methods.rs:489-497`**

```text
            let delivery: Result<String, String> = match &target {
                SendMessageTarget::MobMember => send_message_on_mob_with_mode(
                    &runtime.mob_handle(),
                    &member_id,
                    content.clone(),
                    handling_mode,
                )
                .await
                .map_err(|err| err.to_string()),
```

The ordinary non-identity-roster send arm delegates to direct delivery, not ensure/spawn.

**`meerkat-mobkit/src/mob_handle_runtime.rs:9595-9600`**

```text
    let mid = crate::member_comms_id::mob_member_id(member_id);
    let _receipt = handle
        .member(&mid)
        .await?
        .send(content, handling_mode)
        .await?;
```

Delivery must resolve an already existing member. The quickstart neither supplies a roster nor ensures agent-1. Supplying a definition containing only profiles does not establish that member.

### Independent adjudication

The shown program configures neither a roster provider nor explicit member creation and immediately sends to a fixed member name. SDK send issues mobkit/send_message without provisioning; the server's unknown/no-identity-runtime target path resolves to ordinary mob-member delivery, which requires handle.member() to succeed. The issue is a missing fresh-runtime prerequisite, not a claim that sending to an already restored/provisioned agent always fails. D-002 expressly excludes membership setup and addresses later SSE authentication; D-001 addresses yielded event fields. C-008's ensure_member error codes, A-003's README prerequisites, and E-017's bootstrap order are distinct document claims, not corrections of this Python example.

**`docs/sdks/python.mdx:34-39`**

```text
async def main():
    async with await MobKit.builder().mob("config/mob.toml").gateway("/path/to/rpc_gateway").build() as rt:
        handle = rt.mob_handle()

        # Send a message (comms)
        await handle.send("agent-1", "Hello")
```

The quickstart neither supplies a matching definition nor creates the hard-coded member before delivery.

**`sdk/python/meerkat_mobkit/runtime.py:1904-1906`**

```text
        else:
            raw = await self._runtime._rpc("mobkit/send_message", params)
        return SendMessageResult.from_dict(raw)
```

Plain-text send invokes delivery, with no intervening ensure or spawn.

**`meerkat-mobkit/src/rpc/mob_methods.rs:333-335`**

```text
    let Some(identity_rt) = configured_identity_runtime else {
        return SendMessageTarget::MobMember;
    };
```

A runtime without identity-first provisioning takes the direct member-delivery branch.

**`meerkat-mobkit/src/rpc/mob_methods.rs:490-497`**

```text
                SendMessageTarget::MobMember => send_message_on_mob_with_mode(
                    &runtime.mob_handle(),
                    &member_id,
                    content.clone(),
                    handling_mode,
                )
                .await
                .map_err(|err| err.to_string()),
```

The server delegates that branch to direct delivery, not ensure_member.

**`meerkat-mobkit/src/mob_handle_runtime.rs:9595-9600`**

```text
    let mid = crate::member_comms_id::mob_member_id(member_id);
    let _receipt = handle
        .member(&mid)
        .await?
        .send(content, handling_mode)
        .await?;
```

Failure to resolve an existing member propagates before send.

**`sdk/python/meerkat_mobkit/runtime.py:1768-1779`**

```text
    async def ensure_member(
        self, member_id: str, role: str, **kwargs: Any
    ) -> MemberSnapshot:
        """Ensure a mob member exists, spawning it if missing.

        Idempotent — returns the member snapshot whether it was just spawned
        or already existed. Use before ``send()`` when handling first contact
        from an unknown user (e.g. new Slack DM).

        Args:
            member_id: Agent identity for the member.
            role: Role (profile name from mob.toml) to spawn with.
```

The SDK has a separate supported provisioning operation with the required member/role parameters.

**`meerkat-mobkit/src/rpc/mob_methods.rs:976-979`**

```text
            let handle = runtime.mob_handle();
            let mid = spec.identity.clone();
            let ensure_result = handle.ensure_member(spec).await;
            drop(raw_reservation);
```

The ensure handler actually invokes the upstream provisioning operation.

**`examples/004-mdm-console-pack/config/mob.toml:45-55`**

```text
[profiles.target]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"
skills = ["mdm_protocol", "target_role"]
peer_description = "Remote managed machine with target-side tools."

[profiles.target.tools]
builtins = true
comms = true
memory = true
```

The repository's live example demonstrates the model/profile/runtime-mode/tools TOML syntax for the proposed minimal worker configuration.

**Required correction:** Make the Python quickstart self-contained for a fresh ordinary mob runtime: show config/mob.toml containing [mob] id = "python-quickstart", [profiles.worker] model = "gpt-5.5" and runtime_mode = "turn_driven", and [profiles.worker.tools] comms = true. State the OpenAI credential prerequisite for that model (or substitute a supported model with its matching credentials). Add await handle.ensure_member("agent-1", "worker") immediately before await handle.send("agent-1", "Hello"). Explain that profiles are templates rather than pre-created members. Coordinate with D-001's event.event correction and D-002's explicit loopback-demo console_auth_required(False) correction; membership alone does not fix those separate streaming problems.

### Changes and final verification

**Changed:** `docs/sdks/python.mdx`.

Added a minimal config/mob.toml with mob id, gpt-5.5 worker profile, turn_driven runtime mode, and comms=true. Named OPENAI_API_KEY (or matching credentials for another supported model), fresh-project working-directory and installed-gateway prerequisites, and the profile-template distinction. ensure_member('agent-1','worker') immediately precedes send. Integrated membership, auth, and event-shape fixes into one example.

**Validation:** Parsed the exact TOML with tomllib and executed the exact Python quickstart with real builder/init/ensure/send/SSE code and mocked external boundaries. Asserted role and member identity consistency, ensure-before-send ordering, auth opt-out serialization, and successful typed event printing. No real model or gateway execution is claimed.

**Final review: pass.** The Python quickstart now provides the precise mob/profile/tools TOML, matching model credentials and installed-binary prerequisites, and explicit ensure_member immediately before send. It correctly frames this as a fresh ordinary worker-plane example rather than durable per-user provisioning. Independently parsed the exact TOML and ran the exact Python code through real SDK logic with mocked external boundaries; observed init, ensure_member, then send_message, with matching member/role and the integrated D-001/D-002 corrections.

**`docs/sdks/python.mdx:30-47`**

```text
[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
```

The named worker role is now defined with the intended turn-driven and comms settings.

**`docs/sdks/python.mdx:73-79`**

```text
        await handle.ensure_member("agent-1", "worker")
        await handle.send("agent-1", "Hello")
```

The required creation step precedes delivery in the actual snippet.

**`sdk/python/meerkat_mobkit/runtime.py:1768-1794`**

```text
        raw = await self._runtime._rpc("mobkit/ensure_member", params)
```

The executed public ensure method issues a real provisioning RPC with role and agent_identity.

**`meerkat-mobkit/src/rpc/mob_methods.rs:976-979`**

```text
            let ensure_result = handle.ensure_member(spec).await;
```

The gateway handler actually invokes the native ensure operation; direct send separately requires a preexisting member in mob_handle_runtime.rs:9595-9600.

**Final review: pass.** The fresh-project quickstart supplies a worker profile with supported model, turn_driven mode, comms and credentials, then ensures exactly agent-1/worker immediately before sending to agent-1. The related D-001 event property and D-002 explicit local auth opt-out are preserved. I parsed the actual TOML and Python block, checked matching values and call order, and traced ensure/send to their distinct SDK/server operations. No live provider execution is claimed.

**`docs/sdks/python.mdx:30-48`**

```text
[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
```

The definition, project working directory, installed gateway and OPENAI_API_KEY prerequisites now precede the runnable example.

**`docs/sdks/python.mdx:63-81`**

```text
await handle.ensure_member("agent-1", "worker")
        await handle.send("agent-1", "Hello")
```

Provisioning precedes delivery; the same block sets console_auth_required(False) and prints event.event.

**`sdk/python/meerkat_mobkit/runtime.py:1768-1793`**

```text
params: dict[str, Any] = {"role": role, "agent_identity": member_id}
```

ensure_member maps the example's member/profile to the actual RPC request and calls mobkit/ensure_member.

**`meerkat-mobkit/src/mob_handle_runtime.rs:9595-9600`**

```text
.member(&mid)
        .await?
        .send(content, handling_mode)
```

Ordinary delivery requires the existing member, so the corrected prerequisite is substantive.

**`sdk/python/meerkat_mobkit/builder.py:596-606`**

```text
self._config.console_require_app_auth = bool(required)
```

The preserved local-demo opt-out uses an implemented setter; the guide clearly warns against public exposure.

## K-008: The Python structural-event reference incorrectly caps MobEventKind at 25 variants

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/sdks/python.mdx:237-240`**

```text
`MobHandle.query_mob_events()` and `subscribe_mob_events()` project
every meerkat `MobEventKind` (25 variants) into a typed envelope
preserving `mob_id`, `run_id`, `step_id`, `agent_identity`, and the
full payload.
```

Consumers are given a false closed cardinality for structural events and may omit valid newer event kinds from dispatch or validation logic.

**`meerkat-mobkit/src/unified_runtime/mob_events.rs:551-634`**

```text
        MobEventKind::MobOwnerBridgeSessionBound { .. } => "mob_owner_bridge_session_bound",
```

The current exhaustive event_kind_label match enumerates 63 distinct MobEventKind arms, including owner binding, destruction, recovered binding, retirement, remote/placed obligations, remote host changes and objectives. Reproducible read-only count: isolate source between 'fn event_kind_label' and '/// Decode a raw mob-roster', then count distinct regex matches MobEventKind::(\w+): result 63, not 25.

**`meerkat-mobkit/src/unified_runtime/mob_events.rs:629-634`**

```text
        MobEventKind::SupervisorEscalation { .. } => "supervisor_escalation",
        MobEventKind::SupervisorEscalationFailed { .. } => "supervisor_escalation_failed",
        MobEventKind::OperatorActionRecorded { .. } => "operator_action_recorded",
        MobEventKind::ObjectiveOwnerBound { .. } => "objective_owner_bound",
        MobEventKind::ObjectiveConcluded { .. } => "objective_concluded",
```

The local projection itself is authority for the event kinds this MobKit version exposes; no moving upstream branch is needed.

### Independent adjudication

The current projection's exhaustive event_kind_label match has 63 distinct MobEventKind arms and no wildcard arm, not 25. I independently counted identifiers in that function only using Python re.findall(r'MobEventKind::(\w+)', source.split('fn event_kind_label', 1)[1].split('/// Decode a raw mob-roster', 1)[0]); len(matches) and len(set(matches)) both returned 63, and a wildcard-arm check returned false. build_envelope calls this label function and serializes the complete enum value, so the count is tied to the shipped projection, not an arbitrary unused list or moving upstream docs. D's coverage explicitly omitted this defect; no A-J finding owns the cardinality assertion. Removing the count is more durable than replacing it with another closed number.

**`docs/sdks/python.mdx:237-240`**

```text
`MobHandle.query_mob_events()` and `subscribe_mob_events()` project
every meerkat `MobEventKind` (25 variants) into a typed envelope
preserving `mob_id`, `run_id`, `step_id`, `agent_identity`, and the
full payload.
```

The false cardinality is current reference text, not a historical release note.

**`meerkat-mobkit/src/unified_runtime/mob_events.rs:174-176`**

```text
        let kind = event_kind_label(&event.kind).to_string();
        let (run_id, step_id, agent_identity) = extract_structural_fields(&event.kind);
        let data = serde_json::to_value(&event.kind).unwrap_or(Value::Null);
```

The exhaustive match is used by the actual envelope projector.

**`meerkat-mobkit/src/unified_runtime/mob_events.rs:551-555`**

```text
fn event_kind_label(kind: &MobEventKind) -> &'static str {
    match kind {
        MobEventKind::MobCreated { .. } => "mob_created",
        MobEventKind::MobDefinitionUpdated { .. } => "mob_definition_updated",
        MobEventKind::MobOwnerBridgeSessionBound { .. } => "mob_owner_bridge_session_bound",
```

This is the start boundary of the independently enumerated 63-arm match.

**`meerkat-mobkit/src/unified_runtime/mob_events.rs:629-636`**

```text
        MobEventKind::SupervisorEscalation { .. } => "supervisor_escalation",
        MobEventKind::SupervisorEscalationFailed { .. } => "supervisor_escalation_failed",
        MobEventKind::OperatorActionRecorded { .. } => "operator_action_recorded",
        MobEventKind::ObjectiveOwnerBound { .. } => "objective_owner_bound",
        MobEventKind::ObjectiveConcluded { .. } => "objective_concluded",
    }
}
```

The function includes newer event families and closes without a catch-all; the same function was counted through this boundary.

**Required correction:** Delete '(25 variants)' from docs/sdks/python.mdx. Retain the statement that the structural-event API projects all current Meerkat MobEventKind variants with the existing envelope fields and payload. If linking implementation, link the current MobKit structural-event projection rather than freezing another enum cardinality. Do not imply filters, authorization, or optional per-variant fields cease to apply.

### Changes and final verification

**Changed:** `docs/sdks/python.mdx`.

Removed the stale '(25 variants)' cardinality while retaining coverage of current Meerkat MobEventKind variants and the existing envelope/payload descriptions.

**Validation:** Counted 63 distinct arms in the current exhaustive event_kind_label projection and verified the current SDK page contains no frozen 25-variant claim. Historical changelog cardinalities remain untouched.

**Final review: pass.** The current Python structural-event reference no longer freezes the enum at 25 variants, while preserving envelope/payload claims and leaving historical changelog counts untouched. Independently counted 63 unique arms in the actual exhaustive event_kind_label function, confirmed no wildcard arm, and followed build_envelope's call and full enum serialization.

**`docs/sdks/python.mdx:281-284`**

```text
every current Meerkat `MobEventKind` into a typed envelope
preserving `mob_id`, `run_id`, `step_id`, `agent_identity`, and the
full payload.
```

The unstable cardinality is removed without changing the intended structural projection contract.

**`meerkat-mobkit/src/unified_runtime/mob_events.rs:174-176`**

```text
        let kind = event_kind_label(&event.kind).to_string();
        let (run_id, step_id, agent_identity) = extract_structural_fields(&event.kind);
        let data = serde_json::to_value(&event.kind).unwrap_or(Value::Null);
```

The independently counted function drives the actual public projection.

**`meerkat-mobkit/src/unified_runtime/mob_events.rs:629-636`**

```text
        MobEventKind::ObjectiveConcluded { .. } => "objective_concluded",
```

The current exhaustive 63-kind vocabulary contains newer event families; no replacement frozen count was introduced.

**Final review: pass.** The false fixed cardinality is removed without replacing it with another count or changing envelope semantics. Independent enumeration of the actual exhaustive projection found 63 distinct arms and no wildcard, and build_envelope uses that projection and serializes the complete enum. Historical release cardinalities were not treated as current-reference defects.

**`docs/sdks/python.mdx:281-284`**

```text
`MobHandle.query_mob_events()` and `subscribe_mob_events()` project
every current Meerkat `MobEventKind` into a typed envelope
preserving `mob_id`, `run_id`, `step_id`, `agent_identity`, and the
full payload.
```

All original envelope claims remain; the stale 25-variant assertion does not.

**`meerkat-mobkit/src/unified_runtime/mob_events.rs:174-176`**

```text
let kind = event_kind_label(&event.kind).to_string();
        let (run_id, step_id, agent_identity) = extract_structural_fields(&event.kind);
        let data = serde_json::to_value(&event.kind).unwrap_or(Value::Null);
```

The counted function drives actual envelopes rather than an unused list.

**`meerkat-mobkit/src/unified_runtime/mob_events.rs:629-635`**

```text
MobEventKind::ObjectiveConcluded { .. } => "objective_concluded",
```

The match beginning at line 551 includes modern event families and closes without a catch-all.

## K-009: The Rust HTTP bind-policy example passes a string where GatewaySurface is required

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/sdks/rust.mdx:312-313`**

```text
your init reply. Run `validate_http_bind_policy("my-host", addr,
HttpBindPolicy::for_gateway(allow_remote, &decisions))` before binding to
```

The recommended pre-bind safety check fails to compile when copied into an embedder, and misleadingly implies the API accepts a custom host label.

**`meerkat-mobkit/src/gateway_composition.rs:615-619`**

```text
pub fn validate_http_bind_policy(
    surface: GatewaySurface,
    listen: SocketAddr,
    policy: HttpBindPolicy,
) -> Result<(), HttpBindPolicyError> {
```

The first argument is the concrete enum, not a string or an Into-convertible generic.

**`meerkat-mobkit/src/gateway_composition.rs:513-518`**

```text
pub enum GatewaySurface {
    /// The SDK stdin JSON-RPC gateway.
    RpcGateway,
    /// The standalone console/HTTP gateway.
    MobkitGateway,
}
```

The surface labels identify the two bundled gateway diagnostic contexts; there is no arbitrary custom-host string variant.

### Independent adjudication

validate_http_bind_policy requires a concrete GatewaySurface, not &str or an Into-convertible generic. Its enum is deliberately closed to the two bundled diagnostic surfaces. Therefore the displayed call cannot compile even when addr, allow_remote, and decisions are correctly supplied. B-004 covers adjacent IPv6 URL wording, and E-009 covers governance validator signatures elsewhere; neither owns this distinct first-argument type mismatch. No A-J finding repeats this claim.

**`docs/sdks/rust.mdx:312-313`**

```text
your init reply. Run `validate_http_bind_policy("my-host", addr,
HttpBindPolicy::for_gateway(allow_remote, &decisions))` before binding to
```

The recommended call passes an arbitrary diagnostic string in an enum slot.

**`meerkat-mobkit/src/gateway_composition.rs:615-619`**

```text
pub fn validate_http_bind_policy(
    surface: GatewaySurface,
    listen: SocketAddr,
    policy: HttpBindPolicy,
) -> Result<(), HttpBindPolicyError> {
```

The signature directly proves the type error and preserves the other two argument types.

**`meerkat-mobkit/src/gateway_composition.rs:513-518`**

```text
pub enum GatewaySurface {
    /// The SDK stdin JSON-RPC gateway.
    RpcGateway,
    /// The standalone console/HTTP gateway.
    MobkitGateway,
}
```

The only supported diagnostic surface values are the two enum variants, not a custom-host label.

**`meerkat-mobkit/src/lib.rs:29`**

```text
pub mod gateway_composition;
```

The proposed module-qualified imports are publicly accessible.

**Required correction:** Import GatewaySurface, HttpBindPolicy, and validate_http_bind_policy from meerkat_mobkit::gateway_composition. Show validate_http_bind_policy(GatewaySurface::RpcGateway, addr, HttpBindPolicy::for_gateway(allow_remote, &decisions))? before binding, or choose GatewaySurface::MobkitGateway when that is the represented surface. Explicitly describe GatewaySurface as the closed gateway diagnostic enum, not an arbitrary host-name argument. Retain the existing bind-policy semantics, and coordinate separately with B-004's adjacent IPv6 correction and E-009's governance changes.

### Changes and final verification

**Changed:** `docs/sdks/rust.mdx`.

Replaced the string-valued HTTP bind-policy call with imports of validate_http_bind_policy, GatewaySurface, and HttpBindPolicy and an enum-typed GatewaySurface::RpcGateway call before binding. Explained the closed diagnostic enum and GatewaySurface::MobkitGateway alternative.

**Validation:** Matched the documented call against the actual public enum-typed signature at gateway_composition.rs:615-619 and confirmed public module export in lib.rs. Retained the real fail-closed non-loopback policy and post-bind warning semantics. Source validation only; no Rust compilation.

**Final review: pass.** The replacement example imports the three public gateway-composition symbols and supplies GatewaySurface::RpcGateway to the actual concrete enum argument. Its closed-enum explanation, MobkitGateway alternative, pre-bind placement, and auth-or-acknowledgement policy agree with source. Verified the public module export and HttpBindPolicy::for_gateway implementation; no Rust compilation is claimed.

**`docs/sdks/rust.mdx:326-340`**

```text
validate_http_bind_policy(
    GatewaySurface::RpcGateway,
    addr,
    HttpBindPolicy::for_gateway(allow_remote, &decisions),
)?;
```

The old arbitrary string argument is replaced with the correct enum value.

**`meerkat-mobkit/src/gateway_composition.rs:615-619`**

```text
pub fn validate_http_bind_policy(
    surface: GatewaySurface,
    listen: SocketAddr,
    policy: HttpBindPolicy,
) -> Result<(), HttpBindPolicyError> {
```

The example now matches all three argument types and the Result-returning API.

**`meerkat-mobkit/src/gateway_composition.rs:513-518`**

```text
pub enum GatewaySurface {
    /// The SDK stdin JSON-RPC gateway.
    RpcGateway,
    /// The standalone console/HTTP gateway.
    MobkitGateway,
}
```

The prose accurately identifies the closed diagnostic vocabulary.

**`meerkat-mobkit/src/gateway_composition.rs:501-505`**

```text
            allow_remote: allow_remote || ConsoleAuthPosture::of(decisions).is_enforced(),
```

The adjacent explanation preserves the actual auth-or-explicit-acknowledgement rule.

**Final review: pass.** The Rust SDK imports the public helper and enum types, passes GatewaySurface::RpcGateway, explains the closed diagnostic enum and alternative surface, and calls the policy check before binding. The adjacent B-004 same-family IPv6 correction is present, not lost. The E-009 signatures/document contract elsewhere on the same page are also correctly propagated.

**`docs/sdks/rust.mdx:325-341`**

```text
validate_http_bind_policy(
    GatewaySurface::RpcGateway,
    addr,
    HttpBindPolicy::for_gateway(allow_remote, &decisions),
)?;
```

The call now uses the required enum rather than a custom host string and retains the valid policy constructor.

**`docs/sdks/rust.mdx:337-341`**

```text
`GatewaySurface` is the closed gateway diagnostic enum, not an arbitrary
host name;
```

The diagnostic enum restriction is explicit.

**`meerkat-mobkit/src/gateway_composition.rs:615-624`**

```text
pub fn validate_http_bind_policy(
    surface: GatewaySurface,
    listen: SocketAddr,
    policy: HttpBindPolicy,
```

The source signature agrees with all argument types. The enum's two variants are defined at 513-518 and for_gateway derives the policy at 501-504.

**`docs/sdks/rust.mdx:315-319`**

```text
family: `0.0.0.0:PORT` becomes `http://127.0.0.1:PORT`, and `[::]:PORT`
becomes `http://[::1]:PORT`.
```

The neighboring IPv6 caveat matches loopback_reachable_addr at gateway_composition.rs:454-463.

## Independent scope checks

> [
>   "git rev-parse HEAD returned af82b6b3ab34faed9bf3e962d148d55f10dcd1dc; initial git status --short was empty.",
>   "All nine current documentation claims were read in context and independently traced to current local implementation. No evidence was taken from commit 01435e7c's patch.",
>   "Read-only enumeration of event_kind_label returned 63 total and 63 distinct MobEventKind variants, with no wildcard arm.",
>   "Read-only JSON inventory returned 140 A-J findings across ten reports; no K duplicate was found.",
>   "Final artifact validation passed: valid JSON, all nine unique K IDs covered exactly once, 57 evidence quotes found within their stated current-source line ranges, nine confirmed and zero rejected. Final git status --short remained empty and HEAD remained the baseline.",
>   "No builds, dependency installation, live gateway/provider invocation, repository edits, commits, or delegation were performed."
> ]

## Final scope checks

> [
>   "Loaded mobkit-platform and meerkat-architecture. Read the audit brief, complete K audit/adjudication, all B-G fixes reports and fixes-coordination. Independently followed each K source contract and required documentary occurrence.",
>   "Initial review read the complete relevant 01435e7cc prior diff across all twelve affected paths. A read-only script identified exactly 57 prior removed-text blocks. Only four neutral lead-ins/headings remained verbatim: 'Resolve a logical destination to a physical route.', 'Register a new route.', 'Remove a registered route.', and '## Session store contracts'. Semantic review found K-R001 despite disappearance of the old text; residual re-review now confirms its restoration in both API and configuration wording.",
>   "Initial cross-document search found two residual SDK roster descriptions; this feedback is retained in review_history. Final search/source assertions now pass across all seven roster documentary surfaces, including both SDK reference pages and both source comments. No 'registered at the time of the call' claim remains in those surfaces.",
>   "Exact Python quickstart block passes ast.parse; its exact TOML passes tomllib.loads. Assertions pass for mob ID, worker runtime_mode/comms, matching ensure/send member IDs/profile, ensure-before-send order, console_auth_required(False), correct event attributes and named credentials.",
>   "Python SDK AST with docstrings stripped is identical to HEAD. TypeScript types.ts with block comments stripped is byte-identical to HEAD. The two coordination source changes are documentary only.",
>   "Independent event_kind_label enumeration: 63 total and 63 distinct MobEventKind arms, no wildcard. build_envelope uses this label function and serializes the enum; the current Python page has no '(25 variants)' claim.",
>   "All prior-mapped paths and the additional SDK documentary paths are regular files, not symlinks. Reviewed MDX fences are balanced. Fifteen newly added absolute local links/heading anchors across the reviewed paths resolve.",
>   "git diff --check passed for the twelve prior-mapped document paths and both SDK source-documentation paths.",
>   "Residual re-review artifact validation passed: exactly K-001 through K-009 once each, nine pass/zero fail, all twelve prior mappings pass, and no active regressions. All 54 current evidence quotes (items, prior mappings, and round-two resolution proofs) match their stated current line ranges. Original failures/regression and their historical quotes remain in review_history round one; round two records both resolutions.",
>   "All 35 evidence quotes for the eight previously passing K items and both E-009 mapping quotes remain valid without citation changes. Refreshed the two corrected SDK-reference quotations and the gating table citation to line 685. The final Python quickstart still parses, its TOML still parses, and the four residual MDX pages have balanced fences.",
>   "Re-read the current topology-query/edge reconciliation snapshot source, full-refresh/reset/bootstrap empty-list callers, gateway context serialization, policy insertion, live SDK/stdin pre-dispatch call and ordinary required-tier parser. Confirmed the residual report against implementation rather than relying on its claimed validation.",
>   "Confirmed the manifest's Meerkat core family and Cargo.lock's exact meerkat-anthropic dependency are 0.8.40. The Python SDK page no longer claims a pinned 0.8.32 release; its updated 718-735 cache wording is explicitly backend-dependent, not blanket disabled. Provider-default semantics are the separate B/D re-review scope. The propagated E-009 Rust signatures/document contract remain valid.",
>   "Independently compiled docs/sdks/python.mdx, docs/sdks/typescript.mdx, docs/api/rpc.mdx and docs/reference/configuration.mdx with the existing artifact-directory @mdx-js/mdx plus remark-frontmatter: 4/4 passed, no installs or output files. Scoped git diff --check passed.",
>   "No repository edits, delegation, dependency installs, product builds, live gateways, provider calls, git-state mutations or commits were performed by this reviewer. Only the requested review artifact was created and refreshed."
> ]

## Review feedback and resolution history

Earlier review failures are retained here; the per-item dispositions above reflect the final re-review rather than erasing the feedback.

```json
[
  {
    "round": 1,
    "phase": "initial_independent_wave4_review",
    "overall_verdict": "fail",
    "counts": {
      "passed": 8,
      "failed": 1,
      "additional_prior_mapping_regressions": 1,
      "outstanding_document_occurrences": 3
    },
    "failed_items": [
      {
        "id": "K-005",
        "verdict": "fail",
        "reason": "The roster guide, RPC guide, quickstart, Python model docstring and TypeScript source comment all correctly qualify the callback-dependent list. However, two public SDK documentation mirrors still promise identities registered at call time and bootstrap-only emptiness. Those are the same confirmed defect, not a new runtime issue. An SDK reader can still interpret an empty full-refresh/reset list as empty membership, and the TypeScript example now contradicts the actual interface comment. All cross-document occurrences are therefore not closed.",
        "required_follow_up": "Apply the topology-snapshot versus bootstrap/full-refresh/reconcile/reset qualification to both public SDK references, explicitly warning that empty does not mean no registered identities.",
        "historical_doc_evidence": [
          {
            "path": "docs/sdks/python.mdx",
            "lines": "231",
            "quote": "Identities the identity runtime has registered at the time of the call. Empty on the bootstrap resolves (nothing is registered yet), populated on later re-derivations such as `mobkit/topology/query` or an edge reconcile."
          },
          {
            "path": "docs/sdks/typescript.mdx",
            "lines": "147-149",
            "quote": "Identities the identity runtime has registered at the time of the call.\n   * Empty on the bootstrap resolves, populated on later re-derivations such as\n   * `mobkit/topology/query` or an edge reconcile."
          }
        ],
        "source_proof": "identity_first/runtime.rs:1050-1056 supplies the registered list only at the topology-snapshot call site; 1280-1285 and 6663-6674 pass Vec::new() at full refresh and reset. The gateway forwards that exact context."
      }
    ],
    "regressions": [
      {
        "id": "K-R001",
        "related_findings": [
          "C-001",
          "B-007"
        ],
        "kind": "lost_prior_qualification",
        "severity": "low",
        "title": "RPC gating/evaluate still marks risk_tier unconditionally caller-required",
        "reason": "The original prior correction explicitly allowed omission when configured gateway policy supplies the tier. The current RPC table says Required=yes and calls it caller-supplied, only mentioning replacement of an existing value. The configuration guide now documents overriding tiers but likewise omits the fill-on-omission case. The low-level parser requires a tier, but the page explicitly documents SDK/stdin and names rpc_gateway, whose live pre-dispatch hook inserts a matching configured tier even when absent. Thus this prior API qualification has not been fully recovered. This is additional to K-005's two residual SDK mirrors, not a second count of that finding.",
        "evidence": [
          {
            "path": "docs/api/rpc.mdx",
            "lines": "684",
            "quote": "| `risk_tier` | `string` | yes | Caller-supplied `r0`, `r1`, `r2`, or `r3`; `rpc_gateway` replaces it with the configured action risk tier when one exists |",
            "explanation": "The current SDK/stdin request schema overstates caller-requiredness."
          },
          {
            "path": "docs/reference/configuration.mdx",
            "lines": "106",
            "quote": "the configured `risk_tier` overrides the caller's tier; a conflicting supplied string tier logs a warning.",
            "explanation": "The cross-reference preserves overriding precedence but no longer states that an omitted tier is supplied too."
          },
          {
            "path": "meerkat-mobkit/src/bin/rpc_gateway.rs",
            "lines": "6680-6707",
            "quote": "params.insert(\"risk_tier\".to_string(), Value::String(risk_tier.clone()));",
            "explanation": "Insertion is unconditional once action matches the configured table; the live stdin dispatcher invokes this helper at 13659-13660."
          },
          {
            "path": "meerkat-mobkit/src/rpc/gating_methods.rs",
            "lines": "109-113",
            "quote": ".ok_or(GatingParamsError::RiskTierRequired)?;",
            "explanation": "A caller tier remains required when no gateway policy supplies one; the correction must retain that distinction rather than globally making the field optional."
          }
        ],
        "prior_diff_proof": "git show 01435e7cc:docs/api/rpc.mdx contains Required='yes, unless supplied by configured gateway policy' and 'supplies the configured tier when omitted and overrides a caller-supplied tier'. The corresponding prior configuration row also explicitly supplies an omitted tier.",
        "correction": "Qualify the RPC required column as required unless matching rpc_gateway SDK/stdio policy supplies it. Explain both fill-on-omission and override-on-conflict in the API/configuration descriptions, keeping the explicit SDK/stdio scope and ordinary parser requirement for unconfigured actions."
      }
    ],
    "validation": "The initial report validated all 52 evidence quotes against then-current line ranges. Its failed document quotes above and in the preserved regression are historical feedback, not assertions that those strings remain in the final source."
  },
  {
    "round": 2,
    "phase": "independent_residual_re_review",
    "fix_report": "fixes-review-residuals.json",
    "overall_verdict": "pass",
    "resolutions": [
      {
        "id": "K-005",
        "previous_verdict": "fail",
        "verdict": "pass",
        "reason": "Read both corrected SDK references in context and re-traced topology snapshots, full refresh, reset-time adoption, bootstrap, and transport serialization. Both references now include the complete callback-dependent contract and empty-list warning. All earlier correct cross-document/source-comment occurrences remain correct; no runtime edit.",
        "evidence": [
          {
            "path": "docs/sdks/python.mdx",
            "lines": "231",
            "quote": "Bootstrap, full roster refresh/reconcile, and reset-time spec adoption currently pass an empty list; an empty list does not imply that no identities are registered.",
            "explanation": "The previously missed Python reference is fixed."
          },
          {
            "path": "docs/sdks/typescript.mdx",
            "lines": "149-151",
            "quote": "currently registered identities. Bootstrap, full roster refresh/reconcile,\n   * and reset-time spec adoption currently pass an empty list; an empty list\n   * does not imply that no identities are registered.",
            "explanation": "The previously missed TypeScript reference is fixed."
          }
        ]
      },
      {
        "id": "K-R001",
        "related_findings": [
          "C-001",
          "B-007"
        ],
        "previous_verdict": "fail",
        "verdict": "pass",
        "reason": "The RPC introduction and required column now admit a gateway-policy tier, and both API/configuration pages explicitly document fill-on-omission plus override-on-supply. Required caller tiers remain for unmatched actions; the config row retains warning and SDK/stdio-only qualifications. Independently re-read the unconditional insertion, its live pre-dispatch call and the downstream required parser.",
        "evidence": [
          {
            "path": "docs/api/rpc.mdx",
            "lines": "685",
            "quote": "| `risk_tier` | `string` | yes, unless matching gateway policy supplies it | `r0`, `r1`, `r2`, or `r3`. For matching `rpc_gateway` SDK/stdio policy, an omitted tier is filled and a supplied tier is overridden by the configured action risk tier. Otherwise, the caller must provide it |",
            "explanation": "The original lost caller-optional qualification is restored without making all tiers optional."
          },
          {
            "path": "docs/reference/configuration.mdx",
            "lines": "106",
            "quote": "the configured `risk_tier` fills an omitted tier or overrides any caller-supplied tier; a conflicting supplied string tier logs a warning. Unconfigured actions retain ordinary request behavior and still require a caller-supplied `risk_tier`.",
            "explanation": "The repeated configuration description is consistent and retains the unmatched-action requirement."
          },
          {
            "path": "meerkat-mobkit/src/bin/rpc_gateway.rs",
            "lines": "6680-6707",
            "quote": "params.insert(\"risk_tier\".to_string(), Value::String(risk_tier.clone()));",
            "explanation": "The matched policy supplies the value irrespective of prior presence; lines 13659-13660 prove the live SDK/stdin call site."
          },
          {
            "path": "meerkat-mobkit/src/rpc/gating_methods.rs",
            "lines": "109-113",
            "quote": ".ok_or(GatingParamsError::RiskTierRequired)?;",
            "explanation": "Without policy insertion the ordinary parser still requires a caller value."
          }
        ]
      }
    ],
    "validation": "Rechecked all 35 evidence quotes for the eight earlier passing K items, plus both E-009 prior-mapping quotes: all remain valid in the current stated ranges. Refreshed the two changed SDK-reference quotes and the gating row citation (now line 685). Final checks are recorded below."
  }
]
```
