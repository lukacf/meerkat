# Experimental Agno function adapter

`meerkat_toolkit.adapters.agno` provides a small synchronous bridge from a host-selected Agno `Toolkit` to its actual allowlisted `Function.entrypoint` callables. It is an implementation step toward broad OSS reuse, not a completed integration library, an Agno agent runtime or evidence that an integration is adopted.

This component does not yet provide a documented ready-to-use supported host setup for its candidate integrations. Host composition and per-integration adoption evidence remain separate work.

The reviewed upstream source labels itself Agno 3.0.11 at revision `8c3d8ec52b4a13c410fca97d07fbd507790cf29a`. The local Python `agno` extra selects the registry artifact `agno==3.0.11` and `jsonschema==4.26.0`. A source revision and a published artifact with the same version label are distinct dependency identities; source verification alone does not establish their equivalence. The Toolkit distribution remains local and unpublished. Recheck the exact source/API and dependency closure before changing that profile.

## Host contract

The host supplies an already configured toolkit and an explicit mapping of selected function names to `read` or `write`. It owns account and credential selection, grants, dependency isolation, transport policy, toolkit connection and lifetime. The bridge does not instantiate toolkits, read environment credentials, call `connect()`/`close()`, retry, switch accounts, or register functions with Meerkat.

Treat the supplied toolkit as trusted executable host code. Its own constructor or methods can have ambient credentials, logging, globals and framework assumptions. This bridge cannot remove those behaviors by wrapping the function. Each integration still needs its actual source, dependency, auth/transport and useful-operation review before it becomes supported. Keep toolkit/client instances scoped to the selected account; do not retarget a borrowed instance between principals.

`has_side_effects=None` does not mean read-only. The host must classify every selected function. A host `read` selection conflicting with an explicit upstream write hint is rejected. Effect labels are local invocation information, not a grant or provider receipt.

## API

```python
from agno.tools.hackernews import HackerNewsTools
from meerkat_toolkit.adapters.agno import AgnoToolkitAdapter

# The host deliberately selects and configures this upstream implementation.
toolkit = HackerNewsTools(timeout=10)
adapter = AgnoToolkitAdapter(
    toolkit,
    functions={"get_top_hackernews_stories": "read"},
    max_argument_bytes=65_536,
    max_result_bytes=1_048_576,
)

# Description only: this does not call Hacker News or register native tools.
descriptions = adapter.describe()
```

Each description has `name`, `description`, `parameters` and the explicit host `effect`. `parameters` is the detached upstream JSON Schema after Agno's ordinary entrypoint processing. A private Function copy is processed for schema/hidden-parameter metadata and Agno's argument-validation wrapper. Invocation uses that processed callable, preserving the original underlying function and host receiver. The original host Function remains unchanged. This relies on the pinned Agno `model_copy(deep=True)` implementation, which preserves the entrypoint reference while copying parameter metadata.

After the host has authorized a concrete call, `adapter.invoke("get_top_hackernews_stories", {"num_stories": 2})` validates arguments and invokes that selected upstream entrypoint once. This call would perform the upstream HTTP operations outside the controlled test environment. The adapter does not perform an authorization decision or turn a function return into verified business completion.

## Supported direct-call profile

- Synchronous, non-generator functions with named keyword-compatible arguments and a complete object parameter schema.
- Plain JSON arguments and results: null, booleans, finite numbers, strings, lists and string-keyed dictionaries. Strings containing JSON stay strings; the adapter does not parse, normalize or interpret them as success/error envelopes.
- Upstream JSON Schema validation and Python signature binding before entry, followed by the upstream validation wrapper's argument conversion. This preserves JSON integer `2.0` conversion to Python `int` and supported structured Pydantic input conversion. Unknown function names cannot cause discovery or registration. External schema references are rejected; validation has no network schema resolver.
- Arguments and results are detached and bounded by encoded UTF-8 JSON bytes, with a depth/node guard. Oversized output is refused, never truncated. These checks bound what crosses the bridge, not memory or time already consumed by upstream execution.
- The declared schema remains the argument contract. For example, the reviewed PubMed `max_results` schema allows omission but excludes explicit null even though its Python annotation is optional. The bridge preserves that distinction.

