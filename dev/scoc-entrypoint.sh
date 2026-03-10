#!/bin/sh
set -e

# Create a wrapper for crun that forces rootless mode
# This prevents crun from trying to set oom_score_adj which requires CAP_SYS_RESOURCE
cat > /usr/local/bin/crun-rootless <<'WRAPPER'
#!/bin/sh
exec /usr/bin/crun --rootless=true "$@"
WRAPPER
chmod +x /usr/local/bin/crun-rootless

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

# Wait for containerd socket to exist
while [ ! -S /run/containerd/containerd.sock ]; do
  sleep 0.1
done

# Wait for containerd to be ready to accept connections
# The socket file may exist before containerd is fully listening
echo "Waiting for containerd to be ready..."
until ctr version > /dev/null 2>&1; do
  sleep 0.2
done
echo "containerd is ready"

# Start SCOC conduit server
exec /usr/local/bin/scoc daemon \
  --node-name "${SCOC_NODE_NAME}" \
  --bind "${SCOC_BIND:-0.0.0.0:50054}" \
  --conduit-address "${SCOC_CONDUIT_ADDRESS}" \
  --orchestrator-address "${SCOC_ORCHESTRATOR_ADDRESS}" \
  --containerd-socket /run/containerd/containerd.sock \
  --cpu-millis "${SCOC_CPU_MILLIS:-4000}" \
  --memory-bytes "${SCOC_MEMORY_BYTES:-8589934592}" \
  --max-pods "${SCOC_MAX_PODS:-100}" \
  --ldb-brokers "${SCOC_LDB_BROKERS:-127.0.0.1:9092}"
