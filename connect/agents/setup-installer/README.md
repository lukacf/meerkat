# Prepare and install an agent-selected integration

This instruction package lets a setup agent continue from a user's request or
research proposal to concrete package inputs and the existing selected-package
installer. The agent researches and chooses the implementation, writes the
package/activation documents and prepares exact dependency inputs. Standard pip
or npm resolves package dependencies. No program chooses an integration from
the user's wording.

Start with [Host setup from a new workspace](HOST-SETUP.md). Its combined-library
path gives one setup agent both adopted integrations and outside-catalog Python
and Node acquisition. The current combined macOS archive also supports
optional MCPB manifest/source inspection before configuration and installation,
and container acquisition/execution through separately scoped Compose profiles.
Its Linux counterpart preserves the existing Python/Node paths without MCPB or
a native Compose planner.
The optional `setup/bootstrap_host.py` entry point prepares the local setup
client and host configuration in one command from the selected archive and a
trusted Python, without requiring a preexisting MCP environment. Its archived
`HOST-BOOTSTRAP.md` contract keeps family/acquisition choices explicit and
preserves partial output after failure. It does not start an agent or sign in.
The separately pinned
[macOS arm64 kit](../../dist/agent-install-kit-20261004-acquisition-recovery/README.md)
retains its earlier supported MCPB and Compose profiles, with its own contracts
and platform limits. Follow the selected archive's extracted guide; no Toolkit
checkout is needed. The host supplies compatible runtimes and permitted research
tools. Ordinary [native MCP attachment](../../docs/site/guides/native-mcp.mdx)
is verified on released 0.8.50 and remains an explicit host step.

First-use discovery and installation do not require saved recipes or a recipe
database. Optional retention has its own host configuration; the included recipe
command can initialize an empty store. The current kit's
[verification record](../../docs/verification/20261004-acquisition-recovery/REPORT.md)
checks its files and acquisition browser behavior; setup MCP startup is
inherited from unchanged predecessor payloads. Earlier trials below retain their own
edition and scenario boundaries.

Supply [instructions.md](instructions.md), the complete original request and a
host execution profile. The profile must identify the actual permitted tools,
trusted executables, runtime/platform, staging and installation locations,
network/file access, input limits, host-owned configuration/policy references,
lifecycle owners and the effect-admission procedure. These are host facts, not
fields the model can invent. Do not supply a preselected implementation when
evaluating discovery.

When the host enables `setup_search_node`, use it to investigate public npm
packages before choosing an exact name and version. You choose the query and
candidate. Descriptions and registry ordering are untrusted research evidence;
inspect the acquired source before selecting installation metadata. Search does
not install anything or grant acquisition permission. The catalog remains a
starting point: host-provided browser, shell and other research tools can reach
sources outside this registry. Use shell only within the host's explicit scope.

This package does not register tools or grant permissions. It uses the host's
existing browser/shell/file tools and these existing Toolkit commands:

- `toolkit-package-inspect integration-package` validates package metadata and
  returns its canonical digest through `IntegrationPackage::content_digest`.
- `toolkit-python-plan` and `toolkit-node-plan` validate complete concrete
  selections through the existing activation owner.
- The selected Python/Node modes of `install-library-consumer.py`, or
  `toolkit-mcpb-install`, realize supported selections and emit ordinary MCP
  configuration with their existing software observations.
- `toolkit-mcp-check.py` runs through the selected Python environment to compare
  selected schemas and make the host-authorized calls once. Its separate
  discovery-only mode returns SDK-visible descriptions and schemas before the
  agent chooses any call. See the
  [execution check guide](../../docs/guides/mcp-execution-check.md).

Current Python installation supports the exact profile described in
[PYTHON-INSTALL.md](../../crates/toolkit-package/PYTHON-INSTALL.md). The
[Node](../../crates/toolkit-package/NODE-INSTALL.md) and
[MCPB](../../crates/toolkit-package/MCPB-INSTALL.md) contracts have different
constraints. An unsupported package or platform needs a different accepted
path; the agent must not disable a validator to make it install.

