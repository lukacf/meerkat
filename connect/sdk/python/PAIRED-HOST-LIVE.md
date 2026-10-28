# Paired host live image input

`meerkat_toolkit.frontend.paired_host_live.PairedHostLiveImageClient` submits one image to a route that a paired host serves for its live conversations. This is a frontend input adapter: the image enters an agent's live conversation. It is not a camera or device service, a second live-session owner, or a general HTTP client.

The optional `paired-host-live` dependency profile is `httpx>=0.28.1,<0.29` and `anyio>=4.13,<5`. It uses Python's asyncio backend. Import this module explicitly; importing the base frontend package does not require these optional dependencies.

```python
from meerkat_toolkit.frontend.paired_host_live import PairedHostLiveImageClient

client = PairedHostLiveImageClient(
    "http://127.0.0.1:8080",
    authorization_provider=current_lease_header,
    route=host_live_image_route,
    authorization_scheme=host_lease_scheme,
)
response = await client.submit_image(
    idempotency_key=existing_image_job_key,
    mime_type="image/jpeg",
    data_b64=encoded_image,
)
```

`response` is a validated HTTP JSON object. It is not evidence that the native runtime committed the image, generated a response, or delivered speech. The caller must separately observe the native `user_content_committed` event for the exact image job on its original live channel generation.

## What the host supplies

The host names the route and the authorization scheme of its lease header; the client hard-codes neither. The route is an absolute path of one to eight unreserved segments, for example `/live/image`: no query, fragment, percent-encoding, dot segment or empty segment. The scheme is one RFC 9110 auth-scheme token, for example `HostLease`. The callback returns the complete header value: that scheme, one space and a token68 credential of 32 to 512 characters. Any other value is refused before I/O as `lease_unavailable`.

The host's lease authority selects and authenticates the identity and resource the image belongs to. The host admits the request, selects the native handle and channel, and commits the content. The Toolkit client cannot supply identity, resource, channel ID or native authorization claims.

The process that owns the image job, for example a device process, keeps the job, socket generation, native observations and any follow-up prompt. It submits the image once, then waits for the exact native commit before queuing follow-up text. An HTTP failure does not disprove commitment: a matching native event may still acknowledge the entered operation. A timeout without that event remains unconfirmed and never causes a prompt or automatic image replay.

## Transport and lifetime

The configured destination is a literal HTTP loopback origin with an explicit port: `127.0.0.1` or `[::1]`. It cannot contain user information, an extra path, query or fragment. DNS names, external origins and alternate URL forms are refused before I/O. The request path is the host-named route. A current lease header is obtained synchronously once for each explicit request; the callback must be quick and must not perform blocking I/O. The host remains the token-validity authority.

Each submission owns one HTTPX client. Ambient proxies, cookies, authentication hooks and redirects are disabled. Redirects are returned as unconfirmed observations without following their locations. The JSON request has only `data`, `idempotency_key` and `mime`; its default encoded limit is 8 MiB. The default response limit is 64 KiB. The client accepts only HTTP 200 with unencoded JSON-object content and rejects malformed JSON, duplicate keys, non-finite numbers and error objects. `{}` is still only an HTTP object, never a content-commit receipt.

`ImageSubmissionError` contains a fixed `ImageFailure`, `request_entered` and optional HTTP status. It excludes response content, image bytes, credentials and raw transport exceptions. `request_entered` means the local request send was entered; it does not prove whether the server performed an effect. No error authorizes an automatic retransmission.

The operation deadline defaults to 30 seconds and covers send/body processing. Response/client cleanup is shielded and joined and can complete after that deadline. Caller cancellation cancels the retained operation's public AnyIO scope and waits for its cleanup. Repeated caller cancellation cannot detach the owned task or interrupt supported cooperative cleanup; the first caller cancellation, including its arguments, is re-raised. An ordinary primary HTTP failure is retained if cleanup also fails. This does not promise hard preemption of an uncooperative transport or a blocking authorization callback.

## Boundaries and provenance

The module was extracted by its author from the author's own projects into Meerkat Toolkit. The extracted copy is licensed under MIT OR Apache-2.0 as stated in `LICENSE-EXTRACTED`. This grant covers the Toolkit copy only; it does not change the original projects or third-party dependency terms.

Local tests use synthetic image bytes, a loopback host and recorded requests. They do not run a native provider, camera, microphone, device, deployment or production account. The adapter provides no automatic durable replay: a host that must recover an entered effect after process loss keeps its own durable owner.
