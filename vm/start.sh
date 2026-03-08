#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
STATE_DIR="$ROOT_DIR/.vm"
CACHE_DIR="$STATE_DIR/cache"

# Configuration (overridable via environment)
ALPINE_VERSION="${ALPINE_VERSION:-3.21}"
ALPINE_RELEASE="${ALPINE_RELEASE:-3.21.4}"
BUILDKIT_VERSION="${BUILDKIT_VERSION:-0.21.1}"
VM_MEMORY="${VM_MEMORY:-2G}"
VM_CPUS="${VM_CPUS:-2}"
VM_DISK_SIZE="${VM_DISK_SIZE:-4G}"

# Port mappings: host_port -> vm_port
BUILDKIT_HOST_PORT="${BUILDKIT_HOST_PORT:-1234}"
SCOC_BASE_PORT="${SCOC_BASE_PORT:-50061}"  # 50061, 50062, 50063

# Orchestrator address (plugin-std-container), reachable from VMs via QEMU gateway
ORCHESTRATOR_HOST_PORT="${ORCHESTRATOR_HOST_PORT:-50053}"

# Temporary directories to clean up
TMPDIRS=()
cleanup_tmpdirs() {
  for d in "${TMPDIRS[@]}"; do
    rm -rf "$d"
  done
}
trap cleanup_tmpdirs EXIT

make_tmpdir() {
  local d
  d=$(mktemp -d)
  TMPDIRS+=("$d")
  echo "$d"
}

log() { echo "==> $*"; }
err() { echo "ERROR: $*" >&2; exit 1; }

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64) echo "x86_64" ;;
    aarch64|arm64) echo "aarch64" ;;
    *) err "Unsupported architecture: $(uname -m)" ;;
  esac
}

ARCH="$(detect_arch)"

# Architecture-specific configuration
case "$ARCH" in
  x86_64)
    QEMU_BIN="qemu-system-x86_64"
    ALPINE_IMAGE_NAME="nocloud_alpine-${ALPINE_RELEASE}-x86_64-bios-cloudinit-r0.qcow2"
    BUILDKIT_ARCH="amd64"
    RUST_TARGET="x86_64-unknown-linux-musl"
    MUSL_BUILDER="clux/muslrust:stable"
    ;;
  aarch64)
    QEMU_BIN="qemu-system-aarch64"
    ALPINE_IMAGE_NAME="nocloud_alpine-${ALPINE_RELEASE}-aarch64-uefi-cloudinit-r0.qcow2"
    BUILDKIT_ARCH="arm64"
    RUST_TARGET="aarch64-unknown-linux-musl"
    MUSL_BUILDER="messense/rust-musl-cross:aarch64-musl"
    ;;
esac

ALPINE_IMAGE_URL="https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_VERSION}/releases/cloud/${ALPINE_IMAGE_NAME}"
BUILDKIT_URL="https://github.com/moby/buildkit/releases/download/v${BUILDKIT_VERSION}/buildkit-v${BUILDKIT_VERSION}.linux-${BUILDKIT_ARCH}.tar.gz"

# Build QEMU acceleration flags
detect_accel() {
  if [ "$(uname -s)" = "Darwin" ]; then
    echo "hvf"
  elif [ -e /dev/kvm ]; then
    echo "kvm"
  else
    echo "tcg"
  fi
}
QEMU_ACCEL="$(detect_accel)"

check_dependencies() {
  for cmd in "$QEMU_BIN" qemu-img mkisofs curl podman; do
    command -v "$cmd" >/dev/null 2>&1 || err "'$cmd' not found. Run 'nix develop' to enter the dev shell."
  done
}

