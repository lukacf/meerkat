# The paired host live image input

`meerkat_toolkit.frontend.paired_host_live.PairedHostLiveImageClient` submits one image to the existing paired host robot gateway route. This is a frontend input adapter: the image enters an agent's live conversation. It is not a camera/device service, a second live-session owner, or a general HTTP client.

The optional `paired-host-live` dependency profile is `httpx>=0.28.1,<0.29` and `anyio>=4.13,<5`. It uses Python's asyncio backend. Import this module explicitly; importing the base frontend package does not require these optional dependencies. The robot daemon adoption selects `meerkat-toolkit[paired-host-live]==0.1.0`.

```python
from meerkat_toolkit.frontend.paired_host_live import PairedHostLiveImageClient

client = PairedHostLiveImageClient(
    "http://127.0.0.1:8080",
    authorization_provider=current_paired_host_lease_header,
)
response = await client.submit_image(
    idempotency_key=existing_image_job_key,
    mime_type="image/jpeg",
    data_b64=encoded_image,
)
```

`response` is a validated HTTP JSON object. It is not evidence that the native runtime committed the image, generated a response, or delivered speech. The caller must separately observe the native `user_content_committed` event for the exact image job on its original live channel generation.

## Existing owners

The paired host's lease authority selects and authenticates the household identity and robot resource. Its RouteManager admits the request and its robot extension calls CoreLiveFacade. CoreLiveFacade selects the native handle/channel and uses its existing PhysicalReceiptExecutor. The Toolkit client cannot supply identity, resource, channel ID or native authorization claims. No new roster, outbox, durable receipt database or C1 interface is introduced.

The robot's existing VoiceLink owns the image job, socket generation, native observations and serialized text injection. In the adoption, it submits the image once, then waits for the exact native commit before queuing the image-description prompt. An HTTP failure does not disprove commitment: a matching native event may still acknowledge the entered operation. A timeout without that event remains unconfirmed and never causes a prompt or automatic image replay.

The image-description prompt retains its image key and exact socket generation in the existing injection queue and in-flight slot. Close discards only image-bound prompts, preserving ordinary announcement behavior. Image prompts cannot enter the ordinary announcement replay window after a send exception, acknowledgement timeout, cancellation or command rejection. A replacement channel cannot consume an old image prompt. This is in-process custody, not durable replay or proof that speech was heard.

## Transport and lifetime

The configured destination is a literal HTTP loopback origin with an explicit port: `127.0.0.1` or `[::1]`. It cannot contain user information, an extra path, query or fragment. DNS names, external origins and alternate URL forms are refused before I/O. The fixed path is `/api/paired_host/robot/live/send_image`. A current lease header is obtained synchronously once for each explicit request; the callback must be quick and must not perform blocking I/O. The paired host remains the token-validity authority.

Each submission owns one HTTPX client. Ambient proxies, cookies, authentication hooks and redirects are disabled. Redirects are returned as unconfirmed observations without following their locations. The JSON request has only `data`, `idempotency_key` and `mime`; its default encoded limit is 8 MiB. The default response limit is 64 KiB. The client accepts only HTTP 200 with unencoded JSON-object content and rejects malformed JSON, duplicate keys, non-finite numbers and error objects. `{}` is still only an HTTP object, never a content-commit receipt.

`ImageSubmissionError` contains a fixed `ImageFailure`, `request_entered` and optional HTTP status. It excludes response content, image bytes, credentials and raw transport exceptions. `request_entered` means the local request send was entered; it does not prove whether the server performed an effect. No error authorizes an automatic retransmission.

The operation deadline defaults to 30 seconds and covers send/body processing. Response/client cleanup is shielded and joined and can complete after that deadline. Caller cancellation cancels the retained operation's public AnyIO scope and waits for its cleanup. Repeated caller cancellation cannot detach the owned task or interrupt supported cooperative cleanup; the first caller cancellation, including its arguments, is re-raised. An ordinary primary HTTP failure is retained if cleanup also fails. This does not promise hard preemption of an uncooperative transport or a blocking authorization callback.

## Boundaries and provenance

This adoption targets the existing paired host route contract and the robot gateway mode at `paired-host-robot` commit `0418a8b4e69a84f94adccd703a6060b9c293a49a`. It replaces the send-image portion of `src/robotd/live.py::_paired_host_live` and repairs the existing image/injection owner. Other gateway operations and service/device clients are not migrated by this slice. The robot's sidecar mode, camera acquisition, audio, motion and safety interlocks are unchanged.

The paired host and the robot are user-owned projects. The captured robot root and extension had no LICENSE, COPYING or NOTICE file and no declared package license. The extracted `meerkat_toolkit.frontend.paired_host_live` copy is licensed under MIT OR Apache-2.0 as stated in `LICENSE-EXTRACTED`. This grant covers the Toolkit copy only; it does not change the original projects or third-party dependency terms. The source provenance, exact baseline and successor files, dependency metadata and source hashes remain retained.

Local tests use synthetic image bytes and native observations, actual VoiceLink code, actual paired host RouteManager/CoreLiveFacade/lease code and recording native handles. The process test runs the robot worker under Python 3.12 against the paired host under Python 3.11. It proves this local composition, including a lost HTTP acknowledgement followed by a matching native commit. It does not run a native provider, camera, microphone, robot, deployment or production account.

The paired host's existing PhysicalReceiptExecutor preserves completed same-key/body results across SQLite reopen. Its current persistence begins after the effect returns, and per-object locking does not establish cross-process ownership or recovery of an entered effect after process loss. This adapter therefore provides no automatic durable replay. Native/C1 authority, independent the robot role activation, broader gateway control migration and the complete Toolkit revision8 target remain open. A new private Toolkit artifact and successor the robot lock/runtime-manifest binding are required before deployment; the inspected old lock and runtime manifest are preserved as historical inputs.
