# Mobpack: Portable Multi-Agent Deployment

> Status: **Design proposal** | Author: Luka + Claude | Date: 2026-02-24

## Overview

A **mobpack** is a self-contained, portable artifact that bundles everything needed to
run a multi-agent team. It is the unit of sharing, versioning, and deployment for mobs.

Mobpack is the intermediate representation in a compiler pipeline. Multiple backend
targets consume the same IR to produce deployment-specific outputs:

```
Definition + Skills + Config
        |
        v
    .mobpack (IR)
        |
        +---> rkat mob deploy       (interpret: run via installed rkat)
        +---> rkat mob embed        (stamp: platform-specific binary, no toolchain)
        +---> rkat mob compile      (build: optimized Rust binary)
        +---> rkat mob web build    (WASM: browser-deployable bundle)
        +---> (future targets: Lambda, CloudFlare Worker, desktop shell, ...)
```

---

## Part 1: Mobpack Format

### Problem

Deploying a mob today requires assembling scattered pieces at runtime -- a definition
JSON, skill files resolved from repositories, MCP server configs, hook definitions,
profile templates, and config layering. Sharing a working mob between users or
environments means sharing setup instructions, not a deployable artifact.

### Archive Structure

```
release-triage.mobpack (tar.gz)
+-- manifest.toml
+-- definition.json
+-- skills/
|   +-- code-review.md
|   +-- changelog-writer.md
+-- hooks/
|   +-- on-complete.toml
+-- mcp/
|   +-- servers.toml
+-- config/
    +-- defaults.toml
```

**Unpacked** it is a directory you can inspect and edit. **Packed** it is a single file
you can share. **Embedded** it is bytes stamped into a binary. All three are
representations of the same artifact.

### manifest.toml

The pack's identity and contract:

```toml
[mobpack]
name = "release-triage"
version = "1.0.0"
description = "Automated release triage with code review and changelog generation"
authors = ["team@example.com"]

[requires]
meerkat = ">=0.4.0"
credentials = ["anthropic_api_key"]       # declared, never stored
capabilities = ["comms", "shell"]         # what the mob needs from the runtime

[models]
default = "claude-sonnet-4-5"
min_context_window = 128000               # reject models that can't fit the workload

[profiles.lead]
model = "claude-opus-4-6"                 # profile-level model override

[surfaces]
cli = true
rpc = true
rest = false
mcp = false

# NOTE: trust policy is NOT declared in the pack (see Part 1b).
# A pack cannot request its own trust level -- that would be a downgrade vector.
```

### Contents

| Directory | Contents | Notes |
|-----------|----------|-------|
| `definition.json` | `MobDefinition` exactly as it exists today | No new format |
| `skills/` | Skill bodies inlined at pack time | No runtime resolution needed |
| `hooks/` | Hook definitions | In-process hooks embedded; command hooks declare external deps |
| `mcp/` | MCP server declarations | Records expected servers; does not bundle binaries |
| `config/defaults.toml` | Default provider, model, budget limits, tool flags | Overridable at deploy time via env vars or CLI flags |

### What Is NOT in the Pack

- API keys (injected via env vars at runtime)
- Session state (created at runtime; checkpoints are a separate export)
- MCP server binaries (declared as external dependencies)

---

## Part 1b: Artifact Trust and Signing

### Digest Semantics

`mobpack_digest` is a **canonical content hash**, not a hash of raw archive bytes.
Raw tar.gz bytes are non-deterministic (timestamps, compression level, metadata).
The canonical digest is computed as:

1. Enumerate all content paths **excluding `signature.toml`** (signature is over
   the digest, not part of it -- including it would create a circular dependency).
2. Sort paths lexicographically.
3. For each path, compute `sha256(file_bytes)`.
4. Build an index: `path || file_hash || executable_bit` per entry.
5. `mobpack_digest = sha256(index)`.

The `executable_bit` is intentionally part of the digest — it is semantic content
(hook scripts require `+x` to execute). To ensure cross-platform determinism,
`rkat mob pack` normalizes the bit at pack time: files under `hooks/` with a
recognized shebang or `.sh`/`.bash` extension get `executable = true`; all other
files get `executable = false`. The packed metadata is authoritative, not the
filesystem mode of the unpacking environment.

