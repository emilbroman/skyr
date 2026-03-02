#!/bin/sh
set -e

# Start containerd in background
containerd &

# Wait for containerd socket
while [ ! -S /run/containerd/containerd.sock ]; do
  sleep 0.1
done

# Start SCOC agent (placeholder - will be enabled in Phase 1)
# exec /usr/local/bin/scoc \
#   --node-name "${SCOC_NODE_NAME}" \
#   --plugin-addr "${SCOC_PLUGIN_ADDR}" \
#   --containerd-socket /run/containerd/containerd.sock

# For now, just keep containerd running
wait
