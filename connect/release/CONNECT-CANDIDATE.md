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
traversal, and what the engine does not enter becomes an extra artifact of the
same engine run: a container nested deeper than four levels, a zstd container
(decompressed), a member that is a bare zlib stream (inflated), and gzip, bzip2
or xz streams that hold a tar past the 1 GiB bound, such as a large product
archive (cut at member boundaries into pieces below the bound, keeping every
original byte; the header fields of each gzip stream are scanned too). The scan
therefore reaches a product archive, the integration library inside it, the
wheels, crates and source archives inside the library, and what they hold. Its
limit is the set of forms it recognises: a member in any other encoding (a
deflate stream without a zlib header, an encrypted or proprietary form) is read
as the raw bytes it is.

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
  `scripts/privacy-rules-from-names.py` writes both from a private name list
  (`CLASS<TAB>TOKEN` lines): one category per class, one rule per token, and a
  built-in category for home directory paths. A token matches case-insensitively
  at word boundaries; a token of 6 or more characters in the classes `product`
  and `private_link` matches inside other words too, so plural, compound and
  camel-case forms are hits. The rules file names the sha256 of its name list.
- A category whose name starts with `review_` is a review category: its hits
  are counted in `report.json` (`review_hits`) and never refuse the payload.
  The converter makes one from each class named `review-...` and from each
  class passed with `--review-class`. A release uses neither: its rules are
  written with `--paths review` and no `--review-class`, so the path category
  is the only review category, and no upload receipt is written from a scan
  whose rules hold a review rule outside it.
- Scan every file that will be attested or uploaded: both product archives with
  their `.sha256` files, the evidence and every other release asset. The
  embedded integration library is reached through the product archives.
- A release lane that assembles products with `assemble-connect-product.py`
  directly runs this command itself on the assembled archives, and uploads
  only files whose sha256 a `report.json` with status `PASS` names in `payload`.
- The scan fails closed. A hit outside the review categories refuses, and so
  does any error the engine reports (a container it cannot decode, a damaged or
  unsupported archive, a member over its size bound, an unreadable input),
  unless an accepted finding or a coverage exception below covers it exactly.
  A member over the size bound and an unreadable input are never accepted.
- A member the scan cannot read passes through an entry of
  `release/payload-scan-coverage-exceptions.json`, a committed list, or through
  an accepted finding of the category `structure` (below). Each entry of the
  list names the member's path inside the payload, the sha256 of the container
  the error was reported in, the error kind, a one-line reason and who accepted
  it. `report.json` counts the entries used on their own (`coverage_exceptions`).
- `--accepted-findings` is optional. Its entries (schema
  `release-payload-privacy-scan/accepted/2`) are findings the release owner
  accepts in exact bytes, such as a third-party file whose text happens to match
  a rule. Each entry names a category, a rule id, a sha256, a one-line reason
  and who accepted it (`accepted_by`); `class` and one `proof` object are
  optional. The sha256 is that of the innermost member that holds the finding,
  the first digest `refused.json` shows for it; a container around that member
  does not do. Only an entry of class `PUBLIC-UPSTREAM-ARCHIVE` may name the
  sha256 of an archive around the finding instead, never of a payload file as a
  whole. A structural error is accepted with the category `structure`, the
  engine's error kind as the rule, and the sha256 of the container it was
  reported in. The same finding in any other bytes refuses, and without the
  file nothing is accepted. The file stays private, beside the rules.
- `--non-published-list` is the library packager's private list of members a
  library does not publish. A payload member, at any depth the scan opens, that
  holds one of its digests is a refused hit (`withheld-member-reference`) that
  cannot be accepted. A release scan refuses without the list. This check
  compares digests, not paths: it reads a listed digest as its 64 hexadecimal
  characters, in either case, and a member that names such a member only by
  its path, or states a digest in another written form, is not a hit.
- `report.json` (schema `release-payload-privacy-scan/report/3`) holds no
  matched text and no member name. Its `status` is `PASS` or `REFUSED`, with the
  reasons by name. It says when it was written, the git commit and sha256 of
  the tool, the sha256 and the number of rules (all, those that can
  refuse, and the review rules outside the path category), the sha256 of the name list the rules declare, the sha256 and number
  of exemptions, accepted findings and coverage exceptions, the sha256 of the
  non-published list, and each payload file's name, size and sha256 as the walk
  read them; the counts of hits and errors, accepted and refused, follow. A
  report cannot be `PASS` when the rules can refuse nothing (no rule outside the
  review categories, no home path category, an exemption in a category that can
  refuse) or when a payload file changed during the scan. A release scan
  refuses unless the tool runs from a git checkout whose commit holds exactly
  the tool and its engine; `--proof-only` marks a run that is not a release
  scan (`release` false, a label that starts with `PROOF-ONLY-`).
  `detail.json` holds the matched strings. It is private (mode 0600) and is
  never uploaded. `refused.json` (mode 0600) lists what was refused without the
  matched text: each hit's category, rule ids, count, path inside the payload
  and the sha256 of the file that holds it and of each enclosing container;
  each error's kind, path and container sha256. A path that matches a rule,
  as it is or in a decoded form, is withheld. The release owner reads the
  listing to decide what to accept. It names members, so it stays private too.
- CI runs only the tests (`scripts/test_release_payload_privacy_scan.py` and
  `scripts/test_privacy_rules_from_names.py`), with synthetic rules and names:
  a made-up marker in `scripts/fixtures/release-payload-privacy-scan/`. CI does
  not scan release bytes.