The digest is stable across sign/re-sign operations since `signature.toml` is
excluded.

### Signature Model

Mobpacks support optional Ed25519 signatures for provenance and authenticity.
Signing is orthogonal to the digest -- the digest proves content integrity,
the signature proves who produced it.

A signed pack includes a `signature.toml` at the archive root (excluded from
digest computation):

```toml
[signature]
signer_id = "team@example.com"
public_key = "ed25519:<base64>"
signature = "<base64 over mobpack_digest>"
timestamp = "2026-02-24T12:00:00Z"
```

### Trust Anchors

The pack's `signature.toml` contains the public key, but the pack cannot vouch
for itself. Trust is anchored externally:

- **Local trust store:** `~/.rkat/trusted-signers.toml` maps `signer_id` to
  pinned public keys. Strict mode accepts only signatures from keys in this store.
- **Project trust store:** `.rkat/trusted-signers.toml` (project-level, committed
  to version control). Merged with user store; project entries take precedence.
- **Registry trust:** Registries maintain their own signer allowlists. Publishing
  requires a key registered with the registry.

```toml
# ~/.rkat/trusted-signers.toml
[[signers]]
id = "team@example.com"
public_key = "ed25519:<base64>"
```

Verification: extract `signer_id` from `signature.toml`, look up expected
`public_key` in trust store, verify Ed25519 signature over `mobpack_digest`.
If `signer_id` is not in the trust store, strict mode rejects; permissive mode
warns.

### Trust Policy

Trust policy is configured per **environment**, never per pack. A pack cannot
declare its own trust level -- that would be a downgrade vector where a tampered
pack requests `permissive` to bypass signature checks.

Configuration (in order of precedence):
1. CLI flag: `--trust-policy strict`
2. Environment variable: `RKAT_TRUST_POLICY=strict`
3. Config file: `trust.policy` in `.rkat/config.toml`
4. Default: `permissive`

| Policy | Behavior | Use Case |
|--------|----------|----------|
| `permissive` | Allow unsigned packs, warn on missing signature | Local dev, experimentation |
| `strict` | Require valid signature from a trusted signer (trust store lookup) | CI, production, shared registries |

### Integrity vs Signature Enforcement

**Integrity (digest)** and **signature** serve different purposes and have different
enforcement semantics:

| | Integrity (digest) | Signature (policy) |
|-|-------------------|-------------------|
| **What it proves** | Content has not been modified since digest was computed | Content was produced by a known signer |
| **Unsigned packs** | Digest is computed and stored/displayed but there is no external reference to verify against. Integrity is self-referential. | No signature to check. |
| **Signed packs** | Digest is verified against the value the signature covers. Tampering is detected. | Signature is verified against trust store. Provenance is established. |

Concrete behavior per command:

| Command | Unsigned pack (permissive) | Unsigned pack (strict) | Signed pack |
|---------|--------------------------|----------------------|-------------|
| `rkat mob deploy` | Compute digest, proceed | Reject: no signature | Verify signature + digest |
| `rkat mob embed` | Compute digest, proceed | Reject: no signature | Verify signature + digest |
| `rkat mob compile` | Compute digest, proceed | Reject: no signature | Verify signature + digest |
| `rkat mob web build` | Compute digest, proceed | Reject: no signature | Verify signature + digest |
| `rkat mob publish` | Reject: registry requires signature | Reject: no signature | Verify signature + digest |
| Checkpoint import | Verify digest matches checkpoint's `mobpack_digest` | Same | Same |

In permissive mode with unsigned packs, "integrity" means the runtime computes
the digest for checkpoint compatibility and display, but cannot detect tampering
since there is no external reference value. This is acceptable for local
development. For any shared or production context, use signed packs with strict
policy.

---

## Part 2: Native Deployment Targets

### Deploy (Interpret)

Run a mobpack through an installed `rkat` runtime. Cross-platform -- the pack is
data, not code.

```bash
# One-shot run
rkat mob deploy release-triage.mobpack "triage the last 5 PRs"

# Start as a service
rkat mob deploy release-triage.mobpack --surface rpc

# Override config at deploy time
ANTHROPIC_API_KEY=sk-... rkat mob deploy release-triage.mobpack \
  --model claude-opus-4-6 \
  --budget-max-tokens 100000
```

