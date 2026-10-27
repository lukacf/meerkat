# Native Toolkit host in TypeScript

The `@rkat/toolkit/native-host` entry connects a trusted application's ordinary
Meta builder to the local Toolkit gateway. It uses the same native runtime,
callback dispatcher, accepted/settled init protocol and shutdown owner as the
Meta SDK. Importing Toolkit's service connectors does not import this entry.

The application startup factory supplies the selected artifacts, gateway,
persistent state and source-owned callbacks while it holds its existing lease.
Neither this API nor the private JSON carrier authenticates arbitrary inputs.

This example shows successful-build handling. If `build()` rejects with
`GatewayStartupCleanupError`, the application's existing failure owner must
retain both errors and its `cleanup` capability until it is rejoined, as
described in "Cleanup and retained failures" below.

```ts
import { Meta } from "@rkat/meta-sdk";
import { ToolkitApplicationUpdate } from "@rkat/toolkit/native-host";
import type { ApplicationUpdateStartup } from "@rkat/toolkit/private-host";

// All these values come from the application's existing admitted update and
// source lease. Keep that custody until the runtime has shut down.
const host = new ToolkitApplicationUpdate(startup as ApplicationUpdateStartup);
const builder = host.configure(Meta.builder()
  .gateway(selectedToolkitGateway)
  .persistentState(originalNativeState)
  .sessionService(selectedSourceAgentBuilder)
  .rosterProvider(selectedSourceRoster));

// Use the native declaration only when selected by that same transaction.
// Zero is a valid expected revision. Ordinary recovery may omit it.
if (expectedRevision !== undefined) builder.declareSpecUpdate(expectedRevision);

const runtime = await builder.build();
try {
  // The application's normal runtime and Console use this same instance.
  await serveApplication(runtime, host.hostControl);
} finally {
  await runtime.shutdown();
}
```

## Fresh application preparation

Use `ToolkitApplicationPreparation.build()` for an admitted fresh schema-4
application plan. This joins the ordinary native builder with the application's
trusted finalizer. Calling the native builder separately would not join that
finalizer and is not this API's completion path.

```ts
import { Meta } from "@rkat/meta-sdk";
import { ToolkitApplicationPreparation } from "@rkat/toolkit/native-host";

// The existing application transaction supplies the exact plan and owns this
// finalizer, its source lease, policy validation and final publication.
const host = new ToolkitApplicationPreparation(
  { plan: selectedSchema4Plan },
  finalizePreparedApplication,
);
const runtime = await host.build(Meta.builder()
  .gateway(selectedToolkitGateway)
  .persistentState(originalNativeState)
  .sessionService(selectedSourceAgentBuilder)
  .rosterProvider(selectedSourceRoster), { signal: startupAbort.signal });
try {
  await serveApplication(runtime, host.hostControl);
} finally {
  await runtime.shutdown();
}
```

`selectedSchema4Plan` is the existing `{ path, sha256 }` selection. The
constructor captures its exact bytes from a regular file, with final-component
no-follow and a 256 KiB limit. It supplies those bytes through the public native
`preparedApplication` carrier before startup and checks the exact carrier at the
same transport's `meta/init` send boundary. Native still validates the private
plan reference, compiler inputs and public/private join. Keep this dedicated
builder and its configuration under the same application transaction.

The private channel sends `PrepareApplication` as request 1, then allows that
same child's ordinary native init and callbacks to run. It receives the actual
completed manifest reference and invokes `finalizePreparedApplication` with it.
The finalizer must validate the real compiler artifacts against the same plan,
perform the application's existing policy preparation and validation, publish
its final selection, and return the four-field `HostBootstrapSelection`.
Toolkit does not implement that policy or publication owner. It adopts the
returned selection as request 2 on the same channel and waits for both adoption
and the native builder's accepted/settled init result before returning a runtime.
A private preparation result or bootstrap acknowledgement alone is not native
Ready or a completed user task.

Abort closes private admission and prevents a finalizer that has not entered
from publishing. If the finalizer has entered, the build joins it and retains
its returned selection even when adoption can no longer complete. If native
Ready races a private failure or abort, Toolkit joins that same runtime's
shutdown before returning the failure. A rejected shutdown returns the existing
`GatewayStartupCleanupError` with the same runtime's cleanup capability.

`host.preparationAttempt` retains this attempt's actual manifest and chosen
selection; `host.settlement` retains the original native, finalizer/adoption and
cleanup outcomes once the build settles. Native startup errors keep their exact
identity and public cleanup capability. Retain these values with the original
application transaction: they are observations, not retry or rollback authority.
A lost acknowledgement reuses the same observation, never another private
write. Failed build, shutdown and reconnect cannot restart this fresh binding.
A new attempt requires the transaction's explicit recovery decision.

For ordinary original-owner Restore, configure the builder with
`new ToolkitHost(originalSelection).configure(builder)`. It sends the existing
private bootstrap selection and leaves the native builder's ordinary mob,
callback and persistence configuration in place. Ordinary reconnect is allowed
only after the same native stop and private client close have settled. A failed
stop keeps that transport for the runtime's next `shutdown()` or `connect()`;
the factory cannot start a successor while that cleanup still rejects.

