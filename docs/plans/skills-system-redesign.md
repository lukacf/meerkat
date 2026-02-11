# Skills System Redesign

## Status: Draft v4 — Architectural & Usage Overview

## Problem Statement

The current skill system does Level 1 (metadata in system prompt) but Level 2 (on-demand
body loading when triggered) is unimplemented. `resolve_and_render()` is never called outside
tests. There is no way to inject skills programmatically (SDK), no remote/API skill source,
no per-turn activation, and no organizational structure beyond a flat list.

## Design Goals

1. **Spec-compliant** — individual skills follow the [Agent Skills Standard](https://agentskills.io/specification) (SKILL.md format, naming rules)
2. **Layered** — from barebones in-memory to managed remote sources
3. **Hierarchical** — namespaced skill IDs with collection-level browsing
4. **Per-turn activation** — skills can be triggered mid-conversation
5. **Pre-loadable** — SDK users can force-load skills at build time
6. **Non-bloating** — new sources behind feature flags, core trait stays lean

---

## Canonical ID Format & Collections

### Skill IDs

The **canonical skill ID** is a slash-delimited path: `{collection-path}/{name}`.

```
extraction/email-extractor                ← collection: "extraction"
extraction/fiction-extractor              ← collection: "extraction"
extraction/medical/diagnosis              ← collection: "extraction/medical"
extraction/medical/imaging/ct-scan        ← collection: "extraction/medical/imaging"
formatting/markdown-output                ← collection: "formatting"
pdf-processing                            ← collection: (root)
```

Rules:
- The **last path segment** is the spec-compliant `name` (lowercase alphanumeric + hyphens,
  max 64 chars, matching the parent directory name per Agent Skills spec).
- Everything before the last `/` is the **collection path** (arbitrary depth).
- Skills without a `/` are **root-level** (no collection).
- The canonical ID is used **everywhere**: core types, HTTP API, wire params,
  tool parameters, user `/skill-ref` syntax. No mapping, no translation.
- Convention: prefer shallow nesting (1-2 levels). Deep hierarchies are
  supported but not encouraged.

### Skill References (user-facing)

User-facing skill references use a leading `/` as the trigger character:

```
/extraction/email-extractor              ← collection + name
/extraction/medical/diagnosis            ← nested collection + name
/pdf-processing                          ← root-level skill
```

The resolver:
1. Strips the leading `/`
2. Looks up the remainder as a `SkillId`
3. No ambiguity: the reference IS the ID

Bare name references (without `/` prefix) are NOT supported — they're
ambiguous and would require fuzzy matching across collections.

### Collection Derivation

Collections are **derived at runtime** from the set of skill IDs, not stored
as separate entities. The `collections()` method scans all skill IDs, extracts
unique parent paths (everything before the last `/`), and counts skills per path.

For browsing, collections form a tree. Browsing a path shows:
- **Direct skills** at that level
- **Subcollections** (child prefixes with their own skills)

Example with these skill IDs:
```
extraction/email-extractor
extraction/fiction-extractor
extraction/medical/diagnosis
extraction/medical/imaging/ct-scan
formatting/markdown-output
pdf-processing
```

Browsing root (`""`):
- Subcollections: `extraction/` (4 skills total), `formatting/` (1)
- Direct skills: `pdf-processing`

Browsing `extraction`:
- Subcollections: `extraction/medical/` (2 skills total)
- Direct skills: `extraction/email-extractor`, `extraction/fiction-extractor`

Browsing `extraction/medical`:
- Subcollections: `extraction/medical/imaging/` (1 skill)
- Direct skills: `extraction/medical/diagnosis`

Collection descriptions come from:
- **Filesystem/git sources**: optional `COLLECTION.md` file in the collection directory
  (single line of text, no frontmatter). If absent, auto-generated as
  `"{count} skills"`.
- **HTTP sources**: the API response includes collection descriptions.
- **In-memory / embedded**: auto-generated from prefix.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Skill Authoring                              │
│                                                                     │
│  Git Repo              Filesystem            API / Dashboard        │
│  skills/               .rkat/skills/         POST /skills           │
│    extraction/           my-skill/           PUT  /skills/{id}      │
│      email/SKILL.md       SKILL.md                                  │
│      fiction/SKILL.md                                               │
└────────┬──────────────────┬──────────────────────┬──────────────────┘
         │                  │                      │
         ▼                  ▼                      ▼
┌─────────────────────────────────────────────────────────────────────┐
│                    SkillSource Implementations                      │
│                                                                     │
│  ┌─────────────┐ ┌──────────────┐ ┌──────────┐ ┌───────────────┐  │
│  │ Filesystem   │ │ Embedded     │ │ InMemory │ │ Http          │  │
│  │ SkillSource  │ │ SkillSource  │ │ Skill    │ │ SkillSource   │  │
│  │              │ │ (inventory)  │ │ Source   │ │               │  │
│  │ .rkat/skills │ │              │ │          │ │ Meerkat wire  │  │
│  │ ~/.rkat/     │ │ compile-time │ │ runtime  │ │ format        │  │
│  └──────┬───────┘ └──────┬───────┘ └────┬─────┘ └──────┬────────┘  │
│         └────────────────┴──────────────┴──────────────┘           │
│                              │                                      │
│                    ┌─────────▼──────────┐                          │
│                    │ CompositeSkill     │  precedence layering      │
│                    │ Source             │                           │
│                    └─────────┬──────────┘                          │
│                              │                                      │
└──────────────────────────────┼──────────────────────────────────────┘
                               │
                    ┌──────────▼──────────┐
                    │  DefaultSkillEngine │
                    │                     │
                    │  • collections()    │
                    │  • inventory XML    │
                    │  • resolve & render │
                    │  • pre-load         │
                    └──────────┬──────────┘
                               │
              ┌────────────────┼────────────────┐
              │                │                │
              ▼                ▼                ▼
    ┌─────────────┐  ┌────────────────┐  ┌──────────────┐
    │ System      │  │ browse_skills  │  │ Per-turn     │
    │ Prompt      │  │ load_skill     │  │ /skill-ref   │
    │ Inventory   │  │ builtin tools  │  │ processing   │
    │ (XML)       │  │                │  │              │
    └─────────────┘  └────────────────┘  └──────────────┘
```

---

## Core Types (meerkat-core)

### SkillSource trait (revised)

```rust
#[async_trait]
pub trait SkillSource: Send + Sync {
    /// List skill descriptors, optionally filtered by collection prefix.
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError>;

    /// Load a skill document by its canonical ID.
    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError>;

    /// List collections with counts.
    /// Default implementation derives collections from skill ID prefixes.
    async fn collections(&self) -> Result<Vec<SkillCollection>, SkillError> {
        let all = self.list(&SkillFilter::default()).await?;
        Ok(derive_collections(&all))
    }
}
```

### New types

```rust
/// Filter for listing skills.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillFilter {
    /// Segment-aware recursive prefix filter: return all skills whose
    /// collection path starts with this value at a `/` boundary.
    ///
    /// "extraction" matches:
    ///   extraction/email-extractor         ✓ (direct child)
    ///   extraction/medical/diagnosis       ✓ (nested descendant)
    /// "extraction" does NOT match:
    ///   extract/something                  ✗ (different segment)
    ///   extractions/foo                    ✗ (different segment)
    ///
    /// Implementation: match when skill's collection path == filter
    /// OR skill's collection path starts with "{filter}/".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    /// Free-text search across name + description (all collections).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}
```

**`SkillFilter.collection` is recursive** (prefix match). The `browse_skills`
tool needs **direct-level** listing (immediate children only). This split is
intentional — it keeps `SkillSource.list()` simple and puts the partitioning
logic in `DefaultSkillEngine`:

```
browse_skills(path: "extraction"):
  1. engine calls source.list(filter: { collection: "extraction" })
     → returns all descendants under extraction/
  2. Engine partitions results:
     - Direct skills: those whose collection path == "extraction" exactly
       (ID has exactly one more segment after "extraction/")
     - Subcollections: unique next-level prefixes from deeper skills
       (e.g. "extraction/medical" derived from "extraction/medical/diagnosis")
  3. Returns { subcollections, skills }
```

```rust
/// A skill collection (derived from namespaced IDs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCollection {
    /// Collection path prefix (e.g. "extraction").
    pub path: String,
    /// Human-readable description.
    pub description: String,
    /// Number of skills in this collection.
    pub count: usize,
}
```

### SkillDescriptor changes

```rust
pub struct SkillDescriptor {
    /// Canonical namespaced ID: "extraction/email-extractor".
    /// This is the ONLY identifier used across all layers.
    pub id: SkillId,
    /// Spec-compliant flat name: "email-extractor" (last segment of ID).
    pub name: String,
    pub description: String,
    pub scope: SkillScope,
    pub requires_capabilities: Vec<String>,
    /// Extensible metadata (from SKILL.md frontmatter `metadata:` field).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub metadata: IndexMap<String, String>,
    /// Repository name this skill came from (e.g. "company", "project").
    /// Populated by CompositeSkillSource from the NamedSource wrapper.
    /// Empty string for sources used outside CompositeSkillSource.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_name: String,
}
```

This is the **single authoritative definition**. The `source_name` field is set
by `CompositeSkillSource` when merging named sources. Individual `SkillSource`
implementations leave it empty — the composite populates it.

### SkillEngine trait (revised)

```rust
#[async_trait]
pub trait SkillEngine: Send + Sync {
    /// Generate the system prompt inventory (XML, compact).
    async fn inventory_section(&self) -> Result<String, SkillError>;

