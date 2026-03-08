.PHONY: image deps compose dev vms vms-down

image:
	podman build -t skyr:latest -t localhost/skyr:latest .

deps:
	podman compose up -d scylla rabbitmq redis redpanda minio oci-registry
	@echo "Waiting for scylla to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr_scylla_1 2>/dev/null)" != "healthy" ]; do sleep 2; done
	@echo "Waiting for rabbitmq to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr_rabbitmq_1 2>/dev/null)" != "healthy" ]; do sleep 2; done

vms:
	vm/start.sh

vms-down:
	vm/stop.sh

compose: image deps vms
	podman compose up \
		api scs de rte-0 rte-1 rte-2 \
		plugin-std-random plugin-std-artifact plugin-std-container

dev:
	nix develop -c cargo watch -s 'set -e; \
		cargo run -p plugin_std_random -- --bind tcp://127.0.0.1:50051 & plugin_random_pid=$$!; \
		cargo run -p plugin_std_artifact -- --bind tcp://127.0.0.1:50052 --adb-endpoint-url http://127.0.0.1:9000 --adb-presign-endpoint-url http://127.0.0.1:9000 --adb-bucket skyr-artifacts --adb-access-key-id minioadmin --adb-secret-access-key minioadmin & plugin_artifact_pid=$$!; \
		cargo run -p plugin_std_container -- --bind 127.0.0.1:50053 --node-registry-hostname 127.0.0.1 --buildkit-addr tcp://127.0.0.1:1234 --registry-url http://127.0.0.1:5000 & plugin_container_pid=$$!; \
		cargo run -p api -- --host 127.0.0.1 --port 8080 --adb-endpoint-url http://127.0.0.1:9000 --adb-presign-endpoint-url http://127.0.0.1:9000 --adb-bucket skyr-artifacts --adb-access-key-id minioadmin --adb-secret-access-key minioadmin --challenge-salt local-dev-challenge-salt & api_pid=$$!; \
		cargo run -p scs -- daemon --address 127.0.0.1:2222 --key host.pem & scs_pid=$$!; \
		cargo run -p de -- daemon & de_pid=$$!; \
		cargo run -p rte -- daemon --plugin Std/Random@tcp://127.0.0.1:50051 --plugin Std/Artifact@tcp://127.0.0.1:50052 --plugin Std/Container@tcp://127.0.0.1:50053 --worker-index 0 --worker-count 3 --local-workers 3 & rte_pid=$$!; \
		trap "kill $$plugin_random_pid $$plugin_artifact_pid $$plugin_container_pid $$api_pid $$scs_pid $$de_pid $$rte_pid 2>/dev/null || true" EXIT INT TERM; \
		wait'
