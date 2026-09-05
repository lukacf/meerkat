---
name: harness-low-memory-watchdog-memfree
description: On the release VM the agent harness's "low memory" watchdog kills long background builds because it reads MemFree (23 GB with 700 GB of page cache) not MemAvailable; a cache-dropping keeper loop is the workaround
metadata:
  type: project
---

Workers on meerkat-dev (731 GB RAM) repeatedly had cargo/nextest background tasks killed with a "low on memory" reason on 2026-09-05 while `MemAvailable` was 650-730 GB. `/proc/meminfo` showed `MemFree` around 23 GB with ~690 GB in clean page cache, so the harness heuristic is evidently keyed on MemFree.

Workaround installed 2026-09-05 03:12Z: `/tmp/rb/memfree-keeper.sh` (nohup, setsid) drops the clean page cache (`sudo sh -c 'echo 1 > /proc/sys/vm/drop_caches'`, passwordless sudo works) whenever MemFree falls under 96 GB, every 45 s; log at `/tmp/rb/memfree-keeper.log`. Workers can also run long commands as detached processes to escape the watchdog.

**Why:** three separate workers lost 10-20 minute build chains to the heuristic (dhealth, dloop-mobkit, dsdk), and a rustc incremental ICE followed one of the kills.

**How to apply:** if a worker reports a "low memory" kill, check `pgrep -f memfree-keeper` and restart the keeper if it is gone; do not treat the kill as a real OOM (check `MemAvailable`). See [[vm-disk-pressure-build-caches]].


UPDATE 2026-09-05 06:03Z: a worker's cargo run was killed by the same watchdog while MemFree was 277 GB and the keeper was alive, so the heuristic is not MemFree alone (it may look at the task's own RSS during rustc compilation). The reliable workaround is to run long cargo/nextest chains DETACHED (`setsid nohup ... &`) with a light waiter polling the log; the keeper stays as a secondary measure.
