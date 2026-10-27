# MCP Apps host integration

Inline widgets are independent of developer `customPanels` and `panel_modules`.
App authors use standard MCP tool `_meta.ui.resourceUri`, `ui://` resources,
`text/html;profile=mcp-app`, and MCP Apps communication. No Meta manifest,
custom result block, or renderer registration is required.

## Stock Console setup

The stock Console creates its native gateway adapter from the experience
configuration. Products do not need to pass `mcpAppsHost` or register renderers.
Configure the isolated sandbox URL in `config/console.toml`:

```toml
mcp_apps_sandbox_url = "http://127.0.0.1:5001/sandbox.html"

[realms.production]
mcp_apps_sandbox_url = "https://console-apps.example.net/sandbox.html"
```

Serve the bundled proxy in a separate Node process:

```sh
MCP_APPS_HOST_ORIGIN=http://127.0.0.1:5000 MCP_APPS_SANDBOX_PORT=5001 npm --prefix console run mcp-apps:sandbox
```

`MCP_APPS_HOST_ORIGIN` is the exact Console origin, including its port. For
multiple approved origins, use comma-separated `MCP_APPS_HOST_ORIGINS`. The
process binds loopback by default; `MCP_APPS_SANDBOX_HOST=0.0.0.0` supports a
container or dedicated reverse proxy. In production, terminate TLS on the
sandbox's dedicated origin and forward only to this process. It serves only
`/sandbox.html`, never gateway APIs. Keep Console cookies host-only and do not
serve credentials, sensitive files, or application APIs on the sandbox origin.
The proxy needs Node's standard library and `src/mcp-apps/sandbox.js`, with no
npm dependencies.

The gateway binaries embed the proxy. Outside a source checkout, have the
gateway you run write it out, then start it with Node and the same
environment variables:

```sh
meta_gateway --write-mcp-apps-sandbox ./mcp-apps-sandbox   # or rpc_gateway
MCP_APPS_HOST_ORIGIN=http://127.0.0.1:5000 MCP_APPS_SANDBOX_PORT=5001 node ./mcp-apps-sandbox/mcp-apps-sandbox.cjs
```

The flag writes `mcp-apps-sandbox.cjs` and `src/mcp-apps/sandbox.js` into the
directory, creating it and replacing earlier copies, prints each file's
SHA-256 and path, and exits without starting a gateway. The proxy and the
Console embedded in the gateway share an internal sandbox protocol, so write
the proxy again from the new binary whenever you upgrade the gateway. The
Python and npm packages do not include the proxy.

Gateways before 0.9.2 do not embed it. For those, copy
`console/mcp-apps-sandbox.cjs` and `console/src/mcp-apps/sandbox.js` from the
release tag that matches the gateway, keep the second file at
`src/mcp-apps/sandbox.js` relative to the first, and check both against the
SHA-256 digests that `docs/guides/console.mdx` lists for 0.9.0 and 0.9.1.

Omitting the URL or setting a realm override to an empty string retains text
tool results and disables the corresponding HTTP app operations. Native live
operations also require an MCP connection that negotiated MCP Apps. Gateways
reject malformed URLs, credentials, fragments and a
sandbox sharing their known local or declared public Console origin before
advertising MCP Apps. The browser also checks its actual origin, including
when an embedding host supplies a custom adapter. A reusable product can
still supply its own `ConsoleMcpAppsHost` adapter, but the stock HTTP adapter is
the default for configured gateways on the Console's authenticated origin.

## Invocation and authority

`ConsoleMcpAppView` uses the official `@modelcontextprotocol/ext-apps` AppBridge.
The conversation projection supplies an identity/session/tool-call locator from
an observed tool invocation. Arbitrary tool text and model-generated JSON cannot
create an authorized binding.

The live Agent can expose an accepted result before the turn commits. These
locator frames use the `tool_application` source; observing a view does not
change committed history or grant permission to invoke a tool.

The stock adapter sends authenticated, non-cacheable requests to:

