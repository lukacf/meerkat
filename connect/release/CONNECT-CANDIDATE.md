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

This manual-only workflow is source preparation for the Core/Meta 0.9.4 installed
selection. It does not publish, tag, install on a user host, dispatch another
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
2. Fetch the exact selected Core/Meta 0.9.4 commits and check any explicitly
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
   the selected Meta commit's actual `parse_candidate_selection` and
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
release, and it retains the assembler's existing exclusions.

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

## Required inputs still unbound

`connect-release-inputs.json` selects Linux; `connect-release-inputs-macos-arm64.json`
selects macOS. Both contain deliberate nulls and refuse before any native build.
The existing owners must supply all of the following separately for each target:

- The final dispatched public Connect source commit, its native package version,
  SDK distribution/version, and exact native Cargo manifest/lock hashes.
- The exact Core and Meta source commits whose native packages are `0.9.4`.
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
  declare the matching `meerkat-meta==0.9.4` native-host dependency.
- A complete offline SDK build-tool requirements file and wheel batch, including
  the build frontend and the selected source backend requirements. No versions
  are inferred. `--no-isolation` uses only that fresh build environment; the
  existing source-owned backend remains responsible for creating the wheel.
- The exact hashes of the four Forge reader source files in the selected
  Meta commit checkout. The historical r5 Meta source has no installer reader;
  its availability is not implied by a version string. Missing or differing
  reader source refuses before resource/native production.
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
credentials; its verifier and Meta-reader child processes receive a scrubbed
home/configuration and no inherited credential environment.

```sh
python3 scripts/connect-candidate.py accept-publication \
  --request /path/to/selected-publication-request.json \
  --candidate /path/to/candidate-files \
  --meta-source /path/to/selected-meta-checkout \
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
version. The Meta checkout's HEAD must be the exact selected commit, with a
clean original `installer/launchkit` package and matching reader source pins.
If the selector includes a tag, its explicit `refs/tags/<tag>` must also resolve
to that commit. Commit-only selection requires no tag. Neither an SDK wheel
nor a separate reader wheel substitutes for that checkout. Both paths retain
the same `0.9.4` native package and Meta SDK version requirements, exact SDK
source comparison, graph checks and actual owner-reader acceptance.

Acceptance performs bounded data-only extraction, preserves original bytes,
and checks the complete retained inventory. The selected Meta package supplies
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

`--accepted` selects the prior `acceptance.json` and `assets/` directory.
`--acceptance-sha256` is its independently selected exact lowercase SHA-256;
`--output` must not exist. The writer parses the same bytes it hashes, retains
and rechecks the three allowlisted assets, and repeats attestation verification
before any upload. Only those original three assets can be added. It imports
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
tag, original selection and layout, and public wrapper-member URLs with exact
archive/member sizes and hashes. This is the final public URL inventory; an
Actions artifact alone is not that handoff.

## Scope and remaining work

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
not establish actual signature verification, selected Meta-reader acceptance,
public upload or Forge inclusion. No Cargo, packager, native verifier, native
binary, GitHub job or remote host was executed for this draft. Actual attested
candidate inputs and the exact selected Meta package must still pass acceptance;
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
