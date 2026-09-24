# Agent 03 Findings

## Scope reviewed

- `docs/guides/auth.mdx`
- `docs/guides/self-hosted-gemma4.mdx`
- Adjacent provider/auth/model docs that these pages depend on, especially `docs/concepts/providers.mdx` and `docs/cli/configuration.mdx`
- Current provider/auth/model code paths in `meerkat-cli`, `meerkat-core`, `meerkat-auth-core`, `meerkat-anthropic`, `meerkat-openai`, `meerkat-gemini`, and `meerkat-runtime`

No findings for the env-var fast path itself: the docs' `RKAT_*` precedence and Gemini fallback to `GOOGLE_API_KEY` match the current resolver and synthesized `env_default` realm behavior in `meerkat-auth-core/src/resolver.rs:30-47` and `meerkat-core/src/connection.rs:271-356`.

## Files reviewed

- `docs/guides/auth.mdx`
- `docs/guides/self-hosted-gemma4.mdx`
- `docs/concepts/providers.mdx`
- `docs/cli/configuration.mdx`
- `meerkat-cli/src/main.rs`
- `meerkat-core/src/connection.rs`
- `meerkat-core/src/config.rs`
- `meerkat-core/src/model_profile/mod.rs`
- `meerkat-core/src/model_registry.rs`
- `meerkat-core/src/provider_matrix/anthropic_auth.rs`
- `meerkat-core/src/provider_matrix/google_backend.rs`
- `meerkat-core/src/provider_matrix/google_auth.rs`
- `meerkat-auth-core/src/resolver.rs`
- `meerkat-anthropic/src/runtime/mod.rs`
- `meerkat-openai/src/runtime/mod.rs`
- `meerkat-openai/src/client_compatible.rs`
- `meerkat-gemini/src/runtime/mod.rs`
- `meerkat-rest/src/auth_endpoints.rs`
- `meerkat-rpc/src/handlers/auth.rs`
- `meerkat-runtime/src/meerkat_machine_types.rs`

## Findings

1. `docs/guides/auth.mdx` currently overstates `rkat auth login` as a realm-scoped flow.

The guide says users can "declare a realm in your config, register credentials via `rkat auth login`" and then use bindings like `prod:default` / `prod:openai` (`docs/guides/auth.mdx:9-12`, `docs/guides/auth.mdx:39-98`). The current CLI does not support `--realm` or `--profile-id` on `rkat auth login`; interactive login always persists to hard-coded `dev` keys like `dev:anthropic_oauth`, `dev:openai_oauth`, and `dev:google_oauth`, and non-interactive login always writes `dev:default_<provider>` (`meerkat-cli/src/main.rs:1380-1411`, `meerkat-cli/src/main.rs:3526-3550`, `meerkat-cli/src/main.rs:3622-3640`). Provider runtimes also resolve persisted credentials using `<realm>:<auth_profile.id>`, so a guide example built around `realm.prod` is not populated by `rkat auth login` as written (`meerkat-anthropic/src/runtime/mod.rs:184-203`, `meerkat-openai/src/runtime/mod.rs:141-160`, `meerkat-gemini/src/runtime/mod.rs:148-165`). This is the largest user-facing mismatch in the page.

2. The auth-method matrix in `docs/guides/auth.mdx` contains stale backend/auth identifiers.

Two entries do not match the strings accepted by the current provider matrix:
- Bedrock SigV4 is documented as `bedrock_aws` in `docs/guides/auth.mdx:141`, but the actual auth method name is `bedrock_aws_sigv4` in `meerkat-core/src/provider_matrix/anthropic_auth.rs:30-49`.
- Gemini Code Assist is documented under backend `code_assist` in `docs/guides/auth.mdx:150`, but the actual backend kind is `google_code_assist` in `meerkat-core/src/provider_matrix/google_backend.rs:11-24`, and the runtime error/help text also uses that name in `meerkat-gemini/src/runtime/mod.rs:331-370`.

Because this table is presented as "what's wired today," these stale identifiers are likely to cause copy/paste config failures.

3. The credential-source section is incomplete and partially misleading relative to the current enum surface.

`docs/guides/auth.mdx:156-167` says "Five shapes are supported" and lists `InlineSecret`, `Env`, `ManagedStore`, `ExternalResolver`, and `Command`. The current `CredentialSourceSpec` also includes `PlatformDefault` and `FileDescriptor` (`meerkat-core/src/connection.rs:81-131`), and the CLI already renders both names (`meerkat-cli/src/main.rs:3865-3871`). In addition, the page frames `ManagedStore { profile }` as the source-level lookup shape, but current provider runtimes load persisted credentials by `binding.auth_profile.id` rather than by `source.profile`, and `source.profile` does not appear to be consumed in the runtime lookup path (`meerkat-anthropic/src/runtime/mod.rs:188-198`, `meerkat-openai/src/runtime/mod.rs:145-155`, `meerkat-gemini/src/runtime/mod.rs:150-160`). That means the docs currently describe a more flexible `ManagedStore` lookup contract than the code actually implements.

4. `docs/guides/self-hosted-gemma4.mdx` needs a stronger caveat around audio support.

The guide says `E2B` and `E4B` "expose native audio support" (`docs/guides/self-hosted-gemma4.mdx:16`, `docs/guides/self-hosted-gemma4.mdx:249`), but Meerkat's self-hosted model/profile surface has no audio capability field at all. The self-hosted config schema only carries temperature/thinking/reasoning/vision/image-tool-results/web-search/realtime/timeouts (`meerkat-core/src/config.rs:1006-1029`), the exported model profile surface also has no audio field (`meerkat-core/src/model_profile/mod.rs:52-68`, `meerkat-runtime/src/meerkat_machine_types.rs:87-101`), and self-hosted aliases are currently registered with `realtime: false` (`meerkat-core/src/model_registry.rs:170-183`). As written, the page can be read as if Meerkat exposes self-hosted Gemma audio capability today, when the current runtime only documents text/image/tooling on this path.

## Suggested follow-ups

- Update `docs/guides/auth.mdx` to distinguish three separate stories clearly:
  `env_default` fast path, CLI login's current fixed `dev:*` behavior, and truly realm/profile-targeted persistence via REST/RPC surfaces.
- Correct the auth matrix identifiers to `bedrock_aws_sigv4` and `google_code_assist`, and consider adding the exact accepted backend/auth strings from the provider matrix enums.
- Either document `PlatformDefault` and `FileDescriptor` explicitly or narrow the "supported shapes" language so it does not claim the list is exhaustive.
- Decide whether `ManagedStore.profile` is intended to matter. If yes, the runtime behavior likely needs a code fix; if no, the docs should stop implying that field drives lookup.
- Add a caveat to `docs/guides/self-hosted-gemma4.mdx` that upstream Gemma audio capability is not currently surfaced as a Meerkat self-hosted capability/realtime transport on this path.
