# Provider-only setup from a TypeScript host

Development API, with source-only examples that have not been executed. Use a
matching reviewed SDK and Connect executable; these examples do not qualify a
published package, live provider flow or deployed browser integration.

The Node entry `@rkat/toolkit/provider-setup` exports `ProviderSetupClient`,
`ProviderSetupError`, `decodeProviderSelection`, and the `ProviderSetupBinding`,
`ProviderSelection` and `ProviderAnswer` types. It starts one selected child with
`--provider-stdio` for native model-target and credential setup, without creating
an agent session or model request. This is separate from Connect's existing
`--stdio` guide protocol and from the browser-only `@rkat/toolkit/ui` entry.

## Trusted launch and credential policy

The host authenticates the viewer and chooses the absolute executable, realm,
roots and finite viewer expiry. `application_id`, `instance_id`, `principal_id`
and `login_scope_id` are trusted host bindings, not browser-selected authority.
Keep the interaction attached to its original viewer, check that viewer before
each answer, and close on revocation or disconnection. An identifier in JSON is
not authentication. Never log keys, callback/authorization URLs or device codes.

The three config roots (`context_root`, `state_root`, `user_config_root`) are
separate from the required `credential_scope` choice:

| Scope | Meaning |
| --- | --- |
| `{ kind: "native_default" }` | Explicit native OS-user/HOME policy. Supply a fixed absolute `HOME` for the host account; this is not isolation between browser viewers. Later guide use needs that same policy and compatible config roots. |
| `{ kind: "file", root: selectedAbsoluteRoot }` | Explicit native file backend with its refresh authority. The same trusted policy must reach every later guide/factory; no keyring fallback or unrelated gateway propagation is implied. |

Pass an explicit minimal environment, never `{ ...process.env }`. Allowed keys
are `PATH`, `LANG`, `LC_ALL`, `TMPDIR`, `TMP`, `TEMP` and `SYSTEMROOT`;
native-default additionally permits `HOME`, `XDG_CONFIG_HOME` and `XDG_DATA_HOME`.
Do not mutate global HOME for individual requests. Separate roots/processes do
not by themselves prove filesystem or tenant isolation.

All `trusted...`/`selected...` values below come from the application's existing
login/deployment owners. `ownedProviderAttempts` is application-owned storage
that outlives the requesting page or HTTP handler.

```ts
import { ProviderSetupClient } from "@rkat/toolkit/provider-setup";
import type { ProviderSetupBinding } from "@rkat/toolkit/provider-setup";

const binding: ProviderSetupBinding = {
  application_id: trustedApplicationId,
  instance_id: trustedInstanceId,
  principal_id: authenticatedPrincipalId,
  login_scope_id: authenticatedLoginScopeId,
  expires_at_ms: authenticatedLoginExpiresAtMs,
  realm_id: selectedRealm,
  context_root: selectedContextRoot,
  state_root: selectedStateRoot,
  user_config_root: selectedUserConfigRoot,
  credential_scope: { kind: "native_default" },
  browser: "manual_return",
};
const client = new ProviderSetupClient(selectedExecutable, binding, {
  PATH: selectedHostPath, LANG: "C", HOME: fixedHostHome,
});
ownedProviderAttempts.set(attemptId, client);
```

For file policy, select `{ kind: "file", root: selectedCredentialRoot }` and omit
HOME/XDG variables. The client supplies `--provider-store-root` or
`--provider-native-default` and an initialization that must match that launch.
No shell or browser-controlled argv is used.

## Present current prompts

`nextEvent(60_000)` observes an event with a timeout in milliseconds. Render
protected controls for `prompt` events and forward the authenticated original
viewer's answer with that exact `prompt_id`. The native endpoint validates the
answer against the still-offered prompt.

```ts
import type { ProviderAnswer } from "@rkat/toolkit/provider-setup";

const event = await client.nextEvent(60_000);
if (event.kind === "prompt" && typeof event.prompt_id === "string") {
  // Supplied only after the original viewer answers this offered prompt.
  const answer: ProviderAnswer = authenticatedAnswer;
  client.answer(event.prompt_id, answer);
}
```

| `event.prompt.kind` | Answer |
| --- | --- |
| `choices` | `{ kind: "choice", choice_id: offeredId }`, only for `selectable: true` |
| `target_review` | `{ kind: "confirm" }` after reviewing the proposed target/routes |
| `action` | `{ kind: "action", action: offeredAction }`, from `actions` |
| `key` | `{ kind: "key", key: enteredKey }` through the protected channel |
| `browser_manual` | `{ kind: "callback", url: returnedFullUrl }` |
| `browser` or `device` | No completion answer; native completion closes the prompt |
| Any current prompt | `{ kind: "cancel" }` |

Choice IDs are opaque. The host does not invent target IDs, profiles or config
patches. Selectable catalog metadata does not prove model entitlement. Creating
a first target has a separate review and native configuration checks. After
credential work, the endpoint reobserves native status and offers actions again;
authentication alone does not select `use` or start a guide.

In `manual_return` mode the user can return the full callback URL from the same
native authorization attempt. Native keeps the target, redirect and state and
joins the loopback listener; the viewer cannot choose a redirect or run a second
OAuth flow. A raw code alone is not this API's answer. `local_loopback` waits for
the actual local callback. Device sign-in displays the native URI/code while the
native owner polls. Provider and deployed-browser acceptance remain unqualified.

## Keep the same child until close settles

Events also include `initialized`, `notice`, `answered`, `prompt_closed` and
`cancelling`. `selected`, `cancelled` and `failed` have different meanings.
A click or acknowledgement does not mean native work completed. Keep draining
the bounded event queue while the interaction is active.

`nextEvent` timeout does not cancel. `close(150_000)` requests cancellation when
needed and awaits the same child's close observation. If it rejects or times
out, retain that client and its original failure; a later `close()` observes the
same child. Do not kill, replace or replay an unknown operation to satisfy an
observation deadline. `ProviderSetupError` must not become a success receipt.

```ts
import { decodeProviderSelection } from "@rkat/toolkit/provider-setup";

// A rejection leaves the same client in the application's retained map.
const terminal = await client.close(150_000);
ownedProviderAttempts.delete(attemptId);
const selection = terminal?.kind === "selected"
  ? decodeProviderSelection(terminal.selection) : undefined;
```

Cancellation/expiry retire admission and join entered native work. Configuration
or credentials may already be saved; cancellation is not rollback. A terminal
`joined` field covers `provider_interaction`, not all host processes, native
background maintenance or physical effects. No token is returned.
`ProviderSelection` is a typed model/provider/binding and credential-status
observation with `live_entitlement: "unobserved"`, not a session, execution grant
or promise that the account can use that model.

Explicit restart is a new interaction that reobserves native configuration and
credentials. Old prompt IDs, callback URLs and cached success are not resumable
authority. Pending cleanup stays with its original child.

## Later guide compatibility

The host separately admits any later guide and preserves its native realm,
config and credential policy. Connect's guide pipe accepts explicit file policy
through `--stdio --provider-store-root <absolute-root>`. Trusted Rust composition
uses `SessionStartup::open_with_provider_store` or
`SessionStartup::prepare_standalone_with_provider_store`. Native-default guide
use keeps the same fixed OS-user/HOME backend; `--provider-native-default` belongs
only to `--provider-stdio`.

A stock Meta/Forge guide does not acquire a custom file-root policy from this
client. This API adds no such setting to that separate gateway; its owner must
provide a compatible native backend before use.
