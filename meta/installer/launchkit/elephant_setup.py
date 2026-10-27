"""Export an isolated knowledge service using verified source contracts."""
from __future__ import annotations

import json
import re
from pathlib import Path
from urllib.parse import urlsplit

from .schema import DEFAULT_MODELS

ELEPHANT_REPOSITORY = "https://github.com/lukacf/meerkat-recall"
ELEPHANT_REVISION = "d0493b6e9ba6e61ecea24e50b5f8748e3ffe8f6c"
# Earlier versions recorded this default pin. Neither repository URL serves its
# revision any more, so a project that still records it builds the current default.
RETIRED_DEFAULT_REVISION = "98307999503609abde1b06861154c1fef5b113e5"
RETIRED_DEFAULT_REPOSITORIES = frozenset({"https://github.com/lukacf/elephant", ELEPHANT_REPOSITORY})
EMBEDDING_MODEL, EMBEDDING_DIMS = "text-embedding-3-large", 3072
TEMPLATES = Path(__file__).with_name("templates_elephant")


def validate_environment_names(token_env: str, space_env: str):
    reserved = {"OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY",
                "JWT_SECRET", "SURREALDB_PASS", "ELEPHANT_OPERATOR_TOKEN", "MCP_BEARER_TOKEN", "PATH", "HOME",
                "USER", "SHELL", "TMPDIR", "TEMP", "SYSTEMROOT", "PYTHONPATH", "VIRTUAL_ENV"}
    for name in (token_env, space_env):
        if not re.fullmatch(r"[A-Z_][A-Z0-9_]*", name):
            raise ValueError("Invalid knowledge service environment name.")
        if name in reserved or name.startswith(("LD_", "DYLD_", "MOBKIT_")):
            raise ValueError("Use dedicated knowledge-service token and space environment names, separate from provider keys and runtime settings.")
    if token_env == space_env:
        raise ValueError("Knowledge service token and space need separate environment names.")


def elephant_capability() -> dict:
    return {
        "repository": ELEPHANT_REPOSITORY,
        "revision": ELEPHANT_REVISION,
        "storage": "Embedded RocksDB storage inside the knowledge service process, private persistent Docker volume",
        "endpoint": "http://127.0.0.1:8091/mcp/agent",
        "access": "JWT principal scoped to one project space; agent connections use the read-only profile.",
        "local_requirements": ["Docker with Compose", "Git", "OpenAI API key for embeddings", "A provider key for the selected extraction model", "Source repository access"],
        "status": "Source-verified export; after startup the deployment helper provisions the project space and verifies live health, MCP capabilities and semantic ingestion readiness.",
    }


