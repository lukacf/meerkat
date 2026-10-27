# F: Console, flow editor, reusable packages and runnable examples

[Audit index](README.md) | [Coverage](coverage.md)

Original documentation and initial evidence line ranges refer to baseline `af82b6b3ab34faed9bf3e962d148d55f10dcd1dc`, unless an external dependency or historical revision is explicitly identified. Final-review citations refer to the corrected files in this change. Source excerpts may be de-indented or omit intervening lines; cited ranges identify the complete context. Quoted defects are preserved as evidence, not current usage guidance.

## F-001: The embedded console still requests external Google Fonts

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/console.mdx:30`**

```text
The HTML, JavaScript, and CSS are compiled into the binary via `include_str!`, with no external CDN or build step at deployment time.
```

Offline or restricted-network deployments and CSP authors are incorrectly told the browser has no external asset requests; actual typography uses fallbacks when Google Fonts is blocked.

**`console/src/console-host.css:5`**

```text
@import url("https://fonts.googleapis.com/css2?family=Inter+Tight:wght@400;500;600;700&family=IBM+Plex+Mono:wght@400;500;600&display=swap");
```

The stock stylesheet requests a remotely hosted font stylesheet. The identical import remains at line 1 of both console/dist/console-app.css and meerkat-mobkit/console-dist/console-app.css (verified by reading the bytes), so this is not only a development-only source import.

### Independent adjudication

The undated deployment claim in console.mdx:30 is too broad. I checked both the source stylesheet and the actual embedded CSS, not merely an unused development file. Both built copies begin with a Google Fonts import, and http_console.rs embeds and serves that CSS. The application assets themselves are embedded and do not need a deployment-time build; this is an optional typography network dependency, not proof that the application cannot operate offline.

**`console/src/console-host.css:5,19-23`**

```text
@import url("https://fonts.googleapis.com/css2?family=Inter+Tight:wght@400;500;600;700&family=IBM+Plex+Mono:wght@400;500;600&display=swap");
font-family: "Inter Tight", "Inter", -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
```

The remote stylesheet and local font fallback stack are both explicit.

**`meerkat-mobkit/console-dist/console-app.css:1`**

```text
@import"https://fonts.googleapis.com/css2?family=Inter+Tight:wght@400;500;600;700&family=IBM+Plex+Mono:wght@400;500;600&display=swap";
```

A read-only Node fs.readFileSync(...).slice(0,205) check found this same prefix in console/dist/console-app.css and the embedded copy.

**`meerkat-mobkit/src/http_console.rs:243-245,451-458`**

```text
const CONSOLE_FRONTEND_APP_CSS: &str = include_str!("../console-dist/console-app.css");
"/console/assets/console-app.css",
```

This is the stylesheet shipped through the console asset route.

**Required correction:** At console.mdx:30 retain that HTML, JavaScript and CSS are embedded and require no deployment-time build. Replace the zero-CDN assertion with: the stock CSS requests optional Google Fonts; when unavailable the configured local/system fallback fonts are used. Do not remove font imports or change assets in this documentation-only task.

### Changes and final verification

**Changed:** `docs/guides/console.mdx`.

Retained embedded HTML/JS/CSS and no deployment-time build, while documenting optional Google Fonts requests and local/system font fallback instead of claiming no external CDN.

**Validation:** Read console/src/console-host.css:5,19-23; read-only assertions confirmed the Google Fonts import in both built CSS copies and the corrected fallback wording.

**Final review: pass.** The guide preserves embedded application assets and the absence of a deployment-time build while explicitly disclosing optional Google Fonts requests and fallback typography. Independently checked the source CSS, both compiled CSS copies, and the actual include_str!/route wiring; no asset/source change was substituted for the documentation correction.

**`docs/guides/console.mdx:30`**

```text
The stock CSS requests optional Google Fonts; when those requests are unavailable or blocked, it uses the configured local/system fallback fonts.
```

The former zero-CDN guarantee is gone, without claiming the application cannot operate offline.

**`console/src/console-host.css:5`**

```text
@import url("https://fonts.googleapis.com/css2?family=Inter+Tight:wght@400;500;600;700&family=IBM+Plex+Mono:wght@400;500;600&display=swap");
```

The remote request also remains at the beginning of console/dist/console-app.css and meerkat-mobkit/console-dist/console-app.css; read-only prefix assertions passed.

**`console/src/console-host.css:19-20`**

```text
font-family: "Inter Tight", "Inter", -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
```

The implementation provides the advertised local/system fallbacks. http_console.rs:243-245 embeds the application assets.

## F-002: The console SSE example uses a nonexistent event name and frame shape

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/console.mdx:499-500`**

```text
    event: text_delta
    data: {"id":"frame-1","cursor":"console:241","identity":"identity:luka","event":"text_delta","data":{"text":"done"}}
```

A custom EventSource listener copied from the guide subscribes to an event that never arrives, and a parser expecting the shown JSON reads nonexistent fields.

**`meerkat-mobkit/src/http_console.rs:3191-3201`**

```text
fn sse_event_from_timeline_event(event: &ConsoleTimelineEvent) -> Option<Event> {
    let (event_name, id) = match event {
        ConsoleTimelineEvent::SnapshotStarted { .. } => ("snapshot_started", None),
        ConsoleTimelineEvent::ConsoleFrame { frame } => (
            if frame.kind == "frame_updated" {
                "frame_updated"
            } else {
                "console_frame"
            },
            Some(frame.cursor.to_string()),
        ),
```

A text delta is transported as event: console_frame with the cursor as SSE id, not event: text_delta.

**`meerkat-mobkit/src/console_aggregator/types.rs:112-132`**

```text
    pub kind: String,
    pub status: ConsoleFrameStatus,
    pub frame_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<u64>,
    pub payload: Value,
    pub source: ConsoleFrameSource,
```

Wire frame fields are kind and payload, not the frontend-normalized event/data fields. The timeline event is a serde type-tagged envelope containing frame; http_console.rs:3211 serializes the event envelope, not the frontend ConsoleFrame type.

**`packages/console-core/src/network.ts:43-46`**

```text
  if (typeof record.type === "string" && "frame" in record) {
    const frame = timelineFrameToConsoleFrame(record.frame);
```

The actual client first unwraps the type/frame envelope before normalizing kind/payload into its internal event/data representation.

### Independent adjudication

The example appears under 'Receive response frames' as the SSE wire contract, not as a deliberately normalized frontend object. The serializer emits console_frame and serializes the tagged ConsoleTimelineEvent around frame. The client's later normalization explains the erroneous event/data shape but does not make it a valid wire example. Frame updates are a separate event name and should not be flattened into this example.

**`meerkat-mobkit/src/http_console.rs:3191-3218`**

```text
ConsoleTimelineEvent::ConsoleFrame { frame } => (
    if frame.kind == "frame_updated" {
        "frame_updated"
    } else {
        "console_frame"
    },
    Some(frame.cursor.to_string()),
),
let data = match serde_json::to_string(event) {
let mut sse = Event::default().event(event_name).data(data);
```

The actual SSE event name, id and serialized object are selected here.

**`meerkat-mobkit/src/console_aggregator/types.rs:112-133,370-379`**

```text
pub kind: String,
pub status: ConsoleFrameStatus,
pub frame_version: u64,
pub payload: Value,
pub source: ConsoleFrameSource,
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConsoleTimelineEvent {
    SnapshotStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<ConsoleCursor>,
    },
    ConsoleFrame {
        frame: ConsoleFrame,
```

The frame has kind/payload and required metadata; the envelope has type/frame.

**`packages/console-core/src/network.ts:43-48,71-80`**

```text
if (typeof record.type === "string" && "frame" in record) {
    const frame = timelineFrameToConsoleFrame(record.frame);
const payload = "payload" in record ? record.payload : record;
```

Frontend normalization is downstream of receiving the envelope.

**Required correction:** Replace console.mdx:499-500 with an SSE example containing id: console:241, event: console_frame, and data: {"type":"console_frame","frame":{"id":"frame-1","cursor":"console:241","dedupe_key":"example:frame-1","timestamp_ms":1750000000000,"runtime_key":"example","identity":"identity:luka","kind":"text_delta","status":"delivered","frame_version":1,"payload":{"text":"done"},"source":{"kind":"console_event"},"interaction_id":"example-interaction"}}. Explain that event/data is the frontend-normalized representation, not the SSE wire shape. Alternatively abbreviate the metadata only if the snippet is clearly labelled abbreviated. Keep correlation fields inside frame.

### Changes and final verification

**Changed:** `docs/guides/console.mdx`.

Replaced the nonexistent text_delta SSE event/normalized frame example with id: console:241, event: console_frame, and the tagged type/frame envelope containing kind/payload, required metadata, source and interaction_id. Distinguished frontend normalization from the wire.

**Validation:** Read http_console.rs:3191-3218 and console_aggregator/types.rs:112-142,370-389. Parsed the documented data line as JSON and verified all 11 required non-optional ConsoleFrame fields from the current Rust definition, plus correlation metadata and matching SSE id.

**Final review: pass.** The example is now a valid tagged timeline SSE envelope with console_frame as its event, a matching console:241 id/cursor, kind/payload fields, all 11 non-optional ConsoleFrame fields, and interaction_id inside frame. Parsed the JSON and derived required fields directly from the Rust struct. Existing live-terminal versus session-history caveats remain untouched.

**`docs/guides/console.mdx:504-506`**

```text
id: console:241
    event: console_frame
    data: {"type":"console_frame","frame":{"id":"frame-1","cursor":"console:241","dedupe_key":"example:frame-1","timestamp_ms":1750000000000,"runtime_key":"example","identity":"identity:luka","kind":"text_delta","status":"delivered","frame_version":1,"payload":{"text":"done"},"source":{"kind":"console_event"},"interaction_id":"example-interaction"}}
```

This is complete illustrative wire JSON, not the old normalized frontend object.

**`meerkat-mobkit/src/http_console.rs:3194-3200`**

```text
ConsoleTimelineEvent::ConsoleFrame { frame } => (
            if frame.kind == "frame_updated" {
                "frame_updated"
            } else {
                "console_frame"
            },
            Some(frame.cursor.to_string()),
```

The source chooses console_frame and the frame cursor for this kind; lines 3211-3217 serialize the whole event and set the SSE id.

**`meerkat-mobkit/src/console_aggregator/types.rs:370-379`**

```text
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConsoleTimelineEvent {
    SnapshotStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<ConsoleCursor>,
    },
    ConsoleFrame {
        frame: ConsoleFrame,
```

The documented type/frame wrapping matches serde. The normalization explanation also matches console-core/src/network.ts:43-65.

## F-003: mobkit/interact is advertised on the console HTTP RPC plane but is only implemented on unified/stdio RPC

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/console.mdx:535`**

```text
- `mobkit/interact` - identity-addressed interaction that returns a `/console/identity/{identity}/stream` route
```

HTTP clients following the console guide receive Method not found, and the incident pack's advertised proof no longer matches its tests.

**`meerkat-mobkit/src/http_console.rs:8931-8939`**

```text
        _ => response_value(
            response_id,
            None,
            Some(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            }),
        ),
