# Protected Services setup

`@rkat/toolkit/service-setup` is a Node.js client for the native Connect private Services v5 pipe. The authenticated host selects the executable, application/viewer binding, exact resource manifest digest, realm roots and credential scope. These selections must come from the host's own composition, not a browser or model request. Native Connect checks them and owns setup, protected approval, credential storage and finalization.

```ts
import { ServiceSetupClient, ServiceSetupLaunchError } from "@rkat/toolkit/service-setup";
import type { ServiceSetupBinding } from "@rkat/toolkit/service-setup";

// Resolve these from the trusted host's installed artifact and login context.
declare const executable: string;
declare const binding: ServiceSetupBinding;
// Long-lived host ownership, shared with its cleanup observer.
declare const retainedOwners: Set<ServiceSetupClient>;

let owner: ServiceSetupClient;
try {
  owner = new ServiceSetupClient(executable, binding);
  retainedOwners.add(owner);
} catch (error) {
  if (error instanceof ServiceSetupLaunchError) {
    // Retain before awaiting: a timeout must not lose the recovered owner.
    retainedOwners.add(error.owner);
    await error.owner.close();
    retainedOwners.delete(error.owner); // Only after actual process join.
  }
  throw error;
}
const event = await owner.nextEvent();
// Render event.payload only in the host's protected presentation channel.
// Initialization can first require a provider prompt; it need not return a
// session immediately. Wait for the actual connect/initialize result before
// calling submit(). Keep this owner until close() joins the actual child.
// On ServiceSetupTimeout the cleanup observer keeps it in retainedOwners and
// calls close() again on that same owner. Remove it only after process join.
```

The constructor starts exactly one child with `--stdio`. A `file` credential scope adds `--provider-store-root` with the exact selected absolute root. It launches with only a small explicit environment; ambient API keys and configuration overrides are not inherited. A `native_default` credential scope requires an explicit environment with a fixed absolute `HOME` selected by the host. It does not pass a synthetic native-default command-line flag.

`submit(text)` queues a setup message and returns its request ID. It does not report native admission or completion. `nextEvent(timeoutMs)` delivers the correlated native result or a private prompt, state, conversation, finalization or refusal observation. Only one pending event observer is allowed. `ServiceSetupEvent.payload` is an immutable native payload; normal inspection and JSON serialization omit it. Explicit payload access can contain protected browser/device instructions or sensitive native observations and belongs only in protected host presentation.

`answer(promptId, answer)` accepts only the current offered prompt. A `service_secret` answer must match the exact `job_id`, `service` and `slot`; the value is bounded to 16 KiB of valid UTF-8 with no NUL. Provider target/action answers must select offered values. Browser/device prompts allow cancellation, not fabricated callback answers. Connector login uses separate `connectorStart`, `connectorComplete` and `connectorCancel` methods, with closed native selector and operation objects. Transient `connector_auth` initializer profiles are not part of v5.

`cancel(sendRequestId)` and a protected `{ kind: "cancel" }` answer permanently retire preparation on this client. Already accepted native work still joins through its native owner and may complete effects. Late prompts remain correlated for closure but are delivered as `retired_prompt` with only their kind; they cannot accept an answer. Cancellation cannot be used to create another setup attempt on the same binding.

Public methods never wait synchronously for pipe writes. The bounded queue includes the in-flight write, and one writer preserves order. Input frames reject malformed UTF-8, duplicate JSON fields, non-finite numbers, foreign scopes and unmatched results. Transport failures and unrecognized native error bodies produce value-free errors. The original child remains owned after transport failure.

`close(timeoutMs)` queues one close request and observes that same process through its actual `close` event, which includes its standard-I/O handles. A timeout throws `ServiceSetupTimeout` and preserves the owner for a later `close()` observation; it does not kill, restart or resend. `process_joined` records process cleanup. `native_resources_closed` additionally requires the exact native close receipt, exit code zero and no transport failure. An unconfirmed receipt, unsuccessful exit or EOF alone cannot establish native cleanup.

Services v5 finalization is one setup constituent. This client does not capture/reopen a governed continuation, commission fresh-host controller/catalog access, perform required-resource probes or declare a service ready. Authentication, grants, finalization, actual operation results and cleanup remain distinct. The pre-Build gate must stay closed until the complete governed contract supplies actual resource/operation evidence. `outcome_unknown` stays unknown.

The native Rust tests and the TypeScript subprocess tests have separate evidence. Artificial wire peers establish client transport behavior, not compiled-native, OAuth, live-resource or deployed acceptance.