- `POST /console/mcp-apps/resolve`
- `POST /console/mcp-apps/read-resource`
- `POST /console/mcp-apps/call-tool`

Every request includes the original locator. The gateway derives the viewer from
its normal authentication and checks access to the originating member. Headers
carrying Console scope are routing context, not authorization. The runtime owns
the physical MCP registration, its retirement checks, app visibility, policy and
approval. Browser code never opens another MCP connection or receives its
credentials.

Resolve returns the original Tool, arguments and complete CallToolResult, plus
an optional retained resource and `canCallTools`. Cached display does not wait
for the current agent turn. `canCallTools` reports the authenticated viewer's
access to the live host action surface; each action still requires fresh native
admission and the original MCP connection. Reopening a view does not run
its original tool. Retained HTML can render a historical invocation; uncached
resource reads and app actions must re-enter the live registration check.
Both require the Console's `agent.send` permission and an editable Console.
A viewer with only `agent.view` can read the exact retained renderer resource
without contacting the MCP server. Other resource URIs require fresh admission;
they need not appear in `resources/list`.

During a run, the native Agent publishes a process-only display observation
only after accepting the actual tool result into its canonical Session. The
session service fences that read to the current actor and run. This observation
is separate from committed history and never authorizes a tool action. After
the run, replay comes from the runtime's committed session. Both paths omit
ambiguous duplicate tool-call IDs. Live timeline frames use
`source.kind = "tool_application"` and carry only locators.

No result body or HTML is stored in localStorage or sessionStorage. UI-only
`_meta` stays in the host channel. Native persistence retains original evidence;
the browser keeps only the current view's data in memory. Text content remains
the fallback for other channels and unavailable views.

The native host preloads the declared renderer resource before `tools/call`,
then delivers the actual invocation's input and result after view initialization.
The host permits admitted resource reads and tool calls. App tool actions pass
through the native pre- and post-tool hooks with fresh invocation context.
A pre-hook refusal prevents dispatch. A post-hook refusal withholds the result
without pretending already settled tool effects were undone. Calls are never
automatically retried. Setup times out after 15 seconds; API calls also obey a
bounded timeout. Closing or changing authority aborts requests and retires its
isolated iframe. The host allows a bounded teardown acknowledgment without
letting the retiring view send further requests or affect a replacement view.

Products requiring a durable action audit should provide native hooks or an
admission/observation implementation that records these actions. The stock
runtime persists session effects and hook notices, with best-effort hook events;
enabling governance alone does not create a durable widget-action journal.
Tool hooks cover app tool calls, not resource reads. Ungoverned members do not
consult the governed work-policy owners. Products requiring resource URI
restrictions, explicit-only app visibility or an audit of every operation need
policy and recording in their trusted native host; these are not stock Console
configuration options.
App results are delivered through MCP Apps rather than added as synthetic model
transcript entries.

Fresh App IO runs independently of the model turn through an executor owned by
the original native actor. Calls that produce no session effects or hook
notices return without queueing a transcript export or session save. Calls that
do produce them retain the native outcome until that exact actor can apply and
persist the settlement, including when a post-hook refuses publication or the
browser stops waiting. Such settlement can wait behind an active turn. The
runtime uses actual collected effects and notices, never `readOnlyHint`, to
decide whether session persistence is required.

The proxy applies CSP from resource metadata to fetches and frames, denying
undeclared origins by default. It rejects grants covering any configured
Console hostname, including alternate ports, transports and covering wildcards.
The inner view has an opaque origin. Camera, microphone,
geolocation, downloads, popups and form submission are unavailable in this host
profile. External-link requests, conversation handoff and model-context updates
are not advertised or supported by this profile.

Navigation retires the bridge when the proxy observes document unload or a
replacement load. This is a lifecycle mechanism: browser event ordering does
not guarantee that retirement precedes every message from a replacement
document, and app JavaScript can remove document listeners. Fresh resource
reads still require native admission; tool actions also cross native tool hooks.
Cached renderer reads remain authorized history lookups without live tool IO.

