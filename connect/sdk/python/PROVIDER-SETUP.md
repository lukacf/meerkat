# Provider-only setup from a Python host

Development API, with source-only examples that have not been executed. Use a
matching reviewed SDK and Connect executable; these examples are not an install
recipe for the published release or evidence of live provider authentication.

`meerkat_toolkit.provider_setup.ProviderSetupClient` starts one selected Connect
child with `--provider-stdio`. It presents native model/backend/auth choices,
first-target review where available, and credential setup. It does not start an
agent session or model request. The existing `--stdio` endpoint runs the separate
Connect guide protocol; its prompts and initialization are not interchangeable
with `connect-provider-setup/1`.

## Bind the trusted host first

The application authenticates the viewer and selects the executable, application,
instance, principal, login scope, finite expiry, realm and roots. These are host
inputs, not fields accepted from a browser request. The child does not turn an
arbitrary principal string into authentication. Keep each interaction attached
to its original viewer, check that viewer before forwarding an answer, and close
the interaction on revocation or disconnection. Do not log keys, callback URLs,
authorization URLs or device codes.

The three configuration roots are `context_root`, `state_root` and
`user_config_root`. Credential storage is a separate explicit policy:

| `credential_scope` | Host responsibility |
| --- | --- |
| `{"kind": "native_default"}` | Pass an explicit environment with the host account's fixed absolute `HOME`. This selects the native OS-user/HOME backend, not isolation between browser viewers. Keep the same OS user, HOME and configuration roots for later guide use. |
| `{"kind": "file", "root": selected_absolute_root}` | Select a native file backend and its refresh authority. Every later guide/factory must receive the same trusted policy; there is no keyring fallback or automatic propagation into an unrelated gateway. |

Do not mutate process-global `HOME` per request or copy the ambient environment.
The client accepts only `PATH`, `LANG`, `LC_ALL`, `TMPDIR`, `TMP`, `TEMP` and
`SYSTEMROOT`; native-default mode additionally permits `HOME`, `XDG_CONFIG_HOME`
and `XDG_DATA_HOME`. Configuration and provider-secret environment overrides are
excluded. Absolute paths and separate child processes alone do not establish
filesystem or tenant isolation; those remain host deployment responsibilities.

This construction example uses values already selected by the host's login and
deployment owners. `owned_provider_attempts` is application-owned storage that
outlives an HTTP request or UI observer. Its entries retain the actual client.

```python
from meerkat_toolkit.provider_setup import ProviderSetupBinding, ProviderSetupClient

binding = ProviderSetupBinding(
    application_id=trusted_application_id,
    instance_id=trusted_instance_id,
    principal_id=authenticated_principal_id,
    login_scope_id=authenticated_login_scope_id,
    expires_at_ms=authenticated_login_expires_at_ms,
    realm_id=selected_realm,
    context_root=selected_context_root,
    state_root=selected_state_root,
    user_config_root=selected_user_config_root,
    credential_scope={"kind": "native_default"},
    browser="manual_return",
)
client = ProviderSetupClient(
    selected_executable, binding,
    environment={"PATH": selected_host_path, "LANG": "C", "HOME": fixed_host_home},
)
owned_provider_attempts[attempt_id] = client
```

For explicit file policy, set `credential_scope` to
`{"kind": "file", "root": selected_credential_root}` and omit HOME/XDG variables. The launcher supplies
`--provider-store-root`; native-default mode supplies `--provider-native-default`.
The immutable initialization must agree with that launch policy.

## Answer the offered prompt

`next_event(timeout=60.0)` is a blocking observation with a timeout in seconds.
Use an appropriate host worker rather than blocking an async server's event
loop. Render `prompt` events as protected controls. Forward an answer only from
the still-authenticated original viewer, using that event's exact `prompt_id`:

```python
event = client.next_event(timeout=60.0)
if event["kind"] == "prompt":
    # authenticated_answer is the viewer's answer to this exact offered prompt.
    client.answer(event["prompt_id"], authenticated_answer)
```

