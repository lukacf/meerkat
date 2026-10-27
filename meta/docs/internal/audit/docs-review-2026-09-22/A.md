# A: Root guidance, hidden skills, contribution and release history

[Audit index](README.md) | [Coverage](coverage.md)

Original documentation and initial evidence line ranges refer to baseline `af82b6b3ab34faed9bf3e962d148d55f10dcd1dc`, unless an external dependency or historical revision is explicitly identified. Final-review citations refer to the corrected files in this change. Source excerpts may be de-indented or omit intervening lines; cited ranges identify the complete context. Quoted defects are preserved as evidence, not current usage guidance.

## A-001: Contributor Rust prerequisite is below the package MSRV and contradicts the pinned development toolchain

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`CONTRIBUTING.md:9`**

```text
- Rust 1.85+ (edition 2024)
```

A contributor provisioning Rust 1.85-1.93 from the prerequisite list cannot build the workspace. The conflicting prerequisite also obscures the deliberate reproducibility pin documented later on the page.

**`Cargo.toml:18`**

```text
rust-version = "1.94.0"
```

The workspace's declared minimum supported compiler is 1.94.0, not the 1.85 edition floor.

**`meerkat-mobkit/Cargo.toml:13`**

```text
rust-version.workspace = true
```

The published crate inherits that higher minimum.

**`rust-toolchain.toml:1-4`**

```text
[toolchain]
channel = "1.97.0"
profile = "minimal"
components = ["clippy", "rustfmt"]
```

Repository development actually selects the pinned 1.97.0 toolchain; the release section already describes this distinction correctly.

### Independent adjudication

The prerequisite really says Rust 1.85+, rather than merely saying that edition 2024 debuted in that release. I checked the possible counterargument that rustup automatically selects the newer compiler: that can rescue a rustup-equipped checkout, but does not make 1.85 the supported compiler floor. The workspace declares 1.94.0, both member crates inherit it, and repository development selects 1.97.0. CONTRIBUTING's later release-toolchain explanation does not qualify or correct its prerequisite list. These are current setup instructions, not historical release notes.

**`CONTRIBUTING.md:7-11`**

```text
- Rust 1.85+ (edition 2024)
```

Independently reread the actual prerequisite in its development-setup context.

**`Cargo.toml:8-19`**

```text
rust-version = "1.94.0"
```

This is the package MSRV contract, not an inference from the language edition.

**`meerkat-mobkit/Cargo.toml:1-13`**

```text
rust-version.workspace = true
```

The public crate actually inherits the workspace MSRV.

**`rust-toolchain.toml:1-4`**

```text
channel = "1.97.0"
```

The development toolchain is an exact, newer pin. It should be distinguished from the minimum supported compiler.

**Required correction:** Replace CONTRIBUTING.md's Rust prerequisite with: 'Rust 1.97.0 via rustup, as pinned by rust-toolchain.toml; the crate's declared MSRV is 1.94.0.' Preserve the later explanation of why the development/CI toolchain is pinned.

### Changes and final verification

**Changed:** `CONTRIBUTING.md`.

Replaced the Rust 1.85+ prerequisite with the rustup-selected Rust 1.97.0 development pin and explicitly distinguished the crate's 1.94.0 MSRV. Preserved the existing release-toolchain rationale.

**Validation:** Read-only python3 -B TOML assertions passed for rust-toolchain.toml, Cargo.toml, and rust-version.workspace in both workspace members; the documented prerequisite matches those values.

**Final review: pass.** The contributor prerequisite now distinguishes the exact repository development toolchain from the package MSRV. Both values match the current authoritative files, and both workspace members inherit the MSRV. The later historical explanation for pinning the compiler was preserved rather than rewritten.

**`CONTRIBUTING.md:9`**

```text
- Rust 1.97.0 via rustup, as pinned by `rust-toolchain.toml`; the crate's declared MSRV is 1.94.0
```

The final prerequisite implements both parts of the adjudicated correction.

**`rust-toolchain.toml:1-4`**

```text
channel = "1.97.0"
```

Independently read and parsed the actual development pin.

**`Cargo.toml:18`**

```text
rust-version = "1.94.0"
```

Independent TOML assertions confirmed this package floor and rust-version.workspace=true in both member manifests.

**`meerkat-mobkit/Cargo.toml:13`**

```text
rust-version.workspace = true
```

The public crate inherits the stated MSRV; this is not merely an unused workspace field.

## A-002: Python 3.10 is sufficient for the SDK but not for the documented release tooling

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`CONTRIBUTING.md:10`**

```text
- Python 3.10+
```

A fully compliant Python-3.10 contributor environment fails the documented release commands at import time, before any protocol validation or dispatch.

**`scripts/release-candidate.py:34-40`**

```text
import subprocess
import sys
import tarfile
import time
import tomllib
import uuid
import zipfile
```

The release facade unconditionally imports tomllib, which is a Python 3.11+ standard-library module. There is no Python-3.10 tomli fallback.

**`Makefile:252-260`**

```text
release-candidate: ## Build immutable candidate on main (SOURCE_SHA required); never publish
	@python3 scripts/release-candidate.py dispatch --mode candidate

release-promote: export RELEASE_TAG := $(RELEASE_TAG)
release-promote: export ARTIFACT_SELECTION_FILE = $(ARTIFACT_SELECTION)
release-promote: export PUBLISH_RELEASE_PACKAGES ?= true
release-promote: export REGISTRY_DRY_RUN ?= false
release-promote: ## Promote accepted original bytes (RELEASE_TAG, ARTIFACT_SELECTION required)
	@python3 scripts/release-candidate.py dispatch --mode promote
```

The commands instructed by the contribution guide invoke that module with the contributor's python3, without selecting a newer interpreter.

**`sdk/python/pyproject.toml:9`**

```text
requires-python = ">=3.10"
```

The SDK's 3.10 floor is valid and should not be falsely raised; the documentation must distinguish SDK support from repository/release tooling.

### Independent adjudication

Python 3.10 is genuinely supported by the SDK, so raising the SDK minimum would be wrong. However, this contribution guide also instructs maintainers to run the release facade, and that facade unconditionally imports the Python-3.11 standard-library module tomllib. I looked for a fallback or interpreter-selection wrapper: there is no fallback in release-candidate.py, no local tomllib shim in scripts or sdk, and Make invokes plain python3. The defect is the undifferentiated prerequisite, not the SDK compatibility promise. No actual Python-3.10 execution is claimed.

**`CONTRIBUTING.md:7-11`**

```text
- Python 3.10+
```

The guide does not distinguish SDK use from the release workflow described on the same page.

**`scripts/release-candidate.py:24-40`**

```text
import tomllib
```

The import is unconditional at module loading, before any dispatch-mode branch or argument validation.

**`Makefile:248-260`**

```text
	@python3 scripts/release-candidate.py dispatch --mode candidate
```

The documented make command does not select or provision a Python interpreter with tomllib.

**`sdk/python/pyproject.toml:5-10`**

```text
requires-python = ">=3.10"
```

This independently confirms the valid, lower SDK floor that the fix must preserve.

**Required correction:** Qualify the prerequisite as 'Python 3.11+ for the release tooling and repository scripts that import tomllib; the Python SDK supports Python 3.10+.' Do not change the SDK's supported-Python declaration or imply that using the SDK itself requires 3.11.

### Changes and final verification

**Changed:** `CONTRIBUTING.md`.

Qualified Python 3.11+ as necessary for release tooling and repository scripts importing tomllib, while retaining the Python SDK's 3.10+ support promise.

**Validation:** Read-only python3 -B AST inspection confirmed the unconditional tomllib import in scripts/release-candidate.py; TOML assertions confirmed sdk/python/pyproject.toml still declares requires-python >=3.10. No interpreter or package support declarations were changed.

**Final review: pass.** The final prerequisite correctly confines Python 3.11+ to the release/tooling path needing tomllib and retains SDK support for Python 3.10+. It does not raise the package's supported-Python floor or claim that all SDK use requires the release interpreter. Source/import and Make-dispatch checks support the distinction; no Python-3.10 runtime execution is claimed.

**`CONTRIBUTING.md:10`**

```text
- Python 3.11+ for the release tooling and repository scripts that import `tomllib`; the Python SDK supports Python 3.10+
```

The final text includes the narrow tooling qualification required by adjudication.

**`scripts/release-candidate.py:38`**

```text
import tomllib
```

An independent AST check confirmed this is a top-level unconditional import, not a version-guarded optional branch.

**`Makefile:252-253`**

```text
release-candidate: ## Build immutable candidate on main (SOURCE_SHA required); never publish
	@python3 scripts/release-candidate.py dispatch --mode candidate
```

The documented facade uses the local python3 rather than selecting a newer interpreter.

**`sdk/python/pyproject.toml:9`**

```text
requires-python = ">=3.10"
```

The still-valid SDK compatibility contract is unchanged.

## A-003: README Quick Start requires an absent mob definition and never supplies its three profiles

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`README.md:108-115`**

```text
rt = await (
    MobKit.builder()
    .mob("config/mob.toml")
    .persistent_state(".mobkit/state")
    .roster(Roster())
    .topology_provider(Topology())
    .gateway("/path/to/rpc_gateway")
    .build()
```

After obtaining the gateway as instructed, a reader still cannot initialize the sample: it fails loading a file the guide never asks them to create. Creating an arbitrary mob.toml is insufficient because the roster requires personal, triage and calendar profiles.

**`sdk/python/meerkat_mobkit/runtime.py:593-600`**