Configure one dedicated builder once. Do not replace its `gatewayTransport` or
the update's `preparedApplication` carrier afterward. The update constructor
copies the selected wire pins and reads the exact selection bytes from a regular
file with final-component no-follow and the existing 16 MiB limit. It passes
those bytes and their SHA-256 through native `preparedApplication`; the gateway
still checks the private/public join. The factory consumes its one replacement
attempt before starting a child. Shutdown, failed startup or another `build()`
on that configured builder cannot automatically replay the update. Constructing
a new binding requires the original transaction's explicit recovery decision;
it is not a retry receipt.

The prior desired policy pin and association directory remain owned by the
existing update transaction. The directory is output custody, not authority or
a finish receipt. Original authority, resources and native root/current remain
unchanged by this transport. Actual prepare/verify association observations and
native tracked convergence keep their existing owners. An init exception can
still represent accepted or uncertain effects; retain the actual native error
and the transaction's state instead of treating it as rollback.

For Restore and replacement, private selection completes in the native
transport's awaited `startAsync`, after native callback registration and before
native init. Fresh preparation starts its request there and awaits the completed
manifest alongside native init, as described above. The transport also
checks that same private client's known failure immediately before its actual
`meta/init` send. Native watch opening is synchronous; the await returning from
startup still permits an intervening private failure. Toolkit does not send a
second init or run another native response reader. Non-init requests and native
cleanup retain their existing native paths.

## Selected SDK and qualification

This source requires the delivered public Meta TS APIs and cleanup fixes
through `162e2154bd3b1264087adbd5217b2561038b6f82`. The inspected full bodies at
`fadbb7761547318a2f5f58b0a8fcd3e2ad849a17` plus the delivered `c953bc83d` and
`162e2154b` successor diffs establish the public composition contract:
`MetaBuilder.gatewayTransport`, `preparedApplication`, `declareSpecUpdate`, and
`PersistentTransport.startAsync`, `extraStdioEndpoint`, `requireHostStartup` and
`sendAsync`, plus `lastTermination`, runtime `lastGatewayTermination` and the
same-runtime `GatewayStartupCleanupError.cleanup` handle. The successor retains
that handle on every rejected composed stop and preserves unprintable original
errors without formatting them. This is source compatibility evidence, not
qualification of a selected package. The optional peer is `@rkat/meta-sdk`
version `0.9.0`; that version alone does not identify the required artifact. The existing qualification lane
must supply the selected matching SDK package before Toolkit's normal build,
typecheck and tests. The native-host control module imports it directly and
does not skip when it is absent. No registry resolution, integrity or temporary
source path has been invented in the lock file. Existing optional `@rkat/sdk`
service use is unchanged.

The new controls use the actual selected Meta builder, runtime, callback
dispatcher and transport with an inert child. They cover exact private/native
carriers, callbacks during startup, known private failure before init, typed and
uncertain native init results, replacement replay refusal, ordinary reconnect,
same-runtime cleanup retry and failed-build cleanup handles. They also cover
private close failure after process exit, both original stop errors, nonzero
exit and a signal handler that exits 0 after native SIGTERM escalation. Fresh
preparation controls cover request-1/native-init/request-2 ordering, finalizer
entry and settlement across abort, native Ready with a lost private reply,
exact plan-carrier refusal, and same-runtime cleanup after that partial result.
The Rust compiler and policy owner are inert in these SDK controls.
They are authored source only here, not a local test result or proof of native
member admission, surface verification, Ready, hardware or application success.

## Cleanup and retained failures

The native record describes the exact stopped child: exit code, signal, highest
attempted escalation and shutdown response. An unknown or absent record keeps
the private client and binding. An observed exit permits private close; if that
close rejects, the same client remains owned for a later native runtime rejoin.
Process exit does not substitute for the private join. When both stop owners
reject, Toolkit returns an `AggregateError` with the native error first and as
its cause, and the original private error second.

After private close completes, a nonzero exit, signal, escalation, refused
shutdown or unanswered shutdown is reported as failure. Natural exit 0 is
accepted only without those facts. The selected Toolkit wrapper returns 0 after
its native and private joins; this says nothing about application readiness or
the outcome of an earlier update. A later shutdown can acknowledge an already
reported terminal failure and release the settled binding. Its native record
remains unchanged and can be inspected with `runtime.lastGatewayTermination()`.
This is cleanup retry, not update replay.

If `builder.build()` fails while native cleanup remains pending, its supported
`GatewayStartupCleanupError` retains `startupError`, `cleanupError` and the same
runtime's `cleanup` capability. Preserve both errors and retain that capability
until `await error.cleanup.shutdown()` fulfils. Its `lastTermination()` reports
only the native child facts. Do not replace this with a new builder or reaper.

The delivered `c953bc83d` correction keeps this handle even when the native
process record says `exited` or `not_started` but composed private cleanup still
rejects. The `162e2154b` follow-up keeps both original errors without invoking
their formatting and avoids repeating a spent child shutdown horizon on rejoin.
Toolkit relies on those existing native fixes. No private native fields, second
process reaper or independent runtime are used; execution qualification remains
with the selected SDK and native gateway lane.