| `event["prompt"]["kind"]` | Allowed answer |
| --- | --- |
| `choices` | `{"kind": "choice", "choice_id": offered_id}` for an offered row with `selectable=true` |
| `target_review` | `{"kind": "confirm"}` after the viewer reviews the proposed native target/routes |
| `action` | `{"kind": "action", "action": offered_action}` using an entry in `actions` |
| `key` | `{"kind": "key", "key": entered_key}` through the protected channel, never logs/history |
| `browser_manual` | `{"kind": "callback", "url": returned_full_url}` |
| `browser` or `device` | No completion answer; native completion closes the prompt |
| Any current prompt | `{"kind": "cancel"}` |

The host does not invent choice IDs, targets, profiles or configuration patches.
Native catalog metadata and a selectable row are not model entitlement. New
target creation requires its separate review and native configuration checks.
Successful key entry or sign-in returns to a fresh native status/action prompt;
it does not automatically start a guide or select `use`.

With `browser="manual_return"`, the viewer may return the full callback URL from
the native authorization attempt. Native retains its original redirect and
state and joins the loopback listener; the browser cannot choose a redirect or
start a second OAuth flow. Do not accept a raw code without its retained flow
context. `local_loopback` instead waits for the actual local callback. Device
sign-in presents the native verification URI/code and polls through its owner.
These source paths do not establish live provider or deployed-browser support.

## Retain the child through cancellation and close

Other events include `initialized`, `notice`, `answered`, `prompt_closed` and
`cancelling`. `selected`, `cancelled` and `failed` are distinct terminal
observations. A click or acknowledgement is not native completion. Keep draining
events while the interaction is active; the observation queue is bounded.

`next_event` timeout leaves the interaction active. `close(timeout=150.0)` requests
cancellation if needed and observes this same child's actual termination. A
timeout raises `TimeoutError` and retains that child; call `close` again on the
same client. A `ProviderSetupError` retains failure rather than reporting a
successful selection. Do not replace, kill or replay the operation on timeout.

```python
from meerkat_toolkit.provider_setup import ProviderSelection

# The application still retains this client if close raises.
terminal = client.close(timeout=150.0)
owned_provider_attempts.pop(attempt_id)
selection = (
    ProviderSelection.decode(terminal["selection"])
    if terminal is not None and terminal["kind"] == "selected" else None
)
```

Cancellation and expiry retire admission, then join entered native work. A
configuration or credential write may already have completed; cancellation is
not rollback. A `joined` terminal field covers `provider_interaction`, not all
host processes, native background maintenance or physical effects. No token is
returned. `ProviderSelection` carries native model/provider/binding and credential
status with `live_entitlement="unobserved"`; it is not a session, execution grant
or guarantee that the account can use the model.

An explicit restart creates a new interaction and reobserves native config and
credentials. Do not replay old prompt IDs, callback URLs or cached success after
failure or restart. Pending close remains attached to the original child.

## Continue into a separately admitted guide

A selection does not launch the later guide. The host must retain compatible
realm, configuration and credential policy when it separately admits that work.
Keep both the returned `ProviderSelection` and the trusted `ProviderSetupBinding`
used for that interaction. The selection contains `interaction_id`, `model`,
`provider`, `auth_binding`, `credential` and `live_entitlement`; it does not carry
the host's `realm_id`, configuration roots or `credential_scope`. Those remain
in the trusted binding and must be supplied through the later owner's supported
construction API. Keep the selection's `auth_binding` structured, including its
native realm, binding and any profile or configured origin. A provider name or
model string alone does not preserve that selection.

Retain the binding in host-owned state associated with the authenticated viewer.
Do not rebuild it from browser fields, add its roots to the selection wire
object, or copy credentials between backends to make a later factory work. The
[SDK types](src/meerkat_toolkit/provider_setup.py) define these two distinct
objects. A successful provider interaction does not itself admit guide work.

For Connect's existing guide pipe, explicit file policy uses
`--stdio --provider-store-root <absolute-root>`. The trusted Rust paths are
`SessionStartup::open_with_provider_store` and
`SessionStartup::prepare_standalone_with_provider_store`. Native-default guide
use keeps the same fixed OS-user/HOME backend; `--provider-native-default` is a
provider-only flag, not an option for `--stdio`.

Do not assume a stock Meta/Forge guide inherits a custom file root from this
client. This API does not add a custom-store setting to that separate gateway.
Its owner must supply a compatible native credential policy before use.
