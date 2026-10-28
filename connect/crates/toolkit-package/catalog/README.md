# Bundled discovery recipes

`bundled_adopted_catalog()` loads `adopted.json`, a generated projection of the
adopted entries in `docs/adoption/oss-inventory.json`. It uses the ordinary
`IntegrationPackage` and `PackageCatalog` contracts. The current 101 entries retain
their selected operations, exact configuration/evidence references and scoped
local MCP compatibility. They contain no executable activation selection or
account binding. Hosts can pass this catalog to `DiscoveryTools::new` explicitly.

Slack and Gmail retain their primary adopted Activepieces strategy and an
additional first-party services strategy in those same records. Operation hints,
instructions and compatibility assessments identify the owning route and its
evidence. The primary provenance remains the OSS adoption provenance; the
additional route records its separate Toolkit source and artifact identity.
Agents select a strategy explicitly. There is no automatic fallback or count
increase from an additional implementation.

The local consumer exporter includes the same bytes as `catalog.json` alongside
the Agno, Activepieces and services installation inputs. Its pinned digest binds
that metadata edition. Evidence paths still refer to this source distribution;
the consumer bundle does not contain the verification history.

Regenerate with `python3 scripts/render-adopted-catalog.py`; use `--check` to reject
stale output. The generator checks referenced local evidence hashes and requires
indexed acceptance, review and support. Repository-relative evidence paths refer
to this source distribution; they are not published download URLs. Metadata
versions derive from the full description content. An inventory revision does
not silently overwrite a retained package identity. Source and artifact versions
remain separate. The projection neither selects integrations nor installs them.

## Earlier research catalog

This inventory contains nine Toolkit-authored packages with ten role hints and ten setup strategies. It contains no executable implementation, credential, grant, selected activation manifest or claim of tested Toolkit support. `bundled_discovery_catalog()` loads it through the same inert metadata validator as other packages. Agents choose integrations and may investigate outside this inventory.

Each source hash in `source-observations.json` identifies the bytes inspected for the recipe. For web documentation it is an observation, not an immutable upstream release or a pin on a remote service. Resolve and verify the selected artifact, endpoint, account and actual operations at setup. No upstream source code is copied by these recipes, and the Toolkit recipe license does not relicense the cited sources or services.

| Package | Role hints | Source |
| --- | --- | --- |
| GitHub | Service | [Official remote server](https://github.com/github/github-mcp-server/blob/main/docs/remote-server.md) |
| Linear | Service | [MCP documentation](https://linear.app/docs/mcp) |
| Notion | Service | [Connection documentation](https://developers.notion.com/guides/mcp/get-started-with-mcp) |
| Slack | Service and frontend | [MCP documentation](https://docs.slack.dev/ai/slack-mcp-server/), [event ingress](https://docs.slack.dev/apis/events-api/using-http-request-urls) |
| Home Assistant | Service | [MCP integration](https://www.home-assistant.io/integrations/mcp_server/) |
| Playwright | Service | [Official MCP implementation](https://github.com/microsoft/playwright-mcp) |
| Gmail | Service | [API setup](https://developers.google.com/workspace/gmail/api/quickstart/python) |
| Google Calendar | Service | [API setup](https://developers.google.com/workspace/calendar/api/quickstart/python) |
| Telegram | Frontend | [Bot API](https://core.telegram.org/bots/api), [SDK frontend modules](../../../sdk/python/TELEGRAM.md) |

The setup instructions retain unresolved requirements. Remote MCP availability does not provide agent admission or response delivery. Frontend hints require separate application identity, conversation and audience bindings. Authentication uses the configured credential/user-action owner; these recipes neither launch a browser nor request a secret in model context.

Telegram's `bot-frontend` strategy points at the Meerkat Toolkit Python SDK frontend modules described in `sdk/python/TELEGRAM.md`. A host that offers its own Telegram extension supplies that extension's source and packaging through its own host integration, not through this catalog. Reading the strategy grants no source access and proves no running host or delivery.

Maintainers can revise recipes after refreshing source evidence. An immutable package version must not change content. Promotion to supported status requires reproducible conformance for the exact service, deployment and operations, with separate frontend evidence where relevant. The current library acceptance obligations remain incomplete.