check_existing_vms() {
  local running=0
  for pidfile in "$STATE_DIR"/*.pid; do
    [ -f "$pidfile" ] || continue
    if kill -0 "$(cat "$pidfile")" 2>/dev/null; then
      running=$((running + 1))
    else
      rm -f "$pidfile"
    fi
  done
  if [ "$running" -gt 0 ]; then
    err "VMs already running ($running found). Run 'make vms-down' first."
  fi
}

download_alpine_image() {
  local dest="$CACHE_DIR/$ALPINE_IMAGE_NAME"
  if [ -f "$dest" ]; then
    log "Alpine image already cached"
    return
  fi
  log "Downloading Alpine cloud image..."
  mkdir -p "$CACHE_DIR"
  if ! curl -fSL --progress-bar -o "$dest.tmp" "$ALPINE_IMAGE_URL"; then
    rm -f "$dest.tmp"
    err "Failed to download Alpine image from $ALPINE_IMAGE_URL
Check that the version exists. Override with ALPINE_RELEASE=x.y.z
Browse available images at: https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_VERSION}/releases/cloud/"
  fi
  mv "$dest.tmp" "$dest"
}

download_buildkit() {
  local dest="$CACHE_DIR/buildkit-v${BUILDKIT_VERSION}.linux-${BUILDKIT_ARCH}.tar.gz"
  if [ -f "$dest" ]; then
    log "BuildKit already cached"
    return
  fi
  log "Downloading BuildKit v${BUILDKIT_VERSION}..."
  mkdir -p "$CACHE_DIR"
  if ! curl -fSL --progress-bar -o "$dest.tmp" "$BUILDKIT_URL"; then
    rm -f "$dest.tmp"
    err "Failed to download BuildKit from $BUILDKIT_URL
Override with BUILDKIT_VERSION=x.y.z"
  fi
  mv "$dest.tmp" "$dest"
}

build_scoc_binary() {
  local dest="$STATE_DIR/scoc"
  if [ -f "$dest" ]; then
    log "SCOC binary already built (delete .vm/scoc to rebuild)"
    return
  fi
  log "Building SCOC binary for $RUST_TARGET (this may take a while)..."
  mkdir -p "$STATE_DIR"

  if [ "$ARCH" = "x86_64" ]; then
    podman run --rm \
      -v "$ROOT_DIR":/volume:Z \
      -w /volume \
      "$MUSL_BUILDER" \
      cargo build --release -p scoc --target "$RUST_TARGET"
    cp "$ROOT_DIR/target/${RUST_TARGET}/release/scoc" "$dest"
  else
    podman run --rm \
      -v "$ROOT_DIR":/volume:Z \
      -w /volume \
      "$MUSL_BUILDER" \
      cargo build --release -p scoc
    cp "$ROOT_DIR/target/release/scoc" "$dest"
  fi
  log "SCOC binary built successfully"
}

create_seed_iso() {
  local name="$1"
  local user_data_file="$2"
  local dest="$STATE_DIR/${name}-seed.iso"

  rm -f "$dest"

  local tmpdir
  tmpdir="$(make_tmpdir)"

  cp "$user_data_file" "$tmpdir/user-data"
  cat > "$tmpdir/meta-data" <<EOF
instance-id: ${name}
local-hostname: ${name}
EOF

  mkisofs -output "$dest" -volid cidata -joliet -rational-rock \
    "$tmpdir/user-data" "$tmpdir/meta-data" 2>/dev/null
}

create_payload_iso() {
  local name="$1"
  local payload_dir="$2"
  local dest="$STATE_DIR/${name}-payload.iso"

  rm -f "$dest"
  mkisofs -output "$dest" -volid PAYLOAD -joliet -rational-rock \
    "$payload_dir" 2>/dev/null
}

create_vm_disk() {
  local name="$1"
  local dest="$STATE_DIR/${name}.qcow2"

  rm -f "$dest"
  qemu-img create -f qcow2 -b "$CACHE_DIR/$ALPINE_IMAGE_NAME" -F qcow2 \
    "$dest" "$VM_DISK_SIZE" >/dev/null
}

# Find UEFI firmware for aarch64
find_uefi_firmware() {
  if [ -n "${OVMF_FD:-}" ]; then
    echo "$OVMF_FD"
    return
  fi
  for candidate in \
    /usr/share/AAVMF/AAVMF_CODE.fd \
    /usr/share/qemu-efi-aarch64/QEMU_EFI.fd \
    /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
    /usr/share/edk2/aarch64/QEMU_EFI.fd; do
    if [ -f "$candidate" ]; then
      echo "$candidate"
      return
    fi
  done
  # Try to find it relative to the qemu binary (nix store)
  local qemu_dir
  qemu_dir="$(dirname "$(dirname "$(command -v qemu-system-aarch64)")")"
  for candidate in \
    "$qemu_dir/share/qemu/edk2-aarch64-code.fd" \
    "$qemu_dir/share/OVMF/AAVMF_CODE.fd"; do
    if [ -f "$candidate" ]; then
      echo "$candidate"
      return
    fi
  done
  err "Could not find UEFI firmware for aarch64. Set OVMF_FD=/path/to/firmware.fd"
}

start_vm() {
  local name="$1"
  local port_forwards="$2"
  local payload_iso="${3:-}"

  local disk="$STATE_DIR/${name}.qcow2"
  local seed="$STATE_DIR/${name}-seed.iso"
  local pidfile="$STATE_DIR/${name}.pid"
  local logfile="$STATE_DIR/${name}.log"

  rm -f "$pidfile" "$logfile"

  local cmd=("$QEMU_BIN")

  # Machine type and acceleration
  case "$ARCH" in
    x86_64)
      cmd+=(-machine q35 -accel "$QEMU_ACCEL")
      ;;
    aarch64)
      cmd+=(-machine virt -accel "$QEMU_ACCEL" -cpu max)
      local firmware
      firmware="$(find_uefi_firmware)"
      cmd+=(-bios "$firmware")
      ;;
  esac

  # If primary accelerator fails, QEMU falls back automatically with -accel
  # Add TCG as fallback if not already the primary
  if [ "$QEMU_ACCEL" != "tcg" ]; then
    cmd+=(-accel tcg)
  fi

  cmd+=(
    -m "$VM_MEMORY"
    -smp "$VM_CPUS"
    -nographic
    -drive "file=${disk},format=qcow2,if=virtio"
    -drive "file=${seed},format=raw,media=cdrom"
    -netdev "user,id=net0,${port_forwards}"
    -device "virtio-net-pci,netdev=net0"
    -pidfile "$pidfile"
    -serial "file:${logfile}"
  )

  # Add payload drive if present
  if [ -n "$payload_iso" ]; then
    cmd+=(-drive "file=${payload_iso},format=raw,media=cdrom")
  fi

  cmd+=(-daemonize)

  log "Starting VM: $name"
  "${cmd[@]}"
}

wait_for_port() {
  local port="$1"
  local name="$2"
  local timeout="${3:-120}"
  local elapsed=0

  log "Waiting for $name on port $port..."
  while ! bash -c "echo >/dev/tcp/127.0.0.1/$port" 2>/dev/null; do
    sleep 2
    elapsed=$((elapsed + 2))
    if [ "$elapsed" -ge "$timeout" ]; then
      echo ""
      echo "--- Last 20 lines of $name log ---"
      tail -20 "$STATE_DIR/${name}.log" 2>/dev/null || true
      echo "---"
      err "Timeout waiting for $name on port $port (${timeout}s)"
    fi
  done
  log "$name is ready on port $port"
}

setup_buildkit_vm() {
  log "Preparing BuildKit VM..."

  local payload_dir
  payload_dir="$(make_tmpdir)"
  tar xzf "$CACHE_DIR/buildkit-v${BUILDKIT_VERSION}.linux-${BUILDKIT_ARCH}.tar.gz" -C "$payload_dir"

  create_payload_iso "buildkit" "$payload_dir"
  create_seed_iso "buildkit" "$SCRIPT_DIR/cloud-init/buildkit-user-data"
  create_vm_disk "buildkit"
  start_vm "buildkit" \
    "hostfwd=tcp::${BUILDKIT_HOST_PORT}-:1234" \
    "$STATE_DIR/buildkit-payload.iso"
}

setup_scoc_vm() {
  local index="$1"
  local name="scoc-${index}"
  local host_port=$((SCOC_BASE_PORT + index - 1))

  log "Preparing SCOC VM: $name (host port: $host_port)..."

  local payload_dir
  payload_dir="$(make_tmpdir)"

  cp "$STATE_DIR/scoc" "$payload_dir/scoc"
  chmod +x "$payload_dir/scoc"
  cp "$SCRIPT_DIR/containerd-vm.toml" "$payload_dir/containerd-config.toml"

  cat > "$payload_dir/scoc-env" <<EOF
SCOC_NODE_NAME=${name}
SCOC_BIND=0.0.0.0:50054
SCOC_CONDUIT_ADDRESS=http://host.containers.internal:${host_port}
SCOC_ORCHESTRATOR_ADDRESS=http://10.0.2.2:${ORCHESTRATOR_HOST_PORT}
SCOC_CPU_MILLIS=4000
SCOC_MEMORY_BYTES=8589934592
SCOC_MAX_PODS=100
EOF

  create_payload_iso "$name" "$payload_dir"
  create_seed_iso "$name" "$SCRIPT_DIR/cloud-init/scoc-user-data"
  create_vm_disk "$name"
  start_vm "$name" \
    "hostfwd=tcp::${host_port}-:50054" \
    "$STATE_DIR/${name}-payload.iso"
}

main() {
  check_dependencies
  mkdir -p "$STATE_DIR" "$CACHE_DIR"
  check_existing_vms

  # Download prerequisites (can be slow on first run)
  download_alpine_image
  download_buildkit
  build_scoc_binary

  # Prepare and start VMs
  setup_buildkit_vm
  for i in 1 2 3; do
    setup_scoc_vm "$i"
  done

  # Wait for all services to become reachable
  wait_for_port "$BUILDKIT_HOST_PORT" "buildkit" 180
  for i in 1 2 3; do
    wait_for_port $((SCOC_BASE_PORT + i - 1)) "scoc-${i}" 180
  done

  echo ""
  log "All VMs are running!"
  echo ""
  echo "  BuildKit:  tcp://127.0.0.1:${BUILDKIT_HOST_PORT}"
  echo "  SCOC-1:    http://127.0.0.1:${SCOC_BASE_PORT}"
  echo "  SCOC-2:    http://127.0.0.1:$((SCOC_BASE_PORT + 1))"
  echo "  SCOC-3:    http://127.0.0.1:$((SCOC_BASE_PORT + 2))"
  echo ""
  echo "From podman containers, VMs are reachable via host.containers.internal"
  echo ""
  echo "  make compose-vm   # start podman services configured for VM mode"
  echo "  make vms-down     # stop all VMs"
}

main "$@"
