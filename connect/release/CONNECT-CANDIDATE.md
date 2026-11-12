# Connect native candidates and publication

## Selected 0.9.4 release route

The release lead permits direct retained-host production for Connect 0.9.4,
including the gateway artifacts consumed by Forge. Linux and macOS retain their
exact source, dependency, SDK, artifact and consumer-verification records. These
direct artifacts do not claim GitHub-hosted build attestation. The complete
Connect asset set is first published as a prerelease that is not latest; that same
release becomes final after the required verdicts. Forge receives the actual
product selection, file pins and relocation paths for those published bytes.

The hosted workflow below remains available as a separate production route. It
is not a prerequisite for the selected direct 0.9.4 release.

## Hosted native candidate workflow

This manual-only workflow is source preparation for an explicitly selected,
supported Core/Meta pair. It does not publish, tag, install on a user host, dispatch another
workflow, or qualify any bytes until its actual producer and verifier succeed.
Connect's native package version and application SDK version are independent
explicit selections, checked against the dispatched source metadata. There is
no default to the public 0.9.0 artifacts or to a development pair.

Dispatch `linux-x86_64` and `macos-arm64` independently. Each dispatch selects
one target and that target's committed input-file digest; neither target can
stand in for the other. Linux uses `ubuntu-24.04`, and macOS uses `macos-15`,
which GitHub lists as an arm64 runner in its
[hosted-runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
The producer also checks the actual operating system and architecture before
any native build. A runner label alone is not target evidence. The separate
attestation job remains on Ubuntu: it hashes and attests the selected bytes
without running the product, so it does not require a matching native host.

The existing producer path is used without weakening its acceptance:

1. Fetch each exact public input by URL, SHA-256 and byte size.
2. Fetch the exact selected Core and Meta commits and check any explicitly
   selected tags against them. Check the selected Cargo manifest/lock,
   complete actual package/feature/dependency graph, and the original public
   Meta SDK wheel against that source. Build the Connect wheel from a
   scratch copy of the dispatched `sdk/python` using its existing PEP 517 backend
   and an explicit offline build-tool wheel batch. Compare the complete original
   wheel payload to the dispatched source; retain its command, log and metadata.
3. Project the selected library name/hash/bytes into the existing starter manifest
   shape, explicitly marked as an unpublished candidate input. Run the existing
   `export-setup-starter.py` with that exact manifest and digest to produce all
   six starter files from this source. Then run `package-connect-resources.py`
   with every library/starter/runtime input explicit and retain `build.json`.
4. Build only `toolkit_gateway` through `repo-cargo`, with the resource digest
   bound in the actual command environment. Retain the actual compiler artifact,
   command/log, source inventory, metadata and native build record.
5. Stage through `assemble-connect-product.py`, then run the existing
   `verify_native_gateway_subprocess.py --case installed-product` against the
   staged gateway and the selected installed SDK wheel bytes. Both its success
   and resource-corruption cases must pass their existing cleanup assertions.
6. Final assembly consumes that exact verifier report and all of its original
   records. The emitted scope remains
   `installed_gateway_schema4_resource_startup_and_shutdown_preview` and its
   existing exclusions remain in `assembly.json`.
7. Emit the existing `forge-meta-toolkit-runtime-selection-v1` shape, then invoke
   the selected Forge source's actual `parse_candidate_selection` and
   `read_meta_toolkit_artifacts` reader. Its absence or incompatibility refuses.
8. Upload the whole candidate closure once. A separate job, which executes no
   native product, checks the original upload ID, run/attempt/source and archive
   digest, then attests those same closure/manifest bytes. It does not rebuild.

The source archive is the public dispatched commit. Candidate source SHA,
workflow SHA and dispatch SHA must agree. There is no restored Cargo/target
cache and no secret needed for downloads or native execution. The attestation
job alone receives OIDC/attestation permissions. The GitHub-hosted runner's
actual tool versions and package inventory are retained (dpkg on Linux;
Homebrew and Xcode SDK on macOS). Python 3.11 and Rust 1.97.0 are selected on
both hosts, matching the selected application dependency closure. macOS requires the native Xcode tools, zstd, pkg-config and OpenSSL;
the workflow records the selected OpenSSL location. The runner image and
system package set are observed, not claimed reproducible from this draft.

## Standalone CLI from the retained producer

The gateway workflow above does not constitute the standalone Connect release.
For 0.9.4, the standalone CLI may use the existing retained native producer,
independent shutdown review and assembler route, retaining checksums, the review
record and the assembler input inventory. It does not require a new CI pipeline.
The selected 0.9.4 direct route also permits the resulting artifacts as Forge
inputs. Preserve original build and verification evidence; do not describe a
retained-host artifact as GitHub-hosted or attested.

The optional CI wrapper below reuses the gateway producer's exact prepared source,
native graph and resource closure. `stage-cli` still requires its CI preflight;
use the existing direct producer and assembler for the retained-host route.

```sh
python3 scripts/connect-candidate.py stage-cli \
  --candidate-work /absolute/retained/connect-candidate

python3 scripts/connect-candidate.py assemble-cli \
  --candidate-work /absolute/retained/connect-candidate \
  --shutdown-review /absolute/review/native-shutdown-review.json \
  --shutdown-review-sha256 <exact-reviewed-record-sha256> \
  --rules /absolute/private/release-privacy-rules.json \
  --exemptions /absolute/private/release-privacy-exemptions.json \
  --output /absolute/new-cli-assembly
```

`stage-cli` uses the same selected dispatch environment and native host as
`build`. It builds the ordinary `meerkat-connect` executable through `repo-cargo`
and copies those production bytes before compiling `packaged_setup`, so test
features cannot replace the selected CLI. Both builds bind the same resource
digest. The existing assembler creates an unqualified prefix; the exact test
executable is copied beside the installed CLI. The two existing ignored tests
then run outside the checkout with an empty runtime PATH:

- `installed_product_native_catalog_preparation_and_help`
- `installed_product_native_shutdown_joins_active_worker`

Each run must actually execute one test and succeed. Actual command records,
logs, native traces, fixture selection and process markers stay under `cli/`.
The composition record describes the test execution. The shutdown template is
`cli/native-shutdown-review.pending.json`, in the existing review record shape
with `disposition: pending`. A passing test never changes that disposition.
The existing independent reviewer inspects the raw observations and emits the
accepted record separately, retaining its exact candidate and evidence list.
There is no new user approval step. Failed execution retains its records and
cannot reach final assembly.

`assemble-cli` checks the supplied review digest, invokes the existing
composition/shutdown validators, and reuses the exact selected CLI, fixture and
resource bytes. It performs no build or test execution and preserves the original
build record. It creates a separate final build record with the two evidence
references, then invokes the existing assembler. The result is the existing
`conversation_catalog_local_preparation_and_help_preview`, with
`cli_tty_qualified: false`. It is neither a Forge gateway selection nor a public
release, and it retains the assembler's existing exclusions. It then scans the
assembled product and its `.sha256` file for private text (see "Scan the release
payload for private text" below) and refuses a product that the scan refuses:
that product is moved to `assembled.refused`, so nothing remains at `assembled`.

These CI wrapper commands require the same retained producer directory through
independent review and assembly. The existing records name absolute executable
and evidence paths; these commands do not relocate them or rewrite historical
execution argv. The current ephemeral GitHub workflow does not yet keep that
producer alive across review. Standalone CLI CI migration is a 0.9.5 item; it does
not block the permitted retained-host route. Cross-job restoration is an optional
implementation choice. Actual production, independent review, consumer acceptance
and assembly remain required on the selected route.

The actual standalone release still needs the existing
`cli_paired_host::actual_cli_public_service_survives_setup_exit_and_fresh_rkat_in_exact_realm`
consumer against the selected packaged bytes at the RC build. Reuse the current
native consumer driver with `CONNECT_REAL_LIBRARY_PREFIX` pointing to the staged
prefix, `CONNECT_SUPPLIED_CLI_SHA256` set to the selected production CLI hash, and
`CONNECT_FINAL_RKAT` / `CONNECT_FINAL_RKAT_SHA256` identifying the matching ordinary
Core executable. Compile that fixture with the same embedded resource digest.
It exercises setup, normal exit, a fresh ordinary `rkat` session in the same realm
and three real Hacker News reads; its controlled model selection is explicit.
Retain its actual CLI/rkat/test/resource identities, answers and cleanup beside
the component preview. Neither these package tests nor an older successful
journey qualifies different RC binaries. Autonomous live-model acceptance is a
separate observation, not inferred from the controlled journey.

## Scan the release payload for private text

Every release file is scanned for private text before it is attested or
uploaded. One engine does the matching and the container checks:
`scripts/release-evidence-privacy-scan.py`, kept unchanged and pinned by its
sha256 in `scripts/release-payload-privacy-scan.py`. The engine decodes nested
containers four levels deep, does not unpack zstd and reads no member or stream
larger than 1 GiB. The wrapper walks each payload file with the engine's own
traversal, and every container the engine does not enter becomes an extra
artifact of the same engine run: one nested deeper than four levels, a zstd
container (decompressed), and a compressed tar past the 1 GiB bound, such as a
large product archive (cut at member boundaries into pieces below the bound,
keeping every original byte). The scan therefore reaches every level: a
product archive, the integration library inside it, the wheels, crates and
source archives inside the library, and what they hold.

```sh
python3 scripts/connect-candidate.py privacy-scan \
  --rules /absolute/private/release-privacy-rules.json \
  --exemptions /absolute/private/release-privacy-exemptions.json \
  --accepted-findings /absolute/private/accepted-findings.json \
  --output /absolute/new-privacy-scan \
  /absolute/staged/<every file to attest or upload>
```

- The rules and exemptions are private and are never committed. Release mode
  takes both on the command line and refuses without them; there is no default.
- Scan every file that will be attested or uploaded: both product archives with
  their `.sha256` files, the evidence and every other release asset. The
  embedded integration library is reached through the product archives.
- The scan fails closed. Any hit refuses, and so does any error the engine
  reports (a container it cannot decode, a damaged or unsupported archive, a
  member over its size bound, an unreadable input), unless an accepted finding
  below covers it exactly. A member over the size bound and an unreadable input
  are never accepted.
- `--accepted-findings` is optional. Its entries (schema
  `release-payload-privacy-scan/accepted/1`) are findings the release owner
  accepts in exact bytes, such as a third-party file whose text happens to match
  a rule. Each entry names a category, a rule id, the sha256 of the file that
  holds the finding (or of a container that encloses it) and a one-line reason.
  A structural error is accepted with the category `structure`, the engine's
  error kind as the rule, and the sha256 of the container it was reported in.
  The same finding in any other bytes refuses, and without the file nothing is
  accepted. The file stays private, beside the rules.
- `report.json` holds counts only: hits and errors, accepted and refused, by
  category and kind, and each payload file's name, size and sha256.
  `detail.json` holds the matched strings. It is private (mode 0600) and is
  never uploaded.
- CI runs only the tests (`scripts/test_release_payload_privacy_scan.py`), with
  synthetic rules: a made-up marker in
  `scripts/fixtures/release-payload-privacy-scan/`. CI does not scan release
  bytes.

## Complete macOS installation from one commit

The complete macOS arm64 installation (`meerkat-connect-<version>-macos-arm64.tar.gz`
and `meerkat-connect-gateway-<version>-macos-arm64.tar.gz`: each executable with
the bundled Python, Node, integration library and setup resources) is produced
by `scripts/connect-candidate.py macos <step>` on a GitHub-hosted `macos-15`
runner. It uses none of the candidate inputs above. Everything comes from one
commit: the checkout builds Connect, both SDK wheels and the integration
library's six native tools; pinned public files are read from the directory the
fetch step filled; the producer itself downloads nothing. Nothing in it uses a
signing identity, a retained host or a private input.

Stage one, one step per workflow step, each with `--work <absolute work root>`:

| Step | What it does | What it leaves |
| --- | --- | --- |
| `preflight --source-sha SHA` | Checks the host (Darwin arm64), the clean checkout at SHA, the macOS selection, and that the active Rust toolchain is the one `rust-toolchain.toml` names. Records the tool versions and writes the source manifest. | `records/preflight.step.json`, `inputs-selection.json`, `source-manifest.json` |
| `library --pins FILE --pinned-inputs DIR` | Builds the six tools of `toolkit-package` with Cargo, signs each, writes each tool's build record, lists their crates, then calls the component build, the consumer exporter, the kit packager and the library packager. | `tools/`, `components/`, `consumers/`, `kit/`, `library/` |
| `resources --pins FILE --pinned-inputs DIR` | Exports the starter for exactly this library and calls the resource packager with the darwin Node prebuilt policy. | `starter/`, `resources/`; prints the resource digest |
| `build` | One Cargo build of `meerkat-connect` and `toolkit_gateway` with the resource digest bound, then for each: copy, ad-hoc signature, verification, Mach-O facts, `--version`. | `binary/`, `records/gateway-build-record.json` |
| `stage --pins FILE --pinned-inputs DIR` | One Cargo test build of `packaged_setup` and `cli_paired_host`, both SDK wheels, and the two unqualified prefixes through `assemble-connect-product.py --stage-only`. | `binary/`, `sdk/`, `gateway-stage/`, `cli-stage/`, `records/cli-build-record.json` |
| `check-gateway` | The installed gateway verifier (`--case installed-product`) in a virtual environment that holds exactly the two SDK wheels. | `gateway-verification/` |
| `check-cli` | The two installed CLI tests, each run once with an empty PATH. | `cli-qualification/installed-composition.json`, `native-shutdown-review.pending.json` |
| `check-consumer` | Setup, exit, then a fresh ordinary `rkat` in the same realm. `--rkat-archive FILE --rkat-archive-sha256 HEX` takes Core's published archive; `--rkat-from-checkout` builds `rkat` from this commit. | `cli-qualification/consumer.json` |
| `check-conversation` | Starts the staged `meerkat-connect` on a pseudo-terminal and closes it once it printed past its resource step and waits. | `cli-qualification/conversation-start.json` |
| `check-official-install` | Runs the official Playwright install through the library's own installer, as the setup backend calls it: the closure's Python, `--node` and `--npm-cli` from the closure's runtime Node, the library's `official/` bundle, and a host profile that pins the runner's Chrome by digest. The install hashes the browser and never starts it. Without a Chrome executable (`--chrome FILE` names another one) the check is recorded as `not_run` with the reason, never as passed. | `cli-qualification/official-playwright.json` |
| `stage-record --output DIR` | Assembles the gateway product and writes the stage record. | `DIR/STAGE.json` and the files it lists |

A step runs once per work root, needs the records of the steps before it, and
refuses when the checkout is no longer the commit that `preflight` recorded. No
step reads the Cargo target directory of an earlier step, so the steps can be
split over jobs that restore the work root at the same absolute path.

Signing. Each executable is copied out of Cargo's output and signed with
`/usr/bin/codesign --force --sign -` before any record names its digest:
signing changes the file's bytes, and the build record, the staging record and
the assembly all bind those bytes. A record takes an executable's digest only
through the signature record written for the same bytes
(`<executable>.signature.json`), and the producer reads the code signature
back from the file: it must be codesign's ad-hoc signature, not the one the
linker leaves. The two test executables and the six tools are signed the same
way. No Developer ID is used and nothing is notarized.

What the producer refuses in an executable: anything but a thin arm64 Mach-O
executable, a linked library outside `/usr/lib/` and `/System/Library/` (a
user's Mac would not have it), a minimum macOS above the selection's
`minimum_macos`, and a `--version` output without the tree's Connect version.
These facts are read from the file in Python. Hashing, sizes, free space and
archives are Python too: no GNU-only tool is used.

Environment. Cargo inherits the job's toolchain environment. Every other
child, and every product executable in particular, gets a complete
environment written by the producer (`HOME`, `TMPDIR`, `PATH`, `LC_ALL` and
the check's own variables), so no variable of the job reaches a product.

The stage record. `STAGE.json` (schema `connect-macos-stage/1`) names the
commit and tree, the checkout and work root, the versions, the selection, pin
file and resource digests, the signed executables, the six gates, and every
file of the directory with its size and SHA-256. It holds the finished
gateway archive, which needs nothing from the review, marked
`assembled_not_scanned_not_published`. It never holds the CLI archive: the
assembler refuses that product without the accepted shutdown review, and
stage one leaves the review `pending` whatever the tests said.

Network. The library step's component build runs `npm ci`, which reads the
public npm registry for integrity-locked packages: the build's one network
use. The gateway verifier and the consumer check read live public pages
through the supplied integration, with a controlled model and no provider
key. The other checks are expected to need none. For each step the stage
record states that expected use, as read from the source, and the host names
the step's own logs and evidence name. The records name expected hosts and do
not measure contacts: the producer captures no traffic, and every entry says
so (`captured: false`).

Stage two. The release owner reads the stage record's evidence and accepts
the shutdown record, the assembly follows, and a clean private-text scan of
the exact archives is the last gate:

```sh
python3 scripts/connect-candidate.py macos accept-review \
  --stage /absolute/downloaded/stage-record \
  --output /absolute/new/native-shutdown-review.accepted.json

python3 scripts/connect-candidate.py macos assemble \
  --work /absolute/stage-one/work-root \
  --stage /absolute/downloaded/stage-record \
  --review /absolute/new/native-shutdown-review.accepted.json \
  --review-sha256 <the sha256 accept-review printed> \
  --output /absolute/new-assembly

python3 scripts/connect-candidate.py macos release-check \
  --assembly /absolute/new-assembly \
  --privacy-report /absolute/report.json \
  --privacy-report-sha256 <exact-report-sha256>
```

`accept-review` records the acceptance. The reader first reads
`cli-qualification/native-shutdown-review.pending.json` in the stage record and
the files its `evidence` rows name; running the command is the act of
accepting them. It checks every file of the stage record, then that each
observation the pending record binds is there with exactly its bytes, writes
the accepted record (the pending record with `disposition: accepted`, the
pending record's digest and the stage record's digest) and prints its SHA-256.
It runs anywhere and never changes the stage record.

`assemble` runs no product, no Cargo and no signing. It runs on macOS, at the
stage's own work root and checkout paths: the records bind absolute paths, the
assembler requires the build's `--manifest-path` to be this checkout's, and the
product tree's symbolic links carry modes on macOS that the installed
executables compare with their inventory. It restores the stage record, checks
every file, requires all gates passed (the consumer check against Core's
published `rkat`, never one built from the checkout), requires the accepted
review to cover every observation of the pending one, and calls the assembler
for the CLI product. Its output holds both product archives and
`ASSEMBLY.json`, and publishes nothing.

`release-check` is the last gate. The private-text scan described below runs
where the private rules are, over exactly four files: the two product archives
and their `.sha256` files. `release-check` takes that scan's report and its
SHA-256, nothing else of the scan. It refuses unless the report is `clean` and
names all four files by name, size and SHA-256, and only then writes
`PUBLISHABLE.json` with those four rows, the report's digest and the digest of
the scanner's list. A report may hold counts by class, file digests, the
digest of the scanner's list, a schema name, a status and a tool version. It
never holds matched text, token text or the list: `release-check` refuses a
report with any other text in it, and one that names a file that is not one
of the four. The rules never enter the workflow.

Publication is not part of this producer and never part of a build job. It is
a separate add-only step after the scan. That step must require
`PUBLISHABLE.json` and check the four digests in it against the files it
uploads, immediately before the upload.

The macOS selection is `connect-release-inputs-macos-arm64.json`
(`schema_version` 2, kind `connect-macos-producer-inputs`): the toolchain, the
Cargo features, the public commit that imported `connect/`, the minimum macOS,
the free-space bound, the five SDK build tools by version, the darwin Node
prebuilt policy and the library pin file, each committed file with its
SHA-256. It names no product version: versions are read from the checkout.
Its library fields are pending in this tree (`"state": "pending"`, a null pin
file digest), and every `macos` step refuses a pending or unbound selection.
It is completed by setting the committed pin file's SHA-256 and `"state":
"selected"`.

`node-prebuilt-policy-macos-arm64.json` is the Linux policy's shape for
`darwin` and `arm64`, with no libc and the SHA-256 of `bin/node` inside the
pinned Node v24.19.0 archive. The resource packager refuses a policy whose
digest is not that of the Node it packages.

Not shown by anything that runs on Linux, and so only by a hosted `macos-15`
run: that the builds fit the job's time and disk, that `codesign` produces the
signature the producer requires, that the six tools and both executables link
system libraries only, and that the gateway verifier, the two installed tests,
the consumer check, the conversation start and the official Playwright install
pass on macOS.

## Required inputs still unbound

`connect-release-inputs.json` selects Linux for the hosted candidate workflow
above and contains deliberate nulls: it refuses before any native build. The
macOS file is no longer a candidate input (see "Complete macOS installation
from one commit"); the candidate commands refuse it as unbound, as before.
The existing owners must supply all of the following for the Linux candidate:

- The final dispatched public Connect source commit, its native package version,
  SDK distribution/version, and exact native Cargo manifest/lock hashes.
- The exact Core and Meta source commits for one supported pair: Core `0.9.4`
  with Meta `0.9.4`, Core `0.9.5` with Meta `0.9.5`, or Core `0.9.7` with Meta
  `0.9.6`. Meta `0.9.6` requires Core `0.9.7`; the `0.9.6`/`0.9.6` pair is
  refused. The Meta SDK and Connect's `native-host` dependency must match the
  selected Meta version, so they remain `0.9.6` for the `0.9.7`/`0.9.6` pair.
  Each existing `native_release.core` / `native_release.meta` selector has
  exactly `repository`, `version`, `source_sha` and `tag`. `source_sha` is a
  full lowercase 40-character commit SHA. Set `tag` to `""` for an explicit
  commit-only selection, including an offered RC before final tags or registry
  publication. A nonempty tag is an additional binding: its exact
  `refs/tags/<tag>` must peel to that same commit. Tags use 1-128 ASCII letters,
  digits, dots, underscores or hyphens, start with a letter or digit, contain no
  `..`, and end with neither `.` nor `.lock`. No branch, default HEAD, inferred
  RC tag or final-release tag is substituted. A tag check does not establish
  publication or release authorization. The draft's existing final-tag values
  remain unbound until the owner supplies its exact selection. Their complete
  selected Cargo package IDs, features, dependency edges/kinds/target predicates
  and native family membership must match. Workspace IDs alone use the explicit
  `path+workspace:///` comparison form. `expected_resolve` contains `root` and
  the complete sorted `nodes`; native null root/target become empty strings and
  null dependency kind becomes `normal` only for comparison. Raw Cargo metadata
  remains unchanged.
- A complete `files` inventory. Each key is a safe relative locator and each
  value is exactly `{url, size, sha256}`. Only pinned GitHub release assets from
  the named public repositories, PyPI file URLs and versioned Node downloads
  are accepted, plus the existing resource owner's exact official npm 12.2.0
  tarball and pip 26.2.1 zipapp URLs. HTTPS, credential/query/fragment exclusion,
  exact size and SHA-256 checks still apply. No private bucket, local path or
  expiring input is accepted.
- The complete selected library archive, Python/Node/pip upstream runtime files
  required by the existing resource packager, acquisition
  Node/npm archives, and selected Node prebuilt policy. `node_build_profile` is
  an optional explicit locator; an empty string means no source-build profile.
  This does not qualify arbitrary source builds or imply a source-build toolchain.
- The original public Meta SDK wheel and complete `source_files` maps for both
  SDKs, from each non-dist-info wheel member to its exact selected repository-
  relative source file. Connect source supplies its own wheel through its current
  build backend; no wheel payload or version is rewritten. The source must
  declare the matching exact `meerkat-meta` native-host dependency.
- A complete offline SDK build-tool requirements file and wheel batch, including
  the build frontend and the selected source backend requirements. No versions
  are inferred. `--no-isolation` uses only that fresh build environment; the
  existing source-owned backend remains responsible for creating the wheel.
- An independent `forge_reader_source` with exactly `repository`, `source_sha`
  and `tag`. The repository is `lukacf/meerkat-meta`; the lowercase 40-character
  commit selects the actual Forge installer reader, which ships under `forge-v*`
  tags independently of Meta's SDK and native release. An empty tag permits an
  explicitly reviewed commit; a nonempty safe `forge-v*` tag must peel to that
  exact commit. The four `forge_reader_source_pins` hashes bind its existing
  owner files. Missing or differing source refuses before resource/native
  production. This checkout supplies both dependency-batch readers and the
  final artifact reader. It never substitutes for the native Meta checkout used
  to compare the selected Meta wheel's source bytes.
- One complete offline dependency requirements file and wheel batch. The
  existing Forge reader validates both this batch and the SDK build-tool batch
  before either install: only exact single-hash package pins, with original
  matching wheels, are accepted. Direct URLs, includes and options refuse.
  The verifier installs
  only these pinned bytes and the original just-built SDK wheel, with no
  dependency download or second SDK build.
- SHA-256 of the completed committed input file, supplied with the exact source
  SHA at dispatch. The draft input file is not an accepted release selection.

Retained producer paths are historical provenance data. They are preserved byte
for byte and are never treated as local input locators by this producer. An
absolute path in a build record, wheel metadata or example does not by itself
require rebuilding the donor. Existing compatible library and tool artifacts may
be reused with their original records and exact hashes; this workflow does not
rewrite provenance or invent missing receipts. Metadata parsing, archive member
and link bounds, selected source/SDK equality, native graph validation and the
actual assembly/Forge reader checks remain required.

The retained library `d0a20df31dfca19129ba36edcad0a2cb46295476bdbf0630bee224cb7c41c71f`
(128051975 bytes) still matches all 45 selected current public source leaves;
its other selected rows explicitly retain historical inputs. This supports
unchanged input reuse, not a claim that a 0.9.4 installed consumer has passed.
The library and Node policy still need selected public artifact URLs. Exact
Core/Meta 0.9.4 revisions, SDKs and actual reader/consumer qualification remain
unbound. The Connect wheel and starter are generated by their existing owners
in this workflow, so they do not depend on publication of the same candidate.

## Accept an exact candidate without write credentials

`accept-publication` accepts an already emitted candidate. It does not run the
build or fetch commands. Use a separate environment without GitHub write
credentials; its verifier and Forge-reader child processes receive a scrubbed
home/configuration and no inherited credential environment.

```sh
python3 scripts/connect-candidate.py accept-publication \
  --request /path/to/selected-publication-request.json \
  --candidate /path/to/candidate-files \
  --forge-reader-source /path/to/selected-forge-checkout \
  --output /path/to/new-acceptance-directory
```

These paths describe a future selected operation, not runnable release inputs
from this draft. The request must contain exactly these ten fields:

| Field | Required value |
| --- | --- |
| `source_sha` | Exact 40-character lowercase Git commit SHA of the candidate's public Connect source. |
| `source_tree` | Exact 40-character lowercase source tree SHA retained by that candidate. |
| `source_ref` | Selected branch ref in `refs/heads/...` form. |
| `run_id`, `run_attempt` | Positive integer identities of the same workflow invocation. |
| `inputs_sha256` | Exact 64-character lowercase SHA-256 of the original committed input selection. |
| `connect_version` | Exact selected native package version in `major.minor.patch` form; not inferred from Core or Meta. |
| `release_id`, `tag` | Positive integer ID and exact tag of the existing release selected for later publication. |
| `target` | Exactly `linux-x86_64` or `macos-arm64`, matching the manifest, input selection and original runtime selection. |

The candidate directory must supply these three original files:

- `connect-<target>-candidate.tar.gz`, the complete candidate closure below 2 GiB.
- `connect-<target>-candidate.json`, its original attested manifest, at most 32 MiB.
- `connect-<target>-provenance.jsonl`, the attestation bundle for both subjects, at most
  32 MiB. Unsigned bundle JSON is not verification evidence.

For example, the macOS archive is `connect-macos-arm64-candidate.tar.gz`.
Every subject and bundle name includes the target, so the two independently
accepted candidates can coexist on the same selected release without overwrites.

The repository is fixed to `lukacf/meerkat-connect` and the workflow to
`.github/workflows/connect-candidate.yml`. GitHub's `gh attestation verify`
verifies each subject; the checked result must match the exact source,
workflow, ref, run/attempt, issuer and GitHub-hosted runner. The manifest and
original input selection bind the input digest, native releases and Connect
version. The original input digest also binds the independent Forge reader
selection. That checkout's HEAD must be the exact selected commit, with a clean
original `installer/launchkit` package and matching reader source pins.
If the selector includes a tag, its explicit `refs/tags/<tag>` must also resolve
to that commit. Commit-only selection requires no tag. Neither an SDK wheel
nor a separate reader wheel substitutes for that checkout. Both paths retain
the exact selected native package and Meta SDK versions, SDK source comparison,
graph checks and actual owner-reader acceptance.

Historical attested inputs without `forge_reader_source` retain their original
exact `native_release.meta` reader selection. An explicit null or malformed
reader selection refuses; it never falls back. New production requires the
independent field. `--meta-source` remains a legacy spelling for the reader
checkout, with the same checks; the two CLI spellings cannot be supplied together.

Acceptance performs bounded data-only extraction, preserves original bytes,
and checks the complete retained inventory. The selected Forge source supplies
`parse_candidate_selection`, `packaged_runtime.relocate_selection` and
`selected_runtime_artifacts`; only that owner relocates physical paths in its
in-memory view. The original v1 selection and producer records are not
rewritten. The reader must accept `installed-gateway-assembly` with the exact
installed preview scope, including the original product checksum sidecar.
Development and staging substitutions refuse.

The output directory must not exist. Success writes `acceptance.json` with the
exact request, scope, three asset hashes/sizes, original selection, physical
layout map and owner-validated evidence inventory; `publication` remains
`not_performed`. It also retains `assets/`, `unpacked/`, the layout and both
verified subject results. A refused acceptance may leave partial local files;
it produces no completed acceptance record. Select the exact SHA-256 of the
successful `acceptance.json` independently before handing it to the writer.
This record is a local operation handoff, not a new runtime-selection format.

## Publish only the accepted bytes

`publish-accepted` is the separate write-capable command. It requires an already
public repository and an existing non-draft release whose tag resolves to the
accepted source. It creates no release or tag, changes no release state and
overwrites no conflicting asset. Release creation and authorization remain with
the release owner.

```sh
python3 scripts/connect-candidate.py publish-accepted \
  --accepted /path/to/acceptance-directory \
  --acceptance-sha256 "$SELECTED_ACCEPTANCE_SHA256" \
  --output /path/to/new-publication-observation
```

`--accepted` selects the prior `acceptance.json`, `assets/`, and the retained
`unpacked/` text evidence. The writer rechecks each text file against the exact
size and SHA-256 in that independently selected acceptance.
`--acceptance-sha256` is its independently selected exact lowercase SHA-256;
`--output` must not exist. The writer parses the same bytes it hashes, retains
and rechecks the three allowlisted assets, and repeats attestation verification
before any upload. It also checks the complete prospective Forge fragment and
all pinned text evidence as described below. Only those original three assets can be added. It imports
no product or Forge code and performs no archive extraction. Each upload keeps
the verified bytes in an open anonymous file through `gh --input -`, so replacing
an input path cannot substitute unaccepted bytes at the public write.

The writer checks actual public repository visibility and exact release/tag
identity before upload and on subsequent release observations. Existing equal
assets require unauthenticated public readback without reupload. All selected
assets already present are checked before the first upload, so a conflict
visible at that point is detected before any write. Newly uploaded
assets require the same size/digest readback. A final state check must preserve
all original and newly verified asset identities. These are observations at
the time of publication; they do not promise that another owner will never
change the release afterward.

Once the writer enters its retained-asset phase, `publication.json` records
`refused_or_partial`, attempted uploads and verified assets if any step fails.
Earlier argument or handoff refusal may occur before that report exists. No
partial result contains a final file inventory, and uploaded assets are not
deleted as rollback. Only complete readback and final checks produce
`status: verified_public_inventory`, the selected acceptance digest, release ID,
tag, unchanged layout, and public wrapper-member URLs with exact archive/member
sizes and hashes. `forge-fragment.json` contains exactly `files`, `selection`
and `paths`. Its selection omits only `artifact_root` and the physical `path`
fields of artifacts and dependency wheels. All other values and row order are
preserved. `forge-fragment-attribution.json` binds that fragment to the selected
acceptance and the unchanged original `RUNTIME-SELECTION.json` archive member.
Forge supplies its own Guide and ordinary-gateway inputs. An Actions artifact
alone is not the final public handoff.

### Forge delivery checks

Acceptance preserves historical producer data. A successful acceptance can
still be incompatible with final delivery. Before any public write, the writer
checks the portable fragment's keys and string values, then every owner-selected
pinned `.json`, `.jsonl`, `.md`, `.txt` and `.sha256` file, including indirect
build, resource, gateway-verification and source-manifest records and the
original runtime selection. It reads each retained text file once, checks its
selected bytes, and scans that same in-memory copy. The existing 32 MiB metadata
bound also applies to these reads. Missing, changed or incompatible text refuses
without rewriting evidence or uploading an asset. Successful observations appear
under `forge_delivery` in `publication.json`.

The rules are selected from `forge-fill-inputs-030.py`, SHA-256
`73d9a50e5f7644aa5fa05008b47a25259543a079fde1f8232f95ca31846ec3a4`. Its corrected home-prefix rule permits
`/home/.cargo/` in pinned evidence and refuses `/Users/.cargo/`. The fragment must
contain relative or URL-based fields only: absolute paths, home prefixes and
private-name matches refuse. Pinned evidence uses the owner's narrower personal
home-path rule, permitting `/Users/runner/`, `/home/runner/` and `/home/.cargo/`.
These exceptions never admit such paths into the portable fragment.

Decoded JSON keys and string values are checked, so escaped slashes and Unicode
escapes cannot hide forbidden content. The exact selected wheel file entries
(F1) and `source-manifest.json` top-level `files[].path` entries (F1b) exempt
private names only; personal paths always refuse. JSON object keys and sibling
fields receive no F1b exception. Other text, including JSONL, uses the selected
helper's raw-text fallback and exact selected wheel-name removal for its private
name check. No new private-name or path exception is inferred from an example.

Use fresh actual hosted-runner production for final evidence. Do not sanitize
old records or copy their hashes onto projected replacements: the source,
resource, build, verification and assembly records bind one another and the
original attested archive. Selected external inputs may still contain historical
paths, so hosted production alone is not a delivery pass. The real selected
Forge fetch/fill/package check against the final public pins remains separate;
Connect checks only its own complete contribution, not Forge's additional inputs.

## Scope and remaining work

The fragment projection and delivery guard passed the offline producer and
publication suites on 26 October 2026: 499 passed, with one Linux
case-sensitive-filesystem check skipped on macOS. All 152 selected guard and
provenance cases passed. These use controlled signature, reader and transport
fixtures; actual hosted production, public Forge acceptance and native product
qualification remain separate.

The candidate workflow was checked by source reading, Python AST parsing,
JSON parsing, YAML parsing and exact patch roundtrip. The publication commands
also passed 201 offline controls using controlled verifier results, a controlled
reader and transport responses. Those controls include input substitution,
conflicting assets at each position, partial writes, repository visibility
changes, commit-only/tagged native selection, and real local Git fetches that
refuse commit/tag mismatches before checkout. Both targets exercise the existing
acceptance and publication controls. Additional checks cover native host/target
mismatches, data-only attestation, unchanged historical producer paths, valid bounded metadata, target
binding mismatches and preserving the other target's release assets. The exact
npm/pip additions also reject alternate locators and mismatched downloaded bytes.
They do
not establish actual signature verification, selected Forge-reader acceptance,
public upload or Forge inclusion. No Cargo, packager, native verifier, native
binary, GitHub job or remote host was executed for this draft. Actual attested
candidate inputs and the exact selected Forge reader must still pass acceptance;
the selected source and input joins need a real owner-authorized CI run.
Each target's installed-product scope is narrower than shared setup workflow,
acquisition, account login, model/task completion or installation. Success on
one target establishes nothing about the other target's execution.

Both target input inventories, exact source graph and SDK selections remain
unbound. No current native macOS build or installed-product execution has been
measured. The existing 120-minute build-job and 15-minute attestation-job bounds
are limits, not completion estimates or promises for either platform.

There is no Connect release workflow or prior trigger policy to extend at the
reviewed public source. This adds only `workflow_dispatch`. Candidate publication,
exact-main CI qualification, consumer acceptance, immutable public asset URLs and
Forge inclusion remain with the existing release owners. An Actions artifact is
not a public release asset and does not satisfy Forge's public-fetch allowlist.
The complete wrapper is required to stay below 2 GiB, matching Forge's public
input/archive bound; an oversized closure refuses before upload/attestation.
The final product archive and its original `.sha256` sidecar are both retained.
The separate publisher preserves the accepted original bytes, including the
complete indirect producer records, without sanitizing, rebuilding or resigning.
Its presence does not authorize a CI run or publication of the unbound draft.
