# Agent 03 Findings

## Scope reviewed

- `docs/guides/auth.mdx`
- `docs/guides/self-hosted-gemma4.mdx`
- Adjacent provider/auth/model guidance these pages rely on, especially:
  - `docs/cli/configuration.mdx`
  - `docs/concepts/providers.mdx`
  - `README.md`
- Current auth/provider/self-hosted implementation in:
  - `meerkat-cli`
  - `meerkat-core`
  - `meerkat-auth-core`
  - `meerkat-anthropic`
  - `meerkat-openai`
  - `meerkat-gemini`
  - `meerkat`
  - `meerkat-rpc`

## Files reviewed

- `docs/guides/auth.mdx`
- `docs/guides/self-hosted-gemma4.mdx`
- `docs/cli/configuration.mdx`
- `docs/concepts/providers.mdx`
- `README.md`
- `meerkat-cli/src/main.rs`
- `meerkat-core/src/connection.rs`
- `meerkat-core/src/config.rs`
- `meerkat-core/src/model_registry.rs`
- `meerkat-core/src/provider_matrix/anthropic_auth.rs`
- `meerkat-core/src/provider_matrix/google_auth.rs`
- `meerkat-core/src/provider_matrix/google_backend.rs`
- `meerkat-core/src/provider_matrix/openai_auth.rs`
- `meerkat-auth-core/src/resolver.rs`
- `meerkat-anthropic/src/runtime/mod.rs`
- `meerkat-openai/src/runtime/mod.rs`
- `meerkat-openai/src/client_compatible.rs`
- `meerkat-gemini/src/runtime/mod.rs`
- `meerkat/src/factory.rs`
- `meerkat-rpc/src/handlers/auth.rs`
- `meerkat-rpc/src/handlers/turn.rs`
- `meerkat-mob-mcp/src/agent_tools.rs`

## Findings

### 1. `docs/guides/auth.mdx` still implies `rkat auth login` is part of a general realm-targeted setup flow, but the CLI persists only to fixed `dev:*` identities.

- The page intro and main setup flow still read as "declare a realm in config, register credentials via `rkat auth login`, then use `--connection-ref prod:...`" in one continuous story: `docs/guides/auth.mdx:7-17`, `docs/guides/auth.mdx:54-104`.
- The CLI does not let users choose a target realm or profile during login. Interactive login always persists to fixed `dev` keys such as `dev:anthropic_oauth`, `dev:openai_oauth`, and `dev:google_oauth`; non-interactive login always writes `dev:default_<provider>`: `meerkat-cli/src/main.rs:3522-3550`, `meerkat-cli/src/main.rs:3597-3640`.
- Provider runtimes then read persisted OAuth material by `<realm>:<auth_profile.id>`, not by an arbitrary binding name. If a user follows the guide's `realm.prod` example, `rkat auth login anthropic` does not populate `prod:anthropic_oauth` for them: `meerkat-anthropic/src/runtime/mod.rs:184-203`, `meerkat-openai/src/runtime/mod.rs:141-160`, `meerkat-gemini/src/runtime/mod.rs:146-165`.
- The note at `docs/guides/auth.mdx:15-17` helps, but the surrounding walkthrough still overstates what the CLI login flow can do today.

### 2. The auth guide's reference tables are incomplete/inexact versus the shipped auth/config schema.

- The auth-method matrix claims to summarize what is wired today, but it omits the `external_authorizer` method that is present in all three provider auth enums: `docs/guides/auth.mdx:137-160`; `meerkat-core/src/provider_matrix/anthropic_auth.rs:22-50`; `meerkat-core/src/provider_matrix/openai_auth.rs:23-42`; `meerkat-core/src/provider_matrix/google_auth.rs:14-37`.
- The credential-source table lists `FileDescriptor { fd, format }`, but the current schema is `FileDescriptor { fd, scope_override }`: `docs/guides/auth.mdx:167-175`; `meerkat-core/src/connection.rs:108-129`.
- The guide also says command/file-descriptor sources "let host applications inject tokens" without noting that `FileDescriptor` is not handled by the simple-secret resolver unless a host-scoped reader is provided. Current resolver behavior is an explicit error on that path: `docs/guides/auth.mdx:158-160`; `meerkat-auth-core/src/resolver.rs:95-104`.

### 3. Self-hosted Gemma guidance is internally consistent with current code, but adjacent config docs still carry stale transport recommendations.

- `docs/guides/self-hosted-gemma4.mdx` aligns with the current implementation on the important mechanics:
  - self-hosted config is `self_hosted.servers.*` plus `self_hosted.models.*`: `docs/guides/self-hosted-gemma4.mdx:26-65`; `meerkat-core/src/config.rs:972-1058`
  - only `openai_compatible` transport is currently supported: `meerkat/src/factory.rs:1456-1459`
  - `chat_completions` is the default API style in config: `meerkat-core/src/config.rs:964-993`
  - `rkat doctor` checks the normalized `/v1/models` endpoint and validates alias-to-remote-model mapping there: `meerkat-cli/src/main.rs:3905-3978`; `meerkat-core/src/model_registry.rs:215-221`
- However, `docs/cli/configuration.mdx` still says:
  - "`responses` for Ollama and LM Studio style servers"
  - "`chat_completions` for vLLM style deployments"
  at `docs/cli/configuration.mdx:82-87`.
- That is now stale relative to the Gemma guide's recommendation to prefer `chat_completions` for Ollama, LM Studio, and vLLM (`docs/guides/self-hosted-gemma4.mdx:16-24`, `docs/guides/self-hosted-gemma4.mdx:91-99`, `docs/guides/self-hosted-gemma4.mdx:147-155`, `docs/guides/self-hosted-gemma4.mdx:187-195`) and relative to the code default of `ChatCompletions` (`meerkat-core/src/config.rs:966-993`).

### 4. No code/documentation mismatch found for the env-var fast path, `connection_ref` carry-through, or self-hosted alias registration mechanics.

- The auth guide's env-var precedence and Gemini fallback are consistent with the resolver and synthesized `env_default` realm: `docs/guides/auth.mdx:24-41`; `meerkat-auth-core/src/resolver.rs:28-48`; `meerkat-core/src/connection.rs:268-340`.
- The hot-swap/resume `connection_ref` story is supported by current session/turn plumbing: `docs/guides/auth.mdx:183-229`; `meerkat-rpc/src/handlers/turn.rs:29-80`; `meerkat/src/factory.rs:1210-1215`.
- The self-hosted alias path itself is wired as documented: aliases are merged into the model registry and can be resolved without an explicit provider flag: `meerkat-core/src/model_registry.rs:133-200`; `meerkat-cli/tests/cli_auth_flow.rs:90-120`.

## Suggested follow-ups

1. Rewrite the auth guide's main flow so it clearly separates:
   - env-var fallback (`env_default`)
   - current CLI login persistence into fixed `dev:*` TokenStore entries
   - truly realm/profile-targeted persistence via REST/RPC auth APIs
2. Update `docs/guides/auth.mdx` tables to match the actual schema:
   - add `external_authorizer` where supported
   - fix `FileDescriptor` field names
   - add a caveat that `FileDescriptor` requires host support beyond the plain resolver path
3. Bring `docs/cli/configuration.mdx` into line with the self-hosted Gemma guide and current defaults by removing the older "responses for Ollama/LM Studio, chat_completions for vLLM" recommendation.
4. Optional cleanup: add one explicit note in the auth guide that interactive Google login is the Gemini Code Assist OAuth path, while `google_genai` remains the API-key path.