```text
    def _build_init_params(self) -> dict[str, Any]:
        """Build init params dict from builder config for mobkit/init RPC."""
        params: dict[str, Any] = {}
        if self._config.mob_config_inline:
            params["mob_config"] = self._config.mob_config_inline
        elif self._config.mob_config_path:
            with open(self._config.mob_config_path) as f:
                params["mob_config"] = f.read()
```

The SDK reads the named file literally; it does not synthesize a mob definition from DurableAgentSpec.profile values.

**`README.md:79-98`**

```text
class Roster:
    async def roster(self, ctx):
        return [
            DurableAgentSpec(
                identity="identity:luka",
                profile="personal",
                addressability="addressable",
            ),
            DurableAgentSpec(
                identity="triage:main",
                profile="triage",
                addressability="internal_only",
            ),
            DurableAgentSpec(
                identity="domain:calendar",
                profile="calendar",
                addressability="internal_only",
            ),
        ]
```

The example additionally depends on three named profiles, but the Quick Start contains neither their configuration nor a link to a matching definition. This is a specific missing prerequisite of the advertised startup example, not a general request for more examples.

**`sdk/python/meerkat_mobkit/runtime.py:598-600`**

```text
with open(self._config.mob_config_path) as f:
```

Read-only reproduction from repository root with PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=sdk/python: b = MobKit.builder().mob('config/mob.toml').persistent_state('.mobkit/state'); MobKitRuntime(b._config)._build_init_params(). Result: FileNotFoundError [Errno 2] No such file or directory: 'config/mob.toml'. Path('config/mob.toml').exists() is false. No process or store was created.

### Independent adjudication

The gateway path is explicitly a user-supplied placeholder, so that alone would not be a defect. The missing mob definition is different: the Quick Start never supplies or identifies an existing definition containing its three named profiles. I tested whether convention defaults or the gateway's built-in mob remove the prerequisite. They do not: after builder validation and convention discovery, init-payload construction still raises FileNotFoundError for config/mob.toml. The gateway fallback only supplies profiles.default and cannot run these named-profile agents. This is a concrete missing startup prerequisite; it is not a demand that every illustrative placeholder be executable unchanged.

**`README.md:79-115`**

```text
    .mob("config/mob.toml")
```

The surrounding roster uses personal, triage and calendar; the Quick Start has no TOML definition or setup instruction for them.

**`sdk/python/meerkat_mobkit/builder.py:76-86`**

```text
        self._config.mob_config_path = config_path
```

mob stores a literal path; mob_inline is the separate supported mechanism for supplying an inline definition.

**`sdk/python/meerkat_mobkit/runtime.py:593-600`**

```text
            with open(self._config.mob_config_path) as f:
```

Independent read-only reproduction used MobKit.builder().mob('config/mob.toml').persistent_state('.mobkit/state'), then _validate(), _apply_convention_defaults(), and MobKitRuntime(builder._config)._build_init_params(). Result: FileNotFoundError: [Errno 2] No such file or directory: 'config/mob.toml'. mob_config_inline remained None and no state directory was created. No gateway was launched.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:10971-10982`**

```text
[profiles.default]
model = "gpt-5.5"
external_addressable = true
```

Even the gateway's no-config fallback defines only default, not the personal/triage/calendar profiles used by this example.

**Required correction:** Before the Python block, explicitly require and supply config/mob.toml with a [mob] id and [profiles.personal], [profiles.triage], and [profiles.calendar] definitions matching the roster, using a supported model and comms-enabled tools for the shown topology. Explain the selected provider's credential prerequisite and the addressable personal versus internal-only peers. Alternatively embed that complete definition using .mob_inline(...), whose implementation is present. Retain the explicitly user-supplied gateway path.

### Changes and final verification

**Changed:** `README.md`.

Added explicit instructions to create config/mob.toml from a complete minimal definition with personal, triage, and calendar profiles. All three use gpt-5.5 and enable comms; only personal is externally addressable. Documented OPENAI_API_KEY, the SDK installation prerequisite, relative working-directory semantics, and the existing top-level-await example's async context. Preserved the user-supplied gateway-path placeholder.

**Validation:** PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=sdk/python python3 -B extracted and parsed the documented TOML, compiled the Python with top-level await enabled, executed its provider definitions and builder expression without build(), and passed builder validation. It verified all 3 profile/roster/addressability mappings and both topology edges, then confirmed _build_init_params reads config/mob.toml and serializes that exact TOML using mock_open. Source cross-checks: rpc_gateway.rs:10971-10982 supplies the supported gpt-5.5 baseline; sdk/python/tests/test_identity_first_downstream_e2e.py:79-105 uses the same profile/tools/addressability schema. No gateway or provider request was executed.

**Final review: pass.** The missing startup prerequisite is fully supplied: an explicit working-directory-relative config/mob.toml, a mob id, and all three roster profiles with matching addressability and comms-enabled tools. The added provider credential, SDK installation, async-context and gateway-placeholder instructions are consistent with source. Independently extracted TOML and Python passed parsing/compilation, provider construction, exact roster/profile/addressability checks, canonical undirected topology checks, builder validation and mocked-file init serialization. This establishes the corrected configuration/SDK contract, not an unperformed live gateway/provider run.

**`README.md:74-86`**

````text
Create `config/mob.toml` relative to the directory from which you run the Python
example (create `config/` if needed):

```toml
[mob]
id = "personal-assistant"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true

[profiles.personal.tools]
comms = true
````

The guide now gives the actual file to create rather than assuming an absent configuration. The same TOML block supplies internal-only triage and calendar profiles at lines 88-100.

**`README.md:103-109`**

```text
These profiles match the roster below: the personal agent is externally
addressable, while triage and calendar are internal-only peers. Comms is enabled
for the topology's peer connections. Set `OPENAI_API_KEY` in the launching
environment for the selected OpenAI `gpt-5.5` model. After installing the Python
SDK (see [Install](#install)), run the following in an async context, such as a
notebook supporting top-level `await`, and replace the gateway path with your
extracted or built executable:
```

The added prerequisites match the example's exact profiles and execution form; the Install anchor resolves.

**`sdk/python/meerkat_mobkit/runtime.py:593-600`**

```text
        elif self._config.mob_config_path:
            with open(self._config.mob_config_path) as f:
                params["mob_config"] = f.read()
```

The actual SDK reads the advertised path. An independent mock_open check required one call with config/mob.toml and verified byte-for-byte serialization of the documented TOML, plus roster/topology flags and persistent_state.

**`sdk/python/tests/test_identity_first_downstream_e2e.py:79-105`**

```text
[profiles.triage]
model = "claude-sonnet-4-5"
skills = ["triage_role"]
external_addressable = false

[profiles.triage.tools]
comms = true
```

The existing identity-first integration fixture uses the same profile/addressability/nested-tools schema for the same three roles. Its different model/skills are not copied as README requirements.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:10971-10984`**

```text
[profiles.default]
model = "gpt-5.5"
external_addressable = true
```

The newly selected model matches the gateway's own current built-in default; the following code parses definitions using MobDefinition::from_toml.

**`sdk/python/meerkat_mobkit/identity_first_models.py:579-587`**

```text
        if b < a:
            a, b = b, a
        self.a = a
        self.b = b
```

Peer edges normalize endpoint order. The final independent probe compared canonical pairs, verifying the two documented peer connections without incorrectly treating them as directed.

**`sdk/python/meerkat_mobkit/_transport.py:81-82`**

```text
        self._env = {**os.environ, **(env or {})}
```

The gateway transport inherits the launching environment. CI's OpenAI-profile bootstrap also supplies OPENAI_API_KEY in .github/workflows/ci.yml:123-128; the guide does not promise credential-free construction.

## A-004: README incorrectly includes blob storage in the SQLite-backed store list

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`README.md:60`**

```text
For local and embedded deployments, `.persistent_state(...)` creates SQLite-backed MobKit metadata, console logs, runtime state, session state, and blob storage under one directory.
```

Readers can incorrectly assume backing up the SQLite databases also captures image/blob content, or choose the wrong store integration based on the stated backend.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:11707-11717`**

```text
        let binary_blob_store: Arc<dyn BinaryBlobStore> =
            match ObjectStoreBlobStore::local(storage_layout.blob_root()) {
                Ok(store) => Arc::new(store),
                Err(e) => fail_init(
                    &request_id,
                    STORAGE_RESOLUTION_CODE,
                    format!("failed to open binary blob store: {e}"),
                ),
            };
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(Base64BlobStoreAdapter::new(binary_blob_store.clone()));
```

The persistent gateway chooses the local object-store implementation for blobs, not SQLite.

**`meerkat-mobkit/src/blob_store.rs:95-106`**

```text
impl ObjectStoreBlobStore {
    pub fn local(root: PathBuf) -> Result<Self, BlobStoreError> {
        std::fs::create_dir_all(&root).map_err(|err| BlobStoreError::Internal(err.to_string()))?;
        let store = object_store::local::LocalFileSystem::new_with_prefix(&root)
            .map_err(|err| BlobStoreError::Internal(err.to_string()))?;
        Ok(Self {
            backend: BlobObjectBackend::ObjectStore {
                store: Arc::new(store),
                persistent: true,
            },
            legacy_root: Some(root),
        })
```

The implementation is explicitly a filesystem-backed object store.

**`meerkat-mobkit/src/blob_store.rs:118-123`**

```text
    fn object_path(blob_id: &BlobId) -> ObjectPath {
        ObjectPath::from(format!("objects/{}.bin", storage_key(blob_id)))
    }

    fn meta_path(blob_id: &BlobId) -> ObjectPath {
        ObjectPath::from(format!("meta/{}.json", storage_key(blob_id)))
```

Blob bytes and metadata occupy separate .bin and .json files, not database rows.