## Scan the release payload for credential patterns

The same files are scanned for credential patterns before they are attested
or uploaded. `scripts/release-payload-credential-scan.py` opens every payload
file and every archive nested in it, at every level
(`scripts/release-payload-deep-extract.py`), and lets two builds of the
gitleaks detector read the result: a pinned older build and a current one,
each with its own default rules. The scan fails closed: a finding of either
build refuses unless an acceptance binds it to its exact bytes.

```sh
python3 scripts/connect-candidate.py credential-scan \
  --gitleaks-pinned /absolute/detector-pinned/gitleaks \
  --gitleaks-current /absolute/detector-current/gitleaks \
  --accepted-findings /absolute/private/credential-accepted.json \
  --pending /absolute/private/credential-pending.json \
  --output /absolute/new-credential-scan \
  /absolute/staged/<every file to attest or upload>
```

- Both builds are given by absolute path and must be two different files.
  Each build's default rules file is `gitleaks.toml` beside it (the file
  `config/gitleaks.toml` of that version's source). The scan removes the global
  path allowlist lines that mention `vendor`, `dist-info` or `lock` from each,
  and stops unless a run with the build's embedded default gives the same
  result as a run with the file, over every directory a build reads of every
  payload file (the scan root, the bytes outside archive members, the runs of
  compiled files, the runs read alone): a rules file that lacks a rule gives
  the default's result wherever that rule finds nothing. A build's embedded
  default passes over the paths of the removed lines, so the files the scan
  reads only because it removed them are given to the build once more for
  this comparison, from a directory of their own under numbered names. The
  scan stops when one of the removed lines matches a numbered file of its
  own or its work directory: choose a work directory that no such line names.
- The builds always run redacted and with allow comments ignored, from an
  empty working directory, with the payload one level below the scan root. No
  comment, ignore file or rules file inside a payload switches a finding off.
- Compiled files are covered by the run rule. For each ELF file the scan takes
  every run of eight or more printable ASCII bytes. Each run a finding touches
  is read again as a unit of its own, and only those findings count, so no rule
  can join the end of one run to the start of the next. Such a finding is
  reported at `PATH#strings`, and its line is the byte offset of the run. The
  file of a compiled file's runs begins with its first run, and a run that is
  read again is its own file from the first byte. Only where such a file would
  start like a binary signature, which no build reads, something stands
  before: a line of plain words before the runs, a line break before a run
  that is read again.
- No list of the scan decides whether a build read a file. Each build reads
  every file it is handed twice (a member that is text, text among the bytes
  outside members, the file of a compiled file's runs, a run that is read
  again). The first reading is of the file as it is, and keeps what every rule
  means, at the first byte and at the end of the text. In the second the file
  ends with a line of the scan's own: a made-up name and value that the
  build's rules report. The line stands on the scan's copies only and is cut
  off again afterwards; no digest and no line number sees it. A finding of
  both readings, the same by file, line and rule, counts once. A build has
  read a file when it reports that line there exactly as it reports the line
  alone, which the scan measures once per build: the same rules, as often, at
  that line. The proof of the second reading speaks for the first: a build
  takes or passes over a file for its path and its leading bytes, and those
  are the same in both readings. The one exception is a size limit that the
  added line alone crosses; then there is no proof, which is the safe side.
  Where a build decides by more than its path and leading bytes, what the file
  holds is still reported by the second reading; only a report that needs the
  end of the text can then be missing. A
  file without proof is read again behind a line of plain words, twice in the
  same way, and its findings count at their own lines. Without the line there
  too the file is unread by that build and refuses like every unread file (`no
  proof that a build read it`). The line proves that the build reached the
  file's last line, not that every rule ran on every line. A build that does
  not report the line alone stops the scan. The receipt and the report count,
  per build, the files handed, read as they are, read behind the line and
  unread, and the readings of both kinds. For an upload, the number of handed
  files that a build left unread for lack of proof of read (`read_proof`,
  `unread`, counted per build in the report) must be 0 for both builds: the
  upload receipt writer writes no receipt otherwise, whatever the acceptance
  file says, and none from a report without those counts. The scan itself
  passes when an acceptance of a file's exact bytes covers such a file, and
  shows it (`unread` above `unread_unaccepted`); an acceptance of a position
  never covers one. The other kinds of unread file keep their rule:
  `unread_unaccepted` is 0.
- An archive can hold more than its members, and its reader does not say so.
  Every byte of an opened archive that is not the content of a member the scan
  wrote out is scanned too, at every level, at paths below `ARCHIVE!!outside/`
  (`!outside/` for the payload file itself). `before-start` is what stands
  before the first record of a zip. `after-end` is what follows the end of a
  zip (its end record and comment) or of a tar (the block where its reader
  stops). Both are read as files, without their leading zero bytes, and an
  archive that starts there is opened. `headers-and-gaps` holds, one on a
  line, the runs of eight or more printable ASCII bytes and tabs of every
  other such byte: headers, names, comments, extra fields, padding, gaps
  between members, a member that was not written out, the header and trailer
  of a gzip member, a zstd skippable frame. A tar inside a compressed file has
  its own `tar-after-end` and `tar-headers-and-gaps`. Zero bytes alone make no
  file. Both builds read these files whatever the path of their archive is:
  not below that path, where a path allowlist could skip them, but under
  numbered names in a directory of their own, and a file of them that starts
  with a binary signature by its printable runs, like a compiled file
  (`ARCHIVE!!outside/NAME#strings`). None of them is ever unread. The scan
  stops with a usage error when a build's path allowlist matches one of the
  numbered files it writes for the builds (these, the runs of a compiled file,
  a run that is read alone). An
  `archives` entry for the exact archive covers these paths; a `positions` or
  `files` entry names them like any path.