    /// Resolve skill references and render injection content.
    /// References are canonical IDs (e.g. "extraction/email-extractor").
    async fn resolve_and_render(
        &self,
        ids: &[SkillId],
    ) -> Result<Vec<ResolvedSkill>, SkillError>;

    /// List collections (delegates to source).
    async fn collections(&self) -> Result<Vec<SkillCollection>, SkillError>;

    /// List skills with optional filter (for browse_skills tool).
    async fn list_skills(
        &self,
        filter: &SkillFilter,
    ) -> Result<Vec<SkillDescriptor>, SkillError>;
}

/// A resolved skill ready for injection.
#[derive(Debug, Clone)]
pub struct ResolvedSkill {
    pub id: SkillId,
    pub name: String,
    /// The rendered <skill> XML block, sanitized and size-limited.
    pub rendered_body: String,
    pub byte_size: usize,
}
```

---

## Skill Repositories & Configuration

### Concept

A **skill repository** is a named, configured skill source. Multiple repositories
merge into one unified namespace via `CompositeSkillSource`. The repository name
is metadata for humans (browsing, tracing, debugging) — not part of the skill ID.

This follows the package manager model: you configure repositories (like apt sources
or npm registries), skills from all of them merge, and precedence resolves conflicts.

### Configuration file: `.rkat/skills.toml`

Follows the same pattern as `.rkat/mcp.toml` — project-level and user-level with
precedence merging.

```toml
# .rkat/skills.toml
enabled = true
max_injection_bytes = 32768
inventory_threshold = 12

[[repositories]]
name = "project"
type = "filesystem"
path = ".rkat/skills"

[[repositories]]
name = "company"
type = "git"
url = "https://github.com/company/skills.git"
git_ref = "v1.2.0"
ref_type = "tag"
auth_token = "${GITHUB_TOKEN}"

[[repositories]]
name = "elephant"
type = "http"
url = "http://elephant:8080/api"
auth_header = "X-API-Key"
auth_token = "${ELEPHANT_API_KEY}"
refresh_seconds = 60

# Embedded skills are always present as lowest precedence (no config entry needed).
```

**Merge algorithm** (same as `McpConfig`): project repositories are prepended
before user repositories. Within each file, declaration order is preserved.
Repositories with duplicate names are deduplicated (first occurrence wins).

```
Project .rkat/skills.toml:        [project, company]
User    ~/.rkat/skills.toml:      [company, personal]

Merged result:                    [project, company, personal, (embedded)]
                                   ↑ project's "company" wins, user's is dropped