### Independent adjudication

I checked whether the blob object-store adapter might still use SQLite underneath, and whether the cited code belonged only to an ephemeral path. Neither counterargument holds: the persistent-state gateway branch calls ObjectStoreBlobStore::local, which constructs object_store::local::LocalFileSystem. Blob content and its metadata are separate files under the state directory's blobs subtree. The README's list grammatically includes blob storage among the SQLite-backed stores. The correction should describe the default local layout, not promise that app-injected stores or explicitly ephemeral overrides are SQLite.

**`README.md:60-60`**

```text
creates SQLite-backed MobKit metadata, console logs, runtime state, session state, and blob storage under one directory
```

The quoted shared modifier is the inaccurate backend claim.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:11707-11717`**

```text
            match ObjectStoreBlobStore::local(storage_layout.blob_root()) {
```

Tracing the enclosing branch back to its persistent_state condition at line 11589 shows this is the default persistent launch's blob backend.

**`meerkat-mobkit/src/blob_store.rs:95-106`**

```text
        let store = object_store::local::LocalFileSystem::new_with_prefix(&root)
```

The adapter explicitly opens a filesystem-backed object store, not a database-backed object store.

**`meerkat-mobkit/src/blob_store.rs:118-123`**

```text
        ObjectPath::from(format!("objects/{}.bin", storage_key(blob_id)))
```

Blob bytes use object files, with the adjacent meta_path method producing meta/*.json.

**`meerkat-mobkit/src/storage_layout.rs:470-473`**

```text
        self.state_dir.join(BLOB_ROOT_DIR_NAME)
```

The layout places this store beneath the persistent state directory; BLOB_ROOT_DIR_NAME is 'blobs' at line 79.

**Required correction:** Say that the default local persistent-state layout combines SQLite-backed MobKit metadata, console logs, runtime/session state with filesystem-backed blob storage under one state directory. State that backing up only the SQLite files omits blob content; include the blobs subtree. Preserve the separate caveat for externally supplied stores/providers.

### Changes and final verification

**Changed:** `README.md`.

Separated the default local layout's SQLite-backed metadata, console, runtime, and session stores from filesystem-backed blobs. Explicitly required the blobs subtree in backups and preserved the externally supplied store/provider caveat.

**Validation:** Source-contract review passed: rpc_gateway.rs:11707-11717 opens ObjectStoreBlobStore::local(storage_layout.blob_root()); blob_store.rs:95-106 constructs LocalFileSystem, and :118-123 places bytes and metadata in objects/*.bin and meta/*.json. The replacement is explicitly scoped to the default local layout.

**Final review: pass.** The replacement correctly distinguishes SQLite-backed state from local object-store blob files and explicitly includes the blobs subtree in backup scope. The claim is confined to the default local layout and preserves the app-supplied-provider caveat, so it does not falsely describe deliberate in-memory overrides or external stores. I traced both the ordinary SQLite session path and the identity-continuity adapter path; the latter is also locally SQLite-backed.

**`README.md:60`**

```text
the default `.persistent_state(...)` layout combines SQLite-backed MobKit metadata, console logs, runtime state, and session state with filesystem-backed blob storage under one directory. Include the `blobs/` subtree in backups; copying only the SQLite files omits blob content.
```

The final wording removes the erroneous shared SQLite modifier from blob storage and fixes the consequential backup misunderstanding.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:11707-11717`**

```text
            match ObjectStoreBlobStore::local(storage_layout.blob_root()) {
```

The persistent launch selects the local filesystem object-store backend. The adjacent runtime branch opens SqliteRuntimeStore at line 11742; metadata and console use their SQLite stores at lines 12506 and 12559.

**`meerkat-mobkit/src/blob_store.rs:95-105`**

```text
        let store = object_store::local::LocalFileSystem::new_with_prefix(&root)
```

This is an actual filesystem backend, not a SQLite adapter. object_path/meta_path at lines 118-123 place bytes in objects/*.bin and metadata in meta/*.json.

**`meerkat-mobkit/src/storage_layout.rs:471-473`**

```text
    pub fn blob_root(&self) -> PathBuf {
        self.state_dir.join(BLOB_ROOT_DIR_NAME)
    }
```

BLOB_ROOT_DIR_NAME is explicitly "blobs" at line 79, so the backup subtree named in the fix is correct.

**`meerkat-mobkit/src/identity_first/local_store.rs:19-45`**

```text
use rusqlite::{Connection, OptionalExtension, Transaction};
```

The local continuity store uses rusqlite and declares both continuity_records and session_snapshots tables in this range. The gateway wraps its local continuity substrate in ContinuitySessionStoreAdapter at rpc_gateway.rs:11499-11501, so identity-first local session state does not contradict the narrowed SQLite claim.

## A-005: Platform skill's current Meerkat dependency baseline is sixteen patch versions stale

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`.claude/skills/mobkit-platform/SKILL.md:20`**

```text
Current direct Meerkat dependency family in `meerkat-mobkit/Cargo.toml`: `0.8.24`. Verify the manifest before release or dependency work; do not rely on this note if the checkout has moved.
```

An agent consulting the skill for the current capability floor starts from the wrong release and may reason about already-fixed upstream defects. The existing verification warning reduces severity but does not make the 'Current' assertion correct.

**`meerkat-mobkit/Cargo.toml:59-66`**

```text
meerkat-core = { version = "=0.8.40" }
meerkat-client = { version = "=0.8.40" }
meerkat-comms = { version = "=0.8.40" }
meerkat-contracts = { version = "=0.8.40" }
meerkat-mob = { version = "=0.8.40" }
meerkat-mob-mcp = { version = "=0.8.40" }
meerkat-mcp = { version = "=0.8.40" }
meerkat-models = { version = "=0.8.40" }
```

Every current direct Meerkat pin in this manifest resolves to the 0.8.40 family; 0.8.24 is not the current baseline.

### Independent adjudication

The skill warns readers to verify the manifest, which reduces the impact but does not make its current-baseline literal accurate. I independently parsed both workspace manifests and Cargo.lock rather than relying on a changelog: the main crate has 20 direct/dev upstream version requirements, all =0.8.40; the conformance crate has five; all 35 locked upstream Meerkat packages are 0.8.40. The claim is in Current Baseline, not a dated historical section. No conclusion depends on prior commit 01435e7c.

**`.claude/skills/mobkit-platform/SKILL.md:20-20`**

```text
Current direct Meerkat dependency family in `meerkat-mobkit/Cargo.toml`: `0.8.24`.
```

This is explicitly a current-version assertion, even with the following verification warning.

**`meerkat-mobkit/Cargo.toml:59-66`**

```text
meerkat-core = { version = "=0.8.40" }
```

These exact source requirements, not the note, govern the installed family; the neighboring Meerkat dependencies have the same exact version.

**`meerkat-mobkit/Cargo.toml:168-188`**

```text
meerkat-schedule = { version = "=0.8.40" }
```

The additional development-only family pin independently agrees with the current baseline.

**Required correction:** Remove the duplicated 0.8.24 literal and direct readers to the exact Meerkat requirements in the current workspace manifests and lockfile. If retaining a snapshot instead, update it to 0.8.40 and keep the explicit instruction to verify the manifests before dependency/release work.

### Changes and final verification

**Changed:** `.claude/skills/mobkit-platform/SKILL.md`.

Removed the stale duplicated 0.8.24 current-baseline literal. Directed readers to both workspace manifests, applicable dev-dependencies, and Cargo.lock before release or dependency work.

**Validation:** Read-only TOML validation found 20 main-crate and 5 conformance-crate upstream requirements, all =0.8.40, plus 35 coherent locked upstream packages at 0.8.40. The skill now names those authorities instead of maintaining another version snapshot.

**Final review: pass.** The stale current-version snapshot has been removed rather than merely replaced with another duplicate. The new guidance names both actual workspace manifests, their dev-dependencies and the lockfile. Independent TOML parsing found the current 25 exact upstream requirements and 35 locked upstream packages coherent at 0.8.40; no current-baseline 0.8.24 claim remains in the skill.

**`.claude/skills/mobkit-platform/SKILL.md:24-27`**

```text
For the current Meerkat dependency family, read the exact requirements in
`meerkat-mobkit/Cargo.toml` and `mobkit-store-conformance/Cargo.toml`, including
dev-dependencies, and the resolved versions in `Cargo.lock`. Verify these before
release or dependency work rather than relying on a duplicated version snapshot.
```

This is the adjudicated source-of-truth correction, not an unsupported new dependency-version assertion.

**`meerkat-mobkit/Cargo.toml:59-66`**

```text
meerkat-core = { version = "=0.8.40" }
```

Direct requirements are authoritative; independent parsing counted 20 main-crate normal/dev requirements at this exact family.

**`mobkit-store-conformance/Cargo.toml:37-44`**

```text
meerkat-store-conformance = { version = "=0.8.40" }
```

The second named file really carries five additional independent exact requirements; lockfile parsing verified all 35 resolved upstream Meerkat packages, not just the main crate's first pin.

## A-006: Platform skill overstates verify-version-parity coverage of generated files

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`.claude/skills/mobkit-platform/SKILL.md:16-18`**

```text
The authoritative MobKit release line is `[workspace.package].version` in the
root `Cargo.toml`; `make verify-version-parity` checks every SDK and generated
surface against it.
```

Maintainers are told a green version-parity command verifies all generated surfaces even though a stale mobkit-store-conformance/BUILD.bazel can escape that particular check.

**`scripts/verify-version-parity.sh:85-96`**

```text
BAZEL_FILE="$ROOT/meerkat-mobkit/BUILD.bazel"
if [ -f "$BAZEL_FILE" ]; then
    BAD_BAZEL=$(grep -oE '"CARGO_PKG_VERSION": "[^"]*"' "$BAZEL_FILE" \
        | grep -v "\"$CARGO_VER\"" | sort -u || true)
    if [ -n "$BAD_BAZEL" ]; then
        red "FAIL: BUILD.bazel CARGO_PKG_VERSION entries != $CARGO_VER:"
        printf '%s\n' "$BAD_BAZEL"
        FAIL=1
    else
        green "  BUILD.bazel CARGO_PKG_VERSION: OK"
    fi
fi
```

The verifier explicitly inspects only meerkat-mobkit/BUILD.bazel. Its remaining checks cover SDK manifests/lockfile, MODULE.bazel and two installation docs; it never checks the conformance crate's generated BUILD file.

**`Cargo.toml:1-5`**

```text
[workspace]
members = [
    "meerkat-mobkit",
    "mobkit-store-conformance",
]
```

There is another generated workspace-crate surface outside that hard-coded check.

**`scripts/generate-bazel-rust-builds.mjs:1774-1779`**

```text
// The root BUILD.bazel is HAND-MAINTAINED in mobkit, not generated:
```

The generator's final comment distinguishes per-crate generator-owned BUILD files and its --check freshness gate. That separate gate, not verify-version-parity alone, covers generated BUILD freshness.

### Independent adjudication

I read the whole version-parity script looking for delegation to the generator, workspace-wide BUILD enumeration, or another check of the conformance file. None exists. Cargo metadata supplies the primary package version but does not validate generated BUILD files. The script checks a fixed primary-crate BUILD path, SDK manifests/root lockfile versions, MODULE.bazel and two install documents. The independent generator check does enumerate workspace packages, but it is a different command. This finding concerns the verifier's advertised coverage, not an assertion that today's generated files are stale.

**`.claude/skills/mobkit-platform/SKILL.md:16-18`**

```text
root `Cargo.toml`; `make verify-version-parity` checks every SDK and generated
surface against it.
```

The claim is broader than the actual verifier's inputs.

**`scripts/verify-version-parity.sh:85-96`**

```text
BAZEL_FILE="$ROOT/meerkat-mobkit/BUILD.bazel"
```

The entire generated-BUILD version check is restricted to this file; the rest of the script contains no conformance BUILD check or generator invocation.

**`Cargo.toml:1-5`**

```text
    "mobkit-store-conformance",
```

The omitted generated crate is an active workspace member, not an out-of-scope archived package.

**`scripts/generate-bazel-rust-builds.mjs:20-24`**

```text
    .filter((pkg) => pkg.source === null && workspaceMembers.has(pkg.id))
```

The separate generator obtains local workspace packages rather than hard-coding only the primary crate.

**`scripts/generate-bazel-rust-builds.mjs:628-638`**

```text
    console.error(`stale generated Bazel file: ${relative(root, path)}`);
```

Its check-only branch compares existing file contents to generated contents, providing the broader freshness check the skill should name separately.

**Required correction:** Describe make verify-version-parity as checking SDK package/root-lockfile versions, the root Bazel module, the primary MobKit BUILD version fields, and canonical Rust installation snippets. Separately name 'node scripts/generate-bazel-rust-builds.mjs --check' as the generated per-crate BUILD freshness gate. Do not claim either command proves all release behavior.

### Changes and final verification

**Changed:** `.claude/skills/mobkit-platform/SKILL.md`.

Narrowed verify-version-parity coverage to SDK package versions, TypeScript lockfile root versions, MODULE.bazel, the primary MobKit BUILD version fields, and canonical Rust install snippets. Named the separate generator --check command for generated per-crate BUILD freshness.

**Validation:** Read the full scripts/verify-version-parity.sh and confirmed its fixed meerkat-mobkit/BUILD.bazel check at :85-96, lockfile checks at :98-114, and install-doc checks at :116 onward. Retained the independently adjudicated generator freshness command without claiming to have run a release or generated-file build.

**Final review: pass.** The final skill precisely narrows the parity script's coverage and assigns generated per-crate BUILD freshness to the separate generator --check command. I read the complete verifier and the generator's package selection, check-only comparison and exit behavior. No broad 'every generated surface' promise remains, and the command is not represented as proving all release behavior.

**`.claude/skills/mobkit-platform/SKILL.md:16-22`**

```text
root `Cargo.toml`. `make verify-version-parity` checks the Python and TypeScript
SDK package versions, the TypeScript lockfile's root versions, the root Bazel
module, the primary `meerkat-mobkit/BUILD.bazel` version fields, and canonical
Rust installation snippets. Separately, run
`node scripts/generate-bazel-rust-builds.mjs --check` to check generated
per-crate BUILD freshness.
```

The final inventory matches the actual fixed inputs of the verifier and distinguishes the generator gate.

**`scripts/verify-version-parity.sh:85-96`**

```text
BAZEL_FILE="$ROOT/meerkat-mobkit/BUILD.bazel"
```

The BUILD parity check remains deliberately limited to the primary crate. Other sections check package versions, MODULE.bazel, both root lockfile versions and the two installation documents; there is no hidden workspace-wide generator call.

**`scripts/generate-bazel-rust-builds.mjs:20-24`**

```text
    .filter((pkg) => pkg.source === null && workspaceMembers.has(pkg.id))
```

The separate generator selects all local workspace packages, including the conformance crate.

**`scripts/generate-bazel-rust-builds.mjs:628-638`**

```text
  const existing = existsSync(path) ? readFileSync(path, "utf8") : null;
  if (existing !== contents) {
    staleFileCount += 1;
    console.error(`stale generated Bazel file: ${relative(root, path)}`);
```

The --check path performs content comparison rather than generation writes; the final staleFileCount check at lines 1781-1784 exits nonzero. This review verified the contract from source without running Cargo metadata or regeneration.

## A-007: Platform skill promises persistent ABAC mutations even for an in-memory controller

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`.claude/skills/mobkit-platform/SKILL.md:81`**

```text
- Live config: `AccessController` (std RwLock + revision) persists TOML on every admin mutation; per-request `AccessView` snapshots; agent label/role attributes cached from roster projections so label selectors work on identity-only surfaces.
```

An embedder can follow the documented in-memory controller injection path, make admin edits, and lose those edits on restart despite the unconditional persistence promise.

**`meerkat-mobkit/src/access/controller.rs:75-83`**

```text
        Ok(Self {
            inner: Arc::new(AccessControllerInner {
                state: RwLock::new(AccessState {
                    config: Arc::new(config),
                    revision: 0,
                }),
                persist_path: RwLock::new(None),
                attributes: RwLock::new(BTreeMap::new()),
                mutation: Mutex::new(()),
```

AccessController::new constructs a controller with no persistence path, a supported composition through the runtime.set_access_controller seam named in the same skill.

**`meerkat-mobkit/src/access/controller.rs:224-232`**

```text
        let persist_path = self
            .inner
            .persist_path
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(path) = persist_path {
            persist_config(&path, &config)?;
        }
```

The mutation commit persists only when a path is configured, otherwise changing memory/revision alone.

**`meerkat-mobkit/src/access/controller.rs:106-121`**

```text
        let controller = Self::new(config)?;
        *controller
            .inner
            .persist_path
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(path);
        Ok(controller)
    }

    /// Set (or replace) the persistence path.
    pub fn with_persist_path(self, path: impl Into<PathBuf>) -> Self {
        *self
            .inner
            .persist_path
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(path.into());
```

load_or_default and with_persist_path are the concrete ways to establish the promised durable mutation behavior.

### Independent adjudication

I checked whether runtime injection automatically assigns a persistence path, which could have rescued the unconditional promise. Both the builder's access_controller method and runtime's set_access_controller simply store the supplied controller. AccessController::new sets persist_path to None, while commit writes TOML only inside an if-let-Some branch. File-backed construction does persist; therefore the finding must be narrowed to the missing condition, not described as generally broken persistence. Also, only successful mutations increment the revision; failed validation/persistence does not.

**`.claude/skills/mobkit-platform/SKILL.md:81-81`**

```text
persists TOML on every admin mutation
```

The skill omits the constructor/path condition that source explicitly enforces.

**`meerkat-mobkit/src/access/controller.rs:75-83`**

```text
                persist_path: RwLock::new(None),
```

A controller created from an ordinary validated config is in-memory by default.

**`meerkat-mobkit/src/access/controller.rs:223-241`**

```text
        if let Some(path) = persist_path {
            persist_config(&path, &config)?;
        }
```

Persistence is conditional and precedes the revision increment/in-memory swap.

**`meerkat-mobkit/src/unified_runtime/mod.rs:1249-1253`**

```text
        self.access_controller = Some(controller);
```

The public injection seam does not add persistence behind the caller's back.

**`meerkat-mobkit/src/access/controller.rs:93-122`**

```text
    pub fn with_persist_path(self, path: impl Into<PathBuf>) -> Self {
```

This method and load_or_default establish the path needed for durable admin changes.

**Required correction:** State that successful config mutations validate and advance the revision, and persist TOML only when a persistence path is configured through load_or_default or with_persist_path. AccessController::new and disabled remain in-memory unless a path is subsequently attached. Preserve the snapshot/cache description.

### Changes and final verification

**Changed:** `.claude/skills/mobkit-platform/SKILL.md`.

Qualified validation/revision changes as successful config mutations and TOML persistence as conditional on load_or_default or with_persist_path. Explicitly described new/disabled controllers as in-memory unless a path is attached, preserving per-request snapshots and the roster attribute-cache explanation.

**Validation:** Source-contract review passed against access/controller.rs:67-122 and :204-241: constructors start without a path, load_or_default/with_persist_path attach one, mutation validates, and commit persists conditionally before advancing the revision.

**Final review: pass.** The unconditional persistence guarantee is removed. The final text restricts validation/revision advancement to successful config mutations and persistence to a configured path, names both path-establishing APIs, and preserves the valid snapshot/attribute-cache description. Source confirms validation and optional persistence occur before the revision change, so failed mutation/persistence is not accidentally described as a committed revision.

**`.claude/skills/mobkit-platform/SKILL.md:88`**

```text
successful `AccessController` config mutations validate and advance the revision. They persist TOML only when a persistence path is configured through `load_or_default` or `with_persist_path`; `AccessController::new` and `disabled` remain in-memory unless a path is attached.
```

All adjudicated constructor, path and success qualifiers are present.

**`meerkat-mobkit/src/access/controller.rs:75-90`**

```text
                persist_path: RwLock::new(None),
```

new starts without persistence; disabled delegates to new. Neither constructor silently acquires a path.

**`meerkat-mobkit/src/access/controller.rs:106-122`**

```text
    pub fn with_persist_path(self, path: impl Into<PathBuf>) -> Self {
```

Both this method and load_or_default set persist_path to Some(path), exactly as the revised skill describes.

**`meerkat-mobkit/src/access/controller.rs:218-241`**

```text
        validate_access_config(&config)?;
        self.commit(config)
```

commit conditionally calls persist_config with error propagation, then replaces the in-memory config and increments revision. The successful-mutation qualifier matches actual ordering.

## A-008: Platform skill describes child-first grouping, but the sidebar gives detected ancestors precedence

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`.claude/skills/mobkit-platform/SKILL.md:183`**

```text
Spawned/delegate rows inherit configured group/subgroup metadata from their detected host if they do not carry matching metadata themselves.
```

An application author following the skill expects explicit child labels to override inheritance, but spawned rows remain under the detected ancestor's configured section/subgroup.

**`console/src/panels/Sidebar.tsx:393-409`**

```text
  let current: ConsoleAgent | undefined = agent;
  const seen = new Set<string>();
  while (current && !seen.has(current.member_id)) {
    seen.add(current.member_id);
    chain.push(current);
    if (!parentById || !byId) break;
    const parentId = parentById.get(current.member_id);
    if (!parentId) break;
    current = byId.get(parentId);
  }
  const searchOrder = chain.length > 1 ? [...chain].reverse() : chain;
  for (const candidate of searchOrder) {
    const value = firstConfiguredValue(candidate, selectors);
    if (value) return value;
  }
  return config?.fallback_group?.trim() || "Agents";
```

configuredAgentGroup reverses the child-to-ancestor chain before looking for a value: the highest detected ancestor with a value wins even when the child has its own matching value. configuredAgentSubgroup repeats that algorithm at lines 420-436.

**`console/src/panels/Sidebar.test.ts:260-274`**

```text
    labels: {
      group: "workers",
      scope_id: "child-scope",
    },
    wired_to: ["initiative:parent"],
  };
  const grouped = __sidebarTest.groupSidebarAgents([child, parent], opsGrouping);

  assert.deepEqual(
    grouped.get("initiatives")?.map((row) => [row.agent.member_id, row.subgroup, row.depth]),
    [
      ["initiative:parent", "parent-scope", 0],
      ["initiative:child", "parent-scope", 1],
    ],
  );
```

The existing regression intentionally gives the child conflicting metadata, then requires the parent group and parent subgroup for the child. This is deliberate behavior, not a proposed runtime change.

### Independent adjudication

I checked both configured grouping helpers and their production call site, not just the test's description. groupSidebarAgents actually passes the detected parent map to both helpers; both reverse the child-to-ancestor chain and return the first configured value. A child's explicit matching value therefore loses to an ancestor's value. The existing regression deliberately supplies different parent and child labels and expects the child in the parent's group/subgroup. This is a documentation mismatch, not a request to change the intended UI policy. The qualification 'detected ancestor' matters: missing parents and unconfigured selectors do not follow this configured-chain rule.

**`.claude/skills/mobkit-platform/SKILL.md:183-183`**

```text
Spawned/delegate rows inherit configured group/subgroup metadata from their detected host if they do not carry matching metadata themselves.
```

The sentence makes explicit child metadata appear to prevent inheritance.

**`console/src/panels/Sidebar.tsx:402-407`**

```text
  const searchOrder = chain.length > 1 ? [...chain].reverse() : chain;
```

configuredAgentGroup searches the highest detected ancestor first, rather than only consulting ancestors when the child has no value.

**`console/src/panels/Sidebar.tsx:429-434`**

```text
  const searchOrder = chain.length > 1 ? [...chain].reverse() : chain;
```

configuredAgentSubgroup has the same precedence; this is not just a group-only behavior.

**`console/src/panels/Sidebar.tsx:554-567`**

```text
    const configuredGroup = configuredAgentGroup(a, config, parentById, byId);
```

The real grouping call site supplies the parent chain; the immediately following subgroup call does too.

**`console/src/panels/Sidebar.test.ts:260-274`**

```text
      ["initiative:child", "parent-scope", 1],
```

The child in this test has group='workers' and scope_id='child-scope', yet the expected row belongs to initiatives/parent-scope.

**Required correction:** For configured group_by/subgroup_by selectors, explain that the detected ancestor chain is searched highest-ancestor-first down to the child; the first available configured value wins, keeping children with their host even when child labels differ. Child values apply only if no earlier ancestor supplies a value. Do not extend this inheritance rule to badges or change selector syntax.

### Changes and final verification

**Changed:** `.claude/skills/mobkit-platform/SKILL.md`.

Described configured group_by/subgroup_by precedence as highest-detected-ancestor first, with child values used only when no earlier ancestor supplies a configured value. Explicitly excluded badges from this inheritance rule and preserved selector syntax.

**Validation:** Source-contract review passed against console/src/panels/Sidebar.tsx:383-436: both helpers reverse the detected child-to-ancestor chain. The existing Sidebar.test.ts:239-274 conflicting child-label fixture expects the parent's group and subgroup.

**Final review: pass.** The final text states ancestor-first precedence for configured group_by/subgroup_by, including the conflicting-child-label case, and expressly excludes badges. I checked both helper algorithms, their production parent-map call site and the existing contradictory-child-label fixture. Unconfigured selectors still return None and badge lookup still reads only the row itself, so the fix does not overgeneralize the inheritance rule.

**`.claude/skills/mobkit-platform/SKILL.md:190`**

```text
For configured `group_by` and `subgroup_by` selectors, the detected ancestor chain is searched from the highest ancestor down to the child; the first available configured value wins.
```

The false child-first exception is replaced with the source's actual traversal order; the same paragraph limits child fallback and excludes badges.

**`console/src/panels/Sidebar.tsx:383-436`**

```text
  const searchOrder = chain.length > 1 ? [...chain].reverse() : chain;
```

Both configuredAgentGroup and configuredAgentSubgroup build a child-to-ancestor chain, reverse it and return the first configured value. The production caller passes parentById/byId to both at lines 565-567.

**`console/src/panels/Sidebar.tsx:438-450`**

```text
      const value = configuredFieldValue(agent, badge.field || "");
```

Badges use the supplied agent only, validating the new explicit exclusion.

**`console/src/panels/Sidebar.test.ts:260-274`**

```text
      ["initiative:child", "parent-scope", 1],
```

The fixture gives the child workers/child-scope labels yet expects it under initiatives/parent-scope. This existing expectation independently supports the revised statement; no UI behavior was changed.

## A-009: Platform skill's SDK validation block cannot reliably run the TypeScript tests

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`.claude/skills/mobkit-platform/SKILL.md:290-295`**

````text
For SDK changes:

```bash
cd sdk/python && pytest
cd sdk/typescript && npm test
```
````

Following the block literally never reaches the TypeScript suite after Python. Running its TypeScript line alone still does not validate freshly edited SDK source.

**`sdk/typescript/package.json:18-23`**

```text
  "scripts": {
    "build": "tsc && node scripts/build-cjs.js",
    "typecheck": "tsc --noEmit",
    "test": "npx tsx --test tests/*.test.ts",
    "test:agent-memory-real": "MOBKIT_AGENT_MEMORY_REAL_API_SMOKE=1 npx tsx --test tests/agent-memory-real-smoke.test.ts",
    "validate": "npm run typecheck && npm run build && npm test",
```

npm test neither typechecks nor builds the dist output consumed by many tests; validate is the existing complete command.

**`sdk/typescript/tests/builder-api.test.ts:8-11`**

```text
import { MobKit, MobKitBuilder } from "../dist/index.js";
import { CallbackDispatcher } from "../dist/agent-builder.js";
import { SessionBuildOptions } from "../dist/models.js";
import type { SessionCreatedContext } from "../dist/types.js";
```

Tests execute built output, so npm test on a clean checkout fails or on a previously built checkout tests stale code. dist/index.js is absent in the audited worktree.

**`.github/workflows/ci.yml:266-267`**

```text
      - run: npm --prefix sdk/typescript ci --silent --no-fund --no-audit
      - run: npm --prefix sdk/typescript run validate --silent
```

The repository's real TypeScript CI performs the necessary build via validate. Independently, executing the skill's two lines in one shell leaves cwd at sdk/python before the second cd; sdk/python/sdk/typescript does not exist (confirmed with a read-only pathlib check). The earlier console cd block also lacks root restoration.

### Independent adjudication

There are two independently proven failures. Executing the adjacent cd lines in one shell makes the TypeScript path relative to sdk/python, and that directory does not exist. Even granting the charitable interpretation that each line starts in a fresh repository-root shell, npm test has no pretest hook and does not build dist, while actual tests import dist modules. Thus the commands can fail in a clean checkout or test stale output. I did not infer suite failure from missing installed node_modules: the package scripts/import contract is sufficient, and no npm install or tests were run.

**`.claude/skills/mobkit-platform/SKILL.md:290-295`**

```text
cd sdk/python && pytest
cd sdk/typescript && npm test
```

A read-only shell reproduction entered sdk/python and then failed cd sdk/typescript with 'No such file or directory'.

**`sdk/typescript/package.json:18-25`**

```text
    "validate": "npm run typecheck && npm run build && npm test",
```

validate is the existing complete pipeline; test alone is just npx tsx --test tests/*.test.ts, and the script inventory has no pretest build.

**`sdk/typescript/tests/builder-api.test.ts:8-11`**

```text
import { MobKit, MobKitBuilder } from "../dist/index.js";
```

These tests execute the build output rather than freshly changed src files. Independent path inspection found dist/index.js absent in this checkout.

**`.github/workflows/ci.yml:266-267`**

```text
      - run: npm --prefix sdk/typescript run validate --silent
```

Hosted CI confirms the intended build-before-test command, after installing the SDK's dependencies.

**Required correction:** State that validation commands start at repository root. Replace the SDK block with '(cd sdk/python && python3 -m pytest)' followed by 'npm --prefix sdk/typescript run validate'. Make the console block root-preserving as well, using npm --prefix console for phase0:types, phase1:targets and build (or wrapping that block in a subshell), so later Rust/SDK commands remain rooted correctly. These commands assume the project's existing development dependencies are installed.

### Changes and final verification

**Changed:** `.claude/skills/mobkit-platform/SKILL.md`.

Made the workflow explicitly repository-root based with development dependencies installed. Replaced console cd commands with npm --prefix console, wrapped Python validation in a subshell, and used npm --prefix sdk/typescript run validate so typechecking/building precede tests. Updated the adjacent console build description to match.

**Validation:** Seven affected-context Bash blocks across the four edited documents passed bash -n via stdin without executing them. Package-script assertions confirmed all named console scripts exist and TypeScript validate is exactly npm run typecheck && npm run build && npm test. The skill no longer has either persistent cd sequence.

**Final review: pass.** All affected workflow blocks now keep the shell at repository root, with explicit installed-development-dependency context. The Python cd is isolated in a subshell; console and TypeScript use npm --prefix. TypeScript validate actually typechecks and builds before tests that import dist. Independent Bash syntax checks and package-script inventory checks passed, and the console build-output statement matches its implementation. No test execution or dependency installation is inferred from these checks.

**`.claude/skills/mobkit-platform/SKILL.md:278-286`**

```text
Run these commands from the repository root with the existing development
dependencies installed.
```

The following console block uses npm --prefix console for all three commands, eliminating persistent cd state before later blocks.

**`.claude/skills/mobkit-platform/SKILL.md:302-303`**

```text
(cd sdk/python && python3 -m pytest)
npm --prefix sdk/typescript run validate
```

These commands fix both the sequential-directory failure and the stale/missing-build-output risk.

**`sdk/typescript/package.json:18-23`**

```text
    "validate": "npm run typecheck && npm run build && npm test",
```

The named script exists and guarantees build-before-test ordering.

**`sdk/typescript/tests/builder-api.test.ts:8-11`**

```text
import { MobKit, MobKitBuilder } from "../dist/index.js";
```

Actual tests consume built output, so using validate is substantive rather than a cosmetic rename.

**`console/build.cjs:9-10`**

```text
const outDir = path.join(__dirname, "dist");
const embeddedOutDir = path.join(__dirname, "../meerkat-mobkit/console-dist");
```

The build script writes the browser bundle and copies shared generated files into the embedded output at lines 102-105; the revised command description remains accurate.

## A-010: Platform skill still claims the release hook does not regenerate the conformance BUILD file

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`.claude/skills/mobkit-platform/SKILL.md:320-323`**

```text
The current hook does not regenerate `mobkit-store-conformance/BUILD.bazel`
by itself, so keep the explicit generation step until that release-tooling
gap is fixed. Tag the exact merged main commit only after the release PR is
green.
```

The skill tells maintainers to compensate manually for a defect that no longer exists, hiding the fact that generated files are now part of the automatic release update/staging contract.

**`scripts/release-hook.sh:18-22`**

```text
echo "release-hook: bumping SDK versions to $VERSION"
"$ROOT/scripts/bump-sdk-versions.sh" "$VERSION"

echo "release-hook: verifying version parity"
"$ROOT/scripts/verify-version-parity.sh"
```

The pre-release hook calls the canonical bump script.

**`scripts/bump-sdk-versions.sh:72-77`**

```text
if [ -f "$ROOT/scripts/generate-bazel-rust-builds.mjs" ] && command -v node >/dev/null 2>&1; then
    # Run FROM $ROOT: the generator resolves the workspace with `git rev-parse`
    # against the caller's cwd, so invoking it by absolute path from elsewhere
    # would target whichever repo the caller happened to be standing in.
    ( cd "$ROOT" && node scripts/generate-bazel-rust-builds.mjs >/dev/null )
    echo "  Bazel BUILD.bazel regenerated for all workspace crates: $VERSION"
```

That script now regenerates all workspace-crate BUILD files, including mobkit-store-conformance, not only the primary crate. Its fallback also iterates every BUILD.bazel.

**`scripts/release-hook.sh:42-48`**

```text
bazel_build_files=()
while IFS= read -r tracked; do
    bazel_build_files+=("$tracked")
done < <(git -C "$ROOT" ls-files -- '*BUILD.bazel')

if [ ${#bazel_build_files[@]} -gt 0 ]; then
    git -C "$ROOT" add -- "${bazel_build_files[@]}"
```

The hook stages all tracked generated BUILD files too; the documented release-tooling gap has already been fixed.

### Independent adjudication

I traced the current hook rather than assuming that an explicit generation command in the guide proves a missing capability. The hook calls bump-sdk-versions.sh; in this checkout that script invokes the generator for all workspace crates, and the hook stages all tracked BUILD.bazel files. The conformance crate is included through workspace metadata. I checked the fallback too: it updates version fields in every discovered BUILD.bazel, not only the primary crate. The old gap assertion is therefore false. The replacement should describe normal operation with repository prerequisites available, rather than promise full regeneration when the generator is unavailable or the hook intentionally short-circuits a duplicate version.

**`.claude/skills/mobkit-platform/SKILL.md:320-323`**

```text
The current hook does not regenerate `mobkit-store-conformance/BUILD.bazel`
by itself
```

This is a present-tense tooling limitation, not a historical incident note.

**`scripts/release-hook.sh:18-22`**

```text
"$ROOT/scripts/bump-sdk-versions.sh" "$VERSION"
```

The canonical pre-release hook delegates the version update to the script containing workspace-wide generation.

**`scripts/bump-sdk-versions.sh:72-77`**

```text
    ( cd "$ROOT" && node scripts/generate-bazel-rust-builds.mjs >/dev/null )
```

When the generator and Node are available, this actually regenerates workspace BUILD files. The branch's status message also explicitly says 'all workspace crates'.

**`scripts/release-hook.sh:42-48`**

```text
done < <(git -C "$ROOT" ls-files -- '*BUILD.bazel')
```

The following git add stages the enumerated tracked BUILD files, so the conformance result is not left out of the release commit.

**`scripts/generate-bazel-rust-builds.mjs:20-24`**

```text
const workspaceMembers = new Set(metadata.workspace_members);
```

The generator selects workspace packages, which include mobkit-store-conformance in Cargo.toml.

**Required correction:** Replace the obsolete gap statement with: 'With the repository prerequisites installed, the release hook regenerates the workspace per-crate BUILD files and stages all tracked BUILD.bazel files, including mobkit-store-conformance/BUILD.bazel. Keep the generator --check command as verification.' Any explicit generation line may be retained as redundant verification/setup, not justified by the removed gap. Preserve the exact-merged-main CI/tag requirement.

### Changes and final verification

**Changed:** `.claude/skills/mobkit-platform/SKILL.md`.

Replaced the obsolete missing-conformance-generation claim with the prerequisite-qualified hook behavior: regenerate workspace per-crate BUILD files and stage every tracked BUILD.bazel. Removed the redundant manual generation line while retaining --check verification. Preserved and clarified exact-merged-main CI and accepted-candidate requirements before tagging.

**Validation:** Source-contract review passed: scripts/release-hook.sh:18-22 delegates to bump-sdk-versions.sh, whose :72-77 runs the generator when Node and the script are available; release-hook.sh:42-48 enumerates/stages all tracked BUILD.bazel files. No release hook, tag, staging, dispatch, or publisher was executed.

**Final review: pass.** The obsolete conformance-generation gap is removed and normal hook behavior is accurately prerequisite-qualified. Tracing cargo-release configuration through release-hook.sh and bump-sdk-versions.sh confirms workspace generation and staging of every tracked BUILD.bazel. Removing the redundant generation command does not remove the separate --check guard. Exact merged-main push CI and external candidate acceptance are preserved and clarified, not replaced by a PR-only green check. Explicit version placeholders remain placeholders; the shell examples parse when substituted, and no release action was executed.

**`.claude/skills/mobkit-platform/SKILL.md:328-334`**

```text
With the repository prerequisites installed, the release hook regenerates the
workspace per-crate BUILD files and stages all tracked `BUILD.bazel` files,
including `mobkit-store-conformance/BUILD.bazel`.
```

The final statement matches normal hook operation and retains --check plus exact-commit CI/candidate-acceptance qualifications in the immediately following sentences.

**`scripts/release-hook.sh:18-22`**

```text
"$ROOT/scripts/bump-sdk-versions.sh" "$VERSION"
```

The canonical release hook calls the script containing workspace-wide generation; this is not merely an available manual helper.

**`scripts/bump-sdk-versions.sh:72-77`**

```text
    ( cd "$ROOT" && node scripts/generate-bazel-rust-builds.mjs >/dev/null )
```

With Node and the generator available, the script regenerates per-workspace-crate BUILD files from the repository root. The prerequisite qualifier avoids promising full regeneration in the fallback branch.

**`scripts/release-hook.sh:42-48`**

```text
done < <(git -C "$ROOT" ls-files -- '*BUILD.bazel')
```

The resulting list is staged by the following git add; the conformance BUILD is not omitted by a hard-coded primary-crate path.

**`.github/workflows/release.yml:138-155`**

```text
              workflow_id: "ci.yml",
              branch: "main",
              event: "push",
              head_sha: ref,
```

The release gate queries and then re-filters exact-main push runs. The revised skill's exact-merged-main wording follows the implemented gate; external consumer acceptance remains an owner procedure rather than an asserted automated test.

## A-011: Platform skill's Meerkat upgrade instructions omit the second workspace manifest's exact pins

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`.claude/skills/mobkit-platform/SKILL.md:325`**

```text
When updating Meerkat dependencies, edit `meerkat-mobkit/Cargo.toml`, then run `./scripts/repo-cargo update -p ...` for the Meerkat family and `meerkat-mobkit`.
```

Following the stated upgrade procedure leaves the active conformance harness pinned to the old Meerkat family, so the workspace upgrade is incomplete and can fail dependency resolution or conformance compilation.

**`mobkit-store-conformance/Cargo.toml:37-45`**

```text
meerkat-core = { version = "=0.8.40" }
# The one-remote-bundle reference provider (M4b) implements both levels of
# the composite seam: meerkat's RealmStorageProvider (facade storage_provider
# module + in-memory store implementations) next to MobKit's realm store set.
meerkat = { version = "=0.8.40", features = ["session-store", "memory-store"] }
meerkat-runtime = { version = "=0.8.40" }
meerkat-store = { version = "=0.8.40", features = ["memory"] }
meerkat-store-conformance = { version = "=0.8.40" }
meerkat-mobkit = { path = "../meerkat-mobkit" }
```

The conformance crate has five independently declared exact upstream pins; cargo update changes lockfile resolution, not these manifest requirements.

**`Cargo.toml:1-5`**

```text
[workspace]
members = [
    "meerkat-mobkit",
    "mobkit-store-conformance",
]
```

The omitted manifest is an active workspace member, not an archived example.

### Independent adjudication

I checked whether the conformance crate inherits dependency versions from the main crate or a root workspace.dependencies table, which would make the one-manifest instruction sufficient. It does not: it directly declares five exact upstream requirements. Cargo update changes resolutions compatible with manifest requirements; it cannot change those =0.8.40 declarations. The main crate also has relevant dev-dependency pins. Therefore following the skill literally leaves an active workspace member constrained to the old family. This is a dependency-maintenance instruction defect, not evidence of a currently mixed lockfile; the current lockfile is coherent at 0.8.40.

**`.claude/skills/mobkit-platform/SKILL.md:325-325`**

```text
When updating Meerkat dependencies, edit `meerkat-mobkit/Cargo.toml`, then run `./scripts/repo-cargo update -p ...`
```

The prescribed sequence omits the other direct requirements entirely.

**`mobkit-store-conformance/Cargo.toml:37-45`**

```text
meerkat-store-conformance = { version = "=0.8.40" }
```

This is one of five independently declared exact upstream pins in the conformance member, alongside meerkat-core, meerkat, meerkat-runtime and meerkat-store.

**`Cargo.toml:1-5`**

```text
    "mobkit-store-conformance",
```

The conformance crate is part of ordinary workspace resolution/builds.

**`meerkat-mobkit/Cargo.toml:178-188`**

```text
meerkat-mob = { version = "=0.8.40", features = ["schema"] }
```

Development requirements must also move; updating only normal dependencies is insufficient.

**Required correction:** Require updating the Meerkat-family exact requirements in both meerkat-mobkit/Cargo.toml and mobkit-store-conformance/Cargo.toml, including applicable dev-dependencies, before using ./scripts/repo-cargo update for the affected packages. Verify Cargo.lock resolves the intended coherent upstream family and run the appropriate workspace validation.

### Changes and final verification

**Changed:** `.claude/skills/mobkit-platform/SKILL.md`.

Required updating the Meerkat family's exact requirements in both active workspace manifests, including applicable dev-dependencies, before wrapper-driven cargo update. Added coherent Cargo.lock verification and appropriate workspace validation.

**Validation:** Read-only TOML validation confirmed both active workspace members, 20 main-crate upstream requirements and 5 conformance-crate requirements, and the currently coherent 35-package lockfile family. No manifest or lockfile was changed.

**Final review: pass.** The upgrade procedure now covers both active workspace manifests and applicable development pins before resolution, followed by coherent lockfile and workspace verification. Independent parsing confirms these are real independent requirements, not inherited aliases. The text does not claim cargo update alone changes exact manifest pins or that the current coherent lockfile was previously broken.

**`.claude/skills/mobkit-platform/SKILL.md:336-341`**

```text
When updating Meerkat dependencies, update the family's exact requirements in
both `meerkat-mobkit/Cargo.toml` and `mobkit-store-conformance/Cargo.toml`,
including applicable dev-dependencies, before running
`./scripts/repo-cargo update -p ...` for the affected packages.
```

The following two lines explicitly require coherent Cargo.lock resolution and appropriate wrapper/Make workspace checks.

**`Cargo.toml:1-5`**

```text
members = [
    "meerkat-mobkit",
    "mobkit-store-conformance",
]
```

Both files named in the correction belong to active workspace members.

**`mobkit-store-conformance/Cargo.toml:37-44`**

```text
meerkat-core = { version = "=0.8.40" }
```

This crate directly declares five exact upstream requirements; updating only the main crate cannot move these declarations.

**`meerkat-mobkit/Cargo.toml:178-188`**

```text
meerkat-mob = { version = "=0.8.40", features = ["schema"] }
```

The main crate's dev-dependency section has additional exact pins, validating the explicit dev-dependency instruction.

## A-012: Root validation guidance calls local Make checks the full CI/all-tests gate although required suites are absent

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`README.md:185-188`**

````text
```bash
make ci                         # Full CI pipeline
make test                       # Rust tests
make test-python                # Python SDK tests
````

Contributors and coding agents can report full validation after make ci/make test-all while SDK, voice or console changes have not passed required gates. This is repeated in CLAUDE.md:52-53 ('Full CI') and CONTRIBUTING.md:57 ('Ensure all tests pass (make test-all)').

**`Makefile:79`**

```text
test-all: test test-python test-flow-editor ## Run all tests (Rust + Python + Flow Editor)
```

The local aggregate contains Rust, Python and Flow Editor tests, not every required hosted-CI suite. CONTRIBUTING.md:36 additionally labels this target 'Both', omitting its Flow Editor leg.

**`Makefile:128-129`**

```text
ci: fmt-check verify-version-parity bright-line memory-evals lint test-all audit ## Full CI pipeline
	@echo "$(GREEN)CI pipeline passed.$(NC)"
```

make ci builds on that limited aggregate. A read-only make -n ci confirmed no TypeScript validate or console suite invocation in its recipe expansion.

**`meerkat-mobkit/tests/sdk_parity.rs:203-211`**

```text
#[test]
#[ignore] // requires Python venv setup (~7s)
fn phase11_sdk_001_sdk_002_choke_110_and_e2e_1101_parity_contracts() {
    assert_command_success(
        "TypeScript",
        "validation",
        "npm",
        &["--prefix", "sdk/typescript", "run", "--silent", "validate"],
```

The apparent Rust-test route to the TypeScript suite is ignored by the ordinary nextest command. The similar sdk_productization.rs path is also #[ignore], so they do not make the local command equivalent.

**`.github/workflows/ci.yml:282-296`**

```text
    needs: [fmt-lint, test, test-voice, test-python, test-typescript, console, console-fixtures, console-acceptance, flow-editor, audit]
```

Hosted CI requires separate TypeScript, voice and console jobs; the gate explicitly checks each result. In particular, lines 266-267 execute TypeScript validate, while lines 163-185 execute console suites and browser acceptance that are not make ci recipes.

### Independent adjudication

I treated 'Full CI' charitably as shorthand for an aggregate, then checked whether the aggregate actually invokes the other required suites indirectly. The Make graph expands to Rust, Python and Flow Editor testing, not the TypeScript validation or console workflows. The two Rust tests that could invoke TypeScript validate are #[ignore]; ordinary nextest does not opt into them. Hosted CI also explicitly enables openai-live for its voice lane, absent from the local Make commands. Thus the documented commands are useful but not equivalent to the required hosted gate. This does not justify adding new test behavior or requiring every optional paid/live lane.

**`README.md:184-188`**

```text
make ci                         # Full CI pipeline
```

CLAUDE.md repeats '# Full CI' at lines 52-53; CONTRIBUTING.md calls test-all 'Both' at line 36 and presents it as all-tests validation at line 57.

**`Makefile:79-79`**

```text
test-all: test test-python test-flow-editor
```

This is the complete local test aggregate, including Flow Editor but not the SDK/console suites named separately in CI.

**`Makefile:128-129`**

```text
ci: fmt-check verify-version-parity bright-line memory-evals lint test-all audit
```

Independent 'make -n ci' expansion showed only the Rust nextest, Python pytest and four Flow Editor npm test commands; no SDK validate, console npm suite or openai-live flag appeared.

**`meerkat-mobkit/tests/sdk_parity.rs:203-211`**

```text
#[ignore] // requires Python venv setup (~7s)
```

This potentially countervailing indirect TypeScript validator is ignored. sdk_productization.rs:288-299 likewise marks its validate-calling test #[ignore].

**`.github/workflows/ci.yml:107-111`**

```text
      - run: scripts/repo-cargo test -p meerkat-mobkit --locked --features openai-live --bin rpc_gateway gateway_openai_live_registration
```

The dedicated required voice lane uses a feature/test invocation not performed by the Make aggregate.

**`.github/workflows/ci.yml:282-285`**

```text
    needs: [fmt-lint, test, test-voice, test-python, test-typescript, console, console-fixtures, console-acceptance, flow-editor, audit]
```

The hosted gate explicitly requires the additional suites, rather than treating them as optional informational jobs.

**Required correction:** In README.md and CLAUDE.md, label make ci as the local Make validation aggregate, not the complete hosted-CI pipeline. In CONTRIBUTING.md, label make test-all as Rust + Python + Flow Editor and qualify the PR validation instruction accordingly. Name 'npm --prefix sdk/typescript run validate' for TypeScript changes, and direct contributors to .github/workflows/ci.yml for the required console, voice and other hosted gates. Preserve the existing commands; do not modify runtime code or Make recipes as part of this documentation correction.

### Changes and final verification

**Changed:** `README.md`, `CLAUDE.md`, `CONTRIBUTING.md`.

Relabeled make ci as the local Make validation aggregate, not complete hosted CI. Corrected make test-all to Rust + Python + Flow Editor, added the TypeScript validate command, and pointed to .github/workflows/ci.yml for required console, voice, TypeScript, and other hosted gates. The PR checklist now distinguishes local validation from the hosted checks required before merging.

**Validation:** Read-only make -n ci assertions passed: local testing expands to Rust nextest, Python pytest, and Flow Editor scripts, without TypeScript validate, console scripts, or openai-live flags. ci.yml gate inspection confirmed the additional required jobs. The new relative CI links resolve and all commands remain existing repository commands; no Make recipe was modified.

**Final review: pass.** Every implicated root document now distinguishes the local Make aggregate from hosted CI. CONTRIBUTING names all three local test legs, the PR checklist no longer equates test-all with every required test, and all three documents include TypeScript validate plus a hosted-workflow reference. Independent make -n ci output and the actual gate job confirm the scope distinction. No Make recipe, test behavior or hosted gate was changed.

**`README.md:223-232`**

```text
make ci                         # Local Make validation aggregate
```

The block adds SDK validate; its following paragraph expressly says the aggregate is not complete hosted CI and links the required console/voice/TypeScript gates.

**`CLAUDE.md:52-61`**

```text
# Local Make validation aggregate
make ci
```

The agent guidance also adds TypeScript validate and the same local-versus-hosted qualification, so the original duplicated overclaim is not left behind.

**`CONTRIBUTING.md:36-50`**

```text
make test-all      # Rust + Python + Flow Editor
```

The Testing section lists the actual aggregate coverage, supplies SDK validate, and links the other required hosted checks.

**`CONTRIBUTING.md:64-65`**

```text
3. Run the applicable local checks above (`make test-all` covers Rust + Python + Flow Editor; TypeScript changes also need `npm --prefix sdk/typescript run validate`)
4. Open a PR against `main` with a description of changes; ensure the required hosted-CI gates pass before merging
```

The PR checklist is corrected as well as the command examples, avoiding an adjacent contradictory all-tests promise.

**`Makefile:79`**

```text
test-all: test test-python test-flow-editor ## Run all tests (Rust + Python + Flow Editor)
```

The actual dependency graph has these three legs; the ci aggregate at line 128 adds lint/parity/evals/audit, not the missing hosted suites. Independent make -n ci confirmed this expansion without running recipes.

**`.github/workflows/ci.yml:282-285`**

```text
    needs: [fmt-lint, test, test-voice, test-python, test-typescript, console, console-fixtures, console-acceptance, flow-editor, audit]
```

The hosted gate requires the additional named jobs. The voice feature invocation at lines 107-111, console commands at lines 163-185, and TypeScript validate at line 267 are actual implementations of the documented distinction.

## Independent scope checks

> [
>   "Read audit-brief.md, audit-scopes.json and all 12 audit-A.json findings. Independently reread the four implicated documentation files and their relevant implementation, manifests, scripts, tests and CI call sites.",
>   "git rev-parse HEAD returned af82b6b3ab34faed9bf3e962d148d55f10dcd1dc. git merge-base --is-ancestor 01435e7ccda5e925fc2cf9f462327bdae9584d07 HEAD returned status 1. Prior-work identity was not used to exclude any claim.",
>   "Read-only Python SDK init-payload reproduction, with PYTHONDONTWRITEBYTECODE=1 and python3 -B, confirmed missing config/mob.toml after builder validation and convention discovery. No gateway, provider call or state directory was created.",
>   "Read-only shell path reproduction confirmed that the SDK block's second cd resolves beneath sdk/python and fails.",
>   "make -n ci confirmed the local recipe graph without running builds, tests, installs or publishers.",
>   "Read-only TOML parsing counted 20 main-crate and five conformance-crate upstream dependency/dev-dependency requirements, all =0.8.40, and 35 locked upstream Meerkat packages, all 0.8.40."
> ]

## Final scope checks

> [
>   "Read audit-brief.md, audit-scopes.json, every audit-A finding, every adjudication-A decision/correction and fixes-A. The ledger has exactly 12 confirmed A IDs, zero rejected IDs and no supplemental A findings; all 12 are reviewed above.",
>   "Inspected the complete scope-A diff against af82b6b3ab34faed9bf3e962d148d55f10dcd1dc, the four final changed documents and their actual implementation/manifests/tests/workflows. Exactly README.md, CLAUDE.md, CONTRIBUTING.md and .claude/skills/mobkit-platform/SKILL.md differ in scope A.",
>   "PASS: git diff --check af82b6b3ab34faed9bf3e962d148d55f10dcd1dc -- README.md CLAUDE.md CONTRIBUTING.md CHANGELOG.md .claude. No staged scope-A changes.",
>   "PASS: independent PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=sdk/python python3 -B probe extracts README TOML/Python, compiles with top-level-await support, executes only imports/provider definitions and the builder expression without build(), checks three matching profiles/addressability values and two canonical peer edges, calls builder validation, then verifies exact mocked config-file serialization and init flags. No gateway, store creation or model-provider call.",
>   "The first README probe asserted directed endpoint order and failed because ManagedPeerEdge canonicalizes undirected pairs. Inspected identity_first_models.py:579-587, corrected the review-only assertion to sorted endpoint pairs and reran successfully. This was a reviewer-harness assumption, not a repository or documentation defect.",
>   "PASS: independent python3 -B TOML/AST assertions confirm Rust 1.97.0 development pin, Rust 1.94.0 inherited MSRV, SDK Python >=3.10, unconditional release-script tomllib import, 20 main-crate plus 5 conformance-crate exact upstream requirements, and 35 coherent locked upstream packages at 0.8.40. Python 3.10 itself was not executed.",
>   "PASS: eight affected development/testing Bash blocks parse via bash -n on stdin. Every referenced npm script in those blocks exists. SDK validate is exactly typecheck then build then test; source tests import dist. No build/test script was executed for this syntax/inventory check.",
>   "PASS: three release Bash blocks parse after replacing the explicit <version> metavariable with a sample version. The release-dry-run Make target and cargo-release pre-release-hook configuration exist. No release hook, staging, tag, candidate dispatch, publisher or registry operation ran.",
>   "PASS: make -n ci contains Rust nextest, Python pytest and Flow Editor testing, but no SDK validate, console npm suite or openai-live feature lane. Read ci.yml's required gate and individual voice/console/TypeScript jobs to verify the new distinction.",
>   "PASS: direct source verification of local filesystem blob layout and SQLite state paths, conditional AccessController persistence and successful-revision ordering, configured ancestor-first grouping versus row-local badges, verifier versus generator coverage, and release-hook generation/staging.",
>   "PASS: eight local Markdown/HTML targets across the four edited files exist; the added README #install anchor resolves; Markdown fences are balanced. New CI workflow links resolve to the actual checked workflow.",
>   "PASS historical preservation: CHANGELOG.md and .claude/commands/ship.md are byte-identical to the baseline. All three tracked external skill symlink aliases have unchanged target strings. External referents were not followed or edited. No dated changelog content was reclassified as a current promise.",
>   "Reviewed snapshots (SHA-256): README.md=6e3a158393fc2e3afa627c3b1329d00c5da6698657bac44675fa833e89afbbf9; CLAUDE.md=40f32c1ffb33f018bd1299cb4d682ae37f2324bec367e5beafb4267c2782337b; CONTRIBUTING.md=4141ca7003e8fa831172cb3e8b48c462a9f151c253d48b59cc5a31ee51791b49; .claude/skills/mobkit-platform/SKILL.md=75ecb8a57a455f0ea7e83052430434e26667a33c76876bd604d2667bcab2582e.",
>   "Limits: documentation-only independent review and read-only validation. Full runtime/SDK/console suites, live credentials, release publication and external symlink contents were not exercised. No repository files or git state were changed; only this requested review artifact was written. No delegation."
> ]
