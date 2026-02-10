# Skills System Redesign — Progress

## Current Phase: 1
## Current Status: gating

---

## Phase 1: Core Types & Trait Revision

### Tests
- [x] test_skill_id_collection_extraction
- [x] test_skill_id_nested_collection
- [x] test_skill_id_root_level
- [x] test_skill_id_name_extraction
- [x] test_skill_filter_default_is_empty
- [x] test_derive_collections_basic
- [x] test_derive_collections_nested
- [x] test_derive_collections_empty
- [x] test_collection_prefix_match_segment

### Implementation
- [x] Add SkillFilter, SkillCollection, ResolvedSkill types
- [x] Add collection() and name() methods on SkillId
- [x] Add metadata + source_name to SkillDescriptor
- [x] Add derive_collections() with segment-aware prefix matching
- [x] Change SkillSource::list() signature to list(&self, filter: &SkillFilter)
- [x] Add default collections() method on SkillSource
- [x] Revise SkillEngine trait

### Notes
- Also added: apply_filter() utility function, collection_matches_prefix() helper
- Also added: Default derive for SkillId, SkillScope, SkillDescriptor
- Also added: From<S: Into<String>> for SkillId convenience impl
- Updated all downstream crates (sources, engine, parser, resolver, factory) for compilation
- All existing tests pass with the new types

## Phase 2: Source Updates

### Tests
- [ ] test_list_with_empty_filter
- [ ] test_list_with_collection_filter
- [ ] test_list_collection_filter_no_sibling
- [ ] test_recursive_scan_nested_dirs
- [ ] test_recursive_scan_deep_nesting
- [ ] test_collection_md_loading
- [ ] test_collection_md_missing_fallback
- [ ] test_root_level_skill
- [ ] test_list_with_collection_filter (filesystem)
- [ ] test_named_sources_populate_source_name
- [ ] test_shadowing_by_name
- [ ] test_list_merges_across_sources
- [ ] test_collections_merged_across_sources

### Implementation
- [ ] InMemorySkillSource: filter + segment-aware prefix
- [ ] FilesystemSkillSource: recursive scan + namespaced IDs + COLLECTION.md
- [ ] EmbeddedSkillSource: filter param
- [ ] CompositeSkillSource: NamedSource + source_name + shadowing tracing

## Phase 3: Renderer — XML Format

### Tests
- [ ] test_render_inventory_flat_xml
- [ ] test_render_inventory_collections_xml
- [ ] test_render_inventory_empty
- [ ] test_render_injection_xml
- [ ] test_injection_escapes_closing_tag
- [ ] test_injection_escapes_whitespace_tag
- [ ] test_injection_escapes_case_insensitive
- [ ] test_injection_truncation
- [ ] test_escape_before_truncate
- [ ] test_inventory_threshold_boundary

### Implementation
- [ ] XML inventory renderer (flat + collections modes)
- [ ] XML injection renderer with escaping
- [ ] Escape-before-truncate ordering
- [ ] Inventory threshold parameter

## Phase 4: Engine Revision

### Tests
- [ ] test_inventory_section_uses_xml
- [ ] test_resolve_and_render_returns_vec
- [ ] test_resolve_and_render_unknown_id
- [ ] test_list_skills_no_filter
- [ ] test_list_skills_collection_filter
- [ ] test_list_skills_query_filter
- [ ] test_collections_derived
- [ ] test_preload_missing_skill_errors
- [ ] test_resolve_slash_namespaced_id
- [ ] test_resolve_slash_root_level
- [ ] test_resolve_deep_nested

### Implementation
- [ ] DefaultSkillEngine: new trait shape
- [ ] resolve_and_render → Vec<ResolvedSkill>
- [ ] list_skills() + collections()
- [ ] Resolver: simplified slash-prefix only

## Phase 5: Configuration System

### Tests
- [ ] test_default_config
- [ ] test_parse_filesystem_repo
- [ ] test_parse_http_repo
- [ ] test_parse_git_repo
- [ ] test_env_expansion_in_auth_token
- [ ] test_env_expansion_missing_var_errors
- [ ] test_merge_project_over_user
- [ ] test_merge_project_shadows_user_repo
- [ ] test_resolve_empty_config_uses_defaults
- [ ] test_resolve_filesystem_repo
- [ ] test_resolve_embedded_always_appended
- [ ] test_resolve_disabled_returns_none

### Implementation
- [ ] SkillsConfig, SkillRepositoryConfig, SkillRepoTransport in meerkat-core
- [ ] TOML loading with load() and load_from_paths()
- [ ] Env expansion
- [ ] resolve_repositories() in meerkat-skills
- [ ] Add skills field to Config