- What counts as a member's content is what was decoded, checked by the scan.
  A zip is not opened (which refuses) when a stored member's data is longer
  than its size, when a deflated member's stream does not end with its
  compressed size or does not decode to its size, when a directory entry names
  a place where no record stands or a record of another name, when its zip64
  end records are not where they must be, or when its reader warns about it
  (two entries for one record). The data of a zip entry with the mode of a
  link (the link's target) or of a special file is read as text, as one of
  the files of the bytes outside members (`ARCHIVE!!outside/link-00000`,
  `special-00000`): no link is made, no name of the entry is used, and nothing
  is followed. A directory entry that holds data is not written out
  (`directory-with-data`). A tar member that is no file, no link and no
  directory and has a size is read like a file when the reader gives its data
  (a type it does not know); when it gives none (a pipe, a device) the member
  is not written out (`special-with-data`). A compressed file (gzip, bzip2, xz,
  zstd) is its streams and zero padding: any other byte after its last stream
  makes it a file that is not opened.
- A printed path names one file. A member whose name holds `!`, at any level,
  and a member file whose name ends with `#strings` would print like another
  path (a member of an archive, the runs of a compiled file). Such a member is
  not written out and refuses as every member that is not written out does
  (`separator-in-name`, `strings-mark-in-name`); only an `archives` entry for
  the exact enclosing archive covers it.
- A finding belongs to the member whose file a build names in its report, and
  a report need not give a name back as the disk has it: a member's name that
  is no UTF-8 (a tar member can carry any bytes) comes back with other
  characters in the place of those bytes. Before a build reads the members the
  scan notes each such member: it is known by its path without everything
  that is not printable ASCII, and that form must be its own. When another
  member has the same form, a report could mean either, and the scan stops
  before a build runs. A finding at a path that is no member's stops the scan
  too: no position is made at a path the scan did not hand to a build, and no
  finding is dropped. The stop is wider than the names a report would
  confuse: a UTF-8 name with a letter outside ASCII where the other name has
  such a byte, or the plain name without the byte, stops the scan as well.
  The scan takes it that a build gives back every printable ASCII character
  of a path and adds none; a build that does otherwise never makes such a
  member pass, but may show a refused position at a sibling. The receipt is a
  text that a person reads: a path stands there in a form of its own and on
  one line, with the escape sign written twice and a byte that is no UTF-8,
  the column sign and every control character as `\xNN`. The listing and the
  report hold the path itself.
- `--accepted-findings` is optional and private; without it nothing is
  accepted. Its schema is `release-payload-credential-scan/accepted/1`. A
  `positions` entry accepts one position: `rule`, `path` (the path inside the
  payload, `!` at archive boundaries), `sha256` of the member file that holds
  the finding, `line` (or the run's byte offset for a `PATH#strings` position),
  `class`, `reason` and `accepted_by`; `count` gives the number of findings at
  that position when it is not 1. An `archives` entry (`path`, `sha256`,
  `index`, class `PUBLIC-UPSTREAM-ARCHIVE`) accepts everything inside one exact
  nested archive that was proved byte-equal to its public upstream archive. A
  `files` entry (`path`, `sha256`, class `PUBLIC-UPSTREAM-FILE`) does the same
  for one exact file, and an `unopened` entry (`path`, `sha256`) for one
  archive the scan cannot open. Every entry needs a `reason` and an
  `accepted_by`: the tool has no approver of its own, and an entry without one
  is a usage error. An entry may carry a `proof` object. The same finding in
  other bytes, on another line or at another path refuses.
- `--pending` names positions that wait for their owner (schema
  `release-payload-credential-scan/pending/1`: `rule`, `path`, `line`,
  `sha256`). The list never accepts: a position on it refuses even when an
  acceptance entry matches, and the receipt counts it as `pending_owner`.
- A record field that holds a random identifier is new in every build, so no
  acceptance can be bound to its bytes. The committed
  `release/payload-credential-identifier-classes.json` (schema
  `release-payload-credential-scan/identifier-classes/1`) declares such
  fields. Each entry of `classes` names the detector `rule`, the payload file
  name (`asset`) and the path inside it (`member`), the JSON `key_path` (keys
  joined by dots, a list step written `[]` after its key: `a.b[].c`), the
  `value_form` (`uuid-v4`) and `accepted_by`. In `asset` and `member`,
  `<version>` stands for three numbers joined by dots (no leading zero) and
  `<target>` for a target name of two parts with one hyphen between them (as
  `linux-x86_64`: a payload file's name holds its target's name); every other
  character is itself, and a pattern needs a letter or a digit of its own
  beside its places. A place cannot tell a version or a target from other
  text of that form, so an entry can also name them literally. The scan
  resolves a finding by an entry only when it proves all of this on the
  member's bytes: the member is one JSON document without a repeated key; the
  finding's line holds that one field and its text value and nothing else;
  the value is the value at exactly the declared key path; the whole value is
  a canonical lower-case version 4 UUID; every span the builds report for the
  position lies on that line; rule, payload file name and member path are the
  entry's; and every finding of the position lies in the one file that is
  read. Anything else refuses as before, and each finding stands on its own
  proof. A pending position refuses whatever a class proves, and an
  acceptance bound to bytes counts before a class. A pending entry is bound
  to the member's bytes, and such a member is new in every build: the entry
  holds for the build it was written for, so the pending row of a field is
  removed in the step that enables its class entry. A list that is not of its
  schema, or holds a key twice, is a usage error. An absent list, or a list
  without an entry, changes no output. With an entry in the list the
  receipt's first line ends with `class_tool=<name> class_resolved=<n>`, its
  rule table has one line for the findings a class resolved (it names no
  rule), and the report holds the same number as `findings.class_resolved`
  (so that `findings.total` is the sum of the accepted, the refused and that
  number) and in `identifier_classes` (`tool`, `resolved`, `list_sha256`);
  none of them names a rule, path or line of a resolved finding. The list is
  read from the tool's own checkout. When the tool, its extractor or the list
  is not, byte for byte, what that commit holds, `tool_commit` is null; and
  the receipt writer gives no receipt unless `list_sha256` is the list of the
  report's commit.
- Unread content refuses too, and the rule is per build. A file may be unread
  by a detector build only when it is byte-equal to a pinned upstream member.
  A file that starts with a binary signature and is neither an opened archive
  nor an ELF file, a file that a build's remaining path allowlist skips (both
  builds, or one build while the other reads it), an archive that cannot be
  opened, and an archive member that was not written out (encrypted,
  unreadable or with an unsafe path) each need a `files`, `unopened` or
  enclosing `archives` entry for their exact bytes.
- The output directory holds `receipt.txt`, `report.json` and
  `unaccepted.json`. The first line of the receipt is
  `CREDENTIAL-LEG PASS|REFUSED <name> files=<n> <file>=<sha256> ...` with the
  sha256 of the acceptance file and of the pending list and the counts; it says
  PASS only when `refused`, `pending_owner` and `unread_unaccepted` are all 0.
  `report.json` (schema `release-payload-credential-scan/report/1`) is what the
  upload receipt is written from: `status` (`PASS` or `REFUSED`), `generated_at`,
  `tool_commit` (the commit of the checkout the tool ran from, null when the
  tool's files are not that commit's), each payload file's `name`, `bytes` and
  the `sha256` of the bytes that were scanned, the two `detector_builds` with
  version and sha256, and `accepted` and `pending` with count and sha256.
  `unaccepted.json` (mode 0600) names every refused position and unread file
  with its sha256, which is what an acceptance entry needs. None of the three
  holds a matched value or any line content; the builds' redacted reports stay
  in a private work directory that is removed at the end.
- A release upload needs both scans for the same files: a private-text report
  and a credential report that each say PASS and name every file to upload.
  The next section is the receipt written from them and the check that
  enforces it. Both full reports stay where the scans ran.
- CI runs only the tests (`scripts/test_release_payload_credential_scan.py`)
  against a stand-in detector the tests write themselves, with made-up
  markers. CI does not scan release bytes and does not need the detector.

## Check the receipts before an upload

No file is uploaded, attested as a release asset or added to a release unless
`scripts/release-upload-receipt-check.py` passes for the exact files. It reads
one upload receipt, which is written where the scans ran, from the two full
scan reports:

```sh
python3 scripts/release-upload-receipt-write.py \
  --privacy-report /absolute/privacy-scan/report.json \
  --credential-report /absolute/credential-scan/report.json \
  --version <N.N.N, the release's version> \
  --target <linux-x86_64 or macos-arm64> \
  --receipt /absolute/new/upload-receipt.json \
  [--target release --receipt /absolute/new/upload-receipt-release.json]

python3 scripts/release-upload-receipt-check.py \
  --target <linux-x86_64, macos-arm64 or release> \
  --receipt /absolute/upload-receipt.json \
  --release-commit <full id of the commit being released> \
  --version <N.N.N, the release's version> \
  /absolute/staged/<every file about to be uploaded>
```

- The full reports stay where the scans ran. They hold category names and
  counts of rules, findings and accepted entries. The receipt (schema
  `release-upload-receipt/4`) holds exactly what the check reads: the target,
  each file's name, size and sha256, and for each scan the status word,
  `release`, the time, the tool's commit and sha256, the digests of the inputs
  the scan ran with, the sha256 of the full report, and seven facts as true or
  false (blocking rules present, every rule outside the path category
  blocking, no exemption, every used acceptance entry with a proof tool,
  nothing refused, nothing pending, nothing unread and unaccepted). It names
  the tools whose proofs the used acceptance entries carry, each with the
  number of entries it proved, so that whoever reads a receipt before an
  upload sees which tools vouched and for how much. It holds no category name
  and no other count.
- A finding is accepted by a tool's proof, not by hand. An entry of the
  accepted-findings file is proved when its `proof` is an object whose `tool`
  is the name of the tool that proved it; the rest of the object is that
  tool's own. A tool's name has one form: a lower-case letter or a digit, then
  lower-case letters, digits and single hyphens, 64 characters at most. The
  names reach the receipt as the acceptance file writes them, so any other
  text names no tool: the scan counts such an entry as not proved, and the
  writer and the check refuse a receipt that holds such a name. The scan's
  report counts the entries it used (to accept a hit or a structural error)
  that are not proved, and the used entries by tool. No receipt is written
  unless every used entry is proved. An entry that accepted nothing counts
  nowhere.
- A receipt is written only from two passing release reports. A report that
  says anything but `PASS`, a proof-only report, one with a refused or pending
  finding or an unread unaccepted file, one without a blocking rule, one
  with a review rule outside the path category, one of a scan that ran with
  an exemption and one that used an acceptance entry without a proof tool give
  no receipt. Both scans must have read every file: a file that only one
  report names, or that the two name with other bytes, gives no receipt at
  all. A caller that holds the acceptance inputs can bind them at this step
  (`--privacy-accepted-file`, `--credential-accepted-file`,
  `--credential-pending-file`, or the `-sha256` forms).
- A release's staging directory is scanned once, when every file is staged:
  the release manifest and the checksum list exist only after the other
  files. One writer call then gives the receipt of every target from that one
  pair of reports, with `--target` and `--receipt` in pairs: one receipt for
  the platform target and one for `release`. Each holds exactly the rows whose
  file names are its target's by the committed name rules, which the writer
  reads from its checkout's commit as the check does. The call writes all its
  receipts or none: a staged file that is a name of none of the targets
  given, and a target that gets no file, give no receipt at all.
- The check takes the receipt and never a full report. A receipt with one
  field more or less than its schema refuses, so a full report given in its
  place refuses, and nothing of a full report can ride along.
- The check passes only when both scans say exactly `PASS`, were release runs,
  are no older than 24 hours (`--max-age-hours` can make the bound shorter,
  never longer), and were run by the tools of the release commit
  (`tool_commit`).
- The files given to the check must be exactly the receipt's rows: as many
  files as rows, each file named in the receipt with the same name, the same
  size in bytes and the same sha256 as it has at the time of the check, and no
  row without its file. A caller cannot check a part of a receipt. A file that
  changed after a scan refuses. Every file name is a name of the checked
  target and of no other target (the name rules below).
- The private-text scan must have had a blocking rule, no review rule outside
  the path category, no exemption (the exemptions file is written beside the
  rules and no pin holds it, so a release scan runs with the converter's empty
  one), the pinned rules file,
  the pinned name list, the pinned list of left-out members and the coverage
  exceptions file of the checkout (`coverage_exceptions_sha256` equals the
  committed `release/payload-scan-coverage-exceptions.json`). The credential
  scan must have run with the two pinned detector builds, each reading with
  its pinned default rules file, and left nothing refused, pending or unread
  and unaccepted. A build's digest does not say which rules file the scan
  gave it, and a file that lacks a rule finds less: the receipt states the
  sha256 of the rules file each build read with, and the check holds it to
  a pin, byte for byte.

Where the pins come from:

- The check reads them from the checkout it runs from, as that checkout's
  commit holds them: `release/upload-receipt-pins.json` (schema
  `release-upload-receipt-check/pins/3`). Each target has a name rule and seven
  pins: `name_list_sha256`, `rules_sha256`, `non_published_list_sha256`,
  `detector_pinned_sha256`, `detector_current_sha256`,
  `detector_pinned_default_rules_sha256` and
  `detector_current_default_rules_sha256`. They are digests
  only. A name list, its rules and a list that names left-out members are
  private and enter no repository, workflow input, artifact or log. The two
  default rules pins are the sha256 of the file `gitleaks.toml` beside each
  pinned build (the file `config/gitleaks.toml` of that version's source).
- When a detector pin or a build's default rules file changes, two counts are
  measured again, per default rules file, before the new pin is accepted: the
  rules that can report only at the first byte of a file (an alternative
  anchored at the start of the text whose other alternatives take no line
  break), and the rules with an end-of-text alternative after the value whose
  other alternatives take no line break. The first says how far the stated
  first-byte limit reaches. The second is why every file is read twice, as it
  is and with the scan's line after it. The counting is done by hand: no
  command of the tree measures the two counts, and the gate does not depend on
  them, because each build reads every handed file twice whatever they are.
- The name rule (`names`) says which file names are a target's own. A platform
  target (`linux-x86_64`, `macos-arm64`) has `holds-target`: the file name
  holds the target's name. Both product archives, their `.sha256` files and
  the two evidence files of a platform
  (`<product>-<version>-<target>.evidence.json` and `.evidence.tar.gz`) are
  that platform's files. The target `release` has a list of exact names for
  the files of a release that hold no platform's name:
  `RELEASE-MANIFEST.json`, `SHA256SUMS`, `source-manifest.json`,
  `meerkat_toolkit-<version>-py3-none-any.whl` and `rkat-toolkit-<version>.tgz`.
  `<version>` is the release's version.
- The file names of every target are held to the release's version, which the
  caller gives to the writer and to the check with `--version` in the one form
  `N.N.N`; without it nothing passes. An exact name of `release` is its
  pattern for that version. A platform's file name may hold a version (a
  dotted number of three or more parts) and then holds exactly the release's;
  a name that holds none passes, as the candidate's own three files do. So a
  receipt with a file whose name holds the version passes for no other
  release, also not for another tag of the same commit; files whose names
  hold none are not bound to one release in that way (see the limits below).
  The upload tool gives the tag's version, the candidate tool the request's.
- No file name passes under two targets. A name that holds two platforms'
  names, or that holds a platform's name and is an exact name of `release`,
  passes under neither. The pins file itself refuses, for every target, when
  an exact name holds a platform target's name or is stated twice.
- A release upload therefore has one receipt per target: one for the platform
  target and one for `release`, each with exactly its own rows.
- No option, environment variable, workflow input or secret gives a pin. The
  check refuses when it does not run from a git checkout, when the tool, the
  pins file or the coverage exceptions file is not byte for byte what the
  commit holds, and when a pin of the target is `null` or missing.
- The rules file is pinned beside the name list because a report's
  `name_list_sha256` is only what the rules file says about itself. The rules
  written by `scripts/privacy-rules-from-names.py` are one exact file for one
  list and one set of options, so the digest can be stated for a given list
  only together with the options. A release's rules are written by one fixed
  call: `--paths review` and no `--review-class`. A class that is a review
  class refuses nothing, so the writer gives no receipt unless every rule
  outside the path category is blocking, and the check requires that fact
  (`name_rules_all_blocking`). The path category of `--paths review` is the
  one review category a release's rules may hold.
- A target that leaves nothing out is built and scanned with the empty list
  `release/library-non-published-source-members-empty.json`. It names nothing,
  so it is committed and a hosted job may hold it; its digest is an ordinary
  pin. A list that names a member is private.
- The pins of a release are set by a commit before the scans run, and before
  the release commit: a pins commit sets all seven pins of each target the
  release uploads, from the lane's inputs (the name list, the rules written
  from it by the tree's converter, the list of left-out members written by the
  tree's packager, the two detector builds and the default rules file beside
  each). The release commit inherits them. At the release's version commit the
  lane derives the rules and the list of left-out members again from that
  commit's tree and compares all seven values with the committed pins; the
  release goes on only when every one is equal. They can differ only when the
  release branch changed the converter, the packager, a left-out source member
  or an input pin of the library, or when the library input set was built
  again; then a second pins commit follows, after the release commit and
  before the lane builds what it uploads. Until a target's pins are set they
  are `null` and every upload of that target refuses. The pass line names the
  target, the pins file's sha256 and the commit the pins were read from.
