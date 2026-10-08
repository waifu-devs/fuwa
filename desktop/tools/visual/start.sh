#!/usr/bin/env bash
# Starts the seeded instance on :18080 if it isn't running, seeding it the first time.
# RESET=1 wipes it and seeds again. The server binary is $FUWA_BIN, else the repo's
# target/debug/fuwa, built with the embedded web app (see README.md).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
vis=${FUWA_VISUAL_DIR:-/tmp/fuwa-visual}
bin=${FUWA_BIN:-$repo/target/debug/fuwa}
port=${FUWA_VISUAL_PORT:-18080}
if [ "${RESET:-}" = 1 ]; then
  [ -f "$vis/server.pid" ] && kill "$(cat "$vis/server.pid")" 2>/dev/null || true
  sleep 1; rm -rf "$vis/data" "$vis/state.json" "$vis/seeded"
fi
mkdir -p "$vis/data" "$vis/out"
if ! curl -sf "http://127.0.0.1:$port/healthz" >/dev/null 2>&1; then
  [ -x "$bin" ] || { echo "no server at $bin: build it (README.md) or set FUWA_BIN"; exit 1; }
  FUWA_DATA_PATH=$vis/data FUWA_PORT=$port FUWA_PUBLIC_URL=http://127.0.0.1:$port FUWA_TELEMETRY=off FUWA_UPDATE_CHECK=off \
    nohup "$bin" > "$vis/server.log" 2>&1 &
  echo $! > "$vis/server.pid"
  for _ in $(seq 1 60); do curl -sf "http://127.0.0.1:$port/healthz" >/dev/null 2>&1 && break; sleep 0.5; done
fi
[ -f "$vis/state.json" ] && [ -f "$vis/seeded" ] || { FUWA_VISUAL_PORT=$port node "$here/seed.mjs" && touch "$vis/seeded"; }
echo ready
