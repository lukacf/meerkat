# 003 — Hello Meerkat (TypeScript SDK)

The simplest TypeScript agent. The SDK spawns `rkat-rpc` as a child process
and can use either an installed/downloaded runtime binary or the repo-local
binary built from this checkout.

## Prerequisites

The quickest way is the released SDK in a directory of its own. On first use
it downloads the `rkat-rpc` runtime of its own version and checks it against
a digest pinned in the package, so there is nothing else to install:

```bash
mkdir hello-meerkat && cd hello-meerkat
npm init -y
npm install @rkat/sdk tsx
cp /path/to/meerkat/examples/003-hello-meerkat-ts/main.ts .
ANTHROPIC_API_KEY=sk-... npx tsx main.ts
```

This example follows the main branch. If it uses something newer than the SDK
you installed, copy `main.ts` from that release's tag instead.

To run against this checkout, build the SDK and the runtime from the
repository root. The SDK built from source pins no runtime digest and refuses
to download one, so it needs `MEERKAT_BIN_PATH`:

```bash
npm --prefix sdks/typescript install
npm --prefix sdks/typescript run build
(cd examples && npm install)
./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
```

## Concepts
- `MeerkatClient` — typed async client
- `connect()` / `close()` — process lifecycle
- `createSession(prompt, options)` — execute a prompt
- `Session` — typed session handle with `.text`, `.usage`, `.id`

## Run

From the checkout, after the build above:

```bash
ANTHROPIC_API_KEY=sk-... npx tsx examples/003-hello-meerkat-ts/main.ts
```

The cleanup scope includes the connection handshake, so failed initialization
also closes the spawned runtime. Fatal errors print a diagnostic and exit nonzero.

## Offline regression checks
After installing the example dependencies and building the local TypeScript SDK,
run `npm --prefix examples run check:sdk` and
`npm --prefix examples run test:sdk` from the repository root. The latter also
needs Python 3.10+ and the local Python SDK's dependencies; it runs the nine SDK
examples with local JSONL/HTTP fixtures and validates the emitted sentiment schema
with Ajv. It makes no provider calls and does not replace live-provider validation.
