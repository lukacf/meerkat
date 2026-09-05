Reported by the HomeCore agent while moving HSNS (camera analysis service) from meerkat-sdk 0.7.10 + rkat-rpc 0.7.11 to the 0.8.33 pair (2026-09-04, agent bus).

**1. Python SDK never drains rkat-rpc stderr.** `sdks/python/meerkat/client.py` (around line 699) spawns rkat-rpc with `stderr=asyncio.subprocess.PIPE` and never reads it. When the child exits at startup (here: `Store(UnledgeredDomainObjects ...)` on a legacy realm), the caller only sees `MeerkatError CONNECTION_CLOSED 'rkat-rpc process closed'` and the real reason is invisible. A chatty child under `RUST_LOG` can also block on a full 64 KB pipe. Ask: drain stderr continuously (bounded ring buffer) and attach its tail to the CONNECTION_CLOSED error.

**2. Python SDK `close()` replaces the original error with `ProcessLookupError`.** `client.py` (around line 786) calls `self._process.terminate()` unguarded. When the child has already exited, `terminate()` raises `ProcessLookupError` out of the caller's `finally`, replacing the `MeerkatError` that explained the failure. HSNS logged `Meerkat analysis failed: ProcessLookupError()` for a day of jobs. Ask: treat a vanished child as already closed (guard `terminate()`/`kill()` with `ProcessLookupError`, check `returncode` first).

**3. rkat-rpc renders startup errors with Debug, hiding the remedy.** `meerkat-rpc/src/main.rs` `fn main() -> Result<(), Box<dyn std::error::Error>>` makes Rust print `Error: Store(UnledgeredDomainObjects { domain: "session-store", objects: [...], bridgeable: CatalogAuthenticated })`. The Display form of that error carries the remedy sentence naming `storage migrate --apply --bridge-pre-0-8-10`, so the operator was never told the fix. Ask: print the Display chain (error and sources) to stderr and exit 1; check rkat-rest and rkat-mcp for the same pattern.

Also noted (no action): 0.7.x rkat-rpc catalogs have no profile for gemini-3.8-flash and refuse inline video with `provider_model_profile_missing`; 0.8.33 has it.

Target: 0.8.34.