def generate_elephant_files(knowledge: dict, project_slug: str, provider: str = "openai", model: str | None = None) -> dict[str, str]:
    if hasattr(knowledge, "model_dump"):
        knowledge = knowledge.model_dump()
    mode = knowledge.get("mode", "none")
    if mode == "none":
        return {}
    if mode not in {"local", "remote"}:
        raise ValueError("Unknown knowledge deployment mode.")
    if model is None:
        model = DEFAULT_MODELS[provider]
    if not re.fullmatch(r"[a-z][a-z0-9-]{1,39}", project_slug):
        raise ValueError("Invalid project slug for knowledge setup.")
    url = knowledge.get("url", "http://127.0.0.1:8091/mcp/agent")
    parsed = urlsplit(url)
    if parsed.scheme not in {"https", "http"} or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError("The knowledge service needs an HTTP MCP URL without embedded credentials or query parameters.")
    if parsed.scheme == "http" and parsed.hostname not in {"127.0.0.1", "localhost", "::1"}:
        raise ValueError("Remote knowledge service connections require HTTPS.")
    if parsed.path.rstrip("/") != "/mcp/agent":
        raise ValueError("Knowledge access for agent classes uses the knowledge service's read-only /mcp/agent profile.")
    token_env, space_env = knowledge.get("token_env", "ELEPHANT_TOKEN"), knowledge.get("space_env", "ELEPHANT_SPACE_ID")
    validate_environment_names(token_env, space_env)
    source = knowledge.get("source_repository") or ELEPHANT_REPOSITORY
    revision = knowledge.get("source_revision") or ELEPHANT_REVISION
    if revision == RETIRED_DEFAULT_REVISION and source.removesuffix(".git") in RETIRED_DEFAULT_REPOSITORIES:
        source, revision = ELEPHANT_REPOSITORY, ELEPHANT_REVISION
    if not re.fullmatch(r"https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?", source):
        raise ValueError("Use an HTTPS GitHub source repository for local knowledge service builds.")
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("Pin the local knowledge service source to a full 40-character Git commit.")
    settings = {"mode": mode, "url": url, "project": project_slug, "token_env": token_env, "space_env": space_env, "source_repository": source, "source_revision": revision, "provider": provider, "model": model}
    files = {
        "knowledge/settings.json": json.dumps(settings, indent=2) + "\n",
        "knowledge/manage.py": (TEMPLATES / "manage.py").read_text(),
        "knowledge/README.md": (TEMPLATES / "README.md").read_text(),
        "knowledge/.gitignore": ".env\n.source/\n*.receipt.json\n__pycache__/\n",
    }
    if mode == "remote":
        return files
    if parsed.scheme != "http" or parsed.hostname not in {"127.0.0.1", "localhost"} or not parsed.port or not 1024 <= parsed.port <= 65535:
        raise ValueError("A local knowledge service needs http://127.0.0.1:PORT/mcp/agent with a port from 1024 to 65535.")
    # One process owns the embedded store: migrate and serve take its writer
    # lock in turn, so they run one after the other in the same container.
    environment = {
        "ELEPHANT_DB": "surreal-db", "SURREALDB_URL": "rocksdb:///data/store",
        "SURREALDB_NAMESPACE": "elephant", "SURREALDB_DATABASE": project_slug.replace("-", "_"),
        "ELEPHANT_DURABILITY_PATH": "/data/recovery", "ELEPHANT_SNAPSHOT_DIR": "/data/backups",
        "ELEPHANT_HOST": "0.0.0.0", "ELEPHANT_PORT": "3000", "RUST_LOG": "warn",
        # The runtime status reports this space's budget; without it, a space named "default".
        "MCP_SPACE_ID": project_slug,
        "SKIP_AUTH": "false", "JWT_SECRET": "${JWT_SECRET:?Run knowledge/manage.py setup}",
        "JWT_ALGORITHM": "HS256", "JWT_ISSUER": "mobkit-launch", "JWT_AUDIENCE": project_slug,
        # The extractor reads this project space through the service's own
        # listener, with the worker token that setup mints for that space.
        "MCP_BEARER_TOKEN": "${MCP_BEARER_TOKEN:?Run knowledge/manage.py setup}",
        "PIPELINE_AGENTIC_MCP_URL": "http://127.0.0.1:3000/mcp/agent", "PIPELINE_AGENTIC_MCP_SPACE_ID": project_slug,
        "PIPELINE_AGENTIC_MODEL": model, "PIPELINE_MAINTENANCE_ENABLED": "false",
        "PIPELINE_EMBED_ENABLED": "true", "PIPELINE_PROMOTE_ENABLED": "true",
        "PIPELINE_EMBED_PROVIDER": "openai", "PIPELINE_EMBED_MODEL": EMBEDDING_MODEL, "PIPELINE_EMBED_DIMS": str(EMBEDDING_DIMS),
        "PIPELINE_MAX_CONCURRENT": "2", "PIPELINE_BUDGET_DAILY_ITEMS_GRAPH": "1000",
        "PIPELINE_SKILLS_DIR": "/app/skills", "OPENAI_API_KEY": "${OPENAI_API_KEY:?An OpenAI key is required for knowledge embeddings}",
    }
    if provider != "openai":
        key = {"anthropic": "ANTHROPIC_API_KEY", "gemini": "GEMINI_API_KEY"}[provider]
        environment[key] = "${" + key + ":?Extraction provider key is required}"
    compose = {
        "name": f"{project_slug}-knowledge",
        "services": {
            "elephant": {
                "image": f"mobkit-launch-elephant:{revision[:12]}", "build": {"context": ".source", "dockerfile": "../Dockerfile"},
                "command": ["sh", "-c", "elephant migrate && exec elephant serve"],
                "environment": environment, "ports": [f"127.0.0.1:{parsed.port}:3000"], "restart": "unless-stopped",
                "volumes": ["knowledge-data:/data", "knowledge-assets:/app/assets"],
            },
        },
        "volumes": {"knowledge-data": {}, "knowledge-assets": {}},
    }
    files["knowledge/compose.json"] = json.dumps(compose, indent=2) + "\n"
    files["knowledge/Dockerfile"] = (TEMPLATES / "Dockerfile").read_text()
    files["knowledge/Dockerfile.dockerignore"] = ".git\ntarget\n.env\n.env.*\n.codex\n.agents\n*.log\n"
    return files
