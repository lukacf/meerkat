# Meta Forge

Turn an idea into an agent application through a guided conversation. Answer
one focused question at a time, inspect the proposed team and behavior, and
build a deployment package you can review and install.

Forge is a developer alpha. The current Meta 0.9 preview opens from a complete,
prepared product archive. Its generated README and manifest identify the
selected platform, SDKs and native runtime. A source checkout alone is not that
archive. Guided design, application setup and installed execution are separate
steps; use the controls actually offered by your package.

Forge is the setup and application authoring tool in Meerkat Meta.
Core runs agents and mobs; Meta composes applications around
that runtime with identity, policy and Console. Forge guides design, authoring
and packaging. See the [Meta/MobKit documentation](https://docs.rkat.ai/mobkit/introduction)
for the application layer and its relationship to Core.

**First time here? Start with [the short setup walkthrough](GETTING_STARTED.md)**
for the path from an idea to a useful installed job, and how to return safely.
The reference below covers platform prerequisites, advanced configuration and
development.

Source contributors should read [the development notes](DEVELOPMENT.md) for
shared Console builds and the separate SDK/runtime selections.

Historical local verification records and `.artifacts` directories remain
private. They are not part of this source tree or a standalone ZIP. Current
repository validation is recorded in the pull request and CI.

Application authors adding transport or UI setup through the installed Console
can use the [frontend integration contract](FRONTEND_INTEGRATION.md). It describes
the existing application hook, protected proposal flow and recovery ownership.

## Open the wizard

Use the complete prepared Meta 0.9 product ZIP and the operating system named in
its generated README. Unpack it into a new folder you will keep, open a terminal
there, and run:

```bash
bash start.sh
```

On macOS, to use **Run in the background**, unpack it outside Desktop, Documents,
Downloads, iCloud Drive, cloud storage folders and other volumes, for example into
`~/MetaForge`.

Open the address printed in that terminal, normally
[Meta Forge](http://127.0.0.1:8787). Keep the terminal open while using Forge.
The prepared package includes the frontend and its selected runtime; opening it
does not require Node.js, npm or a source checkout. You need Python 3.11+ with
`venv` and `ensurepip`, plus internet for initial pinned Python dependencies.

Applications install into Forge's own Python, and the Toolkit packages in this
release are built for CPython 3.11. On another CPython, Forge refuses the install
before installing anything and names the version it needs. On Ubuntu 24.04, whose
`python3` is 3.12, install CPython 3.11 with `uv python install 3.11` (or
`python3.11` and `python3.11-venv` from the deadsnakes PPA). If Forge already ran
in this folder, stop it and move only its `.venv` aside; saved setups are kept.
Then start Forge with `PYTHON_BIN` set to that interpreter:

```bash
test ! -e .venv.previous && test ! -L .venv.previous && mv .venv .venv.previous
PYTHON_BIN="$(uv python find 3.11)" bash start.sh
```

To prepare the package without starting its web server or agents, use
`bash start.sh --prepare-only`. To return to an existing setup, reuse the same
extracted folder and private state directory. If Forge is already running,
reopen its printed address instead of starting another copy.

Continue with [your first setup](GETTING_STARTED.md). Its quick path follows the
actual controls from describing a job to trying the installed application.
A prepared package's generated README takes precedence for its artifact and
platform selection; a retained handoff note does not select a new runtime.

### Maintainer preparation

The Meta 0.9 source launcher stops before dependency preparation when the
complete product manifest is absent. Installing a Python SDK or running a
frontend build does not supply its original runtime selection.
[The product packager](launchkit/product_distribution.py) assembles the exact
Meta/Toolkit wheels, selected Connect product, separate guide gateway and
frontend into the launchable archive. Private model configuration stays with
its original owner. The emitted manifest and generated README describe that
specific package.

For applications without a connection setup helper, the packaging owner must
also select the ordinary Meta RPC gateway matching the application Meta SDK's
source, version and destination platform. Pass its trusted
[`ExactFile`](launchkit/connect_installer_caller.py) selection to
`product_distribution.package(..., ordinary_gateway=selected_ordinary_gateway)`.
The packager checks the supplied digest and records the executable as
`vendor/ordinary/rpc_gateway` in the manifest's `ordinary` entry. The separate
guide gateway and the selected Connect gateway do not supply this input.

Omitting `ordinary_gateway` produces a package whose default runtime builds
applications with a connection setup helper only. The generated README states
this limitation. If an intended application has no helper, obtain the matching
ordinary gateway from the existing artifact owner and include it when preparing
the package; keep the application's design unchanged. A successfully opened
Forge guide does not prove that this separate application runtime is present.

The selected Meta guide uses native initialization and explicit revision-pinned
updates. The predecessor registry-repair recipes below apply to their matching
historical runtime. Current startup does not rewrite predecessor native stores.

<details>
<summary>Retained MobKit 0.8 startup and troubleshooting</summary>

These instructions describe retained 0.8 bundles and their matching source
checkouts. They are not the packaging or runtime selection for this Meta 0.9
checkout. Keep an existing installation's original package and state together.

### Retained launcher

If Forge is already running, reopen the address printed in its terminal. Start
it again only after it has stopped, using the same retained folder and private
state directory.

After unpacking the standalone ZIP, open a terminal in the extracted folder:

```bash
bash start.sh
```

Open [Meta Forge](http://127.0.0.1:8787). You need Python 3.11+ with virtual
environment and pip bootstrapping support (`venv` and `ensurepip`), plus internet
access for the initial downloads. The standalone wizard does not require Node.js,
npm or a source checkout. Setup creates its private Python environment and
downloads verified runtime binaries. Keep the terminal open while using the
wizard. The web server binds only to this computer.

The script reuses an executable `.venv/bin/python` from earlier setup. In a new
folder, it uses `python3` from `PATH`. You can explicitly select a Python 3.11+
executable for setup:

```bash
PYTHON_BIN="/absolute/path/to/python3.11" bash start.sh
```

An explicit `PYTHON_BIN` takes precedence for setup checks and creating a new
virtual environment. An existing `.venv` is retained and must itself run Python
3.11 or newer. If it is incompatible or broken, startup stops before installing
dependencies. Selecting `PYTHON_BIN` does not upgrade that environment.

If creating the environment or preparing pip fails, Forge prints the prerequisite
and recovery steps before dependency installation or server startup. A partial
`.venv` is retained. Correct the Python installation first, then use the backup
steps below if an incomplete environment remains. When preparing without starting
Forge, keep `--prepare-only` on the retry command.

If dependency installation or its final check fails, read the error above
**Forge dependency preparation did not finish**. For a download failure, restore
internet access, then use the printed retry command in the same folder. It keeps
your selected Python, port, state folder and gateway settings. Keep `.venv` and
your saved state in place; dependencies may have been partially installed.
Preparation stops before starting the web server. The retry retains
`--prepare-only` when selected.

To recreate only the Python environment, run these commands from the folder
containing `start.sh`. The first command backs it up without replacing an existing
backup; if `.venv.previous` already exists, choose another unused backup name.
Keep `.studio`, your configured state directory and settings in place:

```bash
test ! -e .venv.previous && test ! -L .venv.previous && mv .venv .venv.previous
PYTHON_BIN="/absolute/path/to/python3.11" bash start.sh
```

If port 8787 belongs to an unrelated application, choose a different local port:

```bash
META_FORGE_PORT=8788 bash start.sh
```

Changing ports does not recover an existing running Forge or separate its saved
state. Open the local address printed by the script. By default, each extracted
copy keeps its own setup history; separate running copies need separate state
directories. `META_FORGE_PORT` selects the local wizard port (default `8787`);
`META_FORGE_HOME` selects its private state directory (default `.studio` in this
copy). `MOBKIT_LAUNCH_PORT` and `MOBKIT_LAUNCH_HOME` remain compatibility aliases.
When both names are set, the corresponding `META_FORGE_*` value takes precedence.
Keep the same state directory when returning to saved setups.

### Standalone bundle

A packaged standalone copy runs with Python 3.11+, including `venv` and
`ensurepip`, on macOS or glibc Linux (2.31+).
It contains the prebuilt wizard, its assets and source examples, exact runtime
dependency pins, and a checksum-bound compatible Python SDK wheel. Node.js and a
source checkout are not required to open this wizard. New bundles carry the
published MobKit 0.8.47 SDK, which pins Meerkat 0.8.51, for the application
packages they build. Packaging and startup verify the SDK's fixed PyPI checksum, package
identity, release provenance and matching SDK/native version pins. Agent team
packages install the published Meta 0.9.4 release instead (see **Install agent
team**). The bundle downloads no native runtime of its own: each exported
package installs its runtime and checks the release checksums itself.

This is a developer alpha bundle. Dependency verification establishes the
selected artifact identities, not a complete application or animation acceptance
result. Previously created 0.8.42 bundles retain their local development SDK
source binding and original native pin. They are not silently upgraded.

After unpacking the standalone ZIP into a new folder:

```bash
bash start.sh
```

Use `bash start.sh --prepare-only` to verify the bundle, create its Python
environment and install its pinned dependencies without starting the web server
or native agents. Python package downloads require internet access.

Application builds containing JavaScript need Node.js for syntax checks. Builds
with offline DOM acceptance tests require the reviewed Node.js 25.9.0 runtime;
the standalone bundle carries the existing authenticated jsdom dependency closure.
These checks remain separate from browser rendering and installed application use.

Before a new application build, the wizard checks that its isolated checker is
ready. If preparation is needed, choose **Prepare app checks** in the application
build section. It downloads a private, checksum-verified Node runtime into this
installation without administrator access or changing system Node. The wizard
shows progress and enables building after readiness is confirmed. Preparation
does not start an implementation agent. Existing saved builds remain accessible.

For command-line preparation on macOS or supported glibc Linux, the same checker is available through:

```bash
.venv/bin/python -m launchkit.checker_runtime
```

This explicit command downloads the checksum-pinned official Intel or ARM64
archive and installs only its Node executable under this installer's `.tools`.
Checks prefer this verified private runtime and never download dependencies.
It does not replace system Node.js, frontend tooling or MCP runtimes. Readiness
also checks that the executable starts inside the host's isolation boundary.

Linux application authoring requires a kernel with working unprivileged user
namespaces, bubblewrap, libseccomp and the runtime libraries needed by the
reviewed Node executable. On Debian 12, the host packages are `bubblewrap`,
`libseccomp2`, `libatomic1` and `libstdc++6`. Linux checks use private process,
network and filesystem namespaces, mandatory syscall restrictions, read-only
source and tests, and private writable scratch. If these controls or runtime
libraries are unavailable, the wizard keeps building unavailable. It never
runs generated checks without isolation.

On Ubuntu 24.04 and later, install Python's environment support, `unzip`, the
isolation packages and the runtime libraries, then load the distribution's
`bwrap-userns-restrict` AppArmor profile from `apparmor-profiles`; installing the
package alone does not load it. These releases restrict unprivileged user
namespaces (`kernel.apparmor_restrict_unprivileged_userns=1`), so bubblewrap
cannot create its namespaces until a profile allows it. The profile applies only
to `/usr/bin/bwrap`: bubblewrap may create its namespaces, and the processes it
starts run without capabilities. The host-wide restriction
stays on. If `/etc/apparmor.d` already has a profile for `/usr/bin/bwrap`, such
as a file named `bwrap`, run `sudo apparmor_parser -r` on that file instead of
the last two commands: two profiles with the same name replace each other when
they load.

```bash
sudo apt-get install --no-install-recommends -y python3-venv unzip bubblewrap \
  libseccomp2 libatomic1 libstdc++6 apparmor apparmor-profiles
sudo cp /usr/share/apparmor/extra-profiles/bwrap-userns-restrict /etc/apparmor.d/
sudo apparmor_parser -r /etc/apparmor.d/bwrap-userns-restrict
```

A profile in `/etc/apparmor.d` is loaded again at every boot. To check the
isolation before you open the wizard, run:

```bash
bwrap --unshare-user --unshare-pid --unshare-net --ro-bind /usr /usr \
  --ro-bind /lib /lib --ro-bind-try /lib64 /lib64 -- /usr/bin/true
```

Container hosts also need a real PID 1 reaper, such as Docker's `--init`, and
an outer policy that permits bubblewrap to create its private namespaces and
`/proc`. The wizard does not weaken the container's policy automatically.
See [Node's platform requirements](https://github.com/nodejs/node/blob/v25.9.0/BUILDING.md)
and [bubblewrap's isolation model](https://github.com/containers/bubblewrap/blob/main/README.md)
for host prerequisites.

To recreate a retained 0.8 bundle from its matching source checkout, first build
the frontend and acquire its pinned `meerkat-mobkit` 0.8.47 wheel, then run:

```bash
PYTHONPATH=installer installer/.venv/bin/python -m launchkit.distribution --root installer \
  --sdk-wheel /path/to/meerkat_mobkit-0.8.47-py3-none-any.whl \
  --output /path/to/meta-forge-legacy-0.8-standalone.zip
```

Packaging checks the published wheel against the fixed official SHA-256
`bca838cc3b956949cb13cbe64c91ce5c07738a38385bc791b325ec0a3bdcb85c`,
records its official source revision, and checks that the prebuilt frontend
source maps match current input files. It does not claim the published wheel
matches a modified local SDK checkout. Legacy 0.8.42 packaging still requires
an exact local-source wheel and coherent 0.8.42 source pins. It selects public
application code, templates, reference images, assets and the authenticated DOM
dependency closure only. Saved state, credentials, native binaries and caches are
excluded. The command prints the archive SHA-256 for distribution integrity checks.

### Source checkout

From the root of the matching retained 0.8 repository checkout, use:

```bash
bash installer/start.sh
```

This development path requires Python 3.11+, Node.js 20+ and npm. It installs the
local `sdk/python` development package only when its declared version matches
the installer's SDK pin; otherwise it installs the pinned published package.
It builds the React frontend and downloads verified official runtime and
maintenance binaries. The installer requires the SDK's `gateway_init_timeout`
API. The standalone bundle already carries the checksum-bound compatible wheel.

Manual configuration, live registry search, and supported package compilation work without AI
credentials. Open **Guide settings** in the wizard to save a provider API key
privately on this computer, then choose **Start guide**. Saving does not contact
the provider; the next guide message checks access. When the setup team reports
ready, the first-use screen says **Ready for your first message**. Choose
**Back to my draft**, **Back to my starting point**, or **Return to Forge**,
depending on what you entered; otherwise close settings. Then choose
**Send to guide**. Starting the team keeps your idea and starting point unsent.
These provider steps apply to both standalone and source installations. Keys never enter the
chat or deployment downloads, and persist across normal restart. If a guide
message rejects a locally saved key, wait for current work to finish, open
**Guide settings**, choose **Stop setup team** and wait for its confirmed stop.
Enter a replacement key for the same provider, choose **Save locally**, then
**Start guide**. Saved conversations are retained; the failed answer is not resent.
Return to the conversation and use its offered retry deliberately. Active work
keeps its current owner and credentials; an existing native setup retains its
original provider.

You can also supply `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or `GEMINI_API_KEY`
in the server environment. Inherited keys remain private and read-only in the
wizard. A fresh setup chooses the first configured provider in that order;
`MOBKIT_ARCHITECT_MODEL` can select a compatible model. The generated
application has its own per-class provider/model choices and credential interview.
The OpenAI setup guide, integration specialist and reviewer default to
`gpt-6.1-sol` with low reasoning. The animation programmer also defaults to
`gpt-6.1-sol`, with medium reasoning; `MOBKIT_VISUALIZER_MODEL` selects its model independently.
For OpenAI animation, `MOBKIT_VISUALIZER_REASONING` accepts `low`, `medium` or `high` (default `medium`); it leaves the other setup profiles unchanged.
The application implementation agent also uses `gpt-6.1-sol` with low reasoning;
`MOBKIT_IMPLEMENTATION_MODEL` can override it for the configured provider.

The retained MobKit 0.8.47/Core 0.8.51 runtime owns the `gpt-6.1-sol` model
metadata. Forge preserves the selected model and reasoning without adding a
duplicate custom registry entry. A saved setup with the exact earlier compiler
declaration gets a retained, revision-pinned native definition update removing
only that declaration. Its identities, session history, provider settings and
compaction tuning remain native-owned. Changed or unknown custom declarations
require inspection; Forge does not replace them. An immutable completion record
preserves the repair evidence while later cold configuration updates continue
through the ordinary native revision checks. Historical 0.8.42 package generation
retains its explicit metadata support.

</details>

## Follow the guide

An organization can supply its own starting examples and guide context through
an installed Python plugin selected by the Forge host. Its examples replace the
stock opening suggestions; people can always describe their own idea. Choosing
an example fills an editable draft and never installs a connector or grants
access. Saved setups retain the plugin context they started with.

For operators, see the [installable workplace example](examples/workplace-starters/README.md)
and its version 1 contribution contract. Plugin installation and selection are
host configuration, not a tenant authorization boundary. This context-only
extension does not implement hosted account sign-in or cloud deployment.

The wizard first explores the job, audience and scale, how people will interact,
operational reliability, and deployment. Compatible channels and interfaces can
be selected together. It narrows down tools and connectors once that context is
clear. The next question comes from the
real setup agent, informed by your previous answers and the conversation. A complete
brief can skip answered topics; a vague category opens a concrete follow-up instead
of inventing a team. The guide tracks what you said, proposed defaults, and requested
features that need further work. It checks audience, identity ownership, evidence,
trigger, useful output, and tool authority when these affect the design.

The guide actively explores useful domain and operating specialists, even for
one person. Expertise can justify a role before it has accumulated history.
Contributions and collaboration shape the team; there is no target agent count
or domain-specific roster template. Related work can share a coherent remit.

When the work depends on incoming changes, the guide retains source scope,
recipients, audience and history choices in the proposal. See
[Incoming information](INCOMING_INFORMATION.md) for routing choices, validation
and the implementation checks still required before collection can run.

The left side is one continuous conversation with interactive answer cards.
Select one answer or all that fit. Your selected answer appears above the
composer, where you can remove a choice without losing your writing. Add optional
context in the same composer, then choose **Send answer**. Single, multiple and
combined choices use this same Send action; selecting or removing a choice sends
nothing. Use **More than one fits** when offered to combine choices, or ask for
help before sending. Use the same composer to answer in your own words or ask the
guide a question. Previous answers remain in the conversation and can be revisited.
Revisiting a topic keeps its saved context and asks what you want to change;
the guide uses your answer to shape the next choices.

The right side animates the current proposal: named agents, messages moving along
workflow dependencies, parallel work, tool access and shared knowledge. Choose a
workflow, pause, step, replay, or expand the map. Select an agent or source to
inspect responsibilities, exact grants and collaborators, then refine it with the
guide. The simulation describes the proposed system; it does not run it. Peer
permissions, workflow handoffs and tool access have distinct visual treatments.
Use **Explore identities** or a population button to browse every configured
member. Search narrows the results; Previous and Next move through pages of 60.
Selecting a member in the automatic preview reveals its exact relationships,
including when the overview aggregates a large team.
The visualization programmer studies the supplied household and engineering
organisation source examples and their renderer, together with the actual mob
configuration. It writes a custom Canvas animation program: its own composition,
drawing helpers, layout, labels, motion and story pacing. The host provides
playback, optional chapter navigation and identity inspection. There is no
positions, grouping or choreography schema for the agent to fill in.

The scene can explain intended conversations and result handoffs described in the
brief or agent instructions, even before their application channels are wired.
It distinguishes those intended interactions from configured peer permissions,
tool grants and observed activity. An unspecified conversation owner remains open;
the native default alone does not identify the person-facing owner.

The program runs on an OffscreenCanvas in a dedicated worker inside an
opaque-origin frame, with network access blocked by CSP. An execution timeout
keeps a broken animation from freezing the wizard. The proposal remains the
source for responsibilities, relationships and grants. Freeform drawing means
visual and semantic quality need review; accepting runnable code is not proof
that the picture is accurate.

Animation starts with the first useful draft, before final review, and regenerates
after meaningful guide or manual changes. The prior animation remains visible
and labelled as updating while its replacement is composed. Each open view
declares its current request before the generation debounce. Changing the view
or proposal withdraws that view's obsolete queued work; another window's demand
and an admitted native generation remain intact. Identical requests share work.
View intent persists across server restarts, renews every 30 seconds and expires
after 90 seconds without renewal. Delayed requests cannot revive a released or
expired intent. **Check again** renews the scheduling intent while retaining the
exact proposal, feedback and reviewed correction parent. Completed
programs must match the current proposal and model fingerprints before being
shown as current. Programs are cached by the proposal, model, contract and checked
source/image reference hashes. Updating either original reference image requests
a fresh composition; saved animations remain readable. Animation source remains
available below the preview.
Slow native composition remains an observed request after its bounded observation
window expires. Forge reconnects to the same durable native admission, with
bounded backoff, instead of reporting a failed animation or sending the job twice.
The previous scene stays available; an initial automatic preview is labelled
provisional. Actual native termination without a published program still fails.
The animation endpoint accepts a bounded 8 MB request so the fully resolved graph
can retain every identity and relationship for larger mobs. Other API request
limits remain unchanged.

Use **Refine animation** to tell the visualization agent what would make the
scene clearer. It can change the composition, labels and pacing for the same
proposal. Your current scene stays visible until the replacement draws its
first frame. Build, trial and installation status remains in the host panels;
the authored scene explains the agents and their collaboration.

If another browser has published a clearer animation for this same proposal and
view, choose **Use latest saved animation**. Forge opens that saved result without
waiting for another generation. Your choice survives reload in this browser
session. A changed proposal resumes normal continuous generation from the new
facts. Selecting a saved animation preserves any unsent refinement draft; use
**Refine animation** when you want the agent to change the drawing itself.

Changes from the guide or manual editor update the same simulation. Decisions you have not answered remain labeled as
undecided. Review the final recommendation and accept its design and choices.
When package preparation is available, download the package and follow its
installation checklist. No infrastructure is created by accepting
the plan. Without AI credentials, the wizard explains what is missing and keeps
the last saved step; it does not simulate agent responses.

Change an agent count in the plan, edit **Show config**, or use **Adjust details**
whenever you want direct control. Returning to the guide
sends the concrete changes back to the agent for its next recommendation. Pending
changes remain available to retry, adjust, or discard if that request fails. Guided sessions persist
locally, with version checks to prevent competing browser tabs from overwriting
each other. Your setups keeps previous conversations reachable. Draft answers and
selected choices survive reloads. Long agent turns continue after a browser leaves;
returning reconnects to the saved operation instead of sending a second request.
If the server stops mid-turn, the submitted answer remains available for explicit retry.
Use **Reload saved conversation** to inspect progress before choosing **Retry saved answer**.
**Retry setup team** starts the team while retaining the conversation; it does not resend
your answer. See [Return without starting over](GETTING_STARTED.md#6-return-without-starting-over)
for the recovery controls.

## Build the application

When a guide has saved organization context, each new build retains that exact
snapshot for both the builder and independent reviewer. Updating the installed
organization plugin does not rewrite an existing guide or build. Acceptance and
package matching include the saved context, so changing it requires fresh review.
Older builds without organization context retain their original scope.

That context can be a plain document or skill describing tools, data and how to
reach them. The application agent receives the saved wording and can give it to
Connect's setup agent when it needs a connection. Connect uses its existing
discovery and installation flow; an organization does not need to define a tool
catalog. See the [workplace example](examples/workplace-starters/README.md) for
a short prose contribution and the supported handoff.

This handoff carries visible context and references only. A reference to a private
authentication or installation package does not install it or grant access. The
host must adopt the actual package through its supported source-preparation path,
with compatible dependencies in the selected runtime. Missing source or conflicting
dependency versions remain explicit implementation dependencies.

When the guide has sufficient context and no necessary decisions remain open,
**Get your agent team** opens a visible package card. Package availability is
checked separately from design acceptance. After acceptance, a plan with a
connection setup helper shows **Accepted design - package preparation pending**: its choices
are saved, but new team exports, fresh application builds and runtime rebuilds
wait for installation support. Retain the full design for later, or choose
**Revise with guide** to discuss alternatives while preserving your requirements.
Any deliberate reduction in scope must be an explicit revised proposal that you
review before accepting. Existing installations keep their own controls. A saved
build can continue from its exact retained framework only when its current status
and source permit that action;
refresh unavailable build status before trying to resume it.

When package preparation is available, **Download agent team package**
compiles the current agents, native Console, configured tools and permitted notes
without an application-builder turn. Proposed plans remain labelled as proposed;
downloading does not accept their defaults. Remaining implementation work stays
visible. Follow the package's installation steps and set up credentials and
connection prerequisites on the destination computer. The export does not establish
that custom behavior, connections or deployment have been verified.

After **Use this plan**, and when package preparation is available, choose
**Install agent team** to check the plan's deployment destination. A matching Mac
or Linux host offers private setup in the wizard. The form asks for the required
credentials, destination and Console port. **Install and start** uses the accepted
plan, installs the pinned runtime, checks selected MCP tool grants and waits for
native bootstrap and Console health. A Linux plan prepared on a Mac, or a Mac
plan prepared on Linux, offers its destination package and manual steps instead.
No application-builder turn is needed for either path.
The installed team names its conversation owner and offers its actual Console.
Stop, edit private settings and start again through the same installation controls;
native history and notes stay in the owned project folder.

The agent-team path contains the compiler's native host and configured tools. It
does not add custom application behavior or independently verify a useful job.
Connector prerequisites must already be available on the destination, including
any computer-specific paths in the plan. This direct installation path currently
supports the local computer. Native-team source updates are not yet supported;
refining the guide does not change an existing installation.

An agent-team package also binds the guide's saved organization context. New
installations retain that binding when reporting whether they match the current
guide. Changing or removing the context does not rebind an installed team.
Packages without organization context keep their original scope. Older Forge
versions that cannot read organization-aware team manifests refuse those
packages; use the Forge version that prepared the package for installation.
See [native package compatibility](COMPATIBILITY.md) for affected versions and
the behavior of retained packages and receipts.

The installed Forge (the packaged product) and the developer source entry both
offer the agent-team path. An agent team package, downloaded or installed,
installs the published Meta 0.9.4 release it pins: the `meerkat-meta` 0.9.4 SDK
from PyPI by its fixed SHA-256, and the official `meta-rpc-gateway` and
`meta-gateway` archives, each checked against the release's published SHA-256
checksums. It does not use the runtime an installed Forge selected for its own
applications. A model the pinned release does not list is refused before export
or installation, and the message names models it does list. Such a package keeps
the definition it was installed with; to change instructions, classes or flows,
build the changed team and install it into a new folder. Agent teams installed
earlier on MobKit 0.8 keep their own runtime and controls, and a Meta 0.9 package
refuses to open MobKit 0.8 state. Use **Build application** when your agents need
custom application behavior.

Use the **Custom application** section when your requested outcomes need custom
screens or additional application code. A team export does not implement remaining
application work. **Build application** can start while implementation requirements are still
open, but necessary guide decisions must be resolved first. This opens the build
controls without starting work or treating unfinished requirements as complete.
A saved refinement with a complete brief and no pending question can also use
**Use this plan** directly. This records your explicit acceptance of the current
proposal and proposed defaults without requesting another guide turn.
The native implementation agent reads the
saved brief, conversation, exact team and generated host, then writes ordinary
application code and meaningful tests. Its tool calls, provider reasoning summaries,
results and final response use the same native Console transcript components as the
guide. The proposal simulation and guide remain available while it works.

When the launcher selects a Meta/Toolkit runtime, the builder and reviewer read
that selection's exact SDK source. Recurring-work guidance stays available, while
native scheduling capabilities come from the tools actually granted by that
runtime. Documentation does not establish installed execution.

An optional timing disclosure shows the host's recorded build phases.
Native runs include tool execution and delivery, so phase durations overlap.
These measurements help locate waits; they do not isolate model inference time.
Older builds without measurements show no estimated timing.

The builder can add domain logic, custom per-agent callback tools, channels, UI,
background adapters and native schedule callback handlers. It uses an optional
`application/__init__.py` exporting `build(context)` and an integration object with
`tools_for_agent(agent)`, `schedule_runnables()`, `start()` and `stop()` hooks. Source
is ordinary code with relative imports, not an application design schema. Native
MobKit still owns identity, agent sessions, comms, flows, memory and scheduling.
CLI flow execution registers callbacks without launching background services;
callback adapters must initialize lazily. Native runnable registration does not
grant schedule authoring or certify restored occurrence delivery.

Applications can additionally implement `select_host_tools(agent, available_names)`
to select a unique subset of their compiled-granted local/MCP callbacks for each
identity. Returning `[]` omits the host callbacks; omitting the hook keeps existing
behavior. Scoped application callbacks remain separate, so an application can
exclude global notes/files and use its own authorized context store. This hook
cannot add tools or replace native builtins, memory or comms authority. Invalid
selections fail closed. Its full catalog is validated after connector startup;
`--application-check` validates callability without proving runtime enforcement.
Selection changes require a cold host restart; reconciliation does not rebind
existing agents. Retained Python callback scopes alone do not establish native
access. After native startup, inspect
`context.runtime.mob_handle().identity_resolved_tools(identity)` or
`identity_resolved_tools_detail(identity)` and verify denied old-name calls on
continuing identities before claiming installed isolation.
Frozen packages need an explicit framework refresh/rebuild to acquire this hook.

Applications serving independent groups must also define their native memory
sharing boundary. The qualified MobKit 0.8.47/Core 0.8.51 runtime binds Recorder
identity records to the current identity and realm, binds `memory_search` to the
current session, and checks canonical trusted peers for native messaging.
Its automatic memory manifest also includes titles and descriptions from the
bound Mob and Realm. Disabling automatic promotion does not remove previously
committed shared metadata. Python's current `agent_memory` configuration has no
identity-only manifest switch; disabling memory also removes Recorder.

To retain Recorder while separating shared Mob/Realm metadata between groups
under this runtime, use a domain-partitioned native host: each group has its own
roster, topology, memory realm and canonical
persistent state root, and private assistants have owner-only domains. The
application selects the domain from authenticated ownership before admitting
native work. The current generated host uses one native runtime; callback
selection alone does not establish this separation. Preserve existing sessions
and design their continuation explicitly before changing domains. Scope denial
and restart behavior still need actual native execution evidence.

The agent works in a separate private project. Compiler-owned host, configuration
and deployment files are read-only. It cannot deploy, read studio credentials,
access user files, install dependencies or command devices. Authored Python modules
stay inside `application/` and `tests/` to preserve native imports. UI, assets and
documentation can use ordinary project paths. Dependencies belong in the optional
versioned `requirements.application.txt`, installed by the destination's installer.

Executable checks use a fresh snapshot and scrubbed environment inside macOS
sandbox-exec or verified Linux bubblewrap isolation, with network and user-home
access denied. They cover Python/Node/shell syntax,
generated configuration, application callback contracts and the complete authored
unittest suite. Time and output limits bound execution. At least one executed,
non-skipped test and all passing checks are required for a host-signed receipt tied
to the exact source bytes. Authored assertions are evidence of the behavior they
exercise, not proof of all product requirements. Linux authoring requires the
isolation and runtime prerequisites described under [Standalone bundle](#standalone-bundle).
Choose **Prepare app checks** in the build section to check this computer. If
isolation is unavailable, building remains disabled before a paid build. Packages
authored on a supported Mac or Linux host can target Linux or GCP.

A separate native acceptance reviewer then reads the original brief and frozen
source, writes protected counterexample tests outside the builder's workspace,
and assesses each requested outcome. Its findings can trigger up to two automatic
repair rounds with the builder. A completed native review, passing protected tests
and current source-bound acceptance are required for new checked packages and
wizard installation. Live dependencies remain explicit. The guide receives a
compact projection of these findings so its next question can address what is
actually missing. Older source-only packages remain inspectable and downloadable;
**Review and improve application** brings them through the current review policy.

For a requested human UI, builder and reviewer guidance distinguishes current
application results from earlier agent messages. Plain status, next action and
outcome should follow actual application state, including after confirmation,
refresh and restart. Original native prose remains history; technical records
belong in optional details. Passing API checks alone does not verify that screen.

Feedback continues from saved source in a new build without overwriting earlier
source or native histories. With the same complete proposal and brief, it retains
the accepted host framework as explicit source provenance while running fresh
checks and independent review. It does not silently upgrade host templates.
Changed native configuration or dependencies require a freshly compiled host and
remain subject to the existing migration checks. Older exports without provenance
retain their original report format and package bytes.
A revised proposal keeps authored application files,
regenerates its native host and gives the builder both designs for adaptation;
previous check receipts never carry forward. Browser reconnects resume the current receipt and
transcript; host loss interrupts work and preserves source for retry. Changes to
the proposal or source invalidate download eligibility. **Download checked
application** contains the exact checked source, native host/configuration, test
receipt, unresolved integrations and complete checksums. The main download action
uses that package when available. **Download agent team package** exports the
native team, runtime host and configuration. **Download native setup only**, when
offered under **Manual exports and terminal setup**, also excludes the builder's
custom application code.

A conversation revision alone does not expire an unchanged checked application.
The host compares the complete validated proposal and cumulative application brief,
while preserving the original receipt version, frozen evidence, signatures and
package bytes. Changed scope, configuration, topology or new implementation feedback
still needs fresh work and checks. Legacy artifacts without a complete bound brief
cannot carry forward across conversation revisions. New actions keep their exact
current-version and source checks. This depends on the guide recording requested
scope in the canonical brief; the host cannot detect a requirement omitted by the
guide, so scope capture remains part of design review.

Checked source does not claim live connector authentication, hardware operation or
deployment. Meerkat Toolkit owns the evolving connector investigation/onboarding
direction, separate from application generation. The local host owns credentials,
activation and grants. Outbound service access and inbound user/event frontends
need separate readiness evidence. An optional host-selected Toolkit catalog now
gives the integration specialist `setup_catalog` and `setup_catalog_entry` through
the existing MCP client. The host pins the catalog and bundle hashes, supplies an
isolated supported Toolkit interpreter and fixes its command arguments. Set
`MOBKIT_TOOLKIT_CATALOG_CONFIG` to that local host configuration to enable it.
Each start uses a fresh private workspace; every dispatch is limited to those
two metadata tools. Missing or invalid configuration leaves ordinary registry
discovery available. This does not prepare or install an integration, authenticate
an account or enable Toolkit's native acquisition/continuation dispatcher.

The host can also enable Toolkit's existing software setup controller with
`MOBKIT_TOOLKIT_SETUP_CONFIG`. This separate local binding pins the setup source,
consumer bundle, catalog and supported Python interpreter, and explicitly allows
local software installation. The integration specialist then uses the ordinary
Toolkit preparation, installed help, configuration check, installation and job
tools. The person stays in the guided conversation; they do not edit a catalog.
No credentials are accepted in those tools. Checked credential reference names
become private deployment inputs through the normal connection handoff.
Names reserved for the host, Console or model provider are refused; configure
dedicated connector credential names in Toolkit instead of reusing those fields.

Neither optional catalog nor setup-controller binding removes the package
preparation limitation for a selected connection setup helper. That capability
remains part of the saved proposal; enabling these bindings does not change its
scope or make it ready to package.

For Toolkit's frozen `20261004-host-bootstrap-r2`, `20261004-host-recovery-r2`,
`20261004-gmail-structured-draft` or `20261005-saved-setup` macOS ARM64 or Linux
x86_64 prerelease, the host can instead point `MOBKIT_TOOLKIT_SETUP_CONFIG`
directly at the host preparer's or offline bootstrap's emitted
`configuration/mcp.json`. Keep its original `prepared.json` and emitted selection
files beside it: `selected-install.json` for fresh setup, `recover-manifest.json`
for readback, and the additional `prior-request.json` when reusing saved settings.
Use the maintained `agno,services` families without acquisition or additional
preparation capabilities. Forge
checks the known prerelease source, catalog, runtime and prepared file pins,
then retains the emitted command, arguments and fixed workspace exactly.
These local prerelease pins do not establish public release or future-version
compatibility. The workspace belongs to one setup session: an occupied workspace
is refused before launch and is never deleted or silently replaced. An installed
application's restart remains separate from this optional setup controller.

The architect retains the exact succeeded job's ungranted MCP command and
arguments in its existing discovery path. For the maintained Agno family, only
the checked selection's documented operation names can support explicit class
grants. For Services, the checked selection uses literal MCP tool names supplied
by the publisher's installed help. Missing mappings leave the connection
ungranted; invalid or duplicate names are refused. Operation IDs are never
converted into guessed tool names. A software receipt or
documented tool list does not establish account access or useful execution.
Installed absolute paths support the current host; another computer or a cloud
destination needs its own compatible installation. This optional controller
and its frozen Toolkit bundle are not included in the standalone Forge ZIP.
Shutdown settles owned setup jobs before releasing the MCP controller; an
unconfirmed stop retains that owner rather than starting a replacement.

After setup-process loss, preserve the original workspace and job request
anchors. Once its old workers have stopped, follow the selected recovery-capable
archive's root guide and `selected/contracts/HOST-PREPARATION.md` to prepare a
separate readback configuration and new workspace with `--recover-manifest` and
`--recover-manifest-sha256`. Use that new emitted `mcp.json` as the host binding on its next start.
Forge exposes only `setup_state`, `setup_jobs`, `setup_job` and `setup_reconcile`
in this mode. It cannot install, retry, stop old workers or produce a connection
handoff. An unconfirmed historical job stays unconfirmed; reconciliation is a
separate observation. Recovery does not restore accounts, grants or agent tasks.
Do not delete the original workspace or restart its old setup command.

To reuse nonsecret settings in a fresh setup, the `20261005-saved-setup` helpers
accept the complete `--prior-manifest`, `--prior-manifest-sha256` and
`--prior-request-id` selection. The host must approve the whole selected request
for the new setup audience, retain the original files until preparation finishes,
and choose new configuration and workspace paths (also a new client directory
when bootstrapping). Use this fresh emitted configuration on the host's next start. The
integration specialist uses `setup_successor` to review the settings against
current help and selects current operations before a new check or installation.
This is separate from readback recovery; do not combine those input modes.
Old installation outcomes, account access and grants do not transfer. These
instructions cover only Forge's pinned prereleases above, not a newer Toolkit
starter or library merely because it also emits `mcp.json`.

### Review saved tool settings

Forge also offers a **Saved tools** panel for reviewing a connection from a
Toolkit recovery file. To enable it, the host sets
`MOBKIT_TOOLKIT_SAVED_REQUEST_CONFIG` to the supported `20261005-saved-setup`
edition's original `prepared.json`, and `MOBKIT_TOOLKIT_SAVED_REQUEST_ARCHIVE`
to its exact archive before starting Forge. Both inputs are required; this
binding is separate from `MOBKIT_TOOLKIT_SETUP_CONFIG`.

Open a saved setup in Forge, choose **Saved tools**, then **Choose recovery file**
and select the original Toolkit recovery file. Select a connection, inspect the
full request and confirm that it contains no secrets and suits this setup's
audience. Choose **Review current settings** and keep the original workspace
available while the review finishes. The panel shows current tool help, proposed
settings and any saved values that need a new decision. Current operations must
be selected again before a later installation. **Discuss how this fits my mob**
sends a discussion request to the guide while preserving the composer draft.
This review does not install a connector, authenticate an account or grant tools.

### Connect application-host handoff

The software setup handoff above does not register a Forge-generated application
as a Connect frontend host. The existing concrete frontend selection and
change-management path target a home-automation host. Forge now has reviewed private source for
an application-host path: exact native installer receipts, selected-member policy
input and protected controller-review presentation. These pieces are not activated
in generated applications. A prepared change or operator review request is not
frontend activation.

The selected application path uses Toolkit's same-runtime preparation and native
adoption, followed by Restore of the retained installed selection. Integration
still needs the exact qualified Core/Meta/Toolkit gateway and SDK artifacts, plus
the application's original admission, principal, audience, conversation, delivery
and lifecycle owners. Selecting only the home-automation host does not select a Forge host.

The public guided installer currently supports selected Meta/Toolkit setup on
local Mac or Linux hosts. A trusted Build owner can compile an unchanged GCP
proposal against its selected Linux x86_64 product. The installed source reader
also preserves the original GCP project, VM and package binding. Private source
integrations cover stopped product Install and same-owner settings, setup and
lifecycle actions; their controls keep SDK, native and cloud execution inert.
The normal guided SSH and GCP installer still refuses Meta/Toolkit product
packages. The remote instructions below apply to ordinary supported application
packages. Remote product setup and protected access are now integrated in source.
The served and native paths remain unaccepted until the qualified Toolkit runtime
is selected and a fresh trial proves a useful task, correction and restart.
A real cloud journey also remains release work.

A trusted launcher upgrading Forge can retain an already installed ordinary
application's runtime separately from the default used by new builds. Supply a
tuple of `RetainedOrdinaryApplication` values to
`launchkit.packaged_runtime.create_app(retained_ordinary_applications=...)` (or
the same keyword on `server.select_local_application_runtime`). Each value
selects the original guide, deployment and accepted build IDs, the exact package
SHA256, its `MetaToolkitRuntimeTarget`, and its separately pinned ordinary Meta
gateway. These are launcher-owned inputs, not browser fields or selections
inferred from a saved receipt. Preserve the selected files at their original
paths and preserve the application's original credential configuration.

Normal **Start** or **Restart** then authenticates that installation's signed
build, complete package, private source and stopped ownership before
reconstructing its original owner. The ordinary worker retains its physical
runtime verification before execution; source recovery is not that proof.
This does not make the older build available
for a new Install or change the default for a new Build. A changed or unselected
installed source is refused. An application update that changes the accepted
package needs a corresponding explicit launcher selection before a later Forge
restart. This path does not convert an ordinary app into a background service or
Toolkit Restore; deployments whose process belongs to Forge's service still
require a graceful application stop and restart during host maintenance.

Guided GCP preparation currently creates Debian 12. The trusted remote control
selection must use dependency wheels for the interpreter actually observed in
that guest and retain the exact interpreter/ABI check. Select the matching
retained dependency set before installation; a prepared Ubuntu 24/CPython 3.12
batch does not establish compatibility with this guest. Control dependencies
remain separate from the application's qualified Meta/Toolkit SDK and native
artifacts.

For relocated artifacts, the trusted `MetaToolkitRuntimeTarget` may set
`source_manifest=ArtifactFile(path, size, sha256)` inside its owned
`artifact_root`. The selected copy must retain the exact source bytes, size and
digest recorded by the producer. Point `model_catalog` to its relocated copy
with the unchanged verifier-recorded digest. Keep producer assembly, build and
verification records unchanged. Omitting `source_manifest` preserves the
historical producer locator; explicitly pinned selections require the updated
Forge reader. Relocation checks do not qualify a runtime.

The private Python owner entry `DeploymentService.start_application` now accepts
an explicit `fresh` or `restore` selection for the exact stopped interactive
installation. Fresh supplies the pinned gateway, original preparation plan,
source policy and activation template, plus the original provision, working,
bundle and activation paths. Restore supplies the pinned gateway, selection,
preparation plan and completed setup. The existing deployment owner retains
source, settings and input custody under the project lease, then passes the
original admission and launch lifecycle through the fixed worker and isolated
application child. The browser cannot choose this path. This entry consumes
existing native inputs; it does not produce them or qualify the installed runtime.

An entered Fresh attempt retains its original durable entry. A later explicit
Restore must select that Fresh attempt's activation and bundle paths as well as
the exact current manifest and completed setup. It still needs native validation.
Ordinary startup and new source-update staging remain refused while that entry
is retained. Console health does not establish a native disposition or clear
the entry. The supported follow-on needs the original native outcome, and later
replacement restart also needs the actual last accepted desired policy. Current
source integration and focused tests do not establish these installed outcomes.

The retained development SDK package was source-verified without changing
Forge's active environment. Native factory work still must return the selected policy binding
and verified artifact on the same construction, verify the final tool surface and
retain cleanup ownership through refusal or cancellation. The installer input
adapter retains explicit native identities or joins them through the same
caller's retained inventory, then asks native code to compile the policy. It does
not infer identities or create an operator. The browser presents the retained
review; it does not choose the policy
or start another host. See the [frontend integration contract](FRONTEND_INTEGRATION.md)
for the source boundary. Current source checks are reported in
[Forge CI](https://github.com/lukacf/meerkat-meta/actions/workflows/forge.yml);
installed execution remains a separate outcome.

Forge already has application composition and lifetime hooks in
`launchkit/templates_v2/app.py`: `build(context)`, per-identity callbacks, native
runtime startup, authenticated Console input and ordered shutdown. Integrate the
supported contract through those hooks and the compiler, preserving native
ownership and recovery. Do not substitute a Forge URL into the home-automation host's fixed routes
or represent an inbound frontend as an MCP connection. Acceptance requires a
useful operation through the selected frontend on the actual generated application,
then supported restart with its original conversation and state. Source inspection,
transport availability or a rendered management UI alone does not establish that
outcome.

The installation owner must supply the startup factory to `app.main` inside the
child's existing project lease. The private factory returns an asynchronous
context manager, entered on the child's actual event loop. It retains the
accepted source before constructing and yielding the selected startup. The
existing application run and shutdown finish before the source context exits
and the project lease is released. Selected `--doctor` also enters and exits
that context without starting the application. `--check` and
`--application-check` return before entering it. Its inputs are the accepted
application source,
exact gateway and plan pins, original native authority bindings, finalization
workspace and selected policy files. Restart instead selects the retained
activation, manifest, completed setup and original callback plan. Ordinary
production startup still runs `app.py`. An explicitly admitted replacement now
uses the trusted worker dispatcher and isolated child factory, which retain the
original transaction, accepted source and actual project lease. The authentic
native input producer and installed qualification remain unfinished.
Source-currentness checks must cover the
whole accepted package, including files outside the native compiler's input set.

For a changed application, the private admission retains the owner-supplied
update-selection file, actual prior desired policy, preparation plan and gateway
as exact pins, with the original association directory and revision decision.
The native selection carries the predecessor activation, manifest and local
policy plus the candidate application-compilation input and native tool policy.
Archive pins and accepted-source custody cover the full application source
closure. These remain bound to the admitted `update_id`, candidate package,
build/source identity and original current pointer. Actual native authority,
request and policy bindings remain producer-owned. No replacement is inferred
from a blueprint or a new preparation attempt.

Toolkit's source-delivered `ApplicationUpdateSelection` uses the existing `update.json`
owner: the exact update ID, pinned predecessor `HostBootstrapSelection`, pinned
application-compilation input and pinned native application tool policy. Its
read-only `--validate-application-update --selection FILE` command checks those
inputs against the original preparation and native policy compilation. Success
returns `application_update_validated` with the exact selection, establishing
input validation only. Toolkit's committed source has passed independent source
review; its native controls and matching runtime artifacts remain unqualified.
Forge's dormant `NativeInstallerCaller.validate_application_update` retains
the exact selection, predecessor and candidate pins, owner-supplied source
closure and original attempt custody. Its success is a historical validation
receipt, with no publication or adoption carrier. The production worker now
joins the native selection to the original local archive admission and child
factory. The matching qualified native consumer remains upstream. Neither a
validation receipt nor a selected update publishes desired
policy or establishes member adoption.

Until the native replacement handoff exists, ordinary application updates must
retain the exact bytes and normalized archive mode of `application/tool-policy.json`.
Adding, removing or changing that source needs the verified permission-update
path. The worker checks the saved packages before native entry. Source selection
and receipt publication repeat those checks under the original project lease.
Staged diagnostics and checkpoint progress remain available; they do not
authorize a policy change.

An admitted local update, rollback or recovery retains its exact archive
selection and original installation before the worker is queued. That private
selection travels through the existing worker boundary and is checked again
before source changes, service entry and recovery completion. Normal progress
records may advance; they cannot replace the selected package or destination.
An earlier worker can finish cleanup against its own original source while
leaving the next staged update intact.
A static installed service may report its actual startup health but cannot
complete a restored transaction without the original dynamic selection.

The private Python owner entry `DeploymentService.apply_replacement` accepts
the actual native inputs from the stopped interactive application's owner. It
freezes the caller's selection before waiting for the existing service lock,
retains original file observations through the final accepted-build and guide
checks, and queues the same worker with its exact admission. Joined file reads
keep package checks off the event loop and drain cancellation before releasing
the lock. Pending support custody, background services, active workers and
unconfirmed stopped state are refused. The browser action schema cannot supply
this selection. This entry does not prepare native resources or accept policy;
the real producer and native acknowledgement remain unfinished.

The normal **Apply accepted update** action can use the same admission when the
trusted launcher supplies retained replacement inputs for the exact installation
and update. Selection occurs only after claiming the original command UUID;
rechecking a settled or uncertain command does not select or start it again.
The supplier reads private inputs, without preparing native resources or using
the initial acquisition check as update authority. Existing installed-source,
accepted-candidate and stopped-owner checks still apply. Fresh setup inputs cannot
substitute for a replacement. The authentic supplier and matching qualified
runtime are still required before this path can run an installed product update.

Toolkit's reviewed replacement-startup direction uses Forge's existing stopped
update and the replacement child's startup factory. That factory restores the
original authority, current pointer and resources while selecting the admitted
candidate definition, policy and callback source. The native prepare hook can
compile associations from the actual prepared tool surface before finish;
a completed replacement inventory is not a prerequisite. The private admission,
worker and factory consumer are connected in production source. Their actual
installed outcome and matching runtime package remain unqualified.
Ordinary Restore retains its original definition, policy and callback checks.

The existing update transaction must retain the actual prior accepted desired
policy, including intervening updates. The original bootstrap policy alone
cannot establish that continuity in a new process. Candidate source and desired
selection publication retain their existing project lease and update owner.
Keep native declaration outcomes, each member's convergence, failure and
uncertainty separate from source publication and association observations.
Preparation returns, callback counts and gateway Ready do not prove all-member
or policy adoption. An ambiguous publication or settlement retains its original
selection and attempt. Source changes do not implicitly authorize new issuance,
automatic re-preparation or rollback.

Compiled MCP tools keep their advertised input constraints. For OpenAI agents,
regex lookarounds unsupported by the provider are omitted from the model-facing
schema; the original schema validates arguments before any connector invocation.
This compatibility projection changes neither the selected tools nor their grants.

## Try the design

**Trial** jumps to **Try your mob**. Enter a realistic request, optionally paste
sample context, and choose conversation with the owner or one of the proposed
native flows. This starts the exact proposed identities, roles and peer edges in
separate native state. Granted workspace and notes callbacks use only explicit
fixture data; external services, application adapters, shell and hardware are
unavailable. This is a design rehearsal, not an application or device test.

Conversation trials stay open for follow-ups. **Continue this trial** sends the
next question through the same native identity and conversation, retaining its
sample workspace, notes and application sandbox. It does not rebuild the mob or
repeat sample setup. Native flows remain single-run trials. **End trial** waits
for admitted native work and stops the owned sandbox before exposing feedback.
Trials allow at most ten turns, five minutes waiting for a follow-up and ten
minutes total. An expired, changed or interrupted trial cannot accept another
turn; its actual history stays available.

The native Console transcript shows actual peer activity, tool calls, results and
provider reasoning summaries, with sender labels. **Replay trace** restores the
tool state observed at each saved event and supplies actual identity activity to
the proposal animation. Generated animation code can use that optional activity
without changing its composition contract. Illustrative particles remain distinct
from observed native activity. A changed proposal invalidates the trial's current
activity; its historical transcript remains available.

Once checked application source exists, choose **The built application** to use
its actual callbacks in a separate offline sandbox. Suggested sample jobs come
from the checked package and fill the request and sample data without submitting
it. The builder must reuse production domain code; fake external I/O stays
explicit. Callback observations and native peer activity are retained separately.
A completed conversation may still be awaiting confirmation or missing an adapter.
The main trial status separates native completion from failed or unavailable
application callbacks. The original native detail remains in its history.

An application with a custom UI can also expose `render_preview()` on its offline
rehearsal integration. It returns HTML rendered from the same application state
and production view code, rather than a separate demonstration. Forge refreshes
that view after sample setup and callback outcomes, including failed callbacks
that changed state. It appears beside the native chat and is bound to the exact
trial, checked source and latest recorded application observation. Earlier views
cannot appear as current after that state changes.

This preview is a static view: its controls are disabled, scripts and external
resources are removed, and an isolated frame blocks network access. Use the chat
to exercise callbacks. Missing or invalid preview support keeps the actual
callback results available without inventing an interface. A render timeout stops
the owned sandbox and reports its actual availability. Seeing this view does not
verify interactive UI behavior, login or deployment; those need destination
checks. Builder and independent reviewer guidance require reuse of the production
renderer and meaningful state-change counterexamples.

**Fix this trial** sends recorded host failures or unavailable production callbacks back to the builder; **Improve from
this trial** sends your feedback and bounded observations from any terminal
application trial. Both continue the exact current source and require fresh checks
and independent acceptance. The guide can read current trial statuses without
promoting them into acceptance or live readiness.
Feedback from an open conversation becomes available only after both the native
worker and application sandbox acknowledge cleanup. The guide receives bounded
excerpts from actual admitted turns, not a reconstructed transcript.
Repair feedback includes bounded excerpts of the original job and the last four
actual callback requests as untrusted observations. This lets the builder diagnose
missing context without guessing the job from an error. The builder also checks
that an agent can discover prepared records through its public callbacks, rather
than relying on identifiers known only to a test harness.
Missing-callback feedback includes bounded identity and tool names. The builder
assesses whether sample setup, intentionally absent fake I/O or unfinished offline
support explains the gap. General live-connection limitations do not trigger a
repair automatically.

The progress card's **Try the application** shortcut selects that checked application
and preserves your request and sample drafts. While an owned trial runs, its
activity remains reachable; unavailable or failed callbacks lead to review and
repair rather than a false completed trial.
The accepted builder result offers these same current Try/Run actions nearby,
using the progress card's existing source and activity checks.
Manual exports and terminal setup live in a closed optional disclosure. Installed
runtime references use the same pattern; required remote installation steps,
actual errors and lifecycle controls remain visible.

When you return to a setup, **Checking your saved build** means Forge is still
reading its saved work. If that read fails, **Build status is unavailable** does
not mean your work is gone. Choose **Review build status**, then **Refresh build
status** in the builder. Your unsent builder note stays in place; Refresh does not
submit it or start a build. Existing installed applications remain available
through their management and Console actions while the build status is checked.

## Install on this computer

For an independently accepted package targeting this host, **Run** opens the local
installation panel. Choose a project folder and supply private provider, connector
and application fields. An application's optional `application/configuration.json`
declares ordinary setup fields, not its architecture. Secret values have no source
defaults, do not enter model prompts or browser storage, and are written directly
to the owned destination's mode-600 `.env`. An open form keeps its unsent fields
in page memory when you ask the guide a clarification. Submission waits for fresh
checks of the same application. Selecting a different application or installation,
submitting setup, cancelling or leaving the page clears that private draft.
Using Forge's existing provider keys
requires explicit selection in the form.

For an explicitly selected Meta/Toolkit product package, **Install application**
verifies the selected runtime and leaves the application stopped for its own
setup. **Application setup is next** does not mean startup or interface readiness.
Saving private settings keeps that installation stopped, and ordinary Start
cannot substitute for application setup. The handoff checks the selected
installation before offering its next action. If **Prepare application setup**
is offered, choose it first and wait for confirmed preparation. It retains this
installation's private setup inputs and keeps the application stopped. Then use
**Set up and start application** when offered, or **Start installed
application** after a completed setup and clean stop. Missing required settings
offer **Review private settings**. **Check setup status** only reads the existing
state and never resends setup automatically.

After a completed setup and a clean stop, **Run in background** is a separate
optional choice when this installation supports an owned user service. It uses
the same completed setup. Its persistence description comes from this
computer's service manager; background mode alone does not promise operation
after logout or reboot. If the response is lost, check the same installation's
status before issuing another command.

That completed-setup background control has passed source review but is not yet
enabled in the served alpha. Its real service operation remains unproved. Keep
using the available interactive startup controls until the option is offered.

The interactive setup controls are locally served, and the backend includes the trusted
host wiring. The default launcher selects no Meta/Toolkit application host.
The matching qualified runtime, actual host selection and complete setup journey
remain unfinished in this developer alpha. If **Setup pending** remains, preserve
the same folder and saved settings. **Check setup status** only reads availability;
connecting the guide or saving private settings does not enable this setup path.
Ordinary supported packages retain their **Install and start** path.

Installation runs the fixed pinned installer, local/application checks and any
selected connection checks. Native readiness requires the owned application's
startup report, the expected identities and an available loopback Console. The
panel shows progress, redacted logs, checks, stop/retry and resume. A current-status
card points to the actual build, trial or installation, while saved design notes
remain labeled as earlier assessments. Native bootstrap
and process health prove availability; they do not certify a useful application
transaction or live integration. Those still need an actual destination trial.

**Ask the guide what is next** starts a real guide turn using the saved build,
trial and installation evidence. It keeps any unfinished message in the composer.
A status-only response preserves the agreed brief and plan, clears an obsolete
question and saves the operational advice without changing design readiness.
Edited requirements and designs still use the normal proposal path. A failed
response keeps its submitted request for explicit retry; reconnecting or reloading
does not submit that request again.

The guide's next actual turn can read verified installed source, the installed
build, update/recovery status and recent native health. An older installed build
remains distinct from a newer accepted application. Private settings, destination
paths and runtime logs do not enter this interview context. Reading operation
evidence does not rewrite the person's requested outcomes or certify their UI,
external channels or devices.
The guide also receives the owned installation phase and any safe startup health
category bound to the current attempt. Retry and source recovery clear previous
startup diagnoses. An unknown startup failure remains unknown; raw logs and
private configuration are not passed into the guide.

Local installation is offered only for a matching Mac/Linux host. Other targets
receive the exact independently accepted package, destination-specific deployment
steps and explicit remaining access checks. For Google Cloud, the guided panel
checks an existing signed-in account and project, explains the resources and
charges, and offers explicit computer preparation. It connects the verified VM
to private application setup and startup checks. This path has not completed a
real cloud deployment; a ready VM alone does not establish a working application.
Saved cloud preparation also offers **Review cloud removal**, even after the
proposal changes. It names the original VM, disk and SSH access rule and requires
you to acknowledge permanent disk-data loss. Save anything you need first; Forge
does not create a backup. The removal stays with that original request through
reloads and interrupted responses. It reports each resource separately and calls
removal complete only after all three are observed absent. Unconfirmed removal
keeps its Google Cloud management links and explains that charges may continue.
Stopping an application or closing Forge does not remove cloud resources.
Linux package instructions distinguish installing private settings, starting an
owned user service and opening a private SSH tunnel from the browser computer.

When Forge confirms that a remote settings request was never sent, your entered
values stay on the page. Use **Reload** to check the same installation, review
those values, then choose **Save** yourself. A lost or uncertain reply keeps the
original request and checks its status; Forge never resends it automatically.
A missing saved result does not mean the request was never delivered.

For an existing Linux computer, Forge normally uses this computer's default SSH
identities/agent and trusted host files, without reading SSH config. A technical
host can instead select private files for exact destinations before starting
Forge. Set `META_FORGE_SSH_PROFILES` to an absolute path to an owner-held JSON file:

```json
{
  "version": 1,
  "profiles": [
    {
      "endpoint": {"host": "workshop.example", "user": "volunteer", "ssh_port": 22},
      "identity_file": "/absolute/private/ssh-identity",
      "known_hosts_file": "/absolute/private/verified-known-hosts"
    }
  ]
}
```

The JSON and both selected files must be ordinary files owned by Forge's local
user, readable by that user, with no group/other permissions, hard links or
symlinks in their paths. Use an independently verified host-key entry for the
exact host and port; Forge does not acquire trust from an unchecked scan. The
identity must work noninteractively without an SSH agent or password prompt.
There can be at most 32 unique endpoint selections. Host, login and port must
match exactly; unspecified endpoints retain ordinary defaults. Browser forms
cannot select keys, files, commands or SSH options. The same selection applies
to installation, status/actions and the later private browser tunnel. Existing
Forge-owned GCP access continues through its separate IAP owner.

Keep these files private and outside exported application packages. Forge checks
them at loading and selected use, freezes their bytes for each connection, and
fails closed if the selected files or configuration change. Correct or rotate
them while Forge is stopped, then restart normally after active work has joined.
An invalid configured file never silently falls back to other SSH credentials.

Packages without service support explain the terminal lifecycle. Service status,
stop, restart and private reconfiguration are separate optional references;
background operation alone does not establish operation after logout or reboot.
Choose **Run in the background** for an owned macOS LaunchAgent or Linux user
service when available. That service survives closing Forge and can be recovered,
stopped or restarted from its saved ownership receipt. On macOS it is unavailable
while Forge's folder or the installation folder is in Desktop, Documents,
Downloads, iCloud Drive, a cloud storage folder or another volume, because macOS
does not let a background login item read them. A service installed there earlier
is not moved. Once it has reached ready there (macOS let it read them, for example
with Full Disk Access), or when an earlier Forge installed it, it restarts as before
and its status notes the folder; otherwise Forge refuses to restart it or refresh
its support, says why it is still starting, and can still stop it. The interactive
mode stays supervised by Forge and stops with its host. Both preserve installed
files and native history.

A stop waits for the application's native gateway shutdown. That is the horizon
its runtime reports at readiness (this repository's gateway horizon when it reports
none), plus the SDK's TERM and KILL graces and Forge's own margins. A new owned
service gets that stop timeout from the start, and an interactive run and Forge's
own waits and service commands use it at once. A background service runs the
supervisor that was saved when it was installed, so one installed before Forge
0.9.5 keeps its earlier stop budgets until **Apply support update** under
**Installation support** replaces that supervisor. After that, a Linux user
service keeps its frozen unit: when that unit's stop timeout is shorter than the
derived one (the unreported horizon's when its application reports none), the
service's own run adds an owned systemd drop-in, `forge-stop-budget.conf`, with
the derived stop timeout and reloads the user manager. Until that drop-in and the
reload succeed, a service run fits its application stop inside the stop timeout
its manager enforces (367 seconds under an earlier 397-second unit), so Forge can
still record its cleanup. A destination's supervisor comes with its package and
does the same for its own unit from the next package the destination runs; a
destination deployed from a package that Forge 0.9.4 accepted keeps that
release's helper and stop budgets until the package is rebuilt. A macOS
LaunchAgent keeps the stop timeout it was installed with (397 seconds from Forge
before 0.9.5): launchd has no drop-in, so a longer horizon that its application
reports later is not applied. Its application's stop is shortened to fit that
timeout (367 seconds, or 15 under the 45-second timeout of the earliest
releases), so native cleanup can be cut short. On Linux, logout and shutdown are bounded by the systemd
user manager's own stop timeout (120 seconds by default), whatever the unit sets;
the unit's stop timeout governs an explicit Stop or Restart.

For a supported stopped local installation, **Stage accepted update** compares
the exact independently accepted package with the installed source before changing
anything. Application-only updates can preserve the folder and its private state;
apply and explicit rollback require the selected update ID and owned native/Console
health before reporting completion. Once application code may execute, recovery
preserves all current native and domain records, including work admitted before the
first health response. Earlier snapshots remain evidence rather than automatically
rewinding recorded activity. Native definitions, session instructions, arbitrary
dependencies, protected host files and private setup-field changes need a supported
migration and stay blocked. **Retry current application** can recover the exact still-accepted
candidate from `recovery-needed`, retaining the latest native and application
records. It needs fresh native bootstrap and actual Console health before reporting
success. An arbitrary newer package or schema migration cannot replace this
transaction. An accepted local Reading application passed actual update, rollback,
failed activation/fallback and explicit current-source recovery while preserving
private settings, records and native history. That failure occurred before native
bootstrap. Recovery passed fresh native bootstrap, loopback Console health and
authenticated custom-app API readback. Completed recovery keeps earlier errors
under a history disclosure.

The historical qualified framework transition supports MobKit 0.8.42 to 0.8.46 with Meerkat
0.8.51 for stopped local interactive installations. Use **Rebuild with updated
runtime**, wait for independent acceptance, then stage the accepted update. The
transition requires unchanged identities, models, tool grants, Blueprint and native
mob configuration; only the exact SDK pin and current compiler-owned host files may
change. Application dependency pins remain exact. Background and cloud installations
are outside this transition.

Apply retains the previous verified runtime files, installs and verifies both
official native binaries and the SDK version, and runs MobKit's offline storage
inspection and required migration before startup. It preserves private settings and
current application/native records. Once runtime admission is possible, recovery
stays on the accepted new runtime; restoring 0.8.42 is unavailable. Stopping during
storage preparation waits for the operation already in progress before cleanup
is confirmed. Keep the same folder, reports and current records. **Retry current
application**, when offered, reconciles retained runtime files and native migration
reports using the exact accepted runtime after an interruption. A completed native
apply is not repeated. This source-supported
path still needs actual installed-upgrade and useful private-answer acceptance;
the existing Reading application evidence above does not establish that crossing.

The saved Forge setup team has a separate runtime transition. Before changing
its native executable link or deployment marker, startup takes the exclusive
project lease and retains the verified previous runtime authority. It uses the
same offline native storage migration before building the setup team with the
qualified current runtime. Direct 0.8.42 to 0.8.47 and 0.8.46 to 0.8.47
transitions retain any earlier 0.8.42 to 0.8.46 receipt unchanged. An interrupted transition resumes from its retained
receipt; unknown markers, executable overrides and unrelated links are refused.
This preserves the existing setup namespace and history. Setup-runtime source
checks and package preparation do not establish an actual saved-team upgrade.

For a background installation with a verified current supervisor, stop the owned
service and confirm native cleanup before applying or restoring an update. The
same transaction starts the operating-system service and requires a new service
instance, startup attempt, native bootstrap and authenticated Console health.
The frozen supervisor and service definition remain unchanged. Failed startup
must stop and clean up the attempted runtime before restoring source; unconfirmed
cleanup leaves an explicit recovery state. A manager acknowledgment or PID alone
cannot complete an update.

A retained MobKit 0.8 macOS Reading Companion journey passed background update, independence after
closing the wizard, explicit rollback and automatic previous-source recovery after a
controlled failed native startup. Normal authenticated conversations verified its
saved record and confirmation history after both restores. Private settings, all
prior Console frame rows and the frozen unit/host bytes stayed exact. The service
was stopped and disabled after that proof. Its receipts remain in the private
historical journey record. This does not qualify a current Meta 0.9 installation,
Linux updates, logout/reboot persistence or structural native migrations.

An owned stopped local background application can offer **Review update** under
**Installation support**. Reviewing freezes the exact current and proposed support
files without replacing the application. **Apply support update** changes only that
prepared support; private settings, application files and native history stay
preserved, and the application remains stopped. Do this before staging an
application update. A staged candidate must be resolved first, and agent-team or
cloud installations do not use this support path.

If the connection ends or Forge reloads, status checks observe the saved command
without repeating it. **Resume this update** explicitly continues the same pending
command. When support is already prepared, it uses that exact revision. A missing
required prepared revision or unverifiable custody stays blocked; Forge does not
substitute a different revision. Technical details show the saved
request and descriptor identities. Completing support maintenance is not evidence
of native startup, Console health or a useful application result.

Unverifiable support or service definitions remain preview-only.
**Preview update compatibility** compares source and explains why activation is
unsupported without changing the installation. Service-definition and cloud
migrations remain separate work.

Moving an installed application to Meta 0.9.4 changes its saved state for good.
Meta 0.9.4 keeps sessions and agent identities in Meerkat Core's own stores, so a
state directory that an earlier Meta release wrote needs one offline, one-way
migration (`meta_gateway storage-migrate-native`) before 0.9.4 starts on it.
Meta 0.9.4 refuses a directory that was not migrated
(`NativeStorageMigrationRequired`), and Meta 0.9.3 refuses a migrated one. Keep
the application stopped and back up the complete stopped installation first:
that backup is the only way back.
[Upgrading to Meta 0.9.4](https://docs.rkat.ai/meta/guides/upgrading-to-0-9-4)
lists the earlier releases the migration accepts, each refusal with its remedy,
and how to go back. Check that it lists your installation's release before you
update.

Forge does not run that migration. Each installed application keeps the Meta
release it was installed with. An update that would move it from an earlier
release to Meta 0.9.4 or later is staged as blocked and cannot be applied: the
update review names the migration it needs, and nothing in the installation
changes. A Forge copy that selects another Meta release does not reconnect an
installation made by a different one, so keep the Forge copy and state directory
that installed it. An application's native state is its project folder's
`.mobkit/state`; the agents' comms identity keys are under `.mobkit/home`. Back
up the complete stopped project folder, including `.mobkit/`, before any
migration.

Private setup names the required fields that are still missing and links to their
inputs. Rejected input does not claim a folder or select a queued installation.
Concurrent design/build changes are checked before admission; interrupted candidate
publication preserves the previous selected deployment and permits ordinary retry.

For an eligible stopped installation, **Change private settings** keeps its folder
and lifecycle mode. The form reads the exact verified installed package, so a newer
failed or unaccepted build does not hide the older application's settings. A
running builder or guide operation must finish before configuration can change.
Blank entries retain existing values. Explicit removal is available for saved
optional settings; required and automatic settings cannot be removed. An unchanged
save preserves the private file and lifecycle records exactly. Setting up another
folder starts a separate configuration. Configured-field hints and pending saves
are bound to the exact installed source and folder, including across multiple tabs.
The exported terminal interview shows each field's help, keeps configured values
on Enter, accepts `:remove` for optional values, asks again for missing required
values, and saves atomically. A cancelled interview preserves the previous settings.
Saving leaves the application stopped. Return to **Run** and use the action
offered for this installation: **Start**, **Set up and start application**, or
**Start installed application**. If **Setup pending** remains, keep the same
folder and saved settings until application setup is available. The v2
application uses the values saved by setup ahead of
older terminal exports, while still inheriting fields absent from the private file.

When the implementation agent builds a custom UI, its optional
`application/experience.json` can declare a label, absolute URL path and literal
loopback port or declared nonsecret port setting. The installation panel opens
these links only from verified installed source and a currently available native
runtime. The panel labels them as declared application UI: a link does not prove
reachability, login or a completed workflow. Missing or malformed metadata cannot
disable installation controls. Remote interfaces need explicit authenticated
tunnel mappings; Forge does not infer a public endpoint. For a selected local
knowledge service, ordinary installation attempts to start the knowledge service and
verify authenticated access after its Docker Compose, Git, source-access and
credential prerequisites pass. A Meta/Toolkit product installation stays stopped;
after saving private settings, **Prepare application setup** attempts this work.
Forge shows the result in the installation's activity and any reported error.
Actual local knowledge operation remains unaccepted in this alpha. Once running,
the knowledge service remains running on application stop and has its own
management CLI.

For an owned installation, **Use your application** shows the entrypoints and
first-task choices from that exact installed package. Optional choices come from
the installed application's `application/first-use.json`, authored around its
actual behavior and setup context. Choosing one only fills an editable local
draft. Missing or malformed suggestions leave free-text use available; offline
trial fixtures are never used as installed tasks. Open the checked application
entrypoint first. Preparing a request in Forge is optional; you can start directly
in the application. If **Continue with this task** is offered, it opens your task as an editable draft in
the application. Otherwise, choose **Copy task** and paste it into the application's
usual job controls. If copying fails, choose **Select task to copy** and copy
manually. Opening or copying the draft does not send the job.

Use the declared application UI when available, signing in through its own
account flow. If the application uses native chat for everyday work, open its
native Console, sign in if asked and select the named conversation owner. Submit
the job there and continue in the same conversation for replies and tool activity.
An absent custom UI does not establish native chat as the application's everyday
channel. Check its installed instructions for that route. The native Console also
provides authorized administration and inspection. Forge does not submit the
draft, bypass application or Console authentication, or collect its transcript.
A stopped installation retains the draft while you start it.
For a remote installation whose guidance remains unavailable after checking its
status, choose **Recheck first-job guidance**. This reads the installed-use details
again while retaining your task and problem-report drafts. It does not send a job;
the task handoff still requires fresh observation and verified installed source.
Fresh compiled packages open their conversation owner when no saved Console
layout exists. Restored layouts take precedence. Earlier accepted packages remain
unchanged; changing an installed opening default requires a separately checked
update. Updates can change the three Console layout defaults (initial agent,
control and preset) while every other Console setting and its file mode match.
Authentication, permissions and native configuration changes still require a
supported migration.

After trying the application, open **Report a problem or request an improvement**
for a local installation, or **Report what happened or request an improvement**
for a remote installation. Choose what happened and describe an improvement.
Progress and errors remain visible when this optional form is closed.
The builder receives this as human-reported feedback bound to the installed
package and the selected checked candidate. If those versions differ, explicitly
acknowledge that the builder will improve the newer candidate. Source changes
receive fresh independent acceptance; installed files, private settings and
application records remain unchanged until a separate supported update or
installation. Neither a successful chat nor a reported outcome marks the full
application verified.

### Return to your saved application

Use the same saved installation when you come back. If **Retry installation** is
offered, use it to finish installing the saved checked package in the same folder.
Ordinary Start and private settings remain unavailable until its files are
verified. Otherwise, choose **Start installed application** (or **Start installed
agent team**) when offered for a stopped installation and wait for the startup
checks before opening its interfaces. Open the current links in that installation's
panel; an older
exported port or browser bookmark may differ. Sign in again if asked. Use the
application's own UI for everyday work when one is supplied, or the named
conversation owner for an agent-team installation. Creating another installation
does not resume this one's history.

**Stop application** (or **Stop agent team**) keeps the installed files, private
settings and saved state. Starting it again resumes the same configured identities. Closing
Forge stops an interactively supervised application; a background installation
continues under its owned service. Stopping the application does not stop a
separate knowledge service or remove billed cloud resources.

After a failed interactive or background start, choose **Stop and verify cleanup**
when offered before correcting settings. **Change private settings** becomes
available only after a fresh check confirms the installation is stopped and its
owned cleanup is complete. Save the correction, then explicitly start that same
installed application or team; cleanup does not restart it or resend a task.
Unconfirmed cleanup or unknown ownership keeps the recovery control unavailable.
Keep the same folder, review the displayed reason and use **Check installation
status** before continuing.

**Sign out** in the password-protected Console ends your current Console login
without stopping the application or deleting its saved state. Successful sign-out
returns to the password screen. If that screen does not appear, treat sign-out
as unconfirmed; closing the tab is not confirmation.

### Set up an installed application's channels

For an application built with the matching native Toolkit runtime, use
**Connect setup** in its installed Console. This panel belongs to the selected
`ToolkitApplicationSelection` and supports the capabilities supplied by that
application. Ask what is supported before providing service details, and enter
credentials only in the protected forms.

Return to the same application's Console to continue setup, including when it
runs in the background. This requires an installation that already supports the
panel and a current authorized login. Being installed or running does not itself
add setup support. For an older application without that capability, return to
its saved setup in Forge and review a compatible update; there is no automatic
setup retrofit or separate panel for arbitrary installed applications.

**Set up your app** is a separate, optional generic setup conversation. It uses
a protocol 1 subprocess selected by `MOBKIT_CONNECT_BIN`. This variable is an
operator-only advanced legacy override: the installation owner must supply and
qualify the executable for that application and its current login. Normal
**Connect setup** uses the matching selected Toolkit product and runtime.
Changing the variable does not install native setup support, authenticate the
user or add new application capabilities.

Closing the panel detaches the view and leaves the application running. Reopen
it to check the original work before sending another request after an uncertain
result. A proposal, saved configuration, running adapter and successful channel
reply are separate outcomes. Confirm the final behavior by sending a permitted
message through the configured channel and receiving its actual reply.

Saved application results, conversation history and native memory are distinct.
A saved book or task belongs to the application's own records. An earlier chat
reply belongs to conversation history. An agent's native memory tool stores and
recalls records in that identity's memory scope. Verify the actual saved result
or tool response before relying on it; a reply saying "I will remember" does not
establish a successful native memory write or later recall.

If a native write reports **quarantined**, the record can survive restart while
remaining unavailable to ordinary recall. This can happen after the continuing
session uses a tool whose output is configured as untrusted. In the retained
MobKit 0.8.47 package, the Console quarantine queue is read-only and Forge does
not supply a separate review action. Starting again does not release that record.
Report the failed recall through **Report a problem or request an improvement**;
include the record ID and the reported reason if available, without credentials
or unrelated private data. Keep the original installation and history for a
supported repair. This describes the retained 0.8 example. For another package,
use the capabilities advertised by its own installed native Console; the old
private journey record does not establish its review support.

## Adjust the details

- **Agent classes:** reusable roles, instructions, provider/model, instance count,
  native memory, builtin tools, shell, WorkGraph, scheduling, and application tools.
  Classes compile into exact instance profiles and a durable identity roster. Named
  members give each person, domain, or entity a stable key and individual instructions.
- **Tool grants:** workspace access, no/read/write SQLite notes access, knowledge
  access, and MCP connections with optional exact tool allowlists. Every fresh or
  resumed session receives its class's grants. Unassigned MCPs are not started.
- **MCP discovery:** live search of the official MCP Registry, preserving publisher,
  repository, version, transport, and authentication metadata. Configure stdio or
  Streamable HTTP connections, or import standard `mcpServers` JSON. The workbench
  converts supplied secret values to local environment placeholders. The guide reads
  publisher tool documentation before assigning exact capabilities. Optional and
  nonsecret setup inputs retain their meaning. The exported connection check
  verifies actual tool names against the installed authenticated server.
- **Topology:** hub, mesh, pipeline, or manually selected identity edges. The graph
  previews the draft. Exported deployments apply those edges through the native
  identity topology provider.
- **Workflows:** exact identity targets, prompts, dependencies, parallel branches,
  and timeouts. Validated DAGs compile into native Meerkat flow definitions. The
  native engine owns dispatch, predecessor outputs, parameters, and run outcomes.
- **Knowledge:** connect an existing knowledge service `/mcp/agent` endpoint or export an
  isolated local knowledge service deployment with embedded persistent storage, scoped JWT
  credentials, ingestion helpers, and verification commands.
- **Deployments:** macOS, Linux user services, and GCP VM scripts with IAP access.
  Inspect every file before downloading the executable package.

The current compiler accepts up to 32 classes, 512 members per class and 4,096 identities,
32 MCP connections, and 32 flows (up to 1,024 steps per flow). These are validation bounds, not performance guarantees.

## Work with the setup team

The conversation uses the native MobKit ConsoleComposer, ConversationTranscript
and console timeline adapter. Submitted text and selections clear when the saved
operation is admitted, preserving any newer draft. Native tool calls/results,
streaming prose and provider reasoning summaries appear inside the conversation.
Reasoning summaries are shown when the provider emits them. The setup-scoped
timeline retains native frame versions and replay semantics across reloads;
the optional detailed editor uses the console workbench components. The backend
is a durable four-agent MobKit team: deployment architect, integration specialist,
systems reviewer, and visualization programmer. Open its native console to inspect real identities,
conversations, tools, and memory. Setup, builder and trial Console links use the
Forge origin, so they also work when Forge runs on a remote computer behind an
SSH tunnel or an authenticated host wrapper. Forge serves the existing native
Console assets and forwards its requests only while it owns that live runtime.
An ended or replaced runtime closes the link and its live streams; the saved
conversation and build history remain available in Forge. The host wrapper must
protect the whole Forge origin, including `/api/owned-console/`. These links do
not change the installed application's separate Console access configuration.
The four-role setup runtime keeps its native
state in the Forge state directory: `architect-runtime-meta-v1` for a guide on a
Meta release before 0.9.4, and `architect-runtime-meta-v2` from Meta 0.9.4 on.
Meta 0.9.4 refuses native state that an earlier release wrote until a one-time,
offline migration has converted it, so Forge starts the setup team in that fresh
generation instead of opening the previous one. Earlier generations, including
`architect-runtime-v2` and `architect-runtime`, stay in place unchanged, and a
previous Forge copy that shares the state directory keeps using its own. The new
generation takes over the saved provider and compaction settings. Saved wizard
conversations and proposals keep their existing store.

The fresh setup team does not recall the earlier team's native conversations or
memory. To carry them over, stop Forge before its first start on Meta 0.9.4, copy
the complete `architect-runtime-meta-v1` folder to `architect-runtime-meta-v2` in
the same state directory, and run Meta 0.9.4's
`meta_gateway storage-migrate-native --state-dir architect-runtime-meta-v2/.mobkit/state`
there. If the census is clean, run the same command with `--apply`, then start
Forge. Run it on the same computer as the same user: the agents' comms identity
keys are kept in that user's home directory, not in the state directory. If the
census refuses, remove only the copied `architect-runtime-meta-v2` folder; Forge
then starts a fresh generation. See
[Upgrading to Meta 0.9.4](https://docs.rkat.ai/meta/guides/upgrading-to-0-9-4).

The team can inspect the current blueprint/schema, search the live registry,
read a maintained MobKit design reference grounded in a home-automation host and a
console application, inspect
publisher tool evidence, validate a complete design, and publish a proposal. Only
the architect publishes plan and interview changes; its peers provide specialist help. Applying that proposal changes
the design. It does not deploy infrastructure. New executable MCP configurations
proposed by the agent must match registry evidence or existing user input.

Guided interviews and project drafts have revision-checked server APIs under
`installer/.studio`; native agent sessions remain in MobKit's own state. Set
`META_FORGE_HOME` to select another local state directory; `MOBKIT_LAUNCH_HOME`
remains compatible. This is a single-user local application, not a multi-tenant
public server.

## Run an exported project

The wizard can also try checked application source before installation. Choose
the agent design to explore native collaboration, or the built application to run
its actual callbacks with isolated sample data. The optional ordinary Python
`rehearse(context, fixtures)` hook reuses production callbacks and replaces only
external I/O. Trials freeze the exact checked source and record actual native
activity and callback observations; they do not establish independent acceptance
or live integration readiness. Missing trial callbacks are reported before paid
native agent work starts. Application-code isolation currently requires macOS's
available sandbox; there is no unsandboxed Linux fallback.

For requested offline trials, the implementation agent prepares reusable trial
support and useful sample jobs. Optional `application/trial.json` contains
`{"cases":[{"label":"A plain-language job","request":"A realistic request","fixtures":{}}]}`.
The wizard offers these source-bound choices and keeps a custom request and
advanced JSON override available. This optional metadata contains 1-12 cases in
at most 100 KB, visible labels up to 160 characters, requests up to 6000 characters
and finite JSON fixture objects. It describes authored sample data, not the
application's architecture or evidence that any trial succeeded. Unsupported
offline dependencies stay explicit; a trial cannot invent a live connection.

An accepted local installation offers an explicit background option: an owned
macOS LaunchAgent or Linux systemd user service. It can survive closing Forge,
and a new Forge process verifies and reconnects to its exact service. macOS runs
after login and stops at logout. On existing Linux hosts, logout behavior depends
on configured user lingering. Guided GCP preparation enables lingering for its
installation user. The trusted supervisor source is privately snapshotted, while
its explicitly admitted Python environment must remain installed.
Interactive supervision remains the default. Stop and restart use only the exact
owned service definition, with native readiness checked separately.

Extract the ZIP into a new folder such as `~/src/my-system`. Open the folder
containing `install.sh` and `start.sh`, including any inner project folder, then
open a terminal in that folder and run:

```bash
bash install.sh && \
bash start.sh --doctor && \
bash start.sh --connections-check && \
bash start.sh
```

If installation stops, follow its printed recovery steps and retry from the same
folder. Continue with the checks and startup only after installation succeeds.

The local installer asks for credentials. They stay in a private `.env` on the
destination. Current generated packages also passed isolated installation on an
existing Linux host, including official binary checksums, cache reuse and an actual
empty-roster native runtime/Console prerequisite check. This does not establish
the desired agents, application behavior, user service or connectors on Linux.
The connection check performs actual MCP handshakes and verifies
selected tool names. It starts configured third-party servers but sends no AI
requests. The normal runtime opens the native MobKit console.

```bash
bash start.sh --list-flows
bash start.sh --run-flow briefing --params '{"topic":"Your research question"}'
```

Stop the normal process before these commands, since each acquires the deployment
lock. `--smoke` makes a paid model request. Workflow execution may use models and
connected tools according to class grants.

A new project initializes durable storage through MobKit's own offline maintenance
tool. Imported legacy state requires explicit migration after backup. The complete
`.mobkit/` directory belongs in stopped-deployment backups.
A deployment that moves to Meta 0.9.4 migrates that state once, offline and
one-way, with `meta_gateway storage-migrate-native` while it is stopped and after
such a backup; Meta 0.9.4 refuses state that was not migrated
(`NativeStorageMigrationRequired`). See
[Upgrading to Meta 0.9.4](https://docs.rkat.ai/meta/guides/upgrading-to-0-9-4).

For Google Cloud, review `bash deploy/deploy-gcp.sh --dry-run`, then run the script.
It requests the exact project ID before provisioning billable resources. Enter
credentials on the VM, install selected MCP prerequisites, and use the generated
IAP tunnel. A private deployment receipt supports `--resume` after upload or install
failure, and verifies the VM belongs to this deployment. Recovery and cleanup
commands are printed before provisioning. The wizard's guided cloud panel offers
computer preparation and application setup as an alternative to these commands.
For a new guided Debian 12 VM, preparation also installs pinned Node 22 with
npm/npx or pinned uv/uvx when those commands are granted by the exact accepted
package. Official archives or wheels are checked against fixed SHA-256 hashes
before installation. This does not install or authenticate the selected MCP
servers; their real connection checks still run after private setup. Unknown
commands and the local knowledge service's Docker, Compose and Git prerequisites remain
explicit destination checks. Existing cloud requests retain their original
bootstrap script and do not silently acquire new guest dependencies on resume.

The generated README includes native definition activation, background services,
backup, connector prerequisites, and the selected knowledge setup. Changes to an
existing canonical mob definition use the native revision-pinned activation flow.
Instruction-only updates retain the same identities, grants, models and flow
structure. Continuing sessions require the current checked host and a gateway
that advertises host instruction support for both `send` and `dispatch`.

Subsequent new ordinary host sends or keyed dispatches can carry revised canonical
profile instructions into continuing sessions while retaining their history.
Activation or bootstrap alone does not adopt those instructions, and retries with
the same key retain their original admission. Console, native flows, schedules and
internal peer turns remain outside this instruction carrier. A gateway without
the required capability cannot perform a continuing instruction update. The
accepted Reading upgrade preserved unchanged roles; it does not establish
changed-role adoption.

### Hosted MCP sign-in on the selected Console

The trusted Python launcher can supply `hosted_mcp_redirect_base` and a tuple
of configured `meerkat_toolkit.mcp_auth.McpTarget` objects as
`hosted_mcp_targets`. Fresh, restored and replacement application startup owners
accept these inputs. `BootstrapFactory`, `InstalledBootstrapFactory` and
`run_child` carry them for fresh and restored Toolkit startup. Ordinary Meta
startup does not supply the native hosted-auth endpoint. Omission preserves the
existing local Console. These inputs are not blueprint fields, environment
overrides, agent tools or request JSON; the trusted launcher must select them
again when starting an installed host.

The base is passed unchanged to the Connect runtime constructor and its native
startup option, outside model configuration. Its HTTPS origin selects the whole
authenticated Console mount. The listener remains on loopback at the normal
application port; deployment owns the TLS ingress, HSTS and exact routing.
Supplying the input does not provision a route, register a provider client or
prove that an existing public origin reaches this application.

Authenticated humans open `/console/forge/mcp` to sign in to the selected
services. The host issues aliases for its configured targets. A target is a
locator, not permission: native resolves it against the actual configured
realm, server and account. The host first describes it over the original
enrolled private connection, mounts Core's exact redirect URI, and then uses
Connect's `HostedMcpAuthBinding` to begin and present the login. Each target has
one retained attempt per Console login. Repeated requests reuse that attempt;
they do not retry an uncertain start. A joined native attempt does not disclose
another caller's authorization URL.

The callback must have an ASCII path that the Console can match byte for byte.
Paths with percent escapes, non-ASCII characters, dot segments or a reserved
Console route are refused before native sign-in starts. Forge does not normalize
or rewrite Core's registered redirect URI.

Callback GET serves the SDK's fixed, uncached landing without inspecting a login
or forwarding the query. Its bounded same-origin POST requires the original
live Console login and binding. Callback receipt, credential observation,
connection readiness and permission to invoke a tool are separate outcomes.
The host retains attempts across cancelled HTTP waiters and closes them before
retiring their enrollment on logout or shutdown. It never closes the runtime's
shared private client. Provider callbacks and authorization URLs are excluded
from logs and ordinary Console/model context.

A failed or timed-out start can leave native admission uncertain. A later
request reuses the retained attempt; it does not start a replacement. Logout
revokes local Console access before waiting for cleanup. If cleanup cannot be
confirmed, logout returns HTTP 503 instead of the normal redirect to `/login`.
Native login retirement is still attempted, but its acknowledgment does not
erase an earlier unconfirmed MCP cleanup outcome.

The source tests exercise the actual SDK over anonymous test pipes and ASGI.
They do not establish a real TLS route, provider login, native configuration
resolution, installed persistence or cold restore. Those require the reviewed
matching Core/Meta/Connect artifacts and the deployment owner's selected route.
See [hosted MCP test setup and failure checks](tests/README.md) for the focused
suite, required SDK selection and checks against misleading test passes.

### Explicit application tool selection

A generated application can retain `application/tool-policy.json` alongside its
code. This is source intent for a fixed durable roster. Each identity has an
explicit list of exact tool names, including an empty list when it should receive
no tools. The file contains `provider_id`, `policy_id`, a positive u64 `revision`,
and `members` rows with `member_identity` and `grants`. Each grant contains
`tool_name`, `action: "invoke"` and an application-owned `consequence` from
`r0` through `r3`.

The application checker validates this source and matches names against actual
callbacks per identity. Native-only names remain pending native surface
validation. R0-R3 describes consequences for native observability; it does not
approve an operation or declare a tool read-only. Neither source validation nor
callback availability establishes execution permission.

The selected native preparation path requires explicit policy source before
application loading and connector/state creation, and refuses changed or
malformed source. For the compiler-selected helperless route,
`application/native-tool-policy.json` covers exactly the compiled seed identities;
future members receive grants through native class admission and governed Add.
This route supports dynamic roster hooks. Legacy packages may omit
`application/tool-policy.json`; when supplied, that fixed-roster policy remains
incompatible with dynamic roster hooks. The trusted compiler can join original
source to a retained native inventory from its same caller, preserving the
command, process and source pins. It maps only returned native member bindings and hashes the
original source bytes. The installation owner must retain private exact-byte
copies for this caller while checking the original accepted source separately.
Canonical compilation and native provider binding remain required. This trusted
host observation is not a native principal or approval; source validation alone
does not establish a working native deployment.

Dynamic-roster installations prepared before installation-owned Console domains
were introduced must be installed fresh when moving to a Toolkit that requires
that domain pair. The new preparation selects both `authentication_domain` and
`event_domain`, including when there are no initial members. Restoring an older
preparation preserves its original bytes and refuses member-class Add when the
pair is absent. Updating source or SDK files alone does not upgrade it, and
dynamic rosters cannot be updated in place. Static-roster installations do not
use member-class Add and are unaffected by this requirement.

The trusted preparation draft can explicitly select Toolkit's
`application.helper_tools = "checked_installation"` composition. The native
driver requires the application's real terminal or Console review presenter;
the existing exact software review remains the installation approval. Forge
preserves that selection and does not infer a presenter or issue tool grants.
The coherent native package and its fresh/restart checks are still required.

## Architecture and development

Meerkat owns native agent, session, communication, and workflow semantics. MobKit
provides the SDK, durable integration, gateway, and console. Forge owns desired
deployment blueprints, reviewable proposals, compilation, and packaging.

The React frontend imports the repository's actual `ConversationPane`,
`ConsoleComposer`, and rich message renderer. Optional advanced controls also use
`ConsoleWorkbench` and `TopologyPanel`. These workspace-private components are
bundled into Forge, not presented as published npm dependencies.

`blueprint.py` validates v2 design intent; `compiler.py` produces executable
packages; `connector_discovery.py` translates live registry metadata;
`elephant_setup.py` produces source-pinned knowledge deployment files;
`architect.py` hosts the setup mob; `guide.py` coordinates the native interview;
`guide_store.py` and `projects.py` persist interview and draft revisions;
`guide_operations.py` tracks background responses; `design_knowledge.py` maintains
source-derived design guidance and compiler limits; and
`server.py` exposes the local APIs. The v1 compiler and endpoints remain
available for previously generated simple projects.

For a host-managed Meta/Toolkit application, the operator can explicitly set
`META_FORGE_APPLICATION_HOST_FACTORY` to an importable `module:factory` before
running `start.sh`. That synchronous, zero-argument factory returns either an
exact `ForgeApplicationHost` retaining its selected application source and private
Fresh/Restore inputs, an exact `MetaToolkitRuntimeTarget`, or an exact
`ApplicationRuntimeSelection` containing the original local target and a separate
Linux `ProductControlSelection`. The pair lets one registry select the local or
Linux/GCP source from each deliberate Build's checked Blueprint. It does not let
browser requests choose artifact paths, control dependencies or the guest ABI.
A Linux local launcher keeps its original local target; SSH installation requires
that accepted target to match the selected Linux control tuple. A later design
gets its own source and host, while the accepted installation keeps its original
configuration and setup history. Reconnection reads the installed package's
original Blueprint, even when the current guide selects a different destination.

Application model custody that the launcher configured for the process with
`launchkit.packaged_runtime.configure_application_model_custody` also reaches
this entry. A bare `MetaToolkitRuntimeTarget`, and an ordinary selection without
its own model owner, take it. A selection whose owner has the same selection
keeps its own. Any other selection stops startup, an application host included,
so configured custody is never dropped.

Forge selects the host or registry through the existing server hook before
serving. An invalid selection stops startup; the launcher does not prepare, install
or start an application. Without the setting, the normal launcher remains unchanged.
This developer hook does not supply a qualified runtime or authenticate a user.
The per-build integration and reconstruction of the original installed local
application owner have focused source verification with inert external effects.
The frontend offers Continue setup to request deliberate reconnection after Forge
restarts; read-only status does not reconstruct an owner. Recovery retains the
original accepted design when the current guide has changed. The source tests
cover reconnection before setup, retained prepared Fresh, and completed setup with
Restore, using inert builder, process, inventory and SDK boundaries. Recovery
checks the retained plan against the original application declarations without
replaying native preparation. Qualified native setup and a complete useful
installed journey remain unproven.

```bash
installer/.venv/bin/python -m pip install -e 'installer[test]'
installer/.venv/bin/python -m pytest installer/tests -q
npm --prefix installer/web ci
npm --prefix installer/web run check
npm --prefix installer/web run build
```

Current source results are in [Forge CI](https://github.com/lukacf/meerkat-meta/actions/workflows/forge.yml).
Use the [first-job walkthrough](GETTING_STARTED.md#5-complete-one-useful-job) to
check useful installed work separately from source tests.
Registry metadata is evidence about configuration, not a certification of a
publisher or successful authentication. OAuth-only services need an authenticated
bridge or supported client setup. Local knowledge service source builds need Docker,
repository access, and the configured pipeline provider/embedding credentials.

## Current scope

Generated applications support static durable identities, native flows and Console,
scoped tools, local notes and an optional knowledge service. Installation depends on the
selected runtime and destination. The source includes Mac/Linux service controls
and SSH/GCP transport paths. A prepared Meta preview names its supported platform
in its generated manifest and README; remote startup needs its own original native
setup owner and verified destination. A local package does not select those inputs.
Authenticated end-user apps, inbound messaging adapters, automatic recurring
triggers, data-driven rosters, ephemeral worker fleets, and synchronized ingestion
are application extensions that the implementation agent can build as ordinary
code. Unknown external contracts and destination verification stay visible in the
checked package; code generation does not mark the guide's open requirements as
installed. Operator-led
knowledge service ingestion is available through the generated knowledge helper.

## Design reference

The conversation layout and warm visual direction were informed by the supplied
`MobKit Installer Wizard Design.zip` prototype. Its provided Meerkat illustration
is included in `assets/meerkat-transparent.png`. Plan content, connection details,
and package generation come from the actual installer services.

## Starting again after an incomplete build

For a stopped incomplete build on a host-selected Meta/Connect runtime,
**Runtime and host updates > Start a fresh build** starts generation again from
the current accepted setup using the host's current runtime. It requires the
exact saved build, current source digest and confirmed worker cleanup. The prior
source and history remain in their original build; authored code, repair notes,
private configuration, checks and acceptance are not copied. The new build must
pass its own checks and independent acceptance before installation. This action
does not change a running or installed application. The accepted-design runtime
refresh and ordinary repair actions keep their existing behavior.

## Retained 0.8 runtime refresh

For the retained 0.8 generation, a runtime-only refresh preserves the accepted
Blueprint, native roles and grants,
application source, console settings and connector configuration. It rebuilds
the protected host framework for the qualified current runtime; within retained
design and dependency files, only the MobKit release pins change. It runs new checks
and independent acceptance before the existing Stage and Apply controls become
available. No implementation author rewrites the application.

The retained qualified local crossings are 0.8.42 to 0.8.47 and 0.8.46 to 0.8.47, both
with Meerkat 0.8.51. Background activation requires the exact owned, stopped
service, confirmed native cleanup and its preserved startup protocol. Its frozen
service definition is retained. After native admission, a failure retains the
candidate and supports forward recovery; it does not restart the old runtime.
Previously completed 0.8.42 to 0.8.46 migrations keep their checked archives and
retained completion authority. A later compiler does not rewrite that history.
