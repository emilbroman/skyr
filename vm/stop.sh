#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
STATE_DIR="$ROOT_DIR/.vm"

log() { echo "==> $*"; }

if [ ! -d "$STATE_DIR" ]; then
  log "No VM state directory found. Nothing to stop."
  exit 0
fi

stopped=0
for pidfile in "$STATE_DIR"/*.pid; do
  [ -f "$pidfile" ] || continue
  name="$(basename "$pidfile" .pid)"
  pid="$(cat "$pidfile")"
  if kill -0 "$pid" 2>/dev/null; then
    log "Stopping $name (PID $pid)..."
    kill "$pid" 2>/dev/null || true
    # Wait for process to exit
    for _ in $(seq 1 30); do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.5
    done
    # Force kill if still running
    if kill -0 "$pid" 2>/dev/null; then
      log "Force killing $name..."
      kill -9 "$pid" 2>/dev/null || true
    fi
    stopped=$((stopped + 1))
  fi
  rm -f "$pidfile"
done

if [ "$stopped" -eq 0 ]; then
  log "No running VMs found."
else
  log "Stopped $stopped VM(s)."
fi