```

The merged list becomes the source precedence for `CompositeSkillSource`.
Embedded skills are always appended last (lowest precedence).

When no `skills.toml` exists, the default chain applies:
```
1. Project filesystem  (.rkat/skills/)     ← highest
2. User filesystem     (~/.rkat/skills/)
3. Embedded (inventory)                    ← lowest
```

### Configuration types

**Lives in `meerkat-core`** (following the current pattern where `McpServerConfig`
lives in `meerkat-core`). See technical debt note below.

```rust
/// Complete skills configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    pub enabled: bool,
    pub max_injection_bytes: usize,
    /// Inventory mode threshold. When total skill count <= this value,
    /// the system prompt uses flat skill listing. When > this value,
    /// it uses collection summary mode. Default: 12.
    pub inventory_threshold: usize,
    #[serde(default)]
    pub repositories: Vec<SkillRepositoryConfig>,
}

/// A named skill repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRepositoryConfig {
    /// Human-readable name (used in browsing, tracing, shadowing logs).
    pub name: String,
    /// Repository type and transport-specific config.
    #[serde(flatten)]
    pub transport: SkillRepoTransport,
}

/// Transport configuration for a skill repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SkillRepoTransport {
    Filesystem {
        path: String,
    },
    Http {
        url: String,
        #[serde(default)]
        auth_header: Option<String>,
        #[serde(default)]
        auth_token: Option<String>,
        #[serde(default = "default_refresh_seconds")]
        refresh_seconds: u64,
    },
    Git {
        url: String,
        #[serde(default = "default_git_ref")]
        git_ref: String,       // "main", "v1.2.0", or commit SHA
        #[serde(default)]
        ref_type: GitRefType,  // branch, tag, or commit
        #[serde(default)]
        skills_root: Option<String>,
        #[serde(default)]
        auth_token: Option<String>,
        #[serde(default)]
        ssh_key: Option<String>,
        #[serde(default = "default_refresh_seconds")]
        refresh_seconds: u64,
        #[serde(default = "default_clone_depth")]
        depth: Option<usize>,
    },
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitRefType {
    #[default]
    Branch,
    Tag,
    Commit,
}
```

### Resolution: config → SkillSource

`meerkat-skills` provides a single function that all surfaces call:

```rust
/// Resolve skill repository config into a composed SkillSource.
///
/// This is the single canonical path for turning configuration into
/// a wired skill source. CLI, REST, RPC, MCP Server all call this.
pub async fn resolve_repositories(
    config: &SkillsConfig,
    project_root: Option<&Path>,
) -> Result<Arc<dyn SkillSource>, SkillError>
```

This function:
1. Iterates `config.repositories` in order
2. Constructs the appropriate `SkillSource` for each (Filesystem, Http, Git)
3. Appends `EmbeddedSkillSource` as the lowest precedence
4. Wraps in `CompositeSkillSource` with named sources
5. Returns the composite

When `config.repositories` is empty, falls back to the default chain
(project FS → user FS → embedded).

### SDK bypass

SDK users skip config entirely — pass `skill_source` on the factory:

```rust
let factory = AgentFactory::new("/tmp/store")
    .skill_source(my_custom_source);   // bypasses .rkat/skills.toml
```

When `factory.skill_source` is set, `resolve_repositories()` is not called.

### Config loading

Follows the `McpConfig` pattern:

```rust
impl SkillsConfig {
    /// Load from user + project config, project wins.
    pub async fn load() -> Result<Self, SkillsConfigError>;

    /// Load from explicit paths (testing).
    pub async fn load_from_paths(
        user_path: Option<&Path>,
        project_path: Option<&Path>,
    ) -> Result<Self, SkillsConfigError>;
}
```

Env var expansion (`${GITHUB_TOKEN}`) supported for `auth_token` fields,
following the same `expand_env_in_string` pattern as MCP config.

### Source Precedence & Shadowing

`CompositeSkillSource` layers named sources with explicit precedence.
When multiple sources provide the same `SkillId`, the **first source wins**.

Each source carries its `name` from config. Shadowing is observable:
```
tracing::info!(
    skill_id = %id,
    shadowed_source = shadowed_name,
    winning_source = winning_name,
    "Skill shadowed by higher-precedence repository"
);
```

`SkillDescriptor.source_name` (defined in the core types section) carries
provenance so browse results show which repository a skill came from.

### Technical Debt Note

> **Config placement**: `SkillsConfig` and `SkillRepositoryConfig` currently live
> in `meerkat-core` following the precedent set by `McpServerConfig`. This is
> recognized as technical debt — ideally each optional crate would own its config
> types, with `meerkat-core` providing only the configuration *system* (loading,
> layering, env expansion) and core-specific settings. A future config refactor
> should move `McpServerConfig` to `meerkat-mcp`, `SkillRepositoryConfig` to
> `meerkat-skills`, and have the core provide a generic config registry. This
> refactor is out of scope for the skills redesign but should be tracked.

---

## Skill Sources (meerkat-skills)

### Existing (updated)

| Source | Changes |
|--------|---------|
| `FilesystemSkillSource` | Recursive scan for arbitrary-depth nested directories; derive namespaced IDs from relative paths; optional `COLLECTION.md` at any collection level |
| `EmbeddedSkillSource` | Updated for `list(filter)` signature; IDs remain flat (root-level) |
| `InMemorySkillSource` | Updated for `list(filter)` signature |
| `CompositeSkillSource` | Updated for `list(filter)` signature; named sources (`NamedSource { name, source }`); shadowing tracing with source names; `source_name` populated on descriptors |

### New: `HttpSkillSource` (feature-gated)

HTTP client implementing `SkillSource`. Uses Meerkat's own wire format
(not Anthropic's — their schema lacks skill body content, collection
structure, and uses incompatible ID formats).

```rust
/// HTTP skill source.
///
/// Feature: `skills-http`
/// Requires: `reqwest` (already in workspace)
pub struct HttpSkillSource {
    /// API root URL, e.g. "http://localhost:8080" or "https://api.example.com".
    /// Endpoints are appended as: {base_url}/skills, {base_url}/skills/{id}.
    base_url: Url,
    client: reqwest::Client,
    auth: Option<HttpSkillAuth>,
    cache: RwLock<SkillCache>,
    cache_ttl: Duration,
}

