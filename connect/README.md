# Meerkat Connect

Give your agents the tools they need. Meerkat Connect combines more than 100
adopted OSS integrations with agent-led discovery and installation beyond the
catalog. Describe a task, review the proposed access and let Connect guide setup.

## Install and use

Connect 0.9.0 supports Apple Silicon macOS 13.5 or later and Linux x86_64 with
glibc 2.39. Install [Meerkat Core 0.9.0](https://github.com/lukacf/meerkat/releases/tag/v0.9.0)
for the ordinary `rkat` agent. Connect supplies its own Python, Node/npm,
integration library and setup resources.

Download the complete Connect CLI archive and verify its checksum:

```sh
platform=macos-arm64 # Use linux-x86_64 on Linux.
archive="meerkat-connect-0.9.0-$platform.tar.gz"
release=https://github.com/lukacf/meerkat-connect/releases/download/v0.9.0
curl -fLO "$release/$archive"
curl -fLO "$release/$archive.sha256"
shasum -a 256 -c "$archive.sha256" # On Linux: sha256sum -c "$archive.sha256"
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

## Use Connect in an application

An application with the integrated Connect Console can delegate setup from the
original agent, present installation and authentication controls, and return to
the original task in the same conversation. The application supplies login and
a persistent host. The gateway archive and Rust, JSON-RPC, Python and TypeScript
interfaces support this composition; the gateway alone does not install an app.

Installed-package checks, including controlled native original-task continuation,
passed on both supported platforms. The fresh generated-Console business example
remains incomplete, as documented in the [release notes](docs/releases/0.9.0.md).

Service connectors and agent frontends have different roles. A frontend also
requires the application to own participant identity, audience, routing and
reply delivery. See [host composition](docs/site/guides/host-composition.mdx).

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

See [release notes](docs/releases/0.9.0.md) for platform assets and compatibility,
[BUILDING.md](BUILDING.md) to build from source, and
[source provenance](SOURCE-PROVENANCE.md) for upstream attribution and catalog
identity. The [documentation source](docs/site) contains the full contracts.
