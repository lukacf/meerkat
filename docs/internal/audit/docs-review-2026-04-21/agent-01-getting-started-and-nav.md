# Agent 01 Findings

## Scope reviewed

- `docs/introduction.mdx`
- `docs/quickstart.mdx`
- `docs/docs.json`
- Global navigation and first-run discoverability related to getting-started content

## Files reviewed

- `docs/introduction.mdx`
- `docs/quickstart.mdx`
- `docs/docs.json`
- `docs/examples/gallery.mdx`
- `docs/guides/auth.mdx`
- `docs/cli/commands.mdx`
- `docs/concepts/providers.mdx`
- `docs/concepts/realms.mdx`
- `README.md`
- `Cargo.toml`
- `meerkat/Cargo.toml`
- `meerkat/examples/simple.rs`
- `sdks/python/pyproject.toml`
- `sdks/python/meerkat/client.py`
- `sdks/python/README.md`
- `sdks/typescript/package.json`
- `sdks/typescript/src/client.ts`
- `examples/002-hello-meerkat-py/README.md`
- `examples/003-hello-meerkat-ts/README.md`

## Findings

### 1. Quickstart CLI install path is repo-local, not a real public install path

- `docs/quickstart.mdx:28-32` tells users to run `cargo install --path meerkat-cli`.
- That command only works from a checked-out Meerkat repo, but the page is framed as the primary public quickstart.
- The workspace metadata shows the CLI crate is published as `rkat`, not just as a local path crate: `Cargo.toml` lists `meerkat-cli` with the note `publishes as "rkat" on crates.io` under the workspace members.
- The repo README also points new users to `cargo install rkat` instead of a repo-local `--path` install (`README.md:46-49`).

Why this matters:
- A first-time user following the docs site verbatim can fail immediately unless they already cloned the repo.
- The other install tabs use public package-manager commands, so the CLI tab currently breaks that pattern.

Suggested correction:
- Make `cargo install rkat` the default quickstart instruction.
- If source builds are still worth mentioning, label them explicitly as an alternative for contributors or local development.

### 2. Python and TypeScript quickstart notes incorrectly say `rkat-rpc` must already be on `PATH`

- `docs/quickstart.mdx:16-26` says both the Python and TypeScript SDKs require the `rkat-rpc` binary on `PATH`.
- That no longer matches the current SDK implementations:
  - Python resolves `rkat-rpc`, but if it is missing it attempts to download the binary and can also fall back to `rkat` (`sdks/python/meerkat/client.py:256-268`, `sdks/python/meerkat/client.py:2285-2305`).
  - TypeScript does the same: it first checks `PATH`, then downloads `rkat-rpc`, and only falls back to `rkat` if needed (`sdks/typescript/src/client.ts:274-287`, `sdks/typescript/src/client.ts:2948-2958`).
- The surrounding repo docs/examples already reflect the newer behavior:
  - `examples/003-hello-meerkat-ts/README.md:3-9` says the TypeScript SDK auto-downloads and spawns `rkat-rpc`.
  - The Python SDK README does not list a manual binary install as a prerequisite (`sdks/python/README.md:10-18`).

Why this matters:
- The quickstart currently adds friction that the product has already removed.
- It also creates conflicting guidance between the main quickstart and the SDK-specific docs/examples.

Suggested correction:
- Update the notes to say the SDKs will auto-resolve/download `rkat-rpc` by default.
- Keep the standalone-binary caveat only for advanced realm/context options, which both SDKs still gate behind a real `rkat-rpc` binary.

### 3. The Rust quickstart sample is drifting away from the maintained in-repo starter example

- `docs/quickstart.mdx:40-64` contains a Rust starter snippet.
- The closest maintained canonical example in the repo is `meerkat/examples/simple.rs:22-48`.
- The two versions have already diverged in several observable ways:
  - docs use `claude-sonnet-4-6`; the repo example uses `claude-sonnet-4-5`
  - docs omit the explicit `store.init().await?` call that the checked-in example includes (`meerkat/examples/simple.rs:27-29`)
  - docs print only `result.text`; the example prints response plus session stats (`meerkat/examples/simple.rs:40-48`)

Why this matters:
- Even if the docs snippet still works, there are now two competing "starter" versions to maintain.
- New users who compare the docs against the repo example have to guess which one is authoritative.

Suggested correction:
- Either generate/sync the quickstart Rust snippet from `meerkat/examples/simple.rs`, or clearly point readers to that example as the maintained source of truth.

### 4. Authentication/setup guidance exists but is effectively hidden from the getting-started path

- There is a dedicated auth guide at `docs/guides/auth.mdx`.
- It is not present anywhere in `docs/docs.json`, so it is not discoverable from the main navigation.
- Within the scoped pages, there is also no direct link from `docs/introduction.mdx` or `docs/quickstart.mdx` to the auth guide or even to provider-setup guidance before asking users to run commands.
- This is especially noticeable because the quickstart assumes provider credentials immediately (`docs/quickstart.mdx:45`, `docs/quickstart.mdx:90`) but does not provide a first-run route to the auth/setup material.

Why this matters:
- Credential setup is a first-run task, not an advanced edge case.
- Hiding it in a non-nav page increases the chance that users bounce from the quickstart to guesswork instead of finding the supported setup path.

Suggested correction:
- Add `guides/auth` to `docs/docs.json`, likely in either "Getting started" or "Guides".
- Link to it directly from quickstart install/run sections and/or the introduction page.

### 5. The introduction sends users to an examples gallery that skips the actual hello-world starters

- `docs/introduction.mdx:58-64` sends users from "Get started" to `/examples/gallery`.
- The repo does contain explicit beginner examples:
  - `examples/001-hello-meerkat-rs`
  - `examples/002-hello-meerkat-py`
  - `examples/003-hello-meerkat-ts`
- But `docs/examples/gallery.mdx:9-67` starts at example 017 and focuses on advanced multi-agent, deployment, and demo apps.
- The gallery footer then says "All 35 examples (including single-feature demos and hello-world starters) are in `examples/` in the repository" (`docs/examples/gallery.mdx:71`), but the page itself does not surface those starter examples.

Why this matters:
- The introduction's CTA implies "show me runnable next steps," but the target page is optimized for advanced showcase material, not onboarding.
- This is a discoverability mismatch rather than a broken link, but it affects first-run usability.

Suggested correction:
- Add a "Hello world / starter examples" section to `/examples/gallery`, or change the introduction CTA to point to a more beginner-focused examples page.

### 6. No broken internal destinations found in the scoped intro/quickstart navigation paths

- I did not find any missing internal docs pages referenced directly by:
  - `docs/introduction.mdx`
  - `docs/quickstart.mdx`
  - the relevant `docs/docs.json` navigation entries in scope
- The main issue in this area is discoverability/placement, not broken page targets.

## Suggested follow-ups

- Update `docs/quickstart.mdx` to use a public CLI install command and to remove the outdated "`rkat-rpc` must be on `PATH`" requirement for basic Python/TypeScript usage.
- Add `docs/guides/auth.mdx` to `docs/docs.json` and link it from the quickstart and/or introduction.
- Decide on a single maintained Rust starter source, preferably the checked-in example in `meerkat/examples/simple.rs`, and keep the quickstart aligned with it.
- Rework `/examples/gallery` or the introduction CTA so first-time users can find `001`/`002`/`003` without leaving the docs flow.