The [research package](../setup-lead/README.md) remains useful when only research
tools are available. The [maintained-library MCP tools](../../sdk/consumer-setup/MCP.md)
offer a shorter route for already adapted profiles. This instruction package
addresses the missing preparation work when the candidate is outside that
library. Native `find_and_install` delegation, protected authentication,
requester-bound attachment and durable continuation remain unfinished
integration work. A local software receipt cannot establish those outcomes.

For an interrupted selected Python/Node install response, the standalone kit
includes `--verify-existing` in the same installer. The agent supplies the original
selection; the verifier runs the trusted metadata validator and compares files
without repairing or relaunching the integration. A matching result still needs
the host's account, runtime and readiness checks before use.

A fresh-agent local trial is retained in
[`20261003-agent-prepared-install`](../../docs/verification/20261003-agent-prepared-install/REVIEW.md).
It starts from a vague clock-tool request with no host-selected package or
prepared dependency inputs. Keep its bounded installation and MCP observations
separate from native acquisition and original-task continuation.

The [standalone kit trial](../../docs/verification/20261003-agent-install-kit/REVIEW.md)
uses that prior agent's frozen Time choice. A fresh agent derives the installation
commands from the extracted guide, installs once and reconciles successfully;
the host then passes five MCP checks. It establishes kit delivery and usability,
not a second discovery trial or native task continuation.

The [MCPB kit delivery trial](../../docs/verification/20261003-agent-kit-mcpb/REVIEW.md)
uses the existing exact installer and a private retained server fixture. The
extracted kit passes installation, explicit verification and three ordinary MCP
calls. MCPB `verify` probes the trusted Node executable; it is distinct from
Python/Node `--verify-existing`. The kit contains no donor server or host runtime.


The kit's optional [selected-package setup MCP tools](../../sdk/consumer-setup/SELECTED.md)
expose check/install through the existing controller. The agent prepares the
same canonical inputs in the host-fixed staging root, then uses the returned
validation ID and effect digest. The host supplies standing local installation
scope; neither manifest declarations nor a validation result confer grants.

The current kit preserves this path for supported self-contained MCPB bundles as
well as Python and Node. The [MCPB delivery trial](../../docs/verification/20261004-mcpb-setup-distribution/REPORT.md)
uses a preselected real bundle and proves installation, an ordinary tool call,
same-job replay and fresh-process readback with repository/network access
denied. Supply the four documented MCPB inputs and a host-pinned optimized
installer; allow its 120-second check/readback budget in the client.

The [predecessor kit's extracted trial](../../docs/verification/20261004-recipe-kit-delivery/assembly/REPORT.md)
adds actual Python installation, explicit recipe retention, immutable replay
and scoped recall. It uses an existing initialized store. The newer source
`toolkit-local-recipes init` command is not included in that frozen archive.

The current kit adds [explicit local setup stop](../../docs/verification/20261004-stop-setup/REPORT.md)
through the browser and `setup_stop`. It preserves interrupted files and exact
job replay. Requested stop is distinct from settled worker state, and settling
the direct child does not establish that all descendants exited. The original
business task and account connections are unaffected by this local action.


The [hosted-recipe kit verification](../../docs/verification/20261004-hosted-kit/REPORT.md)
covers empty-store initialization, actionable recipe errors, saved hosted MCP
metadata, portable export and exact replay through the delivered tools.
Explicit preparation retry is also exercised through a configured services
installation. The test uses inert endpoint metadata and no provider calls.
Protected authentication, native attachment and original-agent continuation
remain unfinished.

The current kit also includes optional artifact acquisition through the same
setup controller. Follow the [host guide](HOST-SETUP.md#optional-let-the-agent-acquire-its-selected-artifacts)
and the extracted `contracts/PYTHON-ACQUISITION.md`, `NODE-ACQUISITION.md` or
`COMPOSE-ACQUISITION.md`. The host enables and pins trusted pip, the complete npm
distribution, or its selected container engine; the agent chooses exact packages
or immutable images. Python and Node artifacts enter the existing selected
installer. Container acquisition returns a proposal that still needs separate
host execution admission. The kit supplies no package-manager tool distribution,
host runtime, provider login or native authority.

When promoting a new kit, update the host guide and the installation entry
points together. With the advertised archive available locally, run
`python3 scripts/check-installation-guide.py` from the repository root to check
their edition, archive checksum, basic profile pins and referenced kit files.
This consistency check does not replace a fresh consumer following the guide.
