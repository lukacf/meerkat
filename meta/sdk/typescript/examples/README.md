# TypeScript host runnable

[host-runnables.ts](host-runnables.ts) declares `local.collect`, registers
its callback on the builder before connecting and creates an announcement
source whose runner targets that name. The gateway creates the source's durable interval schedule, fires it
and supplies the schedule ID, occurrence ID, due time and source payload to
the handler. The program then ends the source and shuts down the gateway.

No provider key, model turn or browser is needed. The example logs the
callback; a real collector would read its provider and commit a step with
`announcementSourceAdvance`. A handler's return value is acknowledged but
is not persisted as a schedule result.

Every member that holds the schedule tools and is not private to a person
can create a schedule that fires any runnable the host registers, with
parameters of its choosing (the handler reads them as the occurrence's
`payload`). The fire does not tell the handler who wrote the schedule.
Register only runnables that any such member may fire, and check the payload
in the handler. This example accepts only its
fixed mob and source IDs, with no extra payload fields, before collecting.

The example does not send messages to the reader, so its profile omits
`comms`. If you extend it to message that member, add
`[profiles.reader.tools]` with `comms = true` to the mob definition; without
it, the gateway refuses the messaging wiring.

## Run

Use the SDK from this checkout and either build its gateway or use a
published Meta gateway, 0.9.15 or 0.9.16. To build from the repository root:

```bash
./scripts/repo-cargo build --locked -p meerkat-meta --bin rpc_gateway
export META_GATEWAY_BIN="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rpc_gateway"
cd meta/sdk/typescript
npm ci
npm exec -- tsx examples/host-runnables.ts
```

If a gateway is already available, set `META_GATEWAY_BIN` to its absolute
path and run the last three commands from the repository root, starting
with `cd meta/sdk/typescript`.

The first callback is due when the source is declared. Later callbacks
would be a minute apart, but this example ends the source after receiving
the first one. It checks that this gateway schedules the runner and waits
at most 90 seconds for the callback. Successful output includes
`Collected local feed:` with the occurrence JSON, followed by
`Source ended; its interval schedule is stopped.`

The program uses a fresh temporary directory for its persistent state and
removes it after shutdown. A lasting application keeps a stable state
directory, declares its runnable names and registers their handlers with
`MetaBuilder.onScheduleFire` on every boot, before `build()` connects.
The builder refuses a handler whose exact name was not declared with
`hostRunnables`; either registration order is allowed before `build()`.
`MetaRuntime.onScheduleFire` replaces a running host's handler; the replacement
survives reconnecting the same runtime object. Registering only after
`build()` returns leaves a startup gap for stored schedules. A declared name
without a handler fails as `delivery_failed` / `runtime_rejected`; an undeclared
source-runner target is `misfired` / `target_missing` under the default
policy, without an SDK callback. Neither occurrence is delivered later when
a handler is registered; later interval occurrences remain pending. Throwing or
rejecting also fails the occurrence attempt. Use `occurrence_id` to
deduplicate any external side effects. Upgrading requires no stored-schedule
migration, but does not supply declarations or handlers.

The [TypeScript SDK guide](../../../docs/sdks/typescript.mdx#host-runnables)
describes the API; [source runner configuration](../../../docs/reference/configuration.mdx#mob-announcement-source-runners)
describes how a source creates and maintains its schedule.