- A target whose pins are `null` cannot be uploaded. The pins of
  `macos-arm64` are `null` until the change that binds that target's uploads
  to this check sets them. The pins of `linux-x86_64` and of `release` are
  set.
- Publication is judged by the pins of a commit on the default branch. In the
  hosted workflow the accept and publish jobs refuse, in the step right after
  their checkout, unless the checked-out commit is an ancestor of the default
  branch's tip; a branch that was not merged, a failed fetch and an unknown
  branch name all refuse. Candidates are still built on any branch.

Where the check runs:

- A party that cannot hold the private inputs never makes a receipt. The
  scans run where the private inputs are, and the receipt is written there; a
  hosted job only consumes the receipt for the exact bytes, or builds files
  that are scanned before any publish step.
- `accept-publication` takes `--upload-receipt`. It refuses unless the check
  passes for its retained copies of the candidate's files, with the
  candidate's `source_sha` as the release commit and the request's target, and
  keeps the receipt in the acceptance record (`upload-receipt.json`). The
  record holds no full report.
- `publish-accepted` runs the check again, before its first API call, on the
  bytes it is about to write, with the receipt of the acceptance record, and
  records the check's pass line in `publication.json` (`scan_receipt`).
  Without a passing receipt nothing is written.
- The hosted workflow gives accept the receipt from a dispatch input and gives
  neither command a pin: both run the check of the checkout the workflow was
  dispatched at. No full report is an input of the workflow or part of an
  artifact it uploads. Its build and attest jobs upload workflow artifacts
  that are not bound to a receipt: they are what the scans read, and the
  scans need private inputs that no hosted runner holds.
