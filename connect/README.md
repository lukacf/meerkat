# Meerkat Connect

Give your agents the tools they need. Meerkat Connect combines more than 100
adopted OSS integrations with agent-led discovery and installation beyond the
catalog. Describe a task, review the proposed access and let Connect guide setup.

## Install and use

Connect 0.9.10 is released for Linux x86_64 with glibc 2.39. Its complete Apple
Silicon macOS archive (macOS 13.5 or later) is not built yet and follows later;
the release workflow attaches the [macOS executables](#macos-executables) on
their own.
Install [Meerkat Core 0.9.10](https://github.com/lukacf/meerkat/releases/tag/v0.9.10)
for the ordinary `rkat` agent. Connect supplies its own Python, Node/npm,
integration library and setup resources.

Download the complete Connect CLI archive and verify its checksum:

```sh
platform=linux-x86_64 # 0.9.10 has no complete macos-arm64 archive yet.
archive="meerkat-connect-0.9.10-$platform.tar.gz"
release=https://github.com/lukacf/meerkat/releases/download/connect-v0.9.10
curl -fLO "$release/$archive"
curl -fLO "$release/$archive.sha256"
sha256sum -c "$archive.sha256" # On macOS: shasum -a 256 -c "$archive.sha256"
tar -xpf "$archive"
export PATH="$PWD/meerkat-connect-$platform/bin:$PATH"
```

Keep the extracted directory together. Then, from the project directory where
you want to use the integration:

```sh
meerkat-connect --realm work
```

Complete model setup if prompted and ask:

> Add an integration that reads Hacker News and give the normal agent read-only access.

Review the selected software and access. Complete sign-in or key entry in the
protected controls if the service needs an account. Once setup finishes, enter
`/quit`, then run a fresh ordinary agent from that same project directory:

```sh
rkat --realm work run --wait-for-mcp --mcp-auth stored \
  "Read the three leading Hacker News stories and summarize them."
```

Use the same realm and project context for both commands. See the
[quickstart](docs/site/quickstart.mdx) for custom roots and the integrated Console.
Core and `rkat` do not depend on Connect; Connect is a separate setup tool.

### macOS executables

The release workflow builds the `meerkat-connect` and `toolkit_gateway`
executables for Apple Silicon macOS on a GitHub-hosted macOS runner, the way
Core's and Meta's macOS binaries are built there, and adds them to a release as
`meerkat-connect-<version>-aarch64-apple-darwin.tar.gz` and
`meerkat-connect-gateway-<version>-aarch64-apple-darwin.tar.gz`. Each archive
holds one executable and has a `.sha256` file beside it. A release published
before this lane existed has them only if they were added afterwards: check
its asset list.

These archives are not the complete installation. They carry no bundled
Python, Node/npm, integration library or setup resources, and an executable
built without them refuses every step that needs them: the interactive
`meerkat-connect` setup conversation stops at startup, and no integration can
be prepared, installed or read back. These paths read no bundled resource, so
its absence does not stop them: `--version` and `--help`, the `credentials`
commands for services a realm already has, the provider sign-in and
paired-host adapters an application drives, and gateway startup for an
application whose setup supplies no bundled helper. The complete archive
keeps the name
`meerkat-connect-<version>-macos-arm64.tar.gz`. See
[release routes](release/CONNECT-CANDIDATE.md#macos-executables-from-the-release-workflow)
for how they are built and how to add Intel macOS (`x86_64-apple-darwin`),
which is not built.

## Use Connect in an application

An application with the integrated Connect Console can delegate setup from the
original agent, present installation and authentication controls, and return to
the original task in the same conversation. The application supplies login and
a persistent host. The gateway archive and Rust, JSON-RPC, Python and TypeScript
interfaces support this composition; the gateway alone does not install an app.

The [0.9.10 release](https://github.com/lukacf/meerkat/releases/tag/connect-v0.9.10)
includes ordinary CLI and Console setup. Connect 0.9.10 does not support
in-place updates that change an application's tool policy, definition,
instructions or runtime; Forge's Stage refuses them, so keep the existing
installation and state. Shared multi-caller setup is an unqualified opt-in,
available only when an application starts the gateway with
`--private-workflow-launch`; ordinary CLI and Console setup stay single-caller.
Do not install over an existing installation to update it.
Development work and its remaining checks are tracked in
[follow-up work](docs/releases/follow-up.md).

Service connectors and agent frontends have different roles. A frontend also
requires the application to own participant identity, audience, routing and
reply delivery. See [host composition](docs/site/guides/host-composition.mdx).

For the development provider-only host API, see the
[Python](sdk/python/PROVIDER-SETUP.md) and
[TypeScript](sdk/typescript/PROVIDER-SETUP.md) guides. Their source-only examples
use a dedicated protected child pipe before any agent session; they do not
qualify the published release or a deployed provider flow.

## Scope

- The adopted library contains 101 integration identities on macOS and 100 on
  Linux, which excludes Playwright. Support covers the declared operations and
  setup profiles; the counts do not claim live authentication with every provider.
- Agents can research integrations outside the catalog and propose supported
  packages. Installation, credentials and access are validated before use.
- Broad frontend/custom-host migration, remote deployment and universal provider
  onboarding remain [follow-up work](docs/releases/follow-up.md).
- REST and an outer MCP Connect entry point are deferred. MCP servers used as
  integrations are included.

See [release notes](https://github.com/lukacf/meerkat/releases/tag/connect-v0.9.10) for platform assets and compatibility,
[BUILDING.md](BUILDING.md) to build from source, and
[source provenance](SOURCE-PROVENANCE.md) for upstream attribution and catalog
identity. The [documentation source](docs/site) contains the full contracts.