### Embed (Stamp)

Stamp the pack into a pre-built platform binary. No Rust toolchain needed. The
release pipeline ships "mob shell" binaries for each platform -- generic `rkat`
binaries that read their mob definition from an embedded data section.

`rkat mob embed` is a file operation, not a compilation:

1. Read the pack archive.
2. Read the pre-built mob shell binary for the target platform.
3. Append the pack bytes to the binary using a platform-appropriate method.
4. Write a length trailer so the binary knows where embedded data starts.
5. Output the stamped binary.

**Platform signing caveat:** Appending bytes after build invalidates code signatures
(macOS codesign, Windows Authenticode). For signed distribution:

- **macOS/Windows:** The stamped binary must be re-signed after embedding. `rkat mob embed`
  accepts `--sign` with platform-appropriate credentials, or the consumer re-signs in
  their own release pipeline.
- **Linux:** No code signing requirement for standard distribution.
- **Alternative:** Use a sidecar model where the pack is a separate file loaded at
  startup rather than appended to the binary. This avoids signature invalidation
  entirely at the cost of two-file distribution.

```bash
rkat mob embed release-triage.mobpack -o release-triage
./release-triage "triage the last 5 PRs"

# With a specific surface
rkat mob embed release-triage.mobpack --surface rpc -o release-triage-rpc
./release-triage-rpc

# Cross-platform
rkat mob embed release-triage.mobpack \
  --target x86_64-unknown-linux-gnu \
  -o release-triage-linux
```

### Compile (Optimized Build)

Full Rust build. Tree-shakes unused features and surfaces for a minimal binary.

1. Read the pack, generate a Rust `main.rs` with `include_bytes!` for each asset.
2. Generate `Cargo.toml` with only the features/surfaces needed.
3. Run `cargo build --release --target <target>`.
4. Output the optimized binary.

```bash
rkat mob compile release-triage.mobpack \
  --surface rpc \
  --target x86_64-unknown-linux-gnu \
  -o release-triage-rpc
```

### Comparison

| Mode | Command | Requires | Output | Use Case |
|------|---------|----------|--------|----------|
| **Interpret** | `rkat mob deploy` | `rkat` installed | Process | Dev, testing, cross-platform sharing |
| **Embed** | `rkat mob embed` | `rkat` at build time | Platform binary | Distribution without rkat, Docker |
| **Compile** | `rkat mob compile` | Rust toolchain | Optimized binary | Production, CI/CD, edge, minimal images |

---

## Part 3: WASMpack (Browser Deployment Target)

### Problem

Running a mob in the browser today requires custom glue code, ad hoc API wiring,
and one-off frontend integration. A team can share a mob definition, but not a
deployable browser artifact that is ready to run by URL.

### Concept

A WASMpack is a target-specific runtime projection of a mobpack, not a parallel
ecosystem. It uses the same IR (mobpack) but compiles to a browser-deployable
bundle.

The web bundle's `manifest.web.toml` is a **derived build output**, not a
source-of-truth. It is generated from `manifest.toml` + target-specific build
options. Authors never edit it directly; it exists in the bundle for runtime
introspection. The mobpack's `manifest.toml` is always canonical.

```bash
# Build browser bundle from mobpack
rkat mob web build release-triage.mobpack -o dist/release-triage-web

# Validate browser compatibility
rkat mob validate release-triage.mobpack --target web

# Local preview
rkat mob web serve dist/release-triage-web --port 4173

# Publish to hosting
rkat mob web publish dist/release-triage-web \
  --to s3://org-web-agents/release-triage/1.0.0
```

### Bundle Structure

```
release-triage.webbundle/
+-- index.html
+-- runtime.js
+-- runtime_bg.wasm
+-- mobpack.bin
+-- manifest.web.toml              (derived from manifest.toml, read-only)
+-- adapters/
|   +-- in_page_tools.json
|   +-- oauth_connectors.json
|   +-- remote_worker_tools.json
+-- policy/
|   +-- limits.toml
+-- assets/
    +-- ui-config.json
```

### manifest.web.toml (Derived)

This file is generated by `rkat mob web build` from the source `manifest.toml`
plus web-specific build flags. It is never hand-authored.

