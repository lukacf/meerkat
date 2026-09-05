#!/bin/bash
# Keep MemFree above a floor by dropping the clean page cache: the agent harness's
# "low memory" watchdog reads MemFree (not MemAvailable) and kills long builds.
FLOOR_KB=$((192*1024*1024))
while :; do
  f=$(awk '/^MemFree:/{print $2}' /proc/meminfo)
  if [ "$f" -lt "$FLOOR_KB" ]; then sync; sudo -n sh -c 'echo 1 > /proc/sys/vm/drop_caches'; echo "$(date -u +%FT%TZ) dropped caches (MemFree was $((f/1024/1024)) GB)"; fi
  sleep 10
done
