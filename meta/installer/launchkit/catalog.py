"""Provider documentation facts supplement, but never replace, live discovery."""

REGISTRY_URL = "https://registry.modelcontextprotocol.io"
REGISTRY_DOCS = "https://github.com/modelcontextprotocol/registry/blob/main/docs/reference/api/official-registry-api.md"

PROVIDERS = {
    "github": {
        "name": "GitHub", "query": "github", "registry_id": "io.github.github/github-mcp-server",
        "documentation": "https://github.com/github/github-mcp-server/blob/main/docs/host-integration.md",
        "auth": "Personal access token or GitHub App token. Limit its repository permissions.",
    },
    "linear": {
        "name": "Linear", "query": "linear", "registry_id": "app.linear/linear",
        "documentation": "https://linear.app/docs/mcp",
        "auth": "API key or OAuth token through a Bearer header. Read-only API keys are supported.",
    },
    "notion": {
        "name": "Notion", "query": "notion", "registry_id": "com.notion/mcp",
        "documentation": "https://github.com/makenotion/notion-cookbook/blob/main/docs/mcp-client-integration.md",
        "auth": "Hosted Notion requires OAuth. A separately configured stdio server can use an integration token.",
    },
}


def connector_catalog() -> dict:
    return {
        "registry_url": REGISTRY_URL,
        "documentation": REGISTRY_DOCS,
        "starters": list(PROVIDERS.values()) + [
            {"name": "PostgreSQL", "query": "postgres", "documentation": REGISTRY_DOCS},
            {"name": "Files", "query": "filesystem", "documentation": REGISTRY_DOCS},
            {"name": "Search", "query": "search", "documentation": REGISTRY_DOCS},
        ],
        "notice": "Registry listing identifies a publisher and configuration source; it is not a security review. Packages execute on the deployment host. Assign only the tools each class needs.",
    }
