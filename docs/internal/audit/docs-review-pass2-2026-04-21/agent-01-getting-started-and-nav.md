# Agent 01 Findings

## Scope reviewed

- `docs/introduction.mdx`
- `docs/quickstart.mdx`
- `docs/docs.json`
- Global navigation and discoverability around the getting-started path

## Files reviewed

- `docs/introduction.mdx`
- `docs/quickstart.mdx`
- `docs/docs.json`
- `meerkat/examples/simple.rs`
- `meerkat-cli/Cargo.toml`
- `meerkat-rpc/Cargo.toml`
- `meerkat-rest/Cargo.toml`
- `meerkat-mcp-server/Cargo.toml`
- `examples/README.md`
- `examples/001-hello-meerkat-rs/README.md`
- `examples/002-hello-meerkat-py/README.md`
- `examples/003-hello-meerkat-ts/README.md`
- `docs/examples/gallery.mdx`
- `docs/guides/hooks.mdx`
- `docs/examples/hooks.mdx`
- `examples/011-hooks-guardrails-rs/README.md`
- `meerkat-hooks/src/lib.rs`

## Findings

1. `docs/quickstart.mdx` ends with a docs-site-broken local filesystem link, and the note attached to it is no longer accurate.

   - `docs/quickstart.mdx:150-151` links to `/Users/luka/.codex/worktrees/fc6a/meerkat/meerkat/examples/simple.rs`. That is a machine-local path, not a portable docs URL, so it will not work for published docs readers.
   - The same note says the quickstart Rust snippet is "kept aligned" with the maintained starter example, but the current example uses `claude-sonnet-4-5` in both `factory.build_llm_adapter(...)` and `.model(...)` at `meerkat/examples/simple.rs:25` and `meerkat/examples/simple.rs:34`, while `docs/quickstart.mdx:59` and `docs/quickstart.mdx:66` use `claude-sonnet-4-6`.
   - Impact: the final trust-building note on the page currently does the opposite: it sends readers to a dead link and overstates snippet parity.

2. The introduction's "single 5MB binary" positioning does not match the current shipped surface layout.

   - `docs/introduction.mdx:12` and `docs/introduction.mdx:25` describe Meerkat as a "Single 5MB binary" with "<10ms startup".
   - The current repo ships multiple binaries across separate surface crates, not a single user-facing binary: `rkat` in `meerkat-cli/Cargo.toml`, `rkat-rpc` in `meerkat-rpc/Cargo.toml`, `rkat-rest` in `meerkat-rest/Cargo.toml`, and `rkat-mcp` in `meerkat-mcp-server/Cargo.toml`.
   - `docs/quickstart.mdx:21` and `docs/quickstart.mdx:29` also explicitly tell Python and TypeScript users that the SDK auto-resolves `rkat-rpc`, which further reinforces that the current product story is "multiple surfaces and helper binaries", not "one binary".
   - Impact: this is likely to confuse readers evaluating installation/distribution options from the landing page.

3. The introduction makes an exact "eight hook points" claim while the current repo still contains conflicting "7 hook points" language.

   - `docs/introduction.mdx:16` says there are "Eight hook points".
   - Some current docs and code agree with that: `docs/guides/hooks.mdx:11-22` and `docs/examples/hooks.mdx:453-464`.
   - But current repo content still says 7 in multiple places: `examples/011-hooks-guardrails-rs/README.md:3-8` and `meerkat-hooks/src/lib.rs:14` and `meerkat-hooks/src/lib.rs:26`.
   - Impact: even if eight is the intended current contract, the introduction is presenting a precise count that the rest of the repo has not fully converged on. For a landing page, this reads as unstable guidance.

4. The getting-started navigation underexposes the beginner examples even though the repo has a strong examples-first on-ramp.

   - In `docs/docs.json:29-30`, the "Getting started" group includes only `introduction`, `quickstart`, and `guides/auth`.
   - The repo has an explicit beginner examples entrypoint in `examples/README.md:6-27` and a dedicated "Beginner -- Getting Started" section at `examples/README.md:54-64`, plus language-specific hello examples in `examples/001-hello-meerkat-rs/README.md:1-13`, `examples/002-hello-meerkat-py/README.md:1-20`, and `examples/003-hello-meerkat-ts/README.md:1-20`.
   - `docs/examples/gallery.mdx:12-18` also already exposes those three hello examples, but that page is tucked under the later "Examples" nav group rather than the initial getting-started path.
   - Impact: a new reader can finish the landing page and quickstart without ever being steered to the most obvious language-specific runnable examples.

5. `docs/docs.json` does not appear to have broken scoped page references, but the scoped nav still misses a stronger onboarding path.

   - I did not find any missing page targets among the scoped nav entries in `docs/docs.json`; all referenced pages for the reviewed groups exist under `docs/`.
   - The issue here is discoverability rather than broken navigation: the current structure is valid, but it is not optimized for the first-run journey described by `introduction.mdx` and `quickstart.mdx`.

## Suggested follow-ups

- Replace the local absolute path in `docs/quickstart.mdx` with a portable docs or GitHub link, and either sync the snippet to `meerkat/examples/simple.rs` or remove the claim that it is kept aligned.
- Reword the landing-page binary/performance copy in `docs/introduction.mdx` so it reflects the current multi-surface distribution story without relying on a single-binary simplification.
- Reconcile the hook-point count across the repo before keeping an exact number in the introduction; if that broader cleanup is out of scope, soften the landing-page wording for now.
- Promote examples earlier in the onboarding flow by adding `examples/gallery` to the getting-started nav group in `docs/docs.json`, or by adding a more prominent "Choose your language example" step near the top or bottom of `docs/quickstart.mdx`.
