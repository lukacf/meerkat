# Original-child provider adoption

This source successor adds `ProviderSetupTransport` and `ProviderSetupClient.adopt` in `meerkat_toolkit.provider_setup`. It is available in this development source; installed-host qualification and release are separate. The existing constructor that spawns a provider is retained, including its original environment, protocol and ownership behavior.

```python
from meerkat_toolkit.provider_setup import (
    ProviderSetupBinding, ProviderSetupClient, ProviderSetupTransport,
)

transport = ProviderSetupTransport(
    input=original_provider_stdin_writer,
    output=original_provider_stdout_reader,
    request_retirement=notify_existing_root_retirement,
    wait_process=observe_original_process_exit,
)
client = ProviderSetupClient.adopt(binding, transport=transport)
```

The trusted root retains `transport` before adoption and the original process owner throughout. `transport.client` is the recoverable client after transfer, even if the caller is interrupted before receiving the method return. `transport.transferred` is derived from that same one-time association. No executable, environment, process handle, spawn fallback, reconnect or replay is accepted by adoption.

When adoption runs on a worker, the root must retain and join that actual worker before treating `transport.client is None` as a completed pre-transfer refusal. A temporary `None` while adoption is queued or running does not permit endpoint closure, replacement or a second adoption call. Provider setup uses its own original child and carrier; collection and gateway children and pipe pairs remain separate.

## Endpoint and callback contract

Both endpoints must be the exact original unbuffered `io.FileIO` objects produced by the root's pipe allocation: the write-only original provider stdin endpoint and read-only original provider stdout endpoint. Admission checks that they are open, distinct, blocking FIFO descriptors and close-on-exec. Buffered wrappers, sockets, nonblocking streams and general BinaryIO doubles are refused. The SDK neither duplicates nor reopens descriptors. The root must pass separate original child-end descriptors to its original spawn and must not reuse collection/gateway pairs or construct another carrier over aliases. These semantic origin obligations cannot be proved by inspecting two descriptor numbers.

`request_retirement() -> None` is a synchronous nonblocking notification to the existing root operation. It runs on an SDK worker and cannot wait for SDK close, SDK I/O workers, descendants or complete root cleanup. The implementation attempts it once when close is requested, I/O fails, or the protocol reader ends. A callback error or non-None return is retained as failure; it is never evidence of cleanup. Async callback declarations are refused before transfer. A callback that violates its nonblocking contract can leave the same monitor pending; observers time out without discarding it.

`wait_process(seconds: float) -> int` runs only on the process observer worker. The SDK currently supplies 0.1 seconds, within the declared finite 0-60 interval. The root must observe the same retained original process and return its actual exit code. Builtin `TimeoutError` means pending. Bool, awaitable and other non-integer returns are not exits. Observation errors remain failures while the worker keeps observing the same original process. Immediate pending/error callbacks are paced to avoid a busy loop.

Neither callback is a full-root-cleanup operation. The root's retirement deadline, including its declared default 30 seconds and configured maximum 60 seconds, remains independent of SDK observation timeouts. The SDK does not extend login expiry, reset or replace root retirement, repeat shutdown, kill a process/group, or release backing custody.

## Transfer and failure ownership

Binding validation and finite initialization encoding precede any worker or transfer. Three worker objects are retained on the carrier's preparation, started inactive, and cannot touch endpoints or callbacks until `transport.client` is assigned. That association is the sole transfer commit. The activation gate is released in a finally block; close can release the same gate after a caller interruption. There is no second ownership boolean.

Invalid binding or endpoint refusal leaves root ownership unchanged. A failure during inactive worker preparation retains the preparation on the carrier and prevents reuse of that carrier. Started inactive targets can only return without I/O or callbacks. Their bounded join attempts do not convert uncertain starts into a native cleanup claim. The root still owns the original untransferred endpoints and child and may retire them. No replacement provider is created.

After association, all I/O remains on the same retained workers/client. The writer emits the immutable initialization once, completes short writes only with the remaining bytes of that same frame, and rejects zero/invalid progress. A partial failure stops subsequent requests and retains failure; it does not replay a prefix, append cancellation to a failed frame or send another initialize. The pending queue has two request slots in addition to the active write. Cancellation has one separate flag and cannot block the caller on pipe I/O.

For an adopted client, `answer` returning means the exact bounded request was accepted by the writer, not that the provider accepted it. Only native `answered`/`selected`/other protocol events establish the provider outcome. A full writer queue is refused without automatically retrying. Prompt identity, frame size, event queue size, native selection validation and all parser rules are shared with the existing implementation.

The reader and writer each close only their own original endpoint after their own operation has ended. The other thread never closes a descriptor concurrently with its blocked operation. A close exception retains failure and leaves that endpoint unsettled. It is not blindly retried, because an ambiguously closed descriptor may have been reused. Root must retain the same objects/backing and diagnose uncertainty; neither a process exit nor `FileIO.closed` alone repairs that evidence gap.

## Bounded observation and independent root cleanup

`close(timeout=150.0)` retains the existing positive finite observation-bound contract. The adopted path starts its deadline before cancel/retirement dispatch. It waits for the actual original exit code, successful endpoint settlement and all reader/writer/monitor completion flags, then joins each actual worker using only the remaining time. It does not report join from a flag or direct-child exit alone. A worker cannot call close to join itself.

Timeout means pending ownership. The same close may be observed again; no new child, worker, retirement operation or shutdown is created. After all SDK/process settlement, a stored failure or nonzero exit raises `ProviderSetupError`. A returned native terminal may itself be `failed` or `cancelled`; it is not selection success. Full root process-domain/descendant cleanup and backing release still require the root's independent joined evidence.

An async root should call synchronous close on its retained worker task and shield that same task from an observer's cancellation. Root process/pipe retirement must make independent progress before it waits for SDK I/O; waiting for SDK close before performing the action that releases a blocked pipe would form a cycle. These APIs do not implement or attest the host's full async cleanup.

## Verification scope

The existing spawn tests remain byte-identical. Nineteen adoption regression methods use real original FileIO pipe endpoints and a root-created Python peer, not a native provider, model or private host implementation. They cover transfer interruption, exact initialization/answer/cancel frames, native errors and queue limits, short-write/no-progress handling, independent process retirement with pending writer/reader work, invalid process returns, callback failure, timeout bounds and cancelled observation.

Two private one-call helpers separate raw write/read operations. Deterministic test gates retain real endpoints and exercise worker/ownership boundaries; they do not claim a measured kernel pipe-full stall or full host cleanup. On hosts supporting `os.pipe2`, the fixture allocates original `pipe2(O_CLOEXEC)` endpoints. On macOS it explicitly uses original `os.pipe()` endpoints and checks both child and parent descriptors are blocking and noninheritable before the FileIO transfer. This is a fixture-only platform branch, not a production host fallback or fake pipe2. Mac results can qualify SDK FileIO/protocol/lifetime behavior, not a host's Linux pipe2 allocation or an installed host. Other unsupported platforms fail explicitly. The focused Darwin SDK run passed all 29 methods, including 10 unchanged legacy methods and 19 adoption methods. This does not establish Linux allocation or installed-host results.

Run both focused files with the selected Python/SDK environment and the checkout's `sdk/python/src` on PYTHONPATH:

```text
<reviewed-python> -m unittest discover -s <successor>/sdk/python/tests -p test_provider_setup.py -v
<reviewed-python> -m unittest discover -s <successor>/sdk/python/tests -p test_provider_adoption.py -v
```

Host integration must use the exact implemented source and retain its original process, carrier, workers and independent retirement evidence. SDK fixture success alone does not qualify full host cleanup.