/// Authentication for HTTP skill sources.
pub enum HttpSkillAuth {
    /// Bearer token: sends `Authorization: Bearer {token}`.
    Bearer(String),
    /// Custom header: sends `{name}: {value}` (e.g. X-API-Key).
    Header { name: String, value: String },
}
```

**URL construction**: `base_url` is the API root (no trailing path component).
Endpoints are relative:
- `{base_url}/skills` — list skills
- `{base_url}/skills/{id}` — load skill (ID is URL-encoded: `extraction%2Femail-extractor`)
- `{base_url}/skill-collections` — list collections

The collections endpoint is `/skill-collections` (not `/skills/collections`)
to avoid a namespace collision with a root-level skill named `collections`.

**Cache design**: The cache stores the **full unfiltered skill list** from the
last `GET /skills` call. Filtered `list()` calls apply the filter client-side
against the cached list. Individual `load()` calls are cached per-ID. Both
expire after `cache_ttl`.

**Cache refresh rule**: on cache miss or TTL expiry, the refresh request MUST
call `GET /skills` with **no query parameters** to fetch the complete unfiltered
list. A filtered `list(collection: "extraction")` call that triggers a cache
refresh still fetches the full list, caches it, then applies the filter
client-side. This prevents a filtered response from poisoning the cache
(which would cause subsequent unfiltered or differently-filtered calls to
return incomplete results).

```rust
struct SkillCache {
    /// Full unfiltered descriptor list. Always fetched via GET /skills (no params).
    descriptors: Option<(Vec<SkillDescriptor>, Instant)>,
    /// Per-ID document cache. Fetched via GET /skills/{id}.
    documents: HashMap<SkillId, (SkillDocument, Instant)>,
}
```

This avoids the stale-filter problem (a single cache, not per-query) and
keeps the number of HTTP requests minimal.

### Wire Format (Meerkat Skills API)

Our own format. Servers implementing this contract (Elephant, future registry)
can be consumed by `HttpSkillSource`.

**List skills**

```
GET /skills
  ?collection=extraction       (optional: filter by collection prefix)
  ?query=email                 (optional: text search)

Response 200:
{
  "skills": [
    {
      "id": "extraction/email-extractor",
      "name": "email-extractor",
      "description": "Extract entities from emails",
      "scope": "project",
      "metadata": {}
    }
  ]
}
```

**Get skill (with body)**

```
GET /skills/{id}
  where {id} is URL-encoded canonical ID (e.g. extraction%2Femail-extractor)

Response 200:
{
  "id": "extraction/email-extractor",
  "name": "email-extractor",
  "description": "Extract entities from emails",
  "scope": "project",
  "metadata": {},
  "body": "# Email Entity Extractor\n\nExtract the following..."
}
```

**List collections**

```
GET /skill-collections

Response 200:
{
  "collections": [
    {
      "path": "extraction",
      "description": "Entity and relationship extraction",
      "count": 8
    },
    {
      "path": "formatting",
      "description": "Output formatting and templates",
      "count": 3
    }
  ]
}
```

The API is read-only from Meerkat's perspective. CRUD (create/update/delete)
is the server's own concern — Elephant exposes those on its own terms.

### `GitSkillSource` (feature-gated)

Git repository as a skill library. Clones/pulls a repo into a local cache
directory, delegates to `FilesystemSkillSource` for parsing and collection
derivation. The git repo's directory structure IS the skill hierarchy.

```rust
/// Git-backed skill source.
///
/// Feature: `skills-git`
/// Requires: `gix` (gitoxide — pure Rust git, no libgit2/CLI dependency)
pub struct GitSkillSource {
    config: GitSkillConfig,
    /// Local clone used as the source of truth between refreshes.
    cache_dir: PathBuf,
    /// Delegates all SkillSource operations after sync.
    inner: RwLock<Option<FilesystemSkillSource>>,
    /// Tracks last successful sync for TTL-based refresh.
    last_sync: RwLock<Option<Instant>>,
}

pub struct GitSkillConfig {
    /// Remote repository URL.
    /// Supports HTTPS (`https://github.com/org/skills.git`) and
    /// SSH (`git@github.com:org/skills.git`).
    pub repo_url: String,

    /// Branch, tag, or commit SHA to track.
    /// Default: "main".
    pub git_ref: GitRef,

    /// Local directory for the clone.
    /// Default: `{system_cache_dir}/rkat/skill-repos/{repo_hash}/`.
    /// The repo_hash is derived from (repo_url, git_ref) to allow
    /// multiple refs from the same repo without conflicts.
    pub cache_dir: Option<PathBuf>,

    /// Subdirectory within the repo to scan for skills.
    /// Default: repo root.
    /// Use when skills live in a subdirectory (e.g. "skills/" or "src/skills/").
    pub skills_root: Option<String>,

    /// How often to pull for updates.
    /// Default: 5 minutes.
    /// Set to Duration::MAX to disable automatic refresh (manual only).
    pub refresh_interval: Duration,

    /// Authentication for private repos.
    pub auth: Option<GitSkillAuth>,

    /// Shallow clone depth. Reduces clone time and disk usage.
    /// Default: Some(1) (only latest commit).
    /// Set to None for full history (needed for commit SHA pinning).
    pub depth: Option<usize>,
}

/// Git ref to track.
pub enum GitRef {
    /// Track a branch (pulls latest on refresh). Default: "main".
    Branch(String),
    /// Pin to a tag (immutable — no refresh needed after initial clone).
    Tag(String),
    /// Pin to an exact commit SHA (immutable — no refresh needed).
    Commit(String),
}

impl Default for GitRef {
    fn default() -> Self {
        GitRef::Branch("main".into())
    }
}

/// Authentication for git operations.
pub enum GitSkillAuth {
    /// HTTPS token (used as password with empty username, or as
    /// `x-access-token` for GitHub Apps).
    HttpsToken(String),
    /// SSH key path (for git@ URLs).
    SshKey {
        private_key_path: PathBuf,
        passphrase: Option<String>,
    },
}
```

**Lifecycle:**

```
Construction:
  1. Validate config (repo_url format, cache_dir writable)
  2. Do NOT clone yet — lazy initialization on first access