Agno Agent, Team and `FunctionCall.execute` are not used for invocation. Inspection may load upstream type definitions; no agent or team is created. Toolkit instructions or function behavior that requires an Agno lifecycle is refused: active cache, hooks, confirmation, user input, external execution, approval, display/stop controls, injected runtime/media context and active framework instructions. Framework-owned parameters are rejected by reserved names, resolved metadata and schema/signature correspondence. Variadic, positional-only, asynchronous and streaming/generator functions are outside this profile.

The bridge does not define another conversion system. Agno's existing wrapper performs declared Python type conversion where its schema and callable support it; framework-owned objects and injected media remain excluded. A Function explicitly configured to skip entrypoint processing retains that upstream contract. Failed ordinary wrapper preparation is rejected rather than silently invoking an unvalidated raw callable. Custom Pydantic validators remain trusted upstream executable code and need review with the selected integration.

## Failure and interruption

`AgnoAdapterError` exposes `kind`, `effect`, `entered` and `write_outcome`. Its diagnostics retain no raw exception text, arguments or provider body. Error kinds distinguish invalid selection, unsupported function, unknown function, invalid arguments, entrypoint failure, invalid result and oversized result. `entered` means the processed invocation boundary was reached; a validation wrapper can still fail before the original function body. Write uncertainty is deliberately conservative at that boundary.

| Observation | Write outcome |
| --- | --- |
| Invalid arguments or another refusal before entry | `not_started` |
| Exception, unsupported result or excessive result after write entry | `unknown` |
| Read operation failure | `not_applicable` |

A return means only that the entrypoint returned a bounded value. Some upstream tools catch their own exceptions and return error text; that text is preserved. Their own logging is also outside the adapter's error redaction. Review those behaviors for each supported profile. Nothing automatically retries an unknown effect.

This initial profile is synchronous. It supplies no timer, worker thread, async cancellation or hard cleanup deadline. Process-level interruptions such as `KeyboardInterrupt` propagate. If the host interrupts execution after entry, it must retain an unknown write outcome unless it has separate authoritative evidence. A transport timeout can bound a particular upstream request only to the extent its implementation honors it.

## Verification scope

`tests/test_agno_adapter.py` executes the real pinned Agno Hacker News and PubMed entrypoints with a controlled `httpx.get` boundary. It checks actual upstream JSON/XML parsing, selected functions, supplied host configuration, schema handling, unchanged original callable and receiver identity, and exact preserved result values. Constructed real Agno Toolkit/Function cases cover refusal before entry, hooks/injection, account-instance separation, noncopyable borrowed clients, output limits, unknown writes, interruption and absence of automatic replay.

Those tests do not call live providers, authenticate accounts, install a supported host profile, establish native runtime readiness or complete product adoption. The observed dependency attempt for `brave-search` conflicts with this profile's HTTPX requirements, so Brave is not claimed as a supported or tested bridge integration. Its inventory entry remains a candidate with that prerequisite unresolved.

Run the focused suite in the dependency environment selected for this exact profile:

```sh
PYTHONPATH=sdk/python/src python -m pytest sdk/python/tests/test_agno_adapter.py -q
```

The coordinating task verified the same 15 tests and 30 subtests against a local wheel installed with the published Agno 3.0.11 and jsonschema 4.26.0 dependencies, outside the source checkout with `PYTHONPATH` removed. The installed adapter payload matches its reviewed source. The Agno Toolkit, Function, Hacker News and PubMed modules match the inspected source revision; other modules differ, so this is not a whole-package source-equivalence claim. The private record is `docs/verification/20261002-agno-adapter/installed-acceptance.json`. This installed component proof still does not establish consumer-host activation or integration adoption.
