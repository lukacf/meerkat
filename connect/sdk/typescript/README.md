# Meerkat Toolkit TypeScript operations

## Install 0.9.0

```sh
npm install https://github.com/lukacf/meerkat-connect/releases/download/v0.9.0/rkat-toolkit-0.9.0.tgz
```

This is the Connect release asset, not a Toolkit npm registry selection.
For source development, follow the local packaging commands below.

This optional ESM package exposes Home Assistant, [Gmail REST operations](GMAIL.md),
[Slack history/thread reads](SLACK.md) and [Google Calendar Events operations](GOOGLE-CALENDAR.md). It runs
without Meerkat, MobKit, a tool registry, or a model. The host supplies an already
selected, authenticated Fetch function and an exact instance URL. The library
borrows that function and never closes it, obtains credentials, infers a person,
or decides permission. Node 22 or later is the supported execution profile.

The separate [`@rkat/toolkit/ui` browser entrypoint](UI.md) provides reusable
connection, setup-progress, frontend and package management components. It consumes
authenticated host projections and owner-bound controls; it does not import the
Node connectors or implement authentication and lifecycle owners.

```ts
import { HomeAssistantOperations } from "@rkat/toolkit/connectors/home-assistant";
import type { HomeAssistantContext } from "@rkat/toolkit/connectors/home-assistant";

// Application code obtains this binding from its existing connection/auth owner.
declare const selectedAccount: HomeAssistantContext;
const ha = new HomeAssistantOperations(selectedAccount);
const kitchen = await ha.getState("light.kitchen");
const advertisedServices = await ha.listServices();

// Application admission and resource permission precede this optional write.
const receipt = await ha.callService("light", "turn_on", {
  data: { entity_id: "light.kitchen", brightness: 120 },
});
```

`getState` returns the complete state object including nested attributes and
provider metadata, and verifies the exact entity ID. `listServices` returns the
complete advertised domain/service metadata. `callService` sends the supplied
JSON object once, unchanged within this numeric/data profile. It does not insert
entity targets or transform service names. Identifiers are lowercase ASCII
letters, digits and underscores; an entity is exactly `domain.object_id`.

The normal service response is a list of changed states. With
`returnResponse: true`, the request has exactly the `?return_response` query and
expects `changed_states` and `service_response`. The latter is an object or null.
A nested false value remains false; a null value remains null. A top-level false
service response is invalid, not treated as null. `responseRequested` distinguishes
an absent service response from a requested null response. A provider receipt is
not proof that a physical device reached its desired state.

## Host transport contract

`HomeAssistantContext` has two fields: `baseUrl` and
`fetch: (request: Request) => Promise<Response>`. The application-selected closure
binds the actual account and injects its authentication privately. There is no
account-name parameter that could select another credential. Two accounts on one
instance require two explicit host bindings. Credential rotation within that
binding, revocation, allowed resources, egress/DNS/proxy policy, TLS, transport
pools and transport retirement remain with their existing application owners.

The supplied Fetch must preserve destination, method, body, headers, manual
redirect mode, and cancellation signal; it must return an actual Fetch Response.
It must not retry, replay, retarget, insert default query parameters or switch
accounts. Authentication refresh must not replay the operation behind this API.
This is a trusted in-process transport contract, not protection against a hostile
host closure. The response URL check cannot undo a redirect performed by a
nonconforming transport. The library captures the URL and function once, so later
replacement of context properties cannot silently retarget an existing selection.

The base must use canonical absolute HTTP(S) URL spelling, with no userinfo,
query, fragment, or hidden URL normalization. For example use
`http://127.0.0.1:8123/`, not a dot-segment path or an implicit host slash. A
reverse-proxy prefix is retained; a trailing slash is added to a nonempty prefix
if needed. The library sets `redirect: "manual"`, `credentials: "omit"` and
`cache: "no-store"`; authentication must be explicit in the supplied closure.
It never selects a private network endpoint on its own.

Select this REST connector explicitly. It is not a fallback from Home Assistant's
Assist MCP surface, whose exposed-entity permissions differ. The host must
separately authorize REST access and the requested operations. Browser credential
ceremonies, OAuth, token storage and account verification are not part of this
package. Browser or headless transport support has not been established by the
Node conformance suite.

## Limits and errors

The default deadline is 20,000ms across sending and streaming the whole response,
including decoding. Progress chunks do not reset it. The deadline aborts the
operation and then awaits native Fetch settlement and reader cancellation; it
cannot impose a hard cleanup bound on a host implementation that ignores abort
or never settles cancellation. There are no detached cleanup tasks. Returned
container shape checks occur before the public operation resolves. Responses are
limited to 1,048,576 bytes, including the streamed path without Content-Length.
Both settings can be supplied to the constructor. The byte ceiling bounds the
body consumed by this library, not allocations or eager buffering/decoding
inside the borrowed Fetch implementation. The library requests identity encoding,
refuses other Content-Encoding values, and requires UTF-8 `application/json`.

`jsonc-parser` 3.3.1 validates the receipt in strict mode before native JSON.parse.
Comments, trailing commas, duplicate keys (including escaped spellings), invalid
UTF-8, nonfinite numbers and more than 128 container levels are refused. This
package deliberately does not implement a JSON parser. JavaScript numbers use
IEEE-754 binary64: floating-point values have its usual precision. Integer values
outside the safe integer range are refused rather than returned rounded. Python's
arbitrary integer range is therefore outside this TypeScript profile.

Request data is checked at runtime, not only by TypeScript. Only plain data
objects and dense arrays are supported; undefined, symbol or function values,
accessors, nonenumerable properties, sparse/extended arrays, cycles, non-plain
objects and unsafe/nonfinite numbers are refused before the Fetch call. Negative
zero is refused in requests because JSON.stringify would silently change it to
zero. Getters and toJSON methods are not invoked. In-process Proxy code remains
part of the trusted host environment. Local input errors are safe TypeErrors.

`HomeAssistantError` exposes `kind`, `writeOutcome`, optional `statusCode`, and a
decimal `retryAfterSeconds` when present. It contains no raw provider body, URL,
credential, thrown cause, or user abort reason. Returned provider data can still
be private and must be handled according to application policy. Retry-After is
information, not permission to replay a write.

For reads, `writeOutcome` is `not_applicable`. For every failed service request
once transport is attempted, it is `unknown`, including HTTP refusals, redirects,
malformed success, oversized data, cancellation, timeout and connection loss.
Home Assistant can finish a service after the client disconnects, and an error
can follow entry into the service. Never equate a failed call with no effect.
`HomeAssistantCancelled` preserves cancellation as a distinct typed rejection;
JavaScript has no asyncio-style cancellation base class. An already-aborted
signal sends nothing and also uses the conservative `unknown` service taxonomy.
There is no automatic retry.

## Local conformance and packaging

```sh
npm install --ignore-scripts
npm run validate
npm pack --ignore-scripts
```

The package uses a pinned runtime dependency and a pinned TypeScript compiler.
`build` creates ESM and declaration files in `dist`; tests import the public
package subpath, not the TypeScript source. The Node test suite uses real local
HTTP sockets and application auth closures, including two accounts, refused
redirects and a service that continues after disconnect. It does not use a live
Home Assistant server or actual credentials. Run the same suite with the packed
artifact installed in an isolated consumer before claiming package conformance.
No test or build result is implied by the existence of these commands.