First access (any SkillSource method):
  1. Clone repo to cache_dir (shallow if depth is set)
  2. Checkout the configured git_ref
  3. If skills_root is set, validate the subdirectory exists
  4. Construct FilesystemSkillSource pointing at cache_dir/skills_root
  5. Store in inner RwLock, record last_sync time
  6. Delegate the call to inner FilesystemSkillSource

Subsequent access:
  1. If last_sync + refresh_interval has elapsed → refresh
  2. Otherwise → delegate to cached FilesystemSkillSource

Refresh:
  1. For Branch refs: git pull (fast-forward only)
  2. For Tag/Commit refs: no-op (immutable — skip refresh entirely)
  3. If pull succeeds: rebuild FilesystemSkillSource from updated checkout
  4. If pull fails (network error, auth error):
     - Log warning with error details
     - Continue serving from stale cache (last successful sync)
     - Retry on next access after refresh_interval
  5. Record new last_sync time only on success
```

**Error handling:**

| Scenario | Behavior |
|----------|----------|
| Clone fails on first access | `SkillError::Load("git clone failed: {error}")`. Source returns errors until clone succeeds. |
| Pull fails on refresh | `tracing::warn!`. Continue serving from stale cache. |
| Repo exists but `skills_root` missing | `SkillError::Load("skills_root '{path}' not found in repo")`. |
| Invalid git_ref (branch/tag doesn't exist) | `SkillError::Load("ref '{ref}' not found in repo")`. |
| Repo has deep nesting | Supported — arbitrary depth delegated to `FilesystemSkillSource`. |

**Versioning via git refs:**

Git refs provide natural versioning without a separate version management system:

```rust
// Track latest on main — always up to date
let source = GitSkillSource::new(GitSkillConfig {
    repo_url: "https://github.com/company/skills.git".into(),
    git_ref: GitRef::Branch("main".into()),
    refresh_interval: Duration::from_secs(300),
    ..Default::default()
});

// Pin to a release tag — immutable, no refresh overhead
let source = GitSkillSource::new(GitSkillConfig {
    repo_url: "https://github.com/company/skills.git".into(),
    git_ref: GitRef::Tag("v2.1.0".into()),
    ..Default::default()
});

// Pin to exact commit — maximum reproducibility
let source = GitSkillSource::new(GitSkillConfig {
    repo_url: "https://github.com/company/skills.git".into(),
    git_ref: GitRef::Commit("a1b2c3d4".into()),
    depth: None,  // full history needed for arbitrary commit checkout
    ..Default::default()
});
```

**Expected repo structure:**

```
company-skills/              (or any subdirectory via skills_root)
  extraction/                ← collection
    COLLECTION.md            ← optional collection description
    email-extractor/         ← skill directory
      SKILL.md               ← required (spec-compliant)
      scripts/               ← optional
      references/            ← optional
    fiction-extractor/
      SKILL.md
  formatting/                ← collection
    COLLECTION.md
    markdown-output/
      SKILL.md
  pdf-processing/            ← root-level skill (no collection)
    SKILL.md
```

All parsing, ID derivation, collection detection, and validation rules are
delegated to `FilesystemSkillSource`. `GitSkillSource` only handles clone,
refresh, and cache management.

**Composition with other sources:**

`GitSkillSource` can be layered in a `CompositeSkillSource` like any other source:

```rust
// Local project skills override git repo skills
let composite = CompositeSkillSource::new(vec![
    Box::new(FilesystemSkillSource::new(
        ".rkat/skills".into(),
        SkillScope::Project,
    )),
    Box::new(GitSkillSource::new(GitSkillConfig {
        repo_url: "https://github.com/company/skills.git".into(),
        ..Default::default()
    })),
]);

let factory = AgentFactory::new("/tmp/store")
    .skill_source(Arc::new(composite));
```

**Why `gix` (gitoxide) over shelling out to `git`:**
- Pure Rust: no runtime dependency on git CLI being installed
- No subprocess spawning: cleaner error handling, no PATH issues in containers
- Async-friendly: better integration with tokio runtime
- Already used in the Rust ecosystem (cargo uses it)

---

## Renderer (spec-compliant XML)

### Inventory mode (system prompt)

For small skill sets (≤ threshold, default 12), flat listing:

```xml
<available_skills>
  <skill id="extraction/email-extractor">
    <description>Extract entities from emails</description>
  </skill>
  <skill id="extraction/fiction-extractor">
    <description>Extract characters from fiction</description>
  </skill>
</available_skills>
```

For larger sets, collection summary with tool hint:

```xml
<available_skills mode="collections">
  <collection path="extraction" count="8">Entity and relationship extraction</collection>
  <collection path="formatting" count="3">Output formatting and templates</collection>
  <collection path="analysis" count="5">Text analysis and classification</collection>

  Use the browse_skills tool to list skills in a collection or search.
  Use the load_skill tool or /collection/skill-name to activate a skill.
</available_skills>
```

The `mode` attribute makes it unambiguous whether the block contains
skills or collections.

### Injection mode (per-turn / pre-load)

```xml
<skill id="extraction/email-extractor">
Extract the following from email messages:
- People (sender, recipients, mentioned)
- Organizations
- Dates and deadlines
- Action items
</skill>
```

**Sanitization rules:**
- These tags are consumed by the LLM, not an XML parser. The only structural
  risk is the body containing a literal `</skill>` which would prematurely
  close the wrapper boundary.
- **Escaping**: the renderer scans the body for any closing-tag form matching
  the regex `</skill\s*>` (case-insensitive) and replaces occurrences with
  `<\/skill>`. This covers `</skill>`, `</SKILL>`, `</skill  >`, etc.
  Escaping is applied **before** truncation (so a truncated body never ends
  mid-escape). No other escaping needed — other XML-like content (`<div>`,
  `&amp;`) is left as-is. The LLM understands the boundary from the
  `<skill id="...">` / `</skill>` delimiters without requiring valid XML.
- Maximum injection size: `SkillsConfig.max_injection_bytes` (default 32 KiB).
  Bodies exceeding this are truncated with a trailing `[truncated]` marker.
  The existing `MAX_INJECTION_BYTES` constant in `renderer.rs` already
  enforces this — the truncation behavior is preserved.
- **Trust model**: filesystem and embedded sources are trusted (local).
  HTTP sources should be treated as semi-trusted — the size limit provides
  the primary guardrail. A future `SkillTrust` enum on `SkillScope` could
  add stricter sandboxing for remote skills.

---

## Factory Wiring

### AgentFactory changes

```rust
pub struct AgentFactory {
    // ... existing fields ...