```toml
# AUTO-GENERATED by rkat mob web build -- do not edit
# Source: manifest.toml @ mobpack_digest=sha256:abc123...

[wasmpack]
name = "release-triage-web"
version = "1.0.0"
description = "Browser-deployed release triage mob"
authors = ["team@example.com"]

[requires]
meerkat = ">=0.4.0"
browser_features = ["web_workers", "indexeddb", "fetch"]
credentials = ["oauth:jira", "oauth:salesforce"]

[targets.web]
mode = "browser_only"           # browser_only | browser_plus_remote | extension_bridge
capabilities = ["in_page_tools", "oauth_connectors", "streaming"]
forbid = ["shell", "mcp_stdio", "process_spawn"]

[llm_auth]
mode = "byok"                   # byok | gateway_required | either

[execution]
topology = "auto"               # single | coordinator_pool | per_agent
max_workers = 4

[models]
default = "claude-sonnet-4-5"
provider = "anthropic"

[policies]
max_actions = 200
max_runtime_seconds = 600
human_approval_for = ["write", "delete", "submit"]

[surfaces]
web = true
rpc = false
rest = false
mcp = false
```

### Browser Runtime Modes

| Mode | Requires | Output | Use Case |
|------|----------|--------|----------|
| **Browser Only** | Browser APIs only | Static WASM app | In-app agents, chat, triage |
| **Browser + Remote Workers** | Worker backend endpoint | Static app + remote config | Web research, distributed sensing |
| **Browser + Extension Bridge** | Browser extension installed | Static app + bridge contract | Authenticated UI automation |

### Provider Auth in Browser

API keys cannot live in a browser bundle. The auth model is explicit:

| Mode | Auth mechanism | Manifest declaration |
|------|---------------|---------------------|
| `byok` | User provides key at runtime, held in memory only | `llm_auth.mode = "byok"` |
| `gateway_required` | Calls route through a backend proxy holding the key | `llm_auth.mode = "gateway_required"` |
| `either` | Runtime tries BYOK, falls back to gateway if configured | `llm_auth.mode = "either"` |

`gateway_required` mode is not "fully browser-only" and must not be marketed as such.

**BYOK threat model:** User-provided keys are held in a JS closure, not in
`sessionStorage` or `localStorage`. Keys are never written to any persistent or
script-accessible browser storage. The key is cleared on page unload. This limits
exposure to same-origin XSS during the active session. The runtime should document
this explicitly and recommend Content-Security-Policy headers to mitigate XSS vectors.

**Gateway contract:** When `llm_auth.mode = "gateway_required"`, the manifest must
declare the gateway interface requirements:

```toml
[llm_auth.gateway]
protocol = "openai_compatible"    # openai_compatible | anthropic_proxy | custom
health_check = "/health"          # endpoint for validate --target web
```

`rkat mob validate --target web` must fail if `gateway_required` is set but no
gateway contract is declared. This ensures deploy determinism.

### Web Worker Topology

The default execution model:

- **One runtime worker** as the authoritative mob state machine (deterministic lifecycle).
- **Optional worker pool** for parallelizable tasks (local tools, parsing, scoring).
- Runtime decides placement by default (`topology = "auto"`), with override available.

For v1, do not force one worker per agent. The coordinator+pool model gives
concurrency without making inter-agent comms and state consistency painful.

### Tool Placement (browser_plus_remote)

When a mob uses both local and remote tools, placement is declared per tool:

```toml
[tools.fetch_docs]
placement = "prefer_remote"     # local_only | remote_only | prefer_local | prefer_remote
timeout_ms = 8000
retry = 2
fallback = "local"
```

- **Author** declares placement intent per tool.
- **Deployer** provides remote endpoint + credentials at deploy time.
- **Runtime** enforces policy and handles fallbacks with deadlines, retries,
  idempotency keys, circuit breakers, typed `ToolError`, and flow-level retry semantics.

Remote endpoints are deploy-time config by default, not baked into the pack.

### Extension Bridge Profiles

When browser APIs are insufficient (authenticated enterprise UI automation),
extension bridge mode provides scoped access:

