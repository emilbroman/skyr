#!/bin/sh
set -e

# Setup cgroup delegation for cgroupv2
# The container gets its own cgroup namespace, so we need to:
# 1. Create a child cgroup for our services
# 2. Move our processes there
# 3. Enable all controllers for k8s.io hierarchy
if [ -f /sys/fs/cgroup/cgroup.controllers ]; then
  # Create cgroup for our init processes
  mkdir -p /sys/fs/cgroup/init.scope

  # Move current process to init.scope (this allows enabling controllers at root)
  echo $$ > /sys/fs/cgroup/init.scope/cgroup.procs 2>/dev/null || true

  # Enable all available controllers at the root level
  for controller in $(cat /sys/fs/cgroup/cgroup.controllers); do
    echo "+${controller}" > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null || true
  done

  # Create k8s.io cgroup and enable controllers
  mkdir -p /sys/fs/cgroup/k8s.io
  for controller in $(cat /sys/fs/cgroup/k8s.io/cgroup.controllers); do
    echo "+${controller}" > /sys/fs/cgroup/k8s.io/cgroup.subtree_control 2>/dev/null || true
  done
fi

# Start containerd in background
containerd &

# Wait for containerd socket
while [ ! -S /run/containerd/containerd.sock ]; do
  sleep 0.1
done

# Start SCOC agent
exec /usr/local/bin/scoc daemon \
  --node-name "${SCOC_NODE_NAME}" \
  --plugin-addr "${SCOC_PLUGIN_ADDR}" \
  --containerd-socket /run/containerd/containerd.sock