    /// Optional skill source override. When set, bypasses config-driven
    /// repository resolution entirely. For SDK users who wire sources
    /// programmatically.
    pub skill_source: Option<Arc<dyn SkillSource>>,
}

impl AgentFactory {
    /// Set a custom skill source (bypasses .rkat/skills.toml resolution).
    pub fn skill_source(mut self, source: Arc<dyn SkillSource>) -> Self {
        self.skill_source = Some(source);
        self
    }
}
```

### AgentBuildConfig changes

```rust
pub struct AgentBuildConfig {
    // ... existing fields ...

    /// Skills to pre-load at build time (full body injected into system prompt).
    /// `None` = metadata-only inventory (agent discovers and loads via tools).
    /// `Some(ids)` = pre-load these skills AND include inventory for remaining
    ///   skills. Discovery tools (`browse_skills`, `load_skill`) are always
    ///   registered — pre-loading doesn't disable runtime discovery.
    pub preload_skills: Option<Vec<SkillId>>,
}
```

### Wire format alignment

```rust
/// Skills parameters for session/turn requests.
pub struct SkillsParams {
    /// Pre-load these skills at session creation.
    /// None or empty = inventory-only mode (no pre-loading).
    /// Non-empty = pre-load these skill IDs into the system prompt.
    /// `Some([])` is normalized to `None` to prevent silent misconfiguration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preload_skills: Option<Vec<String>>,

    /// Skill IDs to resolve and inject for this turn.
    /// None or empty = no per-turn skill injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_references: Option<Vec<String>>,
}
```

Both `preload_skills` and `skill_references` use `Option<Vec<String>>`
to preserve None vs empty semantics across the wire boundary. Surface
crates map `None` → `None` and `Some(vec)` → `Some(vec.map(SkillId))`.

### Build flow (factory.rs step 11, revised)

```
0. Early gate: if config.skills.enabled == false AND factory.skill_source is None:
   - Skip ALL skill machinery. No source resolution, no inventory, no tools.
   - Set active_skill_ids = None, inventory_section = "".
   - Proceed to step 12 (system prompt assembly).
   Note: factory.skill_source being set is an explicit SDK override that
   implies intent — it bypasses the enabled flag.

1. Resolve skill source:
   - If factory.skill_source is set → use it (SDK override, ignores enabled flag)
   - Else → meerkat_skills::resolve_repositories(&config.skills, project_root)
     Reads .rkat/skills.toml, constructs sources per config, wraps in composite.
     Falls back to default chain (project FS → user FS → embedded) if no config.

2. Build SkillEngine from resolved source.

3. Normalize preload_skills: Some([]) → None.

4. Generate inventory section (always):
   - engine.inventory_section() → XML metadata for all available skills
   - Inject compact <available_skills> into system prompt (Level 1)

5. If preload_skills is Some(ids):
   - engine.resolve_and_render(ids) → get ResolvedSkill bodies
   - If any ID fails to resolve → BuildAgentError (fail the build).
     Pre-loaded skills are explicitly requested; missing = misconfiguration.
   - Inject full <skill> blocks into system prompt (Level 2 at build time)
   - Pre-loaded skills are ALSO in the inventory (marked as active)

6. Register browse_skills + load_skill tools (always when step 0 was not
   taken). Pre-loading does not disable runtime discovery.

7. Store Arc<dyn SkillEngine> for per-turn resolution + tool access.
```

### Failure semantics

| Path | Missing/invalid skill ID | Behavior |
|------|--------------------------|----------|
| **Pre-load** (`preload_skills`) | Skill not found | `BuildAgentError::SkillNotFound`. Fail the build. Caller explicitly requested this skill — missing is a misconfiguration. |
| **Per-turn** (`/skill-ref`) | Skill not found | Emit `SkillResolutionFailed` event. Turn proceeds without injection. Error message returned to the user (e.g. "Skill 'foo/bar' not found"). |
| **`load_skill` tool** | Skill not found | Tool returns error result. Agent decides how to proceed (may inform user, try alternatives). |
| **`browse_skills` tool** | Empty collection | Returns empty list. Not an error. |

---

## Builtin Tools (meerkat-tools)

### `browse_skills` tool

Registered when the `skills` feature is compiled in AND skills are active at
runtime (`SkillsConfig.enabled == true` or `factory.skill_source` is set).
Returns a stable, typed response.

```
Tool: browse_skills
Description: Browse available skill collections or list skills in a collection.

Parameters:
  path: string (optional) — collection path to browse (e.g. "extraction" or "extraction/medical")
  query: string (optional) — search across name and description (all collections)

Returns (JSON):
  {
    "type": "listing",
    "path": "extraction",                       // browsed path ("" for root)
    "subcollections": [                          // child collections at this level
      { "path": "extraction/medical", "description": "Medical extraction", "count": 2 }
    ],
    "skills": [                                  // direct skills at this level
      { "id": "extraction/email-extractor", "name": "email-extractor", ... }
    ]
  }

When query is provided, returns flat search results across all collections:
  {
    "type": "search",
    "query": "email",
    "skills": [
      { "id": "extraction/email-extractor", ... }
    ]
  }

When BOTH path and query are provided: query wins (search mode).
Path is ignored. This avoids ambiguous semantics — search is always global.
```

Return type discriminated by `"type"`: `"listing"` or `"search"`. No ambiguity.

### `load_skill` tool

Activates a skill mid-turn. Resolves the ID and returns the rendered body.

```
Tool: load_skill
Description: Load a skill's full instructions into the conversation.

Parameters:
  id: string — canonical skill ID (e.g. "extraction/email-extractor")

Returns: The skill body content as text.