| Profile | Scope | Default |
|---------|-------|---------|
| `current_tab_only` | Interact with the active tab's DOM only | Safest |
| `allowlisted_origins` | Interact with declared domains (e.g. `jira.atlassian.com`) | Production default |
| `broad_navigation` | Interact with any page the user navigates to | Disabled by default; explicit opt-in |

The recommended enterprise posture is `allowlisted_origins` + active-tab user gesture +
approval gates for destructive actions.

### Browser Tooling Model

The WASM runtime resolves tools from capability adapters:

| Adapter | Purpose | Access |
|---------|---------|--------|
| `in_page_tools` | Interact with the host app's state/UI | Scoped to embedding page |
| `oauth_connectors` | User-consented API access (Jira, Salesforce, etc.) | Scope-limited per connector |
| `remote_worker_tools` | Backend fetch/render/research workers | Deploy-time endpoint config |
| `extension_bridge` | Authenticated UI automation in enterprise apps | Extension profile required |

No implicit browser-wide access is granted.

### Bundle Size Targets

| Profile | Target (gzipped) | Capabilities |
|---------|------------------|-------------|
| `web_widget` | 0.8 -- 1.5 MB | Constrained: no extension bridge, no remote tools, no heavy parsers |
| `web_app` | 2 -- 4 MB | Full: all adapters, richer runtime |

Use feature slicing and lazy adapter loading so extension bridge, remote tools, and
heavy parsers are not in the baseline bundle.

### Feasible / Not Feasible (Browser WASM)

**Feasible:**
- Multi-agent workflows inside your own web app
- Live log triage, chat copilots, incident coordination
- OAuth/API-driven enterprise integrations
- Local-first processing and privacy-aware UX

**Not feasible:**
- Arbitrary cross-site tab automation
- Playwright-style browser-wide control
- Access to other tabs' DOM/cookies/session without extension
- Native process spawning (shell, stdio MCP servers)

---

## Part 4: Checkpoint Portability

### Cross-Surface Resume

A mob session started in the browser can be exported and resumed natively, and
vice versa. This is an explicit design goal, not an accidental possibility.

### Portable Checkpoint Format

A checkpoint is tied to:

| Field | Purpose |
|-------|---------|
| `mobpack_digest` | Canonical content hash of the originating mobpack (see Part 1b) |
| `contract_version` | Meerkat contract version at checkpoint time |
| `event_log` | Mob event history |
| `run_ledger` | Flow run state |
| `session_snapshots` | Per-member session state |

### Rules

- Secrets/tokens are never exported.
- If pack digest or contract version is incompatible, resume fails with clear
  migration diagnostics.
- Browser export (from in-memory state) -> native import (`rkat mob resume --from-checkpoint`)
  and native export -> browser import are both supported.

---

## Part 5: Compiler Pipeline Architecture

### The Abstraction

Mobpack is an intermediate representation. Each deployment target is a backend
that consumes the IR through a uniform interface:

```
trait BackendTarget {
    fn validate(&self, pack: &Mobpack, env: &BuildEnv) -> Result<(), ValidationError>;
    fn plan(&self, pack: &Mobpack, env: &BuildEnv) -> BuildPlan;
    fn build(&self, pack: &Mobpack, env: &BuildEnv) -> Result<Artifact, BuildError>;
}
```

### Registered Targets

| Target | Output | Backend |
|--------|--------|---------|
| `native_cli` | Stamped or compiled binary (CLI surface) | `NativeCliTarget` |
| `native_rpc` | Stamped or compiled binary (RPC surface) | `NativeRpcTarget` |
| `native_rest` | Stamped or compiled binary (REST surface) | `NativeRestTarget` |
| `web_app` | WASM bundle (full runtime) | `WebAppTarget` |
| `web_widget` | WASM bundle (constrained) | `WebWidgetTarget` |
| Future: `lambda` | AWS Lambda deployment package | `LambdaTarget` |
| Future: `cloudflare_worker` | CF Worker bundle | `CloudflareTarget` |
| Future: `desktop_shell` | Tauri/Electron wrapper | `DesktopTarget` |

### Pipeline Mapping

The release pipeline today:

```
validate -> build(linux_x86) -> build(linux_arm) -> build(macos) -> build(windows) -> publish
```

A mobpack pipeline:

