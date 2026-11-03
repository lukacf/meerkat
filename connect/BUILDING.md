# Building Meerkat Connect

Connect has two Rust workspaces. The root owns `toolkit-package`;
`native-candidate/Cargo.toml` owns the CLI, native host adapters and `toolkit-store`.
Both use checked-in dependency locks.

## Current development selection

The [native manifest](native-candidate/Cargo.toml) and
[native lock](native-candidate/Cargo.lock) select Core 0.9.8 and Meta 0.9.7.
The manifest also selects local Core source patches, Core code generation and
the Meta source import. Preserve those exact source selections with the build
record: Cargo's lock does not encode the Git revision of a path dependency.

The [TypeScript manifest](sdk/typescript/package.json) and
[lock](sdk/typescript/package-lock.json) select `@rkat/sdk` 0.9.8 and
`@rkat/meta-sdk` 0.9.7. The [Python native-host extra](sdk/python/pyproject.toml)
selects `meerkat-meta==0.9.7`. See [SDKs](#sdks) for package versions and validation.

Use the [development build instructions](#build-a-compatible-development-checkout)
with the current checkout's selected sources. Complete products also need their
actual resource inventory digest, emitted binaries and assembly records.
The historical validation below applies only to its named source tuples.

## Historical development validation

This retained development selection builds on Connect base `17641d2` and includes the
reviewed consumer source corrections. The metadata-only resolution
selected 547 packages, including 38 Core packages and one Meta package, with
229 edges into that Core family. The lock changes only Core package versions;
external dependency versions and selected features are unchanged. This resolver
evidence does not establish compilation, consumer execution or release qualification.

- Core source `0d20d8cb09638c72df3207ccbcefc8d57103e23f`, package version 0.9.7.
- Meta owner source `477a9ec3453e9c5e6bf1024438b6518760a5d0ea`, still package
  version 0.9.5. The selected consumer-local projection is
  `eceffb061db8acaf80fd5d011a4fa77441d25264`.

The projection record
records only 32 Core dependency-version selector changes in two Meta manifests.
It is not a final Meta 0.9.6 release re-pin or runtime qualification. Keep the
complete imported workspaces at the paths selected by the
[native manifest](native-candidate/Cargo.toml). A path dependency's Git revision
is not encoded by Cargo's lock; exact source and graph checks remain required.

The earlier native runtime baseline is frozen Connect `7034f0f9`, with the
Core `0d20d8cb` and Meta `eceffb06` sources above. Connect `0dac5363` includes
subsequent fixture and diagnostic changes and has its own execution results
below. Preserve both sets of records: a current pass supersedes a failure only
for the exact behavior exercised, and neither tuple qualifies the final public
dependencies.

| Stage | Retained 7034 result |
| --- | --- |
| Native test build | Three test targets compiled |
| Portable controls | 285 passed, 1 failed; all 286 completed. Three separate retention regressions passed. |
| Additive controls | 11 passed, 1 failed; all 12 completed |
| Historical reader 206 | Passed against genuine predecessor stores |
| Installed test build | Two acceptance hosts and the installed-test executable compiled |
| Five installed controls | First control failed; four did not start |

The following results are bound to frozen Connect `0dac5363`; they do not
transfer to a later source or dependency selection without the required
source, artifact and execution checks.

| Stage | Retained 0dac result |
| --- | --- |
| Native test build | Three test targets compiled; root and independent output reviews passed |
| Python controls | All 69 passed on the exact current inputs |
| SDK imports | All 16 imports passed with the selected source-built SDK closure |
| Resources | Fresh composition and artifact integrity verified |
| Portable controls | 284 passed, 2 failed; all 286 completed. All three separate retention regressions passed. |
| Additive controls | All 12 passed, including corrected cold-wake control 04 |
| Diagnostic and integrity controls | All four passed with the validated fixture interpreter |
| Installed test build | Two acceptance hosts and the installed-test executable compiled |
| Five installed controls | First control failed with confirmed BeginBound; four did not start |
| Separate custody-publication diagnostic | One ignored characterization passed; it is not canonical control 333 |

Compilation and preparation are not installed, product or native runtime
acceptance. Preserve the actual failed records until current execution proves
the relevant behavior and owned cleanup.

On Connect `0dac5363`, portable control 57 refused a Find operation with
`ReprepareRequired/ChangeInProgress`: it was prepared at publication sequence
72 and refused while sequence 75 was being written. The source and audit triage
cannot identify the writer or distinguish the final policy recheck from the
following effect precheck. Its control 333 fails earlier than its 7034
predecessor: it receives `operation_refused` where it expects
`review_unsatisfied`, before retrieving an approval. Its retained audit
has only a prepared row for that initial Find. A later catalog denial belongs
to a different call and does not establish the initial refusal's cause.

The separate custody-publication diagnostic starts with a real Pending
approval, releases one real custody Promote publication, then reads the same
approval. That read refreshes stale consent to Cancelled/ContextChanged and
retires the review attempt. The test verifies equality of typed non-custody
data and no intervening human decision, model resume, new input or helper
entry. It characterizes this controlled boundary; it is not a behavior fix or
proof of the cause of either portable failure.

On Connect `0dac5363`, cold-wake control 04 passes after a fixture correction: recovery
starts before initial submit and observes the actual promotion before
close/reopen. The reopened owner is not prewarmed. The earlier
`OwnerUnavailable` record remains preserved. That installed setup failure
has a direct BeginBound diagnostic
after resource validation, replacing the earlier timing hypothesis. Phase
timings and the underlying cause remain unverified; installed native and
provider cleanup remain unqualified. Diagnostic passes do not resolve that
installed workflow failure.

On the retained 7034 baseline, the 69 Python controls, 41 Linux controls,
five nonhistorical ignored controls, current canonical-G recovery and product
scenarios have no completed execution. Older results, including the 69 Python passes on Connect
`aef90ee3` with Core `f1ae8b5a` and Meta `73f322d1`, remain tied to that older
tuple. See the [earlier integration record](docs/releases/follow-up.md#current-integration-status).
The old-producer capture for historical 207 failed at the 4096-byte mandate
bound and supplies no accepted capture or current-reader pass. The
verified human decision
applies scope version 4: only historical 207 compatibility certification is
deferred for this release. It remains **UNQUALIFIED**. Preserve its selector,
oracle, failed capture, missing producer-complete evidence and unresolved
older receiptless compatibility obligation. Genuine historical reader 206
remains required. Do not report all 334 canonical controls as passed.

The acceptance mapping
preserves the complete 334-control set and original 308-control identity/order
prefix: 286 portable, 41 Linux and 7 explicitly run ignored controls. Every
current-release control except the narrow historical 207 deferral still needs
exact passing evidence. The 12 separate additive controls (5 publication,
2 boxing, 3 bootstrap, 2 cold wake) and 69 Python controls, including the original
67-control prefix, remain required. The five installed controls, seven product
scenarios, four ordinary Private Services controls, current canonical-G and
routable producers remain separate requirements.

That candidate's public qualification required coherent Core 0.9.7 and Meta 0.9.6 source,
SDK, resource, metadata and emitted-artifact provenance. Published Core source
`d920733a807b020e5de3b12ffefe6a8d7d547baf` and Meta source
`923305a634ba4b420464e51fd27403091c157659` were acquired separately.
The Core preparation review
verifies its source and selected macOS artifact; the Meta preparation review
verifies its source, Rust crates and Python wheel/sdist against retained public
registry metadata. At that checkpoint, these prepared artifacts had not replaced
the retained development tuple or passed Connect consumer runtime qualification. Fresh current receiptful
recovery must retain the actual same-emission 78/72 capture and immutable staged
and consumed snapshots, genuine pending consent, exact authority and run
identities, actual owner loss, fenced receipt convergence, same-G reopen,
fresh B work, refusal checks and complete owned cleanup. This proof includes
`PendingTerminalConverged`, run-scoped `CompletedWithoutResult` and idempotent
`NotOwed`; it does not certify historical 207's older receiptless compatibility.
Keep original 32 MiB native controls and exact 16-import SDK/resource bindings.
Test binaries cannot substitute for separately emitted normal products or
installed-host artifacts. This candidate is unreleased.

## Prerequisites

Use Rust 1.97.0, the selected build toolchain, on a supported macOS or Linux host
with its normal compiler and platform libraries. The manifests declare Rust 1.97
(the build toolchain) and edition 2024. Python SDK tooling requires Python 3.11 or later. The TypeScript
SDK requires Node 22.12.0 or later, matching the selected Core SDK's supported
range on Node 22 and newer. Use the package locks for JavaScript dependencies.

## Bundled SQLite build policy

Current Connect builds must select [`.cargo/config.toml`](.cargo/config.toml).
It carries Core0.9.7's required `LIBSQLITE3_FLAGS` with `force = true`: allocator
statistics off, per-connection page caches, and page-cache overflow statistics
off. It adds no runtime policy or feature changes.

The repository wrapper selects this file explicitly, including with a leading
`+toolchain`. For an external Cargo caller or a retained wrapper whose working
directory belongs to another checkout, pass the selected Connect config before
the Cargo command:

```sh
cargo +1.97.0 --config /absolute/connect-source/.cargo/config.toml build \
  --manifest-path /absolute/connect-source/native-candidate/Cargo.toml --locked \
  -p meerkat-connect --features console,toolkit-mobkit/native-host
```

`--manifest-path` does not make Cargo load the target checkout's configuration.
An embedding application or source installation must explicitly select this
same config (or carry the exact declared flags in its own build configuration);
package dependency metadata does not install it for the consumer. Put Cargo
options before a `run --` separator, not among the program arguments.

Retain the selected config with the actual build command and verify the linked
SQLite profile using `sqlite_lock_profile_tests::bundled_sqlite_uses_required_lock_profile`.
The regression queries SQLite's compiled options through the existing `rusqlite`
test dependency. A caller environment string or runtime allocator fallback does
not prove the page-cache switches. New binaries and their resource/source
bindings still require the normal build and consumer acceptance checks.

## Build a compatible development checkout

Use a source checkout with its explicitly selected Core/Meta imports and lock.
The normal product binaries below require their own build and acceptance,
separate from native test artifacts. From that checkout's repository root,
build with its checked-in native lock and no resource binding:

```sh
env -u MEERKAT_CONNECT_RESOURCES_SHA256 ./scripts/repo-cargo +1.97.0 build \
  --manifest-path native-candidate/Cargo.toml --locked --release \
  -p meerkat-connect --features console,toolkit-mobkit/native-host \
  --bin meerkat-connect --bin toolkit_gateway
```

These binaries are for source-only work and cannot prepare integrations. A
resource directory placed beside them later does not add the missing build-time
binding. Git inputs require the private source preparation above; registry inputs
may also need fetching. Use `--offline` only after all exact locked inputs are
cached. Resolution or compilation is not installed-host, model or business
acceptance.

A complete development installation needs the existing resource and assembly
pipeline with explicit selected inputs:

- A target-specific integration library and starter containing the selected setup
  source and package inspector. Setup-source changes require a new library and
  resource closure; copying only a new gateway cannot deliver those changes.
- The selected Python, Node/npm and SDK artifacts, with their exact byte/hash
  bindings. Version labels or SDK source equality alone do not qualify an
  installed composition.
- A new `resources.json` inventory generated by
  [`package-connect-resources.py`](scripts/package-connect-resources.py), and
  native binaries built with `MEERKAT_CONNECT_RESOURCES_SHA256` set to that exact
  inventory's SHA-256. Keep the entire resource prefix together.
- The existing build records, binary/resource hashes, assembly verification and
  installed-host checks used by
  [`assemble-connect-product.py`](scripts/assemble-connect-product.py). Its
  historical defaults do not select the current development composition.

### Derived Linux binaries

The assembler accepts either the exact Cargo artifact or an explicitly recorded
`--strip-debug` derivation for `meerkat-connect` and `toolkit_gateway`. Every
shipped binary stays within 1 GiB. Retained original artifacts and test
executables may be up to 4 GiB; that larger bound does not permit shipping them.

Use [derive-strip-debug.py](scripts/derive-strip-debug.py) on Linux to produce a
separate retained output and its `START.json`, `RESULT.json` and
`DERIVED-BINARY.json` records. It requires the pinned Ubuntu binutils 2.42
`/usr/bin/x86_64-linux-gnu-objcopy` bytes declared in the script. It refuses
another tool or version and never overwrites the original:

```sh
python3 -I scripts/derive-strip-debug.py \
  --binary-kind toolkit_gateway \
  --original /absolute/path/to/cargo/toolkit_gateway \
  --original-sha256 ORIGINAL_SHA256 --original-size ORIGINAL_BYTES \
  --output-dir /absolute/path/to/fresh-derived-output
```

Supply the resulting binary and its hash to the assembler with the existing
explicit resource/build selection, plus `--derived-binary-record` and
`--derived-binary-record-sha256`. Both derivation arguments are required. The
assembler reads the transformation records and independently compares ELF load
segments, allocated sections, build ID and dynamic dependencies. The original
Cargo build record remains byte-identical. Staging and final qualification name
the shipped bytes: acceptance of the original alone cannot qualify a derivation.
Gateway verification must read the staged derivation record, and final assembly
must join that same record.

The assembler retains all three transformation records alongside the staged or
final output. Its `package_envelope` measures the actual package, including
nested gzip tar members, raw tar padding and compressed size. Each member and
the final compressed archive must fit 1 GiB; the expanded outer tar stream must
fit 4 GiB. These checks do not replace installed-host or service qualification.
The assembler does not run the strip tool or a packaged executable.

The published v0.9.0 prefix and SDK assets are not replacements for these
inputs. The following released workflow is scoped to its original tag.

## Rebuild the released v0.9.0 installation

Use a checkout of the `v0.9.0` tag for this section, including that tag's manifests,
lockfiles and SDK sources. It selects the released Core/Meta 0.9.0 registry and
vendored-codegen composition. Do not run the following copy commands using the
current development checkout's binaries.

Download, verify and extract a complete CLI archive using the
[README instructions](README.md#install-and-use). That archive supplies the
matching integration library, runtimes and setup resources. Select its absolute
prefix path and resource digest:

| Platform | Complete CLI archive | SHA-256 of `libexec/meerkat-connect/resources.json` |
| --- | --- | --- |
| macOS arm64 | [meerkat-connect-0.9.0-macos-arm64.tar.gz](https://github.com/lukacf/meerkat-connect/releases/download/v0.9.0/meerkat-connect-0.9.0-macos-arm64.tar.gz) | `cbdc8c838413a732ebe4dbcca5e43da388d1c653beca3cfce102574401b61075` |
| Linux x86_64 | [meerkat-connect-0.9.0-linux-x86_64.tar.gz](https://github.com/lukacf/meerkat-connect/releases/download/v0.9.0/meerkat-connect-0.9.0-linux-x86_64.tar.gz) | `3280c10bef460edb19dba38099f97a115b7d49856e9dd9a092efa2a9097d5d5d` |

For example, for macOS arm64, from the source checkout:

```sh
connect_prefix=/absolute/path/to/meerkat-connect-macos-arm64
export MEERKAT_CONNECT_RESOURCES_SHA256=cbdc8c838413a732ebe4dbcca5e43da388d1c653beca3cfce102574401b61075
./scripts/repo-cargo +1.97.0 build \
  --manifest-path native-candidate/Cargo.toml --locked --release \
  -p meerkat-connect --features console,toolkit-mobkit/native-host \
  --bin meerkat-connect --bin toolkit_gateway
connect_target=$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')
cp "$connect_target/release/meerkat-connect" "$connect_prefix/bin/meerkat-connect"
cp "$connect_target/release/toolkit_gateway" "$connect_prefix/bin/toolkit_gateway"
```

On Linux, select the Linux prefix and digest from the table. The compiled digest
binds the executable to that resource inventory; retain the complete prefix:

```text
<prefix>/bin/meerkat-connect
<prefix>/bin/toolkit_gateway
<prefix>/libexec/meerkat-connect/resources.json
<prefix>/libexec/meerkat-connect/...
```

The repository wrapper uses Git to find the checkout and selects an isolated
Cargo home and target. For an unpacked source archive without `.git`, use ordinary
`cargo +1.97.0` with the same arguments and an explicit, separate
`CARGO_TARGET_DIR`. Copy binaries from that directory's `release/` subdirectory.
Use one writer per target directory.

Compilation can omit `MEERKAT_CONNECT_RESOURCES_SHA256` for source-only work.
Such a binary cannot prepare integrations, even if a resource directory is later
placed beside it. A complete installation needs the matching build-time digest
and prefix above.

## SDKs

For the `v0.9.0` checkout, the SDK distributions are the unchanged release assets
[`meerkat_toolkit-0.9.0-py3-none-any.whl`](https://github.com/lukacf/meerkat-connect/releases/download/v0.9.0/meerkat_toolkit-0.9.0-py3-none-any.whl)
and [`rkat-toolkit-0.9.0.tgz`](https://github.com/lukacf/meerkat-connect/releases/download/v0.9.0/rkat-toolkit-0.9.0.tgz).
See the [Python](docs/site/sdks/python.mdx) and
[TypeScript](docs/site/sdks/typescript.mdx) pages for installation. These are
GitHub release assets; no Toolkit registry upload is required.

For current development, use Connect's `sdk/python` and `sdk/typescript` from
the same finalized Connect source as the native binaries. The TypeScript
manifest and lock select published `@rkat/sdk` 0.9.8 and `@rkat/meta-sdk` 0.9.7;
the Python native-host extra selects `meerkat-meta==0.9.7`. Package installation
and SDK component checks do not qualify native or installed execution.

Earlier source-selected SDK checks used these development trees. Preserve their
source bindings when interpreting the historical results below:

| Dependency | Development source | Python | TypeScript |
| --- | --- | --- | --- |
| Core | `0d20d8cb09638c72df3207ccbcefc8d57103e23f` | `sdks/python` | `sdks/typescript` |
| Meta | Consumer projection `eceffb061db8acaf80fd5d011a4fa77441d25264` of owner source `477a9ec3453e9c5e6bf1024438b6518760a5d0ea` | `sdk/python` | `sdk/typescript` |

The Meta projection changes dependency selectors only. It does not publish or
qualify an SDK. Actual source/import checks, fresh library/starter production
and the resulting resource closure must bind the finalized candidate before
native or installed execution can use them.

The 4be SDK review
and 4be resource review
remain historical evidence for Connect `4beab4f`, Core `f1ae8b5a` and Meta
`9bb1e539`. They do not qualify another SDK selection.

The earlier source-selected Python SDK materialization and imports passed for
Connect `3e06cede` with Meta `a52502e6`; those receipts retain that scope. The
published Core/Meta 0.9.5 TypeScript packages passed compilation and 900 Connect
tests, with one unavailable native-fixture case skipped. See
[current integration status](docs/releases/follow-up.md#current-integration-status)
for the evidence and remaining validation.

The earlier Core `0a3cc302` / Meta `2c80ede4` selection included the
`session_events` TypeScript correction. Its matching Python compatibility
controls, TypeScript builds and selected runtime tests passed for those original
bytes. Core's public-type test initially failed on a stale supported-key list;
an isolated one-line test correction passed. Those results remain specific to
the original SDK bytes; they do not establish native or installed acceptance
for the selection above.
See [follow-up scope](docs/releases/follow-up.md#current-integration-status)
for exact results, the upstream correction's status and remaining installed qualification.

Connect's [Python metadata](sdk/python/pyproject.toml) is 0.9.9, with the native-host
extra requiring `meerkat-meta==0.9.9`. Connect's own
[TypeScript package version](sdk/typescript/package.json) is 0.9.9; its
development and optional peer dependencies are `@rkat/meta-sdk` 0.9.7 and
`@rkat/sdk` 0.9.9. The [TypeScript lock](sdk/typescript/package-lock.json) selects
those published tarballs and their actual integrity hashes. The
`10e5b80` validation
records the earlier 0.9.5 dependency update; missing-distribution checks from
that checkpoint are historical. The current declarations do not select the
earlier development Git SDK sources above or
qualify a release. Record source revisions and actual staged SDK bytes when
assembling a development product. Core supplies the public `Session` type.
Exact SDK source and installed payload checks remain part of candidate production.

Python optional extras select service and native-host dependencies.
`sdk/agno/stage.py` and `sdk/services/stage.py` project explicit source allowlists
from that tree into separate adopted distributions.

For TypeScript source, run `npm ci --ignore-scripts`, `npm run build`, then
`npm pack --ignore-scripts` in `sdk/typescript`. Activepieces has its own package
and scripts under `sdk/activepieces`; its build requires the checked-in vendor
manifest, source and license tree. Keep those hashed upstream inputs intact.

Run `npm run validate` in `sdk/typescript` for local validation. Its cross-language
projection tests require Python 3.11 or later and use `python3` by default. If that
command selects an older Python, set `TOOLKIT_TEST_PYTHON` to an explicit supported
interpreter, for example:

```sh
TOOLKIT_TEST_PYTHON=/absolute/path/to/python3.12 npm run validate
```

## Source checks and documentation

The [default GitHub Actions workflow](.github/workflows/checks.yml) uses Python
3.12 and Node 22 and defines the complete lightweight check set. Run its
packaging, documentation and Console panel subset from the repository root:

```sh
python3 -B -m unittest discover -s scripts -p 'test_package_connect_resource*.py' -v
python3 -B -m unittest -v scripts.test_assemble_connect_product
python3 scripts/check-docs.py
npm --prefix sdk/typescript ci --ignore-scripts --no-audit --no-fund
node --test sdk/typescript/tests/console-setup-panel.test.mjs
```

The packaging and assembler tests use only the standard library and temporary
inert archives.
Resource-test discovery includes archive bounds, mapped paths and symlink
preflight, plus verified input capture across pathname and file-content changes.
CI also runs the retained product readback and immutable-release integration
controls with `mcp==2.2.0`, alongside the pinned candidate-test dependency
`pytest==9.0.2`. Installed-contract controls import the selected native-host
dependency, `meerkat-meta==0.9.7`, and this checkout's `sdk/python/src`. To run
the preparation controls in an environment with the same MCP dependency installed:

```sh
PYTHONPATH=sdk/consumer-setup/tests python3 -B -m unittest -v \
  test_product_local_session test_product_official_readback test_immutable_release
```

These controls use retained test Controllers and controlled recipe programs;
they do not establish native MCP admission, real service access or Linux mount
execution.

The panel tests use the locked JSDOM dependency and import the embedded
JavaScript directly, without a TypeScript build. These checks do not build or
run native executables, install integration donors, or contact live providers.
Platform-specific Python controls may skip when the host lacks their filesystem
behavior. Native builds and installed-product acceptance remain separate.

For an integration tip or newly selected development pair, the validation owner
can optionally check the native packages and test targets without running tests:

```sh
./scripts/repo-cargo check --manifest-path native-candidate/Cargo.toml --locked \
  -p meerkat-connect -p toolkit-mobkit -p meerkat-toolkit \
  --features meerkat-connect/console,toolkit-mobkit/native-host --tests
```

Reuse the candidate's selected toolchain, resource digest and wrapper settings.
Do not overlap another validation command using the same candidate or target.
This compiler-only check does not run tests or replace installed acceptance;
native checks remain outside the lightweight default CI.

Run the Rust source tests separately:

```sh
./scripts/repo-cargo +1.97.0 test --manifest-path Cargo.toml --locked \
  -p toolkit-package
./scripts/repo-cargo +1.97.0 test --manifest-path native-candidate/Cargo.toml --locked \
  -p toolkit-store
python3 scripts/check-docs.py
```

The documentation check covers navigation, frontmatter and local links. Source
tests include their synthetic fixtures. Live service, deployment and hardware
checks use separate scoped environments.

Catalogs are committed release inputs. Historical private evidence locators
remain in their provenance, but rebuilding that evidence is not a public build
step. Some maintainer packaging scripts retain development defaults and need
explicit release inputs. The source-only commands use the checked-in source and
locked dependencies. The released installation commands use only the `v0.9.0`
checkout and its
published platform resources. Neither requires reconstructing a private
acceptance ledger; current-development installed qualification remains separate.
See [SOURCE-PROVENANCE.md](SOURCE-PROVENANCE.md) before changing catalog bytes.