Side effects:
  - Emits AgentEvent::SkillsResolved { skills, injection_bytes }
  - Skill content enters the context window
```

### Per-turn `/skill-ref` processing

When a user message starts with a `/` followed by a valid skill ID pattern:

```
1. Pre-processing step (before LLM call)
2. Regex: ^/([a-z0-9-]+(?:/[a-z0-9-]+)*)(?:\s|$)
   Matches: /extraction/email-extractor, /extraction/medical/diagnosis, /pdf-processing
3. Strip leading /, remainder is the SkillId
4. Call engine.resolve_and_render(&[id])
5. Inject <skill> block as content prepended to the turn
6. Emit SkillsResolved or SkillResolutionFailed event
7. Remove the /skill-ref prefix from user message text
```

---

## Usage Scenarios

### 1. Elephant — SDK with remote skill source

Elephant stores extraction skills in its own database, serves them via HTTP.

```rust
// Elephant startup — skill source points at Elephant's own API
let skill_source = Arc::new(HttpSkillSource::new(
    "http://localhost:8080/api",              // base_url
    Some(HttpSkillAuth::Bearer("my-token".into())),
    Duration::from_secs(60),                  // cache TTL
));

let factory = AgentFactory::new("/tmp/meerkat-store")
    .skill_source(skill_source)
    .builtins(true);

// Per-extraction: pre-load the right skill by canonical ID
let config = AgentBuildConfig::new("claude-sonnet-4-5")
    .with_preloaded_skills(&["extraction/email-extractor"]);

let agent = factory.build_agent(config, &meerkat_config).await?;
agent.run("Extract entities from this email: ...").await?;
```

Elephant implements the Meerkat Skills API:
- `GET /api/skills` — list skills
- `GET /api/skills/extraction%2Femail-extractor` — get skill with body
- `GET /api/skill-collections` — list collections

### 2. CLI — Interactive skill discovery

```
$ rkat run "help me process this PDF"

# System prompt contains:
# <available_skills mode="collections">
#   <collection path="document" count="4">Document processing</collection>
#   <collection path="code" count="6">Code analysis and generation</collection>
# </available_skills>

# Agent calls: browse_skills(path: "document")
# Returns: { "type": "listing", "path": "document",
#   "subcollections": [],
#   "skills": [
#     {"id": "document/pdf-processing", "name": "pdf-processing", ...},
#     {"id": "document/word-export", ...},
#   ]
# }
# Agent calls: load_skill(id: "document/pdf-processing")
# Skill body injected into context
```

### 3. CLI — Explicit skill invocation

```
$ rkat run "/document/pdf-processing extract tables from report.pdf"

# Pre-processing detects /document/pdf-processing
# Strips leading /, resolves SkillId("document/pdf-processing")
# Injects skill body, sends "extract tables from report.pdf" to LLM
```

### 4. SDK — In-memory skills (testing / embedding)

```rust
let skills = vec![
    SkillDocument {
        descriptor: SkillDescriptor {
            id: SkillId("test/greeter".into()),
            name: "greeter".into(),
            description: "Greets users by name".into(),
            scope: SkillScope::Builtin,
            ..Default::default()
        },
        body: "Always greet the user by their first name.".into(),
        extensions: IndexMap::new(),
    },
];

let source = Arc::new(InMemorySkillSource::new(skills));
let factory = AgentFactory::new("/tmp/store").skill_source(source);
```

### 5. Git repo as skill library

A team maintains skills in `github.com/company/extraction-skills`:

```
extraction-skills/
  extraction/
    COLLECTION.md                    ← "Entity and relationship extraction"
    email-extractor/
      SKILL.md
      scripts/parse_headers.py
    legal-extractor/
      SKILL.md
      references/clause-taxonomy.md
  formatting/
    COLLECTION.md                    ← "Output formatting and templates"
    markdown-output/
      SKILL.md
```

Three consumption options:

**Option A** — `GitSkillSource` (recommended for direct use):

```rust
let source = GitSkillSource::new(GitSkillConfig {
    repo_url: "https://github.com/company/extraction-skills.git".into(),
    git_ref: GitRef::Tag("v1.2.0".into()),   // pin to release
    auth: Some(GitSkillAuth::HttpsToken(github_token)),
    ..Default::default()
});

let factory = AgentFactory::new("/tmp/store")
    .skill_source(Arc::new(source));
