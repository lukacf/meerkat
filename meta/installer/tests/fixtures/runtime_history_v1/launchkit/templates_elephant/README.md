# Elephant knowledge

This project can use Elephant for shared, governed knowledge with source provenance. It is separate from each MobKit agent's conversational memory and the project's SQLite notes.

Agent classes granted knowledge access use `/mcp/agent`, Elephant's read-only profile. Read-only JWT scopes are minted for local setups. The space belongs to the JWT principal; the space environment variable is a local verification expectation, not a replacement authorization header.

## Local service

Install Docker with Compose and Git. The build requires access to the source repository recorded in `settings.json`. The initial Rust build can take several minutes and substantial memory. A small cloud VM may need a larger machine for this optional build.

From the deployment root:

```sh
.venv/bin/python knowledge/manage.py setup
.venv/bin/python knowledge/manage.py up
.venv/bin/python knowledge/manage.py verify
```

`setup` generates distinct random database/JWT secrets and project-scoped read-only and operator tokens, saved with mode 0600. It asks for an OpenAI embedding key and the configured extraction provider key locally. No credentials belong in the downloaded blueprint. Tokens expire after 90 days; `renew` rotates the tokens without changing the database or signing secret.

`up` checks out the full pinned Git commit into `knowledge/.source`, builds the included Dockerfile, and starts a project-specific Compose stack. Existing source modifications or a mismatched revision stop setup. The bundled Dockerfile uses the source's declared Rust 1.94 toolchain and `cargo build --locked`; it does not use the upstream Dockerfile's older compiler. The full Git revision and settings are reviewable before execution.

The native `elephant migrate` command initializes the database before either API starts. SurrealDB stores knowledge in a persistent private Docker volume. Only Elephant's API is published, on loopback. A private reader sidecar serves the same project space to Elephant's internal extractor without exposing a host port. The primary API enforces JWT authentication and never receives a global `MCP_BEARER_TOKEN`, because this source revision prefers that variable over incoming client tokens. No source patches are applied. Automated identity merges are disabled; extraction and native truth maintenance remain available. Do not attach untrusted containers to this Compose network.

For semantic ingestion, `verify` must report `semantic_ingest_ready: true`. Starting a process or connecting a read-only agent does not prove extraction is ready. Extraction and embeddings use the selected providers and can incur model costs when you ingest material.

```sh
.venv/bin/python knowledge/manage.py ingest ./workspace/manual.txt --title "Project manual"
.venv/bin/python knowledge/manage.py search "Project manual"
```

Ingest is an explicit operator operation. It calls the live tool schema before submitting, stores the file SHA-256 and original filename as provenance metadata, and refuses until semantic ingestion is ready. It prints the native receipt; ingestion may finish asynchronously. Agent classes never receive the operator token. Search runs under the read-only token.

Stop without removing data:

```sh
.venv/bin/python knowledge/manage.py stop
```

This uses `docker compose stop`; it never removes volumes. Back up the complete SurrealDB volume and `knowledge/.env` using your normal stopped-volume or SurrealDB backup procedure. There is no automatic volume deletion command.

## Existing remote service

Set the token and expected space variables named in `settings.json` in the deployment's local `.env`, then run `knowledge/manage.py verify`. The endpoint must be HTTPS unless it is loopback. Obtain a read-only JWT from that service's administrator for the intended space. Locally decoding a JWT can detect a space mismatch but does not verify its signature; the actual MCP request proves server acceptance. Elephant intentionally hides global `get_health` from space-scoped principals. The helper respects that boundary and reports authenticated runtime status and capabilities instead of requesting global access.

`knowledge/manage.py ingest` on a remote service requires `ELEPHANT_OPERATOR_TOKEN` supplied locally. It never reuses the agent's read-only credential to escalate access.

## Provenance

The generated settings pin the source repository and full Git revision. The implementation follows the source README, `bin/elephant-api/src/auth.rs`, `bin/elephant-api/src/mcp.rs`, `crates/types/src/mcp_simple.rs`, and the current Cargo package/toolchain contract. The helper reports live tool capabilities and semantic readiness rather than assuming the source check alone proves a running deployment.