- A lane's files are attached to a release by
  `scripts/release-upload-attach.py` and by nothing else (the section "Attach
  lane files to a release" below). It runs the check for exactly the staged
  files, one receipt per target, and uploads its own copies of them. An
  upload, recovery or republish script that attaches a file in any other way
  is not to be run.
- CI runs the two tools' tests (`scripts/test_release_upload_receipt_check.py`)
  with synthetic reports and receipts.

Limits and consequences, as they stand:

- The acceptance record (`acceptance.json`) has no field for the receipt. The
  receipt lies beside it (`upload-receipt.json`) and is not part of the digest
  the owner selects, so it can be exchanged for another receipt. That changes
  nothing that is uploaded: the check binds the receipt's rows to the bytes
  about to be written, so another receipt passes only when it names the same
  files with the same bytes.
- The publish path has a name check of its own for the delivered evidence
  (`CONNECT_PUBLICATION_PRIVATE_NAMES`). It refuses only what the publishing
  environment supplies, and nothing when the variable is not set. It is not
  the gate. The gate for private text in a published file is the receipt.
- A maintenance release, cut from a release branch, is accepted and published
  from the default branch like any other: both jobs run on the default branch,
  and the default branch's pins judge. The request still names the release
  branch's commit as the source commit.
- The 24 hours run from the scans, not from the acceptance. Accept and publish
  must both happen within 24 hours of the older scan. A publish after that
  needs new scans, a new receipt and a new accept. The upload tool passes the
  check within those 24 hours as well, but its uploads and read-backs come
  after the check and may take up to an hour each, so the last file of a
  release can be attached later than 24 hours after the scans. What goes up
  are the copies the check passed, so nothing else changes by that.
- `detector_pinned_sha256` is the digest of one build of the pinned older
  detector version, made on one host. Another host meets that pin only with
  that same file, not with a build of its own.
- The hosted publish path cannot run today: the committed release inputs
  select no reader source, and accept requires one. A dispatch runs the
  workflow file of the ref it names, so the step that requires a commit of the
  default branch guards only refs whose workflow holds it. Before the hosted
  path is made runnable it needs a control on the repository's side for older
  refs. None is built here.
- The upload tool asks the release repository for the default branch's tip
  with a git call that reads no configuration. A proxy that is set only in a
  git configuration is therefore not used by that call: a lane host that
  reaches the repository only through such a proxy refuses. The tool's fetch
  needs git 2.29 or later; an older git refuses.
- The upload tool, the writer, the check and the scans run the `gh`, the `git`
  and the Python interpreter they find on `PATH`, and trust them. In the same
  way they trust the environment they are started in: its proxy and
  certificate variables shape the upload tool's question to the release
  repository, and its temporary directory is where that question runs.
- The object store of the lane checkout is trusted by the upload tool, the
  writer and the check: whoever can write to it can forge the history, the
  files and the pins that a commit's id stands for, and the cut's preflight
  runs `git fsck` on the lane checkout as the independent check.
- The upload tool attaches no empty file: a staged file of zero bytes refuses.
- The accepted-findings files are private and are not anchored in the tree: no
  pin holds them. They are read at the cut, where the scans run. The gate
  checks that every acceptance entry a scan used names the tool that proved
  it; it does not check that a proof is true. The proofs are the lane tools'
  own. An entry of the class `PUBLIC-UPSTREAM-ARCHIVE` covers every hit inside
  the archive it names, with no bound on their number. Verifying a
  public-source or public-upstream proof inside the scan is a follow-up that
  is not built.
- The digest of the rules file is vouched for by whoever sets the pins. A
  rules file that is not the converter's output for the fixed call passes when
  its digest is the pinned one. The converter call is fixed in the cut's
  recipe. A scan that derives its rules from the name list itself is not
  built.
- The check trusts the receipt for the scan tools' own bytes. The writer
  compares each report's tool digest with the scan's file as the commit the
  report names holds it; the check holds only the receipt, and its checkout
  need not hold the release commit.
- One release is one run of the upload tool, with all its receipts.
  `RELEASE-MANIFEST.json` and `SHA256SUMS` list other files and are never
  replaced, so they are written once, for every platform of the release, and
  every platform's files go up in that one run. A later run whose receipts do
  not name a file of an earlier run refuses on that file before it uploads
  anything; it completes only when it brings every earlier file again,
  unchanged, with receipts the check still passes.
- Only a file name that holds the version binds a file to one release. A set
  of files whose names hold none (the candidate's three files, or the names
  of `release` without a version alone) passes under every tag of its commit.
- The upload tool never removes an asset. What an interrupted upload leaves
  under a row's name is removed by an owner of the release, by hand; until
  then every rerun refuses.
- An asset named like one of the candidate's files is tolerated by the upload
  tool without being read.
- The upload tool reads each asset back through `gh`, as the signed-in
  account, not as the public receives it. The cut's own anonymous read-back
  step stays the independent check; a public read-back inside the tool is not
  built.
- The credential scan gives the builds the bytes of an archive that are no
  member's content in two forms. What stands before a zip's first record or
  after the end of a zip or a tar is read as a file. Every other such byte
  (headers, names, comments, extra fields, padding, gaps between members, a
  member that is not written out, the header and trailer of a gzip member, a
  zstd skippable frame) reaches the builds only as its runs of eight or more
  printable ASCII bytes and tabs, as a compiled file does. Text in another
  encoding there, a value that another byte splits, and data there that is
  itself compressed are not found. A value that stands across the end of a
  header and the start of a member's data is in no run; a value longer than
  the 4096-byte overlap of the 1 MiB pieces may be whole in no piece. A
  record of a zip that no directory entry names is read as runs, also when a
  whole archive stands there. An
  archive in the file before or after a zip or a tar is opened only when it
  starts that file; behind any other byte it is read as the bytes it is, as in
  any member.
- A rule that reports only at the first byte of a file (a pattern anchored at
  the start of the file) reports a run only where the run stands there: the
  first run of a compiled file, and a run that is read again on its own. It
  does not report the first run when the file of the runs would start like a
  binary signature, nor a run read again that starts like one: something
  stands before those, because no build reads a file that starts so. The same
  holds for bytes outside members that start like a binary signature, which
  are read by their runs. It reports no run in `headers-and-gaps` or
  `tar-headers-and-gaps`, whose first line is the scan's own. The scan's list
  of signatures is a first guess only: a file of a kind that a build passes
  over by a check of its own is found by the proof of read and read again
  behind a line of plain words, and the same limit then holds for it.

The check verifies that both reports are complete, current and bound to the
exact files and the pinned inputs. It does not prove who produced a report.

## Attach lane files to a release

Every file attached to a Connect release is a row of an upload receipt that
the check passed. A release gets files in two ways only: `publish-accepted`
adds its three candidate files, and `scripts/release-upload-attach.py` adds
the files a lane staged. The upload tool is the one attach path for lane
files; no other script or workflow step uploads to a Connect release.

```sh
python3 -I scripts/release-upload-attach.py \
  --staging /absolute/staging \
  --tag connect-vX.Y.Z \
  --receipt <platform target>=/absolute/upload-receipt-<platform target>.json \
  --receipt release=/absolute/upload-receipt-release.json
```

- The reviewed-commit rule holds for the lane path too. The tool runs only
  when it, the check and the candidate tool are byte for byte the files of
  the checkout's commit, and that commit is an ancestor of the default
  branch's tip. It asks the release repository itself, at its fixed public
  address and never through the checkout's own remote, for the branch's name
  and tip, with a git call that reads no configuration: a rewrite rule in the
  checkout's or the user's git configuration cannot send that question to
  another repository. Replace refs, a grafts file and a commit-graph file are
  not read: every commit's parents are read from the commit itself. An unknown
  branch, a failed fetch, an incomplete history, a tip the fetch did not bring
  and a commit that is not an ancestor all refuse.
- The fetch brings the tip's objects into the checkout and writes nothing
  else: no ref, no `FETCH_HEAD`, and no remote-tracking ref moves. Only a
  shallow checkout is changed, by completing its history.
- A run needs the receipt of `release` and of at least one platform target.
  One part of a release alone refuses before anything is read or uploaded.
- The staging directory holds exactly the rows of the receipts given, one
  receipt per target: a staged file that is no row refuses, a row without a
  file refuses, a name under two receipts refuses, and every entry is a
  regular file.
- The check runs once per receipt, for exactly that receipt's rows, on copies
  the tool holds. The same copies are uploaded. The version that the file
  names of every receipt are held to is the tag's. A pass counts only when
  the check read its pins from the commit the reviewed-commit rule passed
  for: a checkout that moved to another commit in between refuses.
- The release exists and is published, and its tag selects the release commit
  (the checkout's commit, or the one `--release-commit` names). A draft or a
  missing release refuses.
- Each uploaded asset is read back from the release and its size and sha256
  are compared with its row. The read-back is an authenticated read.
- An upload and a read-back may take up to an hour each; a read or a listing
  keeps the five minutes of every other request.
- An asset that is already present under a row's name counts as attached when
  it reads back with exactly the row's size and sha256, so a rerun completes
  a run that stopped part way. Other bytes under a row's name refuse.
- An upload that is cut off can leave an asset under the row's name that is
  not the row's bytes, or that the release does not show as complete. Every
  rerun then refuses, and the tool never deletes or replaces it: an owner of
  the release removes that one asset, and the next run completes the release.
- At the end an asset of the release that is neither a row of a receipt this
  run checked nor a candidate file is named, and the run exits non-zero.
- The tool adds files to an existing release. It never creates, edits,
  drafts, publishes, deletes or replaces a release or an asset.
- CI runs the tool's tests (`scripts/test_release_upload_attach.py`) against a
  release held in memory and a scratch repository; they contact no network.

## Required inputs still unbound

`connect-release-inputs.json` selects Linux; `connect-release-inputs-macos-arm64.json`
selects macOS. Both contain deliberate nulls and refuse before any native build.
The existing owners must supply all of the following separately for each target:

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

A library built by `scripts/package-library-release.py` publishes, of its
retained build source (the `selected/provenance/source/` tree and every
`selected/provenance/<name>/source.tar.gz` snapshot), exactly the members
`release/library-published-source-members.json` names, on every target. A tree
without a section in that list refuses the build. A member the list does not
name must be named, with its sha256, by the private list given with
`--non-published-list` (kept outside the repository); it is then left out, and
a member in neither list refuses the build. No published member may name a
member that was left out, by path or by digest: JSON records under
`selected/provenance/` lose the rows that do and gain
`"projected_for_publication": true`, every digest a published member states
about a projected record or snapshot becomes the shipped one, and anything that
still names such a member refuses the build. A release build refuses without
the private list; `--proof-only` builds without it and says so in its result.
The library's `selected/provenance/redactions.json` holds one sentence and the
SHA-256 and size of each shipped source snapshot; it names nothing that was
left out.

One kind of reference is exempt from that refusal. A path that is, letter for
letter, a regular file tracked by the commit of the checkout the packager runs
from (relative to the product root, read from git) names this repository's own
file, also where a member that was left out has the same path. Such references
stay, and the build result counts them (`exempt_references`, reason
`public_repository_path`, with the members that hold them). A digest is never
exempt, and a path that only the work tree, another commit or another branch
holds is not tracked.

The retained library `d0a20df31dfca19129ba36edcad0a2cb46295476bdbf0630bee224cb7c41c71f`
(128051975 bytes) still matches all 45 selected current public source leaves;
its other selected rows explicitly retain historical inputs. This supports
unchanged input reuse, not a claim that a 0.9.4 installed consumer has passed.
The library and Node policy still need selected public artifact URLs. Exact
Core/Meta 0.9.4 revisions, SDKs and actual reader/consumer qualification remain
unbound. The Connect wheel and starter are generated by their existing owners
in this workflow, so they do not depend on publication of the same candidate.

Limits of what the library packager does for members it leaves out: the
projection of a provenance record removes the rows that name such a member and
restates digests. Apart from a snapshot's size beside its restated digest it
does not rewrite a size or a count, and it does not recognise a name that is
split across fields. The search for references reads each published member as
bytes, and a source snapshot's content as well; it does not open another
archive inside the library, and it does not find a path written with other
separators or with escapes other than `\/`.

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
  --upload-receipt /path/to/upload-receipt.json \
  --output /path/to/new-acceptance-directory
```

The upload receipt is the input of the upload receipt check (see "Check the
receipts before an upload"); its pins are the checkout's and are not given
here. Acceptance refuses unless the check passes for the candidate's exact
files, and the record keeps the receipt as `upload-receipt.json`. It takes no
full scan report.

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

Before its first API call the writer runs the upload receipt check on the
bytes it retained, with the upload receipt of the acceptance record and the
pins of its own checkout's commit. A missing, refused, stale or proof-only
receipt, a receipt for other bytes, or a pin that is not set stops the command
with nothing written.

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