```

**Option B** — Clone locally, use `FilesystemSkillSource`:

```bash
git clone https://github.com/company/extraction-skills.git /opt/skills
```

```rust
let source = FilesystemSkillSource::new(
    "/opt/skills".into(),
    SkillScope::Project,
);
```

**Option C** — Sync into Elephant's DB, serve via `HttpSkillSource`:

A CI/CD pipeline or webhook syncs the git repo into Elephant's storage.
Meerkat consumes via the HTTP API. Best when multiple services share
the same skill library and you want centralized management.

---

## Implementation Phases

### Phase 1: Core wiring
- Revise `SkillSource` trait (add `SkillFilter`, `collections()`)
- Add `SkillFilter`, `SkillCollection`, `ResolvedSkill` types to `meerkat-core`
- Add `metadata: IndexMap<String, String>` and `source_name: String` to `SkillDescriptor`
- Add `SkillsConfig`, `SkillRepositoryConfig`, `SkillRepoTransport` to `meerkat-core`
- Add `SkillsConfig` loading (`.rkat/skills.toml`, project/user layering, env expansion)
- Add `skill_source` field to `AgentFactory`
- Add `preload_skills` field to `AgentBuildConfig`
- Implement `resolve_repositories()` in `meerkat-skills` (config → CompositeSkillSource)
- Update factory step 11 to use config-driven resolution + SDK override + pre-load
- Update `FilesystemSkillSource` for recursive arbitrary-depth scanning + `COLLECTION.md`
- Update `CompositeSkillSource` with named sources + shadowing tracing
- Revise `SkillsParams` wire type to use `Option<Vec<String>>`

### Phase 2: Renderer + spec compliance
- Switch renderer from markdown to XML `<available_skills>` format
- Add `mode` attribute for flat vs collection discrimination
- Add `id` attribute on `<skill>` elements (canonical ID)
- Implement flat vs collection-summary threshold logic
- Preserve existing `MAX_INJECTION_BYTES` truncation for injection mode

### Phase 3: Per-turn activation
- Implement `/skill-ref` detection regex
- Wire into agent loop or session service pre-processing
- Call `resolve_and_render()` and inject results
- Emit `SkillsResolved` / `SkillResolutionFailed` events
- Wire `SkillsParams` into surface crates (REST, RPC, MCP Server)

### Phase 4: Discovery tools
- Implement `browse_skills` builtin tool (discriminated return type)
- Implement `load_skill` builtin tool
- Register tools when `skills` feature is enabled
- `Arc<dyn SkillEngine>` stored in tool state and agent

### Phase 5: HttpSkillSource
- Implement `HttpSkillSource` behind `skills-http` feature flag
- `HttpSkillAuth` enum (Bearer / Header)
- Meerkat Skills API wire format (list, get, skill-collections)
- Cache: full unfiltered list + per-ID documents, TTL-based expiry
- Cache refresh rule: always fetch unfiltered on miss/expiry
- URL construction: `{base_url}/skills`, `{base_url}/skills/{url_encoded_id}`, `{base_url}/skill-collections`
- Integration test with mock HTTP server

### Phase 6: GitSkillSource
- Implement `GitSkillSource` behind `skills-git` feature flag
- `gix` (gitoxide) for clone/pull — no git CLI dependency
- `GitSkillConfig`: repo URL, `GitRef` (Branch/Tag/Commit), cache dir, refresh interval, auth
- `GitSkillAuth`: HTTPS token and SSH key support
- Lazy initialization: clone on first access, not construction
- TTL-based refresh for Branch refs; no-op for Tag/Commit (immutable)
- Stale-cache resilience: network failures serve last successful sync
- `skills_root` option for repos where skills live in a subdirectory
- Delegates all parsing/validation to `FilesystemSkillSource`
- Integration tests: clone from local bare repo, refresh cycle, stale cache behavior

---

## Resolved Design Decisions

1. **Single canonical ID format** — `{collection-path}/{name}` everywhere. Arbitrary
   depth. No Anthropic-style `skill_` prefix IDs. No mapping layer. The ID in
   `SkillDescriptor` is the same string used in HTTP URLs (URL-encoded), tool
   parameters, wire params, and user `/skill-ref` syntax.

2. **Arbitrary-depth collections** — collection path is everything before the last
   `/` in the skill ID. `extraction/medical/diagnosis` has collection
   `extraction/medical`. Browsing shows subcollections + direct skills at each
   level. Convention: prefer 1-2 levels, deep nesting supported but not encouraged.

3. **`browse_skills` return type** — discriminated by `"type"` field: `"listing"`
   (path, subcollections, skills) or `"search"` (query, skills). No ambiguity.

4. **`Option<Vec>` semantics** — `Some([])` is normalized to `None` at the wire
   boundary. `None` = inventory mode, `Some(ids)` = pre-load these. To disable
   skills entirely, use `SkillsConfig.enabled = false`.

5. **Injection sanitization** — skill body placed inside `<skill id="...">` wrapper.
   Closing-tag forms matching `</skill\s*>` (case-insensitive) are escaped to
   `<\/skill>`. Escaping applied before truncation. No other XML escaping —
   LLM-consumed delimiters, not parsed XML. Size limited by `max_injection_bytes`
   (32 KiB default), truncated with `[truncated]` marker.

6. **HTTP base URL** — `base_url` is the API root. Endpoints appended: `/skills`,
   `/skills/{id}`, `/skill-collections`. Collections endpoint is `/skill-collections`
   to avoid namespace collision with a root-level skill named `collections`.

7. **Cache** — full unfiltered descriptor list cached as one entry; filtered queries
   applied client-side. Cache refresh MUST fetch unfiltered `GET /skills` (no params)
   to prevent filtered-response poisoning. TTL-based expiry.

8. **Repositories as config concept** — named skill sources in `.rkat/skills.toml`,
   merged into one namespace via `CompositeSkillSource`. Repository name is
   metadata (for browsing/tracing), not part of the skill ID. Precedence =
   declaration order + project-over-user layering.

9. **Source precedence** — first source wins in `CompositeSkillSource`. Shadowing
   logged at `info` level with skill ID and both source names. `SkillDescriptor`
   carries `source_name` for provenance in browse results.

10. **Reference syntax** — strict: leading `/` + canonical ID. Regex:
    `^/([a-z0-9-]+(?:/[a-z0-9-]+)*)(?:\s|$)`. Arbitrary depth. No bare-name
    resolution. Unambiguous.

11. **Auth model** — `HttpSkillAuth` enum: `Bearer(token)` or `Header { name, value }`.
    Supports both `Authorization: Bearer ...` and `X-API-Key: ...` patterns.

12. **Failure semantics** — pre-load failures are hard errors (`BuildAgentError`).
    Per-turn `/skill-ref` failures emit `SkillResolutionFailed` and continue.
    `load_skill` tool failures return error results to the agent.

13. **Discovery tools always registered (when skills are active)** — `browse_skills`
    and `load_skill` are registered whenever the `skills` feature is compiled in
    AND `SkillsConfig.enabled != false` (or `factory.skill_source` is set).
    Pre-loading is additive — it doesn't disable runtime discovery.

14. **Config-driven resolution** — `meerkat_skills::resolve_repositories()` is the
    single canonical function that turns `SkillsConfig` into `Arc<dyn SkillSource>`.
    All surfaces call it. SDK users bypass it via `factory.skill_source()`.

15. **`enabled = false` vs SDK override** — `SkillsConfig.enabled = false` disables
    all skill machinery (no sources, no inventory, no tools). However,
    `factory.skill_source` being set is an explicit SDK override that **bypasses
    the enabled flag**. Rationale: programmatically wiring a skill source is a
    deliberate act that implies intent — the SDK user wants skills regardless of
    what the config file says. This prevents a config file from silently breaking
    an SDK integration.

16. **Config lives in `meerkat-core`** (following `McpServerConfig` precedent).
    Recognized as technical debt — future config refactor should move transport-
    specific types to their implementation crates.
