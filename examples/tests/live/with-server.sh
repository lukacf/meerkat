#!/usr/bin/env bash
# Start a server, wait until a URL answers, run a client command, stop the server.
#
#   with-server.sh <wait-url> <server command...> -- <client command...>
#
# The exit status is the client's. The server runs in its own process group and
# the whole group is stopped afterwards, also when the client fails.
set -u

wait_url="$1"; shift
server=()
while [[ $# -gt 0 && "$1" != "--" ]]; do server+=("$1"); shift; done
[[ $# -gt 0 ]] || { echo "with-server.sh: no client command after --" >&2; exit 2; }
shift

setsid "${server[@]}" < /dev/null &
server_pid=$!
stop_server() {
  kill -TERM -- "-$server_pid" 2>/dev/null || kill -TERM "$server_pid" 2>/dev/null
  wait "$server_pid" 2>/dev/null
}
trap stop_server EXIT

for _ in $(seq 1 120); do
  if curl -s -o /dev/null -m 2 "$wait_url"; then break; fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "with-server.sh: the server ended before $wait_url answered" >&2
    exit 1
  fi
  sleep 0.5
done

"$@"