## Phase 6: Factory Wiring

### Tests
- [ ] test_factory_skill_source_override
- [ ] test_factory_default_chain_no_config
- [ ] test_preload_skills_in_system_prompt
- [ ] test_preload_missing_skill_fails_build
- [ ] test_preload_none_generates_inventory
- [ ] test_enabled_false_skips_skills
- [ ] test_sdk_override_ignores_enabled_flag

### Implementation
- [ ] skill_source on AgentFactory
- [ ] preload_skills on AgentBuildConfig
- [ ] skill_engine on AgentBuilder + Agent
- [ ] Factory step 11 rewrite
- [ ] System prompt assembly update

## Phase 7: Per-Turn Skill Activation

### Tests
- [ ] test_detect_skill_ref_simple
- [ ] test_detect_skill_ref_namespaced
- [ ] test_detect_skill_ref_deep
- [ ] test_detect_skill_ref_none
- [ ] test_detect_skill_ref_midsentence
- [ ] test_detect_skill_ref_only
- [ ] test_strip_skill_ref

### Implementation
- [ ] agent/skills.rs module with detect_skill_ref
- [ ] state.rs skill injection step

## Phase 8: Discovery Tools

### Tests
- [ ] test_browse_root_returns_listing
- [ ] test_browse_collection_returns_listing
- [ ] test_browse_search_returns_search
- [ ] test_browse_both_query_wins
- [ ] test_browse_empty_collection
- [ ] test_load_skill_returns_body
- [ ] test_load_skill_not_found

### Implementation
- [ ] BrowseSkillsTool
- [ ] LoadSkillTool
- [ ] SkillToolSet
- [ ] Register in CompositeDispatcher
- [ ] Factory passes engine to dispatcher

## Phase 9: Wire Format & Surface Integration

### Tests
- [ ] test_skills_params_none_serde
- [ ] test_skills_params_empty_normalizes
- [ ] test_skills_params_with_ids

### Implementation
- [ ] Revise SkillsParams
- [ ] Surface integration (REST, RPC, MCP Server, CLI)
- [ ] Some([]) → None normalization

## Phase 10: HttpSkillSource

### Tests
- [ ] test_list_skills_from_http
- [ ] test_load_skill_from_http
- [ ] test_list_collections_from_http
- [ ] test_cache_serves_on_second_call
- [ ] test_cache_expires_after_ttl
- [ ] test_cache_refresh_always_unfiltered
- [ ] test_auth_bearer_header
- [ ] test_auth_custom_header
- [ ] test_url_encoding_slash_ids
- [ ] test_collection_filter_applied_client_side

### Implementation
- [ ] HttpSkillSource + HttpSkillAuth
- [ ] SkillCache with TTL
- [ ] Unfiltered cache refresh
- [ ] URL construction + wire format

## Phase 11: GitSkillSource

### Tests
- [ ] test_clone_on_first_access
- [ ] test_lazy_no_clone_on_construction
- [ ] test_skills_root_subdirectory
- [ ] test_namespaced_ids_from_repo_structure
- [ ] test_tag_ref_no_refresh
- [ ] test_branch_ref_refreshes_after_ttl
- [ ] test_stale_cache_on_pull_failure
- [ ] test_clone_failure_returns_error
- [ ] test_collection_md_from_repo

### Implementation
- [ ] GitSkillSource + GitSkillConfig + GitRef + GitSkillAuth
- [ ] Lazy init + gix clone/pull
- [ ] TTL refresh for Branch, no-op for Tag/Commit
- [ ] Delegates to FilesystemSkillSource

## Phase 12: Configuration Resolution Wiring

### Tests
- [ ] test_resolve_http_repo
- [ ] test_resolve_git_repo
- [ ] test_resolve_mixed_repos
- [ ] test_resolve_precedence_matches_config

### Implementation
- [ ] Extend resolve_repositories() for HTTP + Git
- [ ] Feature-gate support
- [ ] Error for unsupported transports

---

## Gate Results

### Phase 1 — Attempt 1
- build-gate: PASS (4s, zero warnings)
- test-gate: PASS (all tests pass, 0 failures)
- performance-gate: PASS (build 4s, tests 6s)
- spec-accuracy-gate: PASS (all 9 tests verified, all items implemented, one minor naming deviation accepted)
- rust-quality-gate: PASS (no mandatory violations; allocation warning in collection_matches_prefix fixed; doc comment corrected; blanket From narrowed)