CSP coverage depends on the browser. The proxy emits the standard
`webrtc 'block'` directive, but Chromium 140 ignores it and still gathers ICE
candidates with `connect-src 'none'`. This host therefore does not promise
complete network containment of arbitrary app JavaScript. Products requiring
that guarantee need browser or network enforcement beyond iframe CSP.

## Generated interfaces

The [OpenUI article](https://www.openui.com/blog/how-chatgpt-intelligent-ui-works)
is a reverse-engineering report, not an OpenAI interoperability contract. It
motivates keeping generated descriptions, execution, data and admitted actions
separate. This implementation does not add a generated UI language or compiler.

An MCP server can already expose a predeclared renderer and return a versioned,
opaque generated document with its data. The app interprets that document; the
host retains the result without imposing a language or component catalog. A
bundled generated-UI renderer or incremental document protocol would be an
additional feature. Generated content must not enter the trusted custom-panel
module loader.

Keep these boundaries when adding that feature:

- Preserve invocation identity and original result independently of transient
  view state. Do not execute the original tool to restore a view.
- Apply future ordered updates to an existing view rather than reloading an
  iframe for every patch. Define revision, completion, cancellation and fallback
  semantics when streaming is implemented; completed results are not partial
  generation events.
- Keep local controls local. Route tool effects through the original member and
  registration's admission path. A renderer must not create a credentialed
  browser MCP client or arbitrary network-action map.
- Add conversation handoff only through an explicit host action, preserving its
  app provenance and normal admission. Generated text cannot assert human
  authorization or become a higher-priority instruction.
- Use a constrained renderer or separately budgeted execution environment for
  generated programs. Iframe isolation alone does not bound arbitrary JavaScript
  CPU usage. Bind authoritative values from actual tool results rather than
  asking a model to transcribe them.

[MCP Apps](https://github.com/modelcontextprotocol/ext-apps/blob/main/specification/2026-01-26/apps.mdx)
provides the app hosting and communication contract. It does not standardize a
generated component language or native renderer operations. Those are future
product decisions, not additional requirements on today's app authors.

## Protocol fixture

Run the sandbox policy and real-browser lifecycle checks with:

```sh
npm --prefix console run test:mcp-apps:sandbox
```

```sh
node console/mcp-apps-preview.cjs
```

This deterministic fixture uses in-memory data and standard App/AppBridge
messages. It tests the host protocol and rendering, not native registration,
member policy or durable replay. `/counts` reports resolve, resource-read and
action counts. A native end-to-end check must exercise the real gateway and its
MCP connection separately.

Build the stock Console and the native `mcp_apps_gateway_fixture` example
first, then run the native browser lane against those artifacts:

```sh
META_EXAMPLE_BIN_DIR=/path/to/prebuilt/examples npm --prefix console run e2e:mcp-apps
```

That lane starts the persistent `rpc_gateway` composition with a scripted
model, drives it over stdin like an SDK, and spawns its members through the
gateway's own member factories. Their MCP routers carry the stock connector
call-context provider that every gateway deployment uses, so a regression in
that provider's resource reads fails the lane. It starts a stdio MCP server,
loads the stock Console, invokes its app-only action, and checks reload,
original connection custody, private metadata separation and native request
rejection. It writes screenshots and wire evidence under `output/playwright/`
and never contacts a model provider. The gateway lane spawns classic members.
`META_MCP_APPS_BACKEND=builder` runs the same lane against
`console_acceptance_fixture`, a `UnifiedRuntime::builder()` host with
identity-first members whose routers carry no call-context provider; CI runs
both.

To also exercise live polling through a model turn longer than the host's
request timeout, run the same lane with `META_MCP_APPS_LONG_POLL=1`. This mounts
three native App views, polls through their original MCP connection throughout
the held turn, and checks updates before and after 60 seconds. Core's persistent
service regressions separately count storage operations to verify that calls
without session mutations do not cause a save for every poll.