```

The HTTP runtime dispatcher has no mobkit/interact arm anywhere in this file (read-only exact-string search returned no matches), and its unknown-method fallback returns -32601. Its capabilities list likewise does not advertise the method.

**`meerkat-mobkit/src/rpc.rs:3608-3612`**

```text
        "mobkit/interact" => {
            let identity_rt = match identity_ctx {
                Some(ctx) => &ctx.runtime,
                None => return maybe_identity_not_configured(is_notification, response_id),
            };
```

There is an implementation, but on the separate unified/stdio dispatcher, conditional on identity context. This rules out describing the method as absent from all MobKit surfaces.

**`examples/001-incident-command-center-pack/ts_smoke.ts:136-141`**

```text
  // mobkit/interact left the stock console surface in the May 2026 console
  // projection-path migration; mobkit/console/send is the canonical
  // identity-first send (server-owned acknowledgement + canonical frames).
  const sendResult = await callConsoleRpc<{ interaction_id: string; identity: string }>(
    "mobkit/console/send",
```

The supposedly interact-based example actually uses console/send. The incident README also incorrectly says '- stock `mobkit/interact`' at line 8 and claims it is the demonstrated chat method at line 24.

### Independent adjudication

Confirmed specifically for /console/rpc and the incident pack's advertised chat proof. I traced the HTTP handler to its HTTP-specific dispatcher and checked there is no mobkit/interact string in http_console.rs; that dispatcher has its own method-not-found fallback. The unified dispatcher does implement interact, so the finding must not become a claim that MobKit or its SDK lacks the method. The incident smoke actually sends through console/send.

**`meerkat-mobkit/src/http_console.rs:658-693,8931-8939`**

```text
let response_value = Box::pin(handle_console_runtime_rpc_with_visibility(
Some(JsonRpcError {
    code: -32601,
    message: "Method not found".to_string(),
```

The route uses the HTTP dispatcher rather than rpc.rs. A read-only Node string-count check returned zero occurrences of mobkit/interact in this entire file.

**`meerkat-mobkit/src/rpc.rs:3608-3612`**

```text
"mobkit/interact" => {
    let identity_rt = match identity_ctx {
        Some(ctx) => &ctx.runtime,
        None => return maybe_identity_not_configured(is_notification, response_id),
```

The separate unified/stdio operation exists and requires identity context.

**`examples/001-incident-command-center-pack/ts_smoke.ts:136-146`**

```text
// mobkit/interact left the stock console surface in the May 2026 console
// projection-path migration; mobkit/console/send is the canonical
// identity-first send (server-owned acknowledgement + canonical frames).
const sendResult = await callConsoleRpc<{ interaction_id: string; identity: string }>(
  "mobkit/console/send",
```

Executable smoke code matches the HTTP dispatcher, not the README.

**Required correction:** Remove mobkit/interact from console.mdx's primary /console/rpc methods. If mentioning it elsewhere, explicitly identify the separate identity-first unified/stdio surface. Change incident README lines 8 and 24 to identity-addressed mobkit/console/send with server-owned acceptance and timeline streaming, consolidating the adjacent duplicate send bullet if useful. Coordinate the stream URL correction with F-004.

### Changes and final verification

**Changed:** `docs/guides/console.mdx`, `examples/001-incident-command-center-pack/README.md`.

Removed mobkit/interact from the explicitly HTTP /console/rpc method list and from the incident pack's advertised chat proof. Documented identity-addressed mobkit/console/send with server-owned acceptance and timeline streaming.

**Validation:** Read incident ts_smoke.ts:136-147. Assertions confirmed no mobkit/interact in http_console.rs or either corrected doc, while preserving its actual arm in the separate rpc.rs dispatcher.

**Final review: pass.** Removed interact from the explicitly HTTP method list and the incident pack's proof claims, without asserting that the separate unified/stdio API lacks it. Both corrected documents contain zero mobkit/interact occurrences; the HTTP source has none, while the unified dispatcher retains its identity-context-dependent arm.

**`docs/guides/console.mdx:542-544`**

```text
Primary RPC methods on `/console/rpc`:

- `mobkit/console/send` - identity-addressed console send
```

The surface is now explicit and its first send operation is implemented there.

**`examples/001-incident-command-center-pack/README.md:23`**

```text
- identity-addressed chat with server-owned acceptance over `mobkit/console/send` + `/console/identity/{identity}/stream`
```

The executable smoke's actual send/acceptance flow replaces the obsolete claim.

**`examples/001-incident-command-center-pack/ts_smoke.ts:139-147`**

```text
const sendResult = await callConsoleRpc<{ interaction_id: string; identity: string }>(
    "mobkit/console/send",
```

The existing smoke invokes the documented HTTP operation. Independently read http_console.rs:5269-5286 and rpc.rs:3608-3612 to distinguish the two dispatchers.

## F-004: The incident pack proof list names removed or incomplete SSE URLs

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`examples/001-incident-command-center-pack/README.md:28`**

```text
- all-events updates over `/console/events/stream`
```

A reader using either advertised obsolete URL gets a 404 rather than the demonstrated replay/live events.

**`meerkat-mobkit/src/http_console.rs:463-475`**

```text
    let router = Router::new()
        .route("/console/experience", get(console_json_handler))
        .route("/console/modules", get(console_json_handler))
        .route("/console/identities", get(console_identities_handler))
        .route("/console/timeline", get(console_timeline_handler))
        .route(
            "/console/timeline/stream",
            get(console_timeline_stream_handler),
        )
        .route(
            "/console/identity/{identity}/stream",
            get(console_identity_timeline_stream_handler),
        )
```

The actual router mounts aggregate timeline streaming and an identity-parameterized stream. It does not mount /console/events/stream or the README line 24 path /console/identity/stream.

**`examples/001-incident-command-center-pack/ts_smoke.ts:267-273`**

```text
  // Replay checkpoints are aggregate CURSORS (`console:N`) under the 0.5.0
  // replay contract — frame ids 409. The all-events SSE surface is the
  // aggregate `/console/timeline/stream`.
  const checkpointCursor = canonicalTurn.accepted.cursor;
  const cursorSeq = (cursor?: string) => Number((cursor ?? "console:0").split(":")[1] ?? "0");
  const identityReplay = await readSseFrames(
    `${baseUrl}/console/identity/incident-commander/stream`,
```

The smoke explicitly documents and uses the replacement URLs.

### Independent adjudication

Both URLs in the incident README are wrong as routes: one omits the required identity path segment and the other names an obsolete aggregate surface. I checked the actual mounted routes and the smoke's concrete requests; neither unified HTTP composition nor http_sse.rs supplies the two legacy spellings. This is current runnable guidance, not historical prose.

**`meerkat-mobkit/src/http_console.rs:462-477`**

```text
.route(
    "/console/timeline/stream",
    get(console_timeline_stream_handler),
)
.route(
    "/console/identity/{identity}/stream",
    get(console_identity_timeline_stream_handler),
)
```

These are the mounted timeline stream paths.

**`examples/001-incident-command-center-pack/ts_smoke.ts:267-285`**

```text
`${baseUrl}/console/identity/incident-commander/stream`,
const allEventsFrames = await readSseFrames(`${baseUrl}/console/timeline/stream`, {
```

The actual replay proof uses both replacement paths.

**Required correction:** In incident README line 24 use /console/identity/{identity}/stream (or the concrete incident-commander path); replace /console/events/stream at line 28 with /console/timeline/stream. The existing aggregate-stream bullet at line 27 may be consolidated rather than duplicated. F-003 separately owns the method-name correction.

### Changes and final verification

**Changed:** `examples/001-incident-command-center-pack/README.md`.

Corrected the identity stream to /console/identity/{identity}/stream and consolidated aggregate/all-events streaming into /console/timeline/stream.

**Validation:** Compared mounted routes in http_console.rs and incident ts_smoke.ts:267-285; static checks confirmed both current URLs and absence of the obsolete /console/identity/stream and /console/events/stream spellings.

**Final review: pass.** Both obsolete incident-pack URL spellings are removed. The corrected identity-parameterized and aggregate timeline routes agree with the mounted HTTP router and both replay requests in the smoke; duplicate aggregate wording was consolidated without dropping the replay proof.

**`examples/001-incident-command-center-pack/README.md:23-25`**

```text
- identity-addressed chat with server-owned acceptance over `mobkit/console/send` + `/console/identity/{identity}/stream`
- canonical console timeline replay over `mobkit/console/query_timeline`
- aggregate timeline catch-up/live streaming over `/console/timeline/stream`
```

The reader no longer receives a missing-segment or nonexistent route.

**`meerkat-mobkit/src/http_console.rs:468-475`**

```text
.route(
            "/console/timeline/stream",
            get(console_timeline_stream_handler),
        )
        .route(
            "/console/identity/{identity}/stream",
            get(console_identity_timeline_stream_handler),
        )
```

These are the actual routes; incident ts_smoke.ts:272-287 independently exercises both spellings.

## F-005: The two-terminal console development recipe points Vite at the wrong backend port

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/console.mdx:667-671`**

```text
MOBKIT_REF_ADDR=127.0.0.1:63210 MOBKIT_REF_HTTP_MODE=serve \
  ./scripts/repo-cargo run -p meerkat-mobkit --example library_mode_reference

# Terminal 2: start the Vite dev server (proxies API calls to gateway)
cd console && npm run dev
```

The copied development recipe cannot load its initial experience snapshot. Even after correcting the port, the present proxy table is incomplete for timeline/media behavior.

**`console/vite.config.ts:18-28`**

```text
  server: {
    port: 5199,
    proxy: {
      "/console/experience": "http://127.0.0.1:63220",
      "/console/events": "http://127.0.0.1:63220",
      "/console/identity": "http://127.0.0.1:63220",
      "/console/modules": "http://127.0.0.1:63220",
      "/console/rpc": "http://127.0.0.1:63220",
      "/interactions": "http://127.0.0.1:63220",
      "/healthz": "http://127.0.0.1:63220",
    },
```

Every configured backend proxy targets 63220, not 63210. The list additionally omits current /console/timeline, /console/identities and /blobs paths, so matching the port alone must not be claimed to validate every console surface.

**`meerkat-mobkit/examples/library_mode_reference.rs:92-95`**

```text
    let listen_addr =
        std::env::var("MOBKIT_REF_ADDR").unwrap_or_else(|_| "127.0.0.1:3210".to_string());
    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    println!("reference app listening on http://{listen_addr}");
```

The first terminal really binds the explicitly requested 63210; there is no automatic redirection to the Vite target.

### Independent adjudication

The two-terminal recipe is internally inconsistent: the server uses the supplied address, while Vite's proxy targets another port. I independently parsed both files and obtained docPort 63210 and proxyPorts [63220]. The same check found /console/timeline, /console/identities and /blobs unmatched by the configured proxy prefixes. A documentation port fix alone therefore must not promise a fully working timeline/media development setup.

**`console/vite.config.ts:18-28`**

```text
port: 5199,
proxy: {
  "/console/experience": "http://127.0.0.1:63220",
  "/console/events": "http://127.0.0.1:63220",
  "/console/identity": "http://127.0.0.1:63220",
  "/console/modules": "http://127.0.0.1:63220",
  "/console/rpc": "http://127.0.0.1:63220",
  "/interactions": "http://127.0.0.1:63220",
  "/healthz": "http://127.0.0.1:63220",
```

There is no environment-based target override or catch-all console proxy in this configuration.

**`meerkat-mobkit/examples/library_mode_reference.rs:92-101`**

```text
std::env::var("MOBKIT_REF_ADDR").unwrap_or_else(|_| "127.0.0.1:3210".to_string());
let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
if std::env::var("MOBKIT_REF_HTTP_MODE").ok().as_deref() == Some("serve") {
    runtime.serve(listener, decisions).await?;
```

The documented environment variable determines the actual listener.

**Required correction:** Use MOBKIT_REF_ADDR=127.0.0.1:63220 in the recipe. Immediately state that the checked-in Vite proxy currently omits /console/timeline (including /stream), /console/identities and /blobs; full-contract development requires adding those proxy entries to that same backend, or using the embedded console at the gateway URL. Do not modify vite.config.ts in this documentation-only wave.

### Changes and final verification

**Changed:** `docs/guides/console.mdx`.

Aligned the development backend with Vite's port 63220, disclosed missing timeline/identities/blob proxy entries, and described proxy additions or the rebuilt embedded console as full-contract alternatives. Linked the opening hot-reload recipe to this limitation and kept HMR wording specific to Vite.

**Validation:** Read console/vite.config.ts:18-30. Read-only assertions matched the documented backend port to every configured proxy target, verified the three missing prefixes, and checked the limitation. The changed shell recipe passes bash -n.

**Final review: pass.** The recipe now binds 63220, matching every checked-in Vite backend target. It prominently discloses all three missing proxy prefixes and distinguishes Vite HMR from rebuilding the embedded console. The suggested library host uses TestClient, explicitly disables app auth, reads the supplied address, and serves persistently when MOBKIT_REF_HTTP_MODE=serve; the alternative does not silently require undisclosed credentials.

**`docs/guides/console.mdx:677-678`**

```text
MOBKIT_REF_ADDR=127.0.0.1:63220 MOBKIT_REF_HTTP_MODE=serve \
  ./scripts/repo-cargo run -p meerkat-mobkit --example library_mode_reference
```

Port, environment names, crate, example target, and shell syntax match the implementation.

**`docs/guides/console.mdx:684-689`**

```text
The checked-in Vite proxy targets port `63220`, but currently omits
`/console/timeline` (including `/stream`), `/console/identities`, and `/blobs`.
Full-contract development through Vite requires adding proxy entries for
those paths to the same backend. Alternatively, use the embedded console at
`http://127.0.0.1:63220/console` for full-contract validation; rebuild the
production console assets and Rust host to test frontend changes there.
```

The documentation does not mistake the port fix for a completed proxy implementation.

**`console/vite.config.ts:18-28`**

```text
"/console/experience": "http://127.0.0.1:63220",
```

All seven targets use this backend. None of the proxy prefixes matches timeline, identities, or blobs.

**`meerkat-mobkit/examples/library_mode_reference.rs:78-81`**

```text
console: ConsolePolicy {
            require_app_auth: false,
            ..ConsolePolicy::default()
        },
```

Read the address and serve branches at 92-102 and the TestClient construction at 49-51 as well; this local development host does not impose the production auth default.

## F-006: The console theme, geometry and fixed-color defaults are stale

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/console.mdx:429-431`**

```text
The console defaults to dark mode. Set `data-cc-theme="light"` on the root to switch to light mode. The shared components respond to both themes.

The sidebar uses a panel surface background (`rgba(21, 22, 27, 0.82)`) to differentiate from the main area. The body background is `#131316`.
```

Hosts attempting to reproduce or override stock defaults get a different theme and geometry, and assume variant-specific colors are invariant.

**`console/src/ConsoleApp.tsx:720-728`**

```text
  const [theme, setTheme] = React.useState<ConsoleTheme>(() => {
    try {
      return (
        (localStorage.getItem("mobkit-console-theme") as ConsoleTheme) ||
        "light"
      );
    } catch {
      return "light";
    }
```

Without a saved preference or configured override, the console is light. Lines 1971-1994 only apply configured theme/variant defaults when the corresponding localStorage preference is absent.

**`console/src/console-host.css:65-72`**

```text
.cc-theme-scope {
  --cc-workbench-sidebar-width: 260px;
  --cc-workbench-activity-width: 300px;
  --cc-workbench-collapsed-rail-width: 28px;
  --cc-sidebar-safe-top: 6px;
  --cc-sidebar-pad-left: 0;
  --cc-sidebar-pad-right: 0;
}
```

The stock values are activity width 300px, safe top 6px, and left/right padding 0, not the guide's 280px/8px/14px/14px at lines 419-422.

**`packages/console-components/src/styles/tokens.css:97-103`**

```text
  --_cc-workbench-sidebar-width:  var(--cc-workbench-sidebar-width, 260px);
  --_cc-workbench-activity-width: var(--cc-workbench-activity-width, 300px);
  --_cc-motion-duration: var(--cc-motion-duration, 160ms);
  --_cc-motion-ease:     var(--cc-motion-ease, ease);
  --_cc-sidebar-safe-top:  var(--cc-sidebar-safe-top, 10px);
  --_cc-sidebar-pad-right: var(--cc-sidebar-pad-right, 12px);
  --_cc-sidebar-pad-left:  var(--cc-sidebar-pad-left, 12px);
```

Even the reusable-package fallback defaults differ from the table, so it is wrong under either interpretation. The body uses oklch(0.16 0.005 80) in console-host.css:23; #131316 is specifically the graphite-dark canvas token in themes.css:191-203, not a universal background.

### Independent adjudication

The theme and four geometry defaults are stale under both plausible interpretations of the token table. I checked Rust appearance defaults as well as the React initializer: the server's unconfigured appearance does not silently force dark mode. Stock host defaults differ from reusable stylesheet fallbacks, so the correction must label which it reports. Narrow the color fix: the visible console canvas is token-driven, whereas the underlying body does have a literal oklch background; do not replace the wrong body literal with a new universal token claim.

**`console/src/ConsoleApp.tsx:720-730,1971-1994`**

```text
(localStorage.getItem("mobkit-console-theme") as ConsoleTheme) ||
"light"
if (!localStorage.getItem("mobkit-console-theme"))
  setTheme(configuredTheme);
if (!localStorage.getItem("mobkit-console-variant"))
  setVariant(configuredVariant);
```

Unconfigured theme is light, and configured defaults do not replace saved preferences.

**`console/src/panels/Tweaks.tsx:8-15`**

```text
if (stored === "rams" || stored === "terminal" || stored === "graphite") return stored;
return "rams";
```

The unsaved, unconfigured stock variant is Rams.

**`meerkat-mobkit/src/console_config.rs:77-82`**

```text
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleAppearanceConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_theme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_variant: Option<String>,
```

Default appearance options are absent, not dark/graphite.

**`console/src/console-host.css:19-24,59-71`**

```text
background: oklch(0.16 0.005 80);
background: var(--_cc-surface-canvas);
--cc-workbench-activity-width: 300px;
--cc-sidebar-safe-top: 6px;
--cc-sidebar-pad-left: 0;
--cc-sidebar-pad-right: 0;
```

The body, visible canvas and host token overrides are distinct declarations.

**`packages/console-components/src/styles/tokens.css:97-103`**

```text
--_cc-workbench-activity-width: var(--cc-workbench-activity-width, 300px);
--_cc-sidebar-safe-top:  var(--cc-sidebar-safe-top, 10px);
--_cc-sidebar-pad-right: var(--cc-sidebar-pad-right, 12px);
--_cc-sidebar-pad-left:  var(--cc-sidebar-pad-left, 12px);
```

Shared-package fallback geometry is not the same as the stock host overrides.

**`packages/console-components/src/styles/themes.css:191-203`**

```text
.cc-theme-scope[data-cc-variant="graphite"][data-cc-theme="dark"] {
--_cc-surface-canvas-default:      #131316;
```

#131316 belongs to the graphite-dark canvas, not all themes.

**Required correction:** Label the table's Default column as stock console host defaults and correct activity width to 300px, sidebar safe top to 6px, and left/right padding to 0. If describing shared fallback tokens instead, explicitly use 300px/10px/12px/12px. State that unconfigured stock defaults are light/Rams, configuration supplies defaults, and saved preferences take precedence. Replace the fixed sidebar/body color assertions with theme/variant-driven console surface styling; do not imply the literal body declaration is itself token-driven. Retain the data-cc-theme mechanism for shared hosts.

### Changes and final verification

**Changed:** `docs/guides/console.mdx`.

Labeled the token table as stock-host defaults; corrected activity width/safe top/sidebar padding to 300px/6px/0/0. Documented unconfigured light/Rams, saved-preference precedence, theme/variant surface colors and the distinct shared fallback padding.

**Validation:** Read ConsoleApp.tsx:720-730,1971-1994, Tweaks.tsx:8-14, and console-host.css:19-24,59-71. Assertions matched the four table values to host CSS and checked the corrected theme/precedence wording; shared fallback values follow tokens.css:97-103.

**Final review: pass.** The table now reports stock-host defaults, including 300px activity width and 6px/0/0 sidebar geometry; remaining table defaults also match shared tokens. Light/Rams and saved-preference precedence match React initialization/effects. The canvas/body distinction is retained, and shared fallback geometry is not confused with host overrides.

**`docs/guides/console.mdx:421-424`**

```text
| `--cc-workbench-activity-width` | `300px` | Activity rail column width |
| `--cc-sidebar-safe-top` | `6px` | Top padding in the sidebar |
| `--cc-sidebar-pad-left` | `0` | Left padding in the sidebar |
| `--cc-sidebar-pad-right` | `0` | Right padding in the sidebar |
```

These exact values match console-host.css:65-71.

**`docs/guides/console.mdx:431`**

```text
Without configuration or saved preferences, the stock console defaults to light mode and the Rams variant. `console_config.appearance` can supply different defaults; saved user preferences take precedence.
```

This correctly scopes the default and avoids overriding existing user choices.

**`console/src/ConsoleApp.tsx:720-725`**

```text
(localStorage.getItem("mobkit-console-theme") as ConsoleTheme) ||
        "light"
```

Tweaks.tsx:8-14 supplies Rams, and ConsoleApp.tsx:1971-1994 supplies configured defaults only without saved preferences.

**`packages/console-components/src/styles/themes.css:191-203`**

```text
--_cc-surface-canvas-default:      #131316;
```

This declaration is specifically inside graphite-dark selectors. tokens.css:101-103 supplies 10px/12px/12px shared fallbacks; the body still has its separate literal oklch background.

## F-007: The shared-package architecture statement incorrectly says console-core imports no network code

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/console.mdx:713`**

```text
The shared packages (`console-core` and `console-components`) are designed to be consumed by any application — MobKit, meerkat-app, or other hosts. They never import app stores, network code, or platform-specific APIs.
```

Embedders are given an incorrect portability and dependency boundary and may assume the headless package is transport-free.

**`packages/console-core/src/headless.ts:10-18`**

```text
import {
  callConsoleRpc,
  fetchJson,
  queryTimeline,
  sendConsole,
  sendConsoleMultipart,
  subscribeTimelineEvents,
  uploadConsoleBlobMultipart,
} from "./network";
```

The headless controller explicitly imports its package-owned network implementation.

**`packages/console-core/src/network.ts:210-215`**

```text
  const timer = globalThis.setTimeout(() => controller.abort(timeoutReason), timeoutMs);
  try {
    return await fetch(input, {
      ...init,
      signal: controller.signal,
    });
```

This is a concrete HTTP implementation using platform fetch/AbortController, not only abstract transport types. console-core/README.md already correctly describes the network client and host-transport option.

### Independent adjudication

The blanket 'never import ... network code, or platform-specific APIs' statement is demonstrably false for console-core. I checked the actual imports, fetch implementation and injected transport constructor. This does not disprove portability or separation from app-specific stores/Electron. The correction should distinguish shared transport implementation from host-owned transport and UI components rather than declaring the packages unusable outside MobKit.

**`packages/console-core/src/headless.ts:10-18,409-424,482-492`**

```text
} from "./network";
export function createHttpConsoleTransport({
loadExperience: () => fetchJson<ConsoleExperience>(baseUrl, CONSOLE_REST_PATHS.experience, timeout()),
subscribeTimeline: (input, onFrame) => subscribeTimelineEvents(baseUrl, input, onFrame),
export function createMobKitConsoleController({
  transport,
}: {
  transport: MobKitConsoleTransport;
```

The package owns an HTTP/SSE implementation while its controller accepts a host-provided transport.

**`packages/console-core/src/network.ts:202-215`**

```text
const controller = new AbortController();
const timer = globalThis.setTimeout(() => controller.abort(timeoutReason), timeoutMs);
return await fetch(input, {
  ...init,
  signal: controller.signal,
```

These are concrete platform network APIs, not just transport interfaces.

**`packages/console-core/README.md:30-36,46,51-57`**

```text
- Zustand or other app stores
- Electron/Desktop bridge code and app-owned network transport implementations
4. Use `createMobKitConsoleController` with a host transport or HTTP transport for MobKit console command/timeline flows.
this package is currently private and is
not an npm-public API promise.
```

The package README already documents the narrower architectural and publication boundaries.

**Required correction:** Replace the final architecture assertion in console.mdx with: the shared packages avoid app-store and Electron/Desktop-bridge coupling; console-components owns presentation, while console-core includes a MobKit HTTP/SSE transport using web APIs and accepts injected host transports. Do not claim either package uses no platform APIs. Clarify these are private workspace packages rather than promising an independently published package API.

### Changes and final verification

**Changed:** `docs/guides/console.mdx`.

Replaced the false no-network/no-platform-API guarantee with the app-store/Desktop-bridge boundary, presentation ownership, console-core HTTP/SSE web-API implementation and injected host transport option. Identified the packages as private workspace packages.

**Validation:** Read console-core/src/headless.ts:409-427,482-493 and network.ts:202-215; checked both package.json private flags and the documented transport distinction.

**Final review: pass.** The changed architecture paragraph now describes the actual presentation/transport boundary and private workspace status instead of claiming no network or platform APIs. Checked both package manifests, package documentation, concrete HTTP/SSE methods, and the injected controller transport.

**`docs/guides/console.mdx:730`**

```text
`console-components` owns presentation; `console-core` includes a MobKit HTTP/SSE transport using web APIs and a controller that accepts injected host transports. These are private workspace packages, not independently published npm API promises.
```

The portability and publication claims are now appropriately bounded.

**`packages/console-core/src/headless.ts:418-424`**

```text
subscribeTimeline: (input, onFrame) => subscribeTimelineEvents(baseUrl, input, onFrame),
```

createHttpConsoleTransport owns an actual HTTP/SSE implementation, while createMobKitConsoleController at 482-493 accepts MobKitConsoleTransport.

**`packages/console-core/src/network.ts:208-215`**

```text
return await fetch(input, {
      ...init,
      signal: controller.signal,
    });
```

The implementation uses web APIs as documented; both shared console package.json files have private: true.

## F-008: Identical content alone is not sufficient for console-send idempotent replay

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/console.mdx:490-492`**

```text
    `409` / JSON-RPC `-32009 idempotency_conflict`. Reusing a key with
    identical content is a replay: the original acceptance is returned and the
    turn is not run again.
```

Retrying the same text/key after changing handling_mode unexpectedly fails; retrying with a changed origin can execute a second turn instead of replaying the first.

**`meerkat-mobkit/src/console_aggregator/mod.rs:5460-5469`**

```text
    identity: &str,
    origin: &str,
    idempotency_key: &str,
) -> String {
    format!("send:{runtime_key}:{identity}:{origin}:{idempotency_key}")
}

fn send_request_fingerprint(origin: &str, content: &Value, handling_mode: &str) -> String {
    let content_json = serde_json::to_string(content).unwrap_or_default();
    hash_short(&format!("{origin}\n{handling_mode}\n{content_json}"))
```

The deduplication namespace includes runtime, identity and origin; the fingerprint also includes handling_mode. Same text with a different mode conflicts, and changing origin moves to a different dedupe namespace.

**`meerkat-mobkit/src/console_aggregator/mod.rs:1506-1511`**

```text
            if !same_request {
                return Err(ConsoleSendError::IdempotencyConflict(
                    request.idempotency_key,
                ));
            }
            return accepted_replay_from_frame(&existing);
```

The actual acceptance path rejects a fingerprint mismatch before returning the stored acceptance.

### Independent adjudication

Identical content is insufficient, even on the actual identity-first HTTP path rather than only the generic send path cited in wave 1. I checked reserve_identity_first_interaction: it uses the runtime/identity/origin/key namespace and fingerprints the effective handling mode as well as content. Missing handling_mode becomes queue, so omission versus explicit queue is not itself a conflict. This is a concrete retry contract, not a request to document arbitrary implementation details.

**`meerkat-mobkit/src/console_aggregator/mod.rs:1626-1653,1672-1679`**

```text
/// (same key, same origin, content and handling mode) so the caller
/// answers with the original acceptance and does not dispatch the turn
/// again.
.unwrap_or("queue")
let dedupe_key = send_dedupe_key(
    &runtime_key,
    &request.identity,
    &request.origin,
    &request.idempotency_key,
);
if !same_request {
    return Err(ConsoleSendError::IdempotencyConflict(
        request.idempotency_key,
    ));
```

The identity-first reservation enforces the richer equality before returning the original acceptance.

**`meerkat-mobkit/src/console_aggregator/mod.rs:5458-5469`**

```text
format!("send:{runtime_key}:{identity}:{origin}:{idempotency_key}")
fn send_request_fingerprint(origin: &str, content: &Value, handling_mode: &str) -> String {
    let content_json = serde_json::to_string(content).unwrap_or_default();
    hash_short(&format!("{origin}\n{handling_mode}\n{content_json}"))
```

The namespace and request fingerprint explicitly include the fields omitted from the guide's retry rule.

**Required correction:** Explain that an idempotent retry reuses the same key in the same runtime/identity/origin namespace with identical content and effective handling_mode (default queue). Changed content or mode in that namespace returns HTTP 409 / JSON-RPC -32009 idempotency_conflict. A different identity or origin is a different send namespace, not a guaranteed replay. Preserve the no-second-turn statement for an admitted identical replay.

### Changes and final verification

**Changed:** `docs/guides/console.mdx`.

Specified runtime/identity/origin/key retry scope, identical content and effective handling_mode with queue default, conflicts for changed content/mode, and separate namespaces for changed origin/identity.

**Validation:** Read reserve_identity_first_interaction in console_aggregator/mod.rs:1626-1679 and dedupe/fingerprint helpers at 5458-5469. Assertions verified the documented namespace, mode/default, replay and conflict qualifications against current source.

**Final review: pass.** The retry description now includes the full runtime/identity/origin/key namespace, content, and effective handling mode with queue default. It correctly distinguishes conflicts from a different namespace, and preserves original acceptance/no second dispatch only for identical replay. Checked the identity-first reservation rather than relying solely on the generic send path.

**`docs/guides/console.mdx:491-497`**

```text
UUID per send. An idempotent retry reuses the same key in the same
    runtime/identity/origin namespace with identical content and effective
    `handling_mode` (default `queue`). The original acceptance is returned
    and the turn is not run again. Changed content or handling mode in that
    namespace is rejected with HTTP `409` / JSON-RPC
    `-32009 idempotency_conflict`. A different identity or origin is a
    different send namespace, not a guaranteed replay.
```

The former identical-content-only rule has been fully qualified.

**`meerkat-mobkit/src/console_aggregator/mod.rs:1641-1653`**

```text
.unwrap_or("queue")
```

The effective default participates in the fingerprint. The same block passes runtime_key, identity, origin and idempotency_key to the namespace helper.

**`meerkat-mobkit/src/console_aggregator/mod.rs:5463-5469`**

```text
format!("send:{runtime_key}:{identity}:{origin}:{idempotency_key}")
```

The adjacent fingerprint includes origin, mode and serialized content; 1672-1679 rejects mismatch before replay. http_console.rs:1816 and 1837 independently confirm HTTP 409 and RPC -32009.

## F-009: The Flow Editor navigation description predates the library and three-section tabs

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/flow-editor.mdx:107`**

```text
The top rail switches between **FLOWS** and **AGENTS** views and carries the IMPORT, VALIDATE, PUBLISH, DEPLOY PLAN, DEPLOY, and `⚙ SETTINGS` actions.
```

Readers look for outdated navigation and a settings action rather than the full-page SETTINGS section, conflicting with the guide's own deep-link behavior.

**`packages/flow-editor-core/src/shell/outcomes.ts:115-122`**

```text
    sectionTabs: open
      ? [
        { target: "flow-tab", view: "editor", label: shell.flowTabLabel, current: view === "editor" },
        { target: "agents-tab", view: "agents", label: shell.agentsTabLabel, current: view === "agents" },
        { target: "settings-tab", view: "settings", label: shell.settingsTabLabel, current: view === "settings" },
      ]
      : [],
```

When a mob is open there are three actual section tabs; when none is open there are none.

**`meerkat-mobkit/src/mobpack.rs:2335-2337`**

```text
        "flow_tab_label": "FLOW",
        "agents_tab_label": "AGENTS",
        "settings_tab_label": "SETTINGS",
```

The exact shipped labels are FLOW, AGENTS and SETTINGS, not FLOWS plus a floating gear action.

**`flow-editor/src/app.tsx:70-73`**

```text
  // view: "library" (home: the mob registry) | "editor" (FLOW section) |
  // "agents" | "settings". The three section views are tabs of the open
  // mob; the library is where the app starts and needs no tabs.
  const [view, setView] = React.useState("library");
```

The default landing page is the mob library, with navigation into sections only after opening a mob.

### Independent adjudication

The guide's undated navigation summary no longer matches the actual shell. The initialized view is the library; section tabs are conditional on a mob being open and have FLOW/AGENTS/SETTINGS labels. This is not a proposal versus implementation disagreement. Keep the existing deep-link exception and the substantive descriptions of editing capabilities.

**`flow-editor/src/app.tsx:70-73`**

```text
// view: "library" (home: the mob registry) | "editor" (FLOW section) |
// "agents" | "settings". The three section views are tabs of the open
// mob; the library is where the app starts and needs no tabs.
const [view, setView] = React.useState("library");
```

This is the concrete initial state, not merely design commentary.

**`packages/flow-editor-core/src/shell/outcomes.ts:112-121`**

```text
const open = !!mobOpen;
sectionTabs: open
  ? [
    { target: "flow-tab", view: "editor", label: shell.flowTabLabel, current: view === "editor" },
    { target: "agents-tab", view: "agents", label: shell.agentsTabLabel, current: view === "agents" },
    { target: "settings-tab", view: "settings", label: shell.settingsTabLabel, current: view === "settings" },
  ]
  : [],
```

The view model has three opened-mob section tabs and none in the library.

**`meerkat-mobkit/src/mobpack.rs:2333-2337`**

```text
"flow_tab_label": "FLOW",
"agents_tab_label": "AGENTS",
"settings_tab_label": "SETTINGS",
```

These are the shipped labels.

**Required correction:** Replace flow-editor.mdx:107 with library-first navigation (unless a deep link opens a mob), followed by FLOW, AGENTS and SETTINGS sections when a mob is open. Retain accurate authoring actions without portraying SETTINGS as only a floating gear action. Rename the settings bullet at line 115 to the SETTINGS section and preserve its editor/mob/deploy contents.

### Changes and final verification

**Changed:** `docs/guides/flow-editor.mdx`.

Documented library-first navigation with deep-link exception, FLOW/AGENTS/SETTINGS tabs only for an opened mob, and the full SETTINGS section instead of an obsolete floating gear action.

**Validation:** Read flow-editor/src/app.tsx:70-73, flow-editor-core/src/shell/outcomes.ts:112-121 and mobpack.rs:2333-2337. Assertions checked the initial library state, three tab targets and revised prose.

**Final review: pass.** Navigation now matches library-first initialization and opened-mob FLOW/AGENTS/SETTINGS tabs, retaining the deep-link exception and substantive authoring actions. SETTINGS is a section rather than a floating gear action. No UI code or document-authority semantics were changed.

**`docs/guides/flow-editor.mdx:109`**

```text
The editor starts in the mob library unless a deep link opens a mob. An opened mob has **FLOW**, **AGENTS**, and **SETTINGS** section tabs; the library has no section tabs.
```

This matches the initialized view and conditional section-tab model.

**`flow-editor/src/app.tsx:73`**

```text
const [view, setView] = React.useState("library");
```

Actual initialization establishes the default, not only a comment.

**`packages/flow-editor-core/src/shell/outcomes.ts:112-121`**

```text
{ target: "settings-tab", view: "settings", label: shell.settingsTabLabel, current: view === "settings" },
```

sectionTabs is conditional on mobOpen and contains exactly the three documented views; mobpack.rs:2335-2337 provides the shipped labels.

## F-010: The documented Flow Editor facade cardinality is off by one

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`packages/flow-editor-core/README.md:44-48`**

```text
`createMobKitFlowController({ includeTestExports })` assembles the exact
`window.MobKitFlowController` key set the views consume stringly: 381 keys,
plus 3 test-only exports behind `includeTestExports`. Key-set parity with
`flow-editor/test/controller-export-manifest.json` is enforced by
`controller-export-keys.test.cjs` on every change.
```

Maintainers following the extraction contract are told to preserve a nonexistent extra export and are given the wrong expected gate cardinality.

**`flow-editor/test/controller-export-manifest.json:1-389`**

```text
  "testOnlyExports": [
```

Read-only command: node -e 'const m=require("./flow-editor/test/controller-export-manifest.json"); console.log(JSON.stringify({exports:m.exports.length,testOnlyExports:m.testOnlyExports.length}))'. Actual result: {"exports":380,"testOnlyExports":3}; total 383, not 384.

**`flow-editor/test/controller-export-keys.test.cjs:52-56`**

```text
diff("base facade", baseKeys, manifest.exports);
```

The gate compares the ordinary facade to manifest.exports and compares the test-enabled facade to exports plus testOnlyExports earlier in the same test. Thus the counts are not two different supported definitions of the same manifest.

### Independent adjudication

The manifest count is wrong in all three cited documents. I did not rely solely on counting a potentially stale manifest: a read-only Node check extracted the shorthand-only object literal in controller-facade.ts and asserted exact key equality with manifest.exports, then checked the test-gated Object.assign keys. Both checks passed: 380 ordinary and 3 test-only, total 383. No bundle was generated or parity-test execution claimed.

**`packages/flow-editor-core/src/controller-facade.ts:439-440`**

```text
export function createMobKitFlowController({ includeTestExports } = {}) {
  const MobKitFlowController = {
```

The actual object literal contains exactly the manifest's 380 ordinary keys. The includeTestExports block adds buildDocument, authoringFlowForDocument and authoringDocumentFromState.

**`flow-editor/test/controller-export-manifest.json:1-389`**

```text
"testOnlyExports": [
```

node -e 'const m=require("./flow-editor/test/controller-export-manifest.json");console.log(m.exports.length,m.testOnlyExports.length)' returns 380 3; separate source-literal equality assertions passed.

**`flow-editor/test/controller-export-keys.test.cjs:34-39,53-56`**

```text
[...manifest.exports, ...manifest.testOnlyExports].sort(),
diff("base facade", baseKeys, manifest.exports);
```

The test distinguishes ordinary and test-gated facades, rather than defining another supported count.

**Required correction:** Prefer removing duplicated cardinalities and referring to the exact checked-in facade manifest and parity gate in packages/flow-editor-core/README.md:44-48, packages/flow-editor-components/README.md:33-36 and flow-editor.mdx:206. If counts are retained, use 380 ordinary plus 3 test-only, 383 total.

### Changes and final verification

**Changed:** `docs/guides/flow-editor.mdx`, `packages/flow-editor-core/README.md`, `packages/flow-editor-components/README.md`.

Removed duplicated stale facade cardinalities in all three owned occurrences and pointed to the exact ordinary/test-only export manifest and parity gate.

**Validation:** Read-only extraction of the actual controller-facade.ts object and test-gated Object.assign verified exact set equality with controller-export-manifest.json: 380 ordinary and 3 test-only exports. Assertions confirmed all three docs reference the manifest and omit 381/384 claims.

**Final review: pass.** Removed the stale counts in all three documents, replacing them with the exact ordinary/test-only manifest and parity test. Independently extracted the source facade's shorthand keys and test-only Object.assign keys: exact set equality with the manifest, 380 plus 3. This was source verification, not a claim to have built/executed the bundled facade gate.

**`packages/flow-editor-core/README.md:44-49`**

```text
additional test-only exports behind `includeTestExports`. Both sets are
recorded in the checked-in manifest. Key-set parity with
`flow-editor/test/controller-export-manifest.json` is enforced by
`controller-export-keys.test.cjs` on every change.
```

The exact key sets, rather than a copied count, are authoritative.

**`packages/flow-editor-components/README.md:35-37`**

```text
`@flow-editor-core`'s `createMobKitFlowController` (the exact ordinary and
test-only export sets in `flow-editor/test/controller-export-manifest.json`
are pinned by `controller-export-keys.test.cjs`); the shell assigns it once
```

The second stale occurrence is corrected consistently; flow-editor.mdx:212 does the same for the third.

**`flow-editor/test/controller-export-keys.test.cjs:35-39`**

```text
[...manifest.exports, ...manifest.testOnlyExports].sort(),
```

The test-enabled gate compares both sets; line 53 separately compares the base facade. Source extraction at controller-facade.ts:440-828 passed against both sets.

## F-011: Flow Editor SDK wrappers do not inherit the HTTP author's ABAC and standalone deploy gates

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/flow-editor.mdx:103`**

```text
The same JSON-RPC surface is exposed as typed wrappers in both gateway SDKs: `mobpackCreate` / `mobpackValidate` / `mobpackDeploy` and friends on the TypeScript `MobHandle`, and snake_case equivalents (`mobpack_create`, `mobpack_validate`, `mobpack_deploy`, …) in Python. That makes the full authoring loop — create, edit via `apply_operation`, validate, export, deploy — scriptable without the editor UI, under the same capabilities and access-control gates. See [TypeScript SDK](/sdks/typescript) and [Python SDK](/sdks/python).
```

An embedding host can incorrectly assume end-user Flow Editor authorization applies automatically to trusted stdin SDK calls.

**`sdk/typescript/src/runtime.ts:1447-1452`**

```text
    const params: Record<string, unknown> = { document };
    if (execute !== undefined) params.execute = execute;
    return parseMobpackDeployResult(
      await this._runtime._rpc("mobkit/mobpacks/deploy", params),
    );
```

MobHandle calls its gateway transport rather than /flow-editor/rpc. runtime.ts:876-889 builds the request and passes it directly to _transport.sendAsync. Python runtime.py:1737-1748 follows the same runtime._rpc path.

**`meerkat-mobkit/src/rpc.rs:3195-3198`**

```text
        method if MOBPACK_AUTHORING_METHODS.contains(&method) => {
            handle_unified_mobpack_authoring_rpc(runtime, method, &request.params, response_id)
                .await
        }
```

Unified/stdio authoring is routed to the trusted host dispatcher, not the HTTP access gate.

**`meerkat-mobkit/src/rpc.rs:288-289`**

```text
        "mobkit/mobpacks/deploy" => crate::mobpack::deploy_mobpack(params)
            .and_then(|result| serde_json::to_value(result).map_err(|err| err.to_string())),
```

The authoring dispatcher directly executes deploy_mobpack; it has no AccessView/principal or host_mutation_allowed input. The analogous validate arm at lines 267-268 directly calls validate_mobpack.

**`meerkat-mobkit/src/http_flow_editor.rs:306-307`**

```text
    if let Some(error) = flow_editor_rpc_access_violation(access_view.as_ref(), &parsed_request) {
```

The caller's mobpack.author/deploy grants are enforced in the HTTP surface-specific handler. Standalone --allow-host-deploy policy is also an HTTP router policy, not a gateway SDK transport property.

### Independent adjudication

The standard gateway SDK wrappers share authoring operations, not the Flow Editor HTTP authorization boundary. I traced SDK runtime initialization to PersistentTransport, the gateway stdin loop to the unified dispatcher, and its authoring arm to the direct authoring implementation. The HTTP handler separately supplies AccessView and host-deploy policy. This is documentation of distinct trusted-host and HTTP surfaces, not a request to redesign security or a claim that every arbitrary custom transport is ungated.

**`sdk/typescript/src/runtime.ts:649-653,876-889,1443-1452`**

```text
this._transport = new PersistentTransport(this._config.gatewayBin, {
const response = (await this._transport.sendAsync(
await this._runtime._rpc("mobkit/mobpacks/deploy", params),
```

The ordinary MobHandle deploy wrapper goes through the gateway transport, not /flow-editor/rpc.

**`sdk/python/meerkat_mobkit/runtime.py:507-509,1737-1748`**

```text
transport = PersistentTransport(self._config.gateway_bin)
self._transport = transport
raw = await self._runtime._rpc("mobkit/mobpacks/deploy", params)
```

Python has the same standard transport boundary.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:13648-13685`**

```text
break DispatchLoopEnd::StdinClosed;
meerkat_mobkit::rpc::handle_unified_rpc_json_with_live_arc_delivery(
    &runtime,
    &request_line,
```

The stdin request is delivered to the unified dispatcher.

**`meerkat-mobkit/src/rpc.rs:195-201,267-289,3195-3198`**

```text
return handle_mobpack_authoring_rpc_with_runtime(
"mobkit/mobpacks/validate" => crate::mobpack::validate_mobpack(params)
"mobkit/mobpacks/deploy" => crate::mobpack::deploy_mobpack(params)
method if MOBPACK_AUTHORING_METHODS.contains(&method) => {
    handle_unified_mobpack_authoring_rpc(runtime, method, &request.params, response_id)
```

These authoring calls do not pass a caller AccessView or the HTTP host-mutation report.

**`meerkat-mobkit/src/http_flow_editor.rs:195-211,285-307,345-368`**

```text
host_mutation_allowed: allow_host_deploy,
deploy_execute_allowed: allow_host_deploy,
if let Some(error) = flow_editor_rpc_access_violation(access_view.as_ref(), &parsed_request) {
return Some(ACTION_MOBPACK_DEPLOY);
Some(ACTION_MOBPACK_AUTHOR)
```

Standalone opt-in and caller-specific HTTP ABAC are supplied by this separate surface.

**Required correction:** Keep the typed SDK wrapper names and scriptable authoring claim in flow-editor.mdx:103. Replace 'under the same capabilities and access-control gates' with an explicit distinction: standard gateway MobHandle wrappers use trusted host stdio RPC; runtime /flow-editor/rpc applies HTTP authentication and configured ABAC, while standalone HTTP applies its host-deploy opt-in. Hosts exposing SDK authoring to untrusted callers must provide their own admission policy.

### Changes and final verification

**Changed:** `docs/guides/flow-editor.mdx`.

Kept typed authoring wrappers while distinguishing trusted gateway stdio RPC from runtime HTTP auth/ABAC and standalone HTTP host-deploy opt-in. Required host admission policy for SDK operations exposed to untrusted callers.

**Validation:** Read TS runtime.ts:649-653,876-889,1443-1452 and Python runtime.py:507-509,1737-1748; followed rpc.rs:267-289,3195-3198 versus http_flow_editor.rs:195-211,285-307,345-368. Static wording checks passed.

**Final review: pass.** The guide retains typed authoring wrappers but no longer transfers HTTP caller authorization or standalone deployment policy onto trusted gateway stdio. Checked TypeScript/Python transport construction and deploy wrappers, unified authoring dispatch, and the separate HTTP auth/ABAC/host-opt-in path. The new host-admission warning is justified and does not allege that every custom transport is ungated.

**`docs/guides/flow-editor.mdx:105`**

```text
The authorization boundaries differ: standard gateway `MobHandle` wrappers use trusted host stdio RPC, not `/flow-editor/rpc`. The runtime HTTP route applies HTTP authentication and configured ABAC; standalone HTTP applies its host-deploy opt-in. These HTTP gates are not inherited by the SDK wrappers.
```

This explicitly separates operation availability from surface admission policy.

**`sdk/typescript/src/runtime.ts:1448-1452`**

```text
await this._runtime._rpc("mobkit/mobpacks/deploy", params),
```

The standard transport is PersistentTransport at 649-653; Python uses the corresponding construction at runtime.py:507-509 and deploy call at 1747.

**`meerkat-mobkit/src/rpc.rs:288-289`**

```text
"mobkit/mobpacks/deploy" => crate::mobpack::deploy_mobpack(params)
```

The unified authoring arm at 3195-3198 reaches this trusted-host operation without an HTTP AccessView.

**`meerkat-mobkit/src/http_flow_editor.rs:306`**

```text
if let Some(error) = flow_editor_rpc_access_violation(access_view.as_ref(), &parsed_request) {
```

The runtime HTTP path first resolves HTTP auth and then applies per-caller ABAC. The standalone handler at 195-211 passes allow_host_deploy as the host-mutation policy.

## F-012: Deploy prompt forwarding is conditional on the flow input schema and explicit bindings

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/flow-editor.mdx:141`**

```text
The deploy `prompt` setting becomes the `--prompt` run input:
```

Authors of typed-input flows expect a deploy prompt to be supplied even when it is intentionally omitted, or expect it to override an explicit prompt binding.

**`meerkat-mobkit/src/mobpack.rs:13468-13474`**

```text
    if input_bindings.accepts_prompt() && !input_bindings.params.contains_key("prompt") {
        // Single-token `--prompt=<text>` (NOT two tokens). `mob run`'s
        // `--prompt` has no `allow_hyphen_values`, so a two-token value
        // beginning with `-` (e.g. a prompt like `--summarize this`) is
        // rejected by clap as an unexpected argument. The `=` form makes clap
        // take everything after `=` literally, including leading hyphens.
        argv.push(format!("--prompt={prompt}"));
```

The setting only emits a prompt argument if the input schema accepts prompt and the caller did not already supply a prompt parameter.

**`meerkat-mobkit/src/mobpack.rs:27891-27896`**

```text
    fn deploy_argv_omits_prompt_when_typed_input_schema_lacks_prompt_field() {
        // Typed input params materialize `schemas/main-input.json` with
        // `additionalProperties: false`; `--prompt` is rkat sugar for
        // `--param prompt=…`, so injecting it would fail rkat's input
        // validation before anything runs.
```

A dedicated test pins omission for a schema without prompt; the adjacent tests also pin explicit input_params.prompt taking precedence and JSON-encoded --param forwarding.

### Independent adjudication

The prompt sentence is unconditional, but both the argv builder and focused unit-test assertions show two necessary conditions. I checked how schema_fields is derived so the correction need not infer behavior from comments: a nonempty typed inputParams list restricts accepted field names; no such list permits the legacy prompt fallback. This adjudication verifies the local argv contract, not a live rkat deployment.

**`meerkat-mobkit/src/mobpack.rs:13387-13403,13410-13434,13464-13474`**

```text
self.schema_fields
    .as_ref()
    .is_none_or(|fields| fields.contains("prompt"))
.get("input_params")
.or_else(|| params.get("inputParams"))
let encoded = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
argv.extend(["--param".to_string(), format!("{key}={encoded}")]);
if input_bindings.accepts_prompt() && !input_bindings.params.contains_key("prompt") {
    argv.push(format!("--prompt={prompt}"));
```

Explicit input values are JSON-encoded; prompt is only an allowed, non-overriding fallback.

**`meerkat-mobkit/src/mobpack.rs:27890-27912,27928-27968`**

```text
fn deploy_argv_omits_prompt_when_typed_input_schema_lacks_prompt_field() {
!argv.iter().any(|arg| arg.starts_with("--prompt")),
fn deploy_argv_sends_prompt_when_schema_accepts_prompt_key() {
fn deploy_argv_keeps_prompt_for_packs_without_typed_input_params() {
fn deploy_argv_prefers_supplied_prompt_param_over_prompt_flag() {
```

The existing tests explicitly cover all relevant branches; they were read, not run.

**Required correction:** At flow-editor.mdx:141 say that RPC input_params values are forwarded as repeated --param key=<json> bindings. The deploy prompt supplies --prompt=<text> only if the generated input schema permits prompt (including packs without typed input parameters) and input_params did not already bind prompt. Do not claim it overrides explicit bindings or is always forwarded.

### Changes and final verification

**Changed:** `docs/guides/flow-editor.mdx`.

Documented JSON-valued input_params as repeated --param bindings and --prompt=<text> as a fallback only when the input schema accepts prompt and no explicit prompt binding exists.

**Validation:** Read mobpack.rs:13383-13434,13455-13474. Static checks matched the accepts_prompt/contains_key conditional and verified the corrected binding/default prose; no rkat execution was performed.

**Final review: pass.** The deploy prompt is now correctly described as a schema-permitted, non-overriding fallback; RPC values retain JSON encoding via repeated --param arguments. Read schema_fields derivation, accepts_prompt, argv emission, and existing tests for no prompt field, allowed prompt, untyped packs, and explicit prompt binding.

**`docs/guides/flow-editor.mdx:145`**

```text
RPC `input_params` values are forwarded as repeated `--param key=<json>` bindings. The deploy `prompt` setting supplies the fallback `--prompt=<text>` only when the generated input schema permits `prompt` (including packs without typed input parameters) and `input_params` has not already bound `prompt`. It does not override an explicit binding.
```

Both required conditions and JSON value semantics are present.

**`meerkat-mobkit/src/mobpack.rs:13464-13468`**

```text
for (key, value) in &input_bindings.params {
        let encoded = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
        argv.extend(["--param".to_string(), format!("{key}={encoded}")]);
    }
    if input_bindings.accepts_prompt() && !input_bindings.params.contains_key("prompt") {
```

This is the actual argv branch. accepts_prompt at 13399-13402 allows no-schema or a schema containing prompt; line 13474 emits the single-token --prompt=<text> form.

## F-013: The examples index recommends two npm scripts that do not exist

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`examples/README.md:50-53`**

```text
npm run mdm:smoke
npm run mdm:auth-smoke
npm run mdm:browser-smoke
npm run mdm:docker-smoke
```

The advertised smoke sequence fails with Missing script rather than exercising authentication or Docker.

**`examples/package.json:10-20`**

```text
  "scripts": {
    "mdm:smoke": "tsx 004-mdm-console-pack/ts_smoke.ts",
    "mdm:browser-smoke": "node 004-mdm-console-pack/browser_smoke.cjs",
    "mdm:real-target-smoke": "004-mdm-console-pack/scripts/real-target-smoke.sh",
    "mdm:real-target-e2e": "MDM_REAL_TARGET_E2E=1 004-mdm-console-pack/scripts/real-target-smoke.sh",
    "mdm:real-target-multi-e2e": "MDM_REAL_TARGET_E2E=1 MDM_REAL_TARGET_COUNT=2 004-mdm-console-pack/scripts/real-target-smoke.sh",
    "mdm:upgrade-meerkat": "004-mdm-console-pack/scripts/use-meerkat-version.sh",
    "mdm:local-target": "004-mdm-console-pack/scripts/local-target.sh start",
    "mdm:console": "004-mdm-console-pack/scripts/start-console.sh",
    "mdm:gcp-target": "004-mdm-console-pack/scripts/gcp-target.sh start"
  }
```

This is the complete npm script table. Neither mdm:auth-smoke nor mdm:docker-smoke is defined.

### Independent adjudication

The preceding cd examples makes examples/package.json the applicable script table; neither missing name is defined there. A read-only Node query returned MISSING for both names while resolving the neighboring valid scripts. The finding does not require actually invoking npm, installing dependencies, or inventing replacement auth/Docker coverage.

**`examples/package.json:10-20`**

```text
"mdm:smoke": "tsx 004-mdm-console-pack/ts_smoke.ts",
"mdm:browser-smoke": "node 004-mdm-console-pack/browser_smoke.cjs",
"mdm:real-target-smoke": "004-mdm-console-pack/scripts/real-target-smoke.sh",
```

Object lookups for mdm:auth-smoke and mdm:docker-smoke in this complete scripts object were undefined; the three quoted names resolve.

**Required correction:** Remove npm run mdm:auth-smoke and npm run mdm:docker-smoke from examples/README.md:49-54. Retain the existing valid smoke commands. If adding mdm:real-target-smoke, describe actual external-target peer-turn coverage, not authentication or Docker coverage.

### Changes and final verification

**Changed:** `examples/README.md`.

Removed nonexistent mdm:auth-smoke and mdm:docker-smoke commands while retaining the valid local smoke and browser-smoke commands.

**Validation:** Parsed examples/package.json and checked every mdm npm command still documented in examples/README.md exists in its script table.

**Final review: pass.** Removed only the two nonexistent index scripts and kept existing smoke/browser-smoke coverage without renaming it into Docker/auth coverage. Parsed the current package script table and verified every remaining mdm npm invocation in both the index and MDM README resolves.

**`examples/README.md:83-88`**

````text
Run the MDM console pack's local target smoke:

```bash
npm run mdm:smoke
npm run mdm:browser-smoke
```
````

Neither mdm:auth-smoke nor mdm:docker-smoke remains.

**`examples/package.json:10-13`**

```text
"mdm:smoke": "tsx 004-mdm-console-pack/ts_smoke.ts",
    "mdm:browser-smoke": "node 004-mdm-console-pack/browser_smoke.cjs",
```

Both surviving commands are actual scripts in the examples package selected by the preceding cd.

## F-014: The examples quick-run prerequisites install the wrong workspace for their first build step

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`examples/README.md:24-25`**

```text
cd examples && npm install
./001-incident-command-center-pack/examples.sh
```

A fresh checkout following the advertised setup stops before starting the example: without OPENAI_API_KEY it exits immediately, and with credentials it still lacks the console's esbuild dependency; browser-capable setup additionally needs Chromium.

**`examples/001-incident-command-center-pack/examples.sh:90-94`**

```text
echo "[incident-pack] building console assets"
(cd "$ROOT/console" && npm run build --silent)

echo "[incident-pack] ensuring example JS deps"
(cd "$ROOT/examples" && npm install --silent --no-fund --no-audit)
```

The first build runs in console, whose dependencies are not installed by npm install in examples. Packs 002/003/005 also invoke this console build without installing console dependencies.

**`examples/001-incident-command-center-pack/examples.sh:8`**

```text
: "${OPENAI_API_KEY:?Set OPENAI_API_KEY to run the live incident command center pack}"
```

The first pack also requires the provider key before reaching the build, but the examples index's first-pack recipe does not state it. Its later export OPENAI_API_KEY belongs to the separate second-pack recipe.

**`console/build.cjs:3-6`**

```text
const fs = require("node:fs/promises");
const path = require("node:path");
const crypto = require("node:crypto");
const { build } = require("esbuild");
```

esbuild must resolve from console/build.cjs's Node resolution ancestry; examples/node_modules is not an ancestor and there is no root package.json workspace installation. Read-only reproduction in this checkout: createRequire(path.resolve('console/build.cjs')).resolve('esbuild') returned MODULE_NOT_FOUND Cannot find module 'esbuild'.

**`examples/001-incident-command-center-pack/browser_smoke.cjs:255`**

```text
  const browser = await chromium.launch({ headless: true });
```

The browser lane also requires a separately available Playwright Chromium installation, which npm installing example JavaScript dependencies alone does not guarantee.

### Independent adjudication

Confirmed for launch modes that actually build the console, especially the index's first-pack command. Independent module resolution from console/build.cjs returned MODULE_NOT_FOUND for esbuild; examples is a sibling package, not a workspace installation that supplies console's dependencies. The first launcher also explicitly requires OPENAI_API_KEY. Narrow the propagation: pack 002/003 --smoke exit before their console build and do not require Chromium; only browser lanes require an installed browser. No launcher, install or provider-backed run was executed.

**`examples/001-incident-command-center-pack/examples.sh:8,90-101`**

```text
: "${OPENAI_API_KEY:?Set OPENAI_API_KEY to run the live incident command center pack}"
(cd "$ROOT/console" && npm run build --silent)
(cd "$ROOT/examples" && npm install --silent --no-fund --no-audit)
run_leg "browser smoke" node "$PACK_DIR/browser_smoke.cjs" || status=1
```

Provider-key admission and console build precede the example dependency install and browser smoke.

**`console/build.cjs:3-6`**

```text
const { build } = require("esbuild");
```

node:createRequire(path.resolve('console/build.cjs')).resolve('esbuild') returned MODULE_NOT_FOUND. console/package.json declares esbuild; examples/package.json does not install dependencies into console; there is no root package.json.

**`examples/002-foresight-studio-pack/examples.sh:20-26,83-87`**

```text
if [[ "${1:-}" == "--smoke" ]]; then
  exit 0
fi
(cd "$ROOT/console" && npm run build --silent)
```

Console setup is required for live/browser modes, not this early-return structural smoke; pack 003 has the same ordering.

**`examples/005-access-control-pack/examples.sh:64-65`**

```text
echo "[access-control-pack] building console assets"
(cd "$ROOT/console" && npm run build --silent)
```

The deterministic ABAC lane also consumes the separately installed console toolchain.

**`examples/001-incident-command-center-pack/browser_smoke.cjs:255`**

```text
const browser = await chromium.launch({ headless: true });
```

The first pack's browser lane needs the Playwright Chromium binary in addition to the npm package.

**Required correction:** Add a shared prerequisites section to examples/README.md with repository-root commands npm --prefix console ci and npm --prefix examples ci (both lockfiles exist). For browser lanes add (cd examples && npx playwright install chromium), noting platform browser dependencies if applicable. State that the first pack requires OPENAI_API_KEY. Link the relevant pack Run sections (001, 002 live/browser, 003 live/browser, 005) to the shared setup. Do not imply structural --smoke modes or the ABAC HTTP-only smoke themselves need browser installation or provider credentials.

### Changes and final verification

**Changed:** `examples/README.md`, `examples/001-incident-command-center-pack/README.md`, `examples/002-foresight-studio-pack/README.md`, `examples/003-swarm-stress-pack/README.md`, `examples/005-access-control-pack/README.md`.

Added root-relative console/examples npm ci prerequisites, optional Playwright Chromium installation for browser lanes, provider/Python requirements for the first live pack, and links from the relevant pack Run sections. Preserved structural-smoke and HTTP-only ABAC exceptions; clarified root versus examples cwd.

**Validation:** Read launcher build/early-exit branches for packs 001/002/003/005 and the incident browser/TS/Python legs. Confirmed both lockfiles exist, all four prerequisite links and their anchors resolve, and the install/browser setup snippets parse with bash -n. No dependency installation or launcher execution.

**Final review: pass.** Shared setup now installs console and example dependencies separately with matching committed lockfiles, installs Chromium only for browser lanes, and states the incident pack's provider/Python prerequisites. All four required pack links resolve. The 002/003 early --smoke branches precede console builds; their launchers install/build the SDK themselves. ABAC remains credential-free and its HTTP smoke remains browser-free. Incident launch paths are now explicitly root-relative.

**`examples/README.md:26-29`**

````text
```bash
npm --prefix console ci
npm --prefix examples ci
```
````

Both lockfiles exist and their root dependency specifications match their manifests. examples/node_modules is not relied on to resolve console/build.cjs dependencies.

**`examples/README.md:37-47`**

```text
(cd examples && npx playwright install chromium)
```

The surrounding prose limits Chromium to browser lanes and identifies incident OPENAI_API_KEY/Python, topology no-provider, swarm Gemini keys, and ABAC no-provider distinctions.

**`examples/001-incident-command-center-pack/examples.sh:90-102`**

```text
(cd "$ROOT/console" && npm run build --silent)
```

The console build precedes the browser/TypeScript/Python legs. Line 8 requires OPENAI_API_KEY; python_smoke.py:2-5 imports only standard-library modules.

**`examples/003-swarm-stress-pack/examples.sh:14-26`**

```text
if [[ "${1:-}" == "--smoke" ]]; then
  exit 0
fi
```

SDK installation/build and structural smoke occur before this exit, whereas the console build is after it. Pack 002 has the same ordering. Pack 005 builds console but invokes only node smoke.mjs in its smoke branch.

## F-015: The swarm persistence command silently changes its assumed working directory

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`examples/003-swarm-stress-pack/README.md:51-53`**

````text
```bash
MOBKIT_KEEP_EXAMPLE_STATE=1 npx tsx ./003-swarm-stress-pack/run.ts --skip-build
```
````

Copying the restart command from the same shell used for the preceding commands fails to locate the entrypoint.

**`examples/003-swarm-stress-pack/README.md:41-43`**

````text
```bash
./examples/003-swarm-stress-pack/examples.sh --autoburst --kickoff
```
````

Every earlier README command is repository-root relative, and no cd examples occurs between them and the persistence command.

**`examples/003-swarm-stress-pack/run.ts:366-371`**

```text
async function main() {
  const args = new Set(process.argv.slice(2));
  const once = args.has("--once") || args.has("--smoke");
  const kickoff = args.has("--kickoff");
  const autoBurst = args.has("--autoburst");
  const skipBuild = args.has("--skip-build");
```

The actual entrypoint is under examples/003-swarm-stress-pack. A read-only pathlib check of ./003-swarm-stress-pack/run.ts from repository root returned false, while the actual file exists. The launcher changes cwd only in a child process and cannot change the user's shell.

### Independent adjudication

The swarm README consistently uses repository-root launcher paths until this snippet. No preceding shell cd explains the shorter entrypoint path, and launcher-internal subshells cannot alter the caller's cwd. Independent fs.existsSync checks returned false for 003-swarm-stress-pack/run.ts and true for examples/003-swarm-stress-pack/run.ts from the repository root.

**`examples/003-swarm-stress-pack/README.md:41-53`**

```text
./examples/003-swarm-stress-pack/examples.sh --autoburst --kickoff
MOBKIT_KEEP_EXAMPLE_STATE=1 npx tsx ./003-swarm-stress-pack/run.ts --skip-build
```

The second command silently assumes a different directory from the first.

**`examples/003-swarm-stress-pack/examples.sh:81-82`**

```text
(cd "$ROOT/examples" && npx tsx "$PACK_DIR/run.ts" "$@")
```

The launcher's own cwd change happens only in a child shell.

**Required correction:** Make the snippet self-contained from the repository root: (cd examples && MOBKIT_KEEP_EXAMPLE_STATE=1 npx tsx ./003-swarm-stress-pack/run.ts --skip-build). Preserve the existing state-retention purpose; --skip-build assumes an already built gateway.

### Changes and final verification

**Changed:** `examples/003-swarm-stress-pack/README.md`.

Made the retained-state command self-contained from the repository root using an explicit cd examples subshell and stated the already-built-gateway prerequisite for --skip-build.

**Validation:** Verified examples/003-swarm-stress-pack/run.ts exists, the corrected command includes the required cwd, and bash -n accepts the snippet.

**Final review: pass.** The retained-state command now changes cwd in an explicit examples subshell, preserving local tsx resolution and finding the actual entrypoint from the same repository-root context as the other commands. Its newly stated --skip-build prerequisite matches the entrypoint flag.

**`examples/003-swarm-stress-pack/README.md:58-61`**

```text
(cd examples && MOBKIT_KEEP_EXAMPLE_STATE=1 npx tsx ./003-swarm-stress-pack/run.ts --skip-build)
```

The path exists relative to the chosen cwd and the command passes bash -n.

**`examples/003-swarm-stress-pack/run.ts:366-371`**

```text
const skipBuild = args.has("--skip-build");
```

This entrypoint consumes the exact flag and passes it to ensureGatewayBin at line 425.

## F-016: The swarm README describes unconditional topology rebuilding despite a restored-edge fast path

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`examples/003-swarm-stress-pack/README.md:55-58`**

```text
The dense topology is intentionally reapplied on restart. If you set
`MOBKIT_SWARM_SKIP_DENSE=1`, the current runtime restores the 540-agent roster
but can project zero `wired_to` edges until a proper topology restore path exists;
that mode is useful only for isolating history loading from edge reconciliation.
```

Operators diagnosing expensive restarts or missing edges are told an old rebuild/restore limitation rather than the condition actually used by the example.

**`examples/003-swarm-stress-pack/run.ts:443-452`**

```text
    const handle = runtime.mobHandle();
    let members = await handle.listMembers();
    const restoredEdgeCount = countUndirectedWires(members);
    const shouldApplyDense =
      !skipDense &&
      !(
        keepState &&
        hadPersistentState &&
        restoredEdgeCount >= DENSE_RESTORE_EDGE_FLOOR
      );
```

Dense topology application is explicitly skipped when retained state has enough restored edges, not performed on every restart.

**`examples/003-swarm-stress-pack/run.ts:476-480`**

```text
    } else {
      const reason = skipDense
        ? "requested by MOBKIT_SWARM_SKIP_DENSE"
        : `restored ${restoredEdgeCount} existing topology edges`;
      console.log(`[swarm-stress] skipping dense topology reapply (${reason})`);
```

The code exposes restored edges as a supported reason for not rebuilding. The threshold at lines 66-68 is floor(BASE_TOTAL * PEER_FANOUT_PER_PARENT / 2), or 22500 with the current constants.

### Independent adjudication

Confirmed narrowly as incorrect unconditional documentation of the example's restart decision and an overgeneralized roster claim. I evaluated the actual shouldApplyDense expression read from source: with retained recognized state, 22499 restored edges yields true and 22500 yields false. This proves the source conditional, not that a live 540-agent restart was successfully performed or that every upstream restore path works. The possibility of zero edges need not be denied; it simply must not be presented as the only/current universal behavior.

**`examples/003-swarm-stress-pack/run.ts:391-404,443-480`**

```text
const restoreBurst =
  process.env.MOBKIT_SWARM_RESTORE_BURST === "1" ||
  (process.env.MOBKIT_SWARM_RESTORE_BURST !== "0" &&
    keepState &&
    hadPersistentState);
const shouldApplyDense =
  !skipDense &&
  !(
    keepState &&
    hadPersistentState &&
    restoredEdgeCount >= DENSE_RESTORE_EDGE_FLOOR
  );
if (keepState && hadPersistentState) {
  deferredDenseApply = applyDense;
```

Burst roster selection and topology rebuild decisions are separate. Reapplication has a restored-edge fast path and can be deferred.

**`examples/003-swarm-stress-pack/run.ts:66-68,421-435`**

```text
const DENSE_RESTORE_EDGE_FLOOR = Math.floor(
  (BASE_TOTAL * PEER_FANOUT_PER_PARENT) / 2,
);
const skipDense = process.env.MOBKIT_SWARM_SKIP_DENSE === "1";
if (!skipDense) {
  builder = builder.topologyProvider(topologyProvider);
```

The flag also suppresses installing the dense topology provider. Current scenario constants are 300 baseline agents and 150 peers, producing a 22500-edge threshold.

**`examples/003-swarm-stress-pack/run.ts:517-523`**

```text
const denseApply = deferredDenseApply?.().catch((error: unknown) => {
if (denseApply && (autoBurst || kickoff)) {
  await denseApply;
```

Retained-state rebuilding starts later and is awaited before requested autoburst/kickoff.

**Required correction:** Describe the conditional restart algorithm: if keep-state is set, recognized persistent state exists and restored edge count reaches DENSE_RESTORE_EDGE_FLOOR (22500 currently), skip dense reapplication; otherwise apply it, deferred on retained-state boots. MOBKIT_SWARM_SKIP_DENSE=1 bypasses the dense provider/application regardless of restored count. State that roster size depends separately on burst restoration configuration and retained state. Do not promise successful topology restoration, zero edges, or exactly 540 agents solely from this flag.

### Changes and final verification

**Changed:** `examples/003-swarm-stress-pack/README.md`.

Replaced unconditional topology rebuilding/540-agent claims with the retained-state/restored-edge threshold fast path, deferred dense application, independent skip-dense control and separate burst-roster selection. Avoided guaranteeing topology/session restore success.

**Validation:** Read run.ts:66-68,391-404,421-480,517-523. Static checks verified DENSE_RESTORE_EDGE_FLOOR, restored-edge comparison, conditional provider installation, burst flag names and the qualified documentation.

**Final review: pass.** The restart explanation now matches the real conditional: retained recognized state plus at least 22,500 restored undirected edges skips dense reapplication, otherwise it applies/defer-applies unless skip-dense is selected. The latter also bypasses topology-provider installation. Burst-roster selection is documented independently with the correct 1/0/default behavior, and no successful 540-agent or history/topology restore is promised.

**`examples/003-swarm-stress-pack/README.md:61-66`**

```text
`--skip-build` assumes the gateway is already built. Dense topology reapplication
is skipped when keep-state is enabled, recognized persistent state exists, and
the restored undirected edge count reaches `DENSE_RESTORE_EDGE_FLOOR`
(currently 22,500). Otherwise the example reapplies dense wiring, deferring it
on recognized retained-state boots; requested autoburst/kickoff waits for that
deferred application.
```

The following paragraph separately qualifies MOBKIT_SWARM_SKIP_DENSE and roster restoration.

**`examples/003-swarm-stress-pack/run.ts:446-452`**

```text
const shouldApplyDense =
      !skipDense &&
      !(
        keepState &&
        hadPersistentState &&
        restoredEdgeCount >= DENSE_RESTORE_EDGE_FLOOR
      );
```

The source predicate matches the prose. scenario.ts:2,6,9-10 and run.ts:66-68 give floor(300*150/2)=22500.

**`examples/003-swarm-stress-pack/run.ts:400-404`**

```text
const restoreBurst =
    process.env.MOBKIT_SWARM_RESTORE_BURST === "1" ||
    (process.env.MOBKIT_SWARM_RESTORE_BURST !== "0" &&
      keepState &&
      hadPersistentState);
```

Roster restoration is separate. Provider bypass at 434-436, deferral at 468-474, and waiting at 517-523 also match the corrected text.

## F-017: The MDM README instructs readers to downgrade the entire workspace to a nonexistent current 0.8.2 pin

**Severity:** high. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`examples/004-mdm-console-pack/README.md:146-153`**

````text
The pack is pinned to Meerkat 0.8.2. Re-apply and validate the pin
with:

```bash
cd examples
npm run mdm:upgrade-meerkat -- 0.8.2
npm run mdm:real-target-smoke
```
````

The instruction is factually wrong and, when the helper's registry check admits the requested version, rewrites shared dependencies far backwards rather than reapplying the checked-in pin. The helper can also refuse immediately because its cargo search check only observes the latest visible release.

**`meerkat-mobkit/Cargo.toml:59-64`**

```text
meerkat-core = { version = "=0.8.40" }
meerkat-client = { version = "=0.8.40" }
meerkat-comms = { version = "=0.8.40" }
meerkat-contracts = { version = "=0.8.40" }
meerkat-mob = { version = "=0.8.40" }
meerkat-mob-mcp = { version = "=0.8.40" }
```

The pack is a Cargo example of the current crate, not an independently pinned project; the remaining direct Meerkat dependencies also pin =0.8.40.

**`examples/004-mdm-console-pack/scripts/use-meerkat-version.sh:14`**

```text
crate_manifest="${repo_root}/meerkat-mobkit/Cargo.toml"
```

The documented command operates on the repository's main Rust crate manifest, not a private example-only manifest. Lines 18-36 collect all direct Meerkat normal/dev dependencies; lines 49-50 replace their version strings; lines 90-92 update each package through repo-cargo.

### Independent adjudication

The current-pin claim and 're-apply' command are wrong, but the wave-1 title/impact overstate what was demonstrated. Version 0.8.2 is a historical version, not a nonexistent release. The helper targets the crate's shared manifest and would change its direct Meerkat family if its registry precheck passes; it may instead refuse before mutation. I did not execute it or query live registry state. Treat this as a medium-severity stale setup/version instruction rather than a proved unconditional workspace downgrade. Preserve historical acceptance evidence.

**`meerkat-mobkit/Cargo.toml:59-88`**

```text
meerkat-core = { version = "=0.8.40" }
meerkat-mob = { version = "=0.8.40" }
meerkat = { version = "=0.8.40", features = ["comms", "skills", "mcp", "memory-store", "memory-store-session", "jsonl-store", "session-store", "sqlite-store", "live", "openai-realtime"] }
meerkat-tools = { version = "=0.8.40" }
```

The pack uses the current crate dependency family, not a separate 0.8.2 package.

**`examples/004-mdm-console-pack/scripts/use-meerkat-version.sh:14-35,43-50,87-89`**

```text
crate_manifest="${repo_root}/meerkat-mobkit/Cargo.toml"
if ! cargo search meerkat --limit 1 | grep -F "meerkat = \"${version}\"" >/dev/null; then
  echo "meerkat ${version} is not visible on crates.io yet" >&2
  exit 1
fi
./scripts/repo-cargo update -p "$crate" --precise "$version"
```

The helper enumerates normal/dev Meerkat dependencies, then rewrites the main manifest's versions and updates each package, but only after the registry guard.

**Required correction:** Replace the present-tense 0.8.2 pin/reapply paragraph with use of the repository's pinned Meerkat dependency family (0.8.40 at this baseline) and ordinary smoke validation without dependency mutation. Explain that mdm:upgrade-meerkat is an intentional shared dependency-update helper, not example setup. Retain 0.8.2 acceptance as explicitly historical; do not relabel the old paid/provider acceptance as a newly verified 0.8.40 result.

### Changes and final verification

**Changed:** `examples/004-mdm-console-pack/README.md`.

Replaced current 0.8.2 pin/reapply guidance with the repository's =0.8.40 dependency family and non-mutating smoke validation. Explained the shared dependency-update helper and marked old provider acceptance explicitly historical without upgrading its evidence.

**Validation:** Read meerkat-mobkit/Cargo.toml:59-88 and use-meerkat-version.sh:14-50,87-89. Assertions matched the documented current pin to the manifest, removed the old downgrade command, and verified historical/helper caveats. No dependency mutation or live registry check.

**Final review: pass.** The README now uses the actual shared =0.8.40 pins and a smoke-only validation command, not the obsolete dependency-mutation setup step. It accurately identifies the helper's normal/dev manifest and lockfile mutation after a registry precheck. The 0.8.2 result remains explicitly historical and is not relabeled as current provider acceptance.

**`examples/004-mdm-console-pack/README.md:154-156`**

```text
The pack uses the repository's pinned Meerkat dependency family in
`meerkat-mobkit/Cargo.toml` (currently `=0.8.40`), not an independent pack pin.
Validate the checked-in dependencies without changing them:
```

The following command only runs mdm:real-target-smoke.

**`examples/004-mdm-console-pack/README.md:173-175`**

```text
Historical acceptance: `mdm:real-target-smoke` and the credential-gated
`mdm:real-target-e2e` passed on the published Meerkat 0.8.2 line. That result
does not establish acceptance of the current dependency pin;
```

Historical evidence is preserved without inventing a new acceptance run.

**`meerkat-mobkit/Cargo.toml:59-64`**

```text
meerkat-core = { version = "=0.8.40" }
```

Read the complete direct family at 59-88. use-meerkat-version.sh:14,24-35,43-50,87-89 confirms shared manifest selection, registry guard, rewrites and precise Cargo updates.

## F-018: The MDM pre-existing-binding run recipe forces a demo Hive while promising real peer queries

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`examples/004-mdm-console-pack/README.md:107-113`**

````text
./004-mdm-console-pack/examples.sh --run --targets ./004-mdm-console-pack/target-bindings.json
```

Open the printed `/console` URL. The roster should show `Hive` plus the remote
targets from the binding file. Asking the hive to query hardware should produce
peer messages to the target members; if the timeline only shows local metadata,
that is a MobKit/Meerkat remote-support gap.
````

Operators misdiagnose deterministic demo answers as a remote comms regression even with valid target bindings and credentials.

**`examples/004-mdm-console-pack/examples.sh:32-34`**

```text
  --run)
    npx tsx 004-mdm-console-pack/run.ts --demo-llm --wait "$@"
    ;;
```

The advertised --run branch unconditionally supplies --demo-llm.

**`examples/004-mdm-console-pack/run.ts:561-563`**

```text
  const useDemoLlm =
    Boolean(args["demo-llm"]) ||
    (!Boolean(args["real-llm"]) && !process.env.OPENAI_API_KEY);
```

An explicit demo flag wins even if OPENAI_API_KEY or --real-llm is present; line 586 applies builder.demoLlm(). Thus the recipe cannot establish the real model/tool behavior described immediately afterward.

**`examples/004-mdm-console-pack/scripts/start-console.sh:24-25`**

```text
cd "$examples_dir"
exec npx tsx 004-mdm-console-pack/run.ts --targets "$targets_file" --wait "$@"
```

The existing start-console helper is the supported path that forwards explicit --real-llm without injecting the conflicting demo flag.

### Independent adjudication

The README's specific pre-existing-binding recipe forces demo mode yet immediately asks readers to use Hive answers as evidence of real target querying. I traced the shell branch and evaluated the source selection expression: demo=true stays true with both an API key and real=true. The valid alternative helper does not add the conflicting demo flag. This disproves the recipe as a provider-backed proof; it does not imply remote bindings themselves fail in demo mode.

**`examples/004-mdm-console-pack/examples.sh:29-34`**

```text
--console)
  004-mdm-console-pack/scripts/start-console.sh "$@"
  ;;
--run)
  npx tsx 004-mdm-console-pack/run.ts --demo-llm --wait "$@"
```

The documented --run path injects demo mode unconditionally.

**`examples/004-mdm-console-pack/run.ts:561-563,586`**

```text
const useDemoLlm =
  Boolean(args["demo-llm"]) ||
  (!Boolean(args["real-llm"]) && !process.env.OPENAI_API_KEY);
if (useDemoLlm) builder = builder.demoLlm();
```

Read-only evaluation of this actual expression confirmed explicit demo wins over credentials and --real-llm.

**`examples/004-mdm-console-pack/scripts/start-console.sh:8-15,17-25`**

```text
if [[ "${1:-}" == "--targets" ]]; then
  targets_file="$2"
  shift 2
cd "$examples_dir"
exec npx tsx 004-mdm-console-pack/run.ts --targets "$targets_file" --wait "$@"
```

This helper checks the binding file and forwards --real-llm without adding --demo-llm. Use it from examples or with an absolute binding path to avoid cwd ambiguity.

**Required correction:** For the claimed hardware-query workflow, keep cd examples, require configured provider credentials, and use ./004-mdm-console-pack/scripts/start-console.sh --targets ./004-mdm-console-pack/target-bindings.json --real-llm. Keep the real target/provider/tool prerequisites. If examples.sh --run remains documented, label it demo/shape-only and remove any inference that its local/demo replies alone prove a remote comms regression.

### Changes and final verification

**Changed:** `examples/004-mdm-console-pack/README.md`.

Changed the real hardware-query recipe to start-console.sh --targets ... --real-llm with credentials and target-tool prerequisites. Explicitly labeled examples.sh --run as forced demo mode and removed the inference that metadata-only demo answers establish a bridge regression.

**Validation:** Read examples.sh --run branch, run.ts:561-563,586, and start-console.sh:8-25. Confirmed the chosen helper preserves the binding-file cwd and passes --real-llm without injecting --demo-llm; bash -n accepted the new recipe.

**Final review: pass.** The real-query recipe now selects the helper that does not inject demo mode, supplies --real-llm, and explicitly calls for Hive/target credentials and target tools. The cd examples context keeps the relative binding path valid both before and after the helper's own cwd change. The forced-demo launcher is correctly labeled shape-only, and metadata-only output is no longer automatically diagnosed as a bridge failure.

**`examples/004-mdm-console-pack/README.md:106-110`**

````text
```bash
cd examples
npm install
export OPENAI_API_KEY=...
./004-mdm-console-pack/scripts/start-console.sh --targets ./004-mdm-console-pack/target-bindings.json --real-llm
````

This command chooses the non-demo helper and the correct working directory; bash -n passed.

**`examples/004-mdm-console-pack/scripts/start-console.sh:24-25`**

```text
cd "$examples_dir"
exec npx tsx 004-mdm-console-pack/run.ts --targets "$targets_file" --wait "$@"
```

The helper preserves additional arguments without appending --demo-llm.

**`examples/004-mdm-console-pack/run.ts:561-563`**

```text
const useDemoLlm =
    Boolean(args["demo-llm"]) ||
    (!Boolean(args["real-llm"]) && !process.env.OPENAI_API_KEY);
```

Explicit real mode with no demo flag selects the real provider; examples.sh:32-34 still forces demo and is separately documented as such. The default Hive model is gpt-5.5 at config/mob.toml:33-34.

## F-019: The zero-rewrite fixture documents acceptance although its regression test requires typed refusal

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`meerkat-mobkit/tests/fixtures/README.md:17-19`**

```text
  019f2bdc-a781-7060-bff8-0b97b7a4fcee). Used by the zero-rewrite
  import-on-load regression pinning the meerkat d6cafd405 acceptance and
  its strictness.
```

A maintainer can invert the regression's intended behavior or assume the fixture proves a migration path which the test explicitly forbids.

**`meerkat-mobkit/src/identity_first/adapters.rs:3966-3974`**

```text
    /// Released 0.8.10 ZERO-REWRITE history (the universal mob-supervisor
    /// shape: a transcript graph with one revision and zero commits) is
    /// REFUSED TYPED by the shipping 0.8.11 strict importer, through our
    /// adapter load path, with the durable row left untouched - a load
    /// error scoped to that one session, never a crash or a store-wide
    /// failure. The importer follow-ups that would have accepted this shape
    /// (d6cafd405 and successors) deliberately did NOT ship; this test pins
    /// the shipping contract so a later "fix" cannot silently turn the
    /// refusal into a panic or an adoption.
```

The fixture consumer explicitly says the cited acceptance did not ship and names the opposite invariant.

**`meerkat-mobkit/src/identity_first/adapters.rs:4038-4045`**

```text
        // The strict importer refuses the zero-rewrite graph as a TYPED load
        // error for this session - not a panic, not an adoption.
        let error = meerkat::SessionStore::load(&adapter, &session_id)
            .await
            .expect_err("the shipping strict importer must refuse a zero-rewrite graph");
        assert!(
            error.to_string().contains("import"),
```

The executable test expects an import error. The following assertions preserve released bytes unchanged and prove an unrelated session remains usable.

### Independent adjudication

The README describes what this fixture's current regression pins, so the historical provenance exemption does not protect a reversed test expectation. The consumer loads these exact bytes and uses expect_err, then asserts byte preservation and saves an unrelated session. I did not run a SQLite/tempfile test or establish a new upstream runtime acceptance result. The correction should describe the regression's asserted contract and preserve all capture provenance and bytes.

**`meerkat-mobkit/src/identity_first/adapters.rs:3966-3985`**

```text
/// failure. The importer follow-ups that would have accepted this shape
/// (d6cafd405 and successors) deliberately did NOT ship;
async fn released_zero_rewrite_history_refuses_typed_on_adapter_load() {
    const RELEASED: &[u8] =
        include_bytes!("../../tests/fixtures/v0_8_10_zero_rewrite_supervisor_session.json");
```

This is the fixture consumer, and it explicitly distinguishes the unshipped acceptance follow-up.

**`meerkat-mobkit/src/identity_first/adapters.rs:4038-4068`**

```text
.expect_err("the shipping strict importer must refuse a zero-rewrite graph");
error.to_string().contains("import"),
assert_eq!(
    preserved, RELEASED,
    "the refusal must leave the released bytes byte-identical"
);
.expect("an unrelated session must keep working on the same store");
```

The executable assertions pin import refusal, unchanged fixture-derived storage bytes and unrelated-session usability, not adoption.

**Required correction:** Change the fixture's usage description to identify released_zero_rewrite_history_refuses_typed_on_adapter_load and its typed/session-scoped import-refusal, byte-preservation and unrelated-session-usability assertions. Remove the claim that the regression pins d6cafd405 acceptance; if retaining that commit reference, label it an unshipped historical acceptance follow-up. Do not alter frozen fixture data or provenance.

### Changes and final verification

**Changed:** `meerkat-mobkit/tests/fixtures/README.md`.

Corrected the zero-rewrite fixture usage to typed/session-scoped import refusal, byte preservation and unrelated-session usability. Preserved capture provenance and identified d6cafd405 acceptance as unshipped historical follow-up.

**Validation:** Read adapters.rs:3966-3985,4038-4068 and its actual expect_err/preservation/liveness assertions. Verified git diff under tests/fixtures contains only README.md; no fixture bytes were opened through SQLite or changed.

**Final review: pass.** The fixture's described current regression now matches expect_err, byte preservation, and unrelated-session save assertions, and identifies the old acceptance follow-up as unshipped. Capture provenance and all other frozen/forensic text remain byte-identical around this correction. No fixture bytes changed, and no SQLite corpus was opened.

**`meerkat-mobkit/tests/fixtures/README.md:18-22`**

```text
`released_zero_rewrite_history_refuses_typed_on_adapter_load` in
  `src/identity_first/adapters.rs`, which pins typed, session-scoped import
  refusal, byte preservation, and unrelated-session usability. The meerkat
  d6cafd405 acceptance was an unshipped historical follow-up, not the
  contract asserted by this regression.
```

The correction describes the test contract rather than rewriting the fixture's historical provenance.

**`meerkat-mobkit/src/identity_first/adapters.rs:4040-4045`**

```text
.expect_err("the shipping strict importer must refuse a zero-rewrite graph");
```

The test asserts an import error, not adoption; 4060-4068 requires unchanged released bytes and a successful unrelated save.

## F-020: The fixture README's sole shared-wire-fixture claim omits other current cross-language contracts

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`meerkat-mobkit/tests/fixtures/README.md:79-82`**

```text
Not part of the frozen corpus, and listed here because it is the one fixture
two languages read: the entry below is a hand-authored wire contract, so it is
SUPPOSED to change when the wire changes. The rule above applies only to the
released and forensic captures.
```

Maintainers receive an incorrect cross-language contract inventory and are not told that another mutable wire fixture must be generated rather than hand-edited.

**`sdk/python/tests/test_application_tool_policies.py:28-41`**

```text
# ONE wire fixture, shared with the Rust side. member_tool_policy's
# `the_committed_wire_fixture_installs_its_carried_provider` regenerates it and
# proves those exact bytes install and bind; this file proves the builder emits
# them. Renaming the key on either side goes red here or there, instead of both
# sides staying green while a host arms nothing.
#
# The digest inside is computed from the canonical bytes by the Rust generator,
# so this cannot be hand-edited into validity.
FIXTURE = (
    Path(__file__).resolve().parents[3]
    / "meerkat-mobkit"
    / "tests"
    / "fixtures"
    / "application_tool_policies_init_params.json"
```

A second committed init fixture is shared between Python and Rust and has an important different maintenance rule: compiler-generated digest-bearing bytes.

**`meerkat-mobkit/src/member_tool_policy.rs:887-892`**

```text
    /// Regenerate with MOBKIT_WRITE_FIXTURE=1; a digest is computed from the
    /// canonical bytes, so the fixture cannot be hand-edited into validity.
    #[test]
    fn the_committed_wire_fixture_installs_its_carried_provider() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/application_tool_policies_init_params.json");
```

The Rust consumer confirms the same shared file and explicitly supports regeneration. live_contracts_v1.json is also read by Rust tests/live_contracts.rs, Python tests/test_live_contracts.py and TypeScript tests/live.test.ts.

### Independent adjudication

Confirmed as a false exclusivity statement, not as a generic demand for an exhaustive fixture catalog. The current README calls this 'the one fixture two languages read'; independent consumers identify at least two other shared fixtures, including one read by three languages. The original 'entry below is hand-authored' sentence applies only to role_migrations and is not itself proof that the README instructs hand-editing the compiler-generated fixture. Narrow the fix to accurate plural inventory/maintenance distinctions while preserving the frozen-corpus rule.

**`sdk/python/tests/test_application_tool_policies.py:28-43`**

```text
# ONE wire fixture, shared with the Rust side.
# The digest inside is computed from the canonical bytes by the Rust generator,
# so this cannot be hand-edited into validity.
/ "application_tool_policies_init_params.json"
POLICY_PRIMARY = json.loads(FIXTURE.read_text())["application_tool_policies"][0]
```

Python reads another current shared fixture in this directory.

**`meerkat-mobkit/src/member_tool_policy.rs:879-908`**

```text
/// Regenerate with MOBKIT_WRITE_FIXTURE=1; a digest is computed from the
/// canonical bytes, so the fixture cannot be hand-edited into validity.
fn the_committed_wire_fixture_installs_its_carried_provider() {
.join("tests/fixtures/application_tool_policies_init_params.json");
if std::env::var_os("MOBKIT_WRITE_FIXTURE").is_some() {
```

Rust consumes the same file and owns its explicit regeneration path. No regeneration was performed.

**`meerkat-mobkit/tests/live_contracts.rs:13-15`**

```text
serde_json::from_str(include_str!("fixtures/live_contracts_v1.json"))
```

Rust reads the live wire-contract fixture.

**`sdk/python/tests/test_live_contracts.py:32-35`**

```text
path = Path(__file__).parents[3] / "meerkat-mobkit/tests/fixtures/live_contracts_v1.json"
return json.loads(path.read_text(encoding="utf-8"))
```

Python reads that same live fixture.

**`sdk/typescript/tests/live.test.ts:30-34`**

```text
new URL("../../../meerkat-mobkit/tests/fixtures/live_contracts_v1.json", import.meta.url),
```

The live fixture also has a TypeScript consumer.

**Required correction:** Change README lines 79-82 to distinguish mutable current wire-contract fixtures (plural) from frozen released/forensic captures. Preserve the existing role_migrations entry. Add short entries for application_tool_policies_init_params.json and live_contracts_v1.json with their actual Rust/Python and Rust/Python/TypeScript consumers. For the policy fixture point to the_committed_wire_fixture_installs_its_carried_provider and its MOBKIT_WRITE_FIXTURE=1 regeneration path; do not generalize 'hand-authored' to every current wire fixture.

### Changes and final verification

**Changed:** `meerkat-mobkit/tests/fixtures/README.md`.

Replaced the sole-shared-fixture claim with plural current-wire-contract wording, preserved role migrations, and added application-tool policies and live contract fixtures with real language consumers. Documented the Rust digest-bearing policy generator and MOBKIT_WRITE_FIXTURE=1 separately from frozen captures.

**Validation:** Read member_tool_policy.rs:879-919, Python test_application_tool_policies.py:28-43, Rust tests/live_contracts.rs:13-15, Python test_live_contracts.py:32-35 and TS live.test.ts:30-34. The generator command passes bash -n but was not executed; fixture bytes remain untouched.

**Final review: pass.** The fixture documentation now distinguishes plural mutable cross-language wire contracts from frozen historical captures. It retains role migrations and adds the policy/live consumers accurately. The policy fixture's generated digest-bearing maintenance path is separate from hand-authored role migration data, and the documented regeneration command selects the real Rust unit test.

**`meerkat-mobkit/tests/fixtures/README.md:82-85`**

```text
The current wire-contract fixtures below are shared across languages and are
not part of the frozen corpus. They are expected to change when the wire
changes, following each fixture's maintenance rules. The freeze above applies
only to released and forensic captures.
```

The false sole-shared-fixture claim is removed without weakening frozen-corpus restrictions.

**`meerkat-mobkit/tests/fixtures/README.md:110-112`**

```text
MOBKIT_WRITE_FIXTURE=1 ./scripts/repo-cargo test -p meerkat-mobkit --lib \
    the_committed_wire_fixture_installs_its_carried_provider
```

The command passes shell syntax checking; it was intentionally not executed because it writes the fixture.

**`meerkat-mobkit/src/member_tool_policy.rs:901-908`**

```text
if std::env::var_os("MOBKIT_WRITE_FIXTURE").is_some() {
```

The test writes generated canonical policy params on this opt-in. Python test_application_tool_policies.py:36-43 reads that same file; Rust live_contracts.rs:13-15, Python test_live_contracts.py:32-35 and TypeScript live.test.ts:30-34 all read live_contracts_v1.json.

## Final scope checks

> [
>   "Read audit-brief.md, audit-scopes.json, complete audit-F/adjudication-F/fixes-F, audit-K/adjudication-K, applicable B-013 audit/adjudication, and split K-006 fix evidence in fixes-E. There is no fixes-K artifact; applicable K work is intentionally reported by its document owners.",
>   "Read all 15 documents matched by the F manifest, including the four unchanged package/prompt documents; none is a symlink. Read the full working-tree diff for all 11 changed F-owned files against baseline HEAD af82b6b3ab34faed9bf3e962d148d55f10dcd1dc.",
>   "Independently traced every primary F item and both propagated items to current local implementation, manifests, or actual executable test assertions. No reliance on prior-unmerged-commit identity as proof of a fix.",
>   "git diff --check passed.",
>   "node flow-editor/test/package-boundaries.test.cjs passed: 24 core modules and 10 component modules.",
>   "Read-only source/manifest comparison passed: 380 ordinary facade keys and 3 test-only keys exactly match controller-export-manifest.json. The bundled export-key gate was inspected but not built/run.",
>   "Read-only documentation validation passed: 17 local links/anchors resolve, all 5 JSON fenced examples parse, and all 35 bash/sh fences pass bash -n. One unchanged MDM cross-host fence contains deliberate angle-bracket host placeholders; syntax checking substituted 127.0.0.1 in memory for those placeholders, not in repository files.",
>   "Parsed the corrected SSE data example as JSON and compared its frame keys to the actual Rust ConsoleFrame definition: all 11 non-optional fields present, matching console:241 id/cursor, correct tagged event and console_event source.",
>   "Verified both embedded/built CSS files still contain the Google Fonts import; console/examples lockfiles exist and their root dependency specs match package.json. Every documented mdm npm script in the index and MDM README resolves in examples/package.json.",
>   "Verified actual Vite targets/prefixes, the local example's TestClient/auth-optional policy/address/serve branches, incident send/stream requests, launcher early-exit/build/browser/provider branches, MDM demo selection and relative binding path behavior, swarm restart conditions, and actual fixture consumers.",
>   "git diff baseline --name-only -- meerkat-mobkit/tests/fixtures reports only README.md. Read-only byte hashing passed for both forensic checksum manifests (two referenced byte files). The frozen README introduction and all released-realm/forensic provenance between the corrected zero-rewrite usage and mutable-wire section are byte-identical to baseline.",
>   "All 20 F decisions are confirmed, with no rejected F findings. The F changes apply only confirmed F/K/B corrections; no rejected claim was applied.",
>   "No repository edits, installs, builds, provider/browser launches, fixture regeneration, SQLite opens, git state changes, commits, or delegation were performed by this reviewer. Only the requested review artifact was written."
> ]