```
validate(pack) -> build(pack, native_cli) -> build(pack, web_app) -> publish
```

Same shape. Mob compile targets plug into the same CI matrix.

---

## Part 6: What Changes in Meerkat

| Component | Change |
|-----------|--------|
| New: `meerkat-mob-pack` | Pack format, canonical digest, signing, validation, embed/stamp logic, `BackendTarget` trait |
| `meerkat-mob` | `MobBuilder::from_mobpack(bytes)` constructor |
| `meerkat` (facade) | `AgentFactory::from_mobpack()` -- wires factory from embedded config |
| `meerkat-cli` | `rkat mob pack/deploy/embed/compile/inspect` commands |
| `meerkat-cli` | `rkat mob web build/serve/publish/inspect` commands |
| `meerkat-cli` | `rkat mob validate --target <target>` |
| New: `meerkat-web-runtime` | WASM runtime, worker orchestration, browser adapter registry |
| New: `meerkat-web-adapters` | In-page tools, OAuth connectors, remote-worker bridge |
| Release pipeline | Build and publish mob shell binaries + WASM runtime assets |

## Compatibility

Existing APIs remain supported. New construction paths (`from_mobpack`,
`from_mobpack_web`) are additive. Behavior may differ from existing workflows in:

- **Config resolution:** Pack defaults override file-based config; deploy-time
  overrides take precedence over pack defaults. This is a new layering tier.
- **Skill resolution:** Pack-inlined skills bypass runtime repository resolution.
  Skills not in the pack are unavailable unless the runtime has filesystem access.
- **Validation:** `rkat mob validate --target web` enforces browser capability
  constraints that do not apply to native targets. A pack valid for native may
  fail web validation.

Unaffected:
- `MobDefinition` format
- Flow/lifecycle semantics
- Core mob logic and event contracts
- `MobHandle` and `MobBuilder` trait interfaces

---

## Part 7: Phasing

### Phase 1: Pack Format + Deploy

Define the archive format. Implement `rkat mob pack` and `rkat mob deploy`.
Portable sharing works immediately across platforms.

### Phase 2: Embed

Build mob shell binaries in the release pipeline. Implement `rkat mob embed`.
Standalone binaries without Rust toolchain.

### Phase 3: Compile

Implement `rkat mob compile` with codegen. Optimized production binaries with
tree-shaken features.

### Phase 4: Web Build (Browser-Only)

Browser-safe WASM subset, static bundle deploy, in-page tools, BYOK auth.

### Phase 5: OAuth Connector Pack Contracts

Scoped connectors, consent UX, policy enforcement for browser deployments.

### Phase 6: Remote Worker Tool Bridge

Hybrid browser + backend mode for web research and distributed sensing.

### Phase 7: Extension Bridge Profile

Explicit profile for authenticated UI automation where enterprise needs require it.

### Phase 8: Registry

Distribution, discovery, versioning. Git-based, HTTP, or OCI-compatible.

---

## Appendix: Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Pack format | tar.gz with manifest | Inspectable, editable, diffable, embeddable |
| Universal IR | mobpack | One artifact, many targets. Not a parallel format per target. |
| Digest algorithm | Canonical content hash (sorted paths + per-file sha256) | Deterministic across archive tools, compression, platform |
| Signing | Optional Ed25519, policy-gated (permissive/strict) | Unsigned for DX, signed for production/registry |
| Web manifest | Derived output, never source-of-truth | One IR principle: manifest.toml is canonical |
| Provider auth (browser) | Explicit `llm_auth.mode` with gateway contract | Honest about tradeoffs; no hidden proxies; deterministic validation |
| BYOK key storage | In-memory JS closure, never sessionStorage/localStorage | Minimize XSS exposure surface |
| Worker topology | Coordinator + pool default | Concurrency without state consistency pain |
| Tool placement | Author intent + runtime enforcement | Clean separation of concerns |
| Extension scope | Allowlisted origins default | Security posture is explicit, not implied |
| Checkpoint portability | Explicit goal with canonical digest compatibility | Cross-surface resume is a feature, not an accident |
| Bundle size | Two profiles (widget/app) | Feature slicing keeps baseline small |
| Binary stamping | Platform re-sign step for macOS/Windows | Honest about code signing constraints |
