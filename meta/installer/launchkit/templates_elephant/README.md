# Knowledge service

This project can use a knowledge service for shared, governed knowledge with source provenance. It is separate from each MobKit agent's conversational memory and the project's SQLite notes.

Agent classes granted knowledge access use `/mcp/agent`, the knowledge service's read-only profile. Read-only JWT scopes are minted for local setups. The space belongs to the JWT principal; the space environment variable is a local verification expectation, not a replacement authorization header.

## Local service

Install Docker with Compose and Git. The build requires access to the source repository recorded in `settings.json`. The initial Rust build can take several minutes and substantial memory. A small cloud VM may need a larger machine for this optional build.

From the deployment root:

```sh
.venv/bin/python knowledge/manage.py setup
.venv/bin/python knowledge/manage.py up
.venv/bin/python knowledge/manage.py verify
```

`setup` generates distinct random database/JWT secrets and project-scoped read-only, operator and extractor tokens, saved with mode 0600. It uses saved project provider keys for OpenAI embeddings and the configured extraction provider, asking locally when a key is missing. No credentials belong in the downloaded blueprint. Tokens expire after 90 days; `renew` rotates the tokens without changing the database or signing secret, and the next `up` gives the service its new extractor token. In a folder prepared before the extractor token existed, `up` adds only that token and leaves the other tokens and secrets unchanged. The embedded store does not use the database secret; `setup` keeps it so that knowledge folders from earlier versions keep the same recovery checks.

After an earlier local setup attempt, restore the original private `knowledge/.env` from backup if it is missing or its signing or database secrets are lost, before running `setup`, `renew` or `up`. Keep the existing knowledge volume. Changing provider keys or generating new credentials cannot recover its stored knowledge.

If you operate the exported package yourself, keep the application stopped after replacing provider keys and rerun `setup`, then `up`, then `verify` from the same project folder using the commands above. This keeps the existing database and signing secrets. `up` alone does not reread changed project keys when `knowledge/.env` already exists. For a local knowledge service managed by Forge, **Start installed application** applies saved keys through `setup` and `up` when its installation support is current. Previously installed background services keep their saved support code; apply the offered **Installation support** update and any needed accepted application source update before relying on this behavior.

`up` checks out the full pinned Git commit into `knowledge/.source`, builds the included Dockerfile, and starts a project-specific Compose stack. Existing source modifications or a mismatched revision stop setup. The bundled Dockerfile builds the source's `meerkat-recall-bin` package with its declared Rust 1.94 toolchain, `cargo build --locked` and the source's `rocksdb` storage feature, and the image also provides the binary under its former command name, which the Compose service runs. It does not use the source's own release container build. The full Git revision and settings are reviewable before execution.

The stack is one knowledge service process with its store embedded: RocksDB files in a persistent private Docker volume, with no separate database server. Each start runs the native `elephant migrate` command and then `elephant serve` in the same container, because both take the store's writer lock. Only the service's API is published, on loopback. It enforces JWT authentication. Its internal extractor reads the project space through the service's own `/mcp/agent` listener with a dedicated read-only extractor token (`MCP_BEARER_TOKEN` in `knowledge/.env`) whose subject also holds an active `recall-extractor` grant for this space. No source patches are applied. Automated identity merges are disabled; extraction and native truth maintenance remain available. Do not attach untrusted containers to this Compose network.

After the service is healthy, `up` prepares the project space the way the source's service setup guide describes. A bootstrap token that expires within 15 minutes, held only in memory and never saved, creates the space with its security policy and processing approvals, then grants the extractor its read scopes. The approvals bind query and document embeddings to the OpenAI embedding model and extraction and support verification to the configured extraction model. Their credential references name the provider key environment variables, never the keys. A later `up` only reads the space and grant back; it does not create a second space or change an existing grant. `up` then waits up to two minutes for semantic readiness and reports the space status with the verification result.

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

This uses `docker compose stop`; it never removes volumes. Back up the complete knowledge data volume and `knowledge/.env` with your normal stopped-volume backup procedure. There is no automatic volume deletion command.

## Existing remote service

Set the token and expected space variables named in `settings.json` in the deployment's local `.env`, then run `knowledge/manage.py verify`. The endpoint must be HTTPS unless it is loopback. Obtain a read-only JWT from that service's administrator for the intended space. Locally decoding a JWT can detect a space mismatch but does not verify its signature; the actual MCP request proves server acceptance. The knowledge service intentionally hides global `get_health` from space-scoped principals. The helper respects that boundary and reports authenticated runtime status and capabilities instead of requesting global access.

`knowledge/manage.py ingest` on a remote service requires `ELEPHANT_OPERATOR_TOKEN` supplied locally. It never reuses the agent's read-only credential to escalate access.

## Licences

The knowledge service source is licensed MIT OR Apache-2.0. Its storage engine is SurrealDB 2.6.5: the `surrealdb` and `surrealdb-core` crates, which the source vendors with its own patches and which the image always includes. They are under the Business Source License 1.1 (BUSL-1.1) from SurrealDB Ltd. Its Additional Use Grant does not cover offering a Database Service, and it changes to Apache-2.0 on 2029-09-17. RocksDB comes from `surrealdb-rocksdb` (Apache-2.0) and `surrealdb-librocksdb-sys` (MIT, Apache-2.0 or BSD-3-Clause). The full licence texts are in the checked-out source: `LICENSE`, `vendor/surrealdb-2.6.5/LICENSE` and `vendor/surrealdb-core-2.6.5/LICENSE`; `vendor/RECALL_SURREALDB_PATCH.md` describes the source's patches to those crates. The project licence does not replace the licences of these components.

## Provenance

The generated settings pin the source repository and full Git revision. The implementation follows the source README, `docs/operations/service-setup.mdx`, `bin/elephant-api/src/auth.rs`, `bin/elephant-api/src/mcp.rs`, `crates/types/src/mcp_simple.rs`, and the current Cargo package/toolchain contract. The helper reports live tool capabilities and semantic readiness rather than assuming the source check alone proves a running deployment.
